//! rdc-protocol / 通道枚举
//!
//! 层级：L2 能力层
//! 用途：媒体与信令报文的通道标识；数值 0~4 与 docs/04 传输协议一致
//! 调用方：rdc-transport（分包/调度）、rdc-core（统计分通道）、rdc-relay（转发）

// =====** StreamKind =====**

/// 传输通道类型。
///
/// 数值是协议的一部分：新增通道必须追加在末尾并同步 docs/04，禁止插队或重排。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum StreamKind {
    /// 视频码流，受 pacer 与拥塞控制约束。
    Video = 0,
    /// 音频码流，低带宽、低延迟。
    Audio = 1,
    /// 输入事件，最高优先级、永不被视频阻塞（docs/01 T5）。
    Input = 2,
    /// 控制消息（握手、权限、重配）。
    Control = 3,
    /// 统计与反馈（ACK/NACK/丢包报告）。
    Stats = 4,
}

impl StreamKind {
    /// 返回协议约定的通道数值。
    ///
    /// # Returns
    /// 0~4 的 `u8`，直接写入报文头 `stream` 字段。
    ///
    /// # Errors
    /// 不会失败。
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// 从报文头数值还原通道。
    ///
    /// # Arguments
    /// * `value` - 报文头里的 `stream` 字段
    ///
    /// # Returns
    /// 已知通道返回 `Some`，未知数值返回 `None`（由调用方丢弃并计数）。
    ///
    /// # Errors
    /// 不会失败；未知值用 `None` 表达，避免 panic。
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Video),
            1 => Some(Self::Audio),
            2 => Some(Self::Input),
            3 => Some(Self::Control),
            4 => Some(Self::Stats),
            _ => None,
        }
    }
}
