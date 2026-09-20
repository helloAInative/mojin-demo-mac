//! 阶段 3：信号、持仓、自选和设置的 HTTP API。
use crate::error::{AppError, AppResult, StrWrap};
use crate::model::{
    PositionDto, PositionUpdate, SettingsDocument, SignalEventDto, SignalFeedbackRequest,
    WatchlistItem, WatchlistUpdate,
};
use crate::repo::{SettingsRepo, SignalEventRow, SignalRepo};
use crate::service::quote::normalize_code;
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct SignalQuery {
    code: Option<String>,
    limit: Option<i64>,
}

#[utoipa::path(get, path = "/api/v1/signals", tag = "data",
    params(("code" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = [SignalEventDto]))) ]
pub async fn list_signals(
    state: web::Data<AppState>,
    query: web::Query<SignalQuery>,
) -> AppResult<HttpResponse> {
    let repo = SignalRepo::new(state.db.clone());
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let rows = match query.code.as_deref() {
        Some(code) if !code.trim().is_empty() => {
            let code = normalize(code)?;
            repo.list_by_code(&code, limit).await?
        }
        _ => repo.list_recent(limit).await?,
    };
    Ok(HttpResponse::Ok().json(rows.into_iter().map(signal_dto).collect::<Vec<_>>()))
}

#[utoipa::path(post, path = "/api/v1/signals", tag = "data",
    request_body = SignalEventDto,
    responses((status = 201, body = SignalEventDto), (status = 400, body = crate::error::AppErrorBody))) ]
pub async fn put_signal(
    state: web::Data<AppState>,
    body: web::Json<SignalEventDto>,
) -> AppResult<HttpResponse> {
    let mut event = body.into_inner();
    if event.id.trim().is_empty() || event.kind.trim().is_empty() {
        return Err(AppError::BadRequest(StrWrap::from(
            "id and kind are required",
        )));
    }
    event.code = normalize(&event.code)?;
    let row = signal_row(&event)?;
    SignalRepo::new(state.db.clone()).upsert(&row).await?;
    Ok(HttpResponse::Created().json(event))
}

#[utoipa::path(delete, path = "/api/v1/signals", tag = "data",
    params(("code" = Option<String>, Query)), responses((status = 204))) ]
pub async fn delete_signals(
    state: web::Data<AppState>,
    query: web::Query<SignalQuery>,
) -> AppResult<HttpResponse> {
    if let Some(code) = query.code.as_deref().filter(|code| !code.trim().is_empty()) {
        sqlx::query("DELETE FROM signal_event WHERE code=?")
            .bind(normalize(code)?)
            .execute(&state.db)
            .await?;
    } else {
        sqlx::query("DELETE FROM signal_event")
            .execute(&state.db)
            .await?;
    }
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(post, path = "/api/v1/signals/{id}/feedback", tag = "data",
    params(("id" = String, Path)), request_body = SignalFeedbackRequest,
    responses((status = 200), (status = 400, body = crate::error::AppErrorBody),
               (status = 404, body = crate::error::AppErrorBody))) ]
pub async fn signal_feedback(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<SignalFeedbackRequest>,
) -> AppResult<HttpResponse> {
    let action = body.action.trim().to_lowercase();
    if !["accept", "ignore", "partial", "opened", "sold"].contains(&action.as_str()) {
        return Err(AppError::BadRequest(StrWrap::from(
            "unsupported feedback action",
        )));
    }
    let id = path.into_inner();
    let exists: Option<(String,)> = sqlx::query_as("SELECT id FROM signal_event WHERE id=?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::NotFound(StrWrap::from("signal")));
    }
    let patch = json!({
        "userAction": action,
        "feedbackNote": body.note.clone().unwrap_or_default(),
        "feedbackAt": Utc::now().to_rfc3339(),
    });
    SignalRepo::new(state.db.clone())
        .patch_meta(&id, &patch)
        .await?;
    Ok(HttpResponse::Ok().json(patch))
}

#[utoipa::path(get, path = "/api/v1/positions", tag = "data",
    responses((status = 200, body = [PositionDto]))) ]
