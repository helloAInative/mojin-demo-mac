//! 东财逐笔成交明细（B.4：分时下方逐笔桶 + 下钻数据源）。
//! push2delay 的 details 接口：`HH:MM:SS,价格,量(手),方向,笔数`。
use super::{normalize_code, QuoteError};
use chrono::{FixedOffset, TimeZone, Utc};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct EastMoneyTicks {
    /// 测试可覆写（默认 `https://push2delay.eastmoney.com`）
    pub base_url: String,
}

impl Default for EastMoneyTicks {
    fn default() -> Self {
        Self {
            base_url: "https://push2delay.eastmoney.com".into(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct TickItem {
    /// 成交时间（北京时间转 UTC）
    pub ts: chrono::DateTime<Utc>,
    pub price: f64,
    /// 手
    pub volume: i64,
    /// 1 买盘 / 2 卖盘 / 4 中性（集合竞价为 0）
    pub direction: i64,
}

impl EastMoneyTicks {
    /// 拉最近 `limit` 笔（limit 会被夹到 50..=5000）。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        code: &str,
        limit: i64,
    ) -> Result<Vec<TickItem>, QuoteError> {
        let normalized = normalize_code(code)?;
        let pure: String = normalized
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .to_string();
        let market = if normalized.starts_with("sh") { 1 } else { 0 };
        let secid = format!("{market}.{pure}");
        let pos = format!("-{}", limit.clamp(50, 5000));
        let response = http
            .get(format!("{}/api/qt/stock/details/get", self.base_url))
            .query(&[
                ("secid", secid.as_str()),
                ("fields1", "f1,f2,f3,f4"),
                ("fields2", "f51,f52,f53,f54,f55"),
                ("pos", pos.as_str()),
            ])
            .send()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(QuoteError::Network(format!("status {}", response.status())));
        }
        let json: Value = response
            .json()
            .await
            .map_err(|e| QuoteError::Parse(e.to_string()))?;
        let details = json["data"]["details"]
            .as_array()
            .ok_or(QuoteError::Empty)?;
        let china = FixedOffset::east_opt(8 * 3600).expect("valid offset");
        let today = Utc::now().with_timezone(&china).date_naive();
        let mut items = Vec::with_capacity(details.len());
        for row in details {
            let Some(text) = row.as_str() else { continue };
            let parts: Vec<&str> = text.split(',').collect();
            if parts.len() < 4 {
                continue;
            }
            let (Ok(price), Ok(volume), Ok(direction)) = (
                parts[1].parse::<f64>(),
                parts[2].parse::<i64>(),
                parts[3].parse::<i64>(),
            ) else {
                continue;
            };
            if price <= 0.0 {
                continue;
            }
            let clock: Vec<&str> = parts[0].split(':').collect();
            if clock.len() != 3 {
                continue;
            }
            let (Ok(h), Ok(m), Ok(s)) = (
                clock[0].parse::<u32>(),
                clock[1].parse::<u32>(),
                clock[2].parse::<u32>(),
            ) else {
                continue;
            };
            let Some(ts) = today
                .and_hms_opt(h, m, s)
                .and_then(|dt| china.from_local_datetime(&dt).single())
            else {
                continue;
            };
            items.push(TickItem {
                ts: ts.with_timezone(&Utc),
                price,
                volume,
                direction,
            });
        }
        if items.is_empty() {
            return Err(QuoteError::Empty);
        }
        Ok(items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secid_market_mapping() {
        assert_eq!(
            EastMoneyTicks::default().base_url.contains("push2delay"),
            true
        );
    }
}
