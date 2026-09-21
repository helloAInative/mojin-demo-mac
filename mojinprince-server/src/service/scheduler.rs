//! 阶段 4 后台调度：
//! - `spawn_quote_scheduler`：交易时段轮询自选股并广播最新报价
//! - `generate_daily_report` / `generate_weekly_report`：按 `(kind, period_key)`
//!   幂等落库"收盘复盘 / 周报"，重复执行不会生成多份。
//!
//! 行情轮询 + 复盘生成放在同一文件只是为了方便集中理解；后续可拆目录。

use crate::error::AppError;
use crate::model::{Quote, ReviewContext, ScheduledReport, TicketSummary};
use crate::service::ingest;
use crate::state::AppState;
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Timelike, Utc, Weekday};
use std::time::Duration;

pub fn spawn_quote_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(error) = poll_watchlist(&state).await {
                tracing::warn!(%error, "quote scheduler tick failed");
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    })
}

async fn poll_watchlist(state: &AppState) -> anyhow::Result<()> {
    if !is_trading_session() {
        return Ok(());
    }
    let codes: Vec<String> = sqlx::query_scalar(
        "SELECT code FROM watchlist ORDER BY pinned DESC, added_at ASC LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;
    for code in codes {
        match state.failover.fetch_quote(&state.http, &code).await {
            Ok((quote, _)) => {
                persist_quote(state, &quote).await?;
                state.quote_hub.publish(quote);
            }
            Err(error) => tracing::debug!(%code, %error, "scheduled quote failed"),
        }
    }
    Ok(())
}

fn is_trading_session() -> bool {
    let offset = FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
    let now = Utc::now().with_timezone(&offset);
    is_trading_session_at(now)
}

fn is_trading_session_at(now: chrono::DateTime<FixedOffset>) -> bool {
    if matches!(now.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let minute = now.hour() * 60 + now.minute();
    (9 * 60 + 15..=11 * 60 + 30).contains(&minute) || (13 * 60..=15 * 60 + 5).contains(&minute)
}

pub async fn persist_quote(state: &AppState, quote: &Quote) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO quote(code,ts,name,price,prev,open,high,low,volume,amount,source) \
         VALUES(?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&quote.code)
    .bind(quote.ts.timestamp_millis())
    .bind(&quote.name)
    .bind(quote.price)
    .bind(quote.prev)
    .bind(quote.open)
    .bind(quote.high)
    .bind(quote.low)
    .bind(quote.volume)
    .bind(quote.amount)
    .bind(&quote.source)
    .execute(&state.db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn scheduler_only_runs_during_cn_market_windows() {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let monday_open = tz.with_ymd_and_hms(2026, 9, 21, 10, 0, 0).unwrap();
        let monday_lunch = tz.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let saturday = tz.with_ymd_and_hms(2026, 9, 26, 10, 0, 0).unwrap();
        assert!(is_trading_session_at(monday_open));
        assert!(!is_trading_session_at(monday_lunch));
        assert!(!is_trading_session_at(saturday));
    }
}

// ============================================================================
// 阶段 4：收盘复盘 / 周报（按 period_key 幂等）
// ============================================================================

/// A 股时区（Asia/Shanghai，UTC+8）。
const CN_OFFSET_SECS: i32 = 8 * 3600;

fn cn_tz() -> FixedOffset {
    FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset")
}

/// 解析"今天（Asia/Shanghai）"日期。
pub fn today_cn() -> NaiveDate {
    let offset = FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset");
    Utc::now().with_timezone(&offset).date_naive()
}

/// 计算本周一（Asia/Shanghai），用于 weekly period_key（ISO 周，周一为周首日）。
pub fn week_start_cn(today: NaiveDate) -> NaiveDate {
    let weekday = today.weekday();
    let days_from_monday = weekday.num_days_from_monday() as i64;
    today - chrono::Duration::days(days_from_monday)
}

/// 把 `(date, kind)` 映射到 `period_key`：
/// - daily  → `YYYY-MM-DD`
/// - weekly → `YYYY-Www`（ISO 周编号）
pub fn period_key_for(kind: &str, today: NaiveDate) -> String {
    match kind {
        "weekly" => {
            let iso = today.iso_week();
            format!("{}-W{:02}", iso.year(), iso.week())
        }
        _ => today.format("%Y-%m-%d").to_string(),
    }
}

/// 收盘后自动生成日报 / 周报（幂等，已存在则跳过）。
///
/// 独立 60s 心跳，避免与 3s 行情轮询共用循环时反复查库；自动生成的只是
/// 不含客户端 context 的基线版，Swift 端仍可手动 `POST /api/v1/reviews/run`
/// 用日记 / 委托刷新同一条记录。
pub fn spawn_report_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(error) = maybe_generate_reports(&state).await {
                tracing::warn!(%error, "report scheduler tick failed");
            }
            // ROI #12：收盘后顺带把最近交易日的数据归档到 data/archives/
            let offset = FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset");
            let now = Utc::now().with_timezone(&offset);
            if let Err(error) = maybe_dump_daily_archive(&state, now).await {
                tracing::warn!(%error, "daily archive dump failed");
            }
            // 智能推荐：收盘后自动生成纯量化版 + 回测回写（幂等）
            maybe_generate_picks(&state).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    })
}

/// 北京时间下"当前应存在"的报告 kind：工作日 15:05 后日报；周五 15:05 后与
/// 周六 / 周日周报（周末补生成，覆盖周五晚间服务器未开机的情况）。
fn report_kinds_due(now: chrono::DateTime<FixedOffset>) -> Vec<&'static str> {
    let after_close = now.hour() * 60 + now.minute() >= 15 * 60 + 5;
    match now.weekday() {
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu => {
            if after_close {
                vec!["daily"]
            } else {
                Vec::new()
            }
        }
        Weekday::Fri => {
            if after_close {
                vec!["daily", "weekly"]
            } else {
                Vec::new()
            }
        }
        Weekday::Sat | Weekday::Sun => vec!["weekly"],
    }
}

async fn maybe_generate_reports(state: &AppState) -> anyhow::Result<()> {
    let offset = FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset");
    let now = Utc::now().with_timezone(&offset);
    for kind in report_kinds_due(now) {
        ensure_report(state, kind, now.date_naive()).await?;
    }
    Ok(())
}

