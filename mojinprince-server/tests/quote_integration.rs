//! 集成测试：failover 调度 + provider 解析 + 错误响应结构。
//!
//! 默认 `cargo test` 跑：mock + 端到端 fake。
//! 真实外网测试在 `live_upstream_*`，标 `#[ignore]`，需 `cargo test -- --ignored` 显式启用。
//!
//! 验收对应 docs/架构-Rust后端.md §11：
//! - 三源代码完成且 enum 派发正常
//! - 错误响应结构化（HTTP 状态码 + JSON error/message）
//! - failover 触发 ≤ 3s

use encoding_rs::GBK;
use httpmock::prelude::*;
use mojinprince_server::service::quote::{
    failover::QuoteFailover, EastMoney, QuoteProviders, Sina, Tencent,
};
use mojinprince_server::AppError;
use std::time::Instant;

/// 把 str 编码成 GBK 字节，喂 httpmock 模拟新浪响应。
fn gbk_bytes(s: &str) -> Vec<u8> {
    let (bytes, _, _) = GBK.encode(s);
    bytes.into_owned()
}

fn new_http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(3000))
        .build()
        .unwrap()
}

#[tokio::test]
async fn failover_succeeds_on_first_source() {
    let server = MockServer::start();
    let _m = server.mock(|when, then| {
        when.method(GET).path("/list=sh600460");
        then.status(200)
            .header("content-type", "text/plain; charset=gbk")
            // 新浪至少要 32 个字段（用 0 填充）：
            // 0=名称 1=今开 2=昨收 3=现价 4=最高 5=最低
            // 6=竞买价 7=竞卖价 8=成交量(股) 9=成交额(元)
            // 10..31 后续买卖盘等填充即可
            .body(gbk_bytes(
                r#"var hq_str_sh600460="士兰微,32.50,31.99,32.61,32.75,31.77,32.60,32.61,55092183,1779905404,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0";
"#,
            ));
    });
    let sina = Sina {
        base_url: server.base_url(),
    };
    let failover =
        QuoteFailover::with_providers(vec![QuoteProviders::Sina(sina)], vec!["sina".into()], 3);
    let (q, src) = failover
        .fetch_quote(&new_http(), "sh600460")
        .await
        .expect("sina ok");
    assert_eq!(src, "sina");
    assert_eq!(q.code, "sh600460");
    assert!((q.price - 32.61).abs() < 1e-6);
    assert_eq!(q.name, "士兰微");
}

