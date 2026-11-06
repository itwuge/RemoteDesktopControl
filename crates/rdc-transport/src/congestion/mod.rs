//! rdc-transport / 拥塞控制抽象
//!
//! 层级：L2 能力层
//! 用途：定义拥塞控制种类与统一 trait，并提供按种类构造实例的工厂函数；
//!       GCC 估计与固定码率两种实现共享同一接口
//! 调用方：rdc-core 的发送任务（查询目标码率）与反馈任务（喂入 ACK / 丢包 / RTT）

// =====** 子模块 =====**

mod fixed;
mod gcc;

pub use fixed::FixedController;
pub use gcc::GccController;

// =====** CongestionKind =====**

/// 拥塞控制实现种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CongestionKind {
    /// 基于延迟与丢包梯度估计的 GCC 实现。
    Gcc,
    /// 固定码率，不做自适应（用于调试与极低带宽链路）。
    Fixed,
}

// =====** CongestionController =====**

/// 拥塞控制器接口。
///
/// 线程约定：所有方法都由 rdc-core 的单一网络任务线程调用，实现内部无需加锁；
/// 方法必须非阻塞——不得做 IO、不得 sleep、不得等待 channel，单次调用预算 < 50µs。
/// trait 要求 `Send` 是为了能把控制器移交给网络任务线程。
pub trait CongestionController: Send {
    /// 收到一批 ACK 后更新带宽估计。
    ///
    /// # Arguments
    /// * `batch` - 本批已确认的序号，按到达顺序排列
    ///
    /// # Returns
    /// 无返回值，结果通过 `target_bitrate` 体现。
    ///
    /// # Errors
    /// 不会失败；非法批量由实现内部忽略并计数。
    fn on_ack(&mut self, batch: &[u32]);

    /// 收到丢包反馈后更新带宽估计。
    ///
    /// # Arguments
    /// * `loss_rate` - 本轮观测到的丢包率，取值 `0.0..=1.0`
    /// * `burst_len` - 连续丢包的最大长度，用于区分随机丢包与拥塞丢包
    ///
    /// # Returns
    /// 无返回值，结果通过 `target_bitrate` 体现。
    ///
    /// # Errors
    /// 不会失败；越界的 `loss_rate` 由实现截断到合法区间。
    fn on_loss(&mut self, loss_rate: f32, burst_len: u32);

    /// 收到往返时延样本后更新延迟梯度。
    ///
    /// # Arguments
    /// * `rtt_ms` - 本轮平滑后的 RTT，单位毫秒
    ///
    /// # Returns
    /// 无返回值，结果通过 `target_bitrate` 体现。
    ///
    /// # Errors
    /// 不会失败。
    fn on_rtt(&mut self, rtt_ms: u32);

    /// 查询当前目标码率。
    ///
    /// # Returns
    /// 目标码率（bit/s），作为 pacer 与编码器的输入。
    ///
    /// # Errors
    /// 不会失败。
    fn target_bitrate(&self) -> u32;

    /// 收到带宽探测结果后修正估计。
    ///
    /// # Arguments
    /// * `delivered_bps` - 探测窗口内实测的交付码率（bit/s）
    ///
    /// # Returns
    /// 无返回值，结果通过 `target_bitrate` 体现。
    ///
    /// # Errors
    /// 不会失败。
    fn on_probe_result(&mut self, delivered_bps: u32);
}

// =====** create_controller =====**

/// 按种类构造拥塞控制器。
///
/// # Arguments
/// * `kind` - 需要的实现种类
/// * `initial_bps` - 初始目标码率（bit/s）
///
/// # Returns
/// 装箱后的控制器，交给网络任务线程独占持有。
///
/// # Errors
/// 不会失败；`initial_bps` 为 0 时由 pacer 在首次调度时报 [`crate::TransportError::InvalidRate`]。
pub fn create_controller(kind: CongestionKind, initial_bps: u32) -> Box<dyn CongestionController> {
    match kind {
        CongestionKind::Gcc => Box::new(GccController::new(initial_bps)),
        CongestionKind::Fixed => Box::new(FixedController::new(initial_bps)),
    }
}
