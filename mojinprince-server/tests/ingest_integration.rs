//! §F.3–F.4 数据接入集成测试
//!
//! 覆盖：
//! 1. `GET /api/v1/news/{code}` 解析东财全文搜索并落库，`hours` 过滤旧闻。
//! 2. `GET /api/v1/reports/{code}` 解析研报（机构 / 评级 / 目标价 / 详情页 URL）。
//! 3. `GET /api/v1/sector/{code}` 合并成分列表与批量行情。
//! 4. 非法代码 → 400 `bad_request`；上游 5xx → 502 `upstream_exhausted`。
//! 5. `POST /api/v1/ai/analyze` 带 `include_news=true` 时把近 24h 新闻拼进 prompt。
//! 6. 每日刷新研报后评级信号化：近 7 天明确评级 → kind=report 信号，幂等不重复。
//!
//! 另有 `#[ignore]` 的 live 测试直连东财，需手动 `cargo test -- --ignored` 执行。
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
    // 文件名带 pid + thread id + nanos：并行测试各自独立 SQLite，避免迁移表撞唯一约束。
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
        "mojinprince-ingest-test-{}-{safe_thread}-{nonce}.db",
        std::process::id()
    ));
    let state = AppState::new(config(format!("sqlite://{}", path.display())))
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&state.db).await.unwrap();
    state
}

/// 北京时间 → 用于构造"刚刚发布"的新闻时间戳。
fn cn_time(days_ago: i64, hour: u32, minute: u32) -> String {
    let offset = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let day =
        chrono::Utc::now().with_timezone(&offset).date_naive() - chrono::Duration::days(days_ago);
    format!("{} {:02}:{:02}:00", day.format("%Y-%m-%d"), hour, minute)
}

#[actix_web::test]
async fn news_endpoint_parses_and_filters_by_hours() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200).json_body(json!({
            "code": 0,
            "result": {"cmsArticleWebOld": [
                {
                    "date": cn_time(0, 9, 30),
                    "title": "士兰微发布半年报",
                    "content": "公司上半年营收同比增长…",
                    "mediaName": "证券时报",
                    "url": "https://finance.eastmoney.com/a/202609201.html"
                },
                {
                    "date": cn_time(10, 9, 30),
                    "title": "十年前的旧闻",
                    "content": "旧闻摘要",
                    "mediaName": "旧媒体",
                    "url": "https://finance.eastmoney.com/a/201609201.html"
                },
                {
                    "date": cn_time(0, 10, 0),
                    "title": "缺少链接的条目",
                    "content": "应被丢弃",
                    "mediaName": "无链接",
                    "url": ""
                }
            ]}
        }));
    });
    let mut state = fresh_state().await;
    state.news.base_url = upstream.base_url();
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/news/sh600460?limit=10")
        .to_request();
    let body: Value = test::call_and_read_body_json(&app, request).await;
    let rows = body.as_array().expect("news response is an array");
    // 旧闻被 hours=72 默认窗口过滤，无链接条目被丢弃，只剩 1 条。
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["code"], "sh600460");
    assert_eq!(rows[0]["title"], "士兰微发布半年报");
    assert_eq!(rows[0]["media"], "证券时报");
    assert_eq!(
        rows[0]["url"],
        "https://finance.eastmoney.com/a/202609201.html"
    );
    mock.assert();

    // 落库的是上游返回的全量（缓存），时间窗只作用于响应体。
    let persisted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM news_item WHERE code='sh600460'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(persisted, 2);

    // 再拉一次：同 (code,url) 走 INSERT OR REPLACE，行数不变。
    let request = test::TestRequest::get()
        .uri("/api/v1/news/sh600460?limit=10")
        .to_request();
    let _: Value = test::call_and_read_body_json(&app, request).await;
    let persisted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM news_item WHERE code='sh600460'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(persisted, 2, "重复拉取不应产生新行");
}

#[actix_web::test]
async fn news_hours_param_can_widen_window() {
    let upstream = MockServer::start();
    upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200).json_body(json!({
            "code": 0,
            "result": {"cmsArticleWebOld": [{
                "date": cn_time(10, 9, 30),
                "title": "十年前的旧闻",
                "content": "旧闻摘要",
                "mediaName": "旧媒体",
                "url": "https://finance.eastmoney.com/a/201609201.html"
            }]}
        }));
    });
    let mut state = fresh_state().await;
    state.news.base_url = upstream.base_url();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/news/sh600460?hours=720")
        .to_request();
    let body: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(body.as_array().unwrap().len(), 1);
}

