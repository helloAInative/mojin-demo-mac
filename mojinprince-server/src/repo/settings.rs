//! settings / position / watchlist 三个 KV 表的统一 repo。
//! 每个 row 是 (key TEXT PRIMARY KEY, value JSON TEXT, updated_at INTEGER)。
//! 前端 AppSettings 的整块 JSON（{symbols, levels, positions, ...}）拆成多 key 存，
//! 单 key 取不会因 fields 缺字段破坏整体（向前兼容；参见 docs/架构-Rust后端.md §6）。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SettingsRow {
    pub key: String,
    /// 任意 JSON 值（前端用 [String: Any] 解码；这里存文本，由前端解析）
    pub value: serde_json::Value,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct SettingsRepo {
    db: SqlitePool,
}

impl SettingsRepo {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn ensure_tables(&self) -> Result<(), sqlx::Error> {
        // 注意：本表的建表写在 0005_settings.sql；这里保持兼容仅做 no-op。
        Ok(())
    }

    pub async fn get_all(&self) -> Result<HashMap<String, serde_json::Value>, sqlx::Error> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM settings",
        )
        .fetch_all(&self.db)
        .await?;
        let mut out = HashMap::new();
        for (k, v) in rows {
            let parsed: serde_json::Value =
                serde_json::from_str(&v).unwrap_or(serde_json::Value::Null);
            out.insert(k, parsed);
        }
        Ok(out)
    }

    pub async fn get(&self, key: &str) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM settings WHERE key=?")
                .bind(key)
                .fetch_optional(&self.db)
                .await?;
        Ok(row.and_then(|(s,)| {
            if s.is_empty() {
                None
            } else {
                serde_json::from_str(&s).ok()
            }
        }))
    }

    pub async fn put(&self, key: &str, value: &serde_json::Value) -> Result<(), sqlx::Error> {
        let s = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        sqlx::query(
            "INSERT INTO settings(key, value, updated_at) VALUES(?,?,?) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        )
        .bind(key)
        .bind(&s)
        .bind(Utc::now().timestamp_millis())
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn put_many(&self, items: &HashMap<String, serde_json::Value>) -> Result<(), sqlx::Error> {
        let mut tx = self.db.begin().await?;
        let now = Utc::now().timestamp_millis();
        for (k, v) in items {
            let s = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
            sqlx::query(
                "INSERT INTO settings(key, value, updated_at) VALUES(?,?,?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            )
            .bind(k)
            .bind(&s)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM settings WHERE key=?")
            .bind(key)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}