// ============================================================
// 文件用途：远端视频订阅服务 —— 协议消息 → 重排 → 待取帧队列
//
// 层级：L4 桥接层
// 被谁调用：api.rs 的 video_ingest / video_poll_frame / video_stats / video_reset
// 边界：本模块不碰 socket、不解码；传输层收到视频消息后调用 `ingest`，
//       Flutter 侧用 `poll_frame` 拉取已重组的 Annex-B 与元数据。
//       真正的网络接收（QUIC/媒体通道）尚未接入，宿主测试直接喂字节。
// ============================================================

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::OnceLock;

use rdc_protocol::{
    decode_video_message, VideoCodec, VideoColorRange, VideoColorSpace, VideoEvent,
    VideoFrame, VideoMessage, VideoReceiver, VIDEO_REASSEMBLY_WINDOW,
};

/// 待取帧上限：Flutter 以 8 ms 轮询；超过说明上层卡住，丢最旧帧保实时。
const MAX_QUEUED_FRAMES: usize = 8;

// =====** DTO =====**

/// 视频元数据快照（跨 FFI 用字符串表达枚举，避免 Dart 侧整数映射漂移）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoConfigDto {
    /// 子码流编号。
    pub stream_id: u32,
    /// 配置版本号；变化即需要重建解码器。
    pub config_id: u32,
    /// 编码宽。
    pub width: u16,
    /// 编码高。
    pub height: u16,
    /// 目标帧率。
    pub fps: u16,
    /// `h264`。
    pub codec: String,
    /// `bt709` / `bt601` / `srgb`。
    pub color_space: String,
    /// `limited` / `full`。
    pub color_range: String,
    /// SPS/PPS 是否在码流内。
    pub parameter_sets_in_band: bool,
    /// 带外 SPS 字节数。
    pub sps_len: u32,
    /// 带外 PPS 字节数。
    pub pps_len: u32,
}

/// 已重组的远端视频帧。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoFrameDto {
    /// 子码流编号。
    pub stream_id: u32,
    /// 帧号。
    pub frame_id: u32,
    /// 采集时间戳（微秒）。
    pub pts_us: u64,
    /// 是否 IDR。
    pub keyframe: bool,
    /// 元数据（含本次 config_id）。
    pub config: VideoConfigDto,
    /// Annex-B 字节。
    pub data: Vec<u8>,
}

/// 视频订阅统计。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoStatsDto {
    /// 当前元数据；`null` 表示还没收到 Meta。
    pub config: Option<VideoConfigDto>,
    /// 待 Flutter 拉取的帧数。
    pub queued: u32,
    /// 已重组帧数。
    pub completed: u64,
    /// 丢弃帧数（丢包后等 IDR、超窗淘汰、队列溢出）。
    pub dropped: u64,
    /// 因丢包/乱序重新等 IDR 的次数。
    pub resyncs: u64,
    /// 因队列溢出丢弃的帧数。
    pub queue_drops: u64,
    /// 已输出字节数。
    pub bytes: u64,
    /// 是否正在等关键帧。
    pub needs_keyframe: bool,
    /// 解码失败的消息数（未知 tag/字段非法）。
    pub decode_errors: u64,
    /// 最近错误。
    pub last_error: Option<String>,
}

// =====** 状态 =====**

struct State {
    receiver: VideoReceiver,
    frames: VecDeque<VideoFrameDto>,
    config: Option<VideoConfigDto>,
    queue_drops: u64,
    decode_errors: u64,
    last_error: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            receiver: VideoReceiver::new(VIDEO_REASSEMBLY_WINDOW),
            frames: VecDeque::new(),
            config: None,
            queue_drops: 0,
            decode_errors: 0,
            last_error: None,
        }
    }
}

fn state() -> &'static Mutex<State> {
    static SLOT: OnceLock<Mutex<State>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(State::default()))
}

// =====** 对外接口 =====**

/// 喂入一条视频协议消息（传输层收到的原始字节）。
///
/// 行为：解码 → 重排 → 元数据更新 → 完整帧入队；坏消息只记账不 panic。
pub fn ingest(bytes: Vec<u8>) -> VideoStatsDto {
    let message = match decode_video_message(&bytes) {
        Ok(message) => message,
        Err(error) => {
            let mut guard = match state().lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.decode_errors += 1;
            guard.last_error = Some(error.to_string());
            return snapshot(&guard);
        }
    };
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    match guard.receiver.push(message) {
        Ok(events) => {
            for event in events {
                match event {
                    VideoEvent::ConfigChanged(config) => {
                        guard.config = Some(config_dto(&config));
                    }
                    VideoEvent::Frame(frame) => {
                        if guard.frames.len() >= MAX_QUEUED_FRAMES {
                            guard.frames.pop_front();
                            guard.queue_drops += 1;
                        }
                        guard.frames.push_back(frame_dto(frame));
                    }
                }
            }
        }
        Err(error) => {
            guard.decode_errors += 1;
            guard.last_error = Some(error.to_string());
        }
    }
    snapshot(&guard)
}

/// 拉取一帧已重组的远端视频。
///
/// 返回：`null` 表示当前没有新帧；Flutter 持续轮询即可。
pub fn poll_frame() -> Option<VideoFrameDto> {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.frames.pop_front()
}

/// 读取订阅统计。
pub fn stats() -> VideoStatsDto {
    let guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    snapshot(&guard)
}

/// 清空接收器与待取帧（会话结束/切流）。
pub fn reset() -> VideoStatsDto {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.receiver.reset();
    guard.frames.clear();
    guard.config = None;
    guard.queue_drops = 0;
    guard.decode_errors = 0;
    guard.last_error = None;
    snapshot(&guard)
}

