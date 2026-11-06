//! rdc-security / 失败退避与封禁计数
//!
//! 层级：L2 能力层（纯逻辑，时间由调用方传入）
//! 用途：实现 docs/03 §4 的防爆破策略：单 IP 失败退避 1→2→4→8→16→32→60 s，
//!       15 次/小时封禁 15 分钟；按目标设备维度 10 次/10 分钟封禁 30 分钟。
//! 调用方：rdc-signal（注册/呼叫/配对码校验）、rdc-core（UI 提示剩余等待时间）
//!
//! 为什么时间由调用方传入：便于单元测试用伪造时钟覆盖全部退避档位，
//! 避免 `sleep` 让测试变慢和不稳定。

use std::collections::{HashMap, VecDeque};

// =====** 策略 =====**

/// 退避与封禁策略参数。
#[derive(Debug, Clone)]
pub struct ThrottlePolicy {
    /// 连续失败对应的退避时长（毫秒），按失败次数依次取最后一档。
    pub backoff_ms: [i64; 7],
    /// 触发封禁的失败次数。
    pub ban_threshold: usize,
    /// 统计失败次数的滑动窗口（毫秒）。
    pub window_ms: i64,
    /// 封禁时长（毫秒）。
    pub ban_ms: i64,
}

impl Default for ThrottlePolicy {
    /// 默认策略：1/2/4/8/16/32/60 s，15 次/小时，封禁 15 分钟。
    fn default() -> Self {
        Self {
            backoff_ms: [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000],
            ban_threshold: 15,
            window_ms: 60 * 60 * 1000,
            ban_ms: 15 * 60 * 1000,
        }
    }
}

impl ThrottlePolicy {
    /// 按目标设备维度构造策略：10 次/10 分钟，封禁 30 分钟。
    ///
    /// # Returns
    /// 目标维度策略。
    ///
    /// # Errors
    /// 不会失败。
    pub fn target() -> Self {
        Self {
            backoff_ms: [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000],
            ban_threshold: 10,
            window_ms: 10 * 60 * 1000,
            ban_ms: 30 * 60 * 1000,
        }
    }
}

// =====** 拒绝原因 =====**

/// 限流拒绝原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleReject {
    /// 已被封禁，`until_ms` 之前拒绝。
    Banned {
        /// 解封时间（毫秒时间戳）。
        until_ms: i64,
    },
    /// 处于退避期，`retry_after_ms` 后允许重试。
    RetryAfter {
        /// 剩余等待毫秒。
        retry_after_ms: i64,
    },
}

/// 单个 key 的失败记录。
#[derive(Debug, Default)]
struct Entry {
    /// 连续失败次数（成功后清零）。
    consecutive: u32,
    /// 窗口内失败时间点。
    recent: VecDeque<i64>,
    /// 退避截止时间。
    blocked_until_ms: i64,
    /// 封禁截止时间。
    banned_until_ms: i64,
}

// =====** FailureLimiter =====**

/// 失败退避与封禁计数器。
#[derive(Debug)]
pub struct FailureLimiter {
    /// 策略。
    policy: ThrottlePolicy,
    /// 按 key（IP 或目标 ID）记录。
    entries: HashMap<String, Entry>,
}

