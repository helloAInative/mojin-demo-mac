//! 阶段 3 数据 API：信号、持仓、自选和设置。
use actix_web::{http::StatusCode, test, web, App};
use mojinprince_server::{api, state::AppState, Config};
use serde_json::{json, Value};
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
    // 把 thread id 也加入文件名：cargo test 多线程并行时，同进程内两个测试可能
    // 在同一时钟粒度内取到相同 nonce，共用 sqlite 路径会撞 _sqlx_migrations UNIQUE 约束。
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
        "mojinprince-data-phase3-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

#[actix_web::test]
async fn signal_round_trip_and_feedback_merge_meta() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::data::configure)),
    )
    .await;

    let event = json!({
        "id": "4b91f282-96f6-4ca3-bbf6-f6b7fa5ef9c8",
        "at": "2026-09-20T08:00:00Z",
        "kind": "level",
        "code": "600460",
        "title": "接近支撑",
        "body": "测试",
        "price": 32.1,
        "source": "test",
        "evidence": "unit",
        "why": "support",
        "meta": {"levelKey": "support", "hit": false, "leadSec": 3}
    });
    let req = test::TestRequest::post()
        .uri("/api/v1/signals")
        .set_json(&event)
        .to_request();
    let response = test::call_service(&app, req).await;
    assert_eq!(response.status(), StatusCode::CREATED);

    let req = test::TestRequest::post()
        .uri("/api/v1/signals/4b91f282-96f6-4ca3-bbf6-f6b7fa5ef9c8/feedback")
        .set_json(json!({"action": "accept", "note": "已采纳"}))
        .to_request();
    assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);

    let rows: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/signals?code=sh600460&limit=10")
            .to_request(),
    )
    .await;
    assert_eq!(rows[0]["code"], "sh600460");
    assert_eq!(rows[0]["meta"]["levelKey"], "support");
    assert_eq!(rows[0]["meta"]["hit"], "false");
    assert_eq!(rows[0]["meta"]["leadSec"], "3");
    assert_eq!(rows[0]["meta"]["userAction"], "accept");

    let response = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri("/api/v1/signals")
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let rows: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/api/v1/signals").to_request(),
    )
    .await;
    assert!(rows.as_array().unwrap().is_empty());
}

#[actix_web::test]
async fn position_and_watchlist_are_upserted() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::data::configure)),
    )
    .await;

    let position: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::put()
            .uri("/api/v1/positions/300623")
            .set_json(json!({"cost": 28.5, "shares": 1000, "stopLoss": 26.0, "positionPct": 30}))
            .to_request(),
    )
    .await;
    assert_eq!(position["code"], "sz300623");
    assert_eq!(position["stopLoss"], 26.0);

    let item: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/watchlist")
            .set_json(json!({
                "code": "300623", "name": "捷捷微电", "market": "sz",
                "pinned": true, "group": "半导体"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(item["code"], "sz300623");
    assert_eq!(item["group"], "半导体");

    let positions: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/positions")
            .to_request(),
    )
    .await;
    assert_eq!(positions.as_array().unwrap().len(), 1);
    let watchlist: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/watchlist")
            .to_request(),
    )
    .await;
    assert_eq!(watchlist.as_array().unwrap().len(), 1);
}

#[actix_web::test]
async fn settings_merge_keys_without_losing_previous_values() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::data::configure)),
    )
    .await;

    for value in [
        json!({"theme": "dark", "alertsEnabled": true}),
        json!({"theme": "light", "fontSize": "lg"}),
    ] {
        let req = test::TestRequest::put()
            .uri("/api/v1/settings")
            .set_json(value)
            .to_request();
        assert_eq!(test::call_service(&app, req).await.status(), StatusCode::OK);
    }

    let document: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/settings")
            .to_request(),
    )
    .await;
    assert_eq!(document["values"]["theme"], "light");
    assert_eq!(document["values"]["alertsEnabled"], true);
    assert_eq!(document["values"]["fontSize"], "lg");
}
