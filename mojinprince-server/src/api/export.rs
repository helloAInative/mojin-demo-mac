//! 数据导出 API（ROI #12）：`GET /export/day?date=` 返回某交易日归档 JSON
//! （与收盘自动 dump 的 `data/archives/{date}.json` 同构，复用同一组装逻辑）。
use actix_web::{get, web, HttpResponse};
use chrono::{FixedOffset, NaiveDate, Utc};

use crate::error::{AppError, AppResult};
use crate::service::scheduler;
use crate::state::AppState;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ExportQuery {
    /// YYYY-MM-DD；缺省为最近已收盘交易日
    pub date: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/export/day",
    tag = "export",
    params(ExportQuery),
    responses(
        (status = 200, description = "当日归档：分时 / 收盘快照 / 信号 / AI 用量 / 持仓", body = Object),
        (status = 400, description = "日期格式非法", body = crate::error::AppErrorBody),
    )
)]
#[get("/export/day")]
pub async fn day(
    state: web::Data<AppState>,
    q: web::Query<ExportQuery>,
) -> AppResult<HttpResponse> {
    let date = match q.date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(text) => NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| {
            AppError::BadRequest(format!("date 非法（应为 YYYY-MM-DD）：{text}").into())
        })?,
        None => {
            let tz = FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
            let now = Utc::now().with_timezone(&tz);
            scheduler::last_closed_trading_day(now)
        }
    };
    let archive = scheduler::build_day_archive(&state, date).await?;
    Ok(HttpResponse::Ok().json(archive))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(day);
}
