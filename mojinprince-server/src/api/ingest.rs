//! §F.3–F.4 数据接入 API：新闻 / 研报 / 概念板块。
//!
//! 三个端点都是"按需拉取 + 落库"：上游成功即写入 SQLite（UPSERT 去重），
//! 每日调度器再为自选股增量刷新，客户端可离线读库。
use crate::error::AppResult;
use crate::model::{NewsItem, ResearchReport, SectorBoard};
use crate::service::ingest::{persist_news, persist_reports, persist_sector_boards};
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use chrono::{Duration, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct NewsQuery {
    pub limit: Option<i64>,
    /// 只看最近 N 小时的新闻（默认 72）
    pub hours: Option<i64>,
}

#[derive(Deserialize)]
pub struct ReportsQuery {
    pub limit: Option<i64>,
    /// 拉取最近 N 天的研报（默认 365）
    pub days: Option<i64>,
}

/// 个股相关新闻（近 `hours` 小时，默认 72）。数据源：东财全文搜索。
#[utoipa::path(
    get,
    path = "/api/v1/news/{code}",
    tag = "ingest",
    params(
        ("code" = String, Path, description = "股票代码，例如 600460 / sh600460"),
        ("limit" = Option<i64>, Query, description = "返回条数（1..=100，默认 20）"),
        ("hours" = Option<i64>, Query, description = "时间窗小时数（1..=720，默认 72）"),
    ),
    responses(
        (status = 200, description = "新闻数组（按发布时间倒序）", body = [NewsItem]),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "上游失败 / 解析失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_news(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<NewsQuery>,
) -> AppResult<HttpResponse> {
    let code = path.into_inner();
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let hours = query.hours.unwrap_or(72).clamp(1, 720);
    let items = state.news.fetch(&state.http, &code, limit as usize).await?;
    persist_news(&state.db, &items).await?;
    let cutoff = Utc::now() - Duration::hours(hours);
    let recent: Vec<NewsItem> = items
        .into_iter()
        .filter(|item| item.published_at >= cutoff)
        .collect();
    Ok(HttpResponse::Ok().json(recent))
}

/// 机构研报（最近 `days` 天，默认 365）。数据源：东财研报库。
#[utoipa::path(
    get,
    path = "/api/v1/reports/{code}",
    tag = "ingest",
    params(
        ("code" = String, Path, description = "股票代码"),
        ("limit" = Option<i64>, Query, description = "返回条数（1..=100，默认 20）"),
        ("days" = Option<i64>, Query, description = "回溯天数（1..=1095，默认 365）"),
    ),
    responses(
        (status = 200, description = "研报数组（按发布日期倒序）", body = [ResearchReport]),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "上游失败 / 解析失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_reports(
    state: web::Data<AppState>,
    path: web::Path<String>,
    query: web::Query<ReportsQuery>,
) -> AppResult<HttpResponse> {
    let code = path.into_inner();
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let days = query.days.unwrap_or(365).clamp(1, 1095);
    let items = state
        .reports
        .fetch(&state.http, &code, limit as usize, days)
        .await?;
    persist_reports(&state.db, &items).await?;
    Ok(HttpResponse::Ok().json(items))
}

/// 个股所属概念 / 行业板块，含板块最新指数与涨跌幅。
#[utoipa::path(
    get,
    path = "/api/v1/sector/{code}",
    tag = "ingest",
    params(("code" = String, Path, description = "股票代码")),
    responses(
        (status = 200, description = "板块数组", body = [SectorBoard]),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "上游失败 / 解析失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_sector(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> AppResult<HttpResponse> {
    let code = path.into_inner();
    let boards = state.sector.fetch(&state.http, &code).await?;
    persist_sector_boards(&state.db, &boards).await?;
    Ok(HttpResponse::Ok().json(boards))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/news/{code}").route(web::get().to(get_news)))
        .service(web::resource("/reports/{code}").route(web::get().to(get_reports)))
        .service(web::resource("/sector/{code}").route(web::get().to(get_sector)));
}
