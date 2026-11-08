//! rdc-codec / Linux 软件编码后端（外部 ffmpeg + libx264）
//!
//! 层级：L3 编解码层 · 平台适配
//! 用途：在没有 GPU/VAAPI 的 Linux 环境里提供可用的 H.264 软件编码，
//!       让采集→编码→解码→上屏的链路先在 CPU 上跑通（S8-2）。
//! 调用方：rdc-codec::platform::create_encoder、rdc-cli 的 S8-2 示例
//!
//! # 许可与分发边界（重要）
//!
//! 本模块**不链接也不打包** libx264 / ffmpeg，只在运行期调用系统里的 `ffmpeg`
//! 可执行文件，输入原始 BGRA、输出 Annex-B 码流。libx264 是 **GPL** 许可：
//! - 该后端只作为**开发期可选**能力，发布包不包含 ffmpeg/libx264；
//! - 若未来要闭源分发，应替换为 openh264（BSD）或自研编码器；
//! - 系统没有 ffmpeg 或缺少 libx264 时，[`VideoEncoder::probe`] 返回空列表，
//!   [`VideoEncoder::configure`] 返回 `HardwareUnsupported`，不做静默降级。
//!
//! # 实现说明
//!
//! - 帧提交走无界通道送到写线程，`push` 本身不阻塞（符合 trait 契约）。
//! - ffmpeg 进程输出按 AUD（NAL type 9）切分访问单元；x264 参数里开 `aud=1`。
//! - 每个访问单元与提交顺序上的时间戳 FIFO 配对，保证 `pts_ns` 不错位。
//! - 外部管道无法对单帧强制 IDR（ffmpeg CLI 不暴露该能力）；关键帧由
//!   `keyint=1s` 与首帧 IDR 保证，`request_keyframe` 只置位不生效，见方法注释。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_runtime::PooledBuffer;

use crate::encoder::{
    CodecKind, EncodedPacket, EncoderCapability, EncoderCaps, ProbeReport, VideoChroma,
    VideoConfig, VideoEncoder,
};
use crate::error::CodecError;
use crate::metrics::{EncodeSnapshot, EncodeStats};

// =====** 常量 =====**

/// 后端标识，写入日志与实测报告。
const BACKEND: &str = "linux-ffmpeg-x264";

/// 单边最大分辨率（4096×4096 为 x264 的通用上限，本环境实测到 1080p）。
const MAX_DIM: u32 = 4096;

/// 冲刷时等待 ffmpeg 退出的上限；超时直接 kill，避免会话结束卡住。
const FLUSH_TIMEOUT: Duration = Duration::from_millis(1500);

/// 读线程单次读取块大小。
const READ_CHUNK: usize = 16 * 1024;

// =====** 能力探测 =====**

/// 检测系统 ffmpeg 是否带 libx264 编码器。
///
/// # Returns
/// 可用返回 `true`；ffmpeg 缺失或编码器列表里没有 `libx264` 返回 `false`。
///
/// # Errors
/// 不会失败；命令执行失败按不可用处理。
fn ffmpeg_has_libx264() -> bool {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-encoders"])
        .output();
    let Ok(output) = output else {
        return false;
    };
    // 部分发行版的 ffmpeg 把 -encoders 输出写到 stderr。
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text.contains("libx264")
}

/// 构造本平台的能力报告。
///
/// # Returns
/// 软件编码器可用时 `software` 含一条 H.264 记录，否则 `error` 说明原因。
///
/// # Errors
/// 不会失败。
pub fn probe_report() -> ProbeReport {
    if !ffmpeg_has_libx264() {
        return ProbeReport {
            hardware_available: false,
            hardware: Vec::new(),
            software: Vec::new(),
            error: Some("未找到 ffmpeg 或该 ffmpeg 未编译 libx264".into()),
        };
    }
    ProbeReport {
        hardware_available: false,
        hardware: Vec::new(),
        software: vec![EncoderCapability {
            name: "ffmpeg/libx264（外部进程，GPL，开发期可选）".into(),
            hardware: false,
            codec: CodecKind::H264,
            max_width: MAX_DIM,
            max_height: MAX_DIM,
            max_fps: 240,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            min_bitrate_bps: 100_000,
            max_bitrate_bps: 50_000_000,
            runtime_reconfigure: false,
            note: "能力来自系统 ffmpeg -encoders；未实测超过 1080p；发布包不含该二进制".into(),
        }],
        error: None,
    }
}

