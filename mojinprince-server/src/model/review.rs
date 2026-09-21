use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledReport {
    pub id: String,
    pub kind: String,
    pub period_key: String,
    pub title: String,
    pub body: String,
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// 客户端可选传入的上下文，补充 Swift 端"信号 / 委托 / 日记"等
/// 本地内存里的数据。后端会把这些写入 `payload.context`，让报告内容更完整。
///
/// 设计取舍：客户端信号 / 委托目前**不上后端**（只在 Swift 端内存 + JSON 文件），
/// 所以日报 / 周报的内容只能由前端主动 POST 上来，否则报告只能写后端数据库
/// 看得到的东西（AI 调用、命中回测等）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewContext {
    /// 当日/本周客户端记录的"半自动委托"，每条一行摘要。
    #[serde(default)]
    pub tickets: Vec<TicketSummary>,
    /// 客户端编辑的日记原文（Markdown 友好）。
    #[serde(default)]
    pub diary: Option<String>,
    /// 客户端记录的信号（kind / code / title / body / at / why），
    /// 后端不解析，只原样落到 payload.context.signals。
    #[serde(default)]
    pub signals: Vec<ClientSignal>,
    /// 关注的代码清单；后端会据此在 watchlist 外补一份"前端观察池"。
    #[serde(default)]
    pub focus_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TicketSummary {
    pub code: String,
    pub at: DateTime<Utc>,
    /// 一句话摘要，如 "买入 1000 @ 32.50（已复制）"
    pub summary: String,
    /// filled / copied / draft
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
    /// buy / sell；服务端用它做止损止盈执行对照（旧客户端不传则跳过）
    #[serde(default)]
    pub side: Option<String>,
    /// 委托价；对照偏差用它
    #[serde(default)]
    pub price: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClientSignal {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub code: String,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub why: Option<String>,
}