#[actix_web::test]
async fn reports_endpoint_parses_fields_and_persists() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(GET)
            .path("/report/list")
            .query_param("code", "600460")
            .query_param("qType", "0");
        then.status(200).json_body(json!({
            "data": [{
                "title": "士兰微：功率半导体景气上行",
                "orgSName": "中信证券",
                "publishDate": "2026-09-18 00:00:00",
                "infoCode": "AP202609181234",
                "indvInduName": "半导体",
                "emRatingName": "买入",
                "lastEmRatingName": "增持",
                "ratingChange": 1,
                "researcher": "张三",
                "indvAimPriceT": "45.00",
                "indvAimPriceL": "32.00"
            }, {
                "title": "缺少 infoCode 的条目",
                "orgSName": "某机构",
                "publishDate": "2026-09-17 00:00:00"
            }, {
                "title": "评级下调的条目",
                "orgSName": "群益证券",
                "publishDate": "2026-06-18 00:00:00",
                "infoCode": "AP202606181823",
                "emRatingName": "增持",
                "lastEmRatingName": "买入",
                "ratingChange": "2"
            }]
        }));
    });
    let mut state = fresh_state().await;
    state.reports.base_url = upstream.base_url();
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/reports/600460?limit=10&days=90")
        .to_request();
    let body: Value = test::call_and_read_body_json(&app, request).await;
    let rows = body.as_array().expect("reports response is an array");
    assert_eq!(rows.len(), 2, "infoCode 缺失的条目应被丢弃");
    assert_eq!(rows[0]["code"], "sh600460");
    assert_eq!(rows[0]["org"], "中信证券");
    assert_eq!(rows[0]["publish_date"], "2026-09-18");
    assert_eq!(rows[0]["rating"], "买入");
    assert_eq!(rows[0]["last_rating"], "增持");
    assert_eq!(rows[0]["rating_change"], 1);
    assert_eq!(rows[0]["aim_price_high"], 45.0);
    assert_eq!(rows[0]["aim_price_low"], 32.0);
    assert_eq!(
        rows[0]["url"],
        "https://data.eastmoney.com/report/info/AP202609181234.html"
    );
    // 线上 `ratingChange` 同页混用数字与字符串，两形态都要能落库。
    assert_eq!(rows[1]["rating_change"], 2);
    mock.assert();

    let persisted: (String, String) = sqlx::query_as(
        "SELECT org, rating FROM research_report WHERE code='sh600460' AND info_code='AP202609181234'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(persisted, ("中信证券".to_string(), "买入".to_string()));
}

#[actix_web::test]
async fn sector_endpoint_merges_membership_and_quotes() {
    let upstream = MockServer::start();
    let membership = upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/data/v1/get")
            .query_param("reportName", "RPT_F10_CORETHEME_BOARDTYPE");
        then.status(200).json_body(json!({
            "result": {"data": [{
                "BOARD_CODE": "977",
                "BOARD_NAME": "第三代半导体",
                "IS_PRECISE": "1",
                "NEW_BOARD_CODE": "BK0977",
                "SELECTED_BOARD_REASON": "主营功率器件"
            }, {
                "BOARD_CODE": "1234",
                "BOARD_NAME": "无新代码板块",
                "IS_PRECISE": "0",
                "NEW_BOARD_CODE": "",
                "SELECTED_BOARD_REASON": ""
            }]}
        }));
    });
    let quotes = upstream.mock(|when, then| {
        when.method(GET)
            .path("/api/qt/ulist.np/get")
            .query_param("secids", "90.BK0977,90.BK1234")
            .query_param("fields", "f2,f3,f12");
        then.status(200).json_body(json!({
            "data": {"diff": [
                {"f2": 1234.56, "f3": 2.34, "f12": "BK0977"},
                {"f2": "-", "f3": "-", "f12": "BK1234"}
            ]}
        }));
    });
    let mut state = fresh_state().await;
    state.sector.board_url = upstream.base_url();
    state.sector.quote_url = upstream.base_url();
    let db = state.db.clone();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/sector/600460")
        .to_request();
    let body: Value = test::call_and_read_body_json(&app, request).await;
    let rows = body.as_array().expect("sector response is an array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["board_code"], "BK0977");
    assert_eq!(rows[0]["name"], "第三代半导体");
    assert_eq!(rows[0]["is_precise"], true);
    assert_eq!(rows[0]["reason"], "主营功率器件");
    assert_eq!(rows[0]["price"], 1234.56);
    assert_eq!(rows[0]["change_pct"], 2.34);
    // BOARD_CODE 数字串自动补成 BK1234；"-" 收敛成 null。
    assert_eq!(rows[1]["board_code"], "BK1234");
    assert_eq!(rows[1]["is_precise"], false);
    assert!(rows[1]["price"].is_null());
    membership.assert();
    quotes.assert();

    let persisted: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sector_board WHERE code='sh600460'")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(persisted, 2);
}

