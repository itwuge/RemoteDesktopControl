//! rdc-transport / 通道队列与优先级调度
//!
//! 层级：L2 能力层
//! 用途：8 条通道各自的等待队列与统一出队顺序（docs/04 §2 优先级表）。
//!       输入/控制插队，视频按 pacer 节拍铺开，文件与统计最低。
//! 调用方：rdc-core 的 `rdc-tx` 任务（每 1ms tick 取包）、测试工具

use std::collections::VecDeque;

use rdc_protocol::StreamKind;

/// 单通道队列容量（条）。
///
/// 256 条按 60fps、每帧约 4 个分片估算约 1 秒余量；超出即按通道策略丢弃或拒绝，
/// 任何队列都不允许无限增长（docs/04 §5.2）。
pub const QUEUE_CAPACITY: usize = 256;

/// 一条待发报文。
#[derive(Debug, Clone)]
pub struct QueuedPacket {
    /// 该通道内的单调序号，写入包头 `seq`。
    pub seq: u32,
    /// 采集/发送时间戳（微秒），写入包头 `ts_us`。
    pub ts_us: u64,
    /// 帧号，写入包头 `frame_id`。
    pub frame_id: u16,
    /// 明文载荷（不含包头）。
    pub payload: Vec<u8>,
}

/// 各通道的等待队列。
///
/// 线程约定：由 `rdc-tx` 单线程持有，不实现内部加锁。
pub struct StreamQueues {
    /// 按 [`StreamKind`] 的协议数值索引，长度恒为 8。
    queues: [VecDeque<QueuedPacket>; 8],
    /// 每通道容量上限。
    capacity: usize,
    /// 因满而丢弃的条数，按通道索引。
    dropped: [u64; 8],
}

impl StreamQueues {
    /// 构造空队列组。
    ///
    /// # Arguments
    /// * `capacity` - 每通道容量上限，传 0 会被提升为 1（容量为 0 的队列没有意义）
    ///
    /// # Returns
    /// 全部通道为空的实例。
    pub fn new(capacity: usize) -> Self {
        Self {
            queues: std::array::from_fn(|_| VecDeque::new()),
            capacity: capacity.max(1),
            dropped: [0; 8],
        }
    }

    /// 按通道表的优先级出队。
    ///
    /// 顺序：输入(0) → 剪贴板(1) → 控制(2) → 视频主/音频/视频子(3) → 文件(4) → 统计(5)。
    /// 同一优先级内按协议通道序号排列，保证结果可复现。
    ///
    /// # Returns
    /// 有包时返回所属通道与报文，全空返回 `None`。
    pub fn pop_next(&mut self) -> Option<(StreamKind, QueuedPacket)> {
        for stream in PRIORITY_ORDER {
            let index = stream.as_u8() as usize;
            if let Some(packet) = self.queues[index].pop_front() {
                return Some((stream, packet));
            }
        }
        None
    }

    /// 入队；满时按通道策略处理。
    ///
    /// 可丢通道（视频/音频/统计）丢最旧的一条并计入丢弃数；不可丢通道
    /// （输入/控制/剪贴板/文件）直接拒绝本次入队并计入丢弃数，由调用方降级处理。
    ///
    /// # Returns
    /// 入队成功返回 `true`；因队列满被拒绝或替换返回 `false`。
    pub fn push(&mut self, stream: StreamKind, packet: QueuedPacket) -> bool {
        let index = stream.as_u8() as usize;
        let queue = &mut self.queues[index];
        if queue.len() >= self.capacity {
            self.dropped[index] += 1;
            if stream.may_drop() {
                queue.pop_front();
                queue.push_back(packet);
            }
            return false;
        }
        queue.push_back(packet);
        true
    }

    /// 指定通道的当前队列长度。
    pub fn len(&self, stream: StreamKind) -> usize {
        self.queues[stream.as_u8() as usize].len()
    }

    /// 指定通道累计因满丢弃的条数。
    pub fn dropped(&self, stream: StreamKind) -> u64 {
        self.dropped[stream.as_u8() as usize]
    }

