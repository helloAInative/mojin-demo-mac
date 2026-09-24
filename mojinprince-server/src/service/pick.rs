//! 智能推荐（四层漏斗）：
//! ① 东财涨幅榜 Top100 候选 → ② 技术指标量化粗筛（纯函数可单测）
//! → ③ 研报评级 / 新闻热度加分（查既有缓存表）→ ④ AI 精排（客户端
//! 透传配置触发，密钥不落库；调度器自动版为纯量化）。
//!
//! 落库 `daily_pick`，按 (date, code) 主键；T+1/T+5 收盘对照回写
//! `meta.outcome` 形成命中率闭环。
use crate::error::AppError;
use crate::model::pick::{AiRankConfig, DailyPick, PickStats, PickTagStat, PicksDocument};
use crate::model::DayBar;
use crate::service::ai::{self, ChatMessage};
use crate::service::ingest::MainNetSnapshot;
use crate::state::AppState;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::time::Duration as StdDuration;

/// 新浪 A 股涨幅榜（东财 push2 断连时候选池 fallback；同属既有三源）。
/// 无行业字段 → 板块动量因子自动降级（industry 为空不参与聚合）。
#[derive(Debug, Clone)]
pub struct SinaRanking {
    /// 测试可覆写（默认 `https://vip.stock.finance.sina.com.cn`）
    pub base_url: String,
}

impl Default for SinaRanking {
    fn default() -> Self {
        Self {
            base_url: "https://vip.stock.finance.sina.com.cn".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SinaRow {
    /// 已带 sh/sz/bj 前缀
    symbol: String,
    name: String,
    trade: Option<String>,
    changepercent: Option<f64>,
}

impl SinaRanking {
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        limit: usize,
    ) -> Result<Vec<Candidate>, PickError> {
        let limit_text = limit.to_string();
        let response = http
            .get(format!(
                "{}/quotes_service/api/json_v2.php/Market_Center.getHQNodeData",
                self.base_url
            ))
            .query(&[
                ("page", "1"),
                ("num", limit_text.as_str()),
                ("sort", "changepercent"),
                ("asc", "0"),
                ("node", "hs_a"),
            ])
            .send()
            .await
            .map_err(|e| PickError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(PickError::Network(format!("status {}", response.status())));
        }
        let rows: Vec<SinaRow> = response
            .json()
            .await
            .map_err(|e| PickError::Parse(e.to_string()))?;
        Ok(filter_sina_rows(rows))
    }
}

/// 北交所（bj）与 ST/退市/次新剔除；价格字符串转 f64。
fn filter_sina_rows(rows: Vec<SinaRow>) -> Vec<Candidate> {
    rows.into_iter()
        .filter_map(|row| {
            if !row.symbol.starts_with("sh") && !row.symbol.starts_with("sz") {
                return None;
            }
            if row.name.contains("ST")
                || row.name.contains("退")
                || row.name.starts_with('N')
                || row.name.starts_with('C')
            {
                return None;
            }
            let price = row.trade.as_deref().and_then(|p| p.parse::<f64>().ok())?;
            let pct = row.changepercent?;
            if price <= 0.0 || price > 2000.0 {
                return None;
            }
            let lu = is_limit_up(&row.symbol, pct);
            Some(Candidate {
                code: row.symbol,
                name: row.name,
                price,
                pct,
                industry: String::new(),
                is_limit_up: lu,
            })
        })
        .collect()
}

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
    /// 涨停（无法当日买入，标「次日开盘买入」）
    pub is_limit_up: bool,
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
            let full_code = format!("{}{}", market_prefix(code), code);
            let limit_up = is_limit_up(&full_code, pct);
            out.push(Candidate {
                code: full_code,
                name: name.to_string(),
                price,
                pct,
                industry: row["f100"].as_str().unwrap_or("").to_string(),
                is_limit_up: limit_up,
            });
        }
        Ok(out)
    }
}

/// 涨停判定：按板块代码前缀取涨跌幅上限（留 0.5% 容差应对数据延迟）。
/// 创业板 30/68 开头 20%、科创板 68 开头 20%、北交所（东财不含）30%、其余 10%。
fn is_limit_up(code: &str, pct: f64) -> bool {
    let limit = if code.starts_with("sz30") || code.starts_with("sh68") {
        19.5
    } else {
        9.5
    };
    pct >= limit
}

/// 东财 f12 是 6 位裸码：6 开头沪、其余深。
fn market_prefix(raw: &str) -> &'static str {
    if raw.starts_with('6') {
        "sh"
    } else {
        "sz"
    }
}

// ============================================================================
// §A.5.1 涨停二次校准：拉取实时价/涨幅，重新判定是否当日已封板。
// ============================================================================

/// §A.7 异动检测：单日内 pct 接近涨停但尚未封板（主板 ≥7 / 创业 ≥17）即视为异动。
pub fn is_anomaly(code: &str, pct: f64) -> bool {
    let threshold = if code.starts_with("sz30") || code.starts_with("sh68") {
        17.0
    } else {
        7.0
    };
    pct >= threshold
}

// ============================================================================

/// 单票实时校准结果。`Some(true)` = 实时涨停、`Some(false)` = 未涨停、
/// `None` = 拉取失败（保留 snapshot 的 is_limit_up）。
async fn fetch_realtime_limit_up(
    http: &reqwest::Client,
    base_url: &str,
    code: &str,
) -> Option<bool> {
    let normalized = code;
    let market = if normalized.starts_with("sh") {
        "1"
    } else {
        "0"
    };
    let pure = normalized.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let url = format!("{}/api/qt/stock/get", base_url);
    let resp = http
        .get(&url)
        .query(&[
            ("secid", format!("{}.{}", market, pure).as_str()),
            ("fields", "f2,f3"),
        ])
        .timeout(StdDuration::from_millis(2_000))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    let pct = json["data"]["f3"].as_f64()?;
    Some(is_limit_up(normalized, pct))
}

/// 对 top picks 做实时涨停二次校准；网络失败的票不出现在 map 里，调用方应回退到候选快照。
async fn recalibrate_limit_up_realtime(
    http: &reqwest::Client,
    base_url: &str,
    picks: &[(Candidate, f64, Vec<String>)],
) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for (candidate, _, _) in picks {
        match fetch_realtime_limit_up(http, base_url, &candidate.code).await {
            Some(lu) => {
                out.insert(candidate.code.clone(), lu);
            }
            None => {
                tracing::debug!(code = %candidate.code, "limit-up realtime fetch failed, keep snapshot");
            }
        }
        // 实时校准要快，避免阻塞落库
        tokio::time::sleep(StdDuration::from_millis(50)).await;
    }
    out
}

// ============================================================================
// 隔夜美股情绪（腾讯，与行情网关同源）
// ============================================================================

/// 腾讯美股指数：`/q=usDJI,usIXIC`（GBK 文本，字段 3=现价 4=昨收）。
#[derive(Debug, Clone)]
pub struct TencentUsIndex {
    /// 测试可覆写（默认 `http://qt.gtimg.cn`）
    pub base_url: String,
}

impl Default for TencentUsIndex {
    fn default() -> Self {
        Self {
            base_url: "http://qt.gtimg.cn".into(),
        }
    }
}

/// 道指 / 纳指涨跌（%，美股闭市时即昨日收盘）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsSentiment {
    pub djia_pct: Option<f64>,
    pub ixic_pct: Option<f64>,
}

impl UsSentiment {
    /// 全局情绪分：两指数均值 ≥1 → +10「隔夜美股偏多」；≤−1 → −10「隔夜美股偏空」。
    pub fn mood_score(&self) -> (f64, Option<String>) {
        let values: Vec<f64> = [self.djia_pct, self.ixic_pct]
            .iter()
            .flatten()
            .copied()
            .collect();
        if values.is_empty() {
            return (0.0, None);
        }
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        if avg >= 1.0 {
            (10.0, Some("隔夜美股偏多".into()))
        } else if avg <= -1.0 {
            (-10.0, Some("隔夜美股偏空".into()))
        } else {
            (0.0, None)
        }
    }

    /// 纳指跌超 1%：半导体 / 电子类行业候选额外惩罚（隔夜联动）。
    pub fn semis_drag(&self) -> bool {
        self.ixic_pct.map(|p| p <= -1.0).unwrap_or(false)
    }
}

impl TencentUsIndex {
    pub async fn fetch(&self, http: &reqwest::Client) -> Result<UsSentiment, PickError> {
        let response = http
            .get(format!("{}/q=usDJI,usIXIC", self.base_url))
            .send()
            .await
            .map_err(|e| PickError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(PickError::Network(format!("status {}", response.status())));
        }
        let body = response
            .text()
            .await
            .map_err(|e| PickError::Network(e.to_string()))?;
        // 数字字段对编码不敏感（与 tencent.rs 同款兜底）
        let (decoded, _, _) = encoding_rs::GBK.decode(body.as_bytes());
        Ok(parse_us_index(&decoded))
    }
}

/// 解析 `v_usDJI="200~道琼斯~.DJI~现价~昨收~…"` 两条行。
fn parse_us_index(body: &str) -> UsSentiment {
    let mut out = UsSentiment::default();
    for line in body.lines() {
        let (Some(start), Some(end)) = (line.find('"'), line.rfind('"')) else {
            continue;
        };
        if end <= start {
            continue;
        }
        let fields: Vec<&str> = line[start + 1..end].split('~').collect();
        if fields.len() < 5 {
            continue;
        }
        let pct = fields[3]
            .parse::<f64>()
            .ok()
            .zip(fields[4].parse::<f64>().ok())
            .and_then(|(price, prev)| {
                if prev > 0.0 {
                    Some((price - prev) / prev * 100.0)
                } else {
                    None
                }
            });
        match fields[2] {
            ".DJI" => out.djia_pct = pct,
            ".IXIC" => out.ixic_pct = pct,
            _ => {}
        }
    }
    out
}

