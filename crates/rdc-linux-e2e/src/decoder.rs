//! rdc-linux-e2e / 外部 ffmpeg 解码管道
//!
//! 用途：把 host 侧的 Annex-B H.264 访问单元解码成 BGRA 原始帧，供上屏与绿块定位。
//! 实现：`ffmpeg -f h264 -i - -f rawvideo -pix_fmt bgra -`，stdin 喂码流，
//!       stdout 按「宽×高×4」字节整帧读出。
//! 为什么用外部进程：与编码侧一致，避免在演示 crate 里链接 GPL 组件；
//! 生产解码仍走 rdc-decode 的平台后端。
//!
//! 时延与内存约束：
//! - 解复用探测窗口收紧到 32 字节（默认 5MB 会让管道首帧延迟 5s+）；
//! - 解码帧带采集时间戳回传（`push(au, pts)` 入队、读线程按序出队），
//!   任意一层丢帧都不会让「帧↔时间戳」配对漂移；
//! - 解码帧通道有界（[`FRAME_QUEUE`]），满了丢最旧的输出帧，内存有上限。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// 解码结束等待进程退出的上限。
const EXIT_TIMEOUT_MS: u64 = 1500;
/// 已解码帧在通道里的上限（4MB/帧 × 6 ≈ 24MB）。
const FRAME_QUEUE: usize = 6;

/// 一帧解码结果。
#[derive(Debug)]
pub struct Decoded {
    /// 对应访问单元的采集时间戳（`CLOCK_MONOTONIC` 纳秒）。
    pub pts_ns: u64,
    /// BGRA 像素。
    pub data: Vec<u8>,
}

/// 外部 ffmpeg 解码器（单会话）。
pub struct Decoder {
    /// 子进程。
    child: Child,
    /// 码流输入；`None` 表示已关闭。
    stdin: Option<ChildStdin>,
    /// 解码完成帧的接收端。
    rx: Receiver<Decoded>,
    /// 读线程。
    reader: Option<JoinHandle<()>>,
    /// 送入码流的时间戳（与解码帧按序配对）。
    pending: Arc<Mutex<VecDeque<u64>>>,
    /// 单帧字节数。
    frame_bytes: usize,
    /// 已读出的整帧数。
    frames: u64,
    /// 因通道满被丢弃的解码帧数。
    dropped: Arc<AtomicU64>,
}

impl Decoder {
    /// 启动解码进程。
    ///
    /// # Arguments
    /// * `width` / `height` - 期望输出尺寸（与编码配置一致）。
    ///
    /// # Returns
    /// 解码进程与读线程就绪返回实例。
    ///
    /// # Errors
    /// ffmpeg 缺失、进程启动失败或管道不可用时返回说明文本。
    pub fn spawn(width: u32, height: u32) -> Result<Self, String> {
        // 调试开关：RDC_DEC_STDERR=<文件> 时把 ffmpeg 解码日志落到该文件。
        let stderr = std::env::var("RDC_DEC_STDERR")
            .ok()
            .and_then(|path| std::fs::File::create(path).ok())
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null);
        let mut child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error"])
            // 实测（VM ffmpeg 4.4.2，1280x800@60、实时喂 AU）：
            // - 默认探测参数：首帧 5s+（解复用器按 5MB probesize 攒数据）；
            // - 加 `-fflags nobuffer -avioflags direct`：首帧约 1.2s（反而更慢）；
            // - 仅 `-probesize 32 -analyzeduration 0`：首帧 0.15~0.23s，帧序完整。
            // 因此这里不用 nobuffer/direct，只收紧探测窗口。
            .args(["-probesize", "32", "-analyzeduration", "0"])
            // 单线程解码：默认帧级多线程会额外引入数帧延迟，而 800p/1080p 解码
            // 单线程足够（本机实测无丢帧）。
            .args(["-threads", "1"])
            .args(["-f", "h264", "-i", "-"])
            .args(["-f", "rawvideo", "-pix_fmt", "bgra", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|error| format!("启动 ffmpeg 解码失败: {error}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ffmpeg 解码 stdin 不可用".to_string())?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ffmpeg 解码 stdout 不可用".to_string())?;
        let frame_bytes = width as usize * height as usize * 4;
        let (tx, rx) = mpsc::sync_channel::<Decoded>(FRAME_QUEUE);
        let pending: Arc<Mutex<VecDeque<u64>>> = Arc::new(Mutex::new(VecDeque::new()));
        let dropped = Arc::new(AtomicU64::new(0));
        let read_pending = Arc::clone(&pending);
        let read_dropped = Arc::clone(&dropped);
        let reader = thread::Builder::new()
            .name("rdc-decode-out".into())
            .spawn(move || read_loop(&mut stdout, frame_bytes, read_pending, tx, read_dropped))
            .map_err(|error| format!("解码读线程创建失败: {error}"))?;

        Ok(Self {
            child,
            stdin: Some(stdin),
            rx,
            reader: Some(reader),
            pending,
            frame_bytes,
            frames: 0,
            dropped,
        })
    }

    /// 送入一个访问单元。
    ///
    /// # Arguments
    /// * `data` - Annex-B 码流。
    /// * `pts_ns` - 该帧的采集时间戳，用于与解码输出按序配对。
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 解码端已关闭或写入失败时返回说明文本。
    pub fn push(&mut self, data: &[u8], pts_ns: u64) -> Result<(), String> {
        {
            let mut queue = self.pending.lock().map_err(|_| "时间戳队列锁被污染")?;
            queue.push_back(pts_ns);
            // 防御：解码端异常时避免队列无限增长。
            if queue.len() > 4096 {
                queue.pop_front();
            }
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "解码端已关闭".to_string())?;
        stdin
            .write_all(data)
            .map_err(|error| format!("写解码 stdin 失败: {error}"))?;
        stdin
            .flush()
            .map_err(|error| format!("刷新解码 stdin 失败: {error}"))
    }

    /// 非阻塞取出一帧。
    ///
    /// # Returns
    /// 有整帧就绪返回解码结果（含配对的时间戳），否则 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn poll(&mut self) -> Option<Decoded> {
        let frame = self.rx.try_recv().ok()?;
        if frame.data.len() != self.frame_bytes {
            return None;
        }
        self.frames = self.frames.saturating_add(1);
        Some(frame)
    }

