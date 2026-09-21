//! signal_event 落库 + 查询。
//! 表结构见 migrations/20250918000001_init.sql。
//! 注意 `meta` 字段在 SQLite 里是 JSON 文本，写入时手动 `json!({...}).to_string()`。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SignalEventRow {
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: String,
    pub code: String,
    pub title: String,
    pub body: String,
    pub price: f64,
    pub source: String,
    pub evidence: String,
    pub why: String,
    /// JSON 文本；前端用 [String: String] 解码，旧 Swift 客户端的 [String: String] 模型兼容
    pub meta: String,
}

#[derive(Clone)]
pub struct SignalRepo {
    db: SqlitePool,
}

impl SignalRepo {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    pub async fn upsert(&self, e: &SignalEventRow) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO signal_event(id,at,kind,code,title,body,price,source,evidence,why,meta) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET \
                kind=excluded.kind, code=excluded.code, title=excluded.title, body=excluded.body, \
                price=excluded.price, source=excluded.source, evidence=excluded.evidence, \
                why=excluded.why, meta=excluded.meta",
        )
        .bind(&e.id)
        .bind(e.at.timestamp_millis())
        .bind(&e.kind)
        .bind(&e.code)
        .bind(&e.title)
        .bind(&e.body)
        .bind(e.price)
        .bind(&e.source)
        .bind(&e.evidence)
        .bind(&e.why)
        .bind(&e.meta)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    pub async fn list_recent(&self, limit: i64) -> Result<Vec<SignalEventRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, SignalRowRaw>(
            "SELECT id,at,kind,code,title,body,price,source,evidence,why,meta \
             FROM signal_event ORDER BY at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(SignalEventRow::from).collect())
    }

    pub async fn list_by_code(
        &self,
        code: &str,
        limit: i64,
    ) -> Result<Vec<SignalEventRow>, sqlx::Error> {
        let rows = sqlx::query_as::<_, SignalRowRaw>(
            "SELECT id,at,kind,code,title,body,price,source,evidence,why,meta \
             FROM signal_event WHERE code=? ORDER BY at DESC LIMIT ?",
        )
        .bind(code)
        .bind(limit)
        .fetch_all(&self.db)
        .await?;
        Ok(rows.into_iter().map(SignalEventRow::from).collect())
    }

    pub async fn patch_meta(&self, id: &str, patch: &serde_json::Value) -> Result<(), sqlx::Error> {
        // 读取后合并再写回（事务）。
        let mut tx = self.db.begin().await?;
        let row: Option<(String,)> = sqlx::query_as("SELECT meta FROM signal_event WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        let mut cur: serde_json::Value = match row {
            Some((s,)) if !s.is_empty() => {
                serde_json::from_str(&s).unwrap_or(serde_json::json!({}))
            }
            _ => serde_json::json!({}),
        };
        if let (Some(cur_obj), Some(patch_obj)) = (cur.as_object_mut(), patch.as_object()) {
            for (k, v) in patch_obj {
                cur_obj.insert(k.clone(), v.clone());
            }
        } else {
            cur = patch.clone();
        }
        let new_meta = serde_json::to_string(&cur).unwrap_or_else(|_| "{}".to_string());
        sqlx::query("UPDATE signal_event SET meta=? WHERE id=?")
            .bind(&new_meta)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM signal_event WHERE id=?")
            .bind(id)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct SignalRowRaw {
    id: String,
    at: i64,
    kind: String,
    code: String,
    title: String,
    body: String,
    price: f64,
    source: String,
    evidence: String,
    why: String,
    meta: String,
}

impl From<SignalRowRaw> for SignalEventRow {
    fn from(r: SignalRowRaw) -> Self {
        Self {
            id: r.id,
            at: DateTime::<Utc>::from_timestamp_millis(r.at).unwrap_or_else(Utc::now),
            kind: r.kind,
            code: r.code,
            title: r.title,
            body: r.body,
            price: r.price,
            source: r.source,
            evidence: r.evidence,
            why: r.why,
            meta: r.meta,
        }
    }
}