// ============================================================================
// 板块动量（候选池行业聚合，零新增请求）与新闻关键词
// ============================================================================

/// 候选池按 f100 行业聚合平均涨幅。返回（行业 → 均值, 强势行业集合）：
/// 强势 = 均值 ≥2% 且进入 Top5；调用方对均值 ≤0 的行业减分。
fn industry_momentum(
    candidates: &[Candidate],
) -> (std::collections::HashMap<String, f64>, Vec<String>) {
    let mut bucket: std::collections::HashMap<String, (f64, usize)> =
        std::collections::HashMap::new();
    for candidate in candidates {
        if candidate.industry.is_empty() {
            continue;
        }
        let entry = bucket.entry(candidate.industry.clone()).or_insert((0.0, 0));
        entry.0 += candidate.pct;
        entry.1 += 1;
    }
    let mut avgs: Vec<(String, f64)> = bucket
        .iter()
        .map(|(industry, (sum, count))| (industry.clone(), sum / *count as f64))
        .collect();
    avgs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let strong: Vec<String> = avgs
        .iter()
        .take(5)
        .filter(|(_, avg)| *avg >= 2.0)
        .map(|(industry, _)| industry.clone())
        .collect();
    let map = avgs.into_iter().collect();
    (map, strong)
}

/// 半导体 / 电子类行业（纳指隔夜联动惩罚范围）。
fn is_tech_industry(industry: &str) -> bool {
    industry.contains("半导体")
        || industry.contains("电子")
        || industry.contains("芯片")
        || industry.contains("元件")
        || industry.contains("光学")
}

/// §A.7 负面信号分类。命中扣分的同时，把**具体类型**（减持/问询/立案/...）单独返回，
/// 用于 reasons + meta.negative_signals，便于回溯「为什么这票没推」。
const NEGATIVE_CATEGORIES: &[(&str, &str, f64)] = &[
    ("减持", "股东减持", -8.0),
    ("问询", "收到问询函", -5.0),
    ("立案", "被立案调查", -10.0),
    ("处罚", "监管处罚", -10.0),
    ("退市", "退市风险", -15.0),
    ("终止", "重组/上市终止", -8.0),
    ("诉讼", "重大诉讼", -5.0),
    ("质押", "高比例质押", -3.0),
];

fn negative_signal_score(text: &str) -> (f64, Vec<(String, f64)>) {
    let mut score = 0.0_f64;
    let mut cats = Vec::new();
    for (kw, tag, delta) in NEGATIVE_CATEGORIES {
        if text.contains(kw) {
            score += delta;
            cats.push((tag.to_string(), *delta));
        }
    }
    (score, cats)
}

/// 新闻关键词扫描：利好 +5 / 利空按分类扣分（最多 −15 单条），总分 ±25 封顶。
/// 返回 (score, positive_tag, negative_tags, negative_signals)
///   negative_signals 每条 (来源类型, 命中的原文标题) 供 meta 透出。
fn news_keyword_score(
    items: &[crate::model::NewsItem],
) -> (f64, Option<String>, Vec<String>, Vec<(String, String)>) {
    const POSITIVE: [&str; 9] = [
        "中标", "订单", "回购", "增持", "预增", "突破", "签约", "上调", "涨停",
    ];
    let mut score = 0.0_f64;
    let mut pos_hit = false;
    let mut neg_tags: Vec<String> = Vec::new();
    let mut neg_signals: Vec<(String, String)> = Vec::new();
    for item in items {
        let text = format!("{}{}", item.title, item.summary);
        for word in POSITIVE {
            if text.contains(word) {
                score += 5.0;
                pos_hit = true;
            }
        }
        let (delta, cats) = negative_signal_score(&text);
        if delta < 0.0 {
            // 单条负面 ≥ -15 封底，避免极端词刷屏
            let clamped = delta.max(-15.0);
            score += clamped;
            for (cat_tag, _) in cats {
                if !neg_tags.contains(&cat_tag) {
                    neg_tags.push(cat_tag.clone());
                }
                // 取首条命中条目的标题作展示（若多条同 tag）
                if !neg_signals.iter().any(|(t, _)| t == &cat_tag) {
                    neg_signals.push((cat_tag, item.title.clone()));
                }
            }
        }
    }
    let total = score.clamp(-25.0, 25.0);
    let positive_tag = if pos_hit { Some("利好新闻".to_string()) } else { None };
    (total, positive_tag, neg_tags, neg_signals)
}

/// §A.6 主力资金净流入打分（净流入 / 万元）：
/// |主力净流入| 档位 → 加分；流入为负则对称减分。
///  资金流入：>1亿 +25 / 5000万-1亿 +15 / 1000万-5000万 +8 / 100-1000万 +3
///  资金流出：<1亿 -25 / 5000万-1亿 -15 / 1000万-5000万 -8 / 100-1000万 -3
///  净占比（f170）：|x| >= 10 时 ±5 加成（流入正 / 流出负）
/// 标签：流入>1亿 →「主力抢筹」；流出>1亿 →「主力出逃」；其他档位显「主力流入」/「主力流出」。
pub fn main_net_score(main_net_wan: f64, pct_ratio: f64) -> (f64, Option<String>) {
    let wan = main_net_wan;
    let base: f64 = if wan >= 10_000.0 {
        25.0
    } else if wan >= 5_000.0 {
        15.0
    } else if wan >= 1_000.0 {
        8.0
    } else if wan >= 100.0 {
        3.0
    } else if wan <= -10_000.0 {
        -25.0
    } else if wan <= -5_000.0 {
        -15.0
    } else if wan <= -1_000.0 {
        -8.0
    } else if wan <= -100.0 {
        -3.0
    } else {
        0.0
    };
    // 占比加成（pct_ratio 是百分数；>=10 视为强主力）
    let pct_bonus: f64 = if pct_ratio >= 10.0 {
        5.0
    } else if pct_ratio <= -10.0 {
        -5.0
    } else {
        0.0
    };
    let score = (base + pct_bonus).clamp(-30.0_f64, 30.0_f64);
    let tag = if wan >= 10_000.0 {
        Some("主力抢筹".into())
    } else if wan <= -10_000.0 {
        Some("主力出逃".into())
    } else if wan >= 1_000.0 {
        Some("主力流入".into())
    } else if wan <= -1_000.0 {
        Some("主力流出".into())
    } else {
        None
    };
    (score, tag)
}

// ============================================================================
// A.4 复盘学习：标签胜率自动调权
// ============================================================================

#[derive(Debug, Clone)]
struct TagPerformance {
    samples: i64,
    win_rate: f64,
}

/// 单标签调整：少于 30 样本不介入；30..60 样本线性增加置信度。
/// 胜率 50% 为中性，单标签最多 ±10 分，防止小样本把技术分整体推翻。
fn learned_tag_delta(samples: i64, win_rate: f64) -> f64 {
    if samples < 30 || !win_rate.is_finite() {
        return 0.0;
    }
    let confidence = (samples as f64 / 60.0).clamp(0.5, 1.0);
    ((win_rate.clamp(0.0, 1.0) - 0.5) * 40.0 * confidence).clamp(-10.0, 10.0)
}

/// 返回（总调整分，可解释证据 JSON）。总调整限制在 ±20 分。
fn learned_score_adjustment(
    tags: &[String],
    performance: &HashMap<String, TagPerformance>,
) -> (f64, Value) {
    let mut total = 0.0;
    let mut evidence = Vec::new();
    for tag in tags {
        let Some(stat) = performance.get(tag) else {
            continue;
        };
        let delta = learned_tag_delta(stat.samples, stat.win_rate);
        if delta.abs() < 0.01 {
            continue;
        }
        total += delta;
        evidence.push(serde_json::json!({
            "tag": tag,
            "samples": stat.samples,
            "win_rate": stat.win_rate,
            "delta": delta,
        }));
    }
    let adjustment = total.clamp(-20.0, 20.0);
    (
        adjustment,
        serde_json::json!({
            "adjustment": adjustment,
            "tags": evidence,
            "minimum_samples": 30,
            "lookback_days": 30,
        }),
    )
}

/// 只使用推荐日之前的 T+1 结果，对齐“尾盘买、明日涨”的主目标，
/// 并杜绝重跑当日推荐时的未来数据泄漏。
async fn load_tag_performance(
    db: &SqlitePool,
    as_of: NaiveDate,
) -> Result<HashMap<String, TagPerformance>, PickError> {
    let since = (as_of - Duration::days(30)).format("%Y-%m-%d").to_string();
    let before = as_of.format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT reasons, meta FROM daily_pick \
         WHERE date >= ? AND date < ? \
         AND json_extract(meta,'$.outcome.t1_pct') IS NOT NULL",
    )
    .bind(&since)
    .bind(&before)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let mut bucket: HashMap<String, (i64, i64)> = HashMap::new();
    for (reasons, meta) in rows {
        let t1 = serde_json::from_str::<Value>(&meta)
            .ok()
            .and_then(|m| m["outcome"]["t1_pct"].as_f64());
        let Some(t1) = t1 else { continue };
        let tags: Vec<String> = serde_json::from_str(&reasons).unwrap_or_default();
        for tag in tags {
            let entry = bucket.entry(tag).or_insert((0, 0));
            entry.0 += 1;
            if t1 > 0.0 {
                entry.1 += 1;
            }
        }
    }
    Ok(bucket
        .into_iter()
        .map(|(tag, (samples, wins))| {
            (
                tag,
                TagPerformance {
                    samples,
                    win_rate: if samples > 0 {
                        wins as f64 / samples as f64
                    } else {
                        0.0
                    },
                },
            )
        })
        .collect())
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