// =====** 会话内部结构 =====**

/// 写线程任务：一帧原始像素，或结束标记。
enum Job {
    /// 待写入 ffmpeg stdin 的 BGRA 帧。
    Frame(PooledBuffer),
    /// 关闭 stdin，让 ffmpeg 冲刷退出。
    Eof,
}

/// 读线程输出：一个完整访问单元。
struct RawAu {
    /// Annex-B 码流（含 AUD/SPS/PPS/切片）。
    data: Vec<u8>,
    /// 与提交顺序配对的采集时间戳。
    pts_ns: u64,
    /// 提交时刻，用于统计提交→产出时延。
    submitted: Instant,
    /// 是否含 IDR 切片。
    keyframe: bool,
}

/// 一次编码会话（一个 ffmpeg 子进程 + 两条泵线程）。
struct Session {
    /// 子进程句柄。
    child: Child,
    /// 提交通道；`None` 表示已关闭（flush/drop 中）。
    tx: Option<Sender<Job>>,
    /// 写线程。
    writer: Option<JoinHandle<()>>,
    /// 访问单元输出通道。
    rx: Receiver<RawAu>,
    /// 读线程。
    reader: Option<JoinHandle<()>>,
    /// 已提交但尚未产出的时间戳队列（与访问单元 FIFO 配对）。
    pending: Arc<Mutex<VecDeque<(u64, Instant)>>>,
}

impl Session {
    /// 关闭 stdin、等待 ffmpeg 退出，并回收两条线程。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；进程异常退出按正常结束处理。
    fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            if let Err(error) = tx.send(Job::Eof) {
                tracing::debug!("[codec/linux] 关闭信号发送失败（写线程已退出）: {error}");
            }
            drop(tx);
        }
        if let Some(writer) = self.writer.take() {
            if let Err(error) = writer.join() {
                tracing::debug!("[codec/linux] 写线程异常退出: {error:?}");
            }
        }
        // ffmpeg 在 stdin EOF 后冲刷剩余帧；超时则 kill，保证读线程能退出。
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        if let Err(error) = self.child.kill() {
                            tracing::debug!("[codec/linux] 冲刷超时后 kill 失败: {error}");
                        }
                        if let Err(error) = self.child.wait() {
                            tracing::debug!("[codec/linux] 冲刷超时后 wait 失败: {error}");
                        }
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            if let Err(error) = reader.join() {
                tracing::debug!("[codec/linux] 读线程异常退出: {error:?}");
            }
        }
    }
}

impl Drop for Session {
    /// 兜底清理：先杀子进程再回收线程，避免析构路径卡死。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn drop(&mut self) {
        let _ = self.child.kill(); // gate-allow R-C-2: Drop 兜底击杀，紧随的 shutdown 会再次回收并记录失败
        self.shutdown();
    }
}

// =====** PlatformEncoder =====**

/// Linux 软件编码器（外部 ffmpeg + libx264）。
pub struct PlatformEncoder {
    /// 最近一次配置。
    config: Option<VideoConfig>,
    /// 当前会话；未 configure 时为 `None`。
    session: Option<Session>,
    /// 编码统计。
    stats: EncodeStats,
    /// 最近一次看到的 SPS/PPS（含起始码）。
    seq_header: Vec<u8>,
    /// 统计起点（首次 push）。
    started: Option<Instant>,
    /// 关键帧请求标志（外部管道无法逐帧生效，见 [`VideoEncoder::request_keyframe`]）。
    keyframe_requested: bool,
}

