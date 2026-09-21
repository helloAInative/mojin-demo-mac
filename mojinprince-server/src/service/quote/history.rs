//! 腾讯分时与前复权日 K。网关统一获取，客户端只消费 JSON DTO。
use super::{normalize_code, QuoteError};
use crate::model::{DayBar, MinuteBar};
use chrono::{FixedOffset, TimeZone, Utc};
use serde_json::Value;

pub async fn fetch_minutes(
    http: &reqwest::Client,
    code: &str,
) -> Result<Vec<MinuteBar>, QuoteError> {
    let code = normalize_code(code)?;
    let response = http
        .get("https://web.ifzq.gtimg.cn/appstock/app/minute/query")
        .query(&[("code", code.as_str())])
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
    parse_minutes(&json, &code)
}

pub async fn fetch_days(
    http: &reqwest::Client,
    code: &str,
    limit: i64,
) -> Result<Vec<DayBar>, QuoteError> {
    let code = normalize_code(code)?;
    // 腾讯 WAF 会拒绝部分非常规条数（如 2、60）；120 是其客户端常用请求量。
    let upstream_limit = limit.max(120);
    let param = format!("{code},day,,,{upstream_limit},qfq");
    let response = http
        .get(format!(
            "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param={param}"
        ))
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
    parse_days(&json, &code)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn parse_minutes(json: &Value, code: &str) -> Result<Vec<MinuteBar>, QuoteError> {
    let rows = json["data"][code]["data"]["data"]
        .as_array()
        .ok_or(QuoteError::Empty)?;
    let china = FixedOffset::east_opt(8 * 3600).expect("valid offset");
    let day = Utc::now().with_timezone(&china).date_naive();
    let mut previous_volume = 0.0_f64;
    let mut previous_amount = 0.0_f64;
    let mut bars = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(raw) = row.as_str() else { continue };
        let parts: Vec<_> = raw.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        let (Ok(price), Ok(cumulative_volume), Ok(cumulative_amount)) = (
            parts[1].parse::<f64>(),
            parts[2].parse::<f64>(),
            parts[3].parse::<f64>(),
        ) else {
            continue;
        };
        if price <= 0.0 || parts[0].len() != 4 || !parts[0].bytes().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let (Ok(hour), Ok(minute)) = (parts[0][..2].parse(), parts[0][2..].parse()) else {
            continue;
        };
        let Some(naive) = day.and_hms_opt(hour, minute, 0) else {
            continue;
        };
        let Some(local) = china.from_local_datetime(&naive).single() else {
            continue;
        };
        let average = if cumulative_volume > 0.0 {
            cumulative_amount / cumulative_volume / 100.0
        } else {
            price
        };
        bars.push(MinuteBar {
            code: code.to_string(),
            ts: local.with_timezone(&Utc),
            price,
            avg_price: average,
            volume: (cumulative_volume - previous_volume).max(0.0).round() as i64,
            amount: (cumulative_amount - previous_amount).max(0.0),
        });
        previous_volume = cumulative_volume;
        previous_amount = cumulative_amount;
    }
    if bars.is_empty() {
        return Err(QuoteError::Empty);
    }
    Ok(bars)
}

pub(crate) fn parse_days(json: &Value, code: &str) -> Result<Vec<DayBar>, QuoteError> {
    let stock = &json["data"][code];
    let rows = stock["qfqday"]
        .as_array()
        .or_else(|| stock["day"].as_array())
        .ok_or(QuoteError::Empty)?;
    let mut bars = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(parts) = row.as_array() else {
            continue;
        };
        if parts.len() < 3 {
            continue;
        }
        let (Some(date), Some(close)) = (parts[0].as_str(), number(&parts[2])) else {
            continue;
        };
        if close <= 0.0 {
            continue;
        }
        bars.push(DayBar {
            code: code.to_string(),
            date: date.to_string(),
            open: parts.get(1).and_then(number).unwrap_or(close),
            close,
            high: parts.get(3).and_then(number).unwrap_or(close),
            low: parts.get(4).and_then(number).unwrap_or(close),
            volume: parts.get(5).and_then(number).unwrap_or(0.0).round() as i64,
            amount: parts.get(6).and_then(number).unwrap_or(0.0),
        });
    }
    if bars.is_empty() {
        return Err(QuoteError::Empty);
    }
    Ok(bars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minute_volume_is_incremental() {
        let json = serde_json::json!({"data": {"sz300623": {"data": {"data": [
            "0930 10.00 100 100000", "0931 10.20 140 141000"
        ]}}}});
        let bars = parse_minutes(&json, "sz300623").unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].volume, 100);
        assert_eq!(bars[1].volume, 40);
        assert_eq!(bars[1].amount, 41000.0);
        assert_eq!(bars[1].avg_price, 141000.0 / 140.0 / 100.0);
    }

    #[test]
    fn daily_prefers_adjusted_series() {
        let json = serde_json::json!({"data": {"sz300623": {
            "qfqday": [["2026-09-17", "10", "11", "12", "9", "200"]],
            "day": [["2026-09-17", "20", "21"]]
        }}});
        let bars = parse_days(&json, "sz300623").unwrap();
        assert_eq!(bars[0].close, 11.0);
        assert_eq!(bars[0].volume, 200);
    }
}
