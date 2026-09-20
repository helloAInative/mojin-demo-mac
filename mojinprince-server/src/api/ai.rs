//! AI 网关 HTTP API：Provider 路由、上游调用与 SQLite 用量账本。
//!
//! 设计：
//! - 上游调用走 `service::ai::resolve(id, base_url, api_key)` 得到 `Box<dyn Provider>`，
//!   让 OpenAI 兼容 / Ollama / 后续三方 provider 共用同一路由。
//! - 每次调用按成功 / 失败两条路径写一条 `ai_usage` 行，便于客户端拼 `GET /usage` 汇总。
//! - 失败明细只截 500 字符，避免把上游整段响应塞进数据库。
//! - `service::ai::governor::Governor` 统一冷却 / 配额 / 熔断，所有 handler 共享。
//!
//! 端点：
//!   POST /api/v1/ai/chat       兼容 Swift `AIService.chat`：单条 system+user
//!   POST /api/v1/ai/analyze    复用 chat 但额外落库 signal_event（kind="ai"）
//!   POST /api/v1/ai/reflect    多步反思（最多 5 步，串成 chain-of-thought）
//!   GET  /api/v1/ai/usage      总用量（total/success/failed/avg + 最近 N 条）
//!   GET  /api/v1/ai/accuracy   命中率（kind='level' 信号）
//!   POST /api/v1/ai/feedback   用户反馈（accept / ignore / partial）

use crate::error::{AppError, AppResult};
use crate::model::{AiChatRequest, AiChatResponse, AiUsageRecord, AiUsageSummary};
use crate::repo::signal::{SignalEventRow, SignalRepo};
use crate::service::ai::{
    self,
    governor::{DenyReason, Governor, GovernorConfig},
    prompt::SYSTEM_PROMPT,
    ChatMessage, ChatRequest, ProviderError,
};
use crate::service::ingest;
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use chrono::Utc;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

// 全局单例 governor（按 (provider, model) 二元组分桶）
static GOVERNOR: Lazy<Governor> = Lazy::new(|| Governor::new(GovernorConfig::default()));