impl PlatformEncoder {
    /// 构造未配置的 Linux 软件编码器。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 尚未创建 ffmpeg 进程的实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self {
            config: None,
            session: None,
            stats: EncodeStats::new(),
            seq_header: Vec::new(),
            started: None,
            keyframe_requested: false,
        }
    }

    /// 生成 ffmpeg 命令。
    ///
    /// # Arguments
    /// * `config` - 已验证的编码配置。
    ///
    /// # Returns
    /// 已配置好参数与管道的 `Command`。
    ///
    /// # Errors
    /// 不会失败（参数合法性在此之前校验）。
    fn command(config: &VideoConfig) -> Command {
        let keyint = config.fps.max(1);
        let kbps = (config.bitrate_bps / 1000).max(50);
        let size = format!("{}x{}", config.width, config.height);
        let fps = config.fps.max(1).to_string();
        let x264 = format!(
            "keyint={keyint}:min-keyint={keyint}:scenecut=0:aud=1:repeat_headers=1:\
             rc-lookahead=0:sync-lookahead=0:bframes=0:sliced-threads=1"
        );
        let pix_fmt = match config.chroma {
            VideoChroma::Yuv420 => "yuv420p",
            VideoChroma::Yuv444 => "yuv444p",
        };

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(["-f", "rawvideo", "-pix_fmt", "bgra"])
            .args(["-video_size", size.as_str(), "-framerate", fps.as_str()])
            .args(["-i", "-"])
            .args(["-c:v", "libx264", "-preset", "ultrafast"])
            .args(["-tune", "zerolatency"])
            // 低延迟：限制线程数并显式切片多线程。x264 默认「帧级多线程」会引入
            // (线程数-1) 帧的组帧延迟（VM 实测 p50 编码延迟 43ms ≈ 2~3 帧）；
            // 切片多线程只增加不到 1 帧，代价是压缩率略降（本演示可接受）。
            .args(["-threads", "4"])
            .args(["-x264-params", x264.as_str()])
            .args(["-b:v", format!("{kbps}k").as_str()])
            .args(["-maxrate", format!("{kbps}k").as_str()])
            .args(["-bufsize", format!("{}k", kbps * 2).as_str()])
            .args(["-pix_fmt", pix_fmt, "-f", "h264", "-"]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        cmd
    }

    /// 校验配置是否被本后端接受。
    ///
    /// # Arguments
    /// * `config` - 待校验配置。
    ///
    /// # Returns
    /// 合法返回 `Ok(())`。
    ///
    /// # Errors
    /// 非 H.264、10bit、色度为 4:2:0 但宽高为奇数、超尺寸时返回 `InvalidConfig`。
    fn validate(config: &VideoConfig) -> Result<(), CodecError> {
        if config.codec != CodecKind::H264 {
            return Err(CodecError::Unsupported);
        }
        if config.bit_depth != 8 {
            return Err(CodecError::InvalidConfig(format!(
                "Linux 软编只支持 8bit，收到 {}",
                config.bit_depth
            )));
        }
        if config.width == 0 || config.height == 0 {
            return Err(CodecError::InvalidConfig("宽高不能为 0".into()));
        }
        if config.width > MAX_DIM || config.height > MAX_DIM {
            return Err(CodecError::InvalidConfig(format!(
                "{}x{} 超过上限 {}",
                config.width, config.height, MAX_DIM
            )));
        }
        if config.chroma == VideoChroma::Yuv420
            && (config.width % 2 != 0 || config.height % 2 != 0)
        {
            return Err(CodecError::InvalidConfig(
                "4:2:0 要求宽高为偶数".into(),
            ));
        }
        if config.fps == 0 {
            return Err(CodecError::InvalidConfig("帧率不能为 0".into()));
        }
        Ok(())
    }

    /// 把访问单元里的 SPS/PPS 记进序列头缓存。
    ///
    /// # Arguments
    /// * `data` - 一个访问单元的 Annex-B 字节。
    ///
    /// # Returns
    /// 无返回值；未发现 SPS/PPS 时缓存保持不变。
    ///
    /// # Errors
    /// 不会失败。
    fn remember_headers(&mut self, data: &[u8]) {
        let mut sps: Option<Vec<u8>> = None;
        let mut pps: Option<Vec<u8>> = None;
        let nals = nal_ranges(data);
        for (start, end, kind) in nals {
            match kind {
                7 => sps = Some(data[start..end].to_vec()),
                8 => pps = Some(data[start..end].to_vec()),
                _ => {}
            }
        }
        match (sps, pps) {
            (Some(sps), Some(pps)) => {
                self.seq_header.clear();
                self.seq_header.extend_from_slice(&sps);
                self.seq_header.extend_from_slice(&pps);
            }
            (Some(sps), None) => {
                self.seq_header.clear();
                self.seq_header.extend_from_slice(&sps);
            }
            _ => {}
        }
    }

    /// 把一帧的输出组装成 [`EncodedPacket`] 并记统计。
    ///
    /// # Arguments
    /// * `au` - 读线程产出的访问单元。
    ///
    /// # Returns
    /// 可直接交给传输层的编码包。
    ///
    /// # Errors
    /// 不会失败。
    fn packet_from(&mut self, au: RawAu) -> EncodedPacket {
        self.remember_headers(&au.data);
        let latency_us = au.submitted.elapsed().as_micros() as u64;
        let len = au.data.len();
        self.stats.record(0, latency_us, len, au.keyframe);
        EncodedPacket {
            data: au.data,
            pts_ns: au.pts_ns,
            is_keyframe: au.keyframe,
            suggested_shard_bytes: 0,
        }
    }
}

