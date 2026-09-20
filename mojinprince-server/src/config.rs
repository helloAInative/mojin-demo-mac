//! 配置加载：从环境变量 + .env 文件读取
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub bind_addr: String,
    pub database_url: String,
    /// 单次行情请求超时（毫秒）
    pub quote_timeout_ms: u64,
    /// 单源连续失败 N 次后切换备用源
    pub quote_fail_threshold: u32,
    /// 主源选择顺序：逗号分隔，如 "sina,tencent,eastmoney"
    pub quote_sources: Vec<String>,
    /// Swagger UI 路径前缀
    pub api_prefix: String,
    /// JWT 密钥（空 = 关闭鉴权，本地部署默认）
    pub jwt_secret: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        let bind_addr = env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8732".to_string());
        let database_url = env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://./data/mojinprince.db".to_string());
        let quote_timeout_ms = env::var("QUOTE_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3000);
        let quote_fail_threshold = env::var("QUOTE_FAIL_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let quote_sources = env::var("QUOTE_SOURCES")
            .unwrap_or_else(|_| "sina,tencent,eastmoney".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        let api_prefix = env::var("API_PREFIX").unwrap_or_else(|_| "/api/v1".to_string());
        let jwt_secret = env::var("JWT_SECRET").unwrap_or_default();
        Ok(Self {
            bind_addr,
            database_url,
            quote_timeout_ms,
            quote_fail_threshold,
            quote_sources,
            api_prefix,
            jwt_secret,
        })
    }
}
