//! OpenAPI 文档汇总（utoipa → /swagger-ui/）
use utoipa::OpenApi;

use super::ai;
use super::health;
use super::quote;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mojinprince-server",
        version = "0.2.0",
        description = "摸金小王子 · 行情与 AI 网关（阶段 2）。\
                       完整架构见 docs/架构-Rust后端.md。\
                       当前版本只建议本机或受信任局域网部署；\
                       公网模式必须等 JWT 鉴权实现并验证后再开放。"
    ),
    paths(
        health::health,
        quote::get_quote,
        quote::get_minutes,
        quote::get_days,
        ai::chat,
        ai::analyze,
        ai::reflect,
        ai::usage,
        ai::accuracy,
        ai::feedback,
    ),
    components(
        schemas(
            health::Health,
            crate::model::Quote,
            crate::model::MinuteBar,
            crate::model::DayBar,
            crate::model::AiChatRequest,
            crate::model::AiChatResponse,
            crate::model::AiUsageRecord,
            crate::model::AiUsageSummary,
            crate::error::AppErrorBody,
        )
    ),
    tags(
        (name = "health", description = "健康检查"),
        (name = "quote", description = "实时报价 / 分时 / 日 K"),
        (name = "ai", description = "OpenAI 兼容接口 / Ollama / 用量账本")
    )
)]
pub struct ApiDoc;
