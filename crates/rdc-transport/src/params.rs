//! rdc-transport / 传输层常量
//!
//! 层级：L2 能力层
//! 用途：集中 FEC、NACK、队列与 pacer 的调参常量，避免魔法数字散落在各模块
//! 调用方：rdc-transport 内部各模块、rdc-core 的队列容量计算

// =====** 传输参数 =====**

/// 单个 FEC 组内最多包含的数据分片数。
///
/// 为什么是 32：组越大抗突发丢包越强，但解码延迟与冗余开销线性上升，
/// 32 是 30fps 视频帧内可接受延迟与恢复率的折中（docs/04 §3）。
pub const MAX_FEC_GROUP: usize = 32;

/// 每组默认附加的冗余分片数。
///
/// 取 2 可在随机丢包 5% 内维持 30fps 不花屏，同时不让固定开销超过 10%。
pub const DEFAULT_FEC_REDUNDANCY: u8 = 2;

/// NACK 重传缓存保留的序号数量。
///
/// 512 覆盖约 2 秒的 30fps 视频数据，足以等待一个 RTT 内的 NACK 回来。
pub const NACK_CACHE_LEN: usize = 512;

/// 发送队列输入缓冲的槽位数。
///
/// 256 槽按 60fps 计算约 4 秒缓冲；超过即丢弃并统计，避免延迟无限膨胀。
pub const QUEUE_INPUT_LEN: usize = 256;

/// pacer 定时器的最小调度粒度（毫秒）。
///
/// 1ms 让出包间隔足够细，避免码率低于 1Mbps 时出现整秒突刺。
pub const PACER_TICK_MS: u64 = 1;
