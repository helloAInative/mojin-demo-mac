//! 东方财富 push2 接口：https://push2.eastmoney.com/api/qt/stock/get
//! 参数：secid=1.600460 (1=沪 / 0=深 / 116=北交) + fields=f43,f44,...
use super::{normalize_code, QuoteError};
use crate::model::Quote;
use chrono::Utc;
use serde::Deserialize;

pub struct EastMoney {
    /// 可在测试时覆盖的 base URL（默认 `https://push2.eastmoney.com`）
    pub base_url: String,
}

impl Default for EastMoney {
    fn default() -> Self {
        Self {
            base_url: "https://push2.eastmoney.com".to_string(),
        }
    }
}

impl EastMoney {
    pub async fn fetch_quote(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> Result<Quote, QuoteError> {
        let normalized = normalize_code(code)?;
        let market = if normalized.starts_with("sh") {
            "1"
        } else if normalized.starts_with("bj") {
            "116" // 北交
        } else {
            "0"
        };
        let pure = normalized.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let secid = format!("{}.{}", market, pure);
        let url = format!("{}/api/qt/stock/get", self.base_url);
        let resp = http
            .get(url)
            .query(&[
                ("secid", secid.as_str()),
                ("fields", "f43,f44,f45,f46,f47,f48,f57,f58,f60"),
                ("invt", "2"),
                ("fltt", "2"),
            ])
            .send()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(QuoteError::Network(format!("status {}", resp.status())));
        }
        let body: EMResp = resp
            .json()
            .await
            .map_err(|e| QuoteError::Parse(e.to_string()))?;
        if body.rc.unwrap_or(0) != 0 {
            return Err(QuoteError::Parse(format!("rc={:?}", body.rc)));
        }
        let d = body.data.ok_or(QuoteError::Empty)?;
        let div = |v: Option<i64>| v.unwrap_or(0) as f64 / 100.0;
        Ok(Quote {
            code: normalized.clone(),
            name: d.f58.unwrap_or_default(),
            price: div(d.f43),
            prev: div(d.f60),
            open: div(d.f46),
            high: div(d.f44),
            low: div(d.f45),
            volume: (d.f47.unwrap_or(0)) * 100, // 手 → 股
            amount: (d.f48.unwrap_or(0)) as f64,
            source: "eastmoney".into(),
            ts: Utc::now(),
        })
    }
}

#[derive(Debug, Deserialize)]
struct EMResp {
    data: Option<EMData>,
    rc: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct EMData {
    #[serde(rename = "f43")]
    f43: Option<i64>, // 最新价 *100
    #[serde(rename = "f44")]
    f44: Option<i64>, // 最高 *100
    #[serde(rename = "f45")]
    f45: Option<i64>, // 最低 *100
    #[serde(rename = "f46")]
    f46: Option<i64>, // 今开 *100
    #[serde(rename = "f60")]
    f60: Option<i64>, // 昨收 *100
    #[serde(rename = "f47")]
    f47: Option<i64>, // 成交量（手）
    #[serde(rename = "f48")]
    f48: Option<i64>, // 成交额（元）
    #[serde(rename = "f57")]
    f57: Option<String>, // 代码
    #[serde(rename = "f58")]
    f58: Option<String>, // 名称
}
