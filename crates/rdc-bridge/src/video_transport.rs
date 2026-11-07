// ============================================================
// 文件用途：媒体包网络接缝 —— S4 头解封装 → 会话密钥 AEAD 解密 → 时间戳映射 → 视频重组
//
// 层级：L4 桥接层（连接 rdc-transport 的线格式与 rdc-protocol::video）
// 被谁调用：api.rs 的 video_ingest_datagram / video_set_media_key / video_media_stats
// 边界：本模块不持有 socket。socket 的归属方（会话层）把 `UdpTransport::recv_from`
//       收到的整包交给 `ingest_datagram`；落盘设计见 research/android-s9-video-transport.md。
// ============================================================

use std::sync::Mutex;
use std::sync::OnceLock;

use rdc_protocol::{
    decode_video_message, PacketHeader, StreamKind, VideoMessage, AEAD_TAG_LEN,
    FRAGMENT_PAYLOAD_MAX, PACKET_HEADER_LEN,
};
use rdc_security::{nonce_from_counter, AeadKey};

use crate::video_service::{self, VideoStatsDto};

/// 媒体方向：被控端 → 主控端（视频下行）。
pub const MEDIA_H2C: u8 = 1;
/// 媒体方向：主控端 → 被控端（未来上行）。
pub const MEDIA_C2H: u8 = 2;

// =====** DTO =====**

/// 媒体传输统计（诊断用，不参与业务判定）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaTransportStatsDto {
    /// 已接受并送入视频重组的包数。
    pub accepted: u64,
    /// 解封装失败（魔数/版本/长度错）。
    pub bad_header: u64,
    /// 非视频通道（`StreamKind` 不是 VideoMain/VideoSub）。
    pub wrong_stream: u64,
    /// 未设置媒体密钥就收到密文包。
    pub key_missing: u64,
    /// AEAD 认证失败（密钥错/被篡改/AAD 不一致）。
    pub auth_failures: u64,
    /// 解密后不是合法视频消息。
    pub bad_message: u64,
    /// 帧头 `pts_us` 与包头 `ts_us` 不一致、已以包头为准的次数。
    pub ts_mismatches: u64,
    /// 已安装密钥的方向；`0` 表示尚未安装。
    pub direction: u8,
    /// 最近错误。
    pub last_error: Option<String>,
}

// =====** 状态 =====**

struct MediaState {
    key: Option<AeadKey>,
    direction: u8,
    stats: MediaTransportStatsDto,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            key: None,
            direction: 0,
            stats: MediaTransportStatsDto::default(),
        }
    }
}

fn state() -> &'static Mutex<MediaState> {
    static SLOT: OnceLock<Mutex<MediaState>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(MediaState::default()))
}

fn lock_state() -> std::sync::MutexGuard<'static, MediaState> {
    state().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// =====** 密钥 =====**

/// 安装媒体通道密钥。
///
/// # Arguments
/// * `key` - 32 字节会话密钥（`ChannelKeys::media_h2c` / `media_c2h`）
/// * `direction` - [`MEDIA_H2C`] 或 [`MEDIA_C2H`]
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 密钥长度不是 32 字节或方向非法时返回错误字符串。
pub fn set_media_key(key: Vec<u8>, direction: u8) -> Result<(), String> {
    if key.len() != 32 {
        return Err(format!("媒体密钥必须是 32 字节，实际 {}", key.len()));
    }
    if direction != MEDIA_H2C && direction != MEDIA_C2H {
        return Err(format!("媒体方向必须是 {MEDIA_H2C}/{MEDIA_C2H}，实际 {direction}"));
    }
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&key);
    let mut guard = lock_state();
    guard.key = Some(AeadKey::new(bytes));
    guard.direction = direction;
    Ok(())
}

/// 清除媒体密钥；清除后所有媒体包按“密钥缺失”丢弃。
pub fn clear_media_key() {
    let mut guard = lock_state();
    guard.key = None;
    guard.direction = 0;
}

/// 读取媒体传输统计。
pub fn media_stats() -> MediaTransportStatsDto {
    lock_state().stats.clone()
}

// =====** 解开线上包 =====**