// =====** 映射 =====**

fn config_dto(config: &rdc_protocol::VideoStreamConfig) -> VideoConfigDto {
    VideoConfigDto {
        stream_id: config.stream_id,
        config_id: config.config_id,
        width: config.width,
        height: config.height,
        fps: config.fps,
        codec: match config.codec {
            VideoCodec::H264 => "h264".to_owned(),
        },
        color_space: match config.color_space {
            VideoColorSpace::Bt709 => "bt709".to_owned(),
            VideoColorSpace::Bt601 => "bt601".to_owned(),
            VideoColorSpace::Srgb => "srgb".to_owned(),
        },
        color_range: match config.color_range {
            VideoColorRange::Limited => "limited".to_owned(),
            VideoColorRange::Full => "full".to_owned(),
        },
        parameter_sets_in_band: config.parameter_sets_in_band,
        sps_len: config.sps.len() as u32,
        pps_len: config.pps.len() as u32,
    }
}

fn frame_dto(frame: VideoFrame) -> VideoFrameDto {
    VideoFrameDto {
        stream_id: frame.config.stream_id,
        frame_id: frame.frame_id,
        pts_us: frame.pts_us,
        keyframe: frame.keyframe,
        config: config_dto(&frame.config),
        data: frame.data,
    }
}

fn snapshot(state: &State) -> VideoStatsDto {
    VideoStatsDto {
        config: state.config.clone(),
        queued: state.frames.len() as u32,
        completed: state.receiver.completed(),
        dropped: state.receiver.dropped(),
        resyncs: state.receiver.resyncs(),
        queue_drops: state.queue_drops,
        bytes: state.receiver.bytes(),
        needs_keyframe: state.receiver.needs_keyframe(),
        decode_errors: state.decode_errors,
        last_error: state.last_error.clone(),
    }
}

// =====** 测试 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_protocol::{
        encode_video_message, VideoFrameHeader, VideoStreamConfig, VIDEO_FRAGMENT_BYTES,
    };

    fn config() -> VideoStreamConfig {
        VideoStreamConfig {
            stream_id: 0,
            config_id: 1,
            width: 640,
            height: 360,
            fps: 30,
            codec: VideoCodec::H264,
            color_space: VideoColorSpace::Bt709,
            color_range: VideoColorRange::Limited,
            parameter_sets_in_band: true,
            sps: Vec::new(),
            pps: Vec::new(),
        }
    }

    fn push_frame(data: &[u8], frame_id: u32, key: bool) {
        let header = VideoFrameHeader {
            stream_id: 0,
            config_id: 1,
            frame_id,
            pts_us: frame_id as u64 * 33_000,
            keyframe: key,
            total_len: data.len() as u32,
            frag_count: data.len().div_ceil(VIDEO_FRAGMENT_BYTES) as u16,
        };
        let _ = ingest(encode_video_message(&VideoMessage::Frame(header)).unwrap());
        for (index, chunk) in data.chunks(VIDEO_FRAGMENT_BYTES).enumerate() {
            let message = VideoMessage::Fragment {
                stream_id: 0,
                frame_id,
                frag_index: index as u16,
                frag_count: header.frag_count,
                data: chunk.to_vec(),
            };
            let _ = ingest(encode_video_message(&message).unwrap());
        }
    }

    /// 全局状态被多个用例共享：串行执行，避免 reset 互相踩踏。
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reset_all() {
        let _ = reset();
    }

    #[test]
    fn ingest_and_poll_runs_protocol_to_bridge() {
        let _guard = test_lock();
        reset_all();
        let meta = ingest(encode_video_message(&VideoMessage::Meta(config())).unwrap());
        assert_eq!(meta.config.as_ref().unwrap().width, 640);
        assert_eq!(meta.config.as_ref().unwrap().color_space, "bt709");

        let data = vec![0xA5u8; 4000];
        push_frame(&data, 1, true);
        let stats = stats();
        assert_eq!(stats.completed, 1);
        assert_eq!(stats.queued, 1);

        let frame = poll_frame().expect("应有帧");
        assert_eq!(frame.data, data);
        assert!(frame.keyframe);
        assert_eq!(frame.config.config_id, 1);
        assert!(poll_frame().is_none());
    }

    #[test]
    fn gap_marks_resync_and_waits_for_idr() {
        let _guard = test_lock();
        reset_all();
        let _ = ingest(encode_video_message(&VideoMessage::Meta(config())).unwrap());
        push_frame(&[1u8; 10], 1, true);
        let _ = poll_frame();

        push_frame(&[2u8; 10], 3, false);
        // 变量名不用 stats，避免遮蔽同名导出函数。
        let gap = stats();
        assert!(gap.needs_keyframe);
        assert!(gap.dropped >= 1);
        assert!(gap.resyncs >= 1);
        assert!(poll_frame().is_none());

        push_frame(&[3u8; 10], 4, true);
        let frame = poll_frame().expect("IDR 后恢复");
        assert!(frame.keyframe);
        // 关键帧到达后重新判定：门控解除。
        let after = stats();
        assert!(!after.needs_keyframe);
    }

    #[test]
    fn bad_message_is_counted_not_panicked() {
        let _guard = test_lock();
        reset_all();
        let stats = ingest(vec![0x63, 0x00]);
        assert_eq!(stats.decode_errors, 1);
        assert!(stats.last_error.is_some());
        assert_eq!(stats.queued, 0);
    }
}
