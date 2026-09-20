use actix_web::{web::Data, HttpResponse};
use serde::Serialize;
use sqlx::SqlitePool;

#[derive(Serialize, utoipa::ToSchema)]
pub struct Health {
    /// 进程状态（恒为 "ok"，仅在进程能响应 HTTP 时返回）
    pub status: &'static str,
    /// 数据库可达性："ok" / "down"
    pub db: &'static str,
}

/// 健康检查（数据库 ping）
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "服务存活", body = Health),
    )
)]
pub async fn health(db: Data<SqlitePool>) -> HttpResponse {
    let db_ok = sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(db.get_ref())
        .await
        .is_ok();
    HttpResponse::Ok().json(Health {
        status: "ok",
        db: if db_ok { "ok" } else { "down" },
    })
}
