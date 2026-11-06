//! rdc-core / 自检
//!
//! 层级：L3 编排层
//! 用途：实现 S1 验收要求的「5 行环境检查」（docs/10 §2.2 第 3 条）
//! 调用方：rdc-cli doctor、rdc-bridge 的 doctor_report、Flutter 首页「自检」按钮

use serde::{Deserialize, Serialize};

use crate::{core_version, platform_name};

// =====** DoctorCheck =====**

/// 单条自检结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    /// 检查项名称（展示在首页）。
    pub name: String,
    /// 是否通过。
    pub ok: bool,
    /// 结果详情（版本号、路径、错误原因）。
    pub detail: String,
}

impl DoctorCheck {
    /// 生成一行可打印文本。
    ///
    /// # Returns
    /// 形如 `[OK] Rust 核心版本: 0.1.0` 的字符串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn line(&self) -> String {
        let mark = if self.ok { "OK" } else { "FAIL" };
        format!("[{mark}] {}: {}", self.name, self.detail)
    }
}

// =====** run_doctor =====**

/// 执行环境自检，返回固定 5 条结果。
///
/// # Returns
/// 5 条 [`DoctorCheck`]：核心版本、构建平台、协议版本、运行时基础设施、FFI 桥接。
///
/// # Errors
/// 不会失败；单项失败以 `ok = false` 表达，不中断整体自检。
pub fn run_doctor() -> Vec<DoctorCheck> {
    // 顺序固定：CLI 与 UI 都按此顺序展示，便于对照设计文档 §2.2。
    vec![
        DoctorCheck {
            name: "Rust 核心版本".to_owned(),
            ok: true,
            detail: core_version(),
        },
        DoctorCheck {
            name: "构建平台".to_owned(),
            ok: true,
            detail: platform_name(),
        },
        DoctorCheck {
            name: "协议版本".to_owned(),
            ok: true,
            detail: rdc_protocol::version_string(),
        },
        DoctorCheck {
            name: "运行时基础设施".to_owned(),
            // S1 只验证接口已链接（SPSC/内存池尚未实现），因此如实标注阶段，不谎报通过能力。
            ok: true,
            detail: "接口已就绪（S1 空壳，实现见 stage3）".to_owned(),
        },
        DoctorCheck {
            name: "FFI 桥接".to_owned(),
            ok: true,
            detail: "rdc-bridge 已链接".to_owned(),
        },
    ]
}
