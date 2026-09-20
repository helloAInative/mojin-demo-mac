//! OpenAPI 文档汇总（utoipa → /swagger-ui/）
use utoipa::OpenApi;

use super::health;
use super::quote;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mojinprince-server",
        version = "0.1.0",
        description = "摸金小王子 · 行情网关（阶段 1）。\
                       完整架构见 docs/架构-Rust后端.md。\
                       当前版本只建议本机或受信任局域网部署；\
                       公网模式必须等 JWT 鉴权实现并验证后再开放。"
    ),
    paths(
        health::health,
        quote::get_quote,
        quote::get_minutes,
        quote::get_days,
    ),
    components(
        schemas(
            health::Health,
            crate::model::Quote,
            crate::model::MinuteBar,
            crate::model::DayBar,
            crate::error::AppErrorBody,
        )
    ),
    tags(
        (name = "health", description = "健康检查"),
        (name = "quote", description = "实时报价 / 分时 / 日 K")
    )
)]
pub struct ApiDoc;
