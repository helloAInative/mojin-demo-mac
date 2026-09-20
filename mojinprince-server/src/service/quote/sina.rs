//! 新浪行情：https://hq.sinajs.cn/list=<code>
//! 返回 var hq_str_sh600460="大族激光,60.10,59.80,..."; 形式的字符串
use super::{normalize_code, QuoteError};
use crate::model::Quote;
use chrono::Utc;

pub struct Sina {
    /// 可在测试时覆盖的 base URL（默认 `https://hq.sinajs.cn`）
    pub base_url: String,
}

impl Default for Sina {
    fn default() -> Self {
        Self {
            base_url: "https://hq.sinajs.cn".to_string(),
        }
    }
}

impl Sina {
    pub async fn fetch_quote(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> Result<Quote, QuoteError> {
        let normalized = normalize_code(code)?;
        let url = format!("{}/list={}", self.base_url, normalized);
        let resp = http
            .get(&url)
            .header("Referer", "https://finance.sina.com.cn/")
            .send()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(QuoteError::Network(format!("status {}", resp.status())));
        }
        // GBK 编码（新浪默认）
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| QuoteError::Network(e.to_string()))?;
        let (decoded, _, had_errors) = encoding_rs::GBK.decode(&bytes);
        if had_errors {
            return Err(QuoteError::Parse("gbk decode error".into()));
        }
        let body = decoded.into_owned();
        let quote_line = body
            .lines()
            .find(|l| l.contains(&normalized))
            .ok_or_else(|| QuoteError::Parse("no quote line".into()))?;
        let start = quote_line
            .find('"')
            .ok_or_else(|| QuoteError::Parse("missing open quote".into()))?;
        let end = quote_line
            .rfind('"')
            .ok_or_else(|| QuoteError::Parse("missing close quote".into()))?;
        if start == end {
            return Err(QuoteError::Empty);
        }
        let payload = &quote_line[start + 1..end];
        let fields: Vec<&str> = payload.split(',').collect();
        if fields.len() < 32 {
            return Err(QuoteError::Parse(format!("field count {}", fields.len())));
        }
        let parse_f = |s: &str| s.parse::<f64>().unwrap_or(0.0);
        let parse_i = |s: &str| s.parse::<i64>().unwrap_or(0);
        Ok(Quote {
            code: normalized.clone(),
            name: fields[0].to_string(),
            open: parse_f(fields[1]),
            prev: parse_f(fields[2]),
            price: parse_f(fields[3]),
            high: parse_f(fields[4]),
            low: parse_f(fields[5]),
            volume: parse_i(fields[8]),
            amount: parse_f(fields[9]),
            source: "sina".into(),
            ts: Utc::now(),
        })
    }
}
