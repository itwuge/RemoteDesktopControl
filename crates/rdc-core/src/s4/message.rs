//! rdc-core / S4 会话通道消息编解码
//!
//! 层级：L3 编排层
//! 用途：把输入、剪贴板、文件三类业务消息复用到同一条已加密的会话控制流上，
//!       每条明文消息一个 tag + 定长字段；文件按 1352 字节分片。
//! 调用方：`rdc-core::s4` 的输入/剪贴板/文件任务、rdc-cli 的演示。
//!
//! 约束：本模块只做字节编解码，不加解密；AEAD 由 `EstablishedSession::send_control`
//! 统一完成。解码对长度、枚举与分片边界做严格校验，畸形消息一律 `Protocol`。

use rdc_input::{decode_event, encode_event, InputEvent};

use crate::s4::S4Error;

// =====** 常量 =====**

/// 文件分片载荷上限（docs/04 §2：单包明文上限 1352 字节）。
pub const FILE_FRAGMENT_BYTES: usize = 1352;

/// 消息 tag。
const TAG_INPUT: u8 = 1;
const TAG_CLIPBOARD: u8 = 2;
const TAG_FILE_MANIFEST: u8 = 3;
const TAG_FILE_FRAGMENT: u8 = 4;
const TAG_FILE_ACK: u8 = 5;
const TAG_DONE: u8 = 6;
const TAG_INPUT_SUMMARY: u8 = 7;
const TAG_FILE_DONE: u8 = 8;
const TAG_FILE_CHUNK_META: u8 = 9;

/// 文件清单消息的固定长度：tag(1) + transfer(4) + total(8) + chunk(8) + hash(32)。
const MANIFEST_LEN: usize = 1 + 4 + 8 + 8 + 32;
/// 文件分片的固定头：tag(1) + transfer(4) + chunk(4) + frag(4) + count(4)。
const FRAGMENT_HEADER_LEN: usize = 1 + 4 + 4 + 4 + 4;
/// 分块确认的固定长度：tag(1) + transfer(4) + chunk(4) + ok(1)。
const ACK_LEN: usize = 1 + 4 + 4 + 1;
/// 输入汇总的固定长度：tag(1) + count(4)。
const SUMMARY_LEN: usize = 1 + 4;
/// 文件完成的固定长度：tag(1) + transfer(4) + ok(1) + hash(32)。
const FILE_DONE_LEN: usize = 1 + 4 + 1 + 32;
/// 分块元信息的固定长度：tag(1) + transfer(4) + chunk(4) + hash(32)。
const CHUNK_META_LEN: usize = 1 + 4 + 4 + 32;

// =====** S4Message =====**

/// 会话通道消息。
#[derive(Debug, Clone, PartialEq)]
pub enum S4Message {
    /// 单个输入事件（明文 ≤14 字节）。
    Input(InputEvent),
    /// 剪贴板文本载荷（rdc-clipboard 线格式，含自校验哈希）。
    Clipboard(Vec<u8>),
    /// 文件清单：分块计划与整文件哈希。
    FileManifest {
        /// 传输标识（同一会话内唯一）。
        transfer_id: u32,
        /// 文件总字节数。
        total_bytes: u64,
        /// 分块大小（发送端与接收端必须一致）。
        chunk_size: u64,
        /// 整文件 SHA-256。
        file_hash: [u8; 32],
    },
    /// 文件分片（4 MiB 块再切成 ≤1352 字节）。
    FileFragment {
        /// 传输标识。
        transfer_id: u32,
        /// 原分块下标。
        chunk_index: u32,
        /// 分片下标（从 0 开始）。
        frag_index: u32,
        /// 该分块的分片总数。
        frag_count: u32,
        /// 分片内容。
        data: Vec<u8>,
    },
    /// 分块元信息（在分片之前发送，携带发送端算出的块哈希）。
    FileChunkMeta {
        /// 传输标识。
        transfer_id: u32,
        /// 分块下标。
        chunk_index: u32,
        /// 该块数据的 SHA-256。
        chunk_hash: [u8; 32],
    },
    /// 分块确认。
    FileAck {
        /// 传输标识。
        transfer_id: u32,
        /// 分块下标。
        chunk_index: u32,
        /// 校验是否通过。
        ok: bool,
    },
    /// 本端输入发送结束。
    Done,
    /// 被控端注入统计（演示用，确认 100 条全部落地后才关闭会话）。
    InputSummary(u32),
    /// 接收端完成整文件校验后的最终回报。
    FileDone {
        /// 传输标识。
        transfer_id: u32,
        /// 接收端实际算出的 SHA-256。
        file_hash: [u8; 32],
        /// 与清单声明是否一致。
        ok: bool,
    },
}

