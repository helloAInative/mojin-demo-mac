pub mod eastmoney;
pub mod failover;
pub mod history;
pub mod sina;
pub mod tencent;

pub use eastmoney::EastMoney;
pub use sina::Sina;
pub use tencent::Tencent;

use crate::model::Quote;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum QuoteError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("empty")]
    Empty,
    #[error("bad code: {0}")]
    BadCode(String),
}

/// 行情 provider 抽象：enum 包装，避免 async-trait 与 dyn 不兼容
pub enum QuoteProviders {
    Sina(Sina),
    Tencent(Tencent),
    EastMoney(EastMoney),
}

impl QuoteProviders {
    pub fn name(&self) -> &'static str {
        match self {
            QuoteProviders::Sina(_) => "sina",
            QuoteProviders::Tencent(_) => "tencent",
            QuoteProviders::EastMoney(_) => "eastmoney",
        }
    }
    pub async fn fetch_quote(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> Result<Quote, QuoteError> {
        match self {
            QuoteProviders::Sina(p) => p.fetch_quote(http, code).await,
            QuoteProviders::Tencent(p) => p.fetch_quote(http, code).await,
            QuoteProviders::EastMoney(p) => p.fetch_quote(http, code).await,
        }
    }
}

/// 把代码统一成"小写前缀 + 数字"的形式（sina 协议要求）。
/// 例：600460 → sh600460；000001 → sz000001；bj83xxxx → bj83xxxx
pub fn normalize_code(code: &str) -> Result<String, QuoteError> {
    let raw = code.trim().to_lowercase();
    if let Some(rest) = raw.strip_prefix("sh") {
        if rest.len() == 6 && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(format!("sh{}", rest));
        }
    }
    if let Some(rest) = raw.strip_prefix("sz") {
        if rest.len() == 6 && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(format!("sz{}", rest));
        }
    }
    if let Some(rest) = raw.strip_prefix("bj") {
        if rest.len() == 6 && rest.chars().all(|c| c.is_ascii_digit()) {
            return Ok(format!("bj{}", rest));
        }
    }
    if raw.chars().all(|c| c.is_ascii_digit()) {
        if raw.len() != 6 {
            return Err(QuoteError::BadCode(code.into()));
        }
        // 沪市 60/68/90/11/13；深市 00/30/20；北交 8/43/92
        let prefix2 = &raw[..2];
        let market = match prefix2 {
            "60" | "68" | "90" | "11" | "13" | "50" | "51" | "52" | "56" | "58" => "sh",
            "00" | "30" | "20" | "39" => "sz",
            "83" | "43" | "92" => "bj",
            _ => return Err(QuoteError::BadCode(code.into())),
        };
        return Ok(format!("{}{}", market, raw));
    }
    Err(QuoteError::BadCode(code.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_passes() {
        assert_eq!(normalize_code("600460").unwrap(), "sh600460");
        assert_eq!(normalize_code("000001").unwrap(), "sz000001");
        assert_eq!(normalize_code("835899").unwrap(), "bj835899");
        assert_eq!(normalize_code("sh600460").unwrap(), "sh600460");
    }
}
