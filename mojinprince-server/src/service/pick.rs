//! 智能推荐（四层漏斗）：
//! ① 东财涨幅榜 Top100 候选 → ② 技术指标量化粗筛（纯函数可单测）
//! → ③ 研报评级 / 新闻热度加分（查既有缓存表）→ ④ AI 精排（客户端
//! 透传配置触发，密钥不落库；调度器自动版为纯量化）。
//!
//! 落库 `daily_pick`，按 (date, code) 主键；T+1/T+5 收盘对照回写
//! `meta.outcome` 形成命中率闭环。
use crate::error::AppError;
use crate::model::pick::{AiRankConfig, DailyPick, PickStats, PicksDocument};
use crate::model::DayBar;
use crate::service::ai::{self, ChatMessage};
use crate::state::AppState;
use chrono::{Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::time::Duration as StdDuration;

/// 腾讯前复权日 K 源（base_url 可在测试覆写，默认 web.ifzq.gtimg.cn）。
#[derive(Debug, Clone)]
pub struct DayKSource {
    pub base_url: String,
}

impl Default for DayKSource {
    fn default() -> Self {
        Self {
            base_url: "https://web.ifzq.gtimg.cn".into(),
        }
    }
}

impl DayKSource {
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        code: &str,
        limit: i64,
    ) -> Result<Vec<DayBar>, PickError> {
        let upstream_limit = limit.max(120);
        let param = format!("{code},day,,,{upstream_limit},qfq");
        let response = http
            .get(format!("{}/appstock/app/fqkline/get", self.base_url))
            .query(&[("param", param.as_str())])
            .send()
            .await
            .map_err(|e| PickError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(PickError::Network(format!("status {}", response.status())));
        }
        let json: Value = response
            .json()
            .await
            .map_err(|e| PickError::Parse(e.to_string()))?;
        crate::service::quote::history::parse_days(&json, code)
            .map_err(|e| PickError::Parse(e.to_string()))
    }
}

/// 东财涨幅榜（沪深 A，剔除 ST / 退市 / 次新）。
#[derive(Debug, Clone)]
pub struct EastMoneyRanking {
    /// 测试可覆写（默认 `https://push2.eastmoney.com`）
    pub base_url: String,
}