#[tokio::test]
async fn failover_switches_when_primary_fails() {
    let server = MockServer::start();
    // 新浪返回 500（连续失败）
    let _sina_fail = server.mock(|when, then| {
        when.method(GET).path("/list=sh600460");
        then.status(500);
    });
    // 腾讯兜底成功（GBK）
    let payload = format!(
        r#"v_sh600460="1~士兰微~600460~32.61~31.99~32.50~550921~~32.75~31.77~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~32.75~31.77~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0~0";
"#
    );
    let _tencent_ok = server.mock(|when, then| {
        when.method(GET).path("/q=sh600460");
        then.status(200)
            .header("content-type", "text/plain; charset=gbk")
            .body(gbk_bytes(&payload));
    });

    let failover = QuoteFailover::with_providers(
        vec![
            QuoteProviders::Sina(Sina {
                base_url: server.base_url(),
            }),
            QuoteProviders::Tencent(Tencent {
                base_url: server.base_url(),
            }),
        ],
        vec!["sina".into(), "tencent".into()],
        3,
    );

    let start = Instant::now();
    let (q, src) = failover
        .fetch_quote(&new_http(), "sh600460")
        .await
        .expect("tencent fallback ok");
    let elapsed = start.elapsed();

    assert_eq!(src, "tencent");
    assert!((q.price - 32.61).abs() < 1e-6);
    assert!(
        elapsed.as_secs() < 3,
        "failover must finish within 3s, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn failover_returns_error_when_all_fail() {
    let server = MockServer::start();
    let _fail = server.mock(|when, then| {
        when.method(GET);
        then.status(502);
    });
    let failover = QuoteFailover::with_providers(
        vec![
            QuoteProviders::Sina(Sina {
                base_url: server.base_url(),
            }),
            QuoteProviders::Tencent(Tencent {
                base_url: server.base_url(),
            }),
            QuoteProviders::EastMoney(EastMoney {
                base_url: server.base_url(),
            }),
        ],
        vec!["sina".into(), "tencent".into(), "eastmoney".into()],
        3,
    );
    let err = failover
        .fetch_quote(&new_http(), "sh600460")
        .await
        .expect_err("all fail");
    let msg = err.to_string();
    assert!(msg.contains("upstream failed after"), "msg={msg}");
    assert!(msg.contains("502") || msg.contains("status"), "msg={msg}");
}

#[tokio::test]
async fn breaker_skips_after_threshold() {
    let server = MockServer::start();
    let _always_fail = server.mock(|when, then| {
        when.method(GET).path("/list=sh600460");
        then.status(502);
    });
    // 备源拿齐走自己另一条 mock，便于区分
    let _tencent_ok = server.mock(|when, then| {
        when.method(GET).path("/q=sh600460");
        then.status(200)
            .header("content-type", "text/plain; charset=gbk")
            .body(gbk_bytes(
                r#"v_sh600460="1~士兰微~600460~32.61~31.99~32.50~~~32.75~31.77";
"#,
            ));
    });

    let failover = QuoteFailover::with_providers(
        vec![
            QuoteProviders::Sina(Sina {
                base_url: server.base_url(),
            }),
            QuoteProviders::Tencent(Tencent {
                base_url: server.base_url(),
            }),
        ],
        vec!["sina".into(), "tencent".into()],
        // 阈值 = 2：两次失败就熔断
        2,
    );

    // 第一次：sina 失败（counter=1） → tencent 成功 → 返回 ok
    let (_q1, src1) = failover.fetch_quote(&new_http(), "sh600460").await.unwrap();
    assert_eq!(src1, "tencent");

    // 第二次：sina 失败（counter=2 → 触发熔断） → tencent 成功 → 返回 ok
    let (_q2, src2) = failover.fetch_quote(&new_http(), "sh600460").await.unwrap();
    assert_eq!(src2, "tencent");

    // 第三次：sina 已熔断 → 直接 tencent 成功
    let start = Instant::now();
    let (_q3, src3) = failover.fetch_quote(&new_http(), "sh600460").await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(src3, "tencent");
    assert!(
        elapsed.as_millis() < 200,
        "breaker should skip sina entirely, got {:?}",
        elapsed
    );
}

#[tokio::test]
async fn bad_code_is_rejected_without_http() {
    // 形如 "notacode"：连 normalize 都过不去，不应该发 HTTP 请求。
    // 通过用一个从不命中的 mock 守门：若真发了，mock 报 unmocked。
    let server = MockServer::start();
    let failover = QuoteFailover::with_providers(
        vec![QuoteProviders::Sina(Sina {
            base_url: server.base_url(),
        })],
        vec!["sina".into()],
        3,
    );
    let err = failover
        .fetch_quote(&new_http(), "notacode")
        .await
        .expect_err("bad code");
    assert!(
        matches!(err, AppError::BadRequest(_)),
        "expected 400 BadRequest, got {err:?}"
    );
}

// ---------- 真实外网（默认不跑，需 --ignored）----------

#[tokio::test]
#[ignore = "hits live upstream"]
async fn live_sina_sh600460() {
    let failover = QuoteFailover::new(vec!["sina".into(), "tencent".into(), "eastmoney".into()], 3);
    let (q, src) = failover
        .fetch_quote(&new_http(), "sh600460")
        .await
        .expect("live ok");
    eprintln!("live source={src} code={} price={}", q.code, q.price);
    assert!(q.price > 0.0);
}

#[tokio::test]
#[ignore = "hits live upstream"]
async fn live_tencent_sz000001() {
    let failover = QuoteFailover::new(vec!["tencent".into()], 3);
    let (q, _src) = failover
        .fetch_quote(&new_http(), "sz000001")
        .await
        .expect("tencent live ok");
    assert!(q.price > 0.0);
}