/// 若 `(kind, today)` 对应报告不存在则生成一份基线版（无客户端 context）。
pub async fn ensure_report(state: &AppState, kind: &str, today: NaiveDate) -> Result<(), AppError> {
    let period_key = period_key_for(kind, today);
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM scheduled_report WHERE kind=? AND period_key=?")
            .bind(kind)
            .bind(&period_key)
            .fetch_optional(&state.db)
            .await?;
    if exists.is_some() {
        return Ok(());
    }
    let context = ReviewContext::default();
    if kind == "weekly" {
        generate_weekly_report(state, &context).await?;
    } else {
        generate_daily_report(state, &context).await?;
    }
    tracing::info!(kind, %period_key, "auto-generated scheduled report");
    Ok(())
}

/// 生成当日复盘报告，按 `(daily, YYYY-MM-DD)` UPSERT 幂等。
pub async fn generate_daily_report(
    state: &AppState,
    context: &ReviewContext,
) -> Result<ScheduledReport, AppError> {
    let today = today_cn();
    let period_key = period_key_for("daily", today);
    let draft = build_report(state, "daily", &period_key, today, today, context).await?;
    upsert_report(state, &draft).await?;
    fetch_report(state, "daily", &period_key).await
}

/// 生成本周复盘报告，按 `(weekly, YYYY-Www)` UPSERT 幂等。
pub async fn generate_weekly_report(
    state: &AppState,
    context: &ReviewContext,
) -> Result<ScheduledReport, AppError> {
    let today = today_cn();
    let week_start = week_start_cn(today);
    let week_end = week_start + chrono::Duration::days(6);
    let period_key = period_key_for("weekly", today);
    let draft = build_report(state, "weekly", &period_key, week_start, week_end, context).await?;
    upsert_report(state, &draft).await?;
    fetch_report(state, "weekly", &period_key).await
}