pub async fn list_positions(state: web::Data<AppState>) -> AppResult<HttpResponse> {
    let rows = sqlx::query_as::<_, PositionDto>(
        "SELECT code,cost,shares,stop_loss,take_profit,position_pct,note,updated_at \
         FROM position ORDER BY code",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(HttpResponse::Ok().json(rows))
}

#[utoipa::path(put, path = "/api/v1/positions/{code}", tag = "data",
    params(("code" = String, Path)), request_body = PositionUpdate,
    responses((status = 200, body = PositionDto), (status = 400, body = crate::error::AppErrorBody))) ]
pub async fn put_position(
    state: web::Data<AppState>,
    path: web::Path<String>,
    body: web::Json<PositionUpdate>,
) -> AppResult<HttpResponse> {
    let code = normalize(&path.into_inner())?;
    let p = body.into_inner();
    for value in [p.cost, p.shares, p.stop_loss, p.take_profit, p.position_pct] {
        if !value.is_finite() || value < 0.0 {
            return Err(AppError::BadRequest(StrWrap::from(
                "position values must be finite and non-negative",
            )));
        }
    }
    let updated_at = Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO position(code,cost,shares,stop_loss,take_profit,position_pct,note,updated_at) \
         VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(code) DO UPDATE SET cost=excluded.cost, \
         shares=excluded.shares,stop_loss=excluded.stop_loss,take_profit=excluded.take_profit, \
         position_pct=excluded.position_pct,note=excluded.note,updated_at=excluded.updated_at",
    ).bind(&code).bind(p.cost).bind(p.shares).bind(p.stop_loss).bind(p.take_profit)
        .bind(p.position_pct).bind(&p.note).bind(updated_at).execute(&state.db).await?;
    Ok(HttpResponse::Ok().json(PositionDto {
        code,
        cost: p.cost,
        shares: p.shares,
        stop_loss: p.stop_loss,
        take_profit: p.take_profit,
        position_pct: p.position_pct,
        note: p.note,
        updated_at,
    }))
}

#[utoipa::path(get, path = "/api/v1/watchlist", tag = "data",
    responses((status = 200, body = [WatchlistItem]))) ]
pub async fn list_watchlist(state: web::Data<AppState>) -> AppResult<HttpResponse> {
    let rows = sqlx::query_as::<_, WatchlistRaw>(
        "SELECT code,name,market,pinned,grp,added_at,updated_at FROM watchlist \
         ORDER BY pinned DESC, added_at ASC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(HttpResponse::Ok().json(
        rows.into_iter()
            .map(WatchlistItem::from)
            .collect::<Vec<_>>(),
    ))
}

#[utoipa::path(post, path = "/api/v1/watchlist", tag = "data",
    request_body = WatchlistUpdate, responses((status = 200, body = WatchlistItem))) ]
pub async fn put_watchlist(
    state: web::Data<AppState>,
    body: web::Json<WatchlistUpdate>,
) -> AppResult<HttpResponse> {
    let item = body.into_inner();
    let code = normalize(&item.code)?;
    let now = Utc::now().timestamp_millis();
    sqlx::query(
        "INSERT INTO watchlist(code,name,market,pinned,grp,added_at,updated_at) VALUES(?,?,?,?,?,?,?) \
         ON CONFLICT(code) DO UPDATE SET name=excluded.name,market=excluded.market, \
         pinned=excluded.pinned,grp=excluded.grp,updated_at=excluded.updated_at",
    ).bind(&code).bind(&item.name).bind(&item.market).bind(item.pinned)
        .bind(&item.group_name).bind(now).bind(now).execute(&state.db).await?;
    let row: WatchlistRaw = sqlx::query_as(
        "SELECT code,name,market,pinned,grp,added_at,updated_at FROM watchlist WHERE code=?",
    )
    .bind(&code)
    .fetch_one(&state.db)
    .await?;
    Ok(HttpResponse::Ok().json(WatchlistItem::from(row)))
}

#[utoipa::path(delete, path = "/api/v1/watchlist/{code}", tag = "data",
    params(("code" = String, Path)), responses((status = 204))) ]
pub async fn delete_watchlist(
    state: web::Data<AppState>,
    path: web::Path<String>,
) -> AppResult<HttpResponse> {
    let code = normalize(&path.into_inner())?;
    sqlx::query("DELETE FROM watchlist WHERE code=?")
        .bind(code)
        .execute(&state.db)
        .await?;
    Ok(HttpResponse::NoContent().finish())
}

#[utoipa::path(get, path = "/api/v1/settings", tag = "data",
    responses((status = 200, body = SettingsDocument))) ]
