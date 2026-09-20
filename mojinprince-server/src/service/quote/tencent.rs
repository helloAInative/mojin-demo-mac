//! 腾讯行情：http://qt.gtimg.cn/q=<code>
//! 返回 v_sh600460="1~大族激光~600460~60.10~59.80~..."; 形式
use super::{normalize_code, QuoteError};
use crate::model::Quote;
use chrono::Utc;

pub struct Tencent {
    /// 可在测试时覆盖的 base URL（默认 `http://qt.gtimg.cn`）
    pub base_url: String,
}

impl Default for Tencent {
    fn default() -> Self {
        Self {
            base_url: "http://qt.gtimg.cn".to_string(),
        }
    }
}

impl Tencent {
    pub async fn fetch_quote(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> Result<Quote, QuoteError> {
        let normalized = normalize_code(code)?;
        let url = format!("{}/q={}", self.base_url, normalized);
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(QuoteError::Network(format!("status {}", resp.status())));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        // 兼容 GBK → 用 encoding_rs 兜底
        let (decoded, _, _) = encoding_rs::GBK.decode(body.as_bytes());
        let body = decoded.into_owned();
        let line = body
            .lines()
            .find(|l| l.contains(&normalized))
            .ok_or_else(|| QuoteError::Parse("no quote line".into()))?;
        let start = line
            .find('"')
            .ok_or_else(|| QuoteError::Parse("missing quote".into()))?;
        let end = line
            .rfind('"')
            .ok_or_else(|| QuoteError::Parse("missing quote".into()))?;
        if start == end {
            return Err(QuoteError::Empty);
        }
        let payload = &line[start + 1..end];
        let fields: Vec<&str> = payload.split('~').collect();
        // 典型：1~名称~代码~现价~昨收~今开~成交量(手)~外盘~内盘~买一~买一价~...
        if fields.len() < 10 {
            return Err(QuoteError::Parse(format!("field count {}", fields.len())));
        }
        let parse_f = |s: &str| s.parse::<f64>().unwrap_or(0.0);
        let parse_i = |s: &str| s.parse::<i64>().unwrap_or(0);
        let volume_hands = parse_i(fields[6]); // 单位：手
        let volume = volume_hands * 100;
        Ok(Quote {
            code: normalized.clone(),
            name: fields[1].to_string(),
            price: parse_f(fields[3]),
            prev: parse_f(fields[4]),
            open: parse_f(fields[5]),
            high: parse_f(if fields.len() > 33 { fields[33] } else { "0" }),
            low: parse_f(if fields.len() > 34 { fields[34] } else { "0" }),
            volume,
            amount: 0.0, // 腾讯免费源无成交额
            source: "tencent".into(),
            ts: Utc::now(),
        })
    }
}
