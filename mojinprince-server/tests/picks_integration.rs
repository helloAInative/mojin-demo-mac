//! 智能推荐集成测试：四层漏斗全链路（mock 上游）。
//!
//! 覆盖：
//! 1. `POST /api/v1/picks/run`（带 AI 配置）：涨幅榜候选 → 日 K 量化粗筛
//!    → 落库 daily_pick（DELETE+INSERT 刷新），AI 顺序优先
//! 2. `GET /api/v1/picks`：默认取最近一天，附回测统计（无样本为 0）
//! 3. 不带 AI 重跑：降级为纯量化序
//! 4. `backfill_outcomes`：T+1/T+5 回写 + 命中率统计
use actix_web::{test, web, App};
use httpmock::prelude::*;
use mojinprince_server::{api, service, state::AppState, Config};
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
        "mojinprince-picks-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

/// 40 根健康上行日 K（隔日 +0.21/−0.14 → RSI≈60、尾部放量）。
fn zigzag_bars() -> Vec<Value> {
    let mut bars = Vec::new();
    for i in 0..40 {
        let close = 10.0 + 0.035 * i as f64 + if i % 2 == 0 { 0.105 } else { -0.070 };
        let date = format!("2026-{:02}-{:02}", 8 + i / 28, i % 28 + 1);
        let volume = if i >= 38 { 300 } else { 100 };
        bars.push(json!([
            date,
            (close - 0.05) as f64,
            close,
            close + 0.15,
            close - 0.15,
            volume
        ]));
    }
    bars
}

#[actix_web::test]
async fn picks_run_with_ai_ranks_and_persists() {
    let upstream = MockServer::start();
    // 涨幅榜：两只候选
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/clist/get")
            .query_param("pz", "100");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "600519", "f14": "贵州茅台", "f2": 1500.5, "f3": 3.2, "f100": "白酒"},
                {"f12": "300623", "f14": "捷捷微电", "f2": 35.11, "f3": 2.8, "f100": "半导体"}
            ]}
        }));
    });
    // 日 K：两只都返回同一份健康序列（param 区分不了就都命中）
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {
                "sh600519": {"qfqday": zigzag_bars()},
                "sz300623": {"qfqday": zigzag_bars()}
            }
        }));
    });
    // 隔夜美股 mock：两指数 +2%（偏多情绪 +10）
    upstream.mock(|when, then| {
        when.method(GET).path("/q=usDJI,usIXIC");
        then.status(200).body(
            "v_usDJI=\"200~DJI~.DJI~102.0~100.0~102.5~1\";\nv_usIXIC=\"200~IXIC~.IXIC~102.0~100.0~102.5~1\"",
        );
    });
    // 新闻 mock：一条利好（中标 → +5「利好新闻」）
    upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200).json_body(json!({
            "code": 0,
            "result": {"cmsArticleWebOld": [{
                "date": "2026-09-22 10:30:00",
                "title": "公司中标3.2亿元大订单",
                "content": "利好落地",
                "mediaName": "证券时报",
                "url": "https://news.example.com/1"
            }]}
        }));
    });
    // AI 精排 mock：只推 300623
    upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(json!({
            "choices": [{"message": {"role": "assistant",
                "content": "{\"picks\":[{\"code\":\"sz300623\",\"note\":\"量价配合\"}]}"}}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.us_index.base_url = upstream.base_url();
    state.news.base_url = upstream.base_url();
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let request = test::TestRequest::post()
        .uri("/api/v1/picks/run")
        .set_json(json!({
            "ai": {
                "provider": "openai",
                "base_url": format!("{}/v1", upstream.base_url()),
                "api_key": "test-key",
                "model": "qwen3.7-plus"
            }
        }))
        .to_request();
    let doc: Value = test::call_and_read_body_json(&app, request).await;
    let picks = doc["picks"].as_array().unwrap();
    assert_eq!(picks.len(), 2, "2 候选都入选（AI 1 只 + 量化补齐）：{doc}");
    assert_eq!(picks[0]["code"], "sz300623", "AI 顺序优先：{doc}");
    assert_eq!(picks[0]["rank"], 1);
    assert_eq!(picks[1]["code"], "sh600519");
    assert_eq!(picks[0]["meta"]["plan"]["target"], "T+1");
    assert_eq!(picks[0]["meta"]["plan"]["strategy"], "短线");
    assert_eq!(
        picks[0]["meta"]["plan"]["objective"],
        "next_day_positive_close"
    );
    assert!(
        picks[0]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "放量"),
        "量化标签保留：{doc}"
    );
    // v2 因子：强势行业（两行业均值 ≥2%）+ 隔夜美股偏多 + 利好新闻
    let reasons = picks[0]["reasons"].as_array().unwrap();
    assert!(
        reasons.iter().any(|t| t == "强势行业"),
        "行业动量标签：{doc}"
    );
    assert!(
        reasons.iter().any(|t| t == "隔夜美股偏多"),
        "美股情绪标签：{doc}"
    );
    assert!(
        reasons.iter().any(|t| t == "利好新闻"),
        "新闻关键词标签：{doc}"
    );
    assert_eq!(
        picks[0]["meta"]["us"]["djia"], 2.0,
        "meta 记录隔夜美股：{doc}"
    );
    assert_eq!(doc["market"]["djia"], 2.0, "文档级 market：{doc}");
    assert!(!doc["date"].as_str().unwrap().is_empty());

    // GET：默认最近一天，同一份
    let listed: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get().uri("/api/v1/picks").to_request(),
    )
    .await;
    assert_eq!(listed["date"], doc["date"]);
    assert_eq!(listed["picks"].as_array().unwrap().len(), 2);
    assert_eq!(listed["stats"]["samples"], 0, "无 outcome 样本");

    // 日 K 已顺带落库（回测 / 复盘复用）
    let day_bars: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM day_bar")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(day_bars > 0, "day_bar 应有落库：{day_bars}");
}

#[actix_web::test]
async fn picks_run_without_ai_uses_quant_order() {
    let upstream = MockServer::start();
    upstream.mock(|when, then| {
        when.method(GET).path("/api/qt/clist/get");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "300623", "f14": "捷捷微电", "f2": 35.11, "f3": 2.8, "f100": "半导体"}
            ]}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sz300623": {"qfqday": zigzag_bars()}}
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.us_index.base_url = upstream.base_url();
    state.news.base_url = upstream.base_url();
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/picks/run")
            .to_request(),
    )
    .await;
    assert_eq!(doc["picks"].as_array().unwrap().len(), 1);

    // 幂等刷新：同日重跑仍是 1 行（DELETE+INSERT）
    let doc2: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/picks/run")
            .to_request(),
    )
    .await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM daily_pick")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(count, 1, "重复生成刷新不叠加：{doc2}");
}

