//! rdc-transport / 帧重组
//!
//! 层级：L2 能力层
//! 用途：把同一帧的多个分片按序号拼回完整帧（docs/04 §1.2：重组窗口 4 帧，超时整帧丢弃）。
//! 调用方：rdc-core 的 `rdc-rx` 任务（收包解密后入重组）→ 抖动缓冲
//!
//! 说明：32 字节协议包头里没有 `shard_count`、也没有帧总长，因此每个分片载荷前
//! 额外携带 8 字节「媒体分片头」（本实现新增，已记入决策待办）。它属于载荷层，
//! 不改变包头线格式。

use std::collections::{HashMap, VecDeque};

use crate::error::TransportError;

/// 媒体分片头长度（字节）：`shard_index(u16) + shard_count(u16) + total_len(u32)`。
pub const FRAGMENT_HEADER_LEN: usize = 8;

// =====** FragmentHeader =====**

/// 分片载荷前的媒体分片头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FragmentHeader {
    /// 该分片在帧内的序号（从 0 开始）。
    pub shard_index: u16,
    /// 该帧的分片总数。
    pub shard_count: u16,
    /// 该帧的原始字节数（末片可能不足整片长度）。
    pub total_len: u32,
}

impl FragmentHeader {
    /// 编码为 8 字节（大端序）。
    pub fn encode(&self) -> [u8; FRAGMENT_HEADER_LEN] {
        let mut out = [0u8; FRAGMENT_HEADER_LEN];
        out[0..2].copy_from_slice(&self.shard_index.to_be_bytes());
        out[2..4].copy_from_slice(&self.shard_count.to_be_bytes());
        out[4..8].copy_from_slice(&self.total_len.to_be_bytes());
        out
    }

    /// 从载荷前缀解析。
    ///
    /// # Errors
    /// 长度不足 8 字节、分片总数为 0，或 `shard_index >= shard_count` 时返回
    /// [`TransportError::Internal`]（属于对端报文损坏，不是本地配置错误）。
    pub fn decode(bytes: &[u8]) -> Result<Self, TransportError> {
        if bytes.len() < FRAGMENT_HEADER_LEN {
            return Err(TransportError::Internal(format!(
                "媒体分片头长度不足: {}",
                bytes.len()
            )));
        }
        let header = Self {
            shard_index: u16::from_be_bytes([bytes[0], bytes[1]]),
            shard_count: u16::from_be_bytes([bytes[2], bytes[3]]),
            total_len: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        };
        if header.shard_count == 0 {
            return Err(TransportError::Internal("分片总数为 0".to_owned()));
        }
        if header.shard_index >= header.shard_count {
            return Err(TransportError::Internal(format!(
                "分片序号越界: {} >= {}",
                header.shard_index, header.shard_count
            )));
        }
        Ok(header)
    }
}

// =====** Reassembler =====**

/// 一帧的重组槽位。
struct Slot {
    /// 分片总数。
    count: u16,
    /// 原始帧字节数。
    total_len: u32,
    /// 各分片载荷（不含媒体分片头），`None` 表示尚未到达。
    shards: Vec<Option<Vec<u8>>>,
}

/// 按帧号重组分片。
pub struct Reassembler {
    /// 同时在等的最大帧数（docs/04 §1.2 定义为 4）。
    window: usize,
    /// 帧号 → 槽位。
    slots: HashMap<u16, Slot>,
    /// 等待顺序，用于窗口淘汰。
    order: VecDeque<u16>,
    /// 成功重组帧数。
    completed: u64,
    /// 窗口淘汰丢弃的帧数。
    dropped: u64,
}

