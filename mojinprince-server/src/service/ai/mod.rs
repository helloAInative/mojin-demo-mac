//! AI 网关阶段 2：
//! - `Provider` trait（async + Send + Sync）
//! - `OpenAIProvider`：兼容协议（OpenAI / 阿里 Maas / DeepSeek / Kimi / GLM / MiniMax …）
//! - `OllamaProvider`：本地 Ollama（/api/chat）
//! - `Governor`：按 (provider, model) 二元组做冷却 + 配额 + 失败计数
//!
//! 与 §9 对接：原 Swift `AIService.chat` / `ProviderRegistry` 走 OpenAI 兼容协议。
//! 新增 Ollama 也走兼容协议，UI 上"providerId=ollama" 即可路由。
pub mod governor;
pub mod ollama;
pub mod openai;
pub mod prompt;

pub use prompt::SYSTEM_PROMPT;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ChatMessage {
    /// "system" / "user" / "assistant"
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [ChatMessage],
    pub temperature: f32,
    pub max_tokens: u32,
    /// OpenAI 的 `enable_thinking`（阿里 Maas 透传）。None 表示不传。
    pub enable_thinking: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 按 (provider, model) 估算的美元价；未知时为 0
    pub cost_usd: f64,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub text: String,
    pub model: String,
    pub usage: ChatUsage,
    pub latency_ms: u32,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("bad url: {0}")]
    BadUrl(String),
    #[error("http {status}: {body}")]
    Http { status: u16, body: String },
    #[error("decode: {0}")]
    Decode(String),
    #[error("empty response")]
    Empty,
    #[error("timeout after {0}ms")]
    Timeout(u32),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ProviderError {
    pub fn status_code(&self) -> u16 {
        match self {
            ProviderError::NotConfigured(_) => 400,
            ProviderError::BadUrl(_) => 400,
            ProviderError::Http { status, .. } => *status,
            ProviderError::Decode(_) => 502,
            ProviderError::Empty => 502,
            ProviderError::Timeout(_) => 504,
            ProviderError::Other(_) => 500,
        }
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// 唯一标识：`openai` / `ollama`
    fn id(&self) -> &'static str;
    /// 单次对话
    async fn chat(&self, req: ChatRequest<'_>) -> Result<ChatResponse, ProviderError>;
}

/// 根据 id + 路径参数解析 provider（解耦 config 与构造）。
pub fn resolve(id: &str, base_url: &str, api_key: &str) -> Result<Box<dyn Provider>, ProviderError> {
    let id = id.trim().to_ascii_lowercase();
    match id.as_str() {
        "openai" | "remote" | "" => Ok(Box::new(openai::OpenAIProvider::new(base_url, api_key))),
        "ollama" => Ok(Box::new(ollama::OllamaProvider::new(base_url))),
        other => Err(ProviderError::NotConfigured(format!("unknown provider id: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_routes_known_ids() {
        assert_eq!(
            resolve("openai", "https://x", "k").unwrap().id(),
            "openai"
        );
        assert_eq!(resolve("", "http://x", "k").unwrap().id(), "openai");
        assert_eq!(resolve("ollama", "http://x", "").unwrap().id(), "ollama");
        assert!(resolve("bogus", "x", "y").is_err());
    }
}