//! rdc-runtime · 线程与缓冲基础设施
//!
//! 层级：L2 能力层（不创建线程，只提供线程构造器与缓冲原语）
//! 用途：SPSC 环形缓冲、帧内存池、原子统计、线程命名/优先级/退出令牌/panic 隔离
//! 调用方：rdc-capture、rdc-codec、rdc-transport、rdc-input、rdc-core（docs/01 §3）

// =====** 模块声明 =====**

pub mod metrics;
pub mod pool;
pub mod ring;
pub mod stats;
pub mod thread;

// =====** 对外再导出 =====**

pub use metrics::SampleWindow;
pub use pool::{FramePool, PooledBuffer};
pub use ring::SpscRing;
pub use stats::AtomicCounters;
pub use thread::{spawn_named, ExitToken, ThreadPrio};
