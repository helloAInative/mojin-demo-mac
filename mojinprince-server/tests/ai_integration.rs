//! 阶段 2 AI 网关集成测试
//!
//! 覆盖：
//! 1. `POST /api/v1/ai/chat` 把请求转给上游 OpenAI 兼容协议并把用量写入 SQLite。
//! 2. `POST /api/v1/ai/chat` 用 `provider=ollama` 走 Ollama `/api/chat`。
//! 3. 非法 `provider` 直接 400，并把失败记录写入账本。
//! 4. 非法请求体（如 base_url 不是 http(s)）直接 400，不写账本。
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
        "mojinprince-ai-test-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

#[actix_web::test]
async fn openai_chat_is_proxied_and_recorded() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .header("authorization", "Bearer test-token")
            .json_body_partial(r#"{"model":"test-model","enable_thinking":false}"#);
        then.status(200).json_body(json!({
            "choices": [{"message": {"content": "{\"summary\":\"测试成功\"}"}}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 7}
        }));
    });
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;

    let request = test::TestRequest::post()
        .uri("/api/v1/ai/chat")
        .set_json(json!({
            "provider": "openai",
            "base_url": format!("{}/v1", upstream.base_url()),
            "api_key": "test-token",
            "model": "test-model",
            "system": "你是看盘助手",
            "user": "分析 sh600460",
            "max_tokens": 100,
            "temperature": 0.2,
            "note": "sh600460"
        }))
        .to_request();
    let response: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(response["content"], "{\"summary\":\"测试成功\"}");
    assert_eq!(response["provider"], "openai");
    assert_eq!(response["tokens_in"], 12);
    assert_eq!(response["tokens_out"], 7);
    mock.assert();

    let usage_request = test::TestRequest::get()
        .uri("/api/v1/ai/usage?limit=10")
        .to_request();
    let usage: Value = test::call_and_read_body_json(&app, usage_request).await;
    assert_eq!(usage["total"], 1);
    assert_eq!(usage["success"], 1);
    assert_eq!(usage["failed"], 0);
    assert_eq!(usage["recent"][0]["provider"], "openai");
    assert_eq!(usage["recent"][0]["model"], "test-model");
    assert_eq!(usage["recent"][0]["success"], true);
    assert_eq!(usage["recent"][0]["note"], "sh600460");
}

#[actix_web::test]
async fn ollama_chat_uses_native_protocol() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(POST)
            .path("/api/chat")
            .json_body_partial(r#"{"model":"qwen2.5:1.5b","stream":false}"#);
        then.status(200).json_body(json!({
            "message": {"role": "assistant", "content": "本地模型响应"},
            "prompt_eval_count": 9,
            "eval_count": 4
        }));
    });
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let request = test::TestRequest::post()
        .uri("/api/v1/ai/chat")
        .set_json(json!({
            "provider": "ollama",
            "base_url": upstream.base_url(),
            "model": "qwen2.5:1.5b",
            "system": "system",
            "user": "user"
        }))
        .to_request();
    let response: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(response["content"], "本地模型响应");
    assert_eq!(response["provider"], "ollama");
    assert_eq!(response["tokens_in"], 9);
    assert_eq!(response["tokens_out"], 4);
    mock.assert();
}

#[actix_web::test]
async fn invalid_provider_returns_400_without_writing_usage() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let request = test::TestRequest::post()
        .uri("/api/v1/ai/chat")
        .set_json(json!({
            "provider": "unknown",
            "base_url": "http://127.0.0.1:11434",
            "model": "model",
            "system": "system",
            "user": "user"
        }))
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), 400);

    let usage_request = test::TestRequest::get()
        .uri("/api/v1/ai/usage")
        .to_request();
    let usage: Value = test::call_and_read_body_json(&app, usage_request).await;
    // resolve() 失败被识别为"未配置"（客户端请求格式问题），不走账本；
    // 上游调用阶段的失败才会被记录（见其他测试覆盖）。
    assert_eq!(usage["total"], 0);
}

#[actix_web::test]
async fn invalid_base_url_returns_400_without_writing_usage() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let request = test::TestRequest::post()
        .uri("/api/v1/ai/chat")
        .set_json(json!({
            "provider": "openai",
            "base_url": "not-a-url",
            "api_key": "k",
            "model": "m",
            "system": "s",
            "user": "u"
        }))
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), 400);

    let usage_request = test::TestRequest::get()
        .uri("/api/v1/ai/usage")
        .to_request();
    let usage: Value = test::call_and_read_body_json(&app, usage_request).await;
    // 非法请求体在 validate() 阶段就被拦下，不应当写入账本
    assert_eq!(usage["total"], 0);
}

#[actix_web::test]
async fn upstream_5xx_returns_502_and_records_failure() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(500).body("upstream boom");
    });
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ai::configure)),
    )
    .await;
    let request = test::TestRequest::post()
        .uri("/api/v1/ai/chat")
        .set_json(json!({
            "provider": "openai",
            "base_url": format!("{}/v1", upstream.base_url()),
            "api_key": "k",
            "model": "m",
            "system": "s",
            "user": "u"
        }))
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), 502);
    mock.assert();

    let usage_request = test::TestRequest::get()
        .uri("/api/v1/ai/usage")
        .to_request();
    let usage: Value = test::call_and_read_body_json(&app, usage_request).await;
    assert_eq!(usage["total"], 1);
    assert_eq!(usage["success"], 0);
    assert_eq!(usage["failed"], 1);
    assert_eq!(usage["recent"][0]["success"], false);
    let error = usage["recent"][0]["error"].as_str().unwrap_or_default();
    assert!(error.contains("500") || error.contains("boom"));
}
