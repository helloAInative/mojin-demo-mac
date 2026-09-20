//! 行情网关阶段 1：实时报价 + 分时 + 日 K
use crate::error::{AppError, AppResult};
use crate::service::quote::{history, normalize_code, QuoteError};
use crate::service::scheduler::persist_quote;
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use serde::Deserialize;

/// 单标的实时报价。failover 顺序：sina → tencent → eastmoney；
/// 三个源都失败时返回 502 upstream_exhausted。
#[utoipa::path(
    get,
    path = "/api/v1/quote/{code}",
    tag = "quote",
    params(
        ("code" = String, Path, description = "股票代码，例如 600460 / sh600460 / sz000001 / bj835899"),
    ),
    responses(
        (status = 200, description = "实时报价", body = crate::model::Quote),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "所有行情源失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_quote(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> AppResult<HttpResponse> {
    let code = path.into_inner();
    let (q, src) = state.failover.fetch_quote(&state.http, &code).await?;
    // 写一份到 DB（行情落地，方便回测/统计）
    let _ = persist_quote(&state, &q).await;
    state.quote_hub.publish(q.clone());
    let _ = src; // 已记录到 DB.source
    Ok(HttpResponse::Ok().json(q))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<i64>,
}

/// 当日分时（默认 240 根，约一个完整交易日）。从腾讯拉取后写入 SQLite。
#[utoipa::path(
    get,
    path = "/api/v1/quote/{code}/minutes",
    tag = "quote",
    params(
        ("code" = String, Path, description = "股票代码"),
        ("limit" = Option<i64>, Query, description = "返回最近 N 条（1..=1440）"),
    ),
    responses(
        (status = 200, description = "分时数组", body = [crate::model::MinuteBar]),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "上游失败 / 解析失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_minutes(
    state: web::Data<AppState>,
    path: web::Path<String>,
    q: web::Query<HistoryQuery>,
) -> AppResult<HttpResponse> {
    let code = normalize_code(&path.into_inner()).map_err(history_error)?;
    let limit = q.limit.unwrap_or(240).clamp(1, 1440);
    let mut bars = history::fetch_minutes(&state.http, &code)
        .await
        .map_err(history_error)?;
    if bars.len() > limit as usize {
        bars = bars.split_off(bars.len() - limit as usize);
    }
    let mut tx = state.db.begin().await?;
    for bar in &bars {
        sqlx::query(
            "INSERT OR REPLACE INTO minute_bar(code,ts,price,avg_price,volume,amount) VALUES(?,?,?,?,?,?)",
        )
        .bind(&bar.code)
        .bind(bar.ts.timestamp_millis())
        .bind(bar.price)
        .bind(bar.avg_price)
        .bind(bar.volume)
        .bind(bar.amount)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(HttpResponse::Ok().json(bars))
}

/// 前复权日 K（默认 60 根）。从腾讯拉取后写入 SQLite。
#[utoipa::path(
    get,
    path = "/api/v1/quote/{code}/days",
    tag = "quote",
    params(
        ("code" = String, Path, description = "股票代码"),
        ("limit" = Option<i64>, Query, description = "返回最近 N 条（1..=600）"),
    ),
    responses(
        (status = 200, description = "日 K 数组", body = [crate::model::DayBar]),
        (status = 400, description = "代码非法", body = crate::error::AppErrorBody),
        (status = 502, description = "上游失败 / 解析失败", body = crate::error::AppErrorBody),
    )
)]
pub async fn get_days(
    state: web::Data<AppState>,
    path: web::Path<String>,
    q: web::Query<HistoryQuery>,
) -> AppResult<HttpResponse> {
    let code = normalize_code(&path.into_inner()).map_err(history_error)?;
    let limit = q.limit.unwrap_or(60).clamp(1, 600);
    let mut bars = history::fetch_days(&state.http, &code, limit)
        .await
        .map_err(history_error)?;
    if bars.len() > limit as usize {
        bars = bars.split_off(bars.len() - limit as usize);
    }
    let mut tx = state.db.begin().await?;
    for bar in &bars {
        sqlx::query(
            "INSERT OR REPLACE INTO day_bar(code,date,open,high,low,close,volume,amount) VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(&bar.code)
        .bind(&bar.date)
        .bind(bar.open)
        .bind(bar.high)
        .bind(bar.low)
        .bind(bar.close)
        .bind(bar.volume)
        .bind(bar.amount)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(HttpResponse::Ok().json(bars))
}

fn history_error(error: QuoteError) -> AppError {
    match error {
        QuoteError::BadCode(code) => AppError::BadRequest(code.into()),
        QuoteError::Network(message) => AppError::UpstreamExhausted {
            tries: 1,
            source: message.into(),
        },
        other => AppError::UpstreamParse(other.to_string().into()),
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/quote")
            .route("/{code}", web::get().to(get_quote))
            .route("/{code}/minutes", web::get().to(get_minutes))
            .route("/{code}/days", web::get().to(get_days)),
    );
}