    /// 全部通道的待发总条数。
    pub fn total_len(&self) -> usize {
        self.queues.iter().map(VecDeque::len).sum()
    }
}

/// 出队顺序：先按优先级，再按协议通道序号。
const PRIORITY_ORDER: [StreamKind; 8] = [
    StreamKind::Input,
    StreamKind::Clipboard,
    StreamKind::Control,
    StreamKind::VideoMain,
    StreamKind::Audio,
    StreamKind::VideoSub,
    StreamKind::File,
    StreamKind::Stats,
];

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一条最小报文。
    fn packet(seq: u32) -> QueuedPacket {
        QueuedPacket {
            seq,
            ts_us: seq as u64,
            frame_id: 0,
            payload: vec![seq as u8],
        }
    }

    /// 输入事件必须压过视频：即使视频先入队，也要先出输入。
    #[test]
    fn input_preempts_video() {
        let mut queues = StreamQueues::new(8);
        assert!(queues.push(StreamKind::VideoMain, packet(1)));
        assert!(queues.push(StreamKind::Input, packet(2)));
        let (stream, packet) = queues.pop_next().expect("应有输入包");
        assert_eq!(stream, StreamKind::Input);
        assert_eq!(packet.seq, 2);
        let (stream, _) = queues.pop_next().expect("应有视频包");
        assert_eq!(stream, StreamKind::VideoMain);
    }

    /// 全优先级顺序必须与 docs/04 §2 的表一致。
    #[test]
    fn priority_order_matches_table() {
        let mut queues = StreamQueues::new(8);
        for stream in StreamKind::ALL {
            assert!(queues.push(stream, packet(stream.as_u8() as u32)));
        }
        let mut order = Vec::new();
        while let Some((stream, _)) = queues.pop_next() {
            order.push(stream);
        }
        assert_eq!(order, PRIORITY_ORDER.to_vec());
    }

    /// 视频队列满时丢最旧的一条（保留 I 帧由上层负责，这里只验证不无限增长）。
    #[test]
    fn droppable_queue_drops_oldest() {
        let mut queues = StreamQueues::new(2);
        assert!(queues.push(StreamKind::VideoMain, packet(1)));
        assert!(queues.push(StreamKind::VideoMain, packet(2)));
        assert!(!queues.push(StreamKind::VideoMain, packet(3)));
        assert_eq!(queues.len(StreamKind::VideoMain), 2);
        assert_eq!(queues.dropped(StreamKind::VideoMain), 1);
        let first = queues.pop_next().expect("应有视频包").1;
        assert_eq!(first.seq, 2, "最旧的 1 应被丢弃");
    }

    /// 输入队列满时拒绝新包而不是丢弃旧事件，保证不静默丢用户输入。
    #[test]
    fn undroppable_queue_rejects_new() {
        let mut queues = StreamQueues::new(2);
        assert!(queues.push(StreamKind::Input, packet(1)));
        assert!(queues.push(StreamKind::Input, packet(2)));
        assert!(!queues.push(StreamKind::Input, packet(3)));
        assert_eq!(queues.len(StreamKind::Input), 2);
        assert_eq!(queues.dropped(StreamKind::Input), 1);
        let first = queues.pop_next().expect("应有输入包").1;
        assert_eq!(first.seq, 1, "旧事件必须保留");
    }

    /// 容量 0 要被提升为 1，避免完全无法发送。
    #[test]
    fn zero_capacity_becomes_one() {
        let mut queues = StreamQueues::new(0);
        assert!(queues.push(StreamKind::Control, packet(1)));
        assert!(!queues.push(StreamKind::Control, packet(2)));
        assert_eq!(queues.len(StreamKind::Control), 1);
    }

    /// 空队列出队返回 None。
    #[test]
    fn empty_returns_none() {
        let mut queues = StreamQueues::new(4);
        assert!(queues.pop_next().is_none());
        assert_eq!(queues.total_len(), 0);
    }
}