impl FailureLimiter {
    /// 用默认策略创建。
    ///
    /// # Returns
    /// 空计数器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::with_policy(ThrottlePolicy::default())
    }

    /// 指定策略创建。
    ///
    /// # Arguments
    /// * `policy` - 退避/封禁参数
    ///
    /// # Returns
    /// 空计数器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn with_policy(policy: ThrottlePolicy) -> Self {
        Self {
            policy,
            entries: HashMap::new(),
        }
    }

    /// 检查某 key 当前是否允许尝试。
    ///
    /// # Arguments
    /// * `key` - IP 或目标 ID
    /// * `now_ms` - 当前毫秒时间戳
    ///
    /// # Returns
    /// 允许返回 `Ok(())`。
    ///
    /// # Errors
    /// 封禁中返回 [`ThrottleReject::Banned`]；退避中返回 [`ThrottleReject::RetryAfter`]。
    pub fn check_allowed(&self, key: &str, now_ms: i64) -> Result<(), ThrottleReject> {
        let Some(entry) = self.entries.get(key) else {
            return Ok(());
        };
        if entry.banned_until_ms > now_ms {
            return Err(ThrottleReject::Banned {
                until_ms: entry.banned_until_ms,
            });
        }
        if entry.blocked_until_ms > now_ms {
            return Err(ThrottleReject::RetryAfter {
                retry_after_ms: entry.blocked_until_ms - now_ms,
            });
        }
        Ok(())
    }

    /// 记录一次失败并推进退避/封禁状态。
    ///
    /// # Arguments
    /// * `key` - IP 或目标 ID
    /// * `now_ms` - 当前毫秒时间戳
    ///
    /// # Returns
    /// 记录后的拒绝原因（若本次失败直接触发封禁/退避），便于调用方返回
    /// `rate_limited` 并附带 `retry_after`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn record_failure(&mut self, key: &str, now_ms: i64) -> ThrottleReject {
        let policy = self.policy.clone();
        let entry = self.entries.entry(key.to_owned()).or_default();

        // 滑动窗口只保留窗口内的失败记录。
        while let Some(front) = entry.recent.front() {
            if now_ms - *front > policy.window_ms {
                entry.recent.pop_front();
            } else {
                break;
            }
        }
        entry.recent.push_back(now_ms);

        if entry.recent.len() >= policy.ban_threshold {
            // 达到阈值：立即封禁，并清空窗口避免解封后立刻再次触发。
            entry.banned_until_ms = now_ms + policy.ban_ms;
            entry.recent.clear();
            entry.consecutive = 0;
            entry.blocked_until_ms = 0;
            return ThrottleReject::Banned {
                until_ms: entry.banned_until_ms,
            };
        }

        entry.consecutive = entry.consecutive.saturating_add(1);
        // 取“连续失败次数-1”档；超过数组长度时用最后一档（60 s）。
        let idx = (entry.consecutive as usize - 1).min(policy.backoff_ms.len() - 1);
        entry.blocked_until_ms = now_ms + policy.backoff_ms[idx];
        ThrottleReject::RetryAfter {
            retry_after_ms: policy.backoff_ms[idx],
        }
    }

    /// 记录一次成功：清零该 key 的连续失败与退避状态。
    ///
    /// # Arguments
    /// * `key` - IP 或目标 ID
    ///
    /// # Returns
    /// 不会失败。
    pub fn record_success(&mut self, key: &str) {
        // 保留窗口内历史失败（防“成功一次后继续爆破”绕过阈值），只清零连续计数。
        if let Some(entry) = self.entries.get_mut(key) {
            entry.consecutive = 0;
            entry.blocked_until_ms = 0;
        }
    }

    /// 返回某 key 窗口内失败次数（测试与审计用）。
    ///
    /// # Arguments
    /// * `key` - 查询 key
    ///
    /// # Returns
    /// 窗口内失败次数；未知 key 返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn failures_in_window(&self, key: &str) -> usize {
        self.entries.get(key).map_or(0, |e| e.recent.len())
    }
}

impl Default for FailureLimiter {
    /// 默认策略。
    fn default() -> Self {
        Self::new()
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 退避必须按 1/2/4/8/16/32/60 s 递增。
    #[test]
    fn backoff_grows_exponentially() {
        let mut limiter = FailureLimiter::new();
        let expected = [1_000, 2_000, 4_000, 8_000, 16_000, 32_000, 60_000, 60_000];
        for (i, delay) in expected.iter().enumerate() {
            let now = 1_000_000 + i as i64 * 100_000;
            let reject = limiter.record_failure("1.2.3.4", now);
            assert_eq!(
                reject,
                ThrottleReject::RetryAfter {
                    retry_after_ms: *delay
                },
                "第 {} 次失败", i + 1
            );
        }
    }

    /// 退避期内必须拒绝，时间到后放行。
    #[test]
    fn backoff_blocks_until_deadline() {
        let mut limiter = FailureLimiter::new();
        limiter.record_failure("ip", 1000);
        assert!(matches!(
            limiter.check_allowed("ip", 1500),
            Err(ThrottleReject::RetryAfter { .. })
        ));
        assert!(limiter.check_allowed("ip", 2001).is_ok());
    }

    /// 15 次/小时必须封禁 15 分钟。
    #[test]
    fn fifteen_failures_trigger_ban() {
        let mut limiter = FailureLimiter::new();
        let mut now = 0;
        let mut banned_at = 0;
        for i in 0..15 {
            now = i as i64 * 5_000; // 全部落在 1 小时窗口内
            if let ThrottleReject::Banned { until_ms } = limiter.record_failure("ip", now) {
                banned_at = until_ms;
            }
        }
        assert_eq!(banned_at, now + 15 * 60 * 1000);
        assert!(matches!(
            limiter.check_allowed("ip", now + 1_000),
            Err(ThrottleReject::Banned { .. })
        ));
        assert!(limiter.check_allowed("ip", banned_at + 1).is_ok());
    }

    /// 成功一次不应清空窗口历史（防止“打一次成功继续爆破”绕过阈值）。
    #[test]
    fn success_resets_consecutive_but_keeps_window() {
        let mut limiter = FailureLimiter::new();
        limiter.record_failure("ip", 0);
        limiter.record_failure("ip", 1_000);
        assert_eq!(limiter.failures_in_window("ip"), 2);
        limiter.record_success("ip");
        assert_eq!(limiter.failures_in_window("ip"), 2);
        // 连续计数已清零，第 3 次失败应回到 1 s 退避。
        let reject = limiter.record_failure("ip", 2_000);
        assert_eq!(reject, ThrottleReject::RetryAfter { retry_after_ms: 1_000 });
    }

    /// 目标维度策略：10 次/10 分钟封禁 30 分钟。
    #[test]
    fn target_policy_bans_after_ten() {
        let mut limiter = FailureLimiter::with_policy(ThrottlePolicy::target());
        let mut last = ThrottleReject::RetryAfter { retry_after_ms: 0 };
        for i in 0..10 {
            last = limiter.record_failure("host", i as i64 * 1_000);
        }
        assert!(matches!(last, ThrottleReject::Banned { .. }));
    }
}