async fn build_report(
    state: &AppState,
    kind: &str,
    period_key: &str,
    start: NaiveDate,
    end: NaiveDate,
    context: &ReviewContext,
) -> Result<ScheduledReport, AppError> {
    let start_ms = cn_date_start_ms(start);
    let end_ms = cn_date_end_ms(end);

    // 后端视角：拉该窗口内的信号 / AI 用量 / 命中回测
    let signals = sqlx::query_as::<_, (String, String, String, String, i64)>(
        "SELECT id, kind, code, title, at FROM signal_event \
         WHERE at >= ? AND at <= ? ORDER BY at ASC",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_all(&state.db)
    .await?;
    let ai_calls: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_usage WHERE at >= ? AND at <= ? AND success=1")
            .bind(start_ms)
            .bind(end_ms)
            .fetch_one(&state.db)
            .await?;
    let ai_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ai_usage WHERE at >= ? AND at <= ?")
            .bind(start_ms)
            .bind(end_ms)
            .fetch_one(&state.db)
            .await?;
    let tokens_in: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(tokens_in),0) FROM ai_usage WHERE at >= ? AND at <= ?",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;
    let tokens_out: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(tokens_out),0) FROM ai_usage WHERE at >= ? AND at <= ?",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;
    let cost_usd: f64 = sqlx::query_scalar(
        "SELECT CAST(COALESCE(SUM(cost),0) AS REAL) FROM ai_usage WHERE at >= ? AND at <= ?",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;
    let level_total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM signal_event \
         WHERE kind='level' AND at >= ? AND at <= ?",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;
    let level_hit: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM signal_event \
         WHERE kind='level' AND at >= ? AND at <= ? \
         AND json_extract(meta,'$.hit')='1'",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;

    // 失效归因（ROI #9，仅周报）：本周 level 信号按 day_bar 判定失败原因
    let level_attribution = if kind == "weekly" {
        Some(build_level_attribution(state, start_ms, end_ms, end).await?)
    } else {
        None
    };

    let title = match kind {
        "weekly" => format!("周报 · {period_key}"),
        _ => format!("收盘复盘 · {period_key}"),
    };

    let mut body = String::new();
    body.push_str(&format!("# {title}\n\n"));
    body.push_str(&format!(
        "区间：{} ~ {}\n\n",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d")
    ));

    // 后端视角摘要
    body.push_str("## 后端摘要\n");
    body.push_str(&format!("- 信号事件：{} 条\n", signals.len()));
    body.push_str(&format!(
        "- level 命中：{} / {}（{:.1}%）\n",
        level_hit,
        level_total,
        if level_total > 0 {
            level_hit as f64 / level_total as f64 * 100.0
        } else {
            0.0
        }
    ));
    body.push_str(&format!(
        "- AI 调用：{} / {}（成功/总数）\n",
        ai_calls, ai_total
    ));
    body.push_str(&format!(
        "- AI 用量：tokens_in={} tokens_out={} cost≈${:.4}\n\n",
        tokens_in, tokens_out, cost_usd
    ));

    // 失效归因（ROI #9）：让回测有说服力——失败的不是黑盒，给出每条的失败原因
    if let Some(attribution) = &level_attribution {
        body.push_str(&attribution.section);
        if !attribution.section.is_empty() {
            body.push('\n');
        }
    }

    // 后端信号时间线
    if !signals.is_empty() {
        body.push_str("## 信号时间线（后端落库）\n");
        for (id, kind_name, code, title_at, at_ms) in &signals {
            let at_str = chrono::DateTime::<Utc>::from_timestamp_millis(*at_ms)
                .map(|dt| {
                    dt.with_timezone(&FixedOffset::east_opt(CN_OFFSET_SECS).unwrap())
                        .format("%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "-".to_string());
            body.push_str(&format!(
                "- {at_str} [{kind_name}] {code} {title_at} （id={id}）\n"
            ));
        }
        body.push('\n');
    }

    // 客户端日记
    if let Some(diary) = context.diary.as_deref().filter(|d| !d.trim().is_empty()) {
        body.push_str("## 日记\n");
        body.push_str(diary.trim());
        body.push_str("\n\n");
    }

    // 客户端委托照抄
    if !context.tickets.is_empty() {
        body.push_str("## 半自动委托\n");
        let mut sorted: Vec<_> = context.tickets.iter().collect();
        sorted.sort_by_key(|t| t.at);
        for t in sorted {
            let at_str =
                t.at.with_timezone(&FixedOffset::east_opt(CN_OFFSET_SECS).unwrap())
                    .format("%m-%d %H:%M")
                    .to_string();
            let note = t
                .note
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!(" · {}", s))
                .unwrap_or_default();
            body.push_str(&format!(
                "- {at_str} {} [{}] {}{}\n",
                t.code, t.status, t.summary, note
            ));
        }
        body.push('\n');
    }

    // 止损止盈执行对照（ROI #2）：已成交卖出 vs 持仓止损/止盈价，复盘"该割没割 / 该止盈没止盈"
    let filled_sells: Vec<&TicketSummary> = context
        .tickets
        .iter()
        .filter(|t| t.status == "filled" && t.side.as_deref() == Some("sell"))
        .filter(|t| t.price.map(|p| p > 0.0).unwrap_or(false))
        .collect();
    if !filled_sells.is_empty() {
        let positions: Vec<(String, f64, f64)> = sqlx::query_as(
            "SELECT code, stop_loss, take_profit FROM position WHERE stop_loss > 0 OR take_profit > 0",
        )
        .fetch_all(&state.db)
        .await?;
        let mut lines: Vec<String> = Vec::new();
        for t in &filled_sells {
            let price = t.price.unwrap_or_default();
            let at_str =
                t.at.with_timezone(&FixedOffset::east_opt(CN_OFFSET_SECS).unwrap())
                    .format("%m-%d %H:%M")
                    .to_string();
            let pos = positions.iter().find(|(c, _, _)| *c == t.code);
            let (stop, take) = match pos {
                Some((_, stop, take)) => (*stop, *take),
                None => (0.0, 0.0),
            };
            let mut parts: Vec<String> = Vec::new();
            if stop > 0.0 {
                parts.push(format!(
                    "止损 {:.2}（{:+.2}，{:+.1}%）",
                    stop,
                    price - stop,
                    (price - stop) / stop * 100.0
                ));
            }
            if take > 0.0 {
                parts.push(format!(
                    "止盈 {:.2}（{:+.2}，{:+.1}%）",
                    take,
                    price - take,
                    (price - take) / take * 100.0
                ));
            }
            if parts.is_empty() {
                lines.push(format!(
                    "- {at_str} {} 卖出成交 {:.2} · 未设止损/止盈，纪律不完整",
                    t.code, price
                ));
            } else {
                lines.push(format!(
                    "- {at_str} {} 卖出成交 {:.2} vs {}",
                    t.code,
                    price,
                    parts.join("；")
                ));
            }
        }
        if !lines.is_empty() {
            body.push_str("## 止损止盈执行对照\n");
            for line in lines {
                body.push_str(&line);
                body.push('\n');
            }
            body.push('\n');
        }
    }

    // 客户端信号时间线（如果传了）
    if !context.signals.is_empty() {
        body.push_str("## 客户端信号\n");
        let mut sorted: Vec<_> = context.signals.iter().collect();
        sorted.sort_by_key(|s| s.at);
        for s in sorted {
            let at_str =
                s.at.with_timezone(&FixedOffset::east_opt(CN_OFFSET_SECS).unwrap())
                    .format("%m-%d %H:%M")
                    .to_string();
            let body_part = s.body.as_deref().unwrap_or("");
            let why_part = s
                .why
                .as_deref()
                .filter(|w| !w.is_empty())
                .map(|w| format!("（{}）", w))
                .unwrap_or_default();
            body.push_str(&format!(
                "- {at_str} [{}] {} {} {}{}\n",
                s.kind, s.code, s.title, body_part, why_part
            ));
        }
        body.push('\n');
    }

    let id = uuid_like(&period_key, kind);
    let now = Utc::now();
    let mut payload = serde_json::json!({
        "kind": kind,
        "period_key": period_key,
        "start": start.format("%Y-%m-%d").to_string(),
        "end": end.format("%Y-%m-%d").to_string(),
        "summary": {
            "signals_total": signals.len(),
            "level_hit": level_hit,
            "level_total": level_total,
            "ai_calls_success": ai_calls,
            "ai_calls_total": ai_total,
            "tokens_in": tokens_in,
            "tokens_out": tokens_out,
            "cost_usd": (cost_usd * 10_000.0).round() / 10_000.0,
        },
        "context": {
            "tickets": context.tickets,
            "diary": context.diary,
            "signals": context.signals,
            "focus_codes": context.focus_codes,
        },
        "signals": signals
            .into_iter()
            .map(|(id, kind_name, code, title_at, at)| {
                serde_json::json!({
                    "id": id,
                    "kind": kind_name,
                    "code": code,
                    "title": title_at,
                    "at": at,
                })
            })
            .collect::<Vec<_>>(),
    });
    if let Some(attribution) = &level_attribution {
        if let Some(summary) = payload.get_mut("summary").and_then(|s| s.as_object_mut()) {
            summary.insert(
                "level_crossed_unconfirmed".into(),
                attribution.crossed_unconfirmed.into(),
            );
            summary.insert(
                "level_never_reached".into(),
                attribution.never_reached.into(),
            );
            summary.insert("level_window_open".into(), attribution.window_open.into());
            summary.insert("level_no_bars".into(), attribution.no_bars.into());
            summary.insert("ai_feedback_ignored".into(), attribution.ignored.into());
        }
    }

    Ok(ScheduledReport {
        id,
        kind: kind.into(),
        period_key: period_key.into(),
        title,
        body: body.trim_end().to_string(),
        payload,
        created_at: now,
    })
}

/// 幂等写入：同 `(kind, period_key)` 第二次执行刷新 `body` / `payload` / `title`，
/// 但保留原始 `created_at`，便于客户端识别"首次生成 vs 刷新"。
async fn upsert_report(state: &AppState, report: &ScheduledReport) -> Result<(), AppError> {
    let payload_text = serde_json::to_string(&report.payload).unwrap_or_else(|_| "{}".into());
    let now_ms = Utc::now().timestamp_millis();
    // 用 SQLite 的 `ON CONFLICT` 一步完成：
    //   - 首次插入：用 `now_ms` 作为 created_at
    //   - 已存在：不覆盖 created_at，只刷新 title / body / payload
    sqlx::query(
        "INSERT INTO scheduled_report(id,kind,period_key,title,body,payload,created_at) \
         VALUES(?,?,?,?,?,?,?) \
         ON CONFLICT(kind,period_key) DO UPDATE SET \
            title=excluded.title, body=excluded.body, payload=excluded.payload",
    )
    .bind(&report.id)
    .bind(&report.kind)
    .bind(&report.period_key)
    .bind(&report.title)
    .bind(&report.body)
    .bind(&payload_text)
    .bind(now_ms)
    .execute(&state.db)
    .await
    .map_err(AppError::from)?;

    Ok(())
}

/// 从数据库读回最新报告（含真实 created_at）。`upsert_report` 后调用以拿到准确时间。
async fn fetch_report(
    state: &AppState,
    kind: &str,
    period_key: &str,
) -> Result<ScheduledReport, AppError> {
    let row: (String, String, String, String, String, i64) = sqlx::query_as(
        "SELECT id,kind,period_key,title,payload,created_at FROM scheduled_report \
         WHERE kind=? AND period_key=?",
    )
    .bind(&kind)
    .bind(&period_key)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::from)?;
    let (id, kind, period_key, title, payload_text, created_at_ms) = row;
    let body: String =
        sqlx::query_scalar("SELECT body FROM scheduled_report WHERE kind=? AND period_key=?")
            .bind(&kind)
            .bind(&period_key)
            .fetch_one(&state.db)
            .await
            .map_err(AppError::from)?;
    let payload: serde_json::Value =
        serde_json::from_str(&payload_text).unwrap_or_else(|_| serde_json::json!({}));
    Ok(ScheduledReport {
        id,
        kind,
        period_key,
        title,
        body,
        payload,
        created_at: chrono::DateTime::<Utc>::from_timestamp_millis(created_at_ms)
            .unwrap_or_else(Utc::now),
    })
}

fn cn_date_start_ms(date: NaiveDate) -> i64 {
    let offset = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
    let dt = date
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    dt.timestamp_millis()
}

fn cn_date_end_ms(date: NaiveDate) -> i64 {
    let offset = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
    let dt = date
        .and_hms_opt(23, 59, 59)
        .unwrap()
        .and_local_timezone(offset)
        .unwrap();
    dt.timestamp_millis()
}

// ============================================================================
// 失效归因（ROI #9）：周报里解释 level 预警为什么失败
// ============================================================================

#[derive(Debug, Clone)]
struct LevelSignalInfo {
    code: String,
    at_ms: i64,
    level_key: String,
    level_price: f64,
    hit: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum LevelOutcome {
    Hit,
    /// 曾穿越价位（当日高/低触及）但 6h 内未确认——多为假突破扫损或客户端没盯到
    CrossedUnconfirmed {
        date: String,
        cross_price: f64,
    },
    /// 从未到价：给出最近偏离度与已过天数（价位设得太远的证据）
    NeverReached {
        days: i64,
        nearest_pct: f64,
    },
    /// 6h 判定窗口未满（周五尾盘发射的信号）
    WindowOpen,
    /// 无日线数据（网关没拉过该 code 的日 K）
    NoBars,
}

#[derive(Debug, Clone)]
struct DayBarLite {
    date: String,
    high: f64,
    low: f64,
}

struct LevelAttribution {
    section: String,
    crossed_unconfirmed: usize,
    never_reached: usize,
    window_open: usize,
    no_bars: usize,
    ignored: i64,
}

/// 单条信号的归因（纯函数，`now` / `today` 参数化便于测试）。
fn attribute_level(
    signal: &LevelSignalInfo,
    bars: &[DayBarLite],
    now_ms: i64,
    today: NaiveDate,
) -> LevelOutcome {
    if signal.hit {
        return LevelOutcome::Hit;
    }
    // 客户端命中回写窗口是发射后 6h；窗口未满不归因
    if signal.at_ms + 6 * 3600 * 1000 > now_ms {
        return LevelOutcome::WindowOpen;
    }
    if signal.level_price <= 0.0 {
        return LevelOutcome::NoBars;
    }
    let fired = DateTime::<Utc>::from_timestamp_millis(signal.at_ms)
        .map(|dt| dt.with_timezone(&cn_tz()).date_naive());
    let relevant: Vec<&DayBarLite> = bars
        .iter()
        .filter(|b| {
            NaiveDate::parse_from_str(&b.date, "%Y-%m-%d")
                .map(|d| fired.map_or(false, |f| d >= f) && d <= today)
                .unwrap_or(false)
        })
        .collect();
    if relevant.is_empty() {
        return LevelOutcome::NoBars;
    }
    for bar in &relevant {
        let crossed = match signal.level_key.as_str() {
            "support" => bar.low > 0.0 && bar.low <= signal.level_price,
            "resistance" => bar.high >= signal.level_price,
            _ => {
                (bar.high >= signal.level_price) || (bar.low > 0.0 && bar.low <= signal.level_price)
            }
        };
        if crossed {
            return LevelOutcome::CrossedUnconfirmed {
                date: bar.date.clone(),
                cross_price: if signal.level_key == "support" {
                    bar.low
                } else {
                    bar.high
                },
            };
        }
    }
    let mut nearest = f64::MAX;
    for bar in &relevant {
        for price in [bar.high, bar.low] {
            if price > 0.0 {
                nearest =
                    nearest.min((price - signal.level_price).abs() / signal.level_price * 100.0);
            }
        }
    }
    let days = fired.map(|f| (today - f).num_days().max(0)).unwrap_or(0);
    LevelOutcome::NeverReached {
        days,
        nearest_pct: if nearest == f64::MAX { 100.0 } else { nearest },
    }
}

fn level_key_label(key: &str) -> &'static str {
    match key {
        "support" => "支撑",
        "resistance" => "阻力",
        _ => "基准",
    }
}

async fn build_level_attribution(
    state: &AppState,
    start_ms: i64,
    end_ms: i64,
    today: NaiveDate,
) -> Result<LevelAttribution, AppError> {
    let rows: Vec<(String, String, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, code, at, meta FROM signal_event \
         WHERE kind='level' AND at >= ? AND at <= ? ORDER BY at ASC",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_all(&state.db)
    .await?;
    let now_ms = Utc::now().timestamp_millis();

    let mut infos: Vec<LevelSignalInfo> = Vec::with_capacity(rows.len());
    for (id, code, at_ms, meta) in &rows {
        let meta: serde_json::Value =
            serde_json::from_str(meta.as_deref().unwrap_or("{}")).unwrap_or_default();
        let level_price = meta
            .get("levelPrice")
            .and_then(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            .or_else(|| meta.get("levelPrice").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);
        let _ = &id;
        infos.push(LevelSignalInfo {
            code: code.clone(),
            at_ms: *at_ms,
            level_key: meta
                .get("levelKey")
                .and_then(|v| v.as_str())
                .unwrap_or("base")
                .to_string(),
            level_price,
            hit: meta.get("hit").and_then(|v| v.as_str()) == Some("1")
                || meta.get("hit").and_then(|v| v.as_i64()) == Some(1),
        });
    }

    // 每个 code 一次性拉日 K（date >= start 提前一周余量，归因时按发射日过滤）
    let mut bars_by_code: std::collections::HashMap<String, Vec<DayBarLite>> =
        std::collections::HashMap::new();
    for code in infos
        .iter()
        .map(|i| i.code.clone())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let bars: Vec<(String, f64, f64)> =
            sqlx::query_as("SELECT date, high, low FROM day_bar WHERE code = ? ORDER BY date ASC")
                .bind(&code)
                .fetch_all(&state.db)
                .await?;
        bars_by_code.insert(
            code,
            bars.into_iter()
                .map(|(date, high, low)| DayBarLite { date, high, low })
                .collect(),
        );
    }

    let mut hit = 0usize;
    let mut crossed = 0usize;
    let mut crossed_lines: Vec<String> = Vec::new();
    let mut never = 0usize;
    let mut never_lines: Vec<(f64, String)> = Vec::new();
    let mut window_open = 0usize;
    let mut no_bars = 0usize;
    for info in &infos {
        let bars = bars_by_code.get(&info.code).cloned().unwrap_or_default();
        match attribute_level(info, &bars, now_ms, today) {
            LevelOutcome::Hit => hit += 1,
            LevelOutcome::WindowOpen => window_open += 1,
            LevelOutcome::NoBars => no_bars += 1,
            LevelOutcome::CrossedUnconfirmed { date, cross_price } => {
                crossed += 1;
                crossed_lines.push(format!(
                    "- {} {} {} {:.2}（{} 日 {} 触及后未确认）",
                    date,
                    info.code,
                    level_key_label(&info.level_key),
                    info.level_price,
                    if info.level_key == "support" {
                        "低"
                    } else {
                        "高"
                    },
                    format!("{cross_price:.2}")
                ));
            }
            LevelOutcome::NeverReached { days, nearest_pct } => {
                never += 1;
                never_lines.push((
                    nearest_pct,
                    format!(
                        "- {} {} {} {:.2} · 最近偏离 {:.1}% · 已 {} 天",
                        ms_cn_date(info.at_ms),
                        info.code,
                        level_key_label(&info.level_key),
                        info.level_price,
                        nearest_pct,
                        days
                    ),
                ));
            }
        }
    }

    let ignored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ai_feedback WHERE sentiment='ignore' AND at >= ? AND at <= ?",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_one(&state.db)
    .await?;

    let mut section = String::new();
    if !infos.is_empty() {
        section.push_str("## 失效归因（level 预警 · 本周）\n");
        section.push_str(&format!(
            "- 本周 level 信号 {} 条：命中 {}、穿越未确认 {}、未到价 {}、窗口未满 {}、无日线 {}\n",
            infos.len(),
            hit,
            crossed,
            never,
            window_open,
            no_bars
        ));
        if !crossed_lines.is_empty() {
            section.push_str("- 穿越未确认（曾到价但 6h 内未确认，多为假突破扫损）:\n");
            for line in crossed_lines.iter().take(5) {
                section.push_str(line);
                section.push('\n');
            }
        }
        if !never_lines.is_empty() {
            section.push_str("- 未到价 TOP3（按最近偏离度，价位可能设得太远）:\n");
            never_lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            for (_, line) in never_lines.iter().take(3) {
                section.push_str(line);
                section.push('\n');
            }
        }
        if ignored > 0 {
            section.push_str(&format!(
                "- 被忽略反馈：{} 条（AI 结论被标记 ignore，考虑收紧阈值）\n",
                ignored
            ));
        }
    }

    Ok(LevelAttribution {
        section,
        crossed_unconfirmed: crossed,
        never_reached: never,
        window_open,
        no_bars,
        ignored,
    })
}

/// 毫秒时间戳 → 北京时间 MM-dd。
fn ms_cn_date(ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|dt| dt.with_timezone(&cn_tz()).format("%m-%d").to_string())
        .unwrap_or_else(|| "--".into())
}

// ============================================================================
// 每日数据归档（ROI #12）：收盘后把最近交易日的全量快照 dump 成 JSON 文件
// ============================================================================

/// 归档目录跟着数据库走：dev 在 `data/archives/`，常驻安装在 Application Support。
fn archives_dir(state: &AppState) -> std::path::PathBuf {
    let raw = state
        .cfg
        .database_url
        .trim_start_matches("sqlite://")
        .trim_start_matches("sqlite:");
    let parent = std::path::Path::new(raw)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    parent.join("archives")
}

/// 最近**已收盘**的交易日：工作日 15:10 后是今天；否则回退到上一个工作日
/// （周末 / 节前不拦，覆盖"周五晚间没开机"的补归档；无节假日感知）。
pub fn last_closed_trading_day(now: chrono::DateTime<FixedOffset>) -> NaiveDate {
    let after_close = now.hour() * 60 + now.minute() >= 15 * 60 + 10;
    let mut day = now.date_naive();
    let today_is_trading = !matches!(day.weekday(), Weekday::Sat | Weekday::Sun);
    if !(after_close && today_is_trading) {
        day = day.pred_opt().unwrap_or(day);
    }
    while matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
        day = day.pred_opt().unwrap_or(day);
    }
    day
}