/// 按 `PacketHeader` 计算媒体 AEAD nonce。
///
/// 组合方式：`[0,0,0,direction] ‖ (frame_id<<32 | seq)`，见设计文档 §3。
/// `frame_id` 与 `seq` 都在包头里，接收端无需额外状态即可复算。
fn media_nonce(header: &PacketHeader, direction: u8) -> [u8; 12] {
    let mut nonce = nonce_from_counter(((header.frame_id as u64) << 32) | header.seq as u64);
    nonce[0] = direction;
    nonce
}

/// 把一个媒体 datagram 解封装 + 解密，返回包头与视频消息明文。
fn open_datagram(
    datagram: &[u8],
    key: Option<&AeadKey>,
    direction: u8,
) -> Result<(PacketHeader, Vec<u8>), (&'static str, String)> {
    let header = PacketHeader::decode(datagram).map_err(|error| ("bad_header", error.to_string()))?;
    header
        .ensure_supported()
        .map_err(|error| ("bad_header", error.to_string()))?;
    let stream = StreamKind::from_u8(header.stream)
        .ok_or_else(|| ("wrong_stream", format!("未知通道 {}", header.stream)))?;
    if stream.as_u8() != StreamKind::VideoMain.as_u8()
        && stream.as_u8() != StreamKind::VideoSub.as_u8()
    {
        return Err(("wrong_stream", format!("通道 {} 不是视频通道", header.stream)));
    }
    let key = key.ok_or_else(|| ("key_missing", "媒体密钥尚未安装".to_owned()))?;
    let payload_len = header.payload_len as usize;
    if payload_len > FRAGMENT_PAYLOAD_MAX {
        return Err(("bad_header", "载荷超过单包上限".to_owned()));
    }
    let expected = PACKET_HEADER_LEN + payload_len + AEAD_TAG_LEN;
    if datagram.len() != expected {
        return Err((
            "bad_header",
            format!("包长 {} 与头声明 {} 不一致", datagram.len(), expected),
        ));
    }
    let nonce = media_nonce(&header, direction);
    // AAD = 完整 32 字节包头：篡改 stream/seq/ts/frame_id/payload_len 都会认证失败。
    let aad = &datagram[..PACKET_HEADER_LEN];
    let ciphertext = &datagram[PACKET_HEADER_LEN..PACKET_HEADER_LEN + payload_len + AEAD_TAG_LEN];
    let plaintext = key
        .open_with_aad(ciphertext, &nonce, aad)
        .map_err(|error| ("auth_failures", error.to_string()))?;
    Ok((header, plaintext))
}

/// 喂入一个收到的媒体包（整包：32 字节头 + 密文 + tag）。
///
/// 行为：解封装 → AEAD 解密（AAD=包头）→ 视频消息解码 → 时间戳映射 → 多码流过滤 → 重组。
/// 任何一步失败只计数并返回错误，不 panic。
///
/// # Arguments
/// * `datagram` - UDP 收到的完整 datagram
///
/// # Returns
/// 成功时返回最新视频统计。
///
/// # Errors
/// 解封装/认证/消息解码失败时返回错误字符串（统计里按类别累加）。
pub fn ingest_datagram(datagram: &[u8]) -> Result<VideoStatsDto, String> {
    let (header, plaintext) = {
        let guard = lock_state();
        match open_datagram(datagram, guard.key.as_ref(), guard.direction) {
            Ok(ok) => ok,
            Err((kind, message)) => {
                drop(guard);
                return record_error(kind, message);
            }
        }
    };
    let mut message = match decode_video_message(&plaintext) {
        Ok(message) => message,
        Err(error) => return record_error("bad_message", error.to_string()),
    };
    // 时间戳映射：包头 ts 是权威采集时间；不一致时覆盖帧头，保证延迟统计口径统一。
    if let VideoMessage::Frame(frame) = &mut message {
        if frame.pts_us != header.ts_us {
            let mut guard = lock_state();
            guard.stats.ts_mismatches += 1;
        }
        frame.pts_us = header.ts_us;
    }
    {
        let mut guard = lock_state();
        guard.stats.accepted += 1;
    }
    Ok(video_service::ingest_message(message))
}

fn record_error(kind: &'static str, message: String) -> Result<VideoStatsDto, String> {
    let mut guard = lock_state();
    match kind {
        "bad_header" => guard.stats.bad_header += 1,
        "wrong_stream" => guard.stats.wrong_stream += 1,
        "key_missing" => guard.stats.key_missing += 1,
        "auth_failures" => guard.stats.auth_failures += 1,
        _ => guard.stats.bad_message += 1,
    }
    guard.stats.last_error = Some(message.clone());
    Err(message)
}