#[actix_web::test]
async fn backfill_writes_outcome_and_stats() {
    let upstream = MockServer::start();
    // 推荐日 = 第 31 根（回看窗内），之后有 5 根且 T+5 为正收益
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sz300623": {"qfqday": zigzag_bars()}}
        }));
    });
    let bars = zigzag_bars();
    let pick_date = bars[31][0].as_str().unwrap().to_string();
    let base_close = bars[31][2].as_f64().unwrap();
    let t1_close = bars[32][2].as_f64().unwrap();
    let t5_close = bars[36][2].as_f64().unwrap();

    let mut state = fresh_state().await;
    state.day_k.base_url = upstream.base_url();
    sqlx::query(
        "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
         VALUES(?,?,?,?,?,?,?,?,?)",
    )
    .bind(&pick_date)
    .bind("sz300623")
    .bind("捷捷微电")
    .bind(1)
    .bind(80.0)
    .bind("[]")
    .bind("")
    .bind(json!({"close": base_close}).to_string())
    .bind(0)
    .execute(&state.db)
    .await
    .unwrap();

    let updated = service::pick::backfill_outcomes(&state).await.unwrap();
    assert_eq!(updated, 1);
    let meta: String =
        sqlx::query_scalar("SELECT meta FROM daily_pick WHERE date=? AND code='sz300623'")
            .bind(&pick_date)
            .fetch_one(&state.db)
            .await
            .unwrap();
    let meta: Value = serde_json::from_str(&meta).unwrap();
    let t1 = (t1_close - base_close) / base_close * 100.0;
    let t5 = (t5_close - base_close) / base_close * 100.0;
    assert!((meta["outcome"]["t1_pct"].as_f64().unwrap() - t1).abs() < 1e-9);
    assert!((meta["outcome"]["t5_pct"].as_f64().unwrap() - t5).abs() < 1e-9);

    // 统计：主口径 T+1，同时保留 T+5 中线参考。
    let doc = service::pick::list_picks(&state.db, Some(&pick_date))
        .await
        .unwrap();
    assert_eq!(doc.stats.samples, 1);
    assert!(doc.stats.t1_win_rate > 0.99);
    assert!((doc.stats.avg_t1_pct - t1).abs() < 1e-9);
    assert_eq!(doc.stats.t5_samples, 1);
    assert!(doc.stats.t5_win_rate > 0.99);
    assert!((doc.stats.avg_t5_pct - t5).abs() < 1e-9);
}

