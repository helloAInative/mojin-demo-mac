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
use chrono::{Datelike, FixedOffset, NaiveDate, Timelike, Utc, Weekday};
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
            let at_str = t
                .at
                .with_timezone(&FixedOffset::east_opt(CN_OFFSET_SECS).unwrap())
                .format("%m-%d %H:%M")
                .to_string();
            let pos = positions.iter().find(|(c, _, _)| *c == t.code);
            let (stop, take) = match pos {
                Some((_, stop, take)) => (*stop, *take),
                None => (0.0, 0.0),
            };
            let mut parts: Vec<String> = Vec::new();
            if stop > 0.0 {
                parts.push(format!("止损 {:.2}（{:+.2}，{:+.1}%）", stop, price - stop, (price - stop) / stop * 100.0));
            }
            if take > 0.0 {
                parts.push(format!("止盈 {:.2}（{:+.2}，{:+.1}%）", take, price - take, (price - take) / take * 100.0));
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
    let payload = serde_json::json!({
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
