//! 智能推荐 API：`GET /picks`（默认最近一份）+ `POST /picks/run`（可带 AI 配置精排）。
use actix_web::{get, post, web, HttpResponse};
use chrono::FixedOffset;

use crate::error::AppResult;
use crate::model::pick::{PicksDocument, PicksRunRequest};
use crate::service::{pick, scheduler};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct PicksQuery {
    /// YYYY-MM-DD；缺省取库中最近一天
    pub date: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/picks",
    tag = "pick",
    params(PicksQuery),
    responses(
        (status = 200, description = "当日推荐 + 近 30 天 T+5 回测统计", body = PicksDocument),
    )
)]
#[get("/picks")]
pub async fn list(
    state: web::Data<AppState>,
    q: web::Query<PicksQuery>,
) -> AppResult<HttpResponse> {
    let date = q.date.as_deref().filter(|d| !d.trim().is_empty());
    let doc: PicksDocument = pick::list_picks(&state.db, date).await?;
    Ok(HttpResponse::Ok().json(doc))
}

#[utoipa::path(
    post,
    path = "/api/v1/picks/run",
    tag = "pick",
    request_body(content = Option<PicksRunRequest>, description = "ai 字段可选：客户端透传 AI 配置做精排（密钥不落库）"),
    responses(
        (status = 200, description = "已生成 / 刷新的当日推荐", body = PicksDocument),
        (status = 502, description = "涨幅榜或日 K 上游失败", body = crate::error::AppErrorBody),
    )
)]
#[post("/picks/run")]
pub async fn run(
    state: web::Data<AppState>,
    body: Option<web::Json<PicksRunRequest>>,
) -> AppResult<HttpResponse> {
    let tz = FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
    let now = chrono::Utc::now().with_timezone(&tz);
    let date = scheduler::pick_target_date(now);
    let ai = body.map(|b| b.into_inner().ai).flatten();
    let doc: PicksDocument = pick::generate_picks(&state, date, ai.as_ref()).await?;
    Ok(HttpResponse::Ok().json(doc))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list).service(run);
}