// =====** 编码 =====**

/// 编码一条消息。
///
/// # Arguments
/// * `message` - 待编码消息
///
/// # Returns
/// 可直接交给 `send_control` 的明文字节。
///
/// # Errors
/// 文件分片超过 [`FILE_FRAGMENT_BYTES`] 返回 [`S4Error::Protocol`]。
pub fn encode(message: &S4Message) -> Result<Vec<u8>, S4Error> {
    match message {
        S4Message::Input(event) => {
            let mut out = vec![TAG_INPUT];
            out.extend_from_slice(&encode_event(event));
            Ok(out)
        }
        S4Message::Clipboard(payload) => {
            let mut out = Vec::with_capacity(1 + payload.len());
            out.push(TAG_CLIPBOARD);
            out.extend_from_slice(payload);
            Ok(out)
        }
        S4Message::FileManifest {
            transfer_id,
            total_bytes,
            chunk_size,
            file_hash,
        } => {
            let mut out = Vec::with_capacity(MANIFEST_LEN);
            out.push(TAG_FILE_MANIFEST);
            out.extend_from_slice(&transfer_id.to_le_bytes());
            out.extend_from_slice(&total_bytes.to_le_bytes());
            out.extend_from_slice(&chunk_size.to_le_bytes());
            out.extend_from_slice(file_hash);
            Ok(out)
        }
        S4Message::FileFragment {
            transfer_id,
            chunk_index,
            frag_index,
            frag_count,
            data,
        } => {
            if data.len() > FILE_FRAGMENT_BYTES {
                return Err(S4Error::Protocol(format!(
                    "分片 {} 字节超过上限 {}",
                    data.len(),
                    FILE_FRAGMENT_BYTES
                )));
            }
            let mut out = Vec::with_capacity(FRAGMENT_HEADER_LEN + data.len());
            out.push(TAG_FILE_FRAGMENT);
            out.extend_from_slice(&transfer_id.to_le_bytes());
            out.extend_from_slice(&chunk_index.to_le_bytes());
            out.extend_from_slice(&frag_index.to_le_bytes());
            out.extend_from_slice(&frag_count.to_le_bytes());
            out.extend_from_slice(data);
            Ok(out)
        }
        S4Message::FileChunkMeta {
            transfer_id,
            chunk_index,
            chunk_hash,
        } => {
            let mut out = Vec::with_capacity(CHUNK_META_LEN);
            out.push(TAG_FILE_CHUNK_META);
            out.extend_from_slice(&transfer_id.to_le_bytes());
            out.extend_from_slice(&chunk_index.to_le_bytes());
            out.extend_from_slice(chunk_hash);
            Ok(out)
        }
        S4Message::FileAck {
            transfer_id,
            chunk_index,
            ok,
        } => {
            let mut out = Vec::with_capacity(ACK_LEN);
            out.push(TAG_FILE_ACK);
            out.extend_from_slice(&transfer_id.to_le_bytes());
            out.extend_from_slice(&chunk_index.to_le_bytes());
            out.push(u8::from(*ok));
            Ok(out)
        }
        S4Message::Done => Ok(vec![TAG_DONE]),
        S4Message::InputSummary(count) => {
            let mut out = Vec::with_capacity(SUMMARY_LEN);
            out.push(TAG_INPUT_SUMMARY);
            out.extend_from_slice(&count.to_le_bytes());
            Ok(out)
        }
        S4Message::FileDone {
            transfer_id,
            file_hash,
            ok,
        } => {
            let mut out = Vec::with_capacity(FILE_DONE_LEN);
            out.push(TAG_FILE_DONE);
            out.extend_from_slice(&transfer_id.to_le_bytes());
            out.push(u8::from(*ok));
            out.extend_from_slice(file_hash);
            Ok(out)
        }
    }
}