pub async fn get_settings(state: web::Data<AppState>) -> AppResult<HttpResponse> {
    let values = SettingsRepo::new(state.db.clone()).get_all().await?;
    Ok(HttpResponse::Ok().json(SettingsDocument {
        values: serde_json::to_value(values).unwrap_or_else(|_| json!({})),
        updated_at: Utc::now(),
    }))
}

#[utoipa::path(put, path = "/api/v1/settings", tag = "data",
    request_body(content = Object), responses((status = 200, body = SettingsDocument))) ]
pub async fn put_settings(
    state: web::Data<AppState>,
    body: web::Json<HashMap<String, Value>>,
) -> AppResult<HttpResponse> {
    if body
        .keys()
        .any(|key| key.trim().is_empty() || key.len() > 128)
    {
        return Err(AppError::BadRequest(StrWrap::from(
            "setting keys must be 1..=128 characters",
        )));
    }
    let values = body.into_inner();
    SettingsRepo::new(state.db.clone())
        .put_many(&values)
        .await?;
    Ok(HttpResponse::Ok().json(SettingsDocument {
        values: serde_json::to_value(values).unwrap_or_else(|_| json!({})),
        updated_at: Utc::now(),
    }))
}

#[derive(sqlx::FromRow)]
struct WatchlistRaw {
    code: String,
    name: Option<String>,
    market: Option<String>,
    pinned: i64,
    grp: Option<String>,
    added_at: i64,
    updated_at: i64,
}

impl From<WatchlistRaw> for WatchlistItem {
    fn from(row: WatchlistRaw) -> Self {
        Self {
            code: row.code,
            name: row.name.unwrap_or_default(),
            market: row.market.unwrap_or_default(),
            pinned: row.pinned != 0,
            group_name: row.grp.unwrap_or_default(),
            added_at: row.added_at,
            updated_at: row.updated_at,
        }
    }
}

fn normalize(code: &str) -> AppResult<String> {
    normalize_code(code).map_err(|error| AppError::BadRequest(StrWrap::from(error.to_string())))
}

fn signal_dto(row: SignalEventRow) -> SignalEventDto {
    let meta = stringify_meta(serde_json::from_str(&row.meta).unwrap_or_else(|_| json!({})));
    SignalEventDto {
        id: row.id,
        at: row.at,
        kind: row.kind,
        code: row.code,
        title: row.title,
        body: row.body,
        price: row.price,
        source: row.source,
        evidence: row.evidence,
        why: row.why,
        meta,
    }
}

/// Swift 的历史模型使用 `[String: String]`。阶段 2 写入的 AI 信号可能含数字或布尔值，
/// 在 API 边界统一转成字符串，避免一个非字符串字段导致整份 meta 解码为空。
fn stringify_meta(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(value) => Value::String(value),
                        Value::Null => Value::String(String::new()),
                        other => Value::String(other.to_string()),
                    };
                    (key, value)
                })
                .collect(),
        ),
        _ => json!({}),
    }
}

fn signal_row(event: &SignalEventDto) -> AppResult<SignalEventRow> {
    let meta = serde_json::to_string(&event.meta)
        .map_err(|error| AppError::BadRequest(StrWrap::from(error.to_string())))?;
    Ok(SignalEventRow {
        id: event.id.clone(),
        at: event.at,
        kind: event.kind.clone(),
        code: event.code.clone(),
        title: event.title.clone(),
        body: event.body.clone(),
        price: event.price,
        source: event.source.clone(),
        evidence: event.evidence.clone(),
        why: event.why.clone(),
        meta,
    })
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::resource("/signals")
            .route(web::get().to(list_signals))
            .route(web::post().to(put_signal))
            .route(web::delete().to(delete_signals)),
    )
    .service(web::resource("/signals/{id}/feedback").route(web::post().to(signal_feedback)))
    .service(web::resource("/positions").route(web::get().to(list_positions)))
    .service(web::resource("/positions/{code}").route(web::put().to(put_position)))
    .service(
        web::resource("/watchlist")
            .route(web::get().to(list_watchlist))
            .route(web::post().to(put_watchlist)),
    )
    .service(web::resource("/watchlist/{code}").route(web::delete().to(delete_watchlist)))
    .service(
        web::resource("/settings")
            .route(web::get().to(get_settings))
            .route(web::put().to(put_settings)),
    );
}