#[actix_web::test]
async fn bad_code_returns_400_before_hitting_upstream() {
    let upstream = MockServer::start();
    let mock = upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200)
            .json_body(json!({"code": 0, "result": {"cmsArticleWebOld": []}}));
    });
    let mut state = fresh_state().await;
    state.news.base_url = upstream.base_url();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/news/not-a-code")
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), 400);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["error"], "bad_request");
    assert_eq!(mock.hits(), 0, "代码非法时不应打上游");
}

#[actix_web::test]
async fn upstream_5xx_maps_to_502() {
    let upstream = MockServer::start();
    upstream.mock(|when, then| {
        when.method(GET).path("/report/list");
        then.status(500).body("boom");
    });
    let mut state = fresh_state().await;
    state.reports.base_url = upstream.base_url();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state))
            .service(web::scope("/api/v1").configure(api::ingest::configure)),
    )
    .await;

    let request = test::TestRequest::get()
        .uri("/api/v1/reports/sh600460")
        .to_request();
    let response = test::call_service(&app, request).await;
    assert_eq!(response.status(), 502);
    let body: Value = test::read_body_json(response).await;
    assert_eq!(body["error"], "upstream_exhausted");
}

#[actix_web::test]
async fn analyze_injects_recent_news_when_requested() {
    let upstream = MockServer::start();
    let news = upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200).json_body(json!({
            "code": 0,
            "result": {"cmsArticleWebOld": [{
                "date": cn_time(0, 8, 15),
                "title": "士兰微获大额订单",
                "content": "摘要",
                "mediaName": "上证报",
                "url": "https://finance.eastmoney.com/a/202609202.html"
            }]}
        }));
    });
    let chat = upstream.mock(|when, then| {
        when.method(POST)
            .path("/v1/chat/completions")
            .body_contains("近 24h 新闻")
            .body_contains("士兰微获大额订单");
        then.status(200).json_body(json!({
            "choices": [{"message": {"content": "{\"summary\":\"利好\"}"}}],
            "usage": {"prompt_tokens": 20, "completion_tokens": 5}
        }));
    });
    let mut state = fresh_state().await;
    state.news.base_url = upstream.base_url();
    let app = test::init_service(
        App::new().app_data(web::Data::new(state)).service(
            web::scope("/api/v1")
                .configure(api::ingest::configure)
                .configure(api::ai::configure),
        ),
    )
    .await;

    let request = test::TestRequest::post()
        .uri("/api/v1/ai/analyze")
        .set_json(json!({
            "provider": "openai",
            "base_url": format!("{}/v1", upstream.base_url()),
            "api_key": "test-token",
            // GOVERNOR 是进程内全局单例（按 provider/model 分桶），
            // 同文件里的 analyze 测试必须用不同 model，否则会撞冷却。
            "model": "test-model-with-news",
            "user": "分析 sh600460",
            "code": "sh600460",
            "price": 12.34,
            "include_news": true
        }))
        .to_request();
    let response: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(response["content"], "{\"summary\":\"利好\"}");
    assert_eq!(response["tokens_in"], 20);
    news.assert();
    chat.assert();
}

#[actix_web::test]
async fn analyze_skips_news_by_default() {
    let upstream = MockServer::start();
    let news = upstream.mock(|when, then| {
        when.method(GET).path("/search/jsonp");
        then.status(200)
            .json_body(json!({"code": 0, "result": {"cmsArticleWebOld": []}}));
    });
    let chat = upstream.mock(|when, then| {
        when.method(POST).path("/v1/chat/completions");
        then.status(200).json_body(json!({
            "choices": [{"message": {"content": "{\"summary\":\"ok\"}"}}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2}
        }));
    });
    let mut state = fresh_state().await;
    state.news.base_url = upstream.base_url();
    let app = test::init_service(
        App::new().app_data(web::Data::new(state)).service(
            web::scope("/api/v1")
                .configure(api::ingest::configure)
                .configure(api::ai::configure),
        ),
    )
    .await;

    let request = test::TestRequest::post()
        .uri("/api/v1/ai/analyze")
        .set_json(json!({
            "provider": "openai",
            "base_url": format!("{}/v1", upstream.base_url()),
            "api_key": "test-token",
            "model": "test-model-plain",
            "user": "分析 sh600460",
            "code": "sh600460"
        }))
        .to_request();
    let _: Value = test::call_and_read_body_json(&app, request).await;
    assert_eq!(news.hits(), 0, "默认不拉新闻");
    chat.assert();
}

