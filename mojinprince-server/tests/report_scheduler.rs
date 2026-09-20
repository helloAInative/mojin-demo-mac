//! 阶段 4：自动触发报告生成——幂等性与"已存在则跳过"。
//!
//! 覆盖 `scheduler::ensure_report`：
//! - 首次调用生成基线版并落 `scheduled_report`
//! - 重复调用（含手动刷新后的记录）不再重建，避免覆盖客户端 context
use mojinprince_server::{service::scheduler, state::AppState, Config};
use std::time::{SystemTime, UNIX_EPOCH};

fn config(database_url: String) -> Config {
    Config {
        bind_addr: "127.0.0.1:0".into(),
        database_url,
        quote_timeout_ms: 1_000,
        ai_timeout_ms: 5_000,
        quote_fail_threshold: 3,
        quote_sources: vec!["sina".into()],
        api_prefix: "/api/v1".into(),
        jwt_secret: String::new(),
    }
}

async fn fresh_state() -> AppState {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let thread_id = format!("{:?}", std::thread::current().id());
    let safe_thread = thread_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>();
    let path = std::env::temp_dir().join(format!(
        "mojinprince-report-scheduler-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

#[tokio::test]
async fn ensure_report_generates_baseline_once_and_skips_existing() {
    let state = fresh_state().await;
    let today = scheduler::today_cn();

    scheduler::ensure_report(&state, "daily", today)
        .await
        .unwrap();
    let (count, body): (i64, String) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(MAX(body),'') FROM scheduled_report WHERE kind='daily'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert!(body.contains("后端摘要"));

    // 已存在 → 再次调用跳过；模拟客户端手动刷新后记录不被自动生成覆盖
    sqlx::query("UPDATE scheduled_report SET body='manual-refresh' WHERE kind='daily'")
        .execute(&state.db)
        .await
        .unwrap();
    scheduler::ensure_report(&state, "daily", today)
        .await
        .unwrap();
    let body_after: String =
        sqlx::query_scalar("SELECT body FROM scheduled_report WHERE kind='daily'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(body_after, "manual-refresh");
}

#[tokio::test]
async fn ensure_report_weekly_uses_iso_week_period_key() {
    let state = fresh_state().await;
    let today = scheduler::today_cn();

    scheduler::ensure_report(&state, "weekly", today)
        .await
        .unwrap();
    let period_key: String =
        sqlx::query_scalar("SELECT period_key FROM scheduled_report WHERE kind='weekly'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(period_key, scheduler::period_key_for("weekly", today));
}
