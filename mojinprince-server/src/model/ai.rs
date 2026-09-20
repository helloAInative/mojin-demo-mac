//! AI 网关 DTO。
//!
//! API Token 只用于本次上游调用，不写日志或数据库；
//! `cost` / `fallback` / `prompt_chars` / `output_chars` / `note` 全部对应
//! `migrations/20250918000002_ai_usage.sql` 中的字段。

use serde::{Deserialize, Serialize};

fn default_provider() -> String {
    "openai".to_string()
}

fn default_max_tokens() -> u32 {
    600
}

fn default_temperature() -> f32 {
    0.4
}

/// AI 网关入参：`system + user` 会被合并为两条 `ChatMessage`。
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct AiChatRequest {
    /// `openai` / `ollama`，缺省按 `openai`（OpenAI 兼容协议）。
    #[serde(default = "default_provider")]
    pub provider: String,
    /// OpenAI 兼容根地址（通常以 `/v1` 结尾）或 Ollama 根地址。
    pub base_url: String,
    /// OpenAI 兼容接口所需 Token；Ollama 本机调用可留空。
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    pub system: String,
    pub user: String,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 阿里 Maas 等服务透传 `enable_thinking`；None 表示不传。
    #[serde(default)]
    pub enable_thinking: Option<bool>,
    /// 简单摘要，便于日志检索（标的 / prompt hash 等）。落库 `ai_usage.note`。
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AiChatResponse {
    pub content: String,
    pub provider: String,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_usd: f64,
    pub duration_ms: i64,
}

/// 单条 AI 用量记录。对应 `ai_usage` 表。
#[derive(Debug, Clone, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct AiUsageRecord {
    pub id: i64,
    /// Unix epoch milliseconds。
    pub at: i64,
    pub provider: String,
    pub model: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost: f64,
    pub duration_ms: i64,
    pub success: bool,
    pub fallback: bool,
    pub error: Option<String>,
    pub note: Option<String>,
    pub prompt_chars: i64,
    pub output_chars: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AiUsageSummary {
    pub total: i64,
    pub success: i64,
    pub failed: i64,
    pub avg_duration_ms: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_usd: f64,
    pub recent: Vec<AiUsageRecord>,
}
