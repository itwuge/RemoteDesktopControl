//! rdc-core / S4-2 剪贴板任务
//!
//! 层级：L3 编排层
//! 用途：把本机剪贴板文本变化编码后发到会话通道；把远端载荷写回本机剪贴板。
//!       去重与回环判定全部复用 `rdc-clipboard::ClipboardSync`，本层只负责通道与平台读写。
//! 调用方：rdc-core 会话主流程、rdc-cli 的 `demo-clipboard`。
//!
//! 线程：任务对象由单个任务持有，无锁；平台实现要求同一线程调用。

use rdc_clipboard::{Clipboard, ClipboardSync, SyncStats};
use rdc_connect::EstablishedSession;

use crate::s4::message::S4Message;
use crate::s4::{recv, send, S4Error};

// =====** ClipboardTask =====**

/// 剪贴板任务：持有平台剪贴板与同步状态机。
pub struct ClipboardTask {
    /// 平台剪贴板读写。
    clipboard: Box<dyn Clipboard>,
    /// 去重与回环防护。
    sync: ClipboardSync,
}

impl ClipboardTask {
    /// 构造任务（以当前剪贴板序列号为基线，避免把历史内容当成新变化）。
    ///
    /// # Returns
    /// 尚未发送/应用任何内容的任务；平台不支持时各方法返回 `Unsupported`。
    pub fn new() -> Self {
        Self {
            clipboard: rdc_clipboard::create_clipboard(),
            sync: ClipboardSync::new(),
        }
    }

    /// 本机写入文本并尝试外发。
    ///
    /// # Arguments
    /// * `session` - 已建立的安全会话
    /// * `text` - 要写入本机剪贴板的文本
    ///
    /// # Returns
    /// `true` 表示本次变化已外发；`false` 表示被去重/回环防护挡下。
    ///
    /// # Errors
    /// 写本机剪贴板失败、文本超限或通道发送失败时返回 [`S4Error`]。
    pub async fn publish(
        &mut self,
        session: &mut EstablishedSession,
        text: &str,
    ) -> Result<bool, S4Error> {
        self.clipboard.write_text(text)?;
        let Some(payload) = self.sync.on_local_text(text)? else {
            return Ok(false);
        };
        send(session, &S4Message::Clipboard(payload)).await?;
        Ok(true)
    }

    /// 接收并应用一条远端剪贴板消息。
    ///
    /// # Returns
    /// 实际写回本机的文本；重复或回环时为 `None`。
    ///
    /// # Errors
    /// 收到非剪贴板消息、载荷损坏或写本机剪贴板失败时返回 [`S4Error`]。
    pub async fn apply_remote(
        &mut self,
        session: &mut EstablishedSession,
    ) -> Result<Option<String>, S4Error> {
        let payload = match recv(session).await? {
            S4Message::Clipboard(payload) => payload,
            other => {
                return Err(S4Error::Protocol(format!(
                    "剪贴板通道收到非剪贴板消息: {other:?}"
                )))
            }
        };
        let Some(text) = self.sync.on_remote_payload(&payload)? else {
            return Ok(None);
        };
        self.clipboard.write_text(&text)?;
        Ok(Some(text))
    }

    /// 直接读取本机剪贴板文本（不参与回环判定，用于演示的“读回校验”）。
    ///
    /// # Errors
    /// 平台读失败返回 [`S4Error::Clipboard`]。
    pub fn read(&mut self) -> Result<Option<String>, S4Error> {
        Ok(self.clipboard.read_text()?)
    }

    /// 同步计数快照。
    pub fn stats(&self) -> SyncStats {
        self.sync.stats()
    }
}

impl Default for ClipboardTask {
    fn default() -> Self {
        Self::new()
    }
}
