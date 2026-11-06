//! rdc-protocol / 通道枚举与优先级表
//!
//! 层级：L2 能力层
//! 用途：媒体/控制报文的通道标识、优先级、带宽与丢包策略的唯一真源（docs/04 §2）。
//! 调用方：rdc-transport（调度与 pacer）、rdc-connect（控制通道）、rdc-relay（队列调度）、
//!         rdc-core（分通道统计）
//!
//! 纪律：数值 0~7 是协议的一部分。新增通道只能追加在末尾并同步 docs/04，
//! 禁止插队或重排；未知通道必须“整包丢弃 + 计数”，不得报错或断连。

// =====** StreamKind =====**

/// 传输通道类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u8)]
pub enum StreamKind {
    /// 0：视频主码流，受 pacer 与拥塞控制约束。
    VideoMain = 0,
    /// 1：音频（预留）。
    Audio = 1,
    /// 2：输入事件（鼠标/键盘/触摸），P0 插队，永不因视频阻塞。
    Input = 2,
    /// 3：控制消息（握手、编码重配、权限位），必须可靠送达。
    Control = 3,
    /// 4：统计与反馈（ACK/NACK/丢包报告/心跳），可丢。
    Stats = 4,
    /// 5：剪贴板（文本 ≤1 MB、图片 ≤10 MB/条，瞬时限流 2 MB/s）。
    Clipboard = 5,
    /// 6：文件传输（默认 ≤可用带宽 15%）。
    File = 6,
    /// 7：视频子码流（屏幕墙，0.2~0.4 Mbps/路）。
    VideoSub = 7,
}

impl StreamKind {
    /// 全部已定义通道，按协议顺序排列，供表驱动测试与统计初始化。
    pub const ALL: [StreamKind; 8] = [
        StreamKind::VideoMain,
        StreamKind::Audio,
        StreamKind::Input,
        StreamKind::Control,
        StreamKind::Stats,
        StreamKind::Clipboard,
        StreamKind::File,
        StreamKind::VideoSub,
    ];

    /// 返回协议约定的通道数值。
    ///
    /// # Returns
    /// 0~7 的 `u8`，直接写入报文头 `stream` 字段。
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
    /// 已知通道返回 `Some`，未知数值返回 `None`（调用方整包丢弃 + 计数
    /// `unknown_stream_drops`，不报错不断连）。
    ///
    /// # Errors
    /// 不会失败；未知值用 `None` 表达，避免 panic。
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::VideoMain),
            1 => Some(Self::Audio),
            2 => Some(Self::Input),
            3 => Some(Self::Control),
            4 => Some(Self::Stats),
            5 => Some(Self::Clipboard),
            6 => Some(Self::File),
            7 => Some(Self::VideoSub),
            _ => None,
        }
    }

    /// 返回调度优先级（0 最高，5 最低）。
    ///
    /// 映射依据 docs/04 §2：输入 0 / 剪贴板 1 / 控制 2 / 视频·子码流·音频 3 /
    /// 文件 4 / 统计 5。
    ///
    /// # Returns
    /// 0~5 的优先级数值，写入报文头 `priority` 字段。
    ///
    /// # Errors
    /// 不会失败。
    pub fn priority(self) -> u8 {
        match self {
            Self::Input => 0,
            Self::Clipboard => 1,
            Self::Control => 2,
            Self::VideoMain | Self::VideoSub | Self::Audio => 3,
            Self::File => 4,
            Self::Stats => 5,
        }
    }

    /// 是否为视频类通道（受 pacer 令牌桶约束）。
    ///
    /// # Returns
    /// 主码流或子码流返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_video(self) -> bool {
        matches!(self, Self::VideoMain | Self::VideoSub)
    }

    /// 是否允许丢弃。
    ///
    /// 输入/控制/剪贴板/文件不可丢（靠重传），视频/音频/统计可丢（靠 FEC + NACK）。
    /// 该判定被 pacer 与重传队列共同使用，避免两处各写一份规则。
    ///
    /// # Returns
    /// 允许丢弃返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn may_drop(self) -> bool {
        matches!(self, Self::VideoMain | Self::VideoSub | Self::Audio | Self::Stats)
    }
}

// =====** 单元测试：冻结通道表 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 通道数值是线上协议，必须与 docs/04 完全一致。
    #[test]
    fn stream_numbers_are_frozen() {
        assert_eq!(StreamKind::VideoMain.as_u8(), 0);
        assert_eq!(StreamKind::Audio.as_u8(), 1);
        assert_eq!(StreamKind::Input.as_u8(), 2);
        assert_eq!(StreamKind::Control.as_u8(), 3);
        assert_eq!(StreamKind::Stats.as_u8(), 4);
        assert_eq!(StreamKind::Clipboard.as_u8(), 5);
        assert_eq!(StreamKind::File.as_u8(), 6);
        assert_eq!(StreamKind::VideoSub.as_u8(), 7);
        for kind in StreamKind::ALL {
            assert_eq!(StreamKind::from_u8(kind.as_u8()), Some(kind));
        }
    }

    /// 未知通道返回 None，调用方据此整包丢弃。
    #[test]
    fn unknown_stream_is_none() {
        assert_eq!(StreamKind::from_u8(8), None);
        assert_eq!(StreamKind::from_u8(255), None);
    }

    /// 优先级必须符合表：输入插队、统计最低。
    #[test]
    fn priorities_match_document() {
        assert_eq!(StreamKind::Input.priority(), 0);
        assert_eq!(StreamKind::Clipboard.priority(), 1);
        assert_eq!(StreamKind::Control.priority(), 2);
        assert_eq!(StreamKind::VideoMain.priority(), 3);
        assert_eq!(StreamKind::VideoSub.priority(), 3);
        assert_eq!(StreamKind::Audio.priority(), 3);
        assert_eq!(StreamKind::File.priority(), 4);
        assert_eq!(StreamKind::Stats.priority(), 5);
    }

    /// 可丢性规则必须与表一致，防止输入事件被当作可丢包。
    #[test]
    fn drop_policy_matches_document() {
        assert!(!StreamKind::Input.may_drop());
        assert!(!StreamKind::Control.may_drop());
        assert!(!StreamKind::Clipboard.may_drop());
        assert!(!StreamKind::File.may_drop());
        assert!(StreamKind::VideoMain.may_drop());
        assert!(StreamKind::VideoSub.may_drop());
        assert!(StreamKind::Audio.may_drop());
        assert!(StreamKind::Stats.may_drop());
    }
}