impl Default for PlatformEncoder {
    /// 等价于 [`PlatformEncoder::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// =====** Annex-B 工具 =====**

/// 在 `buf[from..]` 里找 AUD 起始码 `00 00 01 09`，返回 3 字节起始码的下标。
///
/// # Arguments
/// * `buf` - 待扫描字节。
/// * `from` - 起始扫描位置。
///
/// # Returns
/// 找到返回起始码下标；没有返回 `None`。
///
/// # Errors
/// 不会失败。
fn find_aud(buf: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 && buf[i + 3] == 9 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// 枚举一个访问单元内的 NAL 单元范围与类型。
///
/// # Arguments
/// * `data` - Annex-B 字节。
///
/// # Returns
/// `(起始下标, 结束下标, nal_type)` 列表；起始下标指向 `00 00 01`。
///
/// # Errors
/// 不会失败。
fn nal_ranges(data: &[u8]) -> Vec<(usize, usize, u8)> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, data[i + 3] & 0x1f));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, (start, kind)) in starts.iter().enumerate() {
        let end = starts
            .get(idx + 1)
            .map(|(next, _)| *next)
            .unwrap_or(data.len());
        out.push((*start, end, *kind));
    }
    out
}

// =====** 泵线程 =====**

/// `spawn_pumps` 的四个句柄：提交通道、写线程、输出通道、读线程。
type PumpHandles = (Sender<Job>, JoinHandle<()>, Receiver<RawAu>, JoinHandle<()>);

