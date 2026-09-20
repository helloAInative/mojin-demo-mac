//! 全局共享状态：DB 连接池、HTTP 客户端、行情 failover 引擎
use crate::config::Config;
use crate::service::quote::failover::QuoteFailover;
use sqlx::SqlitePool;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: SqlitePool,
    pub http: reqwest::Client,
    pub ai_http: reqwest::Client,
    pub failover: Arc<QuoteFailover>,
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
        })
    }
}
