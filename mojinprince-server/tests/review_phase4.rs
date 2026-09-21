//! 阶段 4：收盘复盘 / 周报 集成测试。
//!
//! 覆盖：
//! - `POST /api/v1/reviews/run?kind=daily` 落库 + 幂等（同日重复执行不生成新记录）
//! - `POST /api/v1/reviews/run?kind=weekly` 周报独立于日报
//! - 可选 JSON body 把 Swift 端"日记 + 委托 + 信号"写入 `payload.context`
//! - `GET /api/v1/reviews?kind=daily` 按时间倒序列出
//! - 非法 `kind` 返回 400
//! - 报告 body 包含后端信号 / AI 用量摘要
use actix_web::{test, web, App};
use chrono::{DateTime, Utc};
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
    // 把 thread id 也加入文件名，避免 cargo test 多线程并行时撞同一 db
    // （否则两个测试共用 sqlite 路径会触发 _sqlx_migrations UNIQUE 约束）。
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
        "mojinprince-review-phase4-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

#[actix_web::test]
async fn run_daily_inserts_then_idempotent() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;

    // 第一次生成
    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .set_json(json!({
                "diary": "今日加仓 sh600460；午后回落 1.2%",
                "tickets": [{
                    "code": "sh600460",
                    "at": "2026-09-20T01:30:00Z",
                    "summary": "买入 1000 @ 32.50",
                    "status": "copied"
                }],
                "focusCodes": ["sh600460"]
            }))
            .to_request(),
    )
    .await;
    assert_eq!(resp["kind"], "daily");
    let period_key = resp["periodKey"].as_str().unwrap().to_string();
    let first_created = resp["createdAt"].as_str().unwrap().to_string();
    let first_id = resp["id"].as_str().unwrap().to_string();
    assert!(resp["body"].as_str().unwrap().contains("后端摘要"));
    assert!(resp["body"].as_str().unwrap().contains("sh600460"));
    assert!(resp["payload"]["context"]["diary"]
        .as_str()
        .unwrap()
        .contains("加仓"));

    // 立即再调一次（同 period_key）—— 应当幂等，不增加行数
    let resp2: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .set_json(json!({
                "diary": "覆盖后的日记"
            }))
            .to_request(),
    )
    .await;
    assert_eq!(resp2["periodKey"], period_key, "periodKey 不应变化");
    assert_eq!(resp2["id"], first_id, "id 应稳定");
    assert_eq!(
        resp2["createdAt"], first_created,
        "createdAt 必须是首次生成时间，不应被刷新"
    );
    assert!(resp2["payload"]["context"]["diary"]
        .as_str()
        .unwrap()
        .contains("覆盖"));

    // 数据库只有一行
    let list: Vec<Value> = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/reviews?kind=daily")
            .to_request(),
    )
    .await;
    assert_eq!(list.len(), 1, "daily 应当只有一条记录（幂等）");
}

#[actix_web::test]
async fn weekly_report_is_independent_of_daily() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;

    let daily: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .to_request(),
    )
    .await;
    let weekly: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=weekly")
            .to_request(),
    )
    .await;
    assert_eq!(daily["kind"], "daily");
    assert_eq!(weekly["kind"], "weekly");
    assert_ne!(daily["periodKey"], weekly["periodKey"]);
    assert_ne!(daily["id"], weekly["id"]);

    // 列表应能看到两条（一个 daily、一个 weekly）
    let list: Vec<Value> = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/api/v1/reviews").to_request(),
    )
    .await;
    assert_eq!(list.len(), 2);
}

#[actix_web::test]
async fn run_without_body_uses_default_context() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;
    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .to_request(),
    )
    .await;
    assert_eq!(resp["kind"], "daily");
    assert_eq!(resp["payload"]["context"]["diary"], Value::Null);
    assert_eq!(resp["payload"]["context"]["tickets"], json!([]));
    assert_eq!(resp["payload"]["context"]["signals"], json!([]));
}

#[actix_web::test]
async fn run_with_invalid_kind_returns_400() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;
    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=monthly")
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), 400);
}

