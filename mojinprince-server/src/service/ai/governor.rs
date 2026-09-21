//! Governor：按 (provider, model) 二元组做冷却 / 配额 / 失败计数。
//!
//! 设计目标：
//! 1. **冷却**：同一 (provider, model) 在 min_interval_ms 之内只允许一次调用。
//! 2. **配额**：每日 max_per_day 次；超出返回 429。
//! 3. **失败熔断**：连续 fail_threshold 次失败后短路 cooldown_ms，期内 fast-fail。
//!
//! 与 Swift 的 `NotifyGovernor` 思路一致；放在 Rust 端可以让所有调用方共享状态。
//! 线程安全：`parking_lot::Mutex` 包裹（这里为简洁仍用 std Mutex；并发量小够用）。
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct GovernorConfig {
    pub min_interval_ms: u64,
    pub max_per_day: u32,
    pub fail_threshold: u32,
    pub breaker_cooldown_ms: u64,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            min_interval_ms: 1_500,
            max_per_day: 500,
            fail_threshold: 5,
            breaker_cooldown_ms: 60_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenyReason {
    CoolingDown { wait_ms: u64 },
    DailyQuotaExceeded { used: u32, cap: u32 },
    BreakerOpen { retry_after_ms: u64 },
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DenyReason::CoolingDown { wait_ms } => {
                write!(f, "cooling_down: retry after {wait_ms}ms")
            }
            DenyReason::DailyQuotaExceeded { used, cap } => {
                write!(f, "daily_quota_exceeded: used={used} cap={cap}")
            }
            DenyReason::BreakerOpen { retry_after_ms } => {
                write!(f, "breaker_open: retry after {retry_after_ms}ms")
            }
        }
    }
}

impl DenyReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            DenyReason::CoolingDown { .. } => "cooling_down",
            DenyReason::DailyQuotaExceeded { .. } => "daily_quota_exceeded",
            DenyReason::BreakerOpen { .. } => "breaker_open",
        }
    }
    pub fn http_status(&self) -> u16 {
        match self {
            DenyReason::CoolingDown { .. } => 429,
            DenyReason::DailyQuotaExceeded { .. } => 429,
            DenyReason::BreakerOpen { .. } => 503,
        }
    }
}

#[derive(Default)]
struct BucketState {
    last_call_at: Option<Instant>,
    today_count: u32,
    today_day: Option<chrono::NaiveDate>,
    fail_streak: u32,
    breaker_until: Option<Instant>,
}

pub struct Governor {
    cfg: GovernorConfig,
    state: Mutex<HashMap<String, BucketState>>,
}

impl Governor {
    pub fn new(cfg: GovernorConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(HashMap::new()),
        }
    }

    /// 检查是否允许调用；不允许返回原因。
    pub fn check(&self, key: &str) -> Result<(), DenyReason> {
        let now = Instant::now();
        let today = chrono::Utc::now().date_naive();
        let mut g = self.state.lock().unwrap();
        let s = g.entry(key.to_string()).or_default();
        // 1) 熔断
        if let Some(until) = s.breaker_until {
            if until > now {
                return Err(DenyReason::BreakerOpen {
                    retry_after_ms: (until - now).as_millis() as u64,
                });
            } else {
                s.breaker_until = None;
            }
        }
        // 2) 日配额（按自然日，重置）
        if s.today_day != Some(today) {
            s.today_day = Some(today);
            s.today_count = 0;
        }
        if s.today_count >= self.cfg.max_per_day {
            return Err(DenyReason::DailyQuotaExceeded {
                used: s.today_count,
                cap: self.cfg.max_per_day,
            });
        }
        // 3) 冷却
        if let Some(last) = s.last_call_at {
            let elapsed = now.duration_since(last);
            if elapsed < Duration::from_millis(self.cfg.min_interval_ms) {
                return Err(DenyReason::CoolingDown {
                    wait_ms: self.cfg.min_interval_ms - elapsed.as_millis() as u64,
                });
            }
        }
        Ok(())
    }

    pub fn on_success(&self, key: &str) {
        let mut g = self.state.lock().unwrap();
        let s = g.entry(key.to_string()).or_default();
        s.last_call_at = Some(Instant::now());
        s.today_count = s.today_count.saturating_add(1);
        s.fail_streak = 0;
        s.breaker_until = None;
    }

    pub fn on_failure(&self, key: &str) {
        let mut g = self.state.lock().unwrap();
        let s = g.entry(key.to_string()).or_default();
        s.last_call_at = Some(Instant::now());
        s.today_count = s.today_count.saturating_add(1);
        s.fail_streak = s.fail_streak.saturating_add(1);
        if s.fail_streak >= self.cfg.fail_threshold {
            s.breaker_until =
                Some(Instant::now() + Duration::from_millis(self.cfg.breaker_cooldown_ms));
            s.fail_streak = 0;
        }
    }

    /// 仅供测试 / 运维：清掉某个 key 的状态。
    #[allow(dead_code)]
    pub fn reset(&self, key: &str) {
        self.state.lock().unwrap().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_blocks_immediate_repeat() {
        let g = Governor::new(GovernorConfig {
            min_interval_ms: 100,
            ..Default::default()
        });
        assert!(g.check("openai/qwen").is_ok());
        g.on_success("openai/qwen");
        let err = g.check("openai/qwen").unwrap_err();
        assert_eq!(err, DenyReason::CoolingDown { wait_ms: 100 });
    }

    #[test]
    fn breaker_opens_after_threshold() {
        let g = Governor::new(GovernorConfig {
            min_interval_ms: 0,
            fail_threshold: 2,
            breaker_cooldown_ms: 50,
            ..Default::default()
        });
        g.on_failure("p/m");
        g.on_failure("p/m");
        let err = g.check("p/m").unwrap_err();
        assert!(matches!(err, DenyReason::BreakerOpen { .. }));
    }
}
