//! OpenAPI 文档汇总（utoipa → /swagger-ui/）
use utoipa::OpenApi;

use super::ai;
use super::data;
use super::health;
use super::ingest;
use super::pick;
use super::quote;
use super::review;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "mojinprince-server",
        version = "0.4.0",
        description = "摸金小王子 · 行情、AI、数据、实时推送与复盘网关（阶段 4）。\
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
        data::list_signals,
        data::put_signal,
        data::delete_signals,
        data::signal_feedback,
        data::list_positions,
        data::put_position,
        data::list_watchlist,
        data::put_watchlist,
        data::delete_watchlist,
        data::get_settings,
        data::put_settings,
        review::list,
        review::run,
        ingest::get_news,
        ingest::get_reports,
        ingest::get_sector,
        pick::list,
        pick::run,
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
            crate::model::SignalEventDto,
            crate::model::SignalFeedbackRequest,
            crate::model::PositionDto,
            crate::model::PositionUpdate,
            crate::model::WatchlistItem,
            crate::model::WatchlistUpdate,
            crate::model::SettingsDocument,
            crate::model::ScheduledReport,
            crate::model::ReviewContext,
            crate::model::TicketSummary,
            crate::model::ClientSignal,
            crate::model::NewsItem,
            crate::model::ResearchReport,
            crate::model::SectorBoard,
            crate::model::DailyPick,
            crate::model::PicksDocument,
            crate::model::PickStats,
            crate::model::PicksRunRequest,
            crate::state::QuotePushEvent,
            crate::error::AppErrorBody,
        )
    ),
    tags(
        (name = "health", description = "健康检查"),
        (name = "quote", description = "实时报价 / 分时 / 日 K"),
        (name = "ai", description = "OpenAI 兼容接口 / Ollama / 用量账本"),
        (name = "data", description = "信号 / 持仓 / 自选 / 设置"),
        (name = "review", description = "收盘复盘 / 周报（按日期幂等）"),
        (name = "ingest", description = "新闻 / 研报 / 概念板块（东财按需拉取 + 落库）"),
        (name = "pick", description = "A 股池智能推荐（涨幅榜 → 量化 → 消息面 → AI 精排）"),
        (name = "realtime", description = "WebSocket 实时行情：/api/v1/ws/quote")
    )
)]
pub struct ApiDoc;
