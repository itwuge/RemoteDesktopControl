//! rdc-connect / 信令客户端抽象
//!
//! 层级：L2 能力层
//! 用途：定义与信令服务交互的最小契约（注册、呼叫、应答、候选交换、中继票据、挂断），
//!       使 rdc-core 不依赖具体传输实现（wss / quic 由 rdc-signal 提供）
//! 调用方：rdc-core（连接编排）、rdc-ctl（由用户操作触发的呼叫与挂断）、
//!         rdc-signal 的实现（提供具体客户端）

use crate::{Candidate, ConnectError, ObservedAddr};

// =====** SignalingClient =====**

/// 信令客户端契约。
///
/// 线程模型：所有方法都是异步的，由 rdc-core 的网络任务（或 rdc-ctl 的命令任务）调用；
/// 实现方内部使用容量为 16 的有界 channel 把请求转交给单一网络线程，
/// 因此调用方不会因并发调用而阻塞整个运行时。
///
/// 为什么用 `async fn` 而不引入 async-trait：S1 只暴露契约，不产生 `dyn` 对象，
/// 原生 RPITIT 足够，可少一个第三方依赖。
#[allow(async_fn_in_trait)]
pub trait SignalingClient {
    /// 注册本端身份并取回公网观测地址。
    ///
    /// # Arguments
    /// * `id` - 本端设备标识
    /// * `public_key_b64` - 本端身份公钥的 Base64 编码
    ///
    /// # Returns
    /// 信令服务观测到的本端公网地址。
    ///
    /// # Errors
    /// 参数非法返回 [`ConnectError::InvalidArgument`]；信令不可达或超时返回
    /// [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn register(&mut self, id: &str, public_key_b64: &str)
        -> Result<ObservedAddr, ConnectError>;

    /// 向目标发起呼叫。
    ///
    /// # Arguments
    /// * `target` - 目标设备标识
    /// * `pairing_code` - 可选配对码，用于未互信设备首次连接
    /// * `allow_relay` - 是否允许对端回退中继
    ///
    /// # Returns
    /// 信令分配的本会话 ID。
    ///
    /// # Errors
    /// 配对码错误返回 [`ConnectError::PairingCodeInvalid`]；目标离线返回
    /// [`ConnectError::P2pFailed`]；信令不可达返回 [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn call(
        &mut self,
        target: &str,
        pairing_code: Option<&str>,
        allow_relay: bool,
    ) -> Result<String, ConnectError>;

    /// 接受对端的呼叫。
    ///
    /// # Arguments
    /// * `session_id` - 信令分配的会话 ID
    ///
    /// # Returns
    /// 成功时返回 `()`。
    ///
    /// # Errors
    /// 会话不存在或已超时返回 [`ConnectError::InvalidArgument`]；信令不可达返回
    /// [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn accept(&mut self, session_id: &str) -> Result<(), ConnectError>;

    /// 拒绝对端的呼叫。
    ///
    /// # Arguments
    /// * `session_id` - 信令分配的会话 ID
    /// * `reason` - 展示给对端的拒绝原因（不承载敏感信息）
    ///
    /// # Returns
    /// 成功时返回 `()`。
    ///
    /// # Errors
    /// 会话不存在返回 [`ConnectError::InvalidArgument`]；信令不可达返回
    /// [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn refuse(&mut self, session_id: &str, reason: &str) -> Result<(), ConnectError>;

    /// 上报本端候选地址列表。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    /// * `list` - 本端收集到的候选地址，按优先级排列
    ///
    /// # Returns
    /// 成功时返回 `()`。
    ///
    /// # Errors
    /// 列表为空或超长返回 [`ConnectError::InvalidArgument`]；信令不可达返回
    /// [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn candidates(&mut self, session_id: &str, list: &[Candidate])
        -> Result<(), ConnectError>;

    /// 申请中继票据。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    ///
    /// # Returns
    /// 中继服务所需的票据字符串。
    ///
    /// # Errors
    /// 中继未启用返回 [`ConnectError::RelayDisabled`]；票据签发失败返回
    /// [`ConnectError::Security`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn relay_ticket(&mut self, session_id: &str) -> Result<String, ConnectError>;

    /// 挂断会话。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    ///
    /// # Returns
    /// 成功时返回 `()`。
    ///
    /// # Errors
    /// 会话已结束返回 `Ok(())`（幂等）；信令不可达返回 [`ConnectError::Transport`]。
    ///
    /// 异步、由 rdc-ctl / 网络任务调用，内部有界 channel(16)。
    async fn bye(&mut self, session_id: &str) -> Result<(), ConnectError>;
}
