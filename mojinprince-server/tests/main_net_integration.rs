//! §A.6 主力资金净流入打分端到端集成测试。
//!
//! 覆盖：
//! 1. `EastMoneyMainNet.fetch` 解析 push2 单股字段（f62/f63/f64/f170/f168/f60）
//! 2. `persist_main_net` UPSERT 落 main_net_snapshot
//! 3. `generate_picks` 把 main_net 注入打分（≥1 亿 +25 / 占比 ≥10% +5 = 封顶 +30）
//!    并打「主力抢筹」标签，meta.main_net 透出
//! 4. 同一票 vs 「主力流出 -1.5 亿」应得「主力出逃」标签且分数被扣

use actix_web::{test, web, App};
use chrono::Utc;
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
        "mojinprince-mainnet-{}-{safe_thread}-{nonce}.db",
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
async fn main_net_strong_inflow_boosts_score_and_tag() {
    let upstream = MockServer::start();
    // 涨幅榜：1 只候选
    upstream.mock(|when, then| {
        when.method(GET).path("/api/qt/clist/get");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f12": "300623", "f14": "捷捷微电", "f2": 35.11, "f3": 2.8, "f100": "半导体"}
            ]}
        }));
    });
    // 日 K：健康上行序列
    upstream.mock(|when, then| {
        when.method(GET).path("/appstock/app/fqkline/get");
        then.status(200).json_body(json!({
            "data": {"sz300623": {"qfqday": zigzag_bars()}}
        }));
    });
    // §A.6 主力净流入 mock：f62=1.5亿（>1亿 +25）、f170=12%（≥10% +5）→ 封顶 +30
    // 单位：f62 是「元」，1.5 亿 = 150_000_000
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(200).json_body(json!({
            "data": {
                "f43": 35.5,
                "f60": 35.0,
                "f62": 150_000_000.0,
                "f63": 100_000_000.0,
                "f64": 50_000_000.0,
                "f168": 5.2,
                "f170": 12.0
            }
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.main_net.base_url = upstream.base_url();

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
    let pick = &doc["picks"][0];
    assert_eq!(pick["code"], "sz300623");

    // 标签包含「主力抢筹」（>1亿）
    let reasons = pick["reasons"].as_array().unwrap();
    assert!(
        reasons.iter().any(|t| t == "主力抢筹"),
        "主力抢筹标签应被加上：{reasons:?}"
    );

    // meta.main_net 字段透出（万元：150_000_000 / 10000 = 15000.0）
    let mn = &pick["meta"]["main_net"];
    assert!(!mn.is_null(), "meta.main_net 不应为 None：{doc}");
    assert!(
        (mn["main_net_wan"].as_f64().unwrap() - 15_000.0).abs() < 1e-6,
        "main_net_wan 转万元：{mn}"
    );
    assert!((mn["pct_ratio"].as_f64().unwrap() - 12.0).abs() < 1e-9);
    assert!((mn["super_net_wan"].as_f64().unwrap() - 10_000.0).abs() < 1e-6);
    assert!((mn["big_net_wan"].as_f64().unwrap() - 5_000.0).abs() < 1e-6);
    assert!((mn["turnover"].as_f64().unwrap() - 5.2).abs() < 1e-9);
    // §A.6 delta 透出：流入 +30（封顶）
    assert!(
        (mn["score_delta"].as_f64().unwrap() - 30.0).abs() < 1e-6,
        "流入 delta 应等于 +30：{mn}"
    );

    // 落库：main_net_snapshot 应有 (sz300623, 今日) 一行
    let today_cn = Utc::now()
        .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).unwrap())
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    let rows: Vec<(f64, f64)> = sqlx::query_as(
        "SELECT main_net, pct_ratio FROM main_net_snapshot WHERE code='sz300623' AND trade_date=?",
    )
    .bind(&today_cn)
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "main_net_snapshot 应有 1 行：{rows:?}");
    assert!((rows[0].0 - 15_000.0).abs() < 1e-6);
    assert!((rows[0].1 - 12.0).abs() < 1e-9);
}

#[actix_web::test]
async fn main_net_strong_outflow_drops_score() {
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
    // 主力净流出 -1.5 亿 / 占比 -15% → 封顶 -30，「主力出逃」
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "0.300623");
        then.status(200).json_body(json!({
            "data": {
                "f62": -150_000_000.0,
                "f63": -90_000_000.0,
                "f64": -60_000_000.0,
                "f170": -15.0
            }
        }));
    });

    let mut state = fresh_state().await;
    state.pick_ranking.base_url = upstream.base_url();
    state.day_k.base_url = upstream.base_url();
    state.main_net.base_url = upstream.base_url();

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
    assert!(
        reasons.iter().any(|t| t == "主力出逃"),
        "主力出逃标签：{reasons:?}"
    );
    assert!(
        pick["meta"]["main_net"]["main_net_wan"]
            .as_f64()
            .unwrap()
            < -10_000.0,
        "main_net 透出流出绝对值 > 1.0 亿"
    );
    // 关键不在 score 绝对值（依赖 base 分 + 是否拉到新闻加分），而在 main_net delta：
// 流出 -1.5 亿 / 占比 -15% → main_net_score 返回 -30，且 meta.main_net.score_delta == -30
    let mn_delta = pick["meta"]["main_net"]["score_delta"]
        .as_f64()
        .expect("meta.main_net.score_delta 应存在");
    assert!(
        (mn_delta - (-30.0)).abs() < 1e-6,
        "流出 delta 应等于 -30：{mn_delta}"
    );
    // score 必须小于 base（base ≥ 100 + 量价 + 行业 + ...），流出 -30 后仍在合格线上方；
    // 不要求具体值，只要求不被 main_net 这一路反向加分。
    let score_with_outflow = pick["score"].as_f64().unwrap();
    assert!(
        score_with_outflow < 100.0,
        "流出 -30 后 score 应 < 100：{score_with_outflow}"
    );
}

#[actix_web::test]
async fn main_net_empty_when_upstream_returns_no_fields() {
    // 字段全 0 时 fetch 应返回 None（早盘 / 停牌 / 接口升级容错）
    let snapshot = mojinprince_server::service::ingest::MainNetSnapshot {
        code: "sh600519".into(),
        trade_date: "2026-09-23".into(),
        main_net_wan: 0.0,
        super_net_wan: 0.0,
        big_net_wan: 0.0,
        pct_ratio: 0.0,
        turnover: 0.0,
        prev_close: 0.0,
    };
    // 复用 fresh_state()：临时 db + 已迁移 schema
    let state = fresh_state().await;
    let db = state.db.clone();
    // 全 0 也允许落库（schema 容错），但 main_net_score 应返回 0 分无标签
    service::ingest::persist_main_net(&db, std::slice::from_ref(&snapshot))
        .await
        .unwrap();
    let (score, tag) = mojinprince_server::service::pick::main_net_score(0.0, 0.0);
    assert_eq!(score, 0.0);
    assert_eq!(tag, None);
    // fetch 路径：mock 让 data=null，验证容错返回 Ok(None)
    let upstream = MockServer::start();
    upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/stock/get")
            .query_param("secid", "1.600519");
        then.status(200).json_body(json!({ "data": null }));
    });
    let mut state2 = fresh_state().await;
    state2.main_net.base_url = upstream.base_url();
    let fetched = state2
        .main_net
        .fetch(&state2.http, "sh600519")
        .await
        .unwrap();
    assert!(fetched.is_none(), "data=null 时 fetch 应返回 Ok(None)");
}