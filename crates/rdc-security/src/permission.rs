//! rdc-security / 权限位集合
//!
//! 层级：L2 能力层
//! 用途：用 u32 位图表达「键盘 / 指针 / 剪贴板文本 / 文件传输 / 音频」五类授权，
//!       位定义是协议的一部分（与 docs/03 权限开关一致）
//! 调用方：rdc-core（会话授权校验）、rdc-bridge（权限开关的 FFI 映射）、
//!         rdc-connect（握手时报文携带权限位）

use serde::{Deserialize, Serialize};

// =====** PermissionSet =====**

/// 权限位集合。
///
/// 默认值为 `0`（无任何权限），符合「最小授权」原则：未显式开启即拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(transparent)]
pub struct PermissionSet(
    /// 位图原始值：第 n 位对应一类权限，可直接写入握手报文。
    pub u32,
);

impl PermissionSet {
    /// 键盘 / 文本输入权限。
    pub const KEYBOARD: u32 = 1 << 0;
    /// 鼠标指针控制权限。
    pub const POINTER: u32 = 1 << 1;
    /// 剪贴板文本互通权限。
    pub const CLIPBOARD_TEXT: u32 = 1 << 2;
    /// 文件传输权限。
    pub const FILE_TRANSFER: u32 = 1 << 3;
    /// 音频播放权限。
    pub const AUDIO: u32 = 1 << 4;
    /// 当前定义的全部权限位。
    ///
    /// 为什么显式相或而不是 `(1 << 5) - 1`：新增位时必须同步修改这里，
    /// 避免旧代码误把未定义的位当作已授权。
    pub const ALL: u32 =
        Self::KEYBOARD | Self::POINTER | Self::CLIPBOARD_TEXT | Self::FILE_TRANSFER | Self::AUDIO;

    /// 判断是否包含指定权限。
    ///
    /// # Arguments
    /// * `mask` - 单个权限位或多位组合的掩码
    ///
    /// # Returns
    /// `mask` 中每一位都已置位时返回 `true`；`mask` 为 0 时返回 `true`（空集恒真）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn contains(self, mask: u32) -> bool {
        self.0 & mask == mask
    }

    /// 求两个权限集合的交集。
    ///
    /// # Arguments
    /// * `other` - 另一个权限集合
    ///
    /// # Returns
    /// 仅保留双方都授予的位。
    ///
    /// # Errors
    /// 不会失败。
    pub fn intersect(self, other: PermissionSet) -> PermissionSet {
        PermissionSet(self.0 & other.0)
    }

    /// 导出位图原始值。
    ///
    /// # Returns
    /// u32 位图，可直接写入握手报文。
    ///
    /// # Errors
    /// 不会失败。
    pub fn to_bits(self) -> u32 {
        self.0
    }
}