/// 为最近已收盘交易日写 `archives/{date}.json`：文件已存在即跳过——同一天幂等，
/// 且周五晚间没开机时周六 / 周日 / 周一自动补。
pub async fn maybe_dump_daily_archive(
    state: &AppState,
    now: chrono::DateTime<FixedOffset>,
) -> anyhow::Result<bool> {
    let date = last_closed_trading_day(now);
    let dir = archives_dir(state);
    let file = dir.join(format!("{}.json", date.format("%Y-%m-%d")));
    if file.exists() {
        return Ok(false);
    }
    let archive = build_day_archive(state, date).await?;
    let text = serde_json::to_string_pretty(&archive)?;
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(&file, text)?;
    tracing::info!(date = %date.format("%Y-%m-%d"), "daily archive dumped");
    Ok(true)
}

/// 组装某交易日的归档：自选股分时（minute_bar）+ 当日收盘快照（quote 表最后一条）
/// + 当日信号 + AI 用量汇总 + 持仓快照（dump 时刻状态，历史日无法回溯持仓）。
pub(crate) async fn build_day_archive(
    state: &AppState,
    date: NaiveDate,
) -> Result<serde_json::Value, AppError> {
    let start_ms = cn_date_start_ms(date);
    let end_ms = cn_date_end_ms(date);
    let date_key = date.format("%Y-%m-%d").to_string();

    let watch: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT code, name FROM watchlist ORDER BY pinned DESC, added_at ASC")
            .fetch_all(&state.db)
            .await?;

    let mut codes = serde_json::Map::new();
    for (code, name) in &watch {
        // 当日分时（分钟粒度即可，逐笔回放是 §B.4 的事）
        // volume 是 INTEGER 列，按 i64 解码再转 JSON
        let minutes: Vec<(i64, f64, f64, i64)> = sqlx::query_as(
            "SELECT ts, price, avg_price, volume FROM minute_bar \
             WHERE code = ? AND ts >= ? AND ts <= ? ORDER BY ts ASC",
        )
        .bind(code)
        .bind(start_ms)
        .bind(end_ms)
        .fetch_all(&state.db)
        .await?;
        let quote: Option<(f64, f64, f64, f64, f64)> = sqlx::query_as(
            "SELECT price, prev, open, high, low FROM quote \
             WHERE code = ? AND ts >= ? AND ts <= ? ORDER BY ts DESC LIMIT 1",
        )
        .bind(code)
        .bind(start_ms)
        .bind(end_ms)
        .fetch_optional(&state.db)
        .await?;
        let quote_json = match quote {
            Some((price, prev, open, high, low)) => serde_json::json!({
                "close": price, "prev": prev, "open": open, "high": high, "low": low,
            }),
            None => serde_json::Value::Null,
        };
        codes.insert(
            code.clone(),
            serde_json::json!({
                "name": name,
                "quote": quote_json,
                // [ts(ms), price, avg, volume] 紧凑数组省体积
                "minutes": minutes
                    .into_iter()
                    .map(|(ts, price, avg, volume)| serde_json::json!([ts, price, avg, volume]))
                    .collect::<Vec<_>>(),
            }),
        );
    }

    let signals: Vec<serde_json::Value> = sqlx::query_as::<_, (String, i64, String, String, String)>(
        "SELECT id, at, kind, code, title FROM signal_event \
         WHERE at >= ? AND at <= ? ORDER BY at ASC",
    )
    .bind(start_ms)
    .bind(end_ms)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(id, at, kind, code, title)| {
        serde_json::json!({ "id": id, "at": at, "kind": kind, "code": code, "title": title })
    })
    .collect();

    let (ai_total, ai_success, tokens_in, tokens_out, cost_usd): (i64, i64, i64, i64, f64) =
        sqlx::query_as(
            "SELECT COUNT(*), COALESCE(SUM(success),0), COALESCE(SUM(tokens_in),0), \
             COALESCE(SUM(tokens_out),0), CAST(COALESCE(SUM(cost),0) AS REAL) \
             FROM ai_usage WHERE at >= ? AND at <= ?",
        )
        .bind(start_ms)
        .bind(end_ms)
        .fetch_one(&state.db)
        .await?;

    let positions: Vec<serde_json::Value> = sqlx::query_as::<_, (String, f64, f64, f64, f64, f64)>(
        "SELECT code, cost, shares, stop_loss, take_profit, position_pct FROM position",
    )
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(
        |(code, cost, shares, stop_loss, take_profit, position_pct)| {
            serde_json::json!({
                "code": code, "cost": cost, "shares": shares,
                "stop_loss": stop_loss, "take_profit": take_profit, "position_pct": position_pct,
            })
        },
    )
    .collect();

    Ok(serde_json::json!({
        "date": date_key,
        "generated_at": Utc::now().to_rfc3339(),
        "ai_usage": {
            "total": ai_total, "success": ai_success,
            "tokens_in": tokens_in, "tokens_out": tokens_out, "cost_usd": cost_usd,
        },
        "positions": positions,
        "signals": signals,
        "codes": codes,
    }))
}