#[actix_web::test]
async fn report_body_includes_ai_usage_and_signals() {
    let state = fresh_state().await;
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;

    // 注入 2 条 ai_usage + 1 条 signal_event，让 body 包含摘要
    let now = Utc::now().timestamp_millis();
    for i in 0..2 {
        sqlx::query(
            "INSERT INTO ai_usage(at,provider,model,tokens_in,tokens_out,cost,duration_ms,success,fallback,error,note,prompt_chars,output_chars) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(now - 1000 + i * 100)
        .bind("openai")
        .bind("test-model")
        .bind(120)
        .bind(45)
        .bind(0.001)
        .bind(800)
        .bind(true)
        .bind(false)
        .bind(None::<String>)
        .bind(Some("note"))
        .bind(0)
        .bind(0)
        .execute(&db)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO signal_event(id,at,kind,code,title,body,price,source,evidence,why,meta) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind("sig-1")
    .bind(now - 500)
    .bind("level")
    .bind("sh600460")
    .bind("支撑位 32.30")
    .bind("")
    .bind(32.5)
    .bind("sina")
    .bind("")
    .bind("")
    .bind("{\"hit\":\"1\",\"leadSec\":60}")
    .execute(&db)
    .await
    .unwrap();

    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .to_request(),
    )
    .await;
    let body = resp["body"].as_str().unwrap();
    assert!(body.contains("AI 调用：2 / 2"), "body 应显示 AI 成功/总数，body={body}");
    assert!(body.contains("tokens_in=240"), "body 应累计 tokens_in，实际：{body}");
    assert!(body.contains("支撑位 32.30"), "body 应列出后端 signal_event 标题");
    assert!(body.contains("level 命中：1 / 1"), "level 命中率应展示");
    // payload 是手写 json!() 生成的，key 保持 snake_case
    let summary = &resp["payload"]["summary"];
    assert_eq!(summary["signals_total"], 1);
    assert_eq!(summary["level_hit"], 1);
    assert_eq!(summary["level_total"], 1);
    assert_eq!(summary["ai_calls_total"], 2);
    assert_eq!(summary["ai_calls_success"], 2);
}

#[actix_web::test]
async fn list_filters_by_kind_and_respects_limit() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;
    for kind in ["daily", "weekly"] {
        let _: Value = test::call_and_read_body_json(
            &app,
            test::TestRequest::post()
                .uri(&format!("/api/v1/reviews/run?kind={kind}"))
                .to_request(),
        )
        .await;
    }

    let only_daily: Vec<Value> = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/reviews?kind=daily&limit=10")
            .to_request(),
    )
    .await;
    assert_eq!(only_daily.len(), 1);
    assert_eq!(only_daily[0]["kind"], "daily");

    let capped: Vec<Value> = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/reviews?limit=1")
            .to_request(),
    )
    .await;
    assert_eq!(capped.len(), 1, "limit=1 应只返回 1 条");
}

#[actix_web::test]
async fn created_at_is_iso8601_string() {
    let state = fresh_state().await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;
    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .to_request(),
    )
    .await;
    let s = resp["createdAt"].as_str();
    assert!(
        s.is_some(),
        "createdAt 必须是字符串而非 {:?}",
        resp["createdAt"]
    );
    let s = s.unwrap();
    assert!(
        DateTime::parse_from_rfc3339(s).is_ok(),
        "createdAt 必须是 ISO-8601: {s}"
    );
}

#[actix_web::test]
async fn report_compares_filled_sells_against_stop_take() {
    let state = fresh_state().await;
    // 持仓 sh600460 设了止损 32.0 / 止盈 35.0；sz000001 无止损止盈
    sqlx::query(
        "INSERT INTO position(code, stop_loss, take_profit, updated_at) VALUES('sh600460', 32.0, 35.0, 0)",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::review::configure)),
    )
    .await;

    let resp: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/reviews/run?kind=daily")
            .set_json(json!({
                "tickets": [
                    {
                        "code": "sh600460",
                        "at": "2026-09-21T07:00:00Z",
                        "summary": "卖出 600460 1000股 @31.80",
                        "status": "filled",
                        "side": "sell",
                        "price": 31.8
                    },
                    {
                        "code": "sz000001",
                        "at": "2026-09-21T07:30:00Z",
                        "summary": "卖出 000001 500股 @10.00",
                        "status": "filled",
                        "side": "sell",
                        "price": 10.0
                    },
                    {
                        "code": "sh600460",
                        "at": "2026-09-21T05:00:00Z",
                        "summary": "买入 600460 500股 @31.50",
                        "status": "filled",
                        "side": "buy",
                        "price": 31.5
                    },
                    {
                        "code": "sh600460",
                        "at": "2026-09-21T06:00:00Z",
                        "summary": "未勾成交的草稿",
                        "status": "draft",
                        "side": "sell",
                        "price": 32.0
                    }
                ]
            }))
            .to_request(),
    )
    .await;
    let body = resp["body"].as_str().unwrap();
    assert!(body.contains("止损止盈执行对照"), "缺少对照小节：\n{body}");
    assert!(
        body.contains("卖出成交 31.80 vs 止损 32.00（-0.20，-0.6%）；止盈 35.00（-3.20，-9.1%）"),
        "sh600460 偏差行：\n{body}"
    );
    assert!(body.contains("sz000001 卖出成交 10.00 · 未设止损/止盈"), "无价位持仓提示：\n{body}");
    // 只有两条已成交卖出参与对照；买入与草稿不产生对照行
    let section = body.split("止损止盈执行对照").nth(1).unwrap_or("");
    assert_eq!(
        section.matches("卖出成交").count(),
        2,
        "对照行数应为 2（买入 / 草稿不参与）：\n{body}"
    );
}
