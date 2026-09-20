use crate::error::{AppError, AppResult, StrWrap};
use crate::model::{ReviewContext, ScheduledReport};
use crate::service::scheduler;
use crate::state::AppState;
use actix_web::{web, HttpResponse};
use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ReviewQuery {
    kind: Option<String>,
    limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct RunQuery {
    kind: Option<String>,
}

#[utoipa::path(get, path = "/api/v1/reviews", tag = "review",
    params(("kind" = Option<String>, Query), ("limit" = Option<i64>, Query)),
    responses((status = 200, body = [ScheduledReport]))) ]
pub async fn list(
    state: web::Data<AppState>,
    query: web::Query<ReviewQuery>,
) -> AppResult<HttpResponse> {
    let limit = query.limit.unwrap_or(20).clamp(1, 200);
    let raw = if let Some(kind) = query.kind.as_deref().filter(|value| !value.is_empty()) {
        sqlx::query_as::<_, ReportRaw>(
            "SELECT id,kind,period_key,title,body,payload,created_at FROM scheduled_report \
             WHERE kind=? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(kind)
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, ReportRaw>(
            "SELECT id,kind,period_key,title,body,payload,created_at FROM scheduled_report \
             ORDER BY created_at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&state.db)
        .await?
    };
    Ok(HttpResponse::Ok().json(raw.into_iter().map(ScheduledReport::from).collect::<Vec<_>>()))
}

/// 生成一份日报 / 周报。
///
/// - `?kind=daily|weekly`（默认 daily）
/// - 可选 JSON body `{tickets, diary, signals, focusCodes}`：补全 Swift 端内存
///   里"信号 / 委托 / 日记"等数据。
///
/// **幂等保证**：按 `(kind, period_key)` UPSERT，重复调用会刷新同一条记录，
/// 不会生成多份。`created_at` 仅在首次生成时写入；之后只更新 body / payload。
#[utoipa::path(post, path = "/api/v1/reviews/run", tag = "review",
    params(("kind" = Option<String>, Query)),
    request_body = Option<ReviewContext>,
    responses(
        (status = 200, description = "生成的报告（首次或刷新后）", body = ScheduledReport),
        (status = 400, description = "kind 非法", body = crate::error::AppErrorBody),
    ))]
pub async fn run(
    state: web::Data<AppState>,
    query: web::Query<RunQuery>,
    body: Option<web::Json<ReviewContext>>,
) -> AppResult<HttpResponse> {
    let context = body.map(|json| json.into_inner()).unwrap_or_default();
    let kind = query.kind.as_deref().unwrap_or("daily");
    let report: ScheduledReport = match kind {
        "daily" => scheduler::generate_daily_report(&state, &context).await?,
        "weekly" => scheduler::generate_weekly_report(&state, &context).await?,
        _ => {
            return Err(AppError::BadRequest(StrWrap::from(
                "kind must be daily or weekly",
            )));
        }
    };
    Ok(HttpResponse::Ok().json(report))
}

#[derive(sqlx::FromRow)]
struct ReportRaw {
    id: String,
    kind: String,
    period_key: String,
    title: String,
    body: String,
    payload: String,
    created_at: i64,
}

impl From<ReportRaw> for ScheduledReport {
    fn from(row: ReportRaw) -> Self {
        Self {
            id: row.id,
            kind: row.kind,
            period_key: row.period_key,
            title: row.title,
            body: row.body,
            payload: serde_json::from_str(&row.payload).unwrap_or_else(|_| serde_json::json!({})),
            created_at: DateTime::<Utc>::from_timestamp_millis(row.created_at).unwrap_or_else(Utc::now),
        }
    }
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/reviews").route(web::get().to(list)))
        .service(web::resource("/reviews/run").route(web::post().to(run)));
}
