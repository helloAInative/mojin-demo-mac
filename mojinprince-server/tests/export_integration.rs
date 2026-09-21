//! 数据导出端点（ROI #12）：GET /export/day 与收盘归档同构。
use actix_web::{test, web, App};
use chrono::TimeZone;
use httpmock::prelude::*;
use mojinprince_server::{api, state::AppState, Config};
use serde_json::Value;
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
        "mojinprince-export-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

#[actix_web::test]
async fn export_day_returns_archive_shape() {
    let _upstream = MockServer::start(); // 本用例不依赖上游
    let state = fresh_state().await;
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let morning = tz.with_ymd_and_hms(2026, 9, 22, 10, 30, 0).unwrap();
    let close = tz.with_ymd_and_hms(2026, 9, 22, 15, 0, 0).unwrap();
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
    sqlx::query("INSERT INTO quote(code, ts, name, price, prev, source) VALUES(?,?,?,?,?,?)")
        .bind("sz300623")
        .bind(close.timestamp_millis())
        .bind("捷捷微电")
        .bind(35.2)
        .bind(34.8)
        .bind("tencent")
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO signal_event(id, at, kind, code, title) VALUES(?,?,?,?,?)")
        .bind("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
        .bind(morning.timestamp_millis())
        .bind("level")
        .bind("sz300623")
        .bind("跌破支撑")
        .execute(&state.db)
        .await
        .unwrap();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::export::configure)),
    )
    .await;

    // 指定日期
    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/export/day?date=2026-09-22")
            .to_request(),
    )
    .await;
    assert_eq!(doc["date"], "2026-09-22");
    assert_eq!(doc["codes"]["sz300623"]["name"], "捷捷微电");
    assert_eq!(
        doc["codes"]["sz300623"]["minutes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(doc["codes"]["sz300623"]["quote"]["close"], 35.2);
    assert_eq!(doc["signals"].as_array().unwrap().len(), 1);
    assert_eq!(doc["signals"][0]["title"], "跌破支撑");

    // 缺省日期 → 200（最近已收盘交易日，空数据也返回结构）
    let default: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/export/day")
            .to_request(),
    )
    .await;
    assert!(!default["date"].as_str().unwrap().is_empty());

    // 非法日期 → 400
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/export/day?date=not-a-date")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), 400);
}