/// 技术面打分（0..110，含超买 / 超涨负分）。`None` = 硬性淘汰（数据不足 / 一字板 / RSI 超买）。
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

    // MACD 零上金叉：趋势内的回踩再启动（比零下金叉胜率高）
    if golden && dif[len - 1] > 0.0 {
        score += 5.0;
        tags.push("零上金叉".into());
    }

    // KDJ（9 日）：近 3 根 K 上穿 D 金叉；J > 100 超买惩罚
    let (k_series, d_series) = kdj_series(highs, lows, closes);
    if k_series.len() >= 4 {
        let mut kdj_golden = false;
        for i in k_series.len().saturating_sub(3)..k_series.len() {
            if i == 0 {
                continue;
            }
            if k_series[i - 1] <= d_series[i - 1] && k_series[i] > d_series[i] {
                kdj_golden = true;
                break;
            }
        }
        if kdj_golden {
            score += 10.0;
            tags.push("KDJ金叉".into());
        }
        let j = 3.0 * k_series[k_series.len() - 1] - 2.0 * d_series[k_series.len() - 1];
        if j > 100.0 {
            score -= 10.0;
            tags.push("KDJ超买".into());
        }
    }

    // BOLL(20,2)：中轨上方 / 突破上轨（非涨停式突破才算）
    if n >= 20 {
        let mid: f64 = closes[n - 20..].iter().sum::<f64>() / 20.0;
        let variance: f64 = closes[n - 20..]
            .iter()
            .map(|c| (c - mid).powi(2))
            .sum::<f64>()
            / 20.0;
        let upper = mid + 2.0 * variance.sqrt();
        if close > mid {
            score += 5.0;
            tags.push("中轨上方".into());
        }
        if close > upper && pct < 7.0 {
            score += 8.0;
            tags.push("突破上轨".into());
        }
    }

    // BIAS5 短期超涨惩罚（乖离 >8% 有回归压力）
    if let Some(ma5) = ma_last(closes, 5) {
        if ma5 > 0.0 {
            let bias = (close - ma5) / ma5 * 100.0;
            if bias > 8.0 {
                score -= 8.0;
                tags.push("短期超涨".into());
            }
        }
    }

    Some(TechScore { score, tags })
}

/// KDJ（9 日 RSV，K/D 平滑系数 1/3）。返回 K / D 序列。
fn kdj_series(highs: &[f64], lows: &[f64], closes: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = closes.len();
    let mut k = Vec::with_capacity(n);
    let mut d = Vec::with_capacity(n);
    let (mut pk, mut pd) = (50.0_f64, 50.0_f64);
    for i in 0..n {
        if i >= 8 {
            let hi = highs[i - 8..=i].iter().cloned().fold(f64::MIN, f64::max);
            let lo = lows[i - 8..=i].iter().cloned().fold(f64::MAX, f64::min);
            let rsv = if hi > lo {
                (closes[i] - lo) / (hi - lo) * 100.0
            } else {
                50.0
            };
            pk = 2.0 / 3.0 * pk + rsv / 3.0;
            pd = 2.0 / 3.0 * pd + pk / 3.0;
        }
        k.push(pk);
        d.push(pd);
    }
    (k, d)
}

// ============================================================================
// ③④ 生成主流程
// ============================================================================

fn build_trade_plan(
    date: NaiveDate,
    now_cn: DateTime<FixedOffset>,
    tags: &[String],
    has_position: bool,
    is_limit_up: bool,
    close: f64,
    sell_zone: Option<(f64, f64)>,
    buy_zone_low: f64,
    buy_zone_high: f64,
) -> Value {
    let momentum = tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "MACD金叉" | "零上金叉" | "放量" | "近20日高" | "突破上轨" | "KDJ金叉"
        )
    });
    let strategy = if has_position {
        "做T"
    } else if momentum {
        "短线"
    } else {
        "中线"
    };
    // 今日已涨停的票：T 日买入不了，必须次日开盘（甚至集合竞价）才有成交。
    // 不论当前是不是 14:45 窗口，entry_label 强制改成「次日开盘」。
    let before_close = now_cn.hour() * 60 + now_cn.minute() < 15 * 60;
    let today_signal = date == now_cn.date_naive();
    let (entry_timing, entry_label, entry_window, objective) = if is_limit_up {
        (
            "next_session_open",
            "次日开盘",
            "09:30-09:35",
            "next_day_open_positive",
        )
    } else if today_signal && before_close {
        ("today_close", "今日尾盘", "14:45-14:57", "next_day_positive_close")
    } else {
        (
            "next_session_pullback",
            "下一交易日回踩",
            "09:35-10:30",
            "next_day_positive_close",
        )
    };
    let exit_rule = match strategy {
        "做T" => "仅适合已有底仓；新增仓按 T+1，次日冲高再减",
        "中线" => "T+1 先验证强弱，趋势未破可观察 3-10 个交易日",
        _ => "T+1 为主；次日冲高或收盘转弱时评估退出",
    };
    // §A.9 买入价区间：现价 ±2%；封板票以封板价 ±2% 作为次日开盘预期买入区
    // §A.9 卖出价区间：基于 T+1 历史均值 × 胜率因子 ±1.5%（无数据时按 1.5% 期望收益推算）
    let (sell_price_low, sell_price_high) = sell_zone.unwrap_or((close * 1.012, close * 1.022));
    serde_json::json!({
        "signal_date": date.format("%Y-%m-%d").to_string(),
        "target": "T+1",
        "objective": objective,
        "entry_timing": entry_timing,
        "entry_label": entry_label,
        "entry_window": entry_window,
        "strategy": strategy,
        "has_base_position": has_position,
        "is_limit_up": is_limit_up,
        "exit_rule": exit_rule,
        "buy_price_low": round_price(buy_zone_low),
        "buy_price_high": round_price(buy_zone_high),
        "sell_price_low": round_price(sell_price_low),
        "sell_price_high": round_price(sell_price_high),
        "buy_basis_close": round_price(close),
    })
}

/// §A.9 价格取整：A 股最小报价 0.01 元，按 0.01 取整便于前端展示。
fn round_price(p: f64) -> f64 {
    (p * 100.0).round() / 100.0
}

/// §A.9 卖出价区间计算器：基于该票近 30 天 outcome.t1_pct 均值与胜率。
///
///  sell_mid = close × (1 + avg_t1_pct × win_rate_factor)
///  win_rate_factor = max(0.5, observed_win_rate)  // 0.5 是「50% 期望收益」的下限
///  返回 (sell_low = sell_mid × 0.985, sell_high = sell_mid × 1.015)
///  样本 < 3 时 fallback：sell_mid = close × 1.012（市场平均 T+1 收益 1.2%）。
pub async fn fetch_sell_zone(
    db: &SqlitePool,
    code: &str,
    close: f64,
) -> Option<(f64, f64)> {
    if close <= 0.0 {
        return None;
    }
    let basis = fetch_sell_zone_basis(db, code).await;
    let Some(basis) = basis else {
        let sell_mid = close * 1.012;
        return Some((round_price(sell_mid * 0.985), round_price(sell_mid * 1.015)));
    };
    let samples = basis["samples"].as_i64().unwrap_or(0);
    if samples == 0 {
        let sell_mid = close * 1.012;
        return Some((round_price(sell_mid * 0.985), round_price(sell_mid * 1.015)));
    }
    let avg_pct = basis["avg_t1_pct"].as_f64().unwrap_or(0.0);
    let win_rate = basis["win_rate"].as_f64().unwrap_or(0.5);
    let factor = ((avg_pct / 100.0) * win_rate).clamp(-0.05, 0.05);
    let sell_mid = close * (1.0 + factor);
    Some((round_price(sell_mid * 0.985), round_price(sell_mid * 1.015)))
}