// ============================================================================
// 智能推荐调度：收盘后自动生成纯量化版 + 回测回写
// ============================================================================

/// 推荐基准日：工作日盘中（09:35 后）或收盘后为今天（当日榜单成立）；
/// 其余（开盘前 / 周末）为最近已收盘交易日（此时上游榜单即该日终盘）。
pub fn pick_target_date(now: chrono::DateTime<FixedOffset>) -> NaiveDate {
    let today = now.date_naive();
    let after_open = now.hour() * 60 + now.minute() >= 9 * 60 + 35;
    let is_weekday = !matches!(today.weekday(), Weekday::Sat | Weekday::Sun);
    if is_weekday && after_open {
        today
    } else {
        last_closed_trading_day(now)
    }
}

/// 工作日 15:30 后（榜单已定型）生成当日纯量化推荐；周末 / 节前自动补上一
/// 交易日。已存在则跳过。随后顺带回写 T+5 回测。
async fn maybe_generate_picks(state: &AppState) {
    let offset = FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset");
    let now = Utc::now().with_timezone(&offset);
    let date = pick_target_date(now);
    let today_final = date == now.date_naive() && now.hour() * 60 + now.minute() >= 15 * 60 + 30;
    let catch_up = date != now.date_naive();
    if today_final || catch_up {
        let date_key = date.format("%Y-%m-%d").to_string();
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM daily_pick WHERE date = ? LIMIT 1")
                .bind(&date_key)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        if exists.is_none() {
            match crate::service::pick::generate_picks(state, date, None).await {
                Ok(doc) => tracing::info!(
                    date = %date_key,
                    count = doc.picks.len(),
                    "daily picks generated (quant)"
                ),
                Err(error) => tracing::warn!(%error, "daily picks generation failed"),
            }
        }
    }
    match crate::service::pick::backfill_outcomes(state).await {
        Ok(updated) if updated > 0 => {
            tracing::info!(updated, "pick outcomes backfilled")
        }
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "pick outcome backfill failed"),
    }
}