#[actix_web::test]
async fn scheduled_refresh_emits_report_signals_once() {
    let upstream = MockServer::start();
    let offset = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let today = chrono::Utc::now().with_timezone(&offset).date_naive();
    let yesterday = (today - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let old = (today - chrono::Duration::days(20))
        .format("%Y-%m-%d")
        .to_string();
    upstream.mock(|when, then| {
        when.method(GET).path("/report/list");
        then.status(200).json_body(json!({
            "data": [
                {
                    "title": "业绩超预期，维持高增长",
                    "orgSName": "中信证券",
                    "publishDate": format!("{yesterday} 00:00:00"),
                    "infoCode": "AP202609201111",
                    "indvInduName": "半导体",
                    "emRatingName": "买入",
                    "lastEmRatingName": "增持",
                    "ratingChange": 1,
                    "researcher": "张三",
                    "indvAimPriceT": "55.00",
                    "indvAimPriceL": "45.00"
                },
                {
                    "title": "中性评级的研报不应出信号",
                    "orgSName": "某机构",
                    "publishDate": format!("{yesterday} 00:00:00"),
                    "infoCode": "AP202609201112",
                    "emRatingName": "持有",
                    "ratingChange": 3
                },
                {
                    "title": "超出 7 天窗口的看多研报不应出信号",
                    "orgSName": "某机构",
                    "publishDate": format!("{old} 00:00:00"),
                    "infoCode": "AP202609001113",
                    "emRatingName": "买入",
                    "ratingChange": 3
                }
            ]
        }));
    });

    let mut state = fresh_state().await;
    // 三个 provider 都指向 mock：新闻 / 板块未注册 mock 会 404，warn 跳过不影响研报路径。
    state.news.base_url = upstream.base_url();
    state.reports.base_url = upstream.base_url();
    state.sector.board_url = upstream.base_url();
    state.sector.quote_url = upstream.base_url();
    sqlx::query("INSERT INTO watchlist(code, added_at, updated_at) VALUES('sh600460', 0, 0)")
        .execute(&state.db)
        .await
        .unwrap();

    use mojinprince_server::service::scheduler;
    let refreshed = scheduler::refresh_watchlist_ingest(&state).await.unwrap();
    assert_eq!(refreshed, 0, "新闻 / 板块 404，本标的整体不算刷新成功");

    let signal: (String, String, String, String) =
        sqlx::query_as("SELECT kind, title, body, meta FROM signal_event WHERE code='sh600460'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(signal.0, "report");
    assert_eq!(signal.1, "机构看多：中信证券上调至买入");
    assert!(
        signal.2.contains("目标价 45.00-55.00 元"),
        "body 含目标价：{}",
        signal.2
    );
    let meta: Value = serde_json::from_str(&signal.3).unwrap();
    assert_eq!(meta["reportId"], "AP202609201111");
    assert_eq!(meta["org"], "中信证券");

    // 同一份研报再刷一次 → INSERT OR IGNORE，仍然只有一条信号。
    scheduler::refresh_watchlist_ingest(&state).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM signal_event")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "同一研报幂等，不重复出信号");
}

// ---- live 测试：直连东财，网络可达时手动执行 `cargo test -- --ignored` ----

#[actix_web::test]
#[ignore = "hits live upstream"]
async fn live_news_sh600460() {
    let state = fresh_state().await;
    let items = state
        .news
        .fetch(&state.http, "sh600460", 5)
        .await
        .expect("eastmoney news reachable");
    assert!(!items.is_empty(), "600460 应有新闻");
    assert!(items.iter().all(|i| !i.url.is_empty()));
}

#[actix_web::test]
#[ignore = "hits live upstream"]
async fn live_reports_sh600460() {
    let state = fresh_state().await;
    let items = state
        .reports
        .fetch(&state.http, "sh600460", 5, 365)
        .await
        .expect("eastmoney reports reachable");
    assert!(!items.is_empty(), "600460 近一年应有研报");
    assert!(items.iter().all(|i| i.url.starts_with("https://")));
}

#[actix_web::test]
#[ignore = "hits live upstream"]
async fn live_sector_sh600460() {
    let state = fresh_state().await;
    let boards = state
        .sector
        .fetch(&state.http, "sh600460")
        .await
        .expect("eastmoney sector reachable");
    assert!(!boards.is_empty(), "600460 应属于至少一个概念板块");
    assert!(boards.iter().all(|b| b.board_code.starts_with("BK")));
}
