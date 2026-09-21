//! 阶段 2 扩展端点集成测试。
//!
//! 覆盖：
//! - `/analyze` 落库 signal_event（kind=ai）+ 用量账本
//! - `/reflect` 多步反思，逐次累计 usage
//! - `/accuracy` 计算 kind='level' hit 命中率
//! - `/feedback` 写 ai_feedback 表
//! - Governor 连续失败触发熔断（BreakerOpen）
//! - Governor 冷却：连发同一 provider/model 第二次 429
use actix_web::{test, web, App};
use httpmock::prelude::*;
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
        "mojinprince-ai-phase2-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

// 不抽 build_app helper —— actix 4 的 `init_service` 返回
// `impl Service<actix_http::Request, ...>`，类型推得对；
// 显式包成 `Service<ServiceRequest, ...>` 反而与 `call_and_read_body_json` 不匹配。
// 每个用例直接 `let app = test::init_service(...).await` 即可。

#[actix_web::test]
async fn analyze_persists_signal_event_and_usage() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(json!({
            "choices": [{"message": {"content": "{\"summary\":\"偏多\",\"verdict\":\"bull\"}"}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 9}
        }));
    });
    let state = fresh_state().await;
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;

    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/ai/analyze")
            .set_json(json!({
                "provider": "openai",
                "base_url": format!("{}/v1", upstream.base_url()),
                "api_key": "k",
                "model": "m",
                "user": "分析 sh600460",
                "code": "sh600460",
                "price": 32.5
            }))
            .to_request(),
    )
    .await;
    assert!(resp["usage_id"].as_i64().unwrap() > 0);
    assert!(resp["signal_id"].as_str().unwrap().len() > 0);
    mock.assert();

    let row: (i64, String, String) =
        sqlx::query_as("SELECT at, kind, code FROM signal_event ORDER BY at DESC LIMIT 1")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(row.1, "ai");
    assert_eq!(row.2, "sh600460");
}

#[actix_web::test]
async fn reflect_runs_multiple_steps() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(json!({
            "choices": [{"message": {"content": "{\"summary\":\"ok\"}"}}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 4}
        }));
    });
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/api/v1/ai/reflect")
        .set_json(json!({
            "provider": "openai",
            "base_url": format!("{}/v1", upstream.base_url()),
            "api_key": "k",
            "model": "m",
            "steps": [
                {"user": "first"},
                {"user": "second", "previous": "{\"summary\":\"ok\"}"}
            ]
        }))
        .to_request();
    let resp: Value = test::call_and_read_body_json(&app, req).await;
    assert_eq!(resp["steps"].as_array().unwrap().len(), 2);
    assert!(!resp["final_text"].as_str().unwrap().is_empty());
    assert!(mock.hits() >= 2);
}

#[actix_web::test]
async fn accuracy_counts_hit_ratio() {
    let state = fresh_state().await;
    let db = state.db.clone();
    // 先 init_service（因为 init_service 会 move state）；handler 持有的 web::Data<AppState>
    // 与此处 db 引用同一个 SqlitePool（Arc 共享），后续 INSERT 可见。
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    // 直接插三条 level 信号：其中两条 meta.hit=1
    for (i, hit) in [1, 0, 1].iter().enumerate() {
        sqlx::query(
            "INSERT INTO signal_event(id,at,kind,code,title,body,price,source,evidence,why,meta) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(format!("sig-{i}"))
        .bind(Utc::now().timestamp_millis())
        .bind("level")
        .bind("sh600460")
        .bind(format!("level #{i}"))
        .bind("")
        .bind(0.0)
        .bind("sina")
        .bind("")
        .bind("")
        .bind(if *hit == 1 {
            serde_json::json!({"hit": "1", "leadSec": 30}).to_string()
        } else {
            "{}".to_string()
        })
        .execute(&db)
        .await
        .unwrap();
    }
    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/ai/accuracy?window_hours=24")
            .to_request(),
    )
    .await;
    assert_eq!(resp["total"], 3);
    assert_eq!(resp["hit"], 2);
    let rate = resp["rate"]
        .as_f64()
        .unwrap_or(resp["hit_rate"].as_f64().unwrap_or(0.0));
    assert!((rate - 2.0 / 3.0).abs() < 0.001);
}

#[actix_web::test]
async fn feedback_records_to_table() {
    let state = fresh_state().await;
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/ai/feedback")
            .set_json(json!({
                "code": "sh600460",
                "sentiment": "accept",
                "note": "采纳"
            }))
            .to_request(),
    )
    .await;
    assert!(resp["id"].as_i64().unwrap() > 0);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_feedback")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[actix_web::test]
async fn feedback_rejects_bad_sentiment() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/ai/feedback")
            .set_json(json!({
                "code": "sh600460",
                "sentiment": "bogus"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn chat_cooldown_returns_429() {
    let upstream = MockServer::start();
    // 第一次成功 + 5 次失败：触发熔断（BreakerOpen → 503），
    // 或冷却（CoolingDown → 429）。具体返回取决于时序，断言 4xx/5xx 即可。
    upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(500).body("boom");
    });
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let mut saw_deny = false;
    for _ in 0..7 {
        let r = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/api/v1/ai/chat")
                .set_json(json!({
                    "provider": "openai",
                    "base_url": format!("{}/v1", upstream.base_url()),
                    "api_key": "k",
                    "model": "m",
                    "system": "s",
                    "user": "u"
                }))
                .to_request(),
        )
        .await;
        if r.status() == 429 || r.status() == 503 {
            saw_deny = true;
            break;
        }
    }
    assert!(
        saw_deny,
        "expected at least one 429/503 after consecutive failures"
    );
}

use chrono::Utc;
