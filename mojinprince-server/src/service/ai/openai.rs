//! OpenAI 兼容协议 provider。
//! 适用：OpenAI / 阿里 Maas / DeepSeek / 月之暗面 Kimi / 智谱 GLM / MiniMax / 火山等。
//!
//! 协议要点（与 AIService.swift 现有 chat 完全一致）：
//!   POST {base}/chat/completions
//!   Authorization: Bearer {key}
//!   { "model", "temperature", "max_tokens", "enable_thinking"?, "messages": [{role, content}] }
//!   → choices[0].message.content | reasoning_content | text
use super::{ChatRequest, ChatResponse, ChatUsage, Provider, ProviderError};
use async_trait::async_trait;
use serde::Deserialize;

pub struct OpenAIProvider {
    pub base_url: String,
    pub api_key: String,
    pub http: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        Self {
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            api_key: api_key.trim().to_string(),
            http,
        }
    }

    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn id(&self) -> &'static str {
        "openai"
    }

    async fn chat(&self, req: ChatRequest<'_>) -> Result<ChatResponse, ProviderError> {
        if self.api_key.is_empty() {
            return Err(ProviderError::NotConfigured("openai api_key is empty".into()));
        }
        let url = format!("{}/chat/completions", self.base_url);
        // 即使为 None 也显式写 false，避免阿里 Maas 等上游默认开启思考模式
        // 带来的"先给一长串 reasoning_content，再给正文"问题。
        let enable_thinking = req.enable_thinking.unwrap_or(false);
        let body = serde_json::json!({
            "model": req.model,
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
            "messages": req.messages,
            "enable_thinking": enable_thinking,
        });
        let started = std::time::Instant::now();
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout(60_000)
                } else {
                    ProviderError::Other(anyhow::anyhow!(e))
                }
            })?;
        let status = resp.status();
        let raw = resp
            .text()
            .await
            .map_err(|e| ProviderError::Other(anyhow::anyhow!(e)))?;
        if !status.is_success() {
            return Err(ProviderError::Http {
                status: status.as_u16(),
                body: raw.chars().take(400).collect(),
            });
        }
        let parsed: ChatCompletions = serde_json::from_str(&raw)
            .map_err(|e| ProviderError::Decode(format!("{e}; raw={}", raw.chars().take(200).collect::<String>())))?;
        let choice = parsed
            .choices
            .first()
            .ok_or(ProviderError::Empty)?;
        let text = pick_text(choice)
            .ok_or(ProviderError::Empty)?
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(ProviderError::Empty);
        }
        let usage = parsed.usage.unwrap_or_default();
        let cost = estimate_cost_openai(req.model, usage.prompt_tokens, usage.completion_tokens);
        Ok(ChatResponse {
            text,
            model: parsed.model.unwrap_or_else(|| req.model.to_string()),
            usage: ChatUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                cost_usd: cost,
            },
            latency_ms: started.elapsed().as_millis() as u32,
        })
    }
}

fn pick_text(c: &Choice) -> Option<&str> {
    if let Some(content) = c.message.content.as_deref() {
        if !content.is_empty() {
            return Some(content);
        }
    }
    if let Some(reason) = c.message.reasoning_content.as_deref() {
        if !reason.is_empty() {
            return Some(reason);
        }
    }
    c.text.as_deref()
}

/// 粗略美元单价表（按 1K tokens）。未知模型 → 0。仅供"体感"展示，不作真账单。
fn price_per_1k(model: &str) -> Option<(f64, f64)> {
    let m = model.to_ascii_lowercase();
    // 千问 3.x
    if m.contains("qwen3.8-max") {
        return Some((0.020, 0.060));
    }
    if m.contains("qwen3.7") || m.contains("qwen3.6") {
        return Some((0.004, 0.012));
    }
    if m.contains("qwen3.5") {
        return Some((0.002, 0.006));
    }
    if m.contains("deepseek-v4") {
        return Some((0.0008, 0.0016));
    }
    if m.contains("kimi-k2") {
        return Some((0.003, 0.009));
    }
    if m.contains("glm-5") {
        return Some((0.001, 0.002));
    }
    if m.contains("MiniMax") {
        return Some((0.001, 0.002));
    }
    None
}

fn estimate_cost_openai(model: &str, pin: u32, pout: u32) -> f64 {
    let Some((pi, po)) = price_per_1k(model) else {
        return 0.0;
    };
    (pin as f64 / 1000.0) * pi + (pout as f64 / 1000.0) * po
}

#[derive(Debug, Deserialize)]
struct ChatCompletions {
    #[serde(default)]
    choices: Vec<Choice>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Debug, Default, Deserialize)]
struct Usage {
    #[serde(default, rename = "prompt_tokens")]
    prompt_tokens: u32,
    #[serde(default, rename = "completion_tokens")]
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: Msg,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Msg {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
}