// =====** 发送侧（被控端/测试用） =====**

/// 把一个视频消息封装成线上媒体包：`S4 头 + AEAD(消息)`。
///
/// 被控端发送路径与集成测试共用同一实现，避免两端线格式漂移。
///
/// # Arguments
/// * `header` - 已填好 stream/seq/ts/frame_id 的包头（`payload_len` 会被覆盖）
/// * `message` - 视频消息
/// * `key` - 32 字节媒体密钥
/// * `direction` - 发送方向（决定 nonce 的方向字节）
///
/// # Returns
/// 可直接交给 `UdpTransport::send` 的整包字节。
///
/// # Errors
/// 消息编码失败、超出单包上限或包头非法时返回错误字符串。
pub fn build_datagram(
    mut header: PacketHeader,
    message: &VideoMessage,
    key: &[u8; 32],
    direction: u8,
) -> Result<Vec<u8>, String> {
    let plaintext = rdc_protocol::encode_video_message(message).map_err(|e| e.to_string())?;
    if plaintext.len() > FRAGMENT_PAYLOAD_MAX {
        return Err(format!(
            "视频消息 {} 字节超过单包上限 {FRAGMENT_PAYLOAD_MAX}",
            plaintext.len()
        ));
    }
    header.payload_len = plaintext.len() as u16;
    let header_bytes = header.encode().map_err(|e| e.to_string())?;
    let key = AeadKey::new(*key);
    let nonce = media_nonce(&header, direction);
    let ciphertext = key
        .seal_with_aad(&plaintext, &nonce, &header_bytes)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(PACKET_HEADER_LEN + ciphertext.len());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// 重置媒体统计（不影响已安装的密钥）。
pub fn reset_stats() {
    let mut guard = lock_state();
    guard.stats = MediaTransportStatsDto::default();
}

// =====** 测试 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_protocol::{
        encode_video_message, VideoCodec, VideoColorRange, VideoColorSpace, VideoFrameHeader,
        VideoStreamConfig, VIDEO_FRAGMENT_BYTES, VIDEO_MEDIA_FRAGMENT_BYTES,
    };

    const KEY: [u8; 32] = [7u8; 32];

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

    fn header(seq: u32, frame_id: u16, ts_us: u64) -> PacketHeader {
        PacketHeader::new(0xA1B2, StreamKind::VideoMain.as_u8(), seq, ts_us, frame_id, 0)
            .expect("包头")
    }

    /// 完整链路：Meta + Frame 头 + 分片 → 解密 → 重组 → 出帧。
    #[test]
    fn datagram_roundtrip_reaches_video_queue() {
        let _guard = crate::video_service::test_lock();
        crate::video_service::reset();
        reset_stats();
        clear_media_key();
        set_media_key(KEY.to_vec(), MEDIA_H2C).unwrap();

        let meta = build_datagram(
            header(1, 0, 0),
            &VideoMessage::Meta(config()),
            &KEY,
            MEDIA_H2C,
        )
        .unwrap();
        ingest_datagram(&meta).expect("meta 应被接受");

        let data = vec![0x5Au8; 2000];
        let frag_count = data.len().div_ceil(VIDEO_MEDIA_FRAGMENT_BYTES) as u16;
        let frame = VideoFrameHeader {
            stream_id: 0,
            config_id: 1,
            frame_id: 1,
            pts_us: 33_000,
            keyframe: true,
            total_len: data.len() as u32,
            frag_count,
        };
        let frame_msg = build_datagram(
            header(2, 1, 33_000),
            &VideoMessage::Frame(frame),
            &KEY,
            MEDIA_H2C,
        )
        .unwrap();
        ingest_datagram(&frame_msg).expect("帧头应被接受");
        for (index, chunk) in data.chunks(VIDEO_MEDIA_FRAGMENT_BYTES).enumerate() {
            let message = VideoMessage::Fragment {
                stream_id: 0,
                frame_id: 1,
                frag_index: index as u16,
                frag_count,
                data: chunk.to_vec(),
            };
            let datagram =
                build_datagram(header(3 + index as u32, 1, 33_000), &message, &KEY, MEDIA_H2C)
                    .unwrap();
            ingest_datagram(&datagram).expect("分片应被接受");
        }
        let frame = crate::video_service::poll_frame().expect("应出帧");
        assert_eq!(frame.data, data);
        assert_eq!(frame.pts_us, 33_000);
        assert_eq!(media_stats().accepted, 1 + 1 + frag_count as u64);
        assert_eq!(media_stats().auth_failures, 0);
    }

    /// 时间戳映射：帧头 pts 与包头 ts 不一致时，以包头为准并计数。
    #[test]
    fn timestamp_mismatch_is_overridden_by_header() {
        let _guard = crate::video_service::test_lock();
        crate::video_service::reset();
        reset_stats();
        set_media_key(KEY.to_vec(), MEDIA_H2C).unwrap();
        ingest_datagram(&build_datagram(header(1, 0, 0), &VideoMessage::Meta(config()), &KEY, MEDIA_H2C).unwrap()).unwrap();

        let frame = VideoFrameHeader {
            stream_id: 0,
            config_id: 1,
            frame_id: 1,
            pts_us: 999_999,
            keyframe: true,
            total_len: 10,
            frag_count: 1,
        };
        ingest_datagram(&build_datagram(header(2, 1, 12_345), &VideoMessage::Frame(frame), &KEY, MEDIA_H2C).unwrap()).unwrap();
        ingest_datagram(
            &build_datagram(
                header(3, 1, 12_345),
                &VideoMessage::Fragment {
                    stream_id: 0,
                    frame_id: 1,
                    frag_index: 0,
                    frag_count: 1,
                    data: vec![1u8; 10],
                },
                &KEY,
                MEDIA_H2C,
            )
            .unwrap(),
        )
        .unwrap();
        let frame = crate::video_service::poll_frame().expect("应出帧");
        assert_eq!(frame.pts_us, 12_345);
        assert_eq!(media_stats().ts_mismatches, 1);
    }

    /// 被篡改的包必须 AEAD 失败，且不进入重组。
    #[test]
    fn tampered_datagram_is_rejected() {
        let _guard = crate::video_service::test_lock();
        crate::video_service::reset();
        reset_stats();
        set_media_key(KEY.to_vec(), MEDIA_H2C).unwrap();
        let mut datagram =
            build_datagram(header(1, 0, 0), &VideoMessage::Meta(config()), &KEY, MEDIA_H2C).unwrap();
        let last = datagram.len() - 1;
        datagram[last] ^= 0xFF;
        assert!(ingest_datagram(&datagram).is_err());
        assert_eq!(media_stats().auth_failures, 1);
        assert_eq!(crate::video_service::stats().config, None);
    }

    /// 未安装密钥：密文包按 key_missing 丢弃，不 panic。
    #[test]
    fn missing_key_is_counted() {
        let _guard = crate::video_service::test_lock();
        reset_stats();
        clear_media_key();
        let datagram =
            build_datagram(header(1, 0, 0), &VideoMessage::Meta(config()), &KEY, MEDIA_H2C).unwrap();
        assert!(ingest_datagram(&datagram).is_err());
        assert_eq!(media_stats().key_missing, 1);
    }

    /// 非视频通道的包必须按 wrong_stream 丢弃。
    #[test]
    fn non_video_stream_is_rejected() {
        let _guard = crate::video_service::test_lock();
        reset_stats();
        set_media_key(KEY.to_vec(), MEDIA_H2C).unwrap();
        let mut bad = header(1, 0, 0);
        bad.stream = 99;
        let datagram = build_datagram(bad, &VideoMessage::Meta(config()), &KEY, MEDIA_H2C).unwrap();
        assert!(ingest_datagram(&datagram).is_err());
        assert_eq!(media_stats().wrong_stream, 1);
    }

    /// 编码后的视频消息不得超过单包上限（防止 IP 分片）。
    #[test]
    fn oversized_message_is_refused() {
        // 用最大分片数据构造：5 字节头 + 1352 数据 = 1357 > 1352。
        let message = VideoMessage::Fragment {
            stream_id: 0,
            frame_id: 1,
            frag_index: 0,
            frag_count: 1,
            data: vec![0u8; VIDEO_FRAGMENT_BYTES],
        };
        assert!(encode_video_message(&message).unwrap().len() > FRAGMENT_PAYLOAD_MAX);
        assert!(build_datagram(header(1, 1, 0), &message, &KEY, MEDIA_H2C).is_err());
    }
}