// =====** 解码 =====**

/// 解码一条消息。
///
/// # Arguments
/// * `bytes` - 控制流上收到的明文
///
/// # Returns
/// 解析出的消息。
///
/// # Errors
/// 空载荷、未知 tag、字段截断、分片下标越界或数据超限时返回 [`S4Error::Protocol`]。
pub fn decode(bytes: &[u8]) -> Result<S4Message, S4Error> {
    let Some((&tag, rest)) = bytes.split_first() else {
        return Err(S4Error::Protocol("空消息".into()));
    };
    match tag {
        TAG_INPUT => decode_event(rest)
            .map(S4Message::Input)
            .map_err(|e| S4Error::Protocol(format!("输入事件解码失败: {e}"))),
        TAG_CLIPBOARD => Ok(S4Message::Clipboard(rest.to_vec())),
        TAG_FILE_MANIFEST => {
            if rest.len() != MANIFEST_LEN - 1 {
                return Err(S4Error::Protocol(format!(
                    "文件清单长度 {} 应为 {}",
                    rest.len(),
                    MANIFEST_LEN - 1
                )));
            }
            Ok(S4Message::FileManifest {
                transfer_id: read_u32(&rest[0..4])?,
                total_bytes: read_u64(&rest[4..12])?,
                chunk_size: read_u64(&rest[12..20])?,
                file_hash: rest[20..52].try_into().expect("长度已校验"),
            })
        }
        TAG_FILE_FRAGMENT => {
            if rest.len() < FRAGMENT_HEADER_LEN - 1 {
                return Err(S4Error::Protocol("文件分片头截断".into()));
            }
            let transfer_id = read_u32(&rest[0..4])?;
            let chunk_index = read_u32(&rest[4..8])?;
            let frag_index = read_u32(&rest[8..12])?;
            let frag_count = read_u32(&rest[12..16])?;
            if frag_count == 0 || frag_index >= frag_count {
                return Err(S4Error::Protocol(format!(
                    "分片下标非法: {frag_index}/{frag_count}"
                )));
            }
            let data = rest[16..].to_vec();
            if data.len() > FILE_FRAGMENT_BYTES {
                return Err(S4Error::Protocol(format!(
                    "分片 {} 字节超过上限 {}",
                    data.len(),
                    FILE_FRAGMENT_BYTES
                )));
            }
            Ok(S4Message::FileFragment {
                transfer_id,
                chunk_index,
                frag_index,
                frag_count,
                data,
            })
        }
        TAG_FILE_ACK => {
            if rest.len() != ACK_LEN - 1 {
                return Err(S4Error::Protocol("分块确认长度非法".into()));
            }
            Ok(S4Message::FileAck {
                transfer_id: read_u32(&rest[0..4])?,
                chunk_index: read_u32(&rest[4..8])?,
                ok: rest[8] != 0,
            })
        }
        TAG_DONE => {
            if !rest.is_empty() {
                return Err(S4Error::Protocol("Done 消息不应带载荷".into()));
            }
            Ok(S4Message::Done)
        }
        TAG_INPUT_SUMMARY => {
            if rest.len() != SUMMARY_LEN - 1 {
                return Err(S4Error::Protocol("输入汇总长度非法".into()));
            }
            Ok(S4Message::InputSummary(read_u32(rest)?))
        }
        TAG_FILE_DONE => {
            if rest.len() != FILE_DONE_LEN - 1 {
                return Err(S4Error::Protocol("文件完成长度非法".into()));
            }
            Ok(S4Message::FileDone {
                transfer_id: read_u32(&rest[0..4])?,
                ok: rest[4] != 0,
                file_hash: rest[5..37].try_into().expect("长度已校验"),
            })
        }
        TAG_FILE_CHUNK_META => {
            if rest.len() != CHUNK_META_LEN - 1 {
                return Err(S4Error::Protocol("分块元信息长度非法".into()));
            }
            Ok(S4Message::FileChunkMeta {
                transfer_id: read_u32(&rest[0..4])?,
                chunk_index: read_u32(&rest[4..8])?,
                chunk_hash: rest[8..40].try_into().expect("长度已校验"),
            })
        }
        other => Err(S4Error::Protocol(format!("未知消息 tag {other}"))),
    }
}