impl Reassembler {
    /// 构造重组器。
    ///
    /// # Arguments
    /// * `window` - 同时在等的最大帧数，传 0 会被提升为 1
    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
            slots: HashMap::new(),
            order: VecDeque::new(),
            completed: 0,
            dropped: 0,
        }
    }

    /// 放入一个分片。
    ///
    /// # Arguments
    /// * `frame_id` - 包头里的帧号
    /// * `payload` - 包头之后的载荷，前 8 字节必须是 [`FragmentHeader`]
    ///
    /// # Returns
    /// 该帧凑齐时返回拼接好的完整帧（按 `total_len` 截断），否则 `None`。
    ///
    /// # Errors
    /// 媒体分片头非法时返回 [`TransportError::Internal`]。
    pub fn push(
        &mut self,
        frame_id: u16,
        payload: &[u8],
    ) -> Result<Option<Vec<u8>>, TransportError> {
        let header = FragmentHeader::decode(payload)?;
        let body = &payload[FRAGMENT_HEADER_LEN..];
        if body.len() > header.total_len as usize {
            return Err(TransportError::Internal(format!(
                "分片 {} 长度 {} 超过帧总长 {}",
                header.shard_index,
                body.len(),
                header.total_len
            )));
        }

        self.make_room(frame_id, header);

        let slot = self.slots.get_mut(&frame_id).expect("make_room 已确保槽位存在");
        let index = header.shard_index as usize;
        // 重复分片直接忽略：同一序号只保留最先到达的那份。
        if slot.shards[index].is_none() {
            slot.shards[index] = Some(body.to_vec());
        }

        if slot.shards.iter().all(Option::is_some) {
            let slot = self.slots.remove(&frame_id).expect("槽位存在");
            self.order.retain(|id| *id != frame_id);
            let mut out = Vec::with_capacity(slot.total_len as usize);
            for shard in slot.shards.iter().flatten() {
                out.extend_from_slice(shard);
            }
            out.truncate(slot.total_len as usize);
            self.completed += 1;
            return Ok(Some(out));
        }
        Ok(None)
    }

    /// 为新帧建立槽位；窗口满时淘汰最旧的未完成帧。
    fn make_room(&mut self, frame_id: u16, header: FragmentHeader) {
        let same_frame_change = self.slots.get(&frame_id).is_some_and(|slot| {
            slot.count != header.shard_count || slot.total_len != header.total_len
        });
        if same_frame_change {
            // 帧号回绕后复用：旧槽位已不可信，直接丢弃。
            self.slots.remove(&frame_id);
            self.order.retain(|id| *id != frame_id);
            self.dropped += 1;
        }
        if !self.slots.contains_key(&frame_id) {
            while self.slots.len() >= self.window {
                if let Some(oldest) = self.order.pop_front() {
                    if self.slots.remove(&oldest).is_some() {
                        self.dropped += 1;
                    }
                } else {
                    break;
                }
            }
            self.slots.insert(
                frame_id,
                Slot {
                    count: header.shard_count,
                    total_len: header.total_len,
                    shards: vec![None; header.shard_count as usize],
                },
            );
            self.order.push_back(frame_id);
        }
    }

    /// 成功重组的帧数。
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// 因窗口淘汰丢弃的帧数。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一个分片打包成「媒体分片头 + 数据」。
    fn fragment(index: u16, count: u16, total_len: u32, body: &[u8]) -> Vec<u8> {
        let header = FragmentHeader {
            shard_index: index,
            shard_count: count,
            total_len,
        };
        let mut out = header.encode().to_vec();
        out.extend_from_slice(body);
        out
    }

    /// 单分片帧应一次凑齐。
    #[test]
    fn single_shard_completes() {
        let mut reasm = Reassembler::new(4);
        let payload = fragment(0, 1, 10, &[1u8; 10]);
        assert_eq!(reasm.push(7, &payload).expect("合法分片"), Some(vec![1u8; 10]));
        assert_eq!(reasm.completed(), 1);
    }

    /// 三个分片乱序到达，最后一片到达时才返回完整帧，且顺序正确。
    #[test]
    fn out_of_order_shards_assemble_in_order() {
        let mut reasm = Reassembler::new(4);
        let mut expected = Vec::new();
        for index in 0..3u16 {
            expected.extend_from_slice(&[index as u8; 4]);
        }
        assert_eq!(reasm.push(9, &fragment(2, 3, expected.len() as u32, &[2u8; 4])).unwrap(), None);
        assert_eq!(reasm.push(9, &fragment(0, 3, expected.len() as u32, &[0u8; 4])).unwrap(), None);
        let complete = reasm
            .push(9, &fragment(1, 3, expected.len() as u32, &[1u8; 4]))
            .unwrap()
            .expect("应凑齐");
        assert_eq!(complete, expected);
    }

    /// 重复分片不影响最终结果。
    #[test]
    fn duplicate_shard_is_ignored() {
        let mut reasm = Reassembler::new(2);
        let total = 8u32;
        assert!(reasm.push(1, &fragment(0, 2, total, &[1u8; 4])).unwrap().is_none());
        // 重复的第 0 片内容不同，必须保留先到的那份。
        assert!(reasm.push(1, &fragment(0, 2, total, &[9u8; 4])).unwrap().is_none());
        let complete = reasm.push(1, &fragment(1, 2, total, &[2u8; 4])).unwrap().unwrap();
        assert_eq!(complete, vec![1u8, 1, 1, 1, 2, 2, 2, 2]);
    }

    /// 末片多余字节按 `total_len` 截断。
    #[test]
    fn total_len_trims_padding() {
        let mut reasm = Reassembler::new(2);
        assert!(reasm.push(3, &fragment(0, 2, 5, &[1u8; 4])).unwrap().is_none());
        let complete = reasm.push(3, &fragment(1, 2, 5, &[2u8; 4])).unwrap().unwrap();
        assert_eq!(complete, vec![1u8, 1, 1, 1, 2]);
    }

    /// 窗口超出时淘汰最旧的未完成帧，避免内存无限增长。
    #[test]
    fn window_evicts_oldest_incomplete_frame() {
        let mut reasm = Reassembler::new(2);
        assert!(reasm.push(1, &fragment(0, 2, 8, &[1u8; 4])).unwrap().is_none());
        assert!(reasm.push(2, &fragment(0, 2, 8, &[2u8; 4])).unwrap().is_none());
        assert!(reasm.push(3, &fragment(0, 2, 8, &[3u8; 4])).unwrap().is_none());
        assert_eq!(reasm.dropped(), 1, "帧 1 应被淘汰");
        // 帧 1 之后再补片也不会被接收（槽位已不存在），补完只影响新槽位。
        assert!(reasm.push(2, &fragment(1, 2, 8, &[2u8; 4])).unwrap().is_some());
    }

    /// 非法分片头必须报错，而不是静默吞掉。
    #[test]
    fn invalid_fragment_header_is_rejected() {
        let mut reasm = Reassembler::new(2);
        assert!(reasm.push(1, &[0u8; 4]).is_err(), "长度不足");
        assert!(reasm.push(1, &fragment(0, 0, 10, &[0u8; 4])).is_err(), "总数为 0");
        assert!(reasm.push(1, &fragment(2, 2, 10, &[0u8; 4])).is_err(), "序号越界");
        assert!(reasm.push(1, &fragment(0, 2, 2, &[0u8; 4])).is_err(), "分片超总长");
    }
}
