//! Ollama 本地 provider。协议：POST {base}/api/chat
//! body: { "model", "messages": [{role, content}], "stream": false, "options": { temperature, num_predict } }
//! resp: { "message": {"role":"assistant","content":"..."}, "eval_count", "prompt_eval_count" }
use super::{ChatRequest, ChatResponse, ChatUsage, Provider, ProviderError};
use async_trait::async_trait;
use serde::Deserialize;

pub struct OllamaProvider {
    pub base_url: String,
    pub http: reqwest::Client,
}

impl OllamaProvider {
    pub fn new(base_url: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self {
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            http,
        }
    }
}

#[async_trait]
impl Provider for OllamaProvider {
    fn id(&self) -> &'static str {
        "ollama"
    }

    async fn chat(&self, req: ChatRequest<'_>) -> Result<ChatResponse, ProviderError> {
        let url = format!("{}/api/chat", self.base_url);
        let body = serde_json::json!({
            "model": req.model,
            "messages": req.messages,
            "stream": false,
            "options": {
                "temperature": req.temperature,
                "num_predict": req.max_tokens,
            }
        });
        let started = std::time::Instant::now();
        let resp = self.http.post(&url).json(&body).send().await.map_err(|e| {
            if e.is_timeout() {
                ProviderError::Timeout(120_000)
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
        let parsed: OllamaChat = serde_json::from_str(&raw).map_err(|e| {
            ProviderError::Decode(format!(
                "{e}; raw={}",
                raw.chars().take(200).collect::<String>()
            ))
        })?;
        let text = parsed.message.content.trim().to_string();
        if text.is_empty() {
            return Err(ProviderError::Empty);
        }
        Ok(ChatResponse {
            text,
            model: parsed.model.unwrap_or_else(|| req.model.to_string()),
            usage: ChatUsage {
                prompt_tokens: parsed.prompt_eval_count.unwrap_or(0),
                completion_tokens: parsed.eval_count.unwrap_or(0),
                cost_usd: 0.0, // 本地模型不计费
            },
            latency_ms: started.elapsed().as_millis() as u32,
        })
    }
}

#[derive(Debug, Deserialize)]
struct OllamaChat {
    #[serde(default)]
    message: Msg,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "prompt_eval_count")]
    prompt_eval_count: Option<u32>,
    #[serde(default, rename = "eval_count")]
    eval_count: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct Msg {
    #[serde(default)]
    content: String,
}