impl Default for EastMoneyRanking {
    fn default() -> Self {
        Self {
            base_url: "https://push2delay.eastmoney.com".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub code: String,
    pub name: String,
    pub price: f64,
    pub pct: f64,
    pub industry: String,
}

impl EastMoneyRanking {
    /// 拉涨幅榜前 `limit` 名（`fltt=2` 数值型字段）。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        limit: usize,
    ) -> Result<Vec<Candidate>, PickError> {
        let response = http
            .get(format!("{}/api/qt/clist/get", self.base_url))
            .query(&[
                ("pn", "1"),
                ("pz", &limit.to_string()),
                ("po", "1"),
                ("np", "1"),
                ("fltt", "2"),
                ("invt", "2"),
                ("fid", "f3"),
                ("fs", "m:0+t:6,m:0+t:80,m:1+t:2,m:1+t:23"),
                ("fields", "f2,f3,f12,f14,f100"),
            ])
            .send()
            .await
            .map_err(|e| PickError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(PickError::Network(format!("status {}", response.status())));
        }
        let json: Value = response
            .json()
            .await
            .map_err(|e| PickError::Parse(e.to_string()))?;
        let rows = json["data"]["diff"]
            .as_array()
            .ok_or(PickError::Parse("missing data.diff".into()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(code) = row["f12"].as_str() else {
                continue;
            };
            let Some(name) = row["f14"].as_str() else {
                continue;
            };
            // 剔除 ST / 退市 / 上市首日（N/C 前缀），一字板后续量化层再拦
            if name.contains("ST")
                || name.contains("退")
                || name.starts_with('N')
                || name.starts_with('C')
            {
                continue;
            }
            let price = row["f2"].as_f64().unwrap_or(0.0);
            let pct = row["f3"].as_f64().unwrap_or(0.0);
            if price <= 0.0 || price > 2000.0 {
                continue;
            }
            out.push(Candidate {
                code: format!("{}{}", market_prefix(code), code),
                name: name.to_string(),
                price,
                pct,
                industry: row["f100"].as_str().unwrap_or("").to_string(),
            });
        }
        Ok(out)
    }
}

/// 东财 f12 是 6 位裸码：6 开头沪、其余深。
fn market_prefix(raw: &str) -> &'static str {
    if raw.starts_with('6') {
        "sh"
    } else {
        "sz"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PickError {
    #[error("network: {0}")]
    Network(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("ai: {0}")]
    Ai(String),
}

impl From<PickError> for AppError {
    fn from(value: PickError) -> Self {
        match value {
            PickError::Network(m) => AppError::UpstreamExhausted {
                tries: 1,
                source: m.into(),
            },
            PickError::Parse(m) => AppError::UpstreamParse(m.into()),
            PickError::Ai(m) => AppError::AiUpstream(m.into()),
        }
    }
}

impl From<sqlx::Error> for PickError {
    fn from(value: sqlx::Error) -> Self {
        PickError::Parse(format!("db: {value}"))
    }
}

// ============================================================================
// ② 量化粗筛：纯函数
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct TechScore {
    pub score: f64,
    pub tags: Vec<String>,
}

fn ema_series(values: &[f64], period: usize) -> Vec<f64> {
    let k = 2.0 / (period as f64 + 1.0);
    let mut out = Vec::with_capacity(values.len());
    let mut prev = values.first().copied().unwrap_or(0.0);
    for (i, &v) in values.iter().enumerate() {
        let cur = if i == 0 { v } else { v * k + prev * (1.0 - k) };
        out.push(cur);
        prev = cur;
    }
    out
}

/// (DIF, DEA) 序列：EMA12 − EMA26 与其 EMA9。
fn macd_series(closes: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let ema12 = ema_series(closes, 12);
    let ema26 = ema_series(closes, 26);
    let dif: Vec<f64> = ema12.iter().zip(&ema26).map(|(a, b)| a - b).collect();
    let dea = ema_series(&dif, 9);
    (dif, dea)
}

fn rsi_last(closes: &[f64], period: usize) -> f64 {
    if closes.len() < period + 1 {
        return 50.0;
    }
    let mut gain = 0.0;
    let mut loss = 0.0;
    let window = &closes[closes.len() - period - 1..];
    for pair in window.windows(2) {
        let delta = pair[1] - pair[0];
        if delta >= 0.0 {
            gain += delta
        } else {
            loss -= delta
        }
    }
    if loss == 0.0 {
        return 100.0;
    }
    let rs = gain / loss;
    100.0 - 100.0 / (1.0 + rs)
}

fn ma_last(values: &[f64], period: usize) -> Option<f64> {
    if values.len() < period {
        return None;
    }
    Some(values[values.len() - period..].iter().sum::<f64>() / period as f64)
}

/// 技术面打分（0..90）。`None` = 硬性淘汰（数据不足 / 一字板 / 超买）。
/// bars: (date, open, close, high, low, volume) 中的 (date, close, high, low, volume)。
pub fn score_bars(
    closes: &[f64],
    highs: &[f64],
    lows: &[f64],
    volumes: &[f64],
) -> Option<TechScore> {
    let n = closes.len();
    if n < 30 {
        return None;
    }
    let close = *closes.last()?;
    let prev = closes[n - 2];
    if close <= 0.0 || prev <= 0.0 {
        return None;
    }
    let pct = (close - prev) / prev * 100.0;
    // 一字涨停买不进：全天一个价且涨幅贴近板
    if highs[n - 1] == lows[n - 1] && pct >= 9.0 {
        return None;
    }
    let rsi = rsi_last(closes, 14);
    if rsi > 75.0 {
        return None;
    }

    let mut score = 0.0;
    let mut tags = Vec::new();

    let (dif, dea) = macd_series(closes);
    let len = dif.len();
    // 近 3 根内 DIF 上穿 DEA = 金叉；否则多头排列给基础分
    let mut golden = false;
    for i in len.saturating_sub(3)..len {
        if i == 0 {
            continue;
        }
        if dif[i - 1] <= dea[i - 1] && dif[i] > dea[i] {
            golden = true;
            break;
        }
    }
    if golden {
        score += 30.0;
        tags.push("MACD金叉".into());
    } else if dif[len - 1] > dea[len - 1] {
        score += 10.0;
        tags.push("MACD多头".into());
    }

    if (45.0..=65.0).contains(&rsi) {
        score += 15.0;
        tags.push("RSI健康".into());
    }

    // 量比：今日 / 前 5 日均量
    if n >= 6 {
        let avg5: f64 = volumes[n - 6..n - 1].iter().sum::<f64>() / 5.0;
        if avg5 > 0.0 && volumes[n - 1] / avg5 >= 1.5 {
            score += 20.0;
            tags.push("放量".into());
        }
    }

    // 20 日箱体位置：强势区 + 近端突破
    let min20 = lows[n - 20..].iter().cloned().fold(f64::MAX, f64::min);
    let max20 = highs[n - 20..].iter().cloned().fold(f64::MIN, f64::max);
    if max20 > min20 {
        let pos = (close - min20) / (max20 - min20);
        if pos >= 0.6 {
            score += 15.0;
            tags.push("箱体强势".into());
        }
        if (max20 - close) / max20 <= 0.05 {
            score += 10.0;
            tags.push("近20日高".into());
        }
    }

    // 均线多头
    let ma5 = ma_last(closes, 5);
    let ma10 = ma_last(closes, 10);
    if let (Some(ma5), Some(ma10)) = (ma5, ma10) {
        if close > ma5 && ma5 > ma10 {
            score += 10.0;
            tags.push("均线多头".into());
        }
    }

    Some(TechScore { score, tags })
}

// ============================================================================
// ③④ 生成主流程
// ============================================================================

/// 生成某基准日推荐。`ai` 为客户端透传配置（None = 纯量化，调度器路径）。
/// 幂等：同日重复生成 DELETE + INSERT 全量刷新。
pub async fn generate_picks(
    state: &AppState,
    date: NaiveDate,
    ai_config: Option<&AiRankConfig>,
) -> Result<PicksDocument, PickError> {
    let candidates = state.pick_ranking.fetch(&state.http, 100).await?;
    if candidates.is_empty() {
        return Err(PickError::Parse("涨幅榜为空".into()));
    }

    // ② 逐候选拉日 K（顺带落库 day_bar，给回测与复盘复用），300ms 间隔防限流
    let mut scored: Vec<(Candidate, TechScore)> = Vec::new();
    for candidate in &candidates {
        match fetch_days_cached(state, &candidate.code, 120).await {
            Ok(bars) => {
                if let Some(tech) = score_bars(
                    &bars.iter().map(|b| b.close).collect::<Vec<_>>(),
                    &bars.iter().map(|b| b.high).collect::<Vec<_>>(),
                    &bars.iter().map(|b| b.low).collect::<Vec<_>>(),
                    &bars.iter().map(|b| b.volume as f64).collect::<Vec<_>>(),
                ) {
                    scored.push((candidate.clone(), tech));
                }
            }
            Err(error) => {
                tracing::debug!(code = %candidate.code, %error, "pick day fetch failed");
            }
        }
        tokio::time::sleep(StdDuration::from_millis(300)).await;
    }
    if scored.is_empty() {
        return Err(PickError::Parse("量化粗筛后无候选".into()));
    }
    scored.sort_by(|a, b| {
        b.1.score
            .partial_cmp(&a.1.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(20);

    // ③ 消息面加分：近 7 天研报评级 + 近 3 天新闻热度
    let since_reports = (date - Duration::days(7)).format("%Y-%m-%d").to_string();
    let since_news_ms = {
        use chrono::TimeZone;
        let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        date.and_hms_opt(0, 0, 0)
            .and_then(|dt| tz.from_local_datetime(&dt).single())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or_else(|| Utc::now().timestamp_millis())
    };
    let mut boosted: Vec<(Candidate, f64, Vec<String>)> = Vec::new();
    for (candidate, tech) in &scored {
        let mut score = tech.score;
        let mut tags = tech.tags.clone();
        let report_rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT rating_change, rating, last_rating FROM research_report \
             WHERE code = ? AND publish_date >= ?",
        )
        .bind(&candidate.code)
        .bind(&since_reports)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        let upgraded = report_rows.iter().any(|(change, rating, last)| {
            *change == 1 || (rating_starts_buy(rating) && last.is_empty())
        });
        let downgraded = report_rows.iter().any(|(change, _, _)| *change == 2);
        if upgraded {
            score += 25.0;
            tags.push("研报看多".into());
        }
        if downgraded {
            score -= 20.0;
        }
        let news_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM news_item WHERE code = ? AND published_at >= ?",
        )
        .bind(&candidate.code)
        .bind(since_news_ms - Duration::days(3).num_milliseconds())
        .fetch_one(&state.db)
        .await
        .unwrap_or(0);
        if news_count >= 3 {
            tags.push("新闻活跃".into());
        }
        score += (news_count.min(5)) as f64 * 2.0;
        boosted.push((candidate.clone(), score, tags));
    }
    boosted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    boosted.truncate(10);

    // ④ AI 精排：top10 喂给模型，出 top5 顺序；失败降级纯量化序
    let mut ai_order: Option<Vec<String>> = None;
    if let Some(config) = ai_config {
        match ai_rank(config, &boosted).await {
            Ok(codes) => ai_order = Some(codes),
            Err(error) => tracing::warn!(%error, "ai rank failed, fallback to quant order"),
        }
    }
    let mut picks: Vec<(Candidate, f64, Vec<String>)> = Vec::new();
    if let Some(codes) = &ai_order {
        for code in codes {
            if picks.len() >= 5 {
                break;
            }
            if let Some((candidate, score, tags)) = boosted.iter().find(|(c, _, _)| &c.code == code)
            {
                picks.push((candidate.clone(), *score, tags.clone()));
            }
        }
    }
    if picks.len() < 5 {
        for (candidate, score, tags) in &boosted {
            if picks.len() >= 5 {
                break;
            }
            if picks.iter().any(|(c, _, _)| c.code == candidate.code) {
                continue;
            }
            picks.push((candidate.clone(), *score, tags.clone()));
        }
    }
    picks.truncate(5);

    // 落库（同日全量刷新）
    let date_key = date.format("%Y-%m-%d").to_string();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM daily_pick WHERE date = ?")
        .bind(&date_key)
        .execute(&mut *tx)
        .await?;
    let now_ms = Utc::now().timestamp_millis();
    for (rank, (candidate, score, tags)) in picks.iter().enumerate() {
        let meta = serde_json::json!({
            "close": candidate.price,
            "pct": candidate.pct,
            "industry": candidate.industry,
        });
        sqlx::query(
            "INSERT INTO daily_pick(date, code, name, rank, score, reasons, ai_note, meta, created_at) \
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(&date_key)
        .bind(&candidate.code)
        .bind(&candidate.name)
        .bind((rank + 1) as i64)
        .bind(*score)
        .bind(serde_json::to_string(&tags).unwrap_or_else(|_| "[]".into()))
        .bind("")
        .bind(meta.to_string())
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    list_picks(&state.db, Some(&date_key)).await
}

fn rating_starts_buy(rating: &str) -> bool {
    matches!(
        rating.trim(),
        "买入" | "增持" | "强买" | "推荐" | "强烈推荐"
    )
}

/// 拉日 K（腾讯）并落库 day_bar。base_url 固定腾讯（与行情网关同源）。
async fn fetch_days_cached(
    state: &AppState,
    code: &str,
    limit: i64,
) -> Result<Vec<DayBar>, PickError> {
    let bars = state.day_k.fetch(&state.http, code, limit).await?;
    let mut tx = state.db.begin().await?;
    for bar in &bars {
        sqlx::query(
            "INSERT OR REPLACE INTO day_bar(code,date,open,high,low,close,volume,amount) VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(&bar.code)
        .bind(&bar.date)
        .bind(bar.open)
        .bind(bar.high)
        .bind(bar.low)
        .bind(bar.close)
        .bind(bar.volume)
        .bind(bar.amount)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(bars)
}

/// AI 精排：返回按 AI 意见排序的 code 列表（≤5，超出丢弃）。
async fn ai_rank(
    config: &AiRankConfig,
    candidates: &[(Candidate, f64, Vec<String>)],
) -> Result<Vec<String>, PickError> {
    let provider = ai::resolve(
        &config.provider,
        config.base_url.trim(),
        config.api_key.trim(),
    )
    .map_err(|e| PickError::Ai(e.to_string()))?;
    let mut lines = vec!["候选池（量化分 + 标签）：".to_string()];
    for (candidate, score, tags) in candidates {
        lines.push(format!(
            "- {} {} · 行业{} · 当日{:+.1}% · 分{:.0} · {}",
            candidate.code,
            candidate.name,
            candidate.industry,
            candidate.pct,
            score,
            tags.join("/")
        ));
    }
    lines.push("请从中挑 5 只未来一周胜率最高的，按把握排序。".into());
    lines.push("只输出 JSON：{\"picks\":[{\"code\":\"sh600xxx\",\"note\":\"一句话理由\"}]}，不要多余文字。".into());
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: "你是严谨的 A 股短线研究员，只基于给定数据判断，不给投资建议措辞。".into(),
        },
        ChatMessage {
            role: "user".into(),
            content: lines.join("\n"),
        },
    ];
    let request = ai::ChatRequest {
        model: &config.model,
        messages: &messages,
        temperature: 0.2,
        max_tokens: 600,
        enable_thinking: None,
    };
    let response = provider
        .chat(request)
        .await
        .map_err(|e| PickError::Ai(e.to_string()))?;
    parse_ai_picks(&response.text)
}

fn parse_ai_picks(text: &str) -> Result<Vec<String>, PickError> {
    let start = text
        .find('{')
        .ok_or(PickError::Parse("ai 输出无 JSON".into()))?;
    let end = text
        .rfind('}')
        .ok_or(PickError::Parse("ai 输出无 JSON 结尾".into()))?;
    let json = &text[start..=end];
    let parsed =
        serde_json::from_str::<AiPicks>(json).map_err(|e| PickError::Parse(e.to_string()))?;
    let codes = parsed
        .picks
        .iter()
        .filter(|p| p.code.len() >= 8)
        .map(|p| p.code.clone())
        .collect::<Vec<_>>();
    if codes.is_empty() {
        return Err(PickError::Parse("ai 未给出 code".into()));
    }
    Ok(codes)
}

#[derive(Deserialize)]
struct AiPicks {
    picks: Vec<AiPick>,
}

#[derive(Deserialize)]
struct AiPick {
    code: String,
    #[serde(default)]
    #[allow(dead_code)]
    note: String,
}

/// 读某日推荐 + 统计。`date = None` 取库中最近一天。
pub async fn list_picks(db: &SqlitePool, date: Option<&str>) -> Result<PicksDocument, PickError> {
    let date_key = match date {
        Some(d) => d.to_string(),
        None => {
            let latest: Option<String> = sqlx::query_scalar("SELECT MAX(date) FROM daily_pick")
                .fetch_one(db)
                .await?;
            latest.unwrap_or_default()
        }
    };
    if date_key.is_empty() {
        return Ok(PicksDocument {
            date: date_key,
            picks: Vec::new(),
            stats: PickStats {
                samples: 0,
                t5_win_rate: 0.0,
                avg_t5_pct: 0.0,
            },
        });
    }
    let rows: Vec<(String, String, String, i64, f64, String, String, String)> = sqlx::query_as(
        "SELECT date, code, name, rank, score, reasons, ai_note, meta FROM daily_pick \
         WHERE date = ? ORDER BY rank ASC",
    )
    .bind(&date_key)
    .fetch_all(db)
    .await?;
    let picks = rows
        .into_iter()
        .map(
            |(date, code, name, rank, score, reasons, ai_note, meta)| DailyPick {
                date,
                code,
                name,
                rank,
                score,
                reasons: serde_json::from_str(&reasons).unwrap_or_default(),
                ai_note,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
            },
        )
        .collect();

    // 回测统计：近 30 天有 outcome 的样本
    let since = (Utc::now().date_naive() - Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let outcomes: Vec<String> = sqlx::query_scalar(
        "SELECT meta FROM daily_pick WHERE date >= ? AND json_extract(meta,'$.outcome.t5_pct') IS NOT NULL",
    )
    .bind(&since)
    .fetch_all(db)
    .await?;
    let mut t5_win = 0.0;
    let mut t5_sum = 0.0;
    let samples = outcomes.len() as i64;
    for meta in &outcomes {
        if let Ok(value) = serde_json::from_str::<Value>(meta) {
            if let Some(pct) = value["outcome"]["t5_pct"].as_f64() {
                t5_sum += pct;
                if pct > 0.0 {
                    t5_win += 1.0;
                }
            }
        }
    }
    Ok(PicksDocument {
        date: date_key,
        picks,
        stats: PickStats {
            samples,
            t5_win_rate: if samples > 0 {
                t5_win / samples as f64
            } else {
                0.0
            },
            avg_t5_pct: if samples > 0 {
                t5_sum / samples as f64
            } else {
                0.0
            },
        },
    })
}

/// 回测回写：推荐日 ≥ 7 个自然日前且 outcome 缺失的行，取推荐日后第 1 / 5 根
/// 日 K 收盘（数据不足则跳过，下次再试）。每次至多处理最近 30 天内的行。
pub async fn backfill_outcomes(state: &AppState) -> Result<usize, PickError> {
    let today = Utc::now().date_naive();
    let cutoff = (today - Duration::days(7)).format("%Y-%m-%d").to_string();
    let since = (today - Duration::days(30)).format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT date, code FROM daily_pick \
         WHERE date >= ? AND date <= ? AND json_extract(meta,'$.outcome.t5_pct') IS NULL \
         ORDER BY date DESC",
    )
    .bind(&since)
    .bind(&cutoff)
    .fetch_all(&state.db)
    .await?;
    let mut updated = 0usize;
    for (date, code) in &rows {
        let Ok(bars) = fetch_days_cached(state, code, 120).await else {
            continue;
        };
        let Some(index) = bars.iter().position(|b| b.date == *date) else {
            continue;
        };
        if index + 5 >= bars.len() || bars[index].close <= 0.0 {
            continue;
        }
        let base = bars[index].close;
        let t1 = (bars[index + 1].close - base) / base * 100.0;
        let t5 = (bars[index + 5].close - base) / base * 100.0;
        let meta: Value = sqlx::query_scalar("SELECT meta FROM daily_pick WHERE date=? AND code=?")
            .bind(date)
            .bind(code)
            .fetch_one(&state.db)
            .await
            .and_then(|m: String| Ok(serde_json::from_str(&m).unwrap_or_default()))
            .unwrap_or_default();
        let mut meta = meta;
        let outcome = serde_json::json!({ "t1_pct": t1, "t5_pct": t5 });
        if let Some(map) = meta.as_object_mut() {
            map.insert("outcome".into(), outcome);
        } else {
            meta = serde_json::json!({ "outcome": outcome });
        }
        sqlx::query("UPDATE daily_pick SET meta=? WHERE date=? AND code=?")
            .bind(meta.to_string())
            .bind(date)
            .bind(code)
            .execute(&state.db)
            .await?;
        updated += 1;
        tokio::time::sleep(StdDuration::from_millis(300)).await;
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closes_up() -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
        // 净上行 + 隔日回调：RSI 落在 45..65 健康区（恒定上行 RSI=100 会被超买过滤）
        let n = 40;
        // 隔日 +0.21 / −0.14：RS=1.5 → RSI≈60（健康区），净上行
        let closes: Vec<f64> = (0..n)
            .map(|i| 10.0 + 0.035 * i as f64 + if i % 2 == 0 { 0.105 } else { -0.070 })
            .collect();
        let highs: Vec<f64> = closes.iter().map(|c| c + 0.15).collect();
        let lows: Vec<f64> = closes.iter().map(|c| c - 0.15).collect();
        let volumes: Vec<f64> = (0..n)
            .map(|i| if i >= n - 2 { 300.0 } else { 100.0 })
            .collect();
        (closes, highs, lows, volumes)
    }

    #[test]
    fn macd_rsi_helpers() {
        let (closes, _, _, _) = closes_up();
        let (dif, dea) = macd_series(&closes);
        assert_eq!(dif.len(), closes.len());
        assert!(
            *dif.last().unwrap() > *dea.last().unwrap(),
            "单边上行 DIF > DEA"
        );
        assert!(rsi_last(&closes, 14) > 50.0 && rsi_last(&closes, 14) < 75.0);
        let expect_ma5 = closes[35..].iter().sum::<f64>() / 5.0;
        assert!((ma_last(&closes, 5).unwrap() - expect_ma5).abs() < 1e-9);
    }

    #[test]
    fn score_bars_filters_and_tags() {
        // 持续上行 + 尾部放量：高分，含放量标签
        let (closes, highs, lows, volumes) = closes_up();
        let tech = score_bars(&closes, &highs, &lows, &volumes).expect("上行序列应可评分");
        assert!(tech.score >= 55.0, "score = {}", tech.score);
        assert!(tech.tags.contains(&"放量".to_string()));
        assert!(
            tech.tags.contains(&"MACD多头".to_string())
                || tech.tags.contains(&"MACD金叉".to_string())
        );

        // 数据不足
        assert!(score_bars(&closes[..20], &highs[..20], &lows[..20], &volumes[..20]).is_none());

        // 一字板：全天同价 + 涨 9.5%
        let mut c2 = closes.clone();
        let last = c2.len() - 1;
        let prev = c2[last - 1];
        c2[last] = prev * 1.095;
        let mut h2 = highs.clone();
        let mut l2 = lows.clone();
        h2[last] = c2[last];
        l2[last] = c2[last];
        let v2 = volumes.clone();
        assert!(score_bars(&c2, &h2, &l2, &v2).is_none(), "一字板应淘汰");
    }

    #[test]
    fn parse_ai_picks_extracts_codes() {
        let text = "分析如下：\n{\"picks\":[{\"code\":\"sh600519\",\"note\":\"龙头\"},{\"code\":\"sz300623\"}]}";
        let codes = parse_ai_picks(text).unwrap();
        assert_eq!(codes, vec!["sh600519".to_string(), "sz300623".to_string()]);
        assert!(parse_ai_picks("没有 JSON").is_err());
    }

    #[test]
    fn market_prefix_maps_codes() {
        assert_eq!(market_prefix("600519"), "sh");
        assert_eq!(market_prefix("300623"), "sz");
    }
}