/// 启动写线程与读线程。
///
/// # Arguments
/// * `child` - 已 spawn 的 ffmpeg 子进程。
/// * `pending` - 与访问单元 FIFO 配对的时间戳队列。
///
/// # Returns
/// `(提交通道, 写线程, 输出通道, 读线程)`。
///
/// # Errors
/// 线程创建失败时返回错误（调用方负责杀子进程）。
fn spawn_pumps(
    child: &mut Child,
    pending: Arc<Mutex<VecDeque<(u64, Instant)>>>,
) -> Result<PumpHandles, CodecError> {
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| CodecError::EncoderFailure("ffmpeg stdin 不可用".into()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| CodecError::EncoderFailure("ffmpeg stdout 不可用".into()))?;

    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let writer = thread::Builder::new()
        .name("rdc-x264-in".into())
        .spawn(move || {
            let mut stdin = stdin;
            while let Ok(job) = job_rx.recv() {
                match job {
                    Job::Frame(frame) => {
                        if stdin.write_all(frame.as_slice()).is_err() {
                            break;
                        }
                        // 立即 flush：管道缓冲不积压，降低端到端时延。
                        if stdin.flush().is_err() {
                            break;
                        }
                    }
                    Job::Eof => break,
                }
            }
            // 写端析构即关闭 stdin，触发 ffmpeg 冲刷退出。
        })
        .map_err(|error| CodecError::EncoderFailure(format!("写线程创建失败: {error}")))?;

    let (au_tx, au_rx) = mpsc::channel::<RawAu>();
    let reader = thread::Builder::new()
        .name("rdc-x264-out".into())
        .spawn(move || {
            let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
            let mut chunk = vec![0u8; READ_CHUNK];
            let mut au_start = 0usize;
            let mut scan = 4usize;
            loop {
                let read = match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..read]);
                // 每当看到下一个 AUD，就把前一个访问单元发出去。
                while let Some(pos) = find_aud(&buf, scan) {
                    if pos > au_start {
                        emit_au(&buf[au_start..pos], &pending, &au_tx);
                    }
                    au_start = pos;
                    scan = pos + 4;
                }
                // 丢弃已产出前缀，避免缓冲随运行时间增长。
                if au_start > 0 {
                    buf.drain(..au_start);
                    scan -= au_start;
                    au_start = 0;
                }
                scan = scan.max(4);
            }
            if buf.len() > au_start {
                emit_au(&buf[au_start..], &pending, &au_tx);
            }
        })
        .map_err(|error| CodecError::EncoderFailure(format!("读线程创建失败: {error}")))?;

    Ok((job_tx, writer, au_rx, reader))
}

/// 把一段访问单元字节与时间戳配对后发送。
///
/// # Arguments
/// * `data` - 访问单元字节。
/// * `pending` - 时间戳队列。
/// * `tx` - 输出通道。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败；无待配对时间戳时用 0。
fn emit_au(data: &[u8], pending: &Arc<Mutex<VecDeque<(u64, Instant)>>>, tx: &Sender<RawAu>) {
    let (pts_ns, submitted) = pending
        .lock()
        .ok()
        .and_then(|mut queue| queue.pop_front())
        .unwrap_or((0, Instant::now()));
    let keyframe = nal_ranges(data).iter().any(|(_, _, kind)| *kind == 5);
    if tx
        .send(RawAu {
            data: data.to_vec(),
            pts_ns,
            submitted,
            keyframe,
        })
        .is_err()
    {
        tracing::debug!("[codec/linux] 访问单元无处投递（接收端已关闭）");
    }
}

// =====** VideoEncoder 实现 =====**

impl VideoEncoder for PlatformEncoder {
    /// 探测软件编码能力。
    ///
    /// # Arguments
    /// * `codecs` - 待探测格式；空切片表示全部。
    ///
    /// # Returns
    /// 系统 ffmpeg 带 libx264 且请求包含 H.264 时返回一条能力记录，否则空列表。
    ///
    /// # Errors
    /// 不会失败。
    fn probe(&self, codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        if !ffmpeg_has_libx264() {
            return Vec::new();
        }
        if !codecs.is_empty() && !codecs.contains(&CodecKind::H264) {
            return Vec::new();
        }
        vec![EncoderCaps {
            codec: CodecKind::H264,
            max_width: MAX_DIM,
            max_height: MAX_DIM,
            max_fps: 240,
            supports_444: true,
            supports_10bit: false,
            supports_runtime_reconfigure: false,
        }]
    }

