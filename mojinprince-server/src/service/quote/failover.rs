//! Failover 调度：
//! - 优先按 sources 顺序逐个尝试
//! - 每个 provider 累计失败次数，连续 N 次后熔断（暂时跳过 60s，N 来自 cfg.quote_fail_threshold）
//! - 返回第一个成功结果；全部失败则报 UpstreamExhausted
use super::{QuoteError, QuoteProviders};
use crate::error::{AppError, AppResult};
use crate::model::Quote;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct QuoteFailover {
    providers: Vec<QuoteProviders>,
    order: Vec<String>,
    fail_threshold: u32,
    fail_counter: Mutex<HashMap<String, u32>>,
    breaker: Mutex<HashMap<String, std::time::Instant>>,
    breaker_window_sec: u64,
}

impl QuoteFailover {
    pub fn new(order: Vec<String>, fail_threshold: u32) -> Self {
        Self::with_providers(
            vec![
                QuoteProviders::Sina(super::Sina::default()),
                QuoteProviders::Tencent(super::Tencent::default()),
                QuoteProviders::EastMoney(super::EastMoney::default()),
            ],
            order,
            fail_threshold,
        )
    }

    /// 自定义 provider 集合（base_url 可改），主要给测试用
    pub fn with_providers(
        providers: Vec<QuoteProviders>,
        order: Vec<String>,
        fail_threshold: u32,
    ) -> Self {
        Self {
            providers,
            order,
            fail_threshold,
            fail_counter: Mutex::new(HashMap::new()),
            breaker: Mutex::new(HashMap::new()),
            breaker_window_sec: 60,
        }
    }

    fn inc_fail(&self, name: &str) {
        let mut m = self.fail_counter.lock().unwrap();
        let c = m.entry(name.into()).or_insert(0);
        *c += 1;
        if *c >= self.fail_threshold {
            let mut b = self.breaker.lock().unwrap();
            b.insert(
                name.into(),
                std::time::Instant::now() + std::time::Duration::from_secs(self.breaker_window_sec),
            );
            *c = 0;
        }
    }

    fn is_open(&self, name: &str) -> bool {
        let b = self.breaker.lock().unwrap();
        match b.get(name) {
            Some(t) if *t > std::time::Instant::now() => true,
            Some(_) => false,
            None => false,
        }
    }

    fn reset(&self, name: &str) {
        let mut m = self.fail_counter.lock().unwrap();
        m.remove(name);
        let mut b = self.breaker.lock().unwrap();
        b.remove(name);
    }

    fn find(&self, name: &str) -> Option<&QuoteProviders> {
        self.providers.iter().find(|p| p.name() == name)
    }

    /// 按 order 顺序尝试，未熔断的优先；返回第一个成功结果或 UpstreamExhausted。
    pub async fn fetch_quote(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> AppResult<(Quote, &'static str)> {
        let mut last_err: Option<QuoteError> = None;
        let mut tries = 0u32;

        // 1. 先按用户配置的 order 顺序
        for src in &self.order {
            if self.is_open(src) {
                continue;
            }
            let Some(p) = self.find(src) else { continue };
            tries += 1;
            match p.fetch_quote(http, code).await {
                Ok(q) => {
                    self.reset(src);
                    return Ok((q, p.name()));
                }
                Err(QuoteError::BadCode(bad)) => {
                    // 用户代码不合法：不算上游失败，直接 400
                    return Err(AppError::BadRequest(format!("bad code: {bad}").into()));
                }
                Err(e) => {
                    tracing::warn!(provider = src, code, err = %e, "fetch failed");
                    self.inc_fail(src);
                    last_err = Some(e);
                }
            }
        }
        // 2. 兜底尝试未在配置中的 provider
        for p in &self.providers {
            let name = p.name();
            if self.order.iter().any(|s| s == name) {
                continue;
            }
            if self.is_open(name) {
                continue;
            }
            tries += 1;
            match p.fetch_quote(http, code).await {
                Ok(q) => {
                    self.reset(name);
                    return Ok((q, name));
                }
                Err(QuoteError::BadCode(bad)) => {
                    return Err(AppError::BadRequest(format!("bad code: {bad}").into()));
                }
                Err(e) => {
                    self.inc_fail(name);
                    last_err = Some(e);
                }
            }
        }
        let msg = match last_err {
            Some(e) => e.to_string(),
            None => "all sources skipped (breaker open)".into(),
        };
        Err(AppError::UpstreamExhausted {
            tries,
            source: msg.into(),
        })
    }
}
