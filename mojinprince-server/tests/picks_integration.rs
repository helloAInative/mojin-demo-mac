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

#[actix_web::test]
async fn negative_signals_split_into_specific_tags_and_meta() {
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
    // 实时校准上游不通 → fallback 到 snapshot（未涨停）
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(500);
    });
    // 新闻上游：1 条减持 + 1 条问询 → 期望两个细分 tag + negative_signals
    upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200).json_body(json!({
            "code": 0,
            "result": {"cmsArticleWebOld": [
                {
                    "date": "2026-09-22 10:30:00",
                    "title": "股东减持计划公告",
                    "content": "拟减持不超过 2%",
                    "mediaName": "证券时报",
                    "url": "https://news.example.com/r1"
                },
                {
                    "date": "2026-09-21 09:15:00",
                    "title": "公司收到问询函",
                    "content": "关注函",
                    "mediaName": "深交所",
                    "url": "https://news.example.com/r2"
                }
            ]}
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.us_index.base_url = upstream.base_url();
    state.news.base_url = upstream.base_url();

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
    let reasons = pick["reasons"].as_array().unwrap();
    let reason_text: Vec<&str> = reasons.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        reason_text.contains(&"股东减持"),
        "reasons 应包含「股东减持」细分 tag：{reasons:?}"
    );
    assert!(
        reason_text.contains(&"收到问询函"),
        "reasons 应包含「收到问询函」细分 tag：{reasons:?}"
    );
    // 不再使用笼统的「利空新闻」tag
    assert!(
        !reason_text.contains(&"利空新闻"),
        "「利空新闻」已被细分 tag 取代：{reasons:?}"
    );
    let neg = pick["meta"]["negative_signals"].as_array().unwrap();
    let cats: Vec<&str> = neg
        .iter()
        .filter_map(|v| v["category"].as_str())
        .collect();
    assert!(cats.contains(&"股东减持"));
    assert!(cats.contains(&"收到问询函"));
    // 每条都带原文标题
    for sig in neg {
        assert!(sig["title"].is_string(), "title 必须存在：{sig}");
        assert!(!sig["title"].as_str().unwrap().is_empty());
    }
}

#[actix_web::test]
async fn anomaly_tag_appears_when_pct_above_board_threshold() {
    let upstream = MockServer::start();
    // 主板 pct=7.2 → 触发「异常波动」阈值 7%
    upstream.mock(|when, then| {
        when.method(GET).path("/api/qt/clist/get");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "600519", "f14": "贵州茅台", "f2": 1500.5, "f3": 7.2, "f100": "白酒"}
            ]}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sh600519": {"qfqday": zigzag_bars()}}
        }));
    });
    upstream.mock(|when, then| {
        when.method(GET).path("/q=usDJI,usIXIC");
        then.status(200).body(
            "v_usDJI=\"200~DJI~.DJI~102.0~100.0~102.5~1\";\nv_usIXIC=\"200~IXIC~.IXIC~102.0~100.0~102.5~1\"",
        );
    });
    // 实时校准返回 pct=7.0（未封板，但仍在异常波动阈值 7% 之上）
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "1.600519");
        then.status(200).json_body(json!({}));
    });
    // 实时校准上游对 600519 也用 path 区分
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "1.600519");
        then.status(200).json_body(json!({"data": {"f2": 1620.0, "f3": 7.2}}));
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
    let reasons = pick["reasons"].as_array().unwrap();
    let text: Vec<&str> = reasons.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        text.contains(&"异常波动"),
        "pct=7.2 应触发「异常波动」：{reasons:?}"
    );
}