    /// 创建 ffmpeg 编码会话。
    ///
    /// # Arguments
    /// * `config` - 编码配置（H.264 / 8bit / 偶数宽高）。
    ///
    /// # Returns
    /// 会话就绪返回 `Ok(())`。
    ///
    /// # Errors
    /// 配置非法返回 `InvalidConfig`；ffmpeg 缺失返回 `HardwareUnsupported`；
    /// 进程启动或管道建立失败返回 `EncoderFailure`。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        Self::validate(&config)?;
        if !ffmpeg_has_libx264() {
            return Err(CodecError::HardwareUnsupported);
        }
        if let Some(mut old) = self.session.take() {
            old.shutdown();
        }
        let mut command = Self::command(&config);
        let mut child = command
            .spawn()
            .map_err(|error| CodecError::EncoderFailure(format!("启动 ffmpeg 失败: {error}")))?;
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, writer, rx, reader) = match spawn_pumps(&mut child, pending.clone()) {
            Ok(pumps) => pumps,
            Err(error) => {
                if let Err(kill_error) = child.kill() {
                    tracing::debug!("[codec/linux] 建管道失败后 kill 也失败: {kill_error}");
                }
                if let Err(wait_error) = child.wait() {
                    tracing::debug!("[codec/linux] 建管道失败后 wait 也失败: {wait_error}");
                }
                return Err(error);
            }
        };
        self.session = Some(Session {
            child,
            tx: Some(tx),
            writer: Some(writer),
            rx,
            reader: Some(reader),
            pending,
        });
        self.config = Some(config);
        self.started = None;
        self.seq_header.clear();
        Ok(())
    }

    /// 提交一帧 BGRA 像素。
    ///
    /// # Arguments
    /// * `frame` - 池化帧；长度不足一帧时拒绝。
    /// * `pts_ns` - 采集时间戳（同机建议用 `SystemTime` 纳秒，便于跨进程比时延）。
    /// * `force_keyframe` - 置位但**不生效**（外部 ffmpeg 管道无法逐帧强制 IDR）。
    ///
    /// # Returns
    /// 入队成功返回 `Ok(())`（写线程异步落管道，不阻塞）。
    ///
    /// # Errors
    /// 未配置、帧尺寸不足、写线程已退出时返回错误。
    fn push(
        &mut self,
        frame: PooledBuffer,
        pts_ns: u64,
        force_keyframe: bool,
    ) -> Result<(), CodecError> {
        let config = self
            .config
            .ok_or_else(|| CodecError::EncoderFailure("编码器未配置".into()))?;
        let need = config.width as usize * config.height as usize * 4;
        if frame.len() < need {
            return Err(CodecError::EncoderFailure(format!(
                "帧缓冲 {} 字节，小于 {}x{} BGRA 所需 {}",
                frame.len(),
                config.width,
                config.height,
                need
            )));
        }
        let session = self
            .session
            .as_mut()
            .ok_or_else(|| CodecError::EncoderFailure("编码会话不存在".into()))?;
        if let Ok(mut pending) = session.pending.lock() {
            pending.push_back((pts_ns, Instant::now()));
        }
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        if force_keyframe {
            self.keyframe_requested = true;
        }
        session
            .tx
            .as_ref()
            .ok_or_else(|| CodecError::EncoderFailure("编码会话已关闭".into()))?
            .send(Job::Frame(frame))
            .map_err(|_| CodecError::EncoderFailure("ffmpeg 写线程已退出".into()))
    }

    /// 非阻塞取出一包编码输出。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 有访问单元就绪返回编码包，否则 `None`。
    ///
    /// # Errors
    /// 不会失败。
    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        let au = self.session.as_mut()?.rx.try_recv().ok()?;
        if au.keyframe {
            self.keyframe_requested = false;
        }
        Some(self.packet_from(au))
    }

    /// 重建会话（外部管道不支持热更新，只能重启 ffmpeg）。
    ///
    /// # Arguments
    /// * `config` - 新配置。
    ///
    /// # Returns
    /// 重建成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 同 [`VideoEncoder::configure`]。
    fn reconfigure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        self.configure(config)
    }

    /// 关键帧请求（外部管道限制，仅置位不生效）。
    ///
    /// 为什么是空实现：ffmpeg CLI 没有「下一帧强制 IDR」的输入侧接口；
    /// 关键帧由首帧 IDR 与 `keyint=1s` 保证，丢包恢复上限为 1 秒。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn request_keyframe(&mut self) {
        self.keyframe_requested = true;
    }

    /// 后端名称。
    fn backend_name(&self) -> &str {
        BACKEND
    }

    /// 编码统计快照。
    ///
    /// # Returns
    /// 已配置时返回帧率/码率/时延分位数。
    ///
    /// # Errors
    /// 不会失败。
    fn stats_snapshot(&self) -> Option<EncodeSnapshot> {
        let started = self.started?;
        Some(self.stats.snapshot(started.elapsed().as_millis() as u64))
    }

    /// 序列头（SPS/PPS，Annex-B）。
    fn sequence_header(&self) -> &[u8] {
        &self.seq_header
    }

    /// 关闭输入并收集剩余码流。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 冲刷出的剩余编码包（按提交顺序）。
    ///
    /// # Errors
    /// 不会失败；子进程异常退出按正常结束处理。
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, CodecError> {
        let Some(mut session) = self.session.take() else {
            return Ok(Vec::new());
        };
        session.shutdown();
        let mut out = Vec::new();
        while let Ok(au) = session.rx.try_recv() {
            out.push(self.packet_from(au));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个合法的 1280x800 H.264 配置。
    fn base() -> VideoConfig {
        VideoConfig {
            codec: CodecKind::H264,
            width: 1280,
            height: 800,
            fps: 60,
            bitrate_bps: 8_000_000,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        }
    }

    /// E1：H.264 合法配置被接受。
    #[test]
    fn accepts_h264_even_dimensions() {
        assert!(PlatformEncoder::validate(&base()).is_ok());
    }

    /// E1：HEVC 明确拒绝，不静默降级。
    #[test]
    fn rejects_hevc() {
        let mut config = base();
        config.codec = CodecKind::Hevc;
        assert!(matches!(
            PlatformEncoder::validate(&config),
            Err(CodecError::Unsupported)
        ));
    }

    /// E1：AV1 明确拒绝。
    #[test]
    fn rejects_av1() {
        let mut config = base();
        config.codec = CodecKind::Av1;
        assert!(matches!(
            PlatformEncoder::validate(&config),
            Err(CodecError::Unsupported)
        ));
    }

    /// 奇数宽高不能用于 4:2:0。
    #[test]
    fn rejects_odd_dimensions_for_yuv420() {
        let mut config = base();
        config.width = 1281;
        assert!(matches!(
            PlatformEncoder::validate(&config),
            Err(CodecError::InvalidConfig(_))
        ));
    }

    /// 超过尺寸上限被拒绝。
    #[test]
    fn rejects_oversize() {
        let mut config = base();
        config.height = MAX_DIM + 2;
        assert!(matches!(
            PlatformEncoder::validate(&config),
            Err(CodecError::InvalidConfig(_))
        ));
    }

    /// 10bit 被拒绝。
    #[test]
    fn rejects_10bit() {
        let mut config = base();
        config.bit_depth = 10;
        assert!(matches!(
            PlatformEncoder::validate(&config),
            Err(CodecError::InvalidConfig(_))
        ));
    }
}
