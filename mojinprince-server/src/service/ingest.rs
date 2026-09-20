//! §F.3–F.4 数据接入：东方财富 新闻 / 研报 / 概念板块。
//!
//! 三个 provider 都是纯拉取（返回模型数组），落库由 `persist_*` 完成，
//! 便于 HTTP handler 与每日调度器共用。`base_url` 全部公开，测试时注入 httpmock。
use super::quote::{normalize_code, QuoteError};
use crate::error::AppError;
use crate::model::{NewsItem, ResearchReport, SectorBoard};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;
use thiserror::Error;

/// 东财接口返回的时间都是北京时间（UTC+8）。
const CN_OFFSET_SECS: i32 = 8 * 3600;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("bad code: {0}")]
    BadCode(String),
}

impl From<QuoteError> for IngestError {
    fn from(value: QuoteError) -> Self {
        match value {
            QuoteError::BadCode(code) => IngestError::BadCode(code),
            QuoteError::Network(message) => IngestError::Network(message),
            QuoteError::Parse(message) => IngestError::Parse(message),
            QuoteError::Empty => IngestError::Parse("empty".into()),
        }
    }
}

impl From<IngestError> for AppError {
    fn from(value: IngestError) -> Self {
        match value {
            IngestError::BadCode(code) => AppError::BadRequest(code.into()),
            IngestError::Network(message) => AppError::UpstreamExhausted {
                tries: 1,
                source: message.into(),
            },
            IngestError::Parse(message) => AppError::UpstreamParse(message.into()),
        }
    }
}

fn pure_digits(normalized: &str) -> String {
    normalized
        .trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .to_string()
}

fn market_suffix(normalized: &str) -> &'static str {
    if normalized.starts_with("sh") {
        "SH"
    } else if normalized.starts_with("bj") {
        "BJ"
    } else {
        "SZ"
    }
}

fn cn_offset() -> FixedOffset {
    FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset")
}