#[utoipa::path(
    post,
    path = "/api/v1/ai/chat",
    tag = "ai",
    request_body = AiChatRequest,
    responses(
        (status = 200, description = "模型响应", body = AiChatResponse),
        (status = 400, description = "请求或 Provider 配置错误", body = crate::error::AppErrorBody),
        (status = 429, description = "冷却 / 配额", body = crate::error::AppErrorBody),
        (status = 502, description = "AI 上游调用失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn chat(
    state: web::Data<AppState>,
    body: web::Json<AiChatRequest>,
) -> AppResult<HttpResponse> {
    let request = body.into_inner();
    let provider_id = normalized_provider(&request.provider);
    let model = request.model.trim().to_string();
    validate(&request, &model)?;

    let provider = ai::resolve(
        &provider_id,
        request.base_url.trim(),
        request.api_key.trim(),
    )
    .map_err(|error| map_provider_error(error, &provider_id, &model))?;
    let bucket = format!("{provider_id}/{model}");
    if let Err(reason) = GOVERNOR.check(&bucket) {
        return Err(map_deny(reason));
    }

    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: request.system.clone(),
        },
        ChatMessage {
            role: "user".into(),
            content: request.user.clone(),
        },
    ];
    let prompt_chars: i64 = (request.system.len() + request.user.len()) as i64;
    let chat_request = ChatRequest {
        model: &model,
        messages: &messages,
        temperature: request.temperature,
        max_tokens: request.max_tokens,
        enable_thinking: request.enable_thinking,
    };

    let started = Instant::now();
    let result = provider.chat(chat_request).await;
    let elapsed = elapsed_ms(started);

    match result {
        Ok(response) => {
            GOVERNOR.on_success(&bucket);
            if let Err(error) = insert_usage(
                &state,
                &provider_id,
                &model,
                response.usage.prompt_tokens as i64,
                response.usage.completion_tokens as i64,
                response.usage.cost_usd,
                elapsed,
                prompt_chars,
                response.text.chars().count() as i64,
                true,
                false,
                None,
                request.note.as_deref(),
            )
            .await
            {
                tracing::warn!(%error, "failed to persist successful AI usage");
            }
            Ok(HttpResponse::Ok().json(AiChatResponse {
                content: response.text,
                provider: provider_id,
                model: response.model,
                tokens_in: response.usage.prompt_tokens as i64,
                tokens_out: response.usage.completion_tokens as i64,
                cost_usd: response.usage.cost_usd,
                duration_ms: elapsed,
            }))
        }
        Err(error) => {
            GOVERNOR.on_failure(&bucket);
            let safe_error = error.to_string().chars().take(500).collect::<String>();
            if let Err(db_error) = insert_usage(
                &state,
                &provider_id,
                &model,
                0,
                0,
                0.0,
                elapsed,
                prompt_chars,
                0,
                false,
                false,
                Some(&safe_error),
                request.note.as_deref(),
            )
            .await
            {
                tracing::warn!(%db_error, "failed to persist failed AI usage");
            }
            Err(map_provider_error(error, &provider_id, &model))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    pub limit: Option<i64>,
}

#[utoipa::path(
    get,
    path = "/api/v1/ai/usage",
    tag = "ai",
    params(("limit" = Option<i64>, Query, description = "最近记录条数，1..=200")),
    responses(
        (status = 200, description = "AI 用量汇总和最近记录", body = AiUsageSummary),
        (status = 500, description = "数据库错误", body = crate::error::AppErrorBody),
    )
)]
pub async fn usage(
    state: web::Data<AppState>,
    query: web::Query<UsageQuery>,
) -> AppResult<HttpResponse> {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let recent = sqlx::query_as::<_, AiUsageRecord>(
        "SELECT id,at,provider,model,tokens_in,tokens_out,cost,duration_ms,\
         CAST(success AS BOOLEAN) AS success,\
         CAST(fallback AS BOOLEAN) AS fallback,\
         error,note,prompt_chars,output_chars \
         FROM ai_usage ORDER BY at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(&state.db)
    .await?;
    let row: (i64, i64, f64, i64, i64, f64) = sqlx::query_as(
        "SELECT COUNT(*), \
         COALESCE(SUM(success),0), \
         CAST(COALESCE(AVG(duration_ms),0) AS REAL), \
         COALESCE(SUM(tokens_in),0), \
         COALESCE(SUM(tokens_out),0), \
         CAST(COALESCE(SUM(cost),0) AS REAL) \
         FROM ai_usage",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(HttpResponse::Ok().json(AiUsageSummary {
        total: row.0,
        success: row.1,
        failed: row.0 - row.1,
        avg_duration_ms: row.2.round() as i64,
        tokens_in: row.3,
        tokens_out: row.4,
        cost_usd: (row.5 * 10_000.0).round() / 10_000.0,
        recent,
    }))
}

// ---- 阶段 2 扩展：analyze / reflect / accuracy / feedback ----

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AnalyzeRequest {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
    pub user: String,
    /// 自定义 system prompt；缺省用内置 SYSTEM_PROMPT
    pub system: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub enable_thinking: Option<bool>,
    /// 当前盯盘代码；缺省空（不写 signal_event）
    pub code: Option<String>,
    /// 当前价；缺省 0
    pub price: Option<f64>,
    /// 是否把「近 24h 新闻」拼进 prompt（§F.3；需要 `code`）
    pub include_news: Option<bool>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AnalyzeResponse {
    pub content: String,
    pub provider: String,
    pub model: String,
    pub usage_id: i64,
    pub duration_ms: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub cost_usd: f64,
    /// 落库的 signal_event.id（成功且 code 非空时）
    pub signal_id: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/ai/analyze",
    tag = "ai",
    request_body = AnalyzeRequest,
    responses(
        (status = 200, description = "AI 分析结果", body = AnalyzeResponse),
        (status = 400, description = "参数非法", body = crate::error::AppErrorBody),
        (status = 429, description = "冷却 / 配额", body = crate::error::AppErrorBody),
        (status = 502, description = "AI 上游失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn analyze(
    state: web::Data<AppState>,
    body: web::Json<AnalyzeRequest>,
) -> AppResult<HttpResponse> {
    let req = body.into_inner();
    if req.model.trim().is_empty() {
        return Err(AppError::BadRequest("model is empty".into()));
    }
    let provider_id = req.provider.clone().unwrap_or_else(|| "openai".to_string());
    let model = req.model.trim().to_string();
    let base_url = req.base_url.clone().unwrap_or_else(|| {
        if provider_id == "ollama" {
            "http://127.0.0.1:11434".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        }
    });
    let key = req.api_key.clone().unwrap_or_default();

    let provider = ai::resolve(&provider_id, &base_url, &key)
        .map_err(|e| map_provider_error(e, &provider_id, &model))?;
    let bucket = format!("{provider_id}/{model}");
    if let Err(reason) = GOVERNOR.check(&bucket) {
        return Err(map_deny(reason));
    }

    let sys = req
        .system
        .clone()
        .unwrap_or_else(|| SYSTEM_PROMPT.to_string());
    let user_content = with_recent_news(&state, &req).await;
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: sys.clone(),
        },
        ChatMessage {
            role: "user".into(),
            content: user_content.clone(),
        },
    ];
    let temperature = req.temperature.unwrap_or(0.4);
    let max_tokens = req.max_tokens.unwrap_or(600);
    if !(0.0..=2.0).contains(&temperature) {
        return Err(AppError::BadRequest("temperature must be 0..=2".into()));
    }
    if max_tokens == 0 || max_tokens > 8192 {
        return Err(AppError::BadRequest("max_tokens must be 1..=8192".into()));
    }
    let chat_req = ChatRequest {
        model: &model,
        messages: &messages,
        temperature,
        max_tokens,
        enable_thinking: req.enable_thinking,
    };

    let started = Instant::now();
    let result = provider.chat(chat_req).await;
    let elapsed = elapsed_ms(started);

    match result {
        Ok(resp) => {
            GOVERNOR.on_success(&bucket);
            let id = insert_usage(
                &state,
                &provider_id,
                &model,
                resp.usage.prompt_tokens as i64,
                resp.usage.completion_tokens as i64,
                resp.usage.cost_usd,
                elapsed,
                (sys.len() + user_content.len()) as i64,
                resp.text.chars().count() as i64,
                true,
                false,
                None,
                Some("analyze"),
            )
            .await
            .unwrap_or(0);
            // 落库 signal_event（kind=ai）—— 供前端 SignalTimeline 显示 / accuracy 统计
            let mut signal_id: Option<String> = None;
            if let Some(code) = req.code.as_deref().filter(|c| !c.is_empty()) {
                let sid = uuid_v4();
                let row = SignalEventRow {
                    id: sid.clone(),
                    at: Utc::now(),
                    kind: "ai".into(),
                    code: code.to_string(),
                    title: format!("AI 分析 · {model}"),
                    body: resp.text.chars().take(160).collect(),
                    price: req.price.unwrap_or(0.0),
                    source: provider_id.clone(),
                    evidence: req.user.clone(),
                    why: format!("analyze via {provider_id}/{model} · {elapsed}ms"),
                    meta: serde_json::json!({
                        "usage_id": id,
                        "model": model,
                        "tokens_in": resp.usage.prompt_tokens,
                        "tokens_out": resp.usage.completion_tokens,
                    })
                    .to_string(),
                };
                let repo = SignalRepo::new(state.db.clone());
                if let Err(e) = repo.upsert(&row).await {
                    tracing::warn!(%e, "failed to persist ai signal_event");
                } else {
                    signal_id = Some(sid);
                }
            }
            Ok(HttpResponse::Ok().json(AnalyzeResponse {
                content: resp.text,
                provider: provider_id,
                model: resp.model,
                usage_id: id,
                duration_ms: elapsed,
                tokens_in: resp.usage.prompt_tokens as i64,
                tokens_out: resp.usage.completion_tokens as i64,
                cost_usd: resp.usage.cost_usd,
                signal_id,
            }))
        }
        Err(e) => {
            GOVERNOR.on_failure(&bucket);
            let safe = e.to_string().chars().take(500).collect::<String>();
            let _ = insert_usage(
                &state,
                &provider_id,
                &model,
                0,
                0,
                0.0,
                elapsed,
                (sys.len() + user_content.len()) as i64,
                0,
                false,
                false,
                Some(&safe),
                Some("analyze"),
            )
            .await;
            Err(map_provider_error(e, &provider_id, &model))
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReflectStep {
    pub user: String,
    /// 上一轮模型返回的文本；首步可省略
    pub previous: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReflectRequest {
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: String,
    pub steps: Vec<ReflectStep>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ReflectResponse {
    pub final_text: String,
    pub steps: Vec<StepResult>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StepResult {
    pub text: String,
    pub latency_ms: i64,
    pub usage_id: i64,
}

#[utoipa::path(
    post,
    path = "/api/v1/ai/reflect",
    tag = "ai",
    request_body = ReflectRequest,
    responses(
        (status = 200, description = "多步反思结果", body = ReflectResponse),
        (status = 400, description = "参数非法 / 步数超限", body = crate::error::AppErrorBody),
        (status = 502, description = "AI 上游失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn reflect(
    state: web::Data<AppState>,
    body: web::Json<ReflectRequest>,
) -> AppResult<HttpResponse> {
    let req = body.into_inner();
    if req.steps.is_empty() || req.steps.len() > 5 {
        return Err(AppError::BadRequest(
            format!("steps must be 1..=5, got {}", req.steps.len()).into(),
        ));
    }
    let provider_id = req.provider.clone().unwrap_or_else(|| "openai".to_string());
    let model = req.model.trim().to_string();
    let base_url = req.base_url.clone().unwrap_or_else(|| {
        if provider_id == "ollama" {
            "http://127.0.0.1:11434".to_string()
        } else {
            "https://api.openai.com/v1".to_string()
        }
    });
    let key = req.api_key.clone().unwrap_or_default();
    let provider = ai::resolve(&provider_id, &base_url, &key)
        .map_err(|e| map_provider_error(e, &provider_id, &model))?;

    let temperature = req.temperature.unwrap_or(0.4);
    let max_tokens = req.max_tokens.unwrap_or(600);
    let mut history: Vec<ChatMessage> = vec![ChatMessage {
        role: "system".into(),
        content: SYSTEM_PROMPT.to_string(),
    }];
    let mut results: Vec<StepResult> = Vec::with_capacity(req.steps.len());
    let mut final_text = String::new();

    for (i, step) in req.steps.iter().enumerate() {
        if let Some(prev) = step.previous.as_deref().filter(|s| !s.is_empty()) {
            history.push(ChatMessage {
                role: "assistant".into(),
                content: prev.to_string(),
            });
        }
        if i > 0 {
            history.push(ChatMessage {
                role: "user".into(),
                content: "请基于以上回答继续反思与修正，输出仍是 JSON 对象。".to_string(),
            });
        }
        history.push(ChatMessage {
            role: "user".into(),
            content: step.user.clone(),
        });
        // reflect 是用户主动触发的多步 agent，短时连续调用是预期行为；
        // 冷却 / 配额交给外层 chat/analyze 把关，避免内层循环自限速。
        let chat_req = ChatRequest {
            model: &model,
            messages: &history,
            temperature,
            max_tokens,
            enable_thinking: None,
        };
        let started = Instant::now();
        let r = provider.chat(chat_req).await;
        let elapsed = elapsed_ms(started);
        match r {
            Ok(resp) => {
                let id = insert_usage(
                    &state,
                    &provider_id,
                    &model,
                    resp.usage.prompt_tokens as i64,
                    resp.usage.completion_tokens as i64,
                    resp.usage.cost_usd,
                    elapsed,
                    history.iter().map(|m| m.content.len()).sum::<usize>() as i64,
                    resp.text.chars().count() as i64,
                    true,
                    false,
                    None,
                    Some(&format!("reflect step {}", i + 1)),
                )
                .await
                .unwrap_or(0);
                history.push(ChatMessage {
                    role: "assistant".into(),
                    content: resp.text.clone(),
                });
                final_text = resp.text.clone();
                results.push(StepResult {
                    text: resp.text,
                    latency_ms: elapsed,
                    usage_id: id,
                });
            }
            Err(e) => {
                let safe = e.to_string().chars().take(500).collect::<String>();
                let _ = insert_usage(
                    &state,
                    &provider_id,
                    &model,
                    0,
                    0,
                    0.0,
                    elapsed,
                    history.iter().map(|m| m.content.len()).sum::<usize>() as i64,
                    0,
                    false,
                    false,
                    Some(&safe),
                    Some(&format!("reflect step {} failed", i + 1)),
                )
                .await;
                return Err(map_provider_error(e, &provider_id, &model));
            }
        }
    }
    Ok(HttpResponse::Ok().json(ReflectResponse {
        final_text,
        steps: results,
    }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AccuracyQuery {
    pub window_hours: Option<i64>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccuracyResponse {
    pub total: i64,
    pub hit: i64,
    pub hit_rate: f64,
    pub avg_lead_sec: Option<f64>,
    pub window_hours: i64,
}

#[utoipa::path(
    get,
    path = "/api/v1/ai/accuracy",
    tag = "ai",
    params(AccuracyQuery),
    responses(
        (status = 200, description = "AI 命中率（level 信号）", body = AccuracyResponse),
    )
)]
pub async fn accuracy(
    state: web::Data<AppState>,
    q: web::Query<AccuracyQuery>,
) -> AppResult<HttpResponse> {
    let hours = q.window_hours.unwrap_or(24).clamp(1, 24 * 30);
    let since_ms = Utc::now().timestamp_millis() - hours * 3600 * 1000;
    let total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM signal_event WHERE kind='level' AND at >= ?")
            .bind(since_ms)
            .fetch_one(&state.db)
            .await?;
    let hit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM signal_event \
         WHERE kind='level' AND at >= ? \
         AND json_extract(meta,'$.hit')='1'",
    )
    .bind(since_ms)
    .fetch_one(&state.db)
    .await?;
    let avg_lead: Option<f64> = sqlx::query_scalar(
        "SELECT AVG(CAST(json_extract(meta,'$.leadSec') AS INTEGER)) \
         FROM signal_event \
         WHERE kind='level' AND at >= ? \
         AND json_extract(meta,'$.hit')='1'",
    )
    .bind(since_ms)
    .fetch_one(&state.db)
    .await
    .ok()
    .flatten();
    Ok(HttpResponse::Ok().json(AccuracyResponse {
        total,
        hit,
        hit_rate: if total > 0 {
            hit as f64 / total as f64
        } else {
            0.0
        },
        avg_lead_sec: avg_lead,
        window_hours: hours,
    }))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct FeedbackRequest {
    pub usage_id: Option<i64>,
    pub signal_id: Option<String>,
    pub code: String,
    /// accept / ignore / partial
    pub sentiment: String,
    pub note: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/v1/ai/feedback",
    tag = "ai",
    request_body = FeedbackRequest,
    responses(
        (status = 200, description = "反馈已记录"),
        (status = 400, description = "sentiment 非法", body = crate::error::AppErrorBody),
    )
)]
pub async fn feedback(
    state: web::Data<AppState>,
    body: web::Json<FeedbackRequest>,
) -> AppResult<HttpResponse> {
    let req = body.into_inner();
    if !["accept", "ignore", "partial"].contains(&req.sentiment.as_str()) {
        return Err(AppError::BadRequest(
            "sentiment must be accept|ignore|partial".into(),
        ));
    }
    let row = sqlx::query(
        "INSERT INTO ai_feedback(at, usage_id, signal_id, code, sentiment, note) \
         VALUES(?,?,?,?,?,?)",
    )
    .bind(Utc::now().timestamp_millis())
    .bind(req.usage_id)
    .bind(req.signal_id.as_deref())
    .bind(&req.code)
    .bind(&req.sentiment)
    .bind(req.note.as_deref())
    .execute(&state.db)
    .await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "id": row.last_insert_rowid() })))
}

// ---- 内部 helpers ----

/// §F.3：把「近 24h 新闻」拼到用户 prompt 后面（opt-in，需要 `code`）。
///
/// 拉取失败只 warn，不阻断分析；命中条数最多 5 条。
async fn with_recent_news(state: &AppState, req: &AnalyzeRequest) -> String {
    let original = req.user.clone();
    if !req.include_news.unwrap_or(false) {
        return original;
    }
    let Some(code) = req.code.as_deref().filter(|c| !c.is_empty()) else {
        return original;
    };
    let items = match state.news.fetch(&state.http, code, 10).await {
        Ok(items) => items,
        Err(error) => {
            tracing::warn!(%error, code, "fetch news for analyze failed; skip injection");
            return original;
        }
    };
    let since = Utc::now() - chrono::Duration::hours(24);
    let recent: Vec<_> = items
        .into_iter()
        .filter(|item| item.published_at >= since)
        .take(5)
        .collect();
    if recent.is_empty() {
        return original;
    }
    if let Err(error) = ingest::persist_news(&state.db, &recent).await {
        tracing::warn!(%error, code, "failed to persist news used by analyze");
    }
    let mut text = format!("{original}\n\n近 24h 新闻：");
    for item in &recent {
        text.push_str(&format!(
            "\n- [{}] {} ({})",
            item.media,
            item.title,
            item.published_at.format("%m-%d %H:%M")
        ));
    }
    text
}

#[allow(clippy::too_many_arguments)]
async fn insert_usage(
    state: &AppState,
    provider: &str,
    model: &str,
    tokens_in: i64,
    tokens_out: i64,
    cost: f64,
    duration_ms: i64,
    prompt_chars: i64,
    output_chars: i64,
    success: bool,
    fallback: bool,
    error: Option<&str>,
    note: Option<&str>,
) -> Result<i64, sqlx::Error> {
    let r = sqlx::query(
        "INSERT INTO ai_usage(\
            at,provider,model,tokens_in,tokens_out,cost,duration_ms,\
            success,fallback,error,note,prompt_chars,output_chars\
         ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(Utc::now().timestamp_millis())
    .bind(provider)
    .bind(model)
    .bind(tokens_in)
    .bind(tokens_out)
    .bind(cost)
    .bind(duration_ms)
    .bind(success)
    .bind(fallback)
    .bind(error)
    .bind(note)
    .bind(prompt_chars)
    .bind(output_chars)
    .execute(&state.db)
    .await?;
    Ok(r.last_insert_rowid())
}

fn validate(request: &AiChatRequest, model: &str) -> AppResult<()> {
    if model.is_empty() {
        return Err(AppError::BadRequest("model is required".into()));
    }
    let url = reqwest::Url::parse(request.base_url.trim())
        .map_err(|_| AppError::BadRequest("base_url is invalid".into()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AppError::BadRequest(
            "base_url must be an http(s) URL".into(),
        ));
    }
    if request.system.trim().is_empty() || request.user.trim().is_empty() {
        return Err(AppError::BadRequest("system and user are required".into()));
    }
    if !(1..=8192).contains(&request.max_tokens) {
        return Err(AppError::BadRequest("max_tokens must be 1..=8192".into()));
    }
    if !(0.0..=2.0).contains(&request.temperature) {
        return Err(AppError::BadRequest("temperature must be 0..=2".into()));
    }
    Ok(())
}

fn elapsed_ms(started: Instant) -> i64 {
    started.elapsed().as_millis().min(i64::MAX as u128) as i64
}

fn normalized_provider(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "" | "remote" => "openai".into(),
        value => value.to_string(),
    }
}

fn map_provider_error(error: ProviderError, provider: &str, model: &str) -> AppError {
    match error {
        ProviderError::NotConfigured(message) | ProviderError::BadUrl(message) => {
            AppError::BadRequest(format!("{provider}/{model}: {message}").into())
        }
        ProviderError::Http { status, body } if (400..500).contains(&status) => {
            AppError::BadRequest(format!("{provider}/{model} HTTP {status}: {body}").into())
        }
        ProviderError::Timeout(_) => {
            AppError::AiUpstream(format!("{provider}/{model} timeout: {error}").into())
        }
        other => AppError::AiUpstream(format!("{provider}/{model}: {other}").into()),
    }
}

fn map_deny(reason: DenyReason) -> AppError {
    let kind = reason.as_str();
    let msg = reason.to_string();
    let s = format!("{kind}: {msg}");
    match reason {
        DenyReason::BreakerOpen { .. } => AppError::Unavailable(s.into()),
        DenyReason::CoolingDown { .. } | DenyReason::DailyQuotaExceeded { .. } => {
            AppError::TooManyRequests(s.into())
        }
    }
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // 简易 UUID v4：32 hex（不加密学安全，但单实例足够区分 signal_event）
    format!("{:032x}", nanos)
}

// Arc<...> 哨兵，安静 dead-code 警告
#[allow(dead_code)]
fn _force_use(_a: Arc<()>) {}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/ai")
            .route("/chat", web::post().to(chat))
            .route("/analyze", web::post().to(analyze))
            .route("/reflect", web::post().to(reflect))
            .route("/usage", web::get().to(usage))
            .route("/accuracy", web::get().to(accuracy))
            .route("/feedback", web::post().to(feedback)),
    );
}