#[actix_web::test]
async fn realtime_limit_up_recalibrates_plan_to_next_session_open() {
    let upstream = MockServer::start();
    // 候选池：一只候选 snapshot pct=2.8（未涨停）
    upstream.mock(|when, then| {
        when.method(GET).path("/api/qt/clist/get");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "300623", "f14": "捷捷微电", "f2": 35.11, "f3": 2.8, "f100": "半导体"}
            ]}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sz300623": {"qfqday": zigzag_bars()}}
        }));
    });
    // 隔夜美股 mock
    upstream.mock(|when, then| {
        when.method(GET).path("/q=usDJI,usIXIC");
        then.status(200).body(
            "v_usDJI=\"200~DJI~.DJI~102.0~100.0~102.5~1\";\nv_usIXIC=\"200~IXIC~.IXIC~102.0~100.0~102.5~1\"",
        );
    });
    // §A.5.1 实时校准 mock：f3=20.0（创业板已封板）→ 实时涨停
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(200).json_body(json!({
            "data": {"f2": 42.13, "f3": 20.0}
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.us_index.base_url = upstream.base_url();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/picks/run")
            .to_request(),
    )
    .await;
    let pick = &doc["picks"][0];
    // 实时校准命中 → plan 强制次日开盘
    assert_eq!(pick["meta"]["limit_up_realtime"], true);
    assert_eq!(pick["meta"]["limit_up_calibrated"], true);
    assert_eq!(pick["meta"]["plan"]["entry_label"], "次日开盘");
    assert_eq!(pick["meta"]["plan"]["entry_timing"], "next_session_open");
    assert_eq!(pick["meta"]["plan"]["entry_window"], "09:30-09:35");
    // snapshot 仍是未涨停（meta.is_limit_up 保留）
    assert_eq!(pick["meta"]["is_limit_up"], false);
}

#[actix_web::test]
async fn realtime_limit_up_recalibrate_falls_back_to_snapshot_on_network_error() {
    let upstream = MockServer::start();
    upstream.mock(|when, then| {
        when.method(GET).path("/api/qt/clist/get");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "300623", "f14": "捷捷微电", "f2": 35.11, "f3": 2.8, "f100": "半导体"}
            ]}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sz300623": {"qfqday": zigzag_bars()}}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/q=usDJI,usIXIC");
        then.status(200).body(
            "v_usDJI=\"200~DJI~.DJI~102.0~100.0~102.5~1\";\nv_usIXIC=\"200~IXIC~.IXIC~102.0~100.0~102.5~1\"",
        );
    });
    // 实时校准接口返回 500 → fetch 返回 None → 不改 plan
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(500);
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.us_index.base_url = upstream.base_url();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::post()
            .uri("/api/v1/picks/run")
            .to_request(),
    )
    .await;
    let pick = &doc["picks"][0];
    assert_eq!(
        pick["meta"]["limit_up_calibrated"], false,
        "上游 500 时不应标 calibrated"
    );
    assert_eq!(pick["meta"]["limit_up_realtime"], false);
    // snapshot 是未涨停，所以 plan 走 14:45 窗口
    let entry_timing = pick["meta"]["plan"]["entry_timing"].as_str().unwrap();
    assert!(
        entry_timing == "today_close" || entry_timing == "next_session_pullback",
        "网络失败应保留 snapshot 决策：{entry_timing}"
    );
}