// ============================================================================
// §F.3–F.4 每日增量拉取：自选股新闻 / 研报 / 概念板块
// ============================================================================

/// 收盘后为自选股增量刷新新闻 / 研报 / 板块（每 5 分钟心跳，工作日 16:00 后只跑一次）。
pub fn spawn_ingest_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_run: Option<NaiveDate> = None;
        loop {
            let offset = FixedOffset::east_opt(CN_OFFSET_SECS).expect("valid UTC+8 offset");
            let now = Utc::now().with_timezone(&offset);
            if ingest_due(now, last_run) {
                match refresh_watchlist_ingest(&state).await {
                    Ok(count) => {
                        last_run = Some(now.date_naive());
                        tracing::info!(count, "ingest refresh done");
                    }
                    Err(error) => tracing::warn!(%error, "ingest refresh failed"),
                }
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    })
}

/// 工作日 16:00 之后（收盘 + 数据源更新完成）触发；同一天只跑一次。
fn ingest_due(now: chrono::DateTime<FixedOffset>, last_run: Option<NaiveDate>) -> bool {
    let after = now.hour() * 60 + now.minute() >= 16 * 60;
    let weekday_ok = matches!(
        now.weekday(),
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu | Weekday::Fri
    );
    after && weekday_ok && last_run != Some(now.date_naive())
}