// =====** 小工具 =====**

/// 读取 4 字节小端整数。
fn read_u32(bytes: &[u8]) -> Result<u32, S4Error> {
    bytes
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| S4Error::Protocol("u32 字段截断".into()))
}

/// 读取 8 字节小端整数。
fn read_u64(bytes: &[u8]) -> Result<u64, S4Error> {
    bytes
        .try_into()
        .map(u64::from_le_bytes)
        .map_err(|_| S4Error::Protocol("u64 字段截断".into()))
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_input::{KeyEvent, PointerEvent, WheelEvent};

    /// 输入事件往返一致。
    #[test]
    fn input_roundtrip() {
        let event = InputEvent::Pointer(PointerEvent {
            x: 100,
            y: 200,
            button: None,
            pressed: false,
        });
        let bytes = encode(&S4Message::Input(event.clone())).expect("编码");
        assert_eq!(decode(&bytes).expect("解码"), S4Message::Input(event));

        let key = InputEvent::Key(KeyEvent {
            scan_code: 0x1E,
            pressed: true,
            modifiers: 0,
        });
        let bytes = encode(&S4Message::Input(key.clone())).expect("编码");
        assert_eq!(decode(&bytes).expect("解码"), S4Message::Input(key));
    }

    /// 剪贴板与文件清单往返一致。
    #[test]
    fn clipboard_and_manifest_roundtrip() {
        let clipboard = S4Message::Clipboard(vec![0xAA; 40]);
        assert_eq!(
            decode(&encode(&clipboard).expect("编码")).expect("解码"),
            clipboard
        );

        let manifest = S4Message::FileManifest {
            transfer_id: 7,
            total_bytes: 5 << 20,
            chunk_size: 4 << 20,
            file_hash: [0x5A; 32],
        };
        assert_eq!(
            decode(&encode(&manifest).expect("编码")).expect("解码"),
            manifest
        );
    }

    /// 分片数据超过 1352 字节被拒。
    #[test]
    fn oversized_fragment_is_rejected() {
        let too_big = S4Message::FileFragment {
            transfer_id: 1,
            chunk_index: 0,
            frag_index: 0,
            frag_count: 1,
            data: vec![0u8; FILE_FRAGMENT_BYTES + 1],
        };
        assert!(matches!(encode(&too_big), Err(S4Error::Protocol(_))));
    }

    /// 畸形消息不 panic：空、未知 tag、截断、非法分片下标。
    #[test]
    fn malformed_messages_are_rejected() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[99]).is_err());
        assert!(decode(&[TAG_FILE_MANIFEST, 1, 2]).is_err());
        let mut bad_fragment = vec![TAG_FILE_FRAGMENT];
        bad_fragment.extend_from_slice(&1u32.to_le_bytes());
        bad_fragment.extend_from_slice(&0u32.to_le_bytes());
        bad_fragment.extend_from_slice(&5u32.to_le_bytes());
        bad_fragment.extend_from_slice(&5u32.to_le_bytes());
        assert!(decode(&bad_fragment).is_err(), "frag_index >= frag_count");
        assert!(decode(&[TAG_DONE, 1]).is_err());
    }

    /// 滚轮事件也走输入 tag。
    #[test]
    fn wheel_roundtrip() {
        let wheel = InputEvent::Wheel(WheelEvent {
            delta_x: -2,
            delta_y: 3,
        });
        let bytes = encode(&S4Message::Input(wheel.clone())).expect("编码");
        assert_eq!(decode(&bytes).expect("解码"), S4Message::Input(wheel));
    }
}