/// §A.9 卖出区间基础数据：直接返回 {samples, win_rate, avg_t1_pct, win_rate_factor}。
///  样本 < 3 视为无效（返回 null），调用方按默认 1.2% 推算。
pub async fn fetch_sell_zone_basis(db: &SqlitePool, code: &str) -> Option<Value> {
    let since = (Utc::now().date_naive() - Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let rows: Vec<f64> = sqlx::query_scalar(
        "SELECT CAST(json_extract(meta,'$.outcome.t1_pct') AS REAL) FROM daily_pick \
         WHERE code = ? AND date >= ? AND json_extract(meta,'$.outcome.t1_pct') IS NOT NULL",
    )
    .bind(code)
    .bind(&since)
    .fetch_all(db)
    .await
    .ok()?;
    if rows.len() < 3 {
        return None;
    }
    let n = rows.len() as i64;
    let avg = rows.iter().sum::<f64>() / rows.len() as f64;
    let wins = rows.iter().filter(|p| **p > 0.0).count() as f64;
    let win_rate_raw = wins / rows.len() as f64;
    let win_rate_factor = win_rate_raw.max(0.5);
    Some(serde_json::json!({
        "samples": n,
        "win_rate": win_rate_raw,
        "avg_t1_pct": avg,
        "win_rate_factor": win_rate_factor,
    }))
}

/// §A.9 上一交易日推荐：拉近 30 天内最近一次生成且已回写 T+1 outcome 的 picks，
/// 给前端展示「昨日推荐 → 今日卖点」用。
///
/// 卖出价基准：
///   - 推荐时 entry_timing=next_session_open → 用 t1_open（真实开盘价）+0.5% 浮动
///   - 推荐时 entry_timing=today_close / next_session_pullback → 用 t1_close × 1.0
///
/// 卖出价区间 ±1.5%（与 build_trade_plan 卖区间一致）
pub async fn fetch_previous_picks(
    db: &SqlitePool,
    today: NaiveDate,
) -> Result<Vec<crate::model::PreviousPick>, sqlx::Error> {
    // 拉最近 30 天内已回写 T+1 outcome 的「距 today 最近一天」的 daily_pick
    let since = (today - Duration::days(30)).format("%Y-%m-%d").to_string();
    // 取最近的 1 个有 outcome 的 date
    let latest: Option<(String,)> = sqlx::query_as(
        "SELECT date FROM daily_pick \
         WHERE date >= ? AND json_extract(meta,'$.outcome.t1_pct') IS NOT NULL \
         ORDER BY date DESC LIMIT 1",
    )
    .bind(&since)
    .fetch_optional(db)
    .await?;
    let Some((latest_date,)) = latest else {
        return Ok(Vec::new());
    };
    let rows: Vec<(String, String, String, i64, String, Option<String>)> = sqlx::query_as(
        "SELECT date, code, name, rank, reasons, meta FROM daily_pick \
         WHERE date = ? AND json_extract(meta,'$.outcome.t1_pct') IS NOT NULL \
         ORDER BY rank ASC",
    )
    .bind(&latest_date)
    .fetch_all(db)
    .await?;
    let mut out: Vec<crate::model::PreviousPick> = Vec::new();
    for (date, code, name, rank, reasons, meta_json) in rows {
        let meta: serde_json::Value = match meta_json {
            Some(s) => serde_json::from_str(&s).unwrap_or(serde_json::json!({})),
            None => continue,
        };
        let close = meta["close"].as_f64();
        let outcome = &meta["outcome"];
        let t1_pct = outcome["t1_pct"].as_f64();
        let t1_real_pct = outcome["t1_real"].as_f64();
        let t1_open = outcome["t1_open_basis"].as_f64();
        let t1_close = outcome["t1_close"].as_f64();
        let entry_gap = outcome["entry_gap"].as_f64();
        let entry_timing = meta["plan"]["entry_timing"].as_str().map(String::from);
        let entry_label = meta["plan"]["entry_label"].as_str().map(String::from);
        let reasons_vec: Vec<String> = serde_json::from_str(&reasons).unwrap_or_default();
        // §A.9 卖出价区间
        let (basis_price, basis_kind) = if matches!(entry_timing.as_deref(), Some("next_session_open")) {
            if let Some(open) = t1_open {
                (Some(open * 1.005), Some("actual_t1_open".to_string()))
            } else {
                (t1_close.or(close), Some("t1_close".to_string()))
            }
        } else {
            (t1_close.or(close), Some("t1_close".to_string()))
        };
        let (sell_low, sell_high) = match basis_price {
            Some(p) => (round_price(p * 0.985), round_price(p * 1.015)),
            None => (0.0, 0.0),
        };
        out.push(crate::model::PreviousPick {
            date,
            code,
            name,
            rank: rank as usize,
            close,
            t1_open,
            t1_close,
            t1_pct,
            t1_real_pct,
            entry_gap,
            sell_price_low: if sell_low > 0.0 { Some(sell_low) } else { None },
            sell_price_high: if sell_high > 0.0 { Some(sell_high) } else { None },
            sell_basis_price: basis_price,
            sell_basis_kind: basis_kind,
            entry_timing,
            entry_label,
            reasons: reasons_vec,
        });
    }
    Ok(out)
}

/// 生成某基准日推荐。`ai` 为客户端透传配置（None = 纯量化，调度器路径）。
/// 幂等：同日重复生成 DELETE + INSERT 全量刷新。
pub async fn generate_picks(
    state: &AppState,
    date: NaiveDate,
    ai_config: Option<&AiRankConfig>,
) -> Result<PicksDocument, PickError> {
    let learned_performance = load_tag_performance(&state.db, date).await?;
    let mut learned_meta: HashMap<String, Value> = HashMap::new();
    // 候选池：东财 push2 断连时切新浪榜（板块动量因子因无行业字段自动降级）
    let candidates = match state.pick_ranking.fetch(&state.http, 100).await {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) => return Err(PickError::Parse("涨幅榜为空".into())),
        Err(error) => {
            tracing::warn!(%error, "eastmoney ranking failed, fallback to sina");
            state.sina_ranking.fetch(&state.http, 100).await?
        }
    };
    if candidates.is_empty() {
        return Err(PickError::Parse("涨幅榜为空".into()));
    }

    // 板块动量（候选池行业聚合）+ 隔夜美股情绪（拉取失败只降级跳过）
    let (industry_avg, strong_industries) = industry_momentum(&candidates);
    let us_sentiment: Option<UsSentiment> = state.us_index.fetch(&state.http).await.ok();
    let (us_mood, us_tag) = us_sentiment
        .as_ref()
        .map(|s| s.mood_score())
        .unwrap_or((0.0, None));
    let semis_drag = us_sentiment
        .as_ref()
        .map(|s| s.semis_drag())
        .unwrap_or(false);

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

    // §A.6 主力净流入抓取（拉取失败只降级跳过）
    let mut main_net_map: std::collections::HashMap<String, MainNetSnapshot> =
        std::collections::HashMap::new();
    // 暴露到 meta 用于回溯：每只票 main_net_score 实际加分（便于回测 / 调试）。
    let mut main_net_deltas: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for (candidate, _) in &scored {
        match state.main_net.fetch(&state.http, &candidate.code).await {
            Ok(Some(snapshot)) => {
                main_net_map.insert(candidate.code.clone(), snapshot);
            }
            Ok(None) => {
                tracing::debug!(code = %candidate.code, "main_net empty (pre-market / halted)");
            }
            Err(error) => {
                tracing::debug!(code = %candidate.code, %error, "main_net fetch failed");
            }
        }
        tokio::time::sleep(StdDuration::from_millis(200)).await;
    }
    if let Err(error) = crate::service::ingest::persist_main_net(
        &state.db,
        &main_net_map.values().cloned().collect::<Vec<_>>(),
    )
    .await
    {
        tracing::warn!(%error, "main_net persist failed");
    }

    // §A.5.1 涨停实时校准（候选池阶段就拉，便于异动检测复用）
    let scored_for_realtime: Vec<(Candidate, f64, Vec<String>)> = scored
        .iter()
        .map(|(c, t)| (c.clone(), t.score, t.tags.clone()))
        .collect();
    let limit_up_realtime: HashMap<String, bool> = recalibrate_limit_up_realtime(
        &state.http,
        &state.pick_ranking.base_url,
        &scored_for_realtime,
    )
    .await;

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
    let mut negative_signals_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    // §A.9 卖出价区间：基于该票近 30 天 outcome.t1_pct 算 sell_zone_map / sell_zone_basis_map
    let mut sell_zone_map: HashMap<String, (f64, f64)> = HashMap::new();
    let mut sell_zone_basis_map: HashMap<String, Value> = HashMap::new();
    for (candidate, _tech) in &scored {
        let zone = fetch_sell_zone(&state.db, &candidate.code, candidate.price).await;
        if let Some((low, high)) = zone {
            sell_zone_map.insert(candidate.code.clone(), (low, high));
        }
        // 同步抓基础信息（样本 / 胜率 / 均值）便于 meta 透出
        let basis = fetch_sell_zone_basis(&state.db, &candidate.code).await;
        sell_zone_basis_map.insert(
            candidate.code.clone(),
            basis.unwrap_or_else(|| {
                serde_json::json!({
                    "samples": 0, "win_rate": 0.0, "avg_t1_pct": 0.0, "win_rate_factor": 0.5
                })
            }),
        );
    }
    for (candidate, tech) in &scored {
        let mut score = tech.score;
        let mut tags = tech.tags.clone();

        // 今日已涨停的票：T 日买不进，必须次日开盘才有成交。
        // 不硬淘汰（好标的仍可保留），扣分 -5（弱惩罚），打「次日开盘」标签；
        // build_trade_plan 会把 entry_label 强制改为「次日开盘 09:30-09:35」。
        if candidate.is_limit_up {
            score -= 5.0;
            tags.push("次日开盘".into());
        }

        // 板块动量：强势行业 +15；行业均值 ≤0 减 10
        if strong_industries.contains(&candidate.industry) {
            score += 15.0;
            tags.push("强势行业".into());
        } else if industry_avg
            .get(&candidate.industry)
            .map(|avg| *avg <= 0.0)
            .unwrap_or(false)
        {
            score -= 10.0;
        }

        // 隔夜美股：全局情绪 ±10；纳指跌超 1% 时半导体 / 电子类额外 −15
        score += us_mood;
        if let Some(tag) = &us_tag {
            tags.push(tag.clone());
        }
        if semis_drag && is_tech_industry(&candidate.industry) {
            score -= 15.0;
            tags.push("隔夜纳指拖累".into());
        }

        // §A.7 异动：实时校准 pct ≥ 阈值（主板 7 / 创业 17）→ 加「异常波动」标签
        // 复用 limit_up_realtime 字典避免二次拉取
        if let Some(true) = limit_up_realtime.get(&candidate.code).copied() {
            // 实时已封板会进 plan 的次日开盘，不在这里再加「异常波动」以免标签重复
        } else {
            // 用 candidate 快照 pct 作为粗筛；若上游不通则跳过
            if is_anomaly(&candidate.code, candidate.pct) {
                tags.push("异常波动".into());
            }
        }

        // 研报评级（近 7 天）
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

        // 新闻：当日拉取（落库复用）+ 关键词扫描 + 热度加分
        match state.news.fetch(&state.http, &candidate.code, 10).await {
            Ok(items) => {
                if let Err(error) = crate::service::ingest::persist_news(&state.db, &items).await {
                    tracing::warn!(code = %candidate.code, %error, "pick news persist failed");
                }
                let (news_score, positive_tag, neg_tags, neg_signals) = news_keyword_score(&items);
                score += news_score;
                if let Some(tag) = positive_tag {
                    tags.push(tag);
                }
                for tag in &neg_tags {
                    tags.push(tag.clone());
                }
                if !neg_signals.is_empty() {
                    negative_signals_map.insert(candidate.code.clone(), neg_signals);
                }
                if items.len() >= 3 {
                    tags.push("新闻活跃".into());
                }
                score += (items.len().min(5)) as f64 * 2.0;
            }
            Err(error) => {
                tracing::debug!(code = %candidate.code, %error, "pick news fetch failed");
            }
        }
        // §A.6 主力净流入打分（候选池 §A.6 已批量抓过，落库）
        let main_net_snapshot = main_net_map.get(&candidate.code);
        if let Some(snapshot) = main_net_snapshot {
            let (delta, tag) = main_net_score(snapshot.main_net_wan, snapshot.pct_ratio);
            score += delta;
            main_net_deltas.insert(candidate.code.clone(), delta);
            if let Some(tag) = tag {
                tags.push(tag);
            }
        }
        let (learned_adjustment, evidence) = learned_score_adjustment(&tags, &learned_performance);
        score += learned_adjustment;
        learned_meta.insert(candidate.code.clone(), evidence);
        boosted.push((candidate.clone(), score, tags));
        tokio::time::sleep(StdDuration::from_millis(300)).await;
    }
    // 高胜率门槛：综合分 <60 不入选（宁缺毋滥，不足 5 只就少推）
    boosted.retain(|(_, score, _)| *score >= 60.0);
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

    // §A.5.1 涨停二次校准已在候选池阶段完成（提前到 scored 之后），这里直接复用。

    // 落库（同日全量刷新）
    let date_key = date.format("%Y-%m-%d").to_string();
    let position_codes: Vec<String> =
        sqlx::query_scalar("SELECT code FROM position WHERE shares > 0")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    let now_cn = Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM daily_pick WHERE date = ?")
        .bind(&date_key)
        .execute(&mut *tx)
        .await?;
    let now_ms = Utc::now().timestamp_millis();
    for (rank, (candidate, score, tags)) in picks.iter().enumerate() {
        let has_position = position_codes.iter().any(|code| code == &candidate.code);
        // 二次校准：实时拉到涨停的票，is_limit_up 视为 true（plan 强制次日开盘）；
        // 实时拉到未涨停，且 snapshot 也未涨停 → false；未拉到则用 snapshot。
        let realtime_limit_up = limit_up_realtime
            .get(&candidate.code)
            .copied()
            .unwrap_or(candidate.is_limit_up);
        let plan_limit_up = realtime_limit_up || candidate.is_limit_up;
        let meta = serde_json::json!({
            "close": candidate.price,
            "pct": candidate.pct,
            "industry": candidate.industry,
            "industry_avg": industry_avg.get(&candidate.industry),
            "is_limit_up": candidate.is_limit_up,
            "limit_up_realtime": limit_up_realtime
                .get(&candidate.code)
                .copied()
                .unwrap_or(false),
            "limit_up_calibrated": limit_up_realtime.contains_key(&candidate.code),
            "us": {
                "djia": us_sentiment.as_ref().and_then(|s| s.djia_pct),
                "ixic": us_sentiment.as_ref().and_then(|s| s.ixic_pct),
            },
            "main_net": main_net_map.get(&candidate.code).map(|s| serde_json::json!({
                "main_net_wan": s.main_net_wan,
                "super_net_wan": s.super_net_wan,
                "big_net_wan": s.big_net_wan,
                "pct_ratio": s.pct_ratio,
                "turnover": s.turnover,
                "trade_date": s.trade_date,
                "score_delta": main_net_deltas.get(&candidate.code).copied().unwrap_or(0.0),
            })),
            "auto_weight": learned_meta.get(&candidate.code).cloned().unwrap_or_else(|| serde_json::json!({
                "adjustment": 0.0,
                "tags": [],
                "minimum_samples": 30,
                "lookback_days": 30,
            })),
            // §A.9 卖出价区间：基于该票近 30 天 outcome.t1_pct 均值与胜率
            "sell_zone_basis": sell_zone_basis_map.get(&candidate.code).cloned().unwrap_or(serde_json::json!({
                "samples": 0, "win_rate": 0.0, "avg_t1_pct": 0.0, "win_rate_factor": 0.5
            })),
            "negative_signals": negative_signals_map
                .get(&candidate.code)
                .map(|signals| {
                    signals
                        .iter()
                        .map(|(cat, title)| {
                            serde_json::json!({"category": cat, "title": title})
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            "plan": build_trade_plan(
                date,
                now_cn,
                tags,
                has_position,
                plan_limit_up,
                candidate.price,
                sell_zone_map.get(&candidate.code).copied(),
                candidate.price * 0.98,
                candidate.price * 1.02,
            ),
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
    let mut doc = list_picks(&state.db, Some(&date_key)).await?;
    // 执行时机：15:00 前生成 → 当天下午可买入；之后 → 次日开盘买入
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let now_cn = chrono::Utc::now().with_timezone(&tz);
    doc.execute_hint = if now_cn.hour() < 15 {
        "当天下午可买入".to_string()
    } else {
        "次日开盘买入（次日开盘价可能高于推荐日收盘价）".to_string()
    };
    Ok(doc)
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
    lines.push("请从中挑选最适合今日尾盘关注、下一交易日收盘上涨概率最高的 5 只，按 T+1 把握排序；避免只适合中长线但隔日不确定的标的。".into());
    lines.push("只输出 JSON：{\"picks\":[{\"code\":\"sh600xxx\",\"note\":\"一句话理由\"}]}，不要多余文字。".into());
    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: "你是严谨的 A 股尾盘选股研究员，主目标是 T+1 正收益概率；只基于给定数据判断，不给承诺性投资建议。".into(),
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
/// §A.8 胜率置信区间（Wilson score interval，95%）。
///
///  真实样本 ≥30 时区间半宽通常 ≤ 0.18；样本 10 的 70% 胜率下界 0.35、
///  上界 0.93——这个宽度足以让「70% 胜率」不再被误读为稳定指标。
///  返回 (low, high, margin)；n == 0 时 low/high/margin 全部为 0。
pub fn wilson_interval(samples: i64, hits: i64, z: f64) -> (f64, f64, f64) {
    if samples <= 0 {
        return (0.0, 0.0, 0.0);
    }
    let n = samples as f64;
    let p = hits as f64 / n;
    let z2 = z * z;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let half = (z * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt()) / denom;
    let low = (center - half).clamp(0.0, 1.0);
    let high = (center + half).clamp(0.0, 1.0);
    (low, high, (high - low) / 2.0)
}

/// 阈值以下视为「样本不足，胜率仅作参考」。来源：A.4 调权门槛 = 30。
pub fn samples_sufficient(samples: i64) -> bool {
    samples >= 30
}

/// 标准化 Wilson 区间元组（用于 PickStats 字段填充）。
pub fn confidence_bounds(samples: i64, hits: i64) -> (f64, f64, f64, bool) {
    let (low, high, margin) = wilson_interval(samples, hits, 1.96);
    (low, high, margin, samples_sufficient(samples))
}

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
                t1_win_rate: 0.0,
                t1_win_rate_low: 0.0,
                t1_win_rate_high: 0.0,
                t1_win_rate_margin: 0.0,
                t1_samples_sufficient: false,
                avg_t1_pct: 0.0,
                t5_samples: 0,
                t5_win_rate: 0.0,
                t5_win_rate_low: 0.0,
                t5_win_rate_high: 0.0,
                t5_win_rate_margin: 0.0,
                t5_samples_sufficient: false,
                avg_t5_pct: 0.0,
                tags: Vec::new(),
                execution: Default::default(),
            },
            market: serde_json::Value::Null,
            execute_hint: String::new(),
            previous_picks: Vec::new(),
        });
    }
    let rows: Vec<(String, String, String, i64, f64, String, String, String)> = sqlx::query_as(
        "SELECT date, code, name, rank, score, reasons, ai_note, meta FROM daily_pick \
         WHERE date = ? ORDER BY rank ASC",
    )
    .bind(&date_key)
    .fetch_all(db)
    .await?;
    let picks: Vec<DailyPick> = rows
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

    // 回测统计：T+1 是主目标，T+5 保留为中线参考。
    let since = (Utc::now().date_naive() - Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let outcomes: Vec<String> = sqlx::query_scalar(
        "SELECT meta FROM daily_pick WHERE date >= ? AND (\
         json_extract(meta,'$.outcome.t1_pct') IS NOT NULL OR \
         json_extract(meta,'$.outcome.t5_pct') IS NOT NULL)",
    )
    .bind(&since)
    .fetch_all(db)
    .await?;
    let mut t1_win = 0.0;
    let mut t1_sum = 0.0;
    let mut t1_samples = 0_i64;
    let mut t5_win = 0.0;
    let mut t5_sum = 0.0;
    let mut t5_samples = 0_i64;
    // §A.8 Wilson 95% 区间：避免「胜率 70% · 样本 10」被误读为稳定指标
    let (mut t1_low, mut t1_high, mut t1_margin, mut t1_sufficient) =
        confidence_bounds(0, 0);
    let (mut t5_low, mut t5_high, mut t5_margin, mut t5_sufficient) =
        confidence_bounds(0, 0);
    for meta in &outcomes {
        if let Ok(value) = serde_json::from_str::<Value>(meta) {
            if let Some(pct) = value["outcome"]["t1_pct"].as_f64() {
                t1_samples += 1;
                t1_sum += pct;
                if pct > 0.0 {
                    t1_win += 1.0;
                }
            }
            if let Some(pct) = value["outcome"]["t5_pct"].as_f64() {
                t5_samples += 1;
                t5_sum += pct;
                if pct > 0.0 {
                    t5_win += 1.0;
                }
            }
        }
    }
    let (l, h, m, ok) = confidence_bounds(t1_samples, t1_win as i64);
    t1_low = l;
    t1_high = h;
    t1_margin = m;
    t1_sufficient = ok;
    let (l, h, m, ok) = confidence_bounds(t5_samples, t5_win as i64);
    t5_low = l;
    t5_high = h;
    t5_margin = m;
    t5_sufficient = ok;
    // 标签级回测：按 reasons 聚合 T+1 胜率，直接服务隔日目标。
    let tag_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT reasons, meta FROM daily_pick \
         WHERE date >= ? AND json_extract(meta,'$.outcome.t1_pct') IS NOT NULL",
    )
    .bind(&since)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let mut tag_bucket: std::collections::HashMap<String, (i64, i64)> =
        std::collections::HashMap::new();
    for (reasons, meta) in &tag_rows {
        let t1: Option<f64> = serde_json::from_str::<Value>(meta)
            .ok()
            .and_then(|m| m["outcome"]["t1_pct"].as_f64());
        let Some(t1) = t1 else { continue };
        let tags: Vec<String> = serde_json::from_str(reasons).unwrap_or_default();
        for tag in tags {
            let entry = tag_bucket.entry(tag).or_insert((0, 0));
            entry.0 += 1;
            if t1 > 0.0 {
                entry.1 += 1;
            }
        }
    }
    let mut tag_stats: Vec<PickTagStat> = tag_bucket
        .into_iter()
        .map(|(tag, (samples, wins))| {
            let (low, high, margin, sufficient) = confidence_bounds(samples, wins);
            PickTagStat {
                tag,
                samples,
                win_rate: if samples > 0 {
                    wins as f64 / samples as f64
                } else {
                    0.0
                },
                win_rate_low: low,
                win_rate_high: high,
                win_rate_margin: margin,
                samples_sufficient: sufficient,
            }
        })
        .collect();
    tag_stats.sort_by(|a, b| b.samples.cmp(&a.samples));
    tag_stats.truncate(6);

    // 真实执行口径统计（从 meta.outcome 提取）
    let exec_rows: Vec<String> = sqlx::query_scalar(
        "SELECT meta FROM daily_pick \
         WHERE date >= ? AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL",
    )
    .bind(&since)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let mut gaps: Vec<f64> = Vec::new();
    let mut t1_reals: Vec<f64> = Vec::new();
    let mut dds: Vec<f64> = Vec::new();
    let mut wins: Vec<f64> = Vec::new();
    let mut losses: Vec<f64> = Vec::new();
    for meta in &exec_rows {
        if let Ok(v) = serde_json::from_str::<Value>(meta) {
            let o = &v["outcome"];
            if let Some(g) = o["entry_gap"].as_f64() {
                gaps.push(g);
            }
            if let Some(r) = o["t1_real"].as_f64() {
                t1_reals.push(r);
                if r > 0.0 {
                    wins.push(r);
                } else {
                    losses.push(r);
                }
            }
            if let Some(d) = o["max_dd"].as_f64() {
                dds.push(d);
            }
        }
    }
    let n = t1_reals.len() as i64;
    let avg = |v: &[f64]| {
        if v.is_empty() {
            0.0
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let real_hits = t1_reals.iter().filter(|r| **r > 0.0).count() as i64;
    let (real_low, real_high, real_margin, real_sufficient) = confidence_bounds(n, real_hits);
    let execution = crate::model::pick::ExecutionStats {
        avg_entry_gap: avg(&gaps),
        t1_real_win_rate: if n > 0 {
            real_hits as f64 / n as f64
        } else {
            0.0
        },
        t1_real_win_rate_low: real_low,
        t1_real_win_rate_high: real_high,
        t1_real_win_rate_margin: real_margin,
        t1_real_samples_sufficient: real_sufficient,
        avg_t1_real: avg(&t1_reals),
        avg_max_dd: avg(&dds),
        win_loss_ratio: {
            let w = avg(&wins);
            let l = avg(&losses).abs();
            if l > 0.0 {
                w / l
            } else if w > 0.0 {
                99.0
            } else {
                0.0
            }
        },
    };

    // market 取首条 meta.us（生成时统一写入）
    let market = picks
        .first()
        .and_then(|p| p.meta.get("us").cloned())
        .unwrap_or(serde_json::Value::Null);
    let previous_picks = fetch_previous_picks(
        db,
        NaiveDate::parse_from_str(&date_key, "%Y-%m-%d").unwrap_or_else(|_| Utc::now().date_naive()),
    )
    .await
    .unwrap_or_default();
    Ok(PicksDocument {
        date: date_key,
        picks,
        stats: PickStats {
            samples: t1_samples,
            t1_win_rate: if t1_samples > 0 {
                t1_win / t1_samples as f64
            } else {
                0.0
            },
            t1_win_rate_low: t1_low,
            t1_win_rate_high: t1_high,
            t1_win_rate_margin: t1_margin,
            t1_samples_sufficient: t1_sufficient,
            avg_t1_pct: if t1_samples > 0 {
                t1_sum / t1_samples as f64
            } else {
                0.0
            },
            t5_samples,
            t5_win_rate: if t5_samples > 0 {
                t5_win / t5_samples as f64
            } else {
                0.0
            },
            t5_win_rate_low: t5_low,
            t5_win_rate_high: t5_high,
            t5_win_rate_margin: t5_margin,
            t5_samples_sufficient: t5_sufficient,
            avg_t5_pct: if t5_samples > 0 {
                t5_sum / t5_samples as f64
            } else {
                0.0
            },
            tags: tag_stats,
            execution,
        },
        market,
        execute_hint: String::new(),
        previous_picks,
    })
}

/// 渐进回测回写：下一交易日先回写 T+1，第 5 个交易日后再补 T+5。
/// 基准价优先使用生成时的尾盘候选价 `meta.close`，更贴近实际可买价。
pub async fn backfill_outcomes(state: &AppState) -> Result<usize, PickError> {
    let today = Utc::now()
        .with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap())
        .date_naive();
    let today_key = today.format("%Y-%m-%d").to_string();
    let t5_cutoff = (today - Duration::days(7)).format("%Y-%m-%d").to_string();
    let since = (today - Duration::days(30)).format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT date, code FROM daily_pick \
         WHERE date >= ? AND date < ? AND (\
           json_extract(meta,'$.outcome.t1_pct') IS NULL OR \
           json_extract(meta,'$.outcome.entry_open') IS NULL OR \
           (date <= ? AND json_extract(meta,'$.outcome.t5_pct') IS NULL)\
         ) \
         ORDER BY date DESC",
    )
    .bind(&since)
    .bind(&today_key)
    .bind(&t5_cutoff)
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
        let meta: Value = sqlx::query_scalar("SELECT meta FROM daily_pick WHERE date=? AND code=?")
            .bind(date)
            .bind(code)
            .fetch_one(&state.db)
            .await
            .and_then(|m: String| Ok(serde_json::from_str(&m).unwrap_or_default()))
            .unwrap_or_default();
        let mut meta = meta;
        let base = meta["close"].as_f64().unwrap_or(bars[index].close);
        if base <= 0.0 {
            continue;
        }
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        let map = meta.as_object_mut().expect("meta normalized to object");
        let outcome = map
            .entry("outcome")
            .or_insert_with(|| serde_json::json!({}));
        if !outcome.is_object() {
            *outcome = serde_json::json!({});
        }
        let outcome_map = outcome
            .as_object_mut()
            .expect("outcome normalized to object");
        let mut changed = false;
        // 真实执行价：次日开盘（用户实际能买到的价格，涨停股尤其重要）
        if outcome_map
            .get("entry_open")
            .and_then(Value::as_f64)
            .is_none()
            && index + 1 < bars.len()
        {
            let entry_open = bars[index + 1].open;
            if entry_open > 0.0 {
                let gap = (entry_open - base) / base * 100.0;
                outcome_map.insert("entry_open".into(), serde_json::json!(entry_open));
                outcome_map.insert("entry_gap".into(), serde_json::json!(gap));
                changed = true;
            }
        }
        let entry = outcome_map
            .get("entry_open")
            .and_then(Value::as_f64)
            .unwrap_or(base);
        if outcome_map.get("t1_pct").and_then(Value::as_f64).is_none() && index + 1 < bars.len() {
            let t1 = (bars[index + 1].close - base) / base * 100.0;
            let t1r = (bars[index + 1].close - entry) / entry * 100.0;
            outcome_map.insert("t1_pct".into(), serde_json::json!(t1));
            outcome_map.insert("t1_real".into(), serde_json::json!(t1r));
            changed = true;
        }
        if outcome_map.get("t5_pct").and_then(Value::as_f64).is_none() && index + 5 < bars.len() {
            let t5 = (bars[index + 5].close - base) / base * 100.0;
            let t5r = (bars[index + 5].close - entry) / entry * 100.0;
            outcome_map.insert("t5_pct".into(), serde_json::json!(t5));
            outcome_map.insert("t5_real".into(), serde_json::json!(t5r));
            // 持有期最大回撤（T+1 到 T+5 期间最低价 vs 真实买入价）
            let min_low = bars[index + 1..=index + 5.min(bars.len() - 1)]
                .iter()
                .map(|b| b.low)
                .filter(|l| *l > 0.0)
                .fold(f64::MAX, f64::min);
            if min_low < f64::MAX && entry > 0.0 {
                let dd = (min_low - entry) / entry * 100.0;
                outcome_map.insert("max_dd".into(), serde_json::json!(dd));
            }
            changed = true;
        }
        if !changed {
            continue;
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
    fn us_index_parse_and_mood() {
        let body = r#"v_usDJI="200~DJI~.DJI~51682.64~51778.04~51826.78~858494006~0~0~51589.80~";
v_usIXIC="200~IXIC~.IXIC~26522.55~26418.30~26522.09~12789451846";"#;
        let us = parse_us_index(body);
        assert!((us.djia_pct.unwrap() + 0.184).abs() < 0.01);
        assert!((us.ixic_pct.unwrap() - 0.395).abs() < 0.01);
        // 均值 ~0.1 → 无情绪标签
        assert_eq!(us.mood_score().1, None);

        let bear = UsSentiment {
            djia_pct: Some(-1.5),
            ixic_pct: Some(-2.0),
        };
        let (score, tag) = bear.mood_score();
        assert_eq!(score, -10.0);
        assert_eq!(tag.as_deref(), Some("隔夜美股偏空"));
        assert!(bear.semis_drag());

        let bull = UsSentiment {
            djia_pct: Some(2.0),
            ixic_pct: None,
        };
        assert_eq!(bull.mood_score().0, 10.0);
        assert!(!bull.semis_drag());
        assert_eq!(UsSentiment::default().mood_score().0, 0.0);
    }

    #[test]
    fn industry_momentum_marks_strong_and_weak() {
        let candidates: Vec<Candidate> = ["半导体", "半导体", "白酒", "银行"]
            .iter()
            .enumerate()
            .flat_map(|(i, industry)| {
                [
                    Candidate {
                        code: format!("sz30000{i}"),
                        name: "甲".into(),
                        price: 10.0,
                        pct: if *industry == "半导体" { 5.0 } else { -1.0 },
                        industry: industry.to_string(),
                        is_limit_up: false,
                    },
                    Candidate {
                        code: format!("sz30001{i}"),
                        name: "乙".into(),
                        price: 10.0,
                        pct: if *industry == "半导体" { 3.0 } else { 0.5 },
                        industry: industry.to_string(),
                        is_limit_up: false,
                    },
                ]
            })
            .collect();
        let (avg, strong) = industry_momentum(&candidates);
        assert_eq!(avg.get("半导体").copied(), Some(4.0));
        assert!(
            strong.contains(&"半导体".to_string()),
            "强势 = Top5 且均值 ≥2"
        );
        assert_eq!(strong.len(), 1, "白酒 0.5 / 银行 -0.25 不入强势");
        assert!(
            avg.get("银行").copied().unwrap_or(0.0) < 0.0,
            "银行均值 ≤0 减分候选"
        );
    }

    #[test]
    fn news_keywords_score_positive_and_negative() {
        let items = vec![
            crate::model::NewsItem {
                code: "sz300623".into(),
                title: "公司中标 3.2 亿元订单".into(),
                summary: "".into(),
                media: "证券时报".into(),
                url: "https://x".into(),
                published_at: Utc::now(),
            },
            crate::model::NewsItem {
                code: "sz300623".into(),
                title: "股东减持计划".into(),
                summary: "拟减持不超过 2%".into(),
                media: "公告".into(),
                url: "https://y".into(),
                published_at: Utc::now(),
            },
        ];
        // 中标 + 订单 = +10；减持 −8 → 净 +2
        let (score, pos_tag, neg_tags, _) = news_keyword_score(&items);
        assert_eq!(score, 2.0);
        assert_eq!(pos_tag.as_deref(), Some("利好新闻"));
        assert!(neg_tags.contains(&"股东减持".to_string()));
        // 纯利空封顶
        let bad: Vec<crate::model::NewsItem> = (0..5).map(|_| items[1].clone()).collect();
        let (score2, _pos2, neg_tags2, _) = news_keyword_score(&bad);
        assert_eq!(score2, -25.0, "负分封顶 -25");
        assert!(neg_tags2.contains(&"股东减持".to_string()));
        // 无命中
        let empty = vec![crate::model::NewsItem {
            code: "x".into(),
            title: "例行公告".into(),
            summary: "".into(),
            media: "".into(),
            url: "".into(),
            published_at: Utc::now(),
        }];
        assert_eq!(news_keyword_score(&empty).0, 0.0);
    }

    #[test]
    fn main_net_score_brackets() {
        // 大档：流入 >1亿 +25 / 占比 ≥10% +5 = +30（封顶）
        let (score, tag) = main_net_score(15_000.0, 12.0);
        assert_eq!(score, 30.0, "流入大 + 占比强，封顶 +30");
        assert_eq!(tag.as_deref(), Some("主力抢筹"));

        // 流出 >1亿 + 占比 ≤-10% = -30
        let (score, tag) = main_net_score(-12_000.0, -11.0);
        assert_eq!(score, -30.0, "流出大 + 占比负，封顶 -30");
        assert_eq!(tag.as_deref(), Some("主力出逃"));

        // 中档：5000-1亿 +15
        let (score, tag) = main_net_score(6_500.0, 0.0);
        assert_eq!(score, 15.0);
        assert_eq!(tag.as_deref(), Some("主力流入"));

        // 小档：1000-5000万 +8
        let (score, tag) = main_net_score(2_000.0, 0.0);
        assert_eq!(score, 8.0);
        assert_eq!(tag.as_deref(), Some("主力流入"));

        // 噪声档：<100 万中性
        let (score, tag) = main_net_score(50.0, 0.0);
        assert_eq!(score, 0.0);
        assert_eq!(tag, None);

        // 流出对称：-2000 万 -8 / 「主力流出」
        let (score, tag) = main_net_score(-2_000.0, 0.0);
        assert_eq!(score, -8.0);
        assert_eq!(tag.as_deref(), Some("主力流出"));
    }

    #[test]
    fn learned_weights_require_enough_samples_and_are_bounded() {
        assert_eq!(learned_tag_delta(29, 1.0), 0.0, "30 样本前不调权");
        assert!((learned_tag_delta(30, 0.7) - 4.0).abs() < 1e-9);
        assert!((learned_tag_delta(60, 0.7) - 8.0).abs() < 1e-9);
        assert_eq!(learned_tag_delta(100, 1.0), 10.0, "单标签上限 +10");
        assert_eq!(learned_tag_delta(100, 0.0), -10.0, "单标签下限 -10");

        let performance = HashMap::from([
            (
                "MACD金叉".to_string(),
                TagPerformance {
                    samples: 60,
                    win_rate: 0.8,
                },
            ),
            (
                "放量".to_string(),
                TagPerformance {
                    samples: 80,
                    win_rate: 0.9,
                },
            ),
            (
                "样本少".to_string(),
                TagPerformance {
                    samples: 12,
                    win_rate: 1.0,
                },
            ),
        ]);
        let tags = vec!["MACD金叉".into(), "放量".into(), "样本少".into()];
        let (adjustment, evidence) = learned_score_adjustment(&tags, &performance);
        assert_eq!(adjustment, 20.0, "多标签总调整上限 +20");
        assert_eq!(
            evidence["tags"].as_array().unwrap().len(),
            2,
            "样本不足不进证据"
        );
        assert_eq!(evidence["minimum_samples"], 30);
    }

    #[test]
    fn trade_plan_marks_entry_timing_and_strategy() {
        use chrono::TimeZone;
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 9, 22).unwrap();
        let before_close = tz.with_ymd_and_hms(2026, 9, 22, 14, 46, 0).unwrap();
        let short = build_trade_plan(
            date,
            before_close,
            &["放量".into()],
            false,
            false,
            10.0,
            None,
            9.8,
            10.2,
        );
        assert_eq!(short["entry_timing"], "today_close");
        assert_eq!(short["strategy"], "短线");
        assert_eq!(short["target"], "T+1");
        assert_eq!(short["entry_label"], "今日尾盘");
        // §A.9 价格区间：close=10 套 ±2% → [9.80, 10.20]；无 sell_zone 数据 fallback 1.2%
        assert!((short["buy_price_low"].as_f64().unwrap() - 9.80).abs() < 1e-9);
        assert!((short["buy_price_high"].as_f64().unwrap() - 10.20).abs() < 1e-9);
        assert!((short["buy_basis_close"].as_f64().unwrap() - 10.00).abs() < 1e-9);
        let sell_low = short["sell_price_low"].as_f64().unwrap();
        let sell_high = short["sell_price_high"].as_f64().unwrap();
        assert!(sell_low > 10.07 && sell_low < 10.18, "sell_low={sell_low}");
        assert!(sell_high > sell_low, "sell_high={sell_high}");

        let with_base = build_trade_plan(
            date,
            before_close,
            &["放量".into()],
            true,
            false,
            20.0,
            Some((21.0, 21.5)),
            19.6,
            20.4,
        );
        assert_eq!(with_base["strategy"], "做T");
        assert_eq!(with_base["has_base_position"], true);
        // sell_zone 显式提供时直接采纳
        assert!((with_base["sell_price_low"].as_f64().unwrap() - 21.0).abs() < 1e-9);
        assert!((with_base["sell_price_high"].as_f64().unwrap() - 21.5).abs() < 1e-9);

        let after_close = tz.with_ymd_and_hms(2026, 9, 22, 15, 10, 0).unwrap();
        let late = build_trade_plan(
            date,
            after_close,
            &["均线多头".into()],
            false,
            false,
            30.0,
            None,
            29.4,
            30.6,
        );
        assert_eq!(late["entry_timing"], "next_session_pullback");
        assert_eq!(late["strategy"], "中线");

        // 今日已涨停：不论窗口是否 14:45 前，强制「次日开盘」
        let locked = build_trade_plan(
            date,
            before_close,
            &["放量".into()],
            false,
            true,
            42.0,
            None,
            41.16,
            42.84,
        );
        assert_eq!(locked["entry_timing"], "next_session_open");
        assert_eq!(locked["entry_label"], "次日开盘");
        assert_eq!(locked["entry_window"], "09:30-09:35");
        assert_eq!(locked["is_limit_up"], true);
        let locked_late = build_trade_plan(
            date,
            after_close,
            &["放量".into()],
            false,
            true,
            42.0,
            None,
            41.16,
            42.84,
        );
        assert_eq!(locked_late["entry_label"], "次日开盘");
    }

    #[test]
    fn kdj_and_boll_extend_tags() {
        let (closes, highs, lows, volumes) = closes_up();
        let tech = score_bars(&closes, &highs, &lows, &volumes).expect("上行序列应可评分");
        assert!(
            tech.tags.contains(&"中轨上方".to_string()),
            "上行序列应在 BOLL 中轨上方：{:?}",
            tech.tags
        );
        // KDJ 序列长度与取值范围（交替回调末端 K/D 关系不保证，只验边界）
        let (k, d) = kdj_series(&highs, &lows, &closes);
        assert_eq!(k.len(), closes.len());
        assert!((0.0..=100.0).contains(k.last().unwrap()));
        assert!((0.0..=100.0).contains(d.last().unwrap()));
    }

    #[test]
    fn sina_rows_filter_bj_st_and_parse_price() {
        let rows = vec![
            SinaRow {
                symbol: "sz300623".into(),
                name: "捷捷微电".into(),
                trade: Some("35.110".into()),
                changepercent: Some(2.8),
            },
            SinaRow {
                symbol: "bj920427".into(),
                name: "华维设计".into(),
                trade: Some("11.030".into()),
                changepercent: Some(29.9),
            },
            SinaRow {
                symbol: "sh600000".into(),
                name: "ST 测试".into(),
                trade: Some("1.0".into()),
                changepercent: Some(5.0),
            },
            SinaRow {
                symbol: "sh600001".into(),
                name: "坏价格".into(),
                trade: Some("abc".into()),
                changepercent: Some(5.0),
            },
        ];
        let out = filter_sina_rows(rows);
        assert_eq!(out.len(), 1, "bj / ST / 坏价格都剔除");
        assert_eq!(out[0].code, "sz300623");
        assert_eq!(out[0].price, 35.110);
        assert_eq!(out[0].pct, 2.8);
        assert!(out[0].industry.is_empty(), "无行业字段，板块动量自动降级");
    }

    #[test]
    fn market_prefix_maps_codes() {
        assert_eq!(market_prefix("600519"), "sh");
        assert_eq!(market_prefix("300623"), "sz");
    }

    #[test]
    fn is_limit_up_respects_board_and_tolerance() {
        // 主板：9.5/10 容差
        assert!(is_limit_up("sh600519", 9.5));
        assert!(is_limit_up("sh600519", 10.0));
        assert!(!is_limit_up("sh600519", 9.4));
        // 创业板/科创板：19.5/20 容差
        assert!(is_limit_up("sz300623", 19.5));
        assert!(is_limit_up("sz300623", 20.0));
        assert!(!is_limit_up("sz300623", 19.4));
        assert!(!is_limit_up("sh688111", 9.5));
    }

    #[test]
    fn is_anomaly_uses_board_threshold() {
        // 主板阈值 7%，创业/科创 17%
        assert!(is_anomaly("sh600519", 7.0));
        assert!(!is_anomaly("sh600519", 6.9));
        assert!(is_anomaly("sz300623", 17.0));
        assert!(!is_anomaly("sz300623", 16.5));
    }

    fn sample_news(title: &str, summary: &str) -> crate::model::NewsItem {
        crate::model::NewsItem {
            code: "sz300623".into(),
            title: title.into(),
            summary: summary.into(),
            media: "证券时报".into(),
            url: format!("https://news.example.com/{}", title),
            published_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn negative_signal_score_classifies_each_keyword() {
        let cases = [
            ("股东减持计划", vec!["股东减持"]),
            ("收到问询函", vec!["收到问询函"]),
            ("被立案调查", vec!["被立案调查"]),
            ("监管处罚决定", vec!["监管处罚"]),
            ("存在退市风险", vec!["退市风险"]),
        ];
        for (kw, expected) in cases {
            let (score, cats) = negative_signal_score(kw);
            assert!(score < 0.0, "{kw} 应扣分，得 {score}");
            let names: Vec<&str> = cats.iter().map(|(n, _)| n.as_str()).collect();
            for tag in expected {
                assert!(names.contains(&tag), "{kw} 应打 {tag}，得 {names:?}");
            }
        }
        // 命中多条关键词累加
        let (score, cats) = negative_signal_score("减持 + 立案 + 处罚");
        assert_eq!(cats.len(), 3);
        assert_eq!(score, -8.0 + -10.0 + -10.0);
    }

    #[test]
    fn wilson_interval_brackets_and_zero_handling() {
        // 0/N 全部命中 → 区间下界 > 0
        let (low, high, margin) = wilson_interval(10, 10, 1.96);
        assert!(low > 0.65, "10/10 下界应 > 0.65：{low}");
        assert!(high > 0.99);
        assert!(margin > 0.0);

        // 0/N 全部失败
        let (low, high, margin) = wilson_interval(10, 0, 1.96);
        assert!(high < 0.35, "0/10 上界应 < 0.35：{high}");
        assert!(margin > 0.0);

        // 7/10 = 0.7 经典误读场景。Wilson 95% 实际给出约 (0.398, 0.892)——
// 与「用 N 算 ±√(p(1-p)/N) ≈ 0.46」相比，区间偏窄但下界依旧低于 0.5。
        let (low, high, _m) = wilson_interval(10, 7, 1.96);
        assert!(low < 0.45, "7/10 下界应 < 0.45：{low}");
        assert!(high > 0.85, "7/10 上界应 > 0.85：{high}");
        assert!(low < 0.5, "下界应低于 0.5（说明 70% 胜率无统计意义）：{low}");

        // 30 样本时区间收窄
        let (low30, high30, _) = wilson_interval(30, 21, 1.96);
        let (low10, high10, _) = wilson_interval(10, 7, 1.96);
        assert!(high30 - low30 < high10 - low10, "30 样本区间更窄");

        // 0 样本：返回 (0,0,0)
        assert_eq!(wilson_interval(0, 0, 1.96), (0.0, 0.0, 0.0));

        // 充分性阈值 = 30
        assert!(!samples_sufficient(29));
        assert!(samples_sufficient(30));
    }

    #[test]
    fn news_keyword_score_returns_separated_positive_and_negative_tags() {
        let items = vec![
            sample_news("公司中标3亿元订单", "利好落地"),
            sample_news("股东减持2%", "拟减持"),
            sample_news("收到问询函", "关注函"),
        ];
        let (score, pos_tag, neg_tags, signals) = news_keyword_score(&items);
        // 中标 +5 + 订单 +5 = +10；减持 -8 + 问询 -5 = -13 → 净 -3
        assert_eq!(score, -3.0);
        assert_eq!(pos_tag.as_deref(), Some("利好新闻"));
        assert!(neg_tags.contains(&"股东减持".to_string()));
        assert!(neg_tags.contains(&"收到问询函".to_string()));
        assert_eq!(neg_tags.len(), 2, "不重复打相同 tag：{neg_tags:?}");
        assert_eq!(signals.len(), 2, "两条原文标题都透出");
        // 极端负向：单条同时命中「减持 + 立案」累计 -18，clamp 单条 -15
        let extreme = vec![sample_news("减持 + 立案 + 处罚", "全部命中")];
        let (score, _, _, _) = news_keyword_score(&extreme);
        assert_eq!(score, -15.0, "单条负面封底 -15");
    }
}