/// 解析北京时间字符串（"2026-09-18 16:35:00"）；异常时退回当前时间。
fn parse_cn_datetime(raw: Option<&str>) -> DateTime<Utc> {
    raw.and_then(|s| NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S").ok())
        .and_then(|naive| cn_offset().from_local_datetime(&naive).single())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

/// 东财 push2 系列接口在停牌 / 无数据时会给字符串 "-"，统一收敛成 None。
fn json_num(value: Option<&serde_json::Value>) -> Option<f64> {
    value.and_then(|v| v.as_f64())
}

/// 东财同一字段会混用数字与字符串（如研报 `ratingChange` 同页既有 3 也有 "3"），两形态都收。
fn json_i64(value: Option<&serde_json::Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

// ---------- 新闻（F.3）----------

/// 东财全文搜索（按时间倒序）→ 个股新闻流。
#[derive(Debug, Clone)]
pub struct EastMoneyNews {
    /// 可在测试时覆盖的 base URL（默认 `https://search-api-web.eastmoney.com`）
    pub base_url: String,
}

impl Default for EastMoneyNews {
    fn default() -> Self {
        Self {
            base_url: "https://search-api-web.eastmoney.com".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NewsEnvelope {
    code: Option<i32>,
    result: Option<NewsResult>,
}

#[derive(Debug, Deserialize)]
struct NewsResult {
    #[serde(rename = "cmsArticleWebOld")]
    cms_article: Option<Vec<NewsRow>>,
}

#[derive(Debug, Deserialize)]
struct NewsRow {
    date: Option<String>,
    title: Option<String>,
    content: Option<String>,
    #[serde(rename = "mediaName")]
    media_name: Option<String>,
    url: Option<String>,
}

impl EastMoneyNews {
    /// 拉取个股相关新闻，按时间倒序最多 `limit` 条。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        code: &str,
        limit: usize,
    ) -> Result<Vec<NewsItem>, IngestError> {
        let normalized = normalize_code(code)?;
        let param = serde_json::json!({
            "uid": "",
            "keyword": pure_digits(&normalized),
            "type": ["cmsArticleWebOld"],
            "client": "web",
            "clientType": "web",
            "clientVersion": "curr",
            "param": {"cmsArticleWebOld": {
                "searchScope": "default",
                "sort": "time",
                "pageIndex": 1,
                "pageSize": limit,
                "preTag": "",
                "postTag": ""
            }}
        });
        let body = param.to_string();
        let resp = http
            .get(format!("{}/search/jsonp", self.base_url))
            .query(&[("cb", ""), ("param", body.as_str())])
            .send()
            .await
            .map_err(|e| IngestError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(IngestError::Network(format!("status {}", resp.status())));
        }
        let parsed: NewsEnvelope = resp
            .json()
            .await
            .map_err(|e| IngestError::Parse(e.to_string()))?;
        if parsed.code.unwrap_or(0) != 0 {
            return Err(IngestError::Parse(format!("code={:?}", parsed.code)));
        }
        let rows = parsed
            .result
            .and_then(|r| r.cms_article)
            .unwrap_or_default();
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let url = row.url.filter(|u| !u.trim().is_empty())?;
                Some(NewsItem {
                    code: normalized.clone(),
                    title: row.title.unwrap_or_default(),
                    summary: row.content.unwrap_or_default(),
                    media: row.media_name.unwrap_or_default(),
                    url,
                    published_at: parse_cn_datetime(row.date.as_deref()),
                })
            })
            .collect())
    }
}

// ---------- 研报（F.4）----------

/// 东财研报库 → 机构评级 / 目标价。
#[derive(Debug, Clone)]
pub struct EastMoneyReports {
    /// 可在测试时覆盖的 base URL（默认 `https://reportapi.eastmoney.com`）
    pub base_url: String,
}

impl Default for EastMoneyReports {
    fn default() -> Self {
        Self {
            base_url: "https://reportapi.eastmoney.com".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ReportEnvelope {
    data: Option<Vec<ReportRow>>,
}

#[derive(Debug, Deserialize)]
struct ReportRow {
    title: Option<String>,
    #[serde(rename = "orgSName")]
    org_s_name: Option<String>,
    #[serde(rename = "publishDate")]
    publish_date: Option<String>,
    #[serde(rename = "infoCode")]
    info_code: Option<String>,
    #[serde(rename = "indvInduName")]
    indv_indu_name: Option<String>,
    #[serde(rename = "emRatingName")]
    em_rating_name: Option<String>,
    #[serde(rename = "lastEmRatingName")]
    last_em_rating_name: Option<String>,
    #[serde(rename = "ratingChange")]
    rating_change: Option<serde_json::Value>,
    researcher: Option<String>,
    #[serde(rename = "indvAimPriceT")]
    aim_price_high: Option<String>,
    #[serde(rename = "indvAimPriceL")]
    aim_price_low: Option<String>,
}

impl EastMoneyReports {
    /// 拉取最近 `days` 天的研报，按发布日期倒序最多 `limit` 条。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        code: &str,
        limit: usize,
        days: i64,
    ) -> Result<Vec<ResearchReport>, IngestError> {
        let normalized = normalize_code(code)?;
        let today = Utc::now().with_timezone(&cn_offset()).date_naive();
        let begin = today - chrono::Duration::days(days.clamp(1, 1095));
        let pure = pure_digits(&normalized);
        let page_size = limit.to_string();
        let begin_text = begin.format("%Y-%m-%d").to_string();
        let end_text = today.format("%Y-%m-%d").to_string();
        let resp = http
            .get(format!("{}/report/list", self.base_url))
            .query(&[
                ("code", pure.as_str()),
                ("pageSize", page_size.as_str()),
                ("pageNo", "1"),
                ("qType", "0"),
                ("beginTime", begin_text.as_str()),
                ("endTime", end_text.as_str()),
            ])
            .send()
            .await
            .map_err(|e| IngestError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(IngestError::Network(format!("status {}", resp.status())));
        }
        let parsed: ReportEnvelope = resp
            .json()
            .await
            .map_err(|e| IngestError::Parse(e.to_string()))?;
        Ok(parsed
            .data
            .unwrap_or_default()
            .into_iter()
            .filter_map(|row| {
                let info_code = row.info_code.filter(|c| !c.trim().is_empty())?;
                Some(ResearchReport {
                    code: normalized.clone(),
                    title: row.title.unwrap_or_default(),
                    org: row.org_s_name.unwrap_or_default(),
                    publish_date: row
                        .publish_date
                        .as_deref()
                        .and_then(|d| d.get(..10))
                        .unwrap_or("")
                        .to_string(),
                    rating: row.em_rating_name.unwrap_or_default(),
                    last_rating: row.last_em_rating_name.unwrap_or_default(),
                    rating_change: json_i64(row.rating_change.as_ref()),
                    researcher: row.researcher.unwrap_or_default(),
                    industry: row.indv_indu_name.unwrap_or_default(),
                    aim_price_high: parse_price(row.aim_price_high.as_deref()),
                    aim_price_low: parse_price(row.aim_price_low.as_deref()),
                    url: format!("https://data.eastmoney.com/report/info/{info_code}.html"),
                })
            })
            .collect())
    }
}

fn parse_price(raw: Option<&str>) -> Option<f64> {
    raw.map(str::trim).filter(|s| !s.is_empty()).and_then(|s| s.parse().ok())
}

// ---------- 概念板块 ----------

/// 个股所属概念板块：成分列表（datacenter-web）+ 板块行情（push2delay 批量）。
#[derive(Debug, Clone)]
pub struct EastMoneySector {
    /// 板块成分接口（默认 `https://datacenter-web.eastmoney.com`）
    pub board_url: String,
    /// 板块行情接口（默认 `https://push2delay.eastmoney.com`；push2 主站部分网络不可达）
    pub quote_url: String,
}

impl Default for EastMoneySector {
    fn default() -> Self {
        Self {
            board_url: "https://datacenter-web.eastmoney.com".to_string(),
            quote_url: "https://push2delay.eastmoney.com".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct DataCenterEnvelope {
    result: Option<DataCenterResult>,
}

#[derive(Debug, Deserialize)]
struct DataCenterResult {
    data: Option<Vec<BoardRow>>,
}

#[derive(Debug, Deserialize)]
struct BoardRow {
    #[serde(rename = "BOARD_CODE")]
    board_code: Option<String>,
    #[serde(rename = "BOARD_NAME")]
    board_name: Option<String>,
    #[serde(rename = "IS_PRECISE")]
    is_precise: Option<String>,
    #[serde(rename = "NEW_BOARD_CODE")]
    new_board_code: Option<String>,
    #[serde(rename = "SELECTED_BOARD_REASON")]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UlistEnvelope {
    data: Option<UlistData>,
}

#[derive(Debug, Deserialize)]
struct UlistData {
    diff: Option<Vec<serde_json::Value>>,
}

impl EastMoneySector {
    /// 拉取个股所属板块（最多 50 个）并合并最新指数 / 涨跌幅。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        code: &str,
    ) -> Result<Vec<SectorBoard>, IngestError> {
        let normalized = normalize_code(code)?;
        let secu = format!(
            "{}.{}",
            pure_digits(&normalized),
            market_suffix(&normalized)
        );
        let filter = format!("(SECUCODE=\"{secu}\")");
        let resp = http
            .get(format!("{}/api/data/v1/get", self.board_url))
            .query(&[
                ("reportName", "RPT_F10_CORETHEME_BOARDTYPE"),
                ("columns", "ALL"),
                ("filter", filter.as_str()),
                ("pageSize", "100"),
                ("pageNumber", "1"),
            ])
            .send()
            .await
            .map_err(|e| IngestError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(IngestError::Network(format!("status {}", resp.status())));
        }
        let parsed: DataCenterEnvelope = resp
            .json()
            .await
            .map_err(|e| IngestError::Parse(e.to_string()))?;
        let rows = parsed.result.and_then(|r| r.data).unwrap_or_default();
        let mut boards: Vec<SectorBoard> = rows
            .into_iter()
            .filter_map(|row| {
                let name = row.board_name.filter(|n| !n.trim().is_empty())?;
                let board_code = row
                    .new_board_code
                    .filter(|c| !c.trim().is_empty())
                    .or_else(|| {
                        row.board_code
                            .and_then(|c| c.trim().parse::<u32>().ok())
                            .map(|n| format!("BK{:04}", n))
                    })?;
                Some(SectorBoard {
                    code: normalized.clone(),
                    board_code,
                    name,
                    is_precise: row.is_precise.as_deref() == Some("1"),
                    reason: row.reason.unwrap_or_default(),
                    price: None,
                    change_pct: None,
                })
            })
            .take(50)
            .collect();
        if boards.is_empty() {
            return Ok(boards);
        }

        let secids: Vec<String> = boards
            .iter()
            .map(|b| format!("90.{}", b.board_code))
            .collect();
        let secids_joined = secids.join(",");
        let resp = http
            .get(format!("{}/api/qt/ulist.np/get", self.quote_url))
            .query(&[
                ("secids", secids_joined.as_str()),
                ("fields", "f2,f3,f12"),
                ("invt", "2"),
                ("fltt", "2"),
            ])
            .send()
            .await
            .map_err(|e| IngestError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(IngestError::Network(format!("status {}", resp.status())));
        }
        let quotes: UlistEnvelope = resp
            .json()
            .await
            .map_err(|e| IngestError::Parse(e.to_string()))?;
        let diff = quotes.data.and_then(|d| d.diff).unwrap_or_default();
        for board in &mut boards {
            if let Some(row) = diff.iter().find(|row| {
                row.get("f12").and_then(|v| v.as_str()) == Some(board.board_code.as_str())
            }) {
                board.price = json_num(row.get("f2"));
                board.change_pct = json_num(row.get("f3"));
            }
        }
        Ok(boards)
    }
}

// ---------- 落库 ----------

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub async fn persist_news(db: &SqlitePool, items: &[NewsItem]) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for item in items {
        sqlx::query(
            "INSERT OR REPLACE INTO news_item(code,url,title,summary,media,published_at,fetched_at) \
             VALUES(?,?,?,?,?,?,?)",
        )
        .bind(&item.code)
        .bind(&item.url)
        .bind(&item.title)
        .bind(&item.summary)
        .bind(&item.media)
        .bind(item.published_at.timestamp_millis())
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn persist_reports(
    db: &SqlitePool,
    items: &[ResearchReport],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for item in items {
        let info_code = item
            .url
            .trim_start_matches("https://data.eastmoney.com/report/info/")
            .trim_end_matches(".html")
            .to_string();
        sqlx::query(
            "INSERT OR REPLACE INTO research_report(code,info_code,title,org,publish_date,rating,\
             last_rating,rating_change,researcher,industry,aim_price_high,aim_price_low,url,fetched_at) \
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&item.code)
        .bind(&info_code)
        .bind(&item.title)
        .bind(&item.org)
        .bind(&item.publish_date)
        .bind(&item.rating)
        .bind(&item.last_rating)
        .bind(item.rating_change)
        .bind(&item.researcher)
        .bind(&item.industry)
        .bind(item.aim_price_high)
        .bind(item.aim_price_low)
        .bind(&item.url)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn persist_sector_boards(
    db: &SqlitePool,
    items: &[SectorBoard],
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    for item in items {
        sqlx::query(
            "INSERT OR REPLACE INTO sector_board(code,board_code,board_name,is_precise,reason,\
             price,change_pct,fetched_at) VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(&item.code)
        .bind(&item.board_code)
        .bind(&item.name)
        .bind(if item.is_precise { 1 } else { 0 })
        .bind(&item.reason)
        .bind(item.price)
        .bind(item.change_pct)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cn_datetime_shifts_to_utc() {
        let dt = parse_cn_datetime(Some("2026-09-18 16:35:00"));
        assert_eq!(dt.format("%Y-%m-%d %H:%M:%S").to_string(), "2026-09-18 08:35:00");
    }

    #[test]
    fn json_num_tolerates_dash() {
        assert_eq!(json_num(Some(&serde_json::json!(2248.73))), Some(2248.73));
        assert_eq!(json_num(Some(&serde_json::json!("-"))), None);
        assert_eq!(json_num(None), None);
        assert_eq!(parse_price(Some("55.0000000000")), Some(55.0));
        assert_eq!(parse_price(Some("")), None);
    }

    #[test]
    fn json_i64_accepts_number_and_string() {
        assert_eq!(json_i64(Some(&serde_json::json!(3))), Some(3));
        assert_eq!(json_i64(Some(&serde_json::json!("2"))), Some(2));
        assert_eq!(json_i64(Some(&serde_json::json!("-"))), None);
        assert_eq!(json_i64(None), None);
    }
}