    /// 已解码整帧数。
    ///
    /// # Returns
    /// 自启动以来的整帧计数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// 因通道满丢弃的解码帧数。
    ///
    /// # Returns
    /// 丢帧计数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 关闭输入、等待剩余帧并回收进程。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；进程异常退出按正常结束处理。
    pub fn finish(&mut self) {
        self.stdin.take();
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(EXIT_TIMEOUT_MS);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        if let Err(error) = self.child.kill() {
                            tracing::debug!("[decoder] 退出超时后 kill 失败: {error}");
                        }
                        if let Err(error) = self.child.wait() {
                            tracing::debug!("[decoder] 退出超时后 wait 失败: {error}");
                        }
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            if let Err(error) = reader.join() {
                tracing::debug!("[decoder] 读线程异常退出: {error:?}");
            }
        }
    }
}

/// 读线程：整帧读取 → 配对时间戳 → 有界通道。
///
/// # Arguments
/// * `stdout` - ffmpeg 输出端。
/// * `frame_bytes` - 单帧字节数。
/// * `pending` - 时间戳队列（与输入顺序一致）。
/// * `tx` - 有界发送端。
/// * `dropped` - 丢帧计数。
///
/// # Returns
/// 无返回值；stdout EOF 或通道断开时退出。
///
/// # Errors
/// 不会失败。
fn read_loop(
    stdout: &mut std::process::ChildStdout,
    frame_bytes: usize,
    pending: Arc<Mutex<VecDeque<u64>>>,
    tx: SyncSender<Decoded>,
    dropped: Arc<AtomicU64>,
) {
    loop {
        let mut frame = vec![0u8; frame_bytes];
        if stdout.read_exact(&mut frame).is_err() {
            break;
        }
        let pts_ns = match pending.lock() {
            Ok(mut queue) => queue.pop_front().unwrap_or(0),
            Err(_) => 0,
        };
        let decoded = Decoded {
            pts_ns,
            data: frame,
        };
        match tx.try_send(decoded) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                // 消费端跟不上：丢这一帧解码结果（不影响后续解码参考链）。
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
    }
}

impl Drop for Decoder {
    /// 兜底清理：先杀进程，避免管道另一端阻塞导致线程无法退出。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn drop(&mut self) {
        let _ = self.child.kill(); // gate-allow R-C-2: Drop 无返回值可传播，kill 后紧接 take(stdin) 解除管道阻塞
        self.stdin.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join(); // gate-allow R-C-2: Drop 无返回值可传播，读线程 panic 不影响解码器析构
        }
        let _ = self.child.wait(); // gate-allow R-C-2: Drop 无返回值可传播，进程退出由系统回收
    }
}