/// 为自选股刷新新闻（近 72h 已由接口时间窗覆盖，这里取最新 20 条）、
/// 研报（近 90 天）与板块行情，逐标的落库；单标的失败不影响其余。
pub async fn refresh_watchlist_ingest(state: &AppState) -> anyhow::Result<usize> {
    let codes: Vec<String> = sqlx::query_scalar(
        "SELECT code FROM watchlist ORDER BY pinned DESC, added_at ASC LIMIT 60",
    )
    .fetch_all(&state.db)
    .await?;
    let mut refreshed = 0usize;
    for code in &codes {
        let mut ok = true;
        match state.news.fetch(&state.http, code, 20).await {
            Ok(items) => ingest::persist_news(&state.db, &items).await?,
            Err(error) => {
                ok = false;
                tracing::warn!(%code, %error, "scheduled news refresh failed");
            }
        }
        match state.reports.fetch(&state.http, code, 20, 90).await {
            Ok(items) => {
                ingest::persist_reports(&state.db, &items).await?;
                // §F.4 信号化：近 7 天且评级明确的研报 → kind=report 信号（幂等）。
                match ingest::persist_report_signals(&state.db, code, &items).await {
                    Ok(signals) if signals > 0 => {
                        tracing::info!(%code, signals, "report signals emitted")
                    }
                    Ok(_) => {}
                    Err(error) => {
                        ok = false;
                        tracing::warn!(%code, %error, "persist report signals failed")
                    }
                }
            }
            Err(error) => {
                ok = false;
                tracing::warn!(%code, %error, "scheduled report refresh failed");
            }
        }
        match state.sector.fetch(&state.http, code).await {
            Ok(boards) => ingest::persist_sector_boards(&state.db, &boards).await?,
            Err(error) => {
                ok = false;
                tracing::warn!(%code, %error, "scheduled sector refresh failed");
            }
        }
        if ok {
            refreshed += 1;
        }
        // 东财接口对高频调用不友好，逐标的之间留一点间隔
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(refreshed)
}

/// 用 `kind + period_key` 生成稳定 id（同一 (kind, period_key) 永远同一 id）。
fn uuid_like(period_key: &str, kind: &str) -> String {
    // 简单哈希；不加密学安全，但 SQLite 里 PRIMARY KEY 唯一即可。
    let mut hash: u64 = 14695981039346656037; // FNV offset basis
    for byte in period_key.as_bytes().iter().chain(kind.as_bytes()) {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn last_closed_trading_day_walks_back_until_closed() {
        let tz = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
        let at = |y, m, d, h, mi| tz.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap();
        // 周末任何时刻 → 上周五
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 20, 10, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap()
        );
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 19, 9, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap()
        );
        // 周一早上（今天没收盘）→ 上周五；周一收盘后 → 今天
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 21, 10, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 18).unwrap()
        );
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 21, 16, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
        );
        // 工作日盘中 → 昨天；收盘后 → 今天
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 22, 11, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
        );
        assert_eq!(
            last_closed_trading_day(at(2026, 9, 22, 16, 0)),
            NaiveDate::from_ymd_opt(2026, 9, 22).unwrap()
        );
    }

    #[test]
    fn attribute_level_classifies_all_outcomes() {
        let tz = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
        // 周五收盘后生成周报
        let now = tz.with_ymd_and_hms(2026, 9, 25, 16, 0, 0).unwrap();
        let now_ms = now.timestamp_millis();
        let today = now.date_naive();
        let base = LevelSignalInfo {
            code: "sh600460".into(),
            at_ms: now_ms - 3 * 86400 * 1000, // 周二发射
            level_key: "support".into(),
            level_price: 32.0,
            hit: false,
        };

        // 命中优先于一切
        let mut hit_sig = base.clone();
        hit_sig.hit = true;
        assert_eq!(
            attribute_level(&hit_sig, &[], now_ms, today),
            LevelOutcome::Hit
        );

        // 6h 窗口未满不归因
        let mut fresh = base.clone();
        fresh.at_ms = now_ms - 3600 * 1000;
        assert_eq!(
            attribute_level(&fresh, &[], now_ms, today),
            LevelOutcome::WindowOpen
        );

        // 支撑穿越未确认：发射后首个触及日（low 31.9 ≤ 32.0）
        let bars = vec![
            DayBarLite {
                date: "2026-09-23".into(),
                high: 33.0,
                low: 31.9,
            },
            DayBarLite {
                date: "2026-09-24".into(),
                high: 33.5,
                low: 32.4,
            },
        ];
        match attribute_level(&base, &bars, now_ms, today) {
            LevelOutcome::CrossedUnconfirmed { date, cross_price } => {
                assert_eq!(date, "2026-09-23");
                assert!((cross_price - 31.9).abs() < 1e-9);
            }
            other => panic!("expect crossed, got {other:?}"),
        }

        // 未到价：最近偏离 0.5/32 = 1.5625%，已过 3 天
        let far = vec![
            DayBarLite {
                date: "2026-09-23".into(),
                high: 33.0,
                low: 32.5,
            },
            DayBarLite {
                date: "2026-09-24".into(),
                high: 33.5,
                low: 32.6,
            },
        ];
        match attribute_level(&base, &far, now_ms, today) {
            LevelOutcome::NeverReached { days, nearest_pct } => {
                assert_eq!(days, 3);
                assert!((nearest_pct - 1.5625).abs() < 0.01);
            }
            other => panic!("expect never reached, got {other:?}"),
        }

        // 阻力穿越：high ≥ level（33.0 ≥ 32.5 不到位，33.5 达标 → 09-24）
        let mut res = base.clone();
        res.level_key = "resistance".into();
        res.level_price = 33.2;
        match attribute_level(&res, &bars, now_ms, today) {
            LevelOutcome::CrossedUnconfirmed { date, cross_price } => {
                assert_eq!(date, "2026-09-24");
                assert!((cross_price - 33.5).abs() < 1e-9);
            }
            other => panic!("expect crossed resistance, got {other:?}"),
        }

        // 无日线 / 发射日之前的 bar 不算
        assert_eq!(
            attribute_level(&base, &[], now_ms, today),
            LevelOutcome::NoBars
        );
        let early = vec![DayBarLite {
            date: "2026-09-19".into(),
            high: 33.0,
            low: 30.0,
        }];
        assert_eq!(
            attribute_level(&base, &early, now_ms, today),
            LevelOutcome::NoBars
        );

        // 未来日期的 bar 不算（today 之后）
        let future = vec![DayBarLite {
            date: "2026-09-26".into(),
            high: 33.0,
            low: 30.0,
        }];
        assert_eq!(
            attribute_level(&base, &future, now_ms, today),
            LevelOutcome::NoBars
        );
    }

    #[test]
    fn period_key_for_daily_uses_iso_date() {
        let d = NaiveDate::from_ymd_opt(2026, 9, 20).unwrap();
        assert_eq!(period_key_for("daily", d), "2026-09-20");
    }

    #[test]
    fn period_key_for_weekly_uses_iso_week() {
        // 2026-09-20 是周日，属于 ISO 周 "2026-W38"
        let d = NaiveDate::from_ymd_opt(2026, 9, 20).unwrap();
        assert_eq!(period_key_for("weekly", d), "2026-W38");
    }

    #[test]
    fn week_start_cn_returns_monday() {
        let sun = NaiveDate::from_ymd_opt(2026, 9, 20).unwrap();
        let mon = NaiveDate::from_ymd_opt(2026, 9, 14).unwrap();
        assert_eq!(week_start_cn(sun), mon);
        let wed = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
        assert_eq!(week_start_cn(wed), mon);
    }

    #[test]
    fn report_kinds_due_matches_close_and_weekend_windows() {
        let tz = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
        let monday_open = tz.with_ymd_and_hms(2026, 9, 21, 10, 0, 0).unwrap();
        let monday_close = tz.with_ymd_and_hms(2026, 9, 21, 15, 10, 0).unwrap();
        let friday_close = tz.with_ymd_and_hms(2026, 9, 25, 15, 6, 0).unwrap();
        let saturday = tz.with_ymd_and_hms(2026, 9, 26, 10, 0, 0).unwrap();
        let sunday = tz.with_ymd_and_hms(2026, 9, 27, 10, 0, 0).unwrap();
        assert!(report_kinds_due(monday_open).is_empty());
        assert_eq!(report_kinds_due(monday_close), vec!["daily"]);
        assert_eq!(report_kinds_due(friday_close), vec!["daily", "weekly"]);
        assert_eq!(report_kinds_due(saturday), vec!["weekly"]);
        assert_eq!(report_kinds_due(sunday), vec!["weekly"]);
    }

    #[test]
    fn uuid_like_is_stable_for_same_inputs() {
        let a = uuid_like("2026-09-20", "daily");
        let b = uuid_like("2026-09-20", "daily");
        assert_eq!(a, b);
        let c = uuid_like("2026-09-21", "daily");
        assert_ne!(a, c);
    }

    #[test]
    fn ingest_due_fires_once_after_close_on_weekdays() {
        let tz = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
        let friday_1630 = tz.with_ymd_and_hms(2026, 9, 25, 16, 30, 0).unwrap();
        assert!(ingest_due(friday_1630, None));
        assert!(!ingest_due(friday_1630, Some(friday_1630.date_naive())));
    }

    #[test]
    fn ingest_due_skips_before_close_and_weekends() {
        let tz = FixedOffset::east_opt(CN_OFFSET_SECS).unwrap();
        let friday_morning = tz.with_ymd_and_hms(2026, 9, 25, 10, 0, 0).unwrap();
        let saturday_1630 = tz.with_ymd_and_hms(2026, 9, 26, 16, 30, 0).unwrap();
        assert!(!ingest_due(friday_morning, None));
        assert!(!ingest_due(saturday_1630, None));
    }
}
