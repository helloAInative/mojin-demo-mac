//! 全局共享状态：DB 连接池、HTTP 客户端、行情 failover 引擎
use crate::config::Config;
use crate::model::Quote;
use crate::service::ingest::{EastMoneyNews, EastMoneyReports, EastMoneySector};
use crate::service::pick::{DayKSource, EastMoneyRanking, SinaRanking, TencentUsIndex};
use crate::service::quote::failover::QuoteFailover;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct QuotePushEvent {
    #[serde(rename = "type")]
    pub kind: String,
    pub quote: Quote,
}

#[derive(Clone)]
pub struct QuoteHub {
    tx: broadcast::Sender<QuotePushEvent>,
}

impl QuoteHub {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<QuotePushEvent> {
        self.tx.subscribe()
    }

    pub fn publish(&self, quote: Quote) {
        let _ = self.tx.send(QuotePushEvent {
            kind: "quote".into(),
            quote,
        });
    }
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: SqlitePool,
    pub http: reqwest::Client,
    pub ai_http: reqwest::Client,
    pub failover: Arc<QuoteFailover>,
    pub quote_hub: QuoteHub,
    /// §F.3 新闻（东财全文搜索）
    pub news: EastMoneyNews,
    /// §F.4 研报（东财研报库）
    pub reports: EastMoneyReports,
    /// 概念板块（成分 + 行情）
    pub sector: EastMoneySector,
    /// 智能推荐：东财涨幅榜候选源
    pub pick_ranking: EastMoneyRanking,
    /// 智能推荐：新浪涨幅榜（东财断连 fallback）
    pub sina_ranking: SinaRanking,
    /// 智能推荐：腾讯日 K 源
    pub day_k: DayKSource,
    /// 智能推荐：隔夜美股情绪源
    pub us_index: TencentUsIndex,
}

impl AppState {
    pub async fn new(cfg: Config) -> anyhow::Result<Self> {
        // SQLite WAL + 串行化写：先同步创建父目录（sqlx 在异步阶段不创建）
        if cfg.database_url.starts_with("sqlite:") {
            let path = cfg
                .database_url
                .trim_start_matches("sqlite://")
                .trim_start_matches("sqlite:");
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).ok();
                }
            }
        }
        let options = sqlx::sqlite::SqliteConnectOptions::from_str(&cfg.database_url)?
            .create_if_missing(true);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::query("PRAGMA journal_mode=WAL")
            .execute(&pool)
            .await
            .ok();
        sqlx::query("PRAGMA synchronous=NORMAL")
            .execute(&pool)
            .await
            .ok();

        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(cfg.quote_timeout_ms))
            .user_agent("mojinprince-server/0.1 (+https://github.com/mojinprince)")
            .build()?;
        let ai_http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(cfg.ai_timeout_ms))
            .user_agent("mojinprince-server/0.2 (+https://github.com/helloAInative/mojin-demo-mac)")
            .build()?;

        let failover = Arc::new(QuoteFailover::new(
            cfg.quote_sources.clone(),
            cfg.quote_fail_threshold,
        ));

        Ok(Self {
            cfg: Arc::new(cfg),
            db: pool,
            http,
            ai_http,
            failover,
            quote_hub: QuoteHub::new(512),
            news: EastMoneyNews::default(),
            reports: EastMoneyReports::default(),
            sector: EastMoneySector::default(),
            pick_ranking: EastMoneyRanking::default(),
            sina_ranking: SinaRanking::default(),
            day_k: DayKSource::default(),
            us_index: TencentUsIndex::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn quote_hub_broadcasts_serializable_event() {
        let hub = QuoteHub::new(4);
        let mut receiver = hub.subscribe();
        hub.publish(Quote {
            code: "sh600460".into(),
            name: "士兰微".into(),
            price: 32.61,
            prev: 31.99,
            open: 32.5,
            high: 32.75,
            low: 31.77,
            volume: 1,
            amount: 2.0,
            source: "test".into(),
            ts: Utc::now(),
        });
        let event = receiver.recv().await.unwrap();
        assert_eq!(event.kind, "quote");
        assert_eq!(event.quote.code, "sh600460");
        assert!(serde_json::to_string(&event)
            .unwrap()
            .contains("\"type\":\"quote\""));
    }
}
