//! 行情 DTO
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 单标的实时报价
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Quote {
    /// 股票代码（统一大小写，例 sh600460）
    pub code: String,
    /// 名称
    pub name: String,
    /// 现价
    pub price: f64,
    /// 昨收
    pub prev: f64,
    /// 今开
    pub open: f64,
    /// 最高
    pub high: f64,
    /// 最低
    pub low: f64,
    /// 成交量（股）
    pub volume: i64,
    /// 成交额（元）
    pub amount: f64,
    /// 数据源（sina / tencent / eastmoney）
    pub source: String,
    /// 时间戳
    pub ts: DateTime<Utc>,
}

/// 分时 K 线（一分钟桶）
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MinuteBar {
    pub code: String,
    pub ts: DateTime<Utc>,
    pub price: f64,
    pub avg_price: f64,
    pub volume: i64,
    pub amount: f64,
}

/// 日 K 线
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DayBar {
    pub code: String,
    /// 日期字符串 YYYY-MM-DD
    pub date: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub amount: f64,
}