#[actix_web::test]
async fn list_picks_exposes_wilson_interval_and_low_sample_flag() {
    // 准备 10 条 daily_pick，命中 7 条（70% 胜率），样本不足触发 samples_sufficient=false
    let state = fresh_state().await;
    let db = state.db.clone();
    let pick_date = "2026-09-15";
    let next_open_better = |base_close: f64, t1_close: f64, t1_open: f64| {
        serde_json::json!({
            "close": base_close,
            "outcome": {
                "t1_pct": (t1_close - base_close) / base_close * 100.0,
                "t1_real": (t1_close - t1_open) / t1_open * 100.0,
                "entry_gap": (t1_open - base_close) / base_close * 100.0,
            }
        })
    };
    for i in 0..10i64 {
        let code = format!("sz30060{}", i + 1);
        let win = i < 7; // 前 7 条赢，后 3 条输
        let base = 10.0 + i as f64 * 0.05;
        let t1_close = if win { base * 1.012 } else { base * 0.985 };
        let t1_open = base * 1.001;
        let meta = next_open_better(base, t1_close, t1_open).to_string();
        sqlx::query(
            "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(pick_date)
        .bind(&code)
        .bind(format!("测试{}", i))
        .bind(i + 1)
        .bind(80.0)
        .bind("[\"放量\"]")
        .bind("")
        .bind(&meta)
        .bind(0)
        .execute(&db)
        .await
        .unwrap();
    }

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri(&format!("/api/v1/picks?date={pick_date}"))
            .to_request(),
    )
    .await;

    let stats = &doc["stats"];
    assert_eq!(stats["samples"].as_i64().unwrap(), 10);
    assert!(
        (stats["t1_win_rate"].as_f64().unwrap() - 0.7).abs() < 1e-9,
        "t1_win_rate 应 = 0.7"
    );
    // 关键断言：< 30 样本应标记为 insufficient，前端据此显示 ⚠️
    assert_eq!(stats["t1_samples_sufficient"], false);
    // Wilson 区间：10/7 实际约为 (0.398, 0.892)
    let low = stats["t1_win_rate_low"].as_f64().unwrap();
    let high = stats["t1_win_rate_high"].as_f64().unwrap();
    let margin = stats["t1_win_rate_margin"].as_f64().unwrap();
    assert!(low < 0.45, "下界应 < 0.45（避免误读 70%）：{low}");
    assert!(high > 0.85, "上界应 > 0.85：{high}");
    assert!(low < 0.5, "下界 < 0.5 明确告诉用户「70% 胜率无统计意义」：{low}");
    assert!((margin - (high - low) / 2.0).abs() < 1e-9, "margin 应等于区间半宽");
    // 真实执行：同样有区间
    let exec = &stats["execution"];
    assert_eq!(exec["t1_real_samples_sufficient"], false);
    let r_low = exec["t1_real_win_rate_low"].as_f64().unwrap();
    assert!(r_low > 0.0, "真实执行区间下界应 > 0");
}

#[actix_web::test]
async fn picks_run_attaches_buy_and_sell_price_zones() {
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
    // 实时校准上游不通 → fallback snapshot
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(500);
    });

    let mut state = fresh_state().await;
    let db = state.db.clone();
    // 给 sz300623 灌 5 条历史 outcome (t1_pct ≈ 1.5% 平均)，让 sell_zone 走历史路径
    for i in 0..5i64 {
        let date = format!("2026-09-{:02}", 1 + i);
        sqlx::query(
            "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(&date)
        .bind("sz300623")
        .bind("捷捷微电")
        .bind(1)
        .bind(80.0)
        .bind("[]")
        .bind("")
        .bind(json!({"outcome": {"t1_pct": 1.5}}).to_string())
        .bind(0)
        .execute(&db)
        .await
        .unwrap();
    }
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
    let plan = &pick["meta"]["plan"];
    // §A.9 买点：close=35.11 ±2% → [34.41, 35.81]
    let buy_low = plan["buy_price_low"].as_f64().unwrap();
    let buy_high = plan["buy_price_high"].as_f64().unwrap();
    assert!((buy_low - 34.41).abs() < 0.05, "buy_low={buy_low}");
    assert!((buy_high - 35.81).abs() < 0.05, "buy_high={buy_high}");
    assert!((plan["buy_basis_close"].as_f64().unwrap() - 35.11).abs() < 0.02);
    // §A.9 卖点：基于 5 条历史 t1_pct=1.5% (samples=5) + win_rate=1.0
    //   factor = (1.5/100)*1.0 = 0.015 → sell_mid = 35.11 * 1.015 = 35.6366
    //   sell_low = 35.6366 * 0.985 = 35.10；sell_high = 35.6366 * 1.015 = 36.17
    let sell_low = plan["sell_price_low"].as_f64().unwrap();
    let sell_high = plan["sell_price_high"].as_f64().unwrap();
    assert!(sell_low > 35.0 && sell_low < 35.20, "sell_low={sell_low}");
    assert!(sell_high > 36.0 && sell_high < 36.30, "sell_high={sell_high}");
    assert!(sell_high > sell_low, "卖区间必须 high > low");
    // meta.sell_zone_basis 透出供前端回溯
    let basis = &pick["meta"]["sell_zone_basis"];
    assert_eq!(basis["samples"].as_i64().unwrap(), 5);
    assert!((basis["win_rate"].as_f64().unwrap() - 1.0).abs() < 1e-9);
}

