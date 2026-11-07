//! rdc-core / S4-1 输入任务
//!
//! 层级：L3 编排层
//! 用途：主控端把 [`InputEvent`] 编成消息发到会话通道；被控端按序解码、注入并回传统计。
//! 调用方：rdc-core 会话主流程、rdc-cli 的 `demo-input` / `demo-keys`。
//!
//! 时序：同一会话通道是**有序**的，因此“移动在前、按键在后”的次序天然成立；
//! 发送端不丢事件（失败直接返回错误，由调用方决定是否终止会话）。

use std::time::Duration;

use rdc_connect::EstablishedSession;
use rdc_input::{InputEvent, InputInjector};

use crate::s4::message::S4Message;
use crate::s4::{recv, send, S4Error};

// =====** InputCounters =====**

/// 输入任务计数（便于日志与演示断言）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputCounters {
    /// 本端发出的输入事件数。
    pub sent: u32,
    /// 本端注入成功的输入事件数。
    pub injected: u32,
}

// =====** 主控端 =====**

/// 主控端：发送一批输入事件，再发结束标记，最后等待被控端汇总。
///
/// # Arguments
/// * `session` - 已建立的安全会话
/// * `events` - 已合并/限频后的输入事件（顺序即注入顺序）
///
/// # Returns
/// 被控端回报的注入条数。
///
/// # Errors
/// 发送失败、未收到汇总或汇总条数与发出条数不一致时返回 [`S4Error`]。
pub async fn run_ctrl(
    session: &mut EstablishedSession,
    events: &[InputEvent],
) -> Result<InputCounters, S4Error> {
    for event in events {
        send(session, &S4Message::Input(event.clone())).await?;
    }
    send(session, &S4Message::Done).await?;
    let injected = match recv(session).await? {
        S4Message::InputSummary(count) => count,
        other => {
            return Err(S4Error::Protocol(format!(
                "期望 InputSummary，收到 {other:?}"
            )))
        }
    };
    if injected as usize != events.len() {
        return Err(S4Error::Protocol(format!(
            "注入条数 {} 与发出条数 {} 不一致",
            injected,
            events.len()
        )));
    }
    Ok(InputCounters {
        sent: events.len() as u32,
        injected,
    })
}

// =====** 被控端 =====**

/// 被控端：按序注入输入事件，直到收到 [`S4Message::Done`]，再回传注入条数。
///
/// # Arguments
/// * `session` - 已建立的安全会话
/// * `injector` - 平台注入器（已完成视口设置）
/// * `max_events` - 安全上限，防止对端无限发事件占满会话
/// * `pace` - 每条注入后的间隔（`None` 表示不等待），供演示逐条采样 `GetCursorPos`
/// * `on_injected` - 每次注入并等待后回调（演示用于 `GetCursorPos` 采样）
///
/// # Returns
/// 实际注入条数。
///
/// # Errors
/// 收到非输入消息、注入失败、超过 `max_events` 或通道失败时返回 [`S4Error`]。
pub async fn run_host<F>(
    session: &mut EstablishedSession,
    injector: &mut dyn InputInjector,
    max_events: u32,
    pace: Option<Duration>,
    mut on_injected: F,
) -> Result<u32, S4Error>
where
    F: FnMut(&InputEvent),
{
    let mut injected = 0u32;
    loop {
        match recv(session).await? {
            S4Message::Input(event) => {
                if injected >= max_events {
                    return Err(S4Error::Protocol(format!("输入事件超过上限 {max_events}")));
                }
                injector.inject(event.clone())?;
                if let Some(pace) = pace {
                    tokio::time::sleep(pace).await;
                }
                injected += 1;
                on_injected(&event);
            }
            S4Message::Done => break,
            other => {
                return Err(S4Error::Protocol(format!(
                    "输入通道收到非输入消息: {other:?}"
                )))
            }
        }
    }
    send(session, &S4Message::InputSummary(injected)).await?;
    Ok(injected)
}
