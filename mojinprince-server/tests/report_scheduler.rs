//! 阶段 4：自动触发报告生成——幂等性与"已存在则跳过"。
//!
//! 覆盖 `scheduler::ensure_report`：
//! - 首次调用生成基线版并落 `scheduled_report`
//! - 重复调用（含手动刷新后的记录）不再重建，避免覆盖客户端 context
use chrono::TimeZone;
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

#[tokio::test]
async fn daily_archive_dumps_after_close_and_is_idempotent() {
    let state = fresh_state().await;
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    // 2026-09-22 周二 16:00（收盘后）；10:30 的分时与收盘报价落在同一天
    let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
    let morning = tz.with_ymd_and_hms(2026, 9, 22, 10, 30, 0).unwrap();
    let close = tz.with_ymd_and_hms(2026, 9, 22, 15, 0, 0).unwrap();

    // 盘中（11:00）：今天没收盘 → 目标是昨天周一，先补一份空归档
    let midday = scheduler::maybe_dump_daily_archive(
        &state,
        tz.with_ymd_and_hms(2026, 9, 22, 11, 0, 0).unwrap(),
    )
    .await
    .unwrap();
    assert!(midday, "盘中应补昨天的归档");
    let raw = state
        .cfg
        .database_url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:");
    let monday_path = std::path::Path::new(raw)
        .parent()
        .unwrap()
        .join("archives")
        .join("2026-09-21.json");
    assert!(monday_path.exists(), "盘中补的是周一归档");

    // 种子数据：自选 1 只 + 2 根分时 + 收盘报价 + 1 条信号 + 1 条 AI 用量 + 1 条持仓
    sqlx::query("INSERT INTO watchlist(code, name, added_at, updated_at) VALUES('sz300623', '捷捷微电', 0, 0)")
        .execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO minute_bar(code, ts, price, avg_price, volume) VALUES(?,?,?,?,?)")
        .bind("sz300623")
        .bind(morning.timestamp_millis())
        .bind(35.1)
        .bind(35.05)
        .bind(1200)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO minute_bar(code, ts, price, avg_price, volume) VALUES(?,?,?,?,?)")
        .bind("sz300623")
        .bind(close.timestamp_millis())
        .bind(35.2)
        .bind(35.1)
        .bind(800)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO quote(code, ts, name, price, prev, open, high, low, source) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind("sz300623").bind(close.timestamp_millis()).bind("捷捷微电")
        .bind(35.2).bind(34.8).bind(34.9).bind(35.5).bind(34.7).bind("tencent")
        .execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO signal_event(id, at, kind, code, title) VALUES(?,?,?,?,?)")
        .bind("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
        .bind(morning.timestamp_millis())
        .bind("ai")
        .bind("sz300623")
        .bind("AI 分析")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO ai_usage(at, provider, model, success, tokens_in, tokens_out, cost) VALUES(?,?,?,?,?,?,?)")
        .bind(morning.timestamp_millis()).bind("openai").bind("qwen3.7-plus").bind(1).bind(100).bind(200).bind(0.01)
        .execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO position(code, cost, shares, stop_loss, take_profit, position_pct, updated_at) VALUES(?,?,?,?,?,?,?)")
        .bind("sz300623").bind(31.234).bind(2700.0).bind(32.57).bind(0.0).bind(100.0).bind(0)
        .execute(&state.db).await.unwrap();

    // 收盘后 dump
    let dumped = scheduler::maybe_dump_daily_archive(
        &state,
        tz.with_ymd_and_hms(2026, 9, 22, 16, 0, 0).unwrap(),
    )
    .await
    .unwrap();
    assert!(dumped, "16:00 应 dump");

    // 归档路径：临时库同目录 archives/2026-09-22.json
    let archive_path = std::path::Path::new(raw)
        .parent()
        .unwrap()
        .join("archives")
        .join("2026-09-22.json");
    assert!(
        archive_path.exists(),
        "归档文件应存在：{}",
        archive_path.display()
    );
    let text = std::fs::read_to_string(&archive_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["date"], "2026-09-22");
    assert_eq!(json["codes"]["sz300623"]["name"], "捷捷微电");
    assert_eq!(
        json["codes"]["sz300623"]["minutes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(json["codes"]["sz300623"]["quote"]["close"], 35.2);
    assert_eq!(json["signals"].as_array().unwrap().len(), 1);
    assert_eq!(json["ai_usage"]["total"], 1);
    assert_eq!(json["positions"][0]["shares"], 2700.0);
    assert_eq!(json["positions"][0]["stop_loss"], 32.57);

    // 幂等：再次调用跳过
    let again = scheduler::maybe_dump_daily_archive(
        &state,
        tz.with_ymd_and_hms(2026, 9, 22, 17, 0, 0).unwrap(),
    )
    .await
    .unwrap();
    assert!(!again, "同一天已存在应跳过");

    // 清理临时归档
    let _ = std::fs::remove_file(&archive_path);
    let _ = std::fs::remove_file(&monday_path);
}

#[tokio::test]
async fn daily_archive_on_weekend_targets_friday() {
    let state = fresh_state().await;
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    // 2026-09-20 周日 10:00 → 目标交易日 2026-09-18 周五（空库也会写空归档）
    let dumped = scheduler::maybe_dump_daily_archive(
        &state,
        tz.with_ymd_and_hms(2026, 9, 20, 10, 0, 0).unwrap(),
    )
    .await
    .unwrap();
    assert!(dumped, "周末应补 dump");
    let raw = state
        .cfg
        .database_url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:");
    let archive_path = std::path::Path::new(raw)
        .parent()
        .unwrap()
        .join("archives")
        .join("2026-09-18.json");
    assert!(archive_path.exists(), "归档目标应为周五");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&archive_path).unwrap()).unwrap();
    assert_eq!(json["date"], "2026-09-18");
    assert_eq!(json["signals"].as_array().unwrap().len(), 0);
    let _ = std::fs::remove_file(&archive_path);
}