#[actix_web::test]
async fn picks_run_falls_back_to_default_sell_zone_when_no_history() {
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
    let plan = &pick["meta"]["plan"];
    // 无历史 → 默认 1.2% 期望收益 → sell_mid = 35.11 * 1.012 = 35.5313
    // sell_low ≈ 35.0、sell_high ≈ 36.06
    let sell_low = plan["sell_price_low"].as_f64().unwrap();
    let sell_high = plan["sell_price_high"].as_f64().unwrap();
    assert!(sell_low > 34.9 && sell_low < 35.2, "无历史 sell_low={sell_low}");
    assert!(sell_high > 35.9 && sell_high < 36.2, "无历史 sell_high={sell_high}");
    // samples = 0
    assert_eq!(pick["meta"]["sell_zone_basis"]["samples"].as_i64().unwrap(), 0);
}

#[actix_web::test]
async fn list_picks_returns_previous_picks_with_sell_zone() {
    // 今天：2026-09-22 → 拉最新生成日 2026-09-21 的 daily_pick 作为 previous_picks
    let state = fresh_state().await;
    let db = state.db.clone();
    let yesterday = "2026-09-21";
    for (i, code) in ["sz300603", "sz300604"].iter().enumerate() {
        sqlx::query(
            "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(yesterday)
        .bind(*code)
        .bind(format!("票{}", i + 1))
        .bind((i + 1) as i64)
        .bind(80.0)
        .bind("[\"放量\"]")
        .bind("")
        .bind(json!({
            "close": 10.0,
            "plan": {"entry_timing": "today_close", "entry_label": "今日尾盘"},
            "outcome": {"t1_pct": 2.0, "t1_close": 10.20, "t1_open_basis": 10.02, "t1_real": 1.7}
        }).to_string())
        .bind(0)
        .execute(&db)
        .await
        .unwrap();
    }

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/picks?date=2026-09-22")
            .to_request(),
    )
    .await;

    let prev = doc["previous_picks"].as_array().expect("必须有 previous_picks");
    assert_eq!(prev.len(), 2, "应返回 2 条昨日推荐");
    // 上一交易日推荐：entry_timing=today_close → basis=t1_close=10.20
    //   sell_low = 10.20 * 0.985 = 10.05, sell_high = 10.20 * 1.015 = 10.35
    let first = &prev[0];
    assert_eq!(first["code"], "sz300603");
    assert_eq!(first["entry_label"], "今日尾盘");
    assert_eq!(first["sell_basis_kind"], "t1_close");
    assert!((first["sell_basis_price"].as_f64().unwrap() - 10.20).abs() < 1e-9);
    assert!((first["sell_price_low"].as_f64().unwrap() - 10.05).abs() < 0.02);
    assert!((first["sell_price_high"].as_f64().unwrap() - 10.35).abs() < 0.02);
    // t1_pct / t1_real 也要透出
    assert!((first["t1_pct"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert!((first["t1_real_pct"].as_f64().unwrap() - 1.7).abs() < 1e-9);
}

#[actix_web::test]
async fn list_picks_previous_picks_uses_open_basis_for_next_session_open() {
    // 推荐时 entry_timing=next_session_open（封板票），昨日推荐应使用 t1_open_basis 作为
    // 卖出价基准（真实买入价），更能反映「昨天推荐的票今天能卖的价」
    let state = fresh_state().await;
    let db = state.db.clone();
    let yesterday = "2026-09-21";
    sqlx::query(
        "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
         VALUES(?,?,?,?,?,?,?,?,?)",
    )
    .bind(yesterday)
    .bind("sz300605")
    .bind("封板票")
    .bind(1)
    .bind(90.0)
    .bind("[\"涨停\"]")
    .bind("")
    .bind(json!({
        "close": 21.00, // T 日收盘/封板价
        "plan": {"entry_timing": "next_session_open", "entry_label": "次日开盘"},
        "outcome": {
            "t1_pct": 5.0,
            "t1_close": 22.05,
            "t1_open_basis": 21.50, // T+1 开盘价（次日开盘买入的真实价）
            "t1_real": 2.3
        }
    }).to_string())
    .bind(0)
    .execute(&db)
    .await
    .unwrap();

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::pick::configure)),
    )
    .await;

    let doc: Value = test::call_and_read_body_json(
        &app,
        test::TestRequest::get()
            .uri("/api/v1/picks?date=2026-09-22")
            .to_request(),
    )
    .await;
    let prev = &doc["previous_picks"][0];
    // basis = t1_open_basis * 1.005 = 21.50 * 1.005 = 21.6075
    assert_eq!(prev["sell_basis_kind"], "actual_t1_open");
    assert!((prev["sell_basis_price"].as_f64().unwrap() - 21.6075).abs() < 0.01);
    let s_low = prev["sell_price_low"].as_f64().unwrap();
    let s_high = prev["sell_price_high"].as_f64().unwrap();
    assert!(s_low >= 21.27 && s_low <= 21.29, "sell_low={s_low}");
    assert!(s_high > 21.92 && s_high < 21.94, "sell_high={s_high}");
}
