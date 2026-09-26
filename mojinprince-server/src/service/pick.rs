//! 智能推荐（四层漏斗）：
//! ① 东财涨幅榜 Top100 候选 → ② 技术指标量化粗筛（纯函数可单测）
//! → ③ 研报评级 / 新闻热度加分（查既有缓存表）→ ④ AI 精排（客户端
//! 透传配置触发，密钥不落库；调度器自动版为纯量化）。
//!
//! 落库 `daily_pick`，按 (date, code) 主键；T+1/T+5 收盘对照回写
//! `meta.outcome` 形成命中率闭环。
use crate::error::AppError;
use crate::model::pick::{
    AiRankConfig, CalibrationBin, CalibrationQuality, DailyPick, HardFilterStat, HealthCurvePoint,
    HealthWindow, PauseCount, PickAudit, PickStats, PickTagStat, PicksDocument, RegimeStat,
    ShadowExperimentStat, StrategyHealth,
};
use crate::model::DayBar;
use crate::service::ai::{self, ChatMessage};
use crate::service::ingest::MainNetSnapshot;
use crate::state::AppState;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Timelike, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
                turnover_pct: None,
                volume_ratio: None,
                pool_sources: vec!["momentum".into()],
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
    /// 当日换手率（%）；新浪 fallback 无该字段时为 None。
    pub turnover_pct: Option<f64>,
    /// 当日量比；上游缺失时由近期日 K 成交量估算。
    pub volume_ratio: Option<f64>,
    /// momentum / relative_strength / pullback；最终可包含多个来源。
    pub pool_sources: Vec<String>,
}

impl EastMoneyRanking {
    async fn fetch_sorted(
        &self,
        http: &reqwest::Client,
        limit: usize,
        fid: &str,
        source: &str,
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
                ("fid", fid),
                ("fs", "m:0+t:6,m:0+t:80,m:1+t:2,m:1+t:23"),
                ("fields", "f2,f3,f8,f10,f12,f14,f100"),
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
                turnover_pct: row["f8"].as_f64(),
                volume_ratio: row["f10"].as_f64(),
                pool_sources: vec![source.to_string()],
            });
        }
        Ok(out)
    }

    /// §A.12 多源候选：涨幅动量 + 活跃换手池。第二源失败时保留原涨幅榜降级路径。
    pub async fn fetch(
        &self,
        http: &reqwest::Client,
        limit: usize,
    ) -> Result<Vec<Candidate>, PickError> {
        let momentum = self.fetch_sorted(http, limit, "f3", "momentum").await?;
        let active = self
            .fetch_sorted(http, limit, "f8", "active")
            .await
            .unwrap_or_default();
        let mut merged: Vec<Candidate> = Vec::with_capacity(momentum.len() + active.len());
        let mut positions: HashMap<String, usize> = HashMap::new();
        for candidate in momentum.into_iter().chain(active) {
            if let Some(index) = positions.get(&candidate.code).copied() {
                for source in candidate.pool_sources {
                    if !merged[index].pool_sources.contains(&source) {
                        merged[index].pool_sources.push(source);
                    }
                }
            } else {
                positions.insert(candidate.code.clone(), merged.len());
                merged.push(candidate);
            }
        }
        Ok(merged)
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
// §A.11 P0 涨停强弱分档：只使用可复现的行情证据，不让「涨停」天然获得正向加分。
// ============================================================================

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
struct LimitUpStrength {
    tier: &'static str,
    score_delta: f64,
    prior_streak: usize,
    turnover_pct: Option<f64>,
    volume_ratio: Option<f64>,
    strong_industry: bool,
    evidence: Vec<String>,
}

fn classify_limit_up_strength(
    candidate: &Candidate,
    bars: &[DayBar],
    strong_industry: bool,
) -> LimitUpStrength {
    let mut prior_streak = 0usize;
    for pair in bars.windows(2).rev() {
        let previous = pair[0].close;
        let current = pair[1].close;
        if previous <= 0.0 {
            break;
        }
        let pct = (current - previous) / previous * 100.0;
        if is_limit_up(&candidate.code, pct) {
            prior_streak += 1;
        } else {
            break;
        }
    }
    let estimated_volume_ratio = if bars.len() >= 6 {
        let last = bars.last().map(|bar| bar.volume as f64).unwrap_or(0.0);
        let previous = &bars[bars.len() - 6..bars.len() - 1];
        let average =
            previous.iter().map(|bar| bar.volume as f64).sum::<f64>() / previous.len() as f64;
        (average > 0.0).then_some(last / average)
    } else {
        None
    };
    let volume_ratio = candidate.volume_ratio.or(estimated_volume_ratio);

    let turnover_healthy = candidate
        .turnover_pct
        .map(|turnover| (3.0..=25.0).contains(&turnover))
        .unwrap_or(false);
    let volume_healthy = volume_ratio
        .map(|ratio| (0.8..=4.0).contains(&ratio))
        .unwrap_or(false);
    let mut evidence = Vec::new();
    let mut strength_points = 0usize;
    if prior_streak > 0 {
        strength_points += 2;
        evidence.push(format!("前序连板 {prior_streak} 天"));
    }
    if strong_industry {
        strength_points += 1;
        evidence.push("强势行业共振".into());
    }
    if turnover_healthy {
        strength_points += 1;
        evidence.push("换手充分".into());
    }
    if volume_healthy {
        strength_points += 1;
        evidence.push("量能健康".into());
    }

    let weak_liquidity = candidate
        .turnover_pct
        .map(|turnover| !(1.0..=30.0).contains(&turnover))
        .unwrap_or(false)
        || volume_ratio
            .map(|ratio| !(0.6..=5.0).contains(&ratio))
            .unwrap_or(false);
    let (tier, score_delta) = if strength_points >= 3 {
        ("strong", 0.0)
    } else if strength_points <= 1 || weak_liquidity {
        ("weak", -12.0)
    } else {
        ("neutral", -5.0)
    };
    if evidence.is_empty() {
        evidence.push("缺少连板/行业/换手确认".into());
    }
    LimitUpStrength {
        tier,
        score_delta,
        prior_streak,
        turnover_pct: candidate.turnover_pct,
        volume_ratio,
        strong_industry,
        evidence,
    }
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
// §A.10 A 股大盘情绪（腾讯 qt.gtimg.cn 同源接口，零新增网络栈）
// ============================================================================

/// 同 TencentUsIndex，复用 qt.gtimg.cn 的 `q=` 多 symbol 语法拉上证 / 深成 / 创业板 / 沪深 300。
#[derive(Debug, Clone)]
pub struct TencentCnIndex {
    /// 测试可覆写（默认 `http://qt.gtimg.cn`）
    pub base_url: String,
}

impl Default for TencentCnIndex {
    fn default() -> Self {
        Self {
            base_url: "http://qt.gtimg.cn".into(),
        }
    }
}

/// 主流指数当日涨跌幅（%）。失败时各字段为 None。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CnSentiment {
    pub sh_pct: Option<f64>,    // 上证综指 sh000001
    pub sz_pct: Option<f64>,    // 深证成指 sz399001
    pub gem_pct: Option<f64>,   // 创业板指 sz399006
    pub hs300_pct: Option<f64>, // 沪深 300 sh000300
}

impl TencentCnIndex {
    pub async fn fetch(&self, http: &reqwest::Client) -> Result<CnSentiment, PickError> {
        let url = format!("{}/q=sh000001,sz399001,sz399006,sh000300", self.base_url);
        let response = http
            .get(&url)
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
        let (decoded, _, _) = encoding_rs::GBK.decode(body.as_bytes());
        Ok(parse_cn_index(&decoded))
    }
}

/// 解析四只指数。腾讯字段布局：
/// 索引 0=市场类型, 1=名称, 2=代码, 3=现价, 4=昨收, 32=涨跌幅（%）
fn parse_cn_index(body: &str) -> CnSentiment {
    let mut out = CnSentiment::default();
    for line in body.lines() {
        let (Some(start), Some(end)) = (line.find('"'), line.rfind('"')) else {
            continue;
        };
        if end <= start {
            continue;
        }
        let fields: Vec<&str> = line[start + 1..end].split('~').collect();
        // 腾讯 qt 接口字段顺序：1~名称~代码~现~昨开~今开~vol~0~0~...~0~0~~yyyyMMddHHmmss~涨跌额~涨跌幅%
        // pct 在字段索引 30
        if fields.len() <= 30 {
            continue;
        }
        // pct 字段（索引 30）由腾讯直接给「涨跌幅（%）」，无需用现价/昨收重算
        let pct = fields[30].parse::<f64>().ok();
        match fields[2] {
            "000001" => out.sh_pct = pct,
            "399001" => out.sz_pct = pct,
            "399006" => out.gem_pct = pct,
            "000300" => out.hs300_pct = pct,
            _ => {}
        }
    }
    out
}

impl CnSentiment {
    /// §A.10 大盘 beta 过滤分（应用到所有候选的全局 delta）：
    ///
    /// 三指数均值 ≤ -2.0  → -30「大盘大跌」、应暂停推荐
    /// 三指数均值 ≤ -0.8  → -15「大盘偏弱」、所有短炒票加权
    /// 三指数均值 ≥ +1.0  → +10「大盘偏多」
    /// 其它区间           → 0
    pub fn risk_score(&self) -> (f64, Option<String>) {
        let values: Vec<f64> = [self.sh_pct, self.sz_pct, self.gem_pct, self.hs300_pct]
            .iter()
            .flatten()
            .copied()
            .collect();
        if values.is_empty() {
            return (0.0, None);
        }
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        if avg <= -2.0 {
            (-30.0, Some("大盘大跌".into()))
        } else if avg <= -0.8 {
            (-15.0, Some("大盘偏弱".into()))
        } else if avg >= 1.0 {
            (10.0, Some("大盘偏多".into()))
        } else {
            (0.0, None)
        }
    }

    /// 大盘三指数均值（用于元数据透出）
    pub fn avg_pct(&self) -> Option<f64> {
        let values: Vec<f64> = [self.sh_pct, self.sz_pct, self.gem_pct, self.hs300_pct]
            .iter()
            .flatten()
            .copied()
            .collect();
        if values.is_empty() {
            None
        } else {
            Some(values.iter().sum::<f64>() / values.len() as f64)
        }
    }

    /// §A.10 极端行情下应暂停推荐（返回 true）
    pub fn should_pause(&self) -> bool {
        self.avg_pct().map(|p| p <= -2.0).unwrap_or(false)
    }
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

fn select_diversified_candidates(
    candidates: &[Candidate],
    industry_avg: &HashMap<String, f64>,
    cn_avg_pct: Option<f64>,
    limit: usize,
) -> Vec<Candidate> {
    let defensive = cn_avg_pct.is_some_and(|pct| pct <= -0.8);
    let (momentum_quota, relative_quota, pullback_quota) = if defensive {
        (25usize, 40usize, 35usize)
    } else {
        (45usize, 30usize, 25usize)
    };
    let mut selected: Vec<Candidate> = Vec::with_capacity(limit);
    let mut seen = std::collections::HashSet::new();
    let push = |candidate: &Candidate,
                source: &str,
                selected: &mut Vec<Candidate>,
                seen: &mut std::collections::HashSet<String>| {
        if selected.len() >= limit || !seen.insert(candidate.code.clone()) {
            return;
        }
        let mut candidate = candidate.clone();
        if !candidate.pool_sources.iter().any(|item| item == source) {
            candidate.pool_sources.push(source.to_string());
        }
        selected.push(candidate);
    };

    let mut momentum: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .pool_sources
                .iter()
                .any(|source| source == "momentum")
        })
        .collect();
    momentum.sort_by(|a, b| {
        b.pct
            .partial_cmp(&a.pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for candidate in momentum.into_iter().take(momentum_quota) {
        push(candidate, "momentum", &mut selected, &mut seen);
    }

    let residual = |candidate: &Candidate| {
        candidate.pct
            - industry_avg
                .get(&candidate.industry)
                .copied()
                .unwrap_or(0.0)
    };
    let mut relative: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| residual(candidate) >= 0.5)
        .collect();
    relative.sort_by(|a, b| {
        residual(b)
            .partial_cmp(&residual(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for candidate in relative.into_iter().take(relative_quota) {
        push(candidate, "relative_strength", &mut selected, &mut seen);
    }

    let mut pullback: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate
                .pool_sources
                .iter()
                .any(|source| source == "active")
                && (-3.0..=3.0).contains(&candidate.pct)
                && candidate
                    .turnover_pct
                    .is_some_and(|turnover| (1.0..=20.0).contains(&turnover))
                && candidate
                    .volume_ratio
                    .is_none_or(|ratio| (0.6..=3.0).contains(&ratio))
        })
        .collect();
    pullback.sort_by(|a, b| {
        residual(b)
            .partial_cmp(&residual(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for candidate in pullback.into_iter().take(pullback_quota) {
        push(candidate, "pullback", &mut selected, &mut seen);
    }

    // 某一类样本不足时按残差强度递补，但不突破总上限。
    let mut fallback: Vec<&Candidate> = candidates.iter().collect();
    fallback.sort_by(|a, b| {
        residual(b)
            .partial_cmp(&residual(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for candidate in fallback {
        push(candidate, "fallback", &mut selected, &mut seen);
    }
    selected
}

fn push_with_industry_cap(
    picks: &mut Vec<(Candidate, f64, Vec<String>)>,
    item: &(Candidate, f64, Vec<String>),
    cap: usize,
) -> bool {
    if picks
        .iter()
        .any(|(candidate, _, _)| candidate.code == item.0.code)
    {
        return false;
    }
    if !item.0.industry.is_empty()
        && picks
            .iter()
            .filter(|(candidate, _, _)| candidate.industry == item.0.industry)
            .count()
            >= cap
    {
        return false;
    }
    picks.push(item.clone());
    true
}

fn push_with_risk_budget(
    picks: &mut Vec<(Candidate, f64, Vec<String>)>,
    item: &(Candidate, f64, Vec<String>),
    cap: usize,
    risk: &HashMap<String, StockRiskMetrics>,
    portfolio_floor: f64,
) -> bool {
    let current_stress: f64 = picks
        .iter()
        .map(|(candidate, _, _)| {
            risk.get(&candidate.code)
                .map(|metrics| metrics.stress_loss_market_down_2pct)
                .unwrap_or(portfolio_floor)
        })
        .sum();
    let candidate_stress = risk
        .get(&item.0.code)
        .map(|metrics| metrics.stress_loss_market_down_2pct)
        .unwrap_or(portfolio_floor);
    let prospective_average = (current_stress + candidate_stress) / (picks.len() + 1) as f64;
    if prospective_average < portfolio_floor {
        return false;
    }
    push_with_industry_cap(picks, item, cap)
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
    let positive_tag = if pos_hit {
        Some("利好新闻".to_string())
    } else {
        None
    };
    (total, positive_tag, neg_tags, neg_signals)
}

#[derive(Debug, Clone, PartialEq)]
struct CalendarHardBlock {
    reason: String,
    title: String,
    published_date: String,
    url: String,
}

/// §A.12 事件日历硬过滤。仅匹配已经发生或确定落地的事件措辞；
/// “拟减持/减持计划”等意向性公告仍沿用软扣分，避免关键词误杀。
fn calendar_hard_block(
    items: &[crate::model::NewsItem],
    as_of: NaiveDate,
) -> Option<CalendarHardBlock> {
    let cn = FixedOffset::east_opt(8 * 3600).expect("valid CN offset");
    for item in items {
        let event_date = item.published_at.with_timezone(&cn).date_naive();
        let age = as_of.signed_duration_since(event_date).num_days();
        if !(0..=7).contains(&age) {
            continue;
        }
        let text = format!("{}{}", item.title, item.summary);
        let reason = if ["被立案调查", "证监会立案", "立案告知书"]
            .iter()
            .any(|keyword| text.contains(keyword))
        {
            Some("监管立案")
        } else if ["终止上市", "退市风险警示", "暂停上市"]
            .iter()
            .any(|keyword| text.contains(keyword))
        {
            Some("退市风险")
        } else if ["限售股上市流通", "解除限售", "股份解禁"]
            .iter()
            .any(|keyword| text.contains(keyword))
        {
            Some("限售解禁")
        } else if ["定增缴款", "定向增发缴款", "认购款缴纳"]
            .iter()
            .any(|keyword| text.contains(keyword))
        {
            Some("定增缴款")
        } else if ["减持实施进展", "累计减持", "已减持", "减持完成"]
            .iter()
            .any(|keyword| text.contains(keyword))
        {
            Some("减持实施")
        } else if ["停牌核查", "复牌公告", "股票复牌"]
            .iter()
            .any(|keyword| text.contains(keyword))
            && age <= 1
        {
            Some("停复牌事件")
        } else if (text.contains("业绩预告") || text.contains("业绩快报"))
            && ["首亏", "续亏", "预亏", "大幅下降", "下修"]
                .iter()
                .any(|keyword| text.contains(keyword))
        {
            Some("业绩风险")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Some(CalendarHardBlock {
                reason: reason.to_string(),
                title: item.title.clone(),
                published_date: event_date.format("%Y-%m-%d").to_string(),
                url: item.url.clone(),
            });
        }
    }
    None
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
    wilson_low: f64,
    wilson_high: f64,
    recent_samples: i64,
    recent_win_rate: f64,
    prior_samples: i64,
    prior_win_rate: f64,
    regime_scope: String,
}

#[derive(Debug, Default, Clone, Copy)]
struct TagWindowBucket {
    recent_samples: i64,
    recent_wins: i64,
    prior_samples: i64,
    prior_wins: i64,
}

impl TagWindowBucket {
    fn performance(self, regime_scope: &str) -> TagPerformance {
        let samples = self.recent_samples + self.prior_samples;
        let wins = self.recent_wins + self.prior_wins;
        let (wilson_low, wilson_high, _) = wilson_interval(samples, wins, 1.96);
        TagPerformance {
            samples,
            win_rate: ratio(wins, samples),
            wilson_low,
            wilson_high,
            recent_samples: self.recent_samples,
            recent_win_rate: ratio(self.recent_wins, self.recent_samples),
            prior_samples: self.prior_samples,
            prior_win_rate: ratio(self.prior_wins, self.prior_samples),
            regime_scope: regime_scope.to_string(),
        }
    }
}

fn ratio(hits: i64, samples: i64) -> f64 {
    if samples > 0 {
        hits as f64 / samples as f64
    } else {
        0.0
    }
}

fn tag_performance_eligible(stat: &TagPerformance) -> bool {
    stat.samples >= 30
        && stat.recent_samples >= 8
        && stat.prior_samples >= 12
        && stat.win_rate.is_finite()
        && (stat.recent_win_rate - stat.prior_win_rate).abs() <= 0.20
}

/// Walk-forward 单标签调整：较早训练窗与最近验证窗必须方向稳定。
/// 正向边际取 Wilson 95% 下界、负向边际取上界；置信区间跨过 50% 时不调权。
/// 单标签最多 ±10 分，防止标签历史覆盖当日硬风控。
fn learned_tag_delta(stat: &TagPerformance) -> f64 {
    if !tag_performance_eligible(stat) {
        return 0.0;
    }
    let conservative_edge = if stat.wilson_low > 0.5 {
        stat.wilson_low - 0.5
    } else if stat.wilson_high < 0.5 {
        stat.wilson_high - 0.5
    } else {
        0.0
    };
    let confidence = (stat.samples as f64 / 80.0).clamp(0.375, 1.0);
    (conservative_edge * 40.0 * confidence).clamp(-10.0, 10.0)
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
        let delta = learned_tag_delta(stat);
        if delta.abs() < 0.01 {
            continue;
        }
        total += delta;
        evidence.push(serde_json::json!({
            "tag": tag,
            "samples": stat.samples,
            "win_rate": stat.win_rate,
            "wilson_low": stat.wilson_low,
            "wilson_high": stat.wilson_high,
            "recent_samples": stat.recent_samples,
            "recent_win_rate": stat.recent_win_rate,
            "prior_samples": stat.prior_samples,
            "prior_win_rate": stat.prior_win_rate,
            "regime_scope": stat.regime_scope,
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
            "recent_minimum_samples": 8,
            "prior_minimum_samples": 12,
            "lookback_days": 120,
            "validation_days": 30,
            "outcome_basis": "t1_real",
            "method": "walk_forward_wilson",
        }),
    )
}

/// 只使用推荐日之前、已经回写的真实成交口径 T+1 结果。
/// 最近 30 个自然日作为验证窗，之前 90 日作为训练窗；当前市场环境样本不足时
/// 逐标签回退到全市场统计，既避免未来泄漏，也避免 regime 小样本过拟合。
async fn load_tag_performance(
    db: &SqlitePool,
    as_of: NaiveDate,
    current_regime: Option<&str>,
) -> Result<HashMap<String, TagPerformance>, PickError> {
    let since = (as_of - Duration::days(120)).format("%Y-%m-%d").to_string();
    let validation_since = (as_of - Duration::days(30)).format("%Y-%m-%d").to_string();
    let before = as_of.format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT date, reasons, meta FROM daily_pick \
         WHERE date >= ? AND date < ? \
         AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL",
    )
    .bind(&since)
    .bind(&before)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let mut global: HashMap<String, TagWindowBucket> = HashMap::new();
    let mut by_regime: HashMap<String, HashMap<String, TagWindowBucket>> = HashMap::new();
    for (date, reasons, meta) in rows {
        let parsed = serde_json::from_str::<Value>(&meta).ok();
        let t1 = parsed
            .as_ref()
            .and_then(|m| m["outcome"]["t1_real"].as_f64());
        let Some(t1) = t1 else { continue };
        let regime = parsed
            .as_ref()
            .and_then(|m| m["cn"]["avg_pct"].as_f64())
            .map(|avg| market_regime(avg).0.to_string());
        let recent = date >= validation_since;
        let tags: Vec<String> = serde_json::from_str(&reasons).unwrap_or_default();
        for tag in tags {
            record_tag_outcome(global.entry(tag.clone()).or_default(), recent, t1 > 0.0);
            if let Some(regime) = &regime {
                record_tag_outcome(
                    by_regime
                        .entry(regime.clone())
                        .or_default()
                        .entry(tag)
                        .or_default(),
                    recent,
                    t1 > 0.0,
                );
            }
        }
    }
    let regime_bucket = current_regime.and_then(|regime| by_regime.get(regime));
    Ok(global
        .into_iter()
        .map(|(tag, bucket)| {
            let global_stat = bucket.performance("all");
            let chosen = regime_bucket
                .and_then(|items| items.get(&tag).copied())
                .map(|bucket| bucket.performance(current_regime.unwrap_or("all")))
                .filter(tag_performance_eligible)
                .unwrap_or(global_stat);
            (tag, chosen)
        })
        .collect())
}

fn record_tag_outcome(bucket: &mut TagWindowBucket, recent: bool, won: bool) {
    if recent {
        bucket.recent_samples += 1;
        bucket.recent_wins += i64::from(won);
    } else {
        bucket.prior_samples += 1;
        bucket.prior_wins += i64::from(won);
    }
}

#[derive(Debug, Clone)]
struct CalibrationSample {
    tags: BTreeSet<String>,
    regime: Option<String>,
    won: bool,
    target_hit_5pct: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
struct CalibrationEvidence {
    samples: i64,
    wins: i64,
    posterior_win_probability: f64,
    wilson_low: f64,
    wilson_high: f64,
    target_samples: i64,
    target_hits: i64,
    target_posterior_probability: f64,
    scope: String,
    confidence_tier: String,
    score_delta: f64,
    abstain: bool,
    positive_boost_enabled: bool,
}

impl CalibrationEvidence {
    fn as_json(&self) -> Value {
        serde_json::json!({
            "samples": self.samples,
            "wins": self.wins,
            "posterior_win_probability": self.posterior_win_probability,
            "wilson_low": self.wilson_low,
            "wilson_high": self.wilson_high,
            "target_5pct_samples": self.target_samples,
            "target_5pct_hits": self.target_hits,
            "target_5pct_posterior_probability": self.target_posterior_probability,
            "scope": self.scope,
            "confidence_tier": self.confidence_tier,
            "score_delta": self.score_delta,
            "abstain": self.abstain,
            "positive_boost_enabled": self.positive_boost_enabled,
            "method": "similar_tags_beta_wilson_v1",
            "minimum_samples": 30,
            "similarity_floor": 0.35,
            "minimum_shared_tags": 2,
            "prior": "Beta(2,2)",
        })
    }
}

fn calibration_tags(tags: &[String]) -> BTreeSet<String> {
    tags.iter()
        .filter(|tag| {
            !tag.starts_with("T+1达5%率")
                && !tag.starts_with("影子候选")
                && !matches!(
                    tag.as_str(),
                    "大盘偏多" | "大盘偏弱" | "大盘大跌" | "美股偏多" | "美股偏空"
                )
        })
        .cloned()
        .collect()
}

fn similar_calibration_samples<'a>(
    candidate_tags: &BTreeSet<String>,
    samples: &'a [CalibrationSample],
    regime: Option<&str>,
) -> Vec<&'a CalibrationSample> {
    samples
        .iter()
        .filter(|sample| regime.is_none_or(|key| sample.regime.as_deref() == Some(key)))
        .filter(|sample| {
            let shared = candidate_tags.intersection(&sample.tags).count();
            let union = candidate_tags.union(&sample.tags).count();
            shared >= 2 && union > 0 && shared as f64 / union as f64 >= 0.35
        })
        .collect()
}

fn calibrate_candidate(
    tags: &[String],
    history: &[CalibrationSample],
    current_regime: Option<&str>,
    positive_boost_enabled: bool,
) -> CalibrationEvidence {
    let tags = calibration_tags(tags);
    let regime_matches = similar_calibration_samples(&tags, history, current_regime);
    let (matches, scope) = if regime_matches.len() >= 30 {
        (regime_matches, current_regime.unwrap_or("all").to_string())
    } else {
        (similar_calibration_samples(&tags, history, None), "all".into())
    };
    let samples = matches.len() as i64;
    let wins = matches.iter().filter(|sample| sample.won).count() as i64;
    let target_samples = matches
        .iter()
        .filter(|sample| sample.target_hit_5pct.is_some())
        .count() as i64;
    let target_hits = matches
        .iter()
        .filter(|sample| sample.target_hit_5pct == Some(true))
        .count() as i64;
    let posterior = (wins as f64 + 2.0) / (samples as f64 + 4.0);
    let target_posterior = (target_hits as f64 + 2.0) / (target_samples as f64 + 4.0);
    let (wilson_low, wilson_high, _) = wilson_interval(samples, wins, 1.96);
    let sufficient = samples >= 30;
    let abstain = sufficient && wilson_high < 0.5;
    let positive = sufficient && wilson_low > 0.5;
    let confidence_tier = if abstain {
        "weak"
    } else if positive {
        "strong"
    } else if sufficient {
        "uncertain"
    } else {
        "insufficient"
    };
    let score_delta = if positive && positive_boost_enabled {
        ((wilson_low - 0.5) * 40.0).clamp(0.0, 8.0)
    } else {
        0.0
    };
    CalibrationEvidence {
        samples,
        wins,
        posterior_win_probability: posterior,
        wilson_low,
        wilson_high,
        target_samples,
        target_hits,
        target_posterior_probability: target_posterior,
        scope,
        confidence_tier: confidence_tier.into(),
        score_delta,
        abstain,
        positive_boost_enabled,
    }
}

async fn load_calibration_history(
    db: &SqlitePool,
    as_of: NaiveDate,
) -> Result<Vec<CalibrationSample>, PickError> {
    let since = (as_of - Duration::days(180)).format("%Y-%m-%d").to_string();
    let before = as_of.format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT reasons, meta FROM daily_pick WHERE date >= ? AND date < ? \
         AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL",
    )
    .bind(since)
    .bind(before)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(reasons, raw)| {
            let tags: Vec<String> = serde_json::from_str(&reasons).ok()?;
            let meta: Value = serde_json::from_str(&raw).ok()?;
            let real = meta["outcome"]["t1_real"].as_f64()?;
            Some(CalibrationSample {
                tags: calibration_tags(&tags),
                regime: meta["cn"]["avg_pct"]
                    .as_f64()
                    .map(|avg| market_regime(avg).0.to_string()),
                won: real > 0.0,
                target_hit_5pct: meta["outcome"]["target_hit_5pct"].as_bool(),
            })
        })
        .collect())
}

fn calibration_quality(predictions: &[(f64, bool)]) -> CalibrationQuality {
    let samples = predictions.len() as i64;
    if predictions.is_empty() {
        return CalibrationQuality {
            status: "insufficient".into(),
            reason: "尚无已完成且推荐时相似样本≥30的概率预测".into(),
            ..Default::default()
        };
    }
    let mut bins = (0..5)
        .map(|index| (index as f64 / 5.0, (index + 1) as f64 / 5.0, 0_i64, 0.0, 0_i64))
        .collect::<Vec<_>>();
    let mut brier_sum = 0.0;
    let mut log_loss_sum = 0.0;
    for (probability, won) in predictions {
        let p = probability.clamp(0.001, 0.999);
        let y = if *won { 1.0 } else { 0.0 };
        brier_sum += (p - y).powi(2);
        log_loss_sum -= y * p.ln() + (1.0 - y) * (1.0 - p).ln();
        let index = ((p * 5.0).floor() as usize).min(4);
        bins[index].2 += 1;
        bins[index].3 += p;
        bins[index].4 += i64::from(*won);
    }
    let brier_score = brier_sum / samples as f64;
    let log_loss = log_loss_sum / samples as f64;
    let mut expected_calibration_error = 0.0;
    let bins = bins
        .into_iter()
        .map(|(lower, upper, count, probability_sum, wins)| {
            let average_probability = if count > 0 { probability_sum / count as f64 } else { 0.0 };
            let observed_win_rate = ratio(wins, count);
            expected_calibration_error +=
                count as f64 / samples as f64 * (average_probability - observed_win_rate).abs();
            CalibrationBin {
                lower,
                upper,
                samples: count,
                average_probability,
                observed_win_rate,
            }
        })
        .collect::<Vec<_>>();
    let positives = predictions.iter().filter(|(_, won)| *won).count() as i64;
    let negatives = samples - positives;
    let auc = if positives > 0 && negatives > 0 {
        let favorable_pairs = predictions
            .iter()
            .filter(|(_, won)| *won)
            .flat_map(|positive| {
                predictions
                    .iter()
                    .filter(|(_, won)| !*won)
                    .map(move |negative| {
                        if positive.0 > negative.0 {
                            1.0
                        } else if (positive.0 - negative.0).abs() < f64::EPSILON {
                            0.5
                        } else {
                            0.0
                        }
                    })
            })
            .sum::<f64>();
        Some(favorable_pairs / (positives * negatives) as f64)
    } else {
        None
    };
    let sufficient = samples >= 50;
    let reliable = sufficient && brier_score <= 0.24 && expected_calibration_error <= 0.10;
    let (status, reason) = if !sufficient {
        (
            "insufficient",
            format!("已完成 {samples}/50 个有效预测，正向概率加分保持关闭"),
        )
    } else if reliable {
        (
            "reliable",
            format!("Brier {brier_score:.3}、ECE {expected_calibration_error:.3} 均通过门槛"),
        )
    } else {
        (
            "degraded",
            format!("校准漂移：Brier {brier_score:.3} 或 ECE {expected_calibration_error:.3} 未达标，正向加分已关闭"),
        )
    };
    CalibrationQuality {
        status: status.into(),
        samples,
        brier_score,
        expected_calibration_error,
        log_loss,
        auc,
        positive_boost_enabled: reliable,
        reason,
        bins,
    }
}

pub(crate) async fn load_calibration_quality(
    db: &SqlitePool,
    through_date: &str,
) -> CalibrationQuality {
    let parsed = NaiveDate::parse_from_str(through_date, "%Y-%m-%d")
        .unwrap_or_else(|_| Utc::now().date_naive());
    let since = (parsed - Duration::days(180)).format("%Y-%m-%d").to_string();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT meta FROM daily_pick WHERE date >= ? AND date <= ? \
         AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL \
         AND json_extract(meta,'$.calibration.posterior_win_probability') IS NOT NULL",
    )
    .bind(since)
    .bind(through_date)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let predictions = rows
        .into_iter()
        .filter_map(|raw| {
            let meta: Value = serde_json::from_str(&raw).ok()?;
            if meta["calibration"]["samples"].as_i64().unwrap_or(0) < 30 {
                return None;
            }
            Some((
                meta["calibration"]["posterior_win_probability"].as_f64()?,
                meta["outcome"]["t1_real"].as_f64()? > 0.0,
            ))
        })
        .collect::<Vec<_>>();
    calibration_quality(&predictions)
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

#[derive(Debug, Clone, PartialEq)]
struct StockRiskMetrics {
    beta_60d: Option<f64>,
    avg_amount_20d: Option<f64>,
    max_drawdown_20d: Option<f64>,
    expected_shortfall_10pct_20d: Option<f64>,
    t1_reach_5pct_rate_20d: Option<f64>,
    t1_reach_5pct_wilson_low: Option<f64>,
    target_stable: bool,
    return_5d: Option<f64>,
    crowding_penalty: f64,
    crowding_blocked: bool,
    stress_loss_market_down_2pct: f64,
    liquidity_blocked: bool,
}

const TARGET_NET_RETURN: f64 = 0.05;
const TARGET_COST_BUFFER: f64 = 0.002;
const BUY_ZONE_UPPER_BUFFER: f64 = 0.02;
const SHADOW_EXPERIMENT_ID: &str = "target-stability-challenger-v1";
const CALIBRATION_EXPERIMENT_ID: &str = "calibrated-abstention-challenger-v1";
const SHADOW_PROMOTION_DAYS: i64 = 20;
const SHADOW_PROMOTION_SAMPLES: i64 = 30;
const PRODUCTION_RULE_VERSION: &str = "production-v2026.09.26";

/// 生产规则的可复现参数清单。修改影响入选或暂停的阈值时必须同步更新版本。
fn production_rule_manifest() -> Value {
    serde_json::json!({
        "candidate_score_floor": 60.0,
        "max_picks": 5,
        "industry_cap": 2,
        "portfolio_stress_floor_pct": -6.0,
        "market_crash_pause_pct": -2.0,
        "market_weak_penalty_pct": -0.8,
        "joint_gate": {"nasdaq_pct": -1.5, "cn_pct": -1.0},
        "loss_streak_pause_days": 3,
        "liquidity_floor": 50_000_000.0,
        "ai_quant_proximity_points": 5.0,
        "target_net_return_pct": 5.0,
        "execution_cost_pct": 0.2,
        "walk_forward": {"lookback_days": 120, "train_days": 90, "validation_days": 30},
        "shadow_experiment_id": SHADOW_EXPERIMENT_ID,
        "calibration": {
            "lookback_days": 180,
            "minimum_similar_samples": 30,
            "similarity_floor": 0.35,
            "minimum_shared_tags": 2,
            "abstain_when_wilson_high_below": 0.5,
            "positive_when_wilson_low_above": 0.5,
            "prior": "Beta(2,2)",
            "quality_gate": {
                "minimum_completed_predictions": 50,
                "maximum_brier_score": 0.24,
                "maximum_expected_calibration_error": 0.10,
                "positive_boost_before_qualified": false
            },
            "shadow_experiment_id": CALIBRATION_EXPERIMENT_ID,
        },
    })
}

fn production_experiment() -> (String, String, Value) {
    let manifest = production_rule_manifest();
    let canonical = serde_json::to_string(&manifest).unwrap_or_default();
    // FNV-1a：跨进程稳定，不依赖 Rust 随机哈希种子。
    let hash = canonical.as_bytes().iter().fold(0xcbf29ce484222325_u64, |acc, byte| {
        (acc ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    });
    let rule_hash = format!("{hash:016x}");
    (
        format!("{PRODUCTION_RULE_VERSION}-{rule_hash}"),
        rule_hash,
        manifest,
    )
}

fn attach_production_experiment(market: &mut Value) {
    let (experiment_id, rule_hash, manifest) = production_experiment();
    market["experiment"] = serde_json::json!({
        "experiment_id": experiment_id,
        "rule_version": PRODUCTION_RULE_VERSION,
        "rule_hash": rule_hash,
        "manifest": manifest,
    });
}

fn close_returns_by_date(bars: &[DayBar]) -> HashMap<&str, f64> {
    bars.windows(2)
        .filter_map(|pair| {
            let previous = pair[0].close;
            (previous > 0.0).then_some((pair[1].date.as_str(), pair[1].close / previous - 1.0))
        })
        .collect()
}

fn estimate_beta(stock: &[DayBar], benchmark: &[DayBar]) -> Option<f64> {
    let benchmark_returns = close_returns_by_date(benchmark);
    let mut pairs: Vec<(f64, f64)> = stock
        .windows(2)
        .filter_map(|pair| {
            let previous = pair[0].close;
            if previous <= 0.0 {
                return None;
            }
            benchmark_returns
                .get(pair[1].date.as_str())
                .map(|market_return| (pair[1].close / previous - 1.0, *market_return))
        })
        .collect();
    if pairs.len() > 60 {
        pairs.drain(..pairs.len() - 60);
    }
    if pairs.len() < 20 {
        return None;
    }
    let stock_mean = pairs.iter().map(|(value, _)| value).sum::<f64>() / pairs.len() as f64;
    let market_mean = pairs.iter().map(|(_, value)| value).sum::<f64>() / pairs.len() as f64;
    let covariance = pairs
        .iter()
        .map(|(stock_return, market_return)| {
            (stock_return - stock_mean) * (market_return - market_mean)
        })
        .sum::<f64>();
    let market_variance = pairs
        .iter()
        .map(|(_, market_return)| (market_return - market_mean).powi(2))
        .sum::<f64>();
    (market_variance > f64::EPSILON).then_some(covariance / market_variance)
}

fn stock_risk_metrics(
    bars: &[DayBar],
    benchmark: &[DayBar],
    turnover_pct: Option<f64>,
    volume_ratio: Option<f64>,
) -> StockRiskMetrics {
    let recent_amounts: Vec<f64> = bars
        .iter()
        .rev()
        .take(20)
        .filter_map(|bar| (bar.amount > 0.0).then_some(bar.amount))
        .collect();
    // 某些降级行情源不返回成交额；样本不足时不误杀，只把指标标成未知。
    let avg_amount_20d = (recent_amounts.len() >= 10)
        .then(|| recent_amounts.iter().sum::<f64>() / recent_amounts.len() as f64);
    let liquidity_blocked = avg_amount_20d.is_some_and(|amount| amount < 50_000_000.0)
        || turnover_pct.is_some_and(|turnover| turnover < 0.5);
    let recent: Vec<f64> = bars
        .iter()
        .rev()
        .take(20)
        .map(|bar| bar.close)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let max_drawdown_20d = if recent.len() >= 2 {
        let mut peak = recent[0];
        let mut drawdown = 0.0_f64;
        for close in &recent {
            peak = peak.max(*close);
            if peak > 0.0 {
                drawdown = drawdown.min((close - peak) / peak * 100.0);
            }
        }
        Some(drawdown)
    } else {
        None
    };
    let mut returns: Vec<f64> = recent
        .windows(2)
        .filter_map(|pair| (pair[0] > 0.0).then_some((pair[1] / pair[0] - 1.0) * 100.0))
        .collect();
    returns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let expected_shortfall_10pct_20d = if returns.is_empty() {
        None
    } else {
        let count = ((returns.len() as f64 * 0.1).ceil() as usize).max(1);
        Some(returns[..count].iter().sum::<f64>() / count as f64)
    };
    let reach_samples: Vec<bool> = bars
        .windows(2)
        .rev()
        .take(40)
        .filter_map(|pair| {
            (pair[0].close > 0.0).then_some(
                pair[1].high / pair[0].close - 1.0
                    >= (1.0 + BUY_ZONE_UPPER_BUFFER)
                        * (1.0 + TARGET_NET_RETURN + TARGET_COST_BUFFER)
                        - 1.0,
            )
        })
        .collect();
    let recent_count = reach_samples.len().min(20);
    let recent_hits = reach_samples[..recent_count]
        .iter()
        .filter(|reached| **reached)
        .count();
    let prior = &reach_samples[recent_count..];
    let prior_hits = prior.iter().filter(|reached| **reached).count();
    let t1_reach_5pct_rate_20d = (recent_count == 20).then_some(recent_hits as f64 / 20.0);
    let total_hits = recent_hits + prior_hits;
    let t1_reach_5pct_wilson_low = (reach_samples.len() >= 39)
        .then(|| wilson_interval(reach_samples.len() as i64, total_hits as i64, 1.96).0);
    let target_stable = recent_count == 20
        && prior.len() >= 19
        && recent_hits >= 2
        && prior_hits >= 2
        && t1_reach_5pct_wilson_low.is_some_and(|lower| lower >= 0.05);
    let beta_60d = estimate_beta(bars, benchmark);
    let return_5d = (bars.len() >= 6 && bars[bars.len() - 6].close > 0.0).then(|| {
        (bars.last().map(|bar| bar.close).unwrap_or(0.0) / bars[bars.len() - 6].close - 1.0) * 100.0
    });
    let overheated_turnover = turnover_pct.is_some_and(|turnover| turnover >= 20.0)
        || volume_ratio.is_some_and(|ratio| ratio >= 2.5);
    let crowding_blocked = return_5d.is_some_and(|value| value >= 25.0);
    let crowding_penalty = if return_5d.is_some_and(|value| value >= 15.0) && overheated_turnover {
        -15.0
    } else if return_5d.is_some_and(|value| value >= 12.0) {
        -8.0
    } else {
        0.0
    };
    let stress_loss_market_down_2pct =
        (-2.0 * beta_60d.unwrap_or(1.0).max(0.5)).min(expected_shortfall_10pct_20d.unwrap_or(-3.0));
    StockRiskMetrics {
        beta_60d,
        avg_amount_20d,
        max_drawdown_20d,
        expected_shortfall_10pct_20d,
        t1_reach_5pct_rate_20d,
        t1_reach_5pct_wilson_low,
        target_stable,
        return_5d,
        crowding_penalty,
        crowding_blocked,
        stress_loss_market_down_2pct,
        liquidity_blocked,
    }
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
        (
            "today_close",
            "今日尾盘",
            "14:45-14:57",
            "next_day_positive_close",
        )
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
    let predicted_zone = sell_zone.unwrap_or((close * 1.012, close * 1.022));
    // 用户目标是净盈利至少 5%；预留约 0.2% 手续费/滑点，并按买区上界计算，
    // 避免在买区内较高价格成交后，展示的卖价仍达不到目标。
    let profit_target_price =
        round_price(buy_zone_high * (1.0 + TARGET_NET_RETURN + TARGET_COST_BUFFER));
    let sell_price_low = predicted_zone.0.max(profit_target_price);
    let sell_price_high = predicted_zone.1.max(sell_price_low * 1.01);
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
        "profit_target_price": profit_target_price,
        "target_return_pct": 5.0,
        "target_cost_buffer_pct": 0.2,
        "buy_basis_close": round_price(close),
    })
}

/// §A.9 价格取整：A 股最小报价 0.01 元，按 0.01 取整便于前端展示。
fn round_price(p: f64) -> f64 {
    (p * 100.0).round() / 100.0
}

fn classify_entry_validity(
    open: f64,
    buy_low: f64,
    buy_high: f64,
) -> Option<(&'static str, String)> {
    if open <= 0.0 || buy_low <= 0.0 || buy_high <= 0.0 {
        return None;
    }
    if open < buy_low {
        Some((
            "invalid_below",
            format!("买入失效 · 开盘 {:.2} 跌破买区 {:.2}", open, buy_low),
        ))
    } else if open > buy_high * 1.02 {
        Some((
            "invalid_gap",
            format!("买入失效 · 高开超买区上界，勿追（{:.2}）", open),
        ))
    } else {
        Some(("valid", "开盘仍在可执行区间".into()))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DynamicExitPlan {
    sell_low: f64,
    sell_high: f64,
    risk_stop: f64,
    open_strength: &'static str,
    action: &'static str,
    action_label: String,
    target_reached: Option<bool>,
}

/// §A.14 动态卖出计划：盈利目标始终按真实执行成本计算；开盘弱只收窄上沿，
/// 风险退出价单列，绝不把止损价包装成“盈利 5%”。
fn dynamic_exit_plan(
    signal_date: NaiveDate,
    today: NaiveDate,
    entry: f64,
    reference_close: f64,
    t1_open: Option<f64>,
    t1_close: Option<f64>,
    entry_status: Option<&str>,
) -> Option<DynamicExitPlan> {
    if entry <= 0.0 {
        return None;
    }
    let gap = t1_open
        .filter(|open| *open > 0.0 && reference_close > 0.0)
        .map(|open| (open - reference_close) / reference_close * 100.0)
        .unwrap_or(0.0);
    let open_strength = if gap >= 1.5 {
        "strong"
    } else if gap <= -1.0 {
        "weak"
    } else {
        "neutral"
    };
    let sell_low = round_price(entry * (1.0 + TARGET_NET_RETURN + TARGET_COST_BUFFER));
    let upper_factor = match open_strength {
        "strong" => 1.02,
        "weak" => 1.005,
        _ => 1.01,
    };
    let sell_high = round_price(sell_low * upper_factor);
    let risk_stop = round_price(entry * 0.97);
    let reached = t1_close.map(|close| close >= sell_low);
    let (action, action_label) = if entry_status.is_some_and(|status| status.starts_with("invalid")) {
        ("entry_invalid", "买入条件已失效，不应按计划追入".to_string())
    } else if t1_close.is_some_and(|close| close < risk_stop) {
        ("risk_exit", format!("跌破风险线 {:.2}，优先控制损失", risk_stop))
    } else if reached == Some(true) {
        ("take_profit", "已达到净5%目标，可分批止盈".to_string())
    } else if today > signal_date && t1_close.is_some() {
        ("t1_timeout", "T+1收盘仍未达目标，尾盘评估退出".to_string())
    } else {
        match open_strength {
            "strong" => ("hold_strength", "强开，目标区内分批止盈".to_string()),
            "weak" => (
                "risk_control",
                format!("弱开，反弹减仓；跌破 {:.2} 优先退出", risk_stop),
            ),
            _ => ("hold_neutral", "平开，按目标区分批处理".to_string()),
        }
    };
    Some(DynamicExitPlan {
        sell_low,
        sell_high,
        risk_stop,
        open_strength,
        action,
        action_label,
        target_reached: reached,
    })
}

/// §A.9 卖出价区间计算器：基于该票近 30 天 outcome.t1_pct 均值与胜率。
///
///  sell_mid = close × (1 + avg_t1_pct × win_rate_factor)
///  win_rate_factor = max(0.5, observed_win_rate)  // 0.5 是「50% 期望收益」的下限
///  返回 (sell_low = sell_mid × 0.985, sell_high = sell_mid × 1.015)
///  样本 < 3 时 fallback：sell_mid = close × 1.012（市场平均 T+1 收益 1.2%）。
pub async fn fetch_sell_zone(db: &SqlitePool, code: &str, close: f64) -> Option<(f64, f64)> {
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
         WHERE date >= ? AND (\
            json_extract(meta,'$.outcome.t1_pct') IS NOT NULL OR \
            json_extract(meta,'$.outcome.entry_open') IS NOT NULL\
         ) \
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
         WHERE date = ? AND (\
            json_extract(meta,'$.outcome.t1_pct') IS NOT NULL OR \
            json_extract(meta,'$.outcome.entry_open') IS NOT NULL\
         ) \
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
        let t1_open = outcome["t1_open_basis"]
            .as_f64()
            .or_else(|| outcome["entry_open"].as_f64());
        let t1_close = outcome["t1_close"].as_f64();
        let entry_gap = outcome["entry_gap"].as_f64();
        let entry_timing = meta["plan"]["entry_timing"].as_str().map(String::from);
        let entry_label = meta["plan"]["entry_label"].as_str().map(String::from);
        let calculated_entry = t1_open.and_then(|open| {
            classify_entry_validity(
                open,
                meta["plan"]["buy_price_low"].as_f64().unwrap_or(0.0),
                meta["plan"]["buy_price_high"].as_f64().unwrap_or(0.0),
            )
        });
        let entry_status = outcome["entry_status"]
            .as_str()
            .map(String::from)
            .or_else(|| {
                calculated_entry
                    .as_ref()
                    .map(|(status, _)| (*status).into())
            });
        let entry_status_label = outcome["entry_status_label"]
            .as_str()
            .map(String::from)
            .or_else(|| calculated_entry.map(|(_, label)| label));
        let reasons_vec: Vec<String> = serde_json::from_str(&reasons).unwrap_or_default();
        // 可执行卖点以真实/计划买入成本为基准，禁止使用已经发生的 T+1 收盘价反推。
        let basis_price = if matches!(entry_timing.as_deref(), Some("next_session_open")) {
            t1_open.or(close)
        } else {
            close
        };
        let basis_kind = Some("actual_execution_5pct_target".to_string());
        let signal_date = NaiveDate::parse_from_str(&date, "%Y-%m-%d").unwrap_or(today);
        let exit_plan = basis_price.and_then(|entry| {
            dynamic_exit_plan(
                signal_date,
                today,
                entry,
                close.unwrap_or(entry),
                t1_open,
                t1_close,
                if matches!(entry_timing.as_deref(), Some("today_close")) {
                    None
                } else {
                    entry_status.as_deref()
                },
            )
        });
        let (sell_low, sell_high) = exit_plan
            .as_ref()
            .map(|plan| (plan.sell_low, plan.sell_high))
            .unwrap_or((0.0, 0.0));
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
            sell_price_high: if sell_high > 0.0 {
                Some(sell_high)
            } else {
                None
            },
            sell_basis_price: basis_price,
            sell_basis_kind: basis_kind,
            open_strength: exit_plan
                .as_ref()
                .map(|plan| plan.open_strength.to_string()),
            sell_action: exit_plan.as_ref().map(|plan| plan.action.to_string()),
            sell_action_label: exit_plan
                .as_ref()
                .map(|plan| plan.action_label.clone()),
            risk_stop_price: exit_plan.as_ref().map(|plan| plan.risk_stop),
            target_reached: exit_plan.as_ref().and_then(|plan| plan.target_reached),
            entry_timing,
            entry_label,
            entry_status,
            entry_status_label,
            reasons: reasons_vec,
        });
    }
    Ok(out)
}

/// §A.10 大盘极端行情下不出推荐时返回的轻量 PicksDocument：
/// picks=[]、stats=默认、execute_hint=暂停提示、market.cn 透出大盘数据。
async fn build_pause_document(
    state: &AppState,
    date: NaiveDate,
    cn: Option<&CnSentiment>,
    us: Option<&UsSentiment>,
    hint: String,
) -> PicksDocument {
    let cn_value = cn.map(|c| {
        serde_json::json!({
            "sh_pct": c.sh_pct,
            "sz_pct": c.sz_pct,
            "gem_pct": c.gem_pct,
            "hs300_pct": c.hs300_pct,
            "avg_pct": c.avg_pct(),
            "risk_score": c.risk_score().0,
        })
    });
    let mut market = serde_json::json!({
        "djia": us.and_then(|s| s.djia_pct),
        "ixic": us.and_then(|s| s.ixic_pct),
        "us": {
            "djia": us.and_then(|s| s.djia_pct),
            "ixic": us.and_then(|s| s.ixic_pct),
        },
        "cn": cn_value,
    });
    attach_production_experiment(&mut market);
    let previous_picks = fetch_previous_picks(&state.db, date)
        .await
        .unwrap_or_default();
    PicksDocument {
        date: date.format("%Y-%m-%d").to_string(),
        picks: Vec::new(),
        stats: PickStats::default(),
        market,
        execute_hint: hint,
        previous_picks,
    }
}

fn joint_market_should_pause(cn: Option<&CnSentiment>, us: Option<&UsSentiment>) -> bool {
    let nasdaq_crash = us
        .and_then(|item| item.ixic_pct)
        .is_some_and(|pct| pct <= -1.5);
    let cn_open_weak = cn
        .and_then(CnSentiment::avg_pct)
        .is_some_and(|pct| pct <= -1.0);
    nasdaq_crash && cn_open_weak
}

async fn persist_pick_run(
    db: &SqlitePool,
    date: NaiveDate,
    market: &Value,
    execute_hint: &str,
    paused: bool,
    pause_source: Option<&str>,
) -> Result<(), PickError> {
    let date_key = date.format("%Y-%m-%d").to_string();
    let pause_source = if paused {
        pause_source.unwrap_or("unknown")
    } else {
        ""
    };
    let mut tx = db.begin().await?;
    // 同一天重跑可能解除暂停或切换触发源：保留历史，但先撤销旧 active 状态。
    sqlx::query("UPDATE pick_pause_audit SET active = 0 WHERE date = ? AND active != 0")
        .bind(&date_key)
        .execute(&mut *tx)
        .await?;
    if paused {
        // 同日重跑由正常清单切换为暂停时，必须移除旧票，避免 GET 再返回它们。
        sqlx::query("DELETE FROM daily_pick WHERE date = ?")
            .bind(&date_key)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO daily_pick_run(date, market, execute_hint, paused, created_at, pause_source) \
         VALUES(?,?,?,?,?,?) ON CONFLICT(date) DO UPDATE SET \
         market=excluded.market, execute_hint=excluded.execute_hint, \
         paused=excluded.paused, created_at=excluded.created_at, \
         pause_source=excluded.pause_source",
    )
    .bind(&date_key)
    .bind(market.to_string())
    .bind(execute_hint)
    .bind(if paused { 1_i64 } else { 0_i64 })
    .bind(Utc::now().timestamp_millis())
    .bind(pause_source)
    .execute(&mut *tx)
    .await?;
    if paused {
        sqlx::query(
            "INSERT INTO pick_pause_audit(date, source, reason, active, created_at) \
             VALUES(?,?,?,?,?) ON CONFLICT(date, source, reason) DO UPDATE SET \
             active=excluded.active, created_at=excluded.created_at",
        )
        .bind(&date_key)
        .bind(pause_source)
        .bind(execute_hint)
        .bind(1_i64)
        .bind(Utc::now().timestamp_millis())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 人工安全阀必须位于所有行情、新闻和 AI 请求之前。设置表保存 JSON，
/// 同时兼容历史客户端可能写入的字符串布尔值。
async fn manual_pick_paused(db: &SqlitePool) -> Result<bool, PickError> {
    let raw: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'smartPicksPaused'")
            .fetch_optional(db)
            .await?;
    Ok(raw
        .and_then(|value| serde_json::from_str::<Value>(&value).ok())
        .is_some_and(|value| {
            value.as_bool().unwrap_or_else(|| {
                value
                    .as_str()
                    .is_some_and(|text| text.eq_ignore_ascii_case("true"))
            })
        }))
}

#[derive(Debug, Clone, PartialEq)]
struct IntradayConfirmation {
    status: &'static str,
    overlap_ratio: f64,
    regime_changed: bool,
    reason: String,
}

fn regime_key_from_market(market: &Value) -> Option<&'static str> {
    market["cn"]["avg_pct"]
        .as_f64()
        .map(|avg| market_regime(avg).0)
}

fn compare_intraday_snapshots(
    morning_codes: &[String],
    morning_market: &Value,
    tail_codes: &[String],
    tail_market: &Value,
) -> IntradayConfirmation {
    let denominator = morning_codes.len().min(tail_codes.len());
    let overlap = morning_codes
        .iter()
        .filter(|code| tail_codes.contains(code))
        .count();
    let overlap_ratio = if denominator > 0 {
        overlap as f64 / denominator as f64
    } else {
        0.0
    };
    let morning_regime = regime_key_from_market(morning_market);
    let tail_regime = regime_key_from_market(tail_market);
    let regime_changed =
        morning_regime.is_some() && tail_regime.is_some() && morning_regime != tail_regime;
    let unstable = regime_changed || overlap_ratio < 0.4;
    let reason = if regime_changed {
        format!(
            "盘中市场环境由 {} 变为 {}",
            morning_regime.unwrap_or("unknown"),
            tail_regime.unwrap_or("unknown")
        )
    } else if overlap_ratio < 0.4 {
        format!("早盘与尾盘 Top 重合率仅 {:.0}%", overlap_ratio * 100.0)
    } else {
        format!("早盘与尾盘 Top 重合率 {:.0}%", overlap_ratio * 100.0)
    };
    IntradayConfirmation {
        status: if unstable {
            "observe_only"
        } else {
            "confirmed"
        },
        overlap_ratio,
        regime_changed,
        reason,
    }
}

async fn persist_pick_snapshot(
    db: &SqlitePool,
    date: NaiveDate,
    session: &str,
    market: &Value,
    codes: &[String],
) -> Result<(), PickError> {
    sqlx::query(
        "INSERT INTO daily_pick_snapshot(date, session, market, picks, created_at) \
         VALUES(?,?,?,?,?) ON CONFLICT(date,session) DO UPDATE SET \
         market=excluded.market, picks=excluded.picks, created_at=excluded.created_at",
    )
    .bind(date.format("%Y-%m-%d").to_string())
    .bind(session)
    .bind(market.to_string())
    .bind(serde_json::to_string(codes).unwrap_or_else(|_| "[]".into()))
    .bind(Utc::now().timestamp_millis())
    .execute(db)
    .await?;
    Ok(())
}

async fn load_pick_snapshot(
    db: &SqlitePool,
    date: NaiveDate,
    session: &str,
) -> Result<Option<(Value, Vec<String>)>, PickError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT market, picks FROM daily_pick_snapshot WHERE date=? AND session=?")
            .bind(date.format("%Y-%m-%d").to_string())
            .bind(session)
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(market, picks)| {
        (
            serde_json::from_str(&market).unwrap_or(Value::Null),
            serde_json::from_str(&picks).unwrap_or_default(),
        )
    }))
}

fn current_pick_session(date: NaiveDate) -> &'static str {
    let tz = FixedOffset::east_opt(8 * 3600).expect("valid UTC+8 offset");
    let now_cn = Utc::now().with_timezone(&tz);
    let minute = now_cn.hour() * 60 + now_cn.minute();
    if date == now_cn.date_naive() && minute < 14 * 60 + 45 {
        "morning"
    } else {
        "tail"
    }
}

async fn consecutive_loss_days(
    db: &SqlitePool,
    before: NaiveDate,
    required: usize,
) -> Result<Vec<(String, f64)>, PickError> {
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT date, AVG(CAST(json_extract(meta,'$.outcome.t1_real') AS REAL)) AS portfolio_t1 \
         FROM daily_pick WHERE date < ? \
         AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL \
         GROUP BY date ORDER BY date DESC LIMIT ?",
    )
    .bind(before.format("%Y-%m-%d").to_string())
    .bind(required as i64)
    .fetch_all(db)
    .await?;
    if rows.len() == required && rows.iter().all(|(_, value)| *value < 0.0) {
        Ok(rows)
    } else {
        Ok(Vec::new())
    }
}

/// 生成某基准日推荐。`ai` 为客户端透传配置（None = 纯量化，调度器路径）。
/// 幂等：同日重复生成 DELETE + INSERT 全量刷新。
pub async fn generate_picks(
    state: &AppState,
    date: NaiveDate,
    ai_config: Option<&AiRankConfig>,
) -> Result<PicksDocument, PickError> {
    // §A.13 人工 Kill Switch：绝对优先级最高，且不得触发任何外部请求。
    if manual_pick_paused(&state.db).await? {
        let hint = "⏸ 已在设置中手动暂停智能推荐；关闭 Kill Switch 后方可恢复".to_string();
        let mut doc = build_pause_document(state, date, None, None, hint).await;
        doc.market["pause_source"] = Value::String("manual".into());
        persist_pick_snapshot(
            &state.db,
            date,
            current_pick_session(date),
            &doc.market,
            &[],
        )
        .await?;
        persist_pick_run(
            &state.db,
            date,
            &doc.market,
            &doc.execute_hint,
            true,
            Some("manual"),
        )
        .await?;
        return Ok(doc);
    }
    // §A.13 策略级熔断：大盘即使正常，最近 3 个已回写推荐日组合仍连续亏损时暂停。
    let loss_days = consecutive_loss_days(&state.db, date, 3).await?;
    if !loss_days.is_empty() {
        let summary = loss_days
            .iter()
            .map(|(day, pct)| format!("{day} {pct:+.1}%"))
            .collect::<Vec<_>>()
            .join(" / ");
        let hint = format!("⚠️ 策略连续 3 个推荐日亏损（{summary}），今日暂停推荐并进入观察期");
        let mut doc = build_pause_document(state, date, None, None, hint).await;
        doc.market["pause_source"] = Value::String("loss_streak".into());
        persist_pick_snapshot(
            &state.db,
            date,
            current_pick_session(date),
            &doc.market,
            &[],
        )
        .await?;
        persist_pick_run(
            &state.db,
            date,
            &doc.market,
            &doc.execute_hint,
            true,
            Some("loss_streak"),
        )
        .await?;
        return Ok(doc);
    }
    let mut learned_meta: HashMap<String, Value> = HashMap::new();
    let mut calibration_meta: HashMap<String, Value> = HashMap::new();
    let mut shadow_experiment_map: HashMap<String, &'static str> = HashMap::new();
    let mut shadow_reason_map: HashMap<String, &'static str> = HashMap::new();
    // 候选池：东财 push2 断连时切新浪榜（板块动量因子因无行业字段自动降级）
    let candidates = match state.pick_ranking.fetch(&state.http, 100).await {
        Ok(rows) if !rows.is_empty() => rows,
        Ok(_) => return Err(PickError::Parse("涨幅榜为空".into())),
        Err(error) => {
            tracing::warn!(%error, "eastmoney ranking failed, fallback to sina");
            state.sina_ranking.fetch(&state.http, 100).await?
        }
    };
    // §A.10 大盘 beta：拉上证/深成/创业板/沪深300 当日涨跌幅，三指数均值
    //  ≤-2.0 视为极端行情——直接短路不出推荐，避免在系统性下跌日追涨杀跌。
    let cn_sentiment: Option<CnSentiment> = state.cn_index.fetch(&state.http).await.ok();
    let (cn_risk, cn_tag) = cn_sentiment
        .as_ref()
        .map(|s| s.risk_score())
        .unwrap_or((0.0, None));
    let us_sentiment: Option<UsSentiment> = state.us_index.fetch(&state.http).await.ok();
    let joint_pause = joint_market_should_pause(cn_sentiment.as_ref(), us_sentiment.as_ref());
    if cn_sentiment
        .as_ref()
        .map(|s| s.should_pause())
        .unwrap_or(false)
        || joint_pause
    {
        let avg = cn_sentiment
            .as_ref()
            .and_then(|s| s.avg_pct())
            .unwrap_or(0.0);
        let hint = if joint_pause && avg > -2.0 {
            let ixic = us_sentiment
                .as_ref()
                .and_then(|item| item.ixic_pct)
                .unwrap_or(0.0);
            format!("📉 隔夜纳指 {ixic:+.2}% + A股大盘 {avg:+.2}%，联动闸门触发，今日暂停推荐")
        } else {
            format!("📉 大盘大跌（主流指数均值 {avg:+.2}%），今日暂停推荐；建议观望 1-2 个交易日")
        };
        tracing::warn!(avg_pct = avg, joint_pause, "市场风险闸门触发，今日暂停推荐");
        // 返回空 picks + 在 market.cn 透出大盘数据 + execute_hint 给前端提示。
        // 同时持久化「零推荐」运行快照，确保随后 GET /picks 不会回退到旧清单。
        let pause_source = if joint_pause {
            "joint_market"
        } else {
            "market_crash"
        };
        let mut doc = build_pause_document(
            state,
            date,
            cn_sentiment.as_ref(),
            us_sentiment.as_ref(),
            hint,
        )
        .await;
        doc.market["pause_source"] = Value::String(pause_source.into());
        persist_pick_snapshot(
            &state.db,
            date,
            current_pick_session(date),
            &doc.market,
            &[],
        )
        .await?;
        persist_pick_run(
            &state.db,
            date,
            &doc.market,
            &doc.execute_hint,
            true,
            Some(pause_source),
        )
        .await?;
        return Ok(doc);
    }
    let current_regime = cn_sentiment
        .as_ref()
        .and_then(CnSentiment::avg_pct)
        .map(|avg| market_regime(avg).0);
    let learned_performance = load_tag_performance(&state.db, date, current_regime).await?;
    let calibration_history = load_calibration_history(&state.db, date).await?;
    let calibration_quality =
        load_calibration_quality(&state.db, &date.format("%Y-%m-%d").to_string()).await;
    if candidates.is_empty() {
        return Err(PickError::Parse("涨幅榜为空".into()));
    }

    // 板块动量（候选池行业聚合）+ 隔夜美股情绪（拉取失败只降级跳过）
    let (industry_avg, strong_industries) = industry_momentum(&candidates);
    let candidates = select_diversified_candidates(
        &candidates,
        &industry_avg,
        cn_sentiment.as_ref().and_then(CnSentiment::avg_pct),
        100,
    );
    let (us_mood, us_tag) = us_sentiment
        .as_ref()
        .map(|s| s.mood_score())
        .unwrap_or((0.0, None));
    let semis_drag = us_sentiment
        .as_ref()
        .map(|s| s.semis_drag())
        .unwrap_or(false);

    // §A.12 个股 beta：沪深300 作为统一基准；失败时降级为不施加 beta 权重。
    let benchmark_bars = fetch_days_cached(state, "sh000300", 80)
        .await
        .unwrap_or_default();

    // ② 逐候选拉日 K（顺带落库 day_bar，给回测与复盘复用），300ms 间隔防限流
    let mut scored: Vec<(Candidate, TechScore)> = Vec::new();
    let mut limit_up_strength_map: HashMap<String, LimitUpStrength> = HashMap::new();
    let mut stock_risk_map: HashMap<String, StockRiskMetrics> = HashMap::new();
    for candidate in &candidates {
        match fetch_days_cached(state, &candidate.code, 120).await {
            Ok(bars) => {
                let risk = stock_risk_metrics(
                    &bars,
                    &benchmark_bars,
                    candidate.turnover_pct,
                    candidate.volume_ratio,
                );
                let liquidity_blocked = risk.liquidity_blocked;
                stock_risk_map.insert(candidate.code.clone(), risk);
                if liquidity_blocked {
                    tracing::debug!(code = %candidate.code, "candidate removed by liquidity floor");
                    continue;
                }
                limit_up_strength_map.insert(
                    candidate.code.clone(),
                    classify_limit_up_strength(
                        candidate,
                        &bars,
                        strong_industries.contains(&candidate.industry),
                    ),
                );
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
    let mut shadow_boosted: Vec<(Candidate, f64, Vec<String>)> = Vec::new();
    let mut negative_signals_map: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut calendar_exclusions: Vec<(String, CalendarHardBlock)> = Vec::new();
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
    'candidate_loop: for (candidate, tech) in &scored {
        let mut score = tech.score;
        let mut tags = tech.tags.clone();
        if candidate
            .pool_sources
            .iter()
            .any(|source| source == "relative_strength")
        {
            tags.push("相对强势".into());
        }
        if candidate
            .pool_sources
            .iter()
            .any(|source| source == "pullback")
        {
            tags.push("健康回调".into());
        }

        // §A.11 涨停不再统一视为正向证据：按连板、行业、换手/量能分档。
        // 强板仅免扣分；中性 -5；弱板 -12，并把分档写入标签与 meta。
        let is_limit_up_now = candidate.is_limit_up
            || limit_up_realtime
                .get(&candidate.code)
                .copied()
                .unwrap_or(false);
        if is_limit_up_now {
            let strength = limit_up_strength_map.get(&candidate.code);
            score += strength.map(|item| item.score_delta).unwrap_or(-12.0);
            tags.push("次日开盘".into());
            tags.push(match strength.map(|item| item.tier) {
                Some("strong") => "强涨停".into(),
                Some("neutral") => "涨停待观察".into(),
                _ => "弱涨停".into(),
            });
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
        // §A.11 截面去市场化：用个股涨幅减行业均值，避免把“随板块普涨”误判为个股强势。
        let residual_pct = candidate.pct
            - industry_avg
                .get(&candidate.industry)
                .copied()
                .unwrap_or(0.0);
        score += (residual_pct * 2.0).clamp(-10.0, 10.0);
        if residual_pct >= 1.0 && !tags.iter().any(|tag| tag == "相对强势") {
            tags.push("相对强势".into());
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

        // T+1 盈利 5% 是硬目标：近 20 个交易日从前收至次日最高至少有 2 次
        // 达到 5.2%（含成本缓冲）才保留。缺足 20 日样本同样不进入可买清单。
        let target_risk = stock_risk_map.get(&candidate.code);
        let reach_rate = target_risk
            .and_then(|risk| risk.t1_reach_5pct_rate_20d)
            .unwrap_or(0.0);
        let mut shadow_only = !target_risk.is_some_and(|risk| risk.target_stable);
        if target_risk.is_some_and(|risk| risk.crowding_blocked) {
            continue;
        }
        let lower = target_risk
            .and_then(|risk| risk.t1_reach_5pct_wilson_low)
            .unwrap_or(0.0);
        let expected_shortfall = target_risk
            .and_then(|risk| risk.expected_shortfall_10pct_20d)
            .unwrap_or(-10.0);
        let max_drawdown = target_risk
            .and_then(|risk| risk.max_drawdown_20d)
            .unwrap_or(-25.0);
        if expected_shortfall < -8.0 || max_drawdown < -20.0 {
            continue;
        }
        let downside_penalty = ((-expected_shortfall - 3.0).max(0.0) * 2.0).min(10.0)
            + (-max_drawdown - 12.0).max(0.0).min(8.0);
        if !shadow_only {
            score += (lower * 40.0).clamp(0.0, 12.0);
        }
        score -= downside_penalty;
        if let Some(risk) = target_risk {
            score += risk.crowding_penalty;
            if risk.crowding_penalty < 0.0 {
                tags.push("短线拥挤".into());
            }
        }
        if shadow_only {
            tags.push("影子候选·5%稳定性待验证".into());
        } else {
            tags.push(format!(
                "T+1达5%率{:.0}%·下界{:.0}%",
                reach_rate * 100.0,
                lower * 100.0
            ));
        }

        // §A.12 偏空日优先低 beta：高 beta 额外扣分，低 beta 给小幅抗跌加分。
        if cn_sentiment
            .as_ref()
            .and_then(CnSentiment::avg_pct)
            .is_some_and(|avg| avg <= -0.8)
        {
            match stock_risk_map
                .get(&candidate.code)
                .and_then(|risk| risk.beta_60d)
            {
                Some(beta) if beta >= 1.3 => {
                    score -= 15.0;
                    tags.push("高Beta风险".into());
                }
                Some(beta) if beta <= 0.8 => {
                    score += 6.0;
                    tags.push("低Beta抗跌".into());
                }
                _ => {}
            }
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
                if let Some(block) = calendar_hard_block(&items, date) {
                    tracing::info!(
                        code = %candidate.code,
                        reason = %block.reason,
                        title = %block.title,
                        "candidate removed by calendar hard filter"
                    );
                    calendar_exclusions.push((candidate.code.clone(), block));
                    continue 'candidate_loop;
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
        // §A.10 大盘 beta：所有候选统一扣分（避免系统性下跌日追涨杀跌）
        score += cn_risk;
        if let Some(tag) = &cn_tag {
            tags.push(tag.clone());
        }
        let (learned_adjustment, evidence) = learned_score_adjustment(&tags, &learned_performance);
        score += learned_adjustment;
        learned_meta.insert(candidate.code.clone(), evidence);
        // §A.15 样本外概率校准：只用推荐日前 180 天真实成交结果。
        // 小样本保持中性；只有充分样本的 Wilson 上界仍低于 50% 才主动弃权。
        let calibration = calibrate_candidate(
            &tags,
            &calibration_history,
            current_regime,
            calibration_quality.positive_boost_enabled,
        );
        score += calibration.score_delta;
        if calibration.score_delta > 0.0 {
            tags.push(format!(
                "校准强证据·下界{:.0}%",
                calibration.wilson_low * 100.0
            ));
        }
        if calibration.abstain {
            shadow_only = true;
            tags.push("影子候选·校准主动弃权".into());
            shadow_experiment_map.insert(candidate.code.clone(), CALIBRATION_EXPERIMENT_ID);
            shadow_reason_map.insert(
                candidate.code.clone(),
                "相似历史样本的真实T+1胜率Wilson上界仍低于50%",
            );
        } else if shadow_only {
            shadow_experiment_map.insert(candidate.code.clone(), SHADOW_EXPERIMENT_ID);
            shadow_reason_map.insert(candidate.code.clone(), "未通过生产版5%双窗口稳定性门槛");
        }
        calibration_meta.insert(candidate.code.clone(), calibration.as_json());
        if shadow_only {
            shadow_boosted.push((candidate.clone(), score, tags));
        } else {
            boosted.push((candidate.clone(), score, tags));
        }
        tokio::time::sleep(StdDuration::from_millis(300)).await;
    }
    // 高胜率门槛：综合分 <60 不入选（宁缺毋滥，不足 5 只就少推）
    boosted.retain(|(_, score, _)| *score >= 60.0);
    boosted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    boosted.truncate(10);
    shadow_boosted.retain(|(_, score, _)| *score >= 60.0);
    shadow_boosted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut shadow_picks: Vec<(Candidate, f64, Vec<String>)> = Vec::new();
    for item in &shadow_boosted {
        if shadow_picks.len() >= 5 {
            break;
        }
        push_with_risk_budget(&mut shadow_picks, item, 2, &stock_risk_map, -6.0);
    }

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
        let ai_score_floor = boosted.first().map(|item| item.1 - 5.0).unwrap_or(0.0);
        for code in codes {
            if picks.len() >= 5 {
                break;
            }
            if let Some((candidate, score, tags)) = boosted.iter().find(|(c, _, _)| &c.code == code)
            {
                if *score < ai_score_floor {
                    continue;
                }
                let mut guarded_tags = tags.clone();
                guarded_tags.push("AI量化共识".into());
                let item = (candidate.clone(), *score, guarded_tags);
                push_with_risk_budget(&mut picks, &item, 2, &stock_risk_map, -6.0);
            }
        }
    }
    if picks.len() < 5 {
        for (candidate, score, tags) in &boosted {
            if picks.len() >= 5 {
                break;
            }
            let item = (candidate.clone(), *score, tags.clone());
            push_with_risk_budget(&mut picks, &item, 2, &stock_risk_map, -6.0);
        }
    }
    picks.truncate(5);
    let final_industry_counts: HashMap<String, usize> =
        picks
            .iter()
            .fold(HashMap::new(), |mut counts, (candidate, _, _)| {
                if !candidate.industry.is_empty() {
                    *counts.entry(candidate.industry.clone()).or_insert(0) += 1;
                }
                counts
            });
    let portfolio_stress_loss = if picks.is_empty() {
        0.0
    } else {
        picks
            .iter()
            .map(|(candidate, _, _)| {
                stock_risk_map
                    .get(&candidate.code)
                    .map(|risk| risk.stress_loss_market_down_2pct)
                    .unwrap_or(-6.0)
            })
            .sum::<f64>()
            / picks.len() as f64
    };

    // §A.5.1 涨停二次校准已在候选池阶段完成（提前到 scored 之后），这里直接复用。

    // 落库（同日全量刷新）
    let date_key = date.format("%Y-%m-%d").to_string();
    let position_codes: Vec<String> =
        sqlx::query_scalar("SELECT code FROM position WHERE shares > 0")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
    let now_cn = Utc::now().with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
    let (production_experiment_id, production_rule_hash, production_manifest) =
        production_experiment();
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM daily_pick WHERE date = ?")
        .bind(&date_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM daily_pick_shadow WHERE date = ?")
        .bind(&date_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM daily_pick_exclusion WHERE date = ? AND stage = 'calendar'")
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
            "experiment_id": production_experiment_id.clone(),
            "experiment": {
                "rule_version": PRODUCTION_RULE_VERSION,
                "rule_hash": production_rule_hash.clone(),
                "manifest": production_manifest.clone(),
            },
            "close": candidate.price,
            "pct": candidate.pct,
            "industry": candidate.industry,
            "industry_avg": industry_avg.get(&candidate.industry),
            "residual_pct": candidate.pct - industry_avg.get(&candidate.industry).copied().unwrap_or(0.0),
            "pool_sources": candidate.pool_sources.clone(),
            "risk": stock_risk_map.get(&candidate.code).map(|risk| serde_json::json!({
                "beta_60d": risk.beta_60d,
                "avg_amount_20d": risk.avg_amount_20d,
                "max_drawdown_20d": risk.max_drawdown_20d,
                "expected_shortfall_10pct_20d": risk.expected_shortfall_10pct_20d,
                "t1_reach_5pct_rate_20d": risk.t1_reach_5pct_rate_20d,
                "t1_reach_5pct_wilson_low": risk.t1_reach_5pct_wilson_low,
                "target_stable": risk.target_stable,
                "return_5d": risk.return_5d,
                "crowding_penalty": risk.crowding_penalty,
                "crowding_blocked": risk.crowding_blocked,
                "stress_loss_market_down_2pct": risk.stress_loss_market_down_2pct,
                "liquidity_floor": 50_000_000.0,
                "liquidity_blocked": risk.liquidity_blocked,
            })),
            "concentration": {
                "industry": candidate.industry,
                "count": final_industry_counts.get(&candidate.industry).copied().unwrap_or(1),
                "cap": 2,
            },
            "portfolio_risk_budget": {
                "stress_loss_market_down_2pct": portfolio_stress_loss,
                "floor": -6.0,
            },
            "is_limit_up": candidate.is_limit_up,
            "limit_up_realtime": limit_up_realtime
                .get(&candidate.code)
                .copied()
                .unwrap_or(false),
            "limit_up_calibrated": limit_up_realtime.contains_key(&candidate.code),
            "limit_up_strength": if plan_limit_up {
                limit_up_strength_map.get(&candidate.code).and_then(|strength| serde_json::to_value(strength).ok())
            } else {
                None
            },
            "us": {
                "djia": us_sentiment.as_ref().and_then(|s| s.djia_pct),
                "ixic": us_sentiment.as_ref().and_then(|s| s.ixic_pct),
            },
            // §A.10 A 股大盘情绪（上证/深成/创业板/沪深300）透出
            "cn": cn_sentiment.as_ref().map(|c| serde_json::json!({
                "sh_pct": c.sh_pct,
                "sz_pct": c.sz_pct,
                "gem_pct": c.gem_pct,
                "hs300_pct": c.hs300_pct,
                "avg_pct": c.avg_pct(),
                "risk_score": c.risk_score().0,
            })),
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
            "calibration": calibration_meta.get(&candidate.code).cloned(),
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
    for (rank, (candidate, score, tags)) in shadow_picks.iter().enumerate() {
        let shadow_experiment_id = shadow_experiment_map
            .get(&candidate.code)
            .copied()
            .unwrap_or(SHADOW_EXPERIMENT_ID);
        let challenger_reason = shadow_reason_map
            .get(&candidate.code)
            .copied()
            .unwrap_or("未通过生产版5%双窗口稳定性门槛");
        let realtime_limit_up = limit_up_realtime
            .get(&candidate.code)
            .copied()
            .unwrap_or(candidate.is_limit_up);
        let plan_limit_up = realtime_limit_up || candidate.is_limit_up;
        let meta = serde_json::json!({
            "experiment_id": shadow_experiment_id,
            "status": "shadow",
            "challenger_reason": challenger_reason,
            "close": candidate.price,
            "pct": candidate.pct,
            "industry": candidate.industry,
            "pool_sources": candidate.pool_sources.clone(),
            "risk": stock_risk_map.get(&candidate.code).map(|risk| serde_json::json!({
                "beta_60d": risk.beta_60d,
                "max_drawdown_20d": risk.max_drawdown_20d,
                "expected_shortfall_10pct_20d": risk.expected_shortfall_10pct_20d,
                "t1_reach_5pct_rate_20d": risk.t1_reach_5pct_rate_20d,
                "t1_reach_5pct_wilson_low": risk.t1_reach_5pct_wilson_low,
                "target_stable": risk.target_stable,
                "stress_loss_market_down_2pct": risk.stress_loss_market_down_2pct,
            })),
            "cn": cn_sentiment.as_ref().map(|c| serde_json::json!({
                "avg_pct": c.avg_pct(),
                "risk_score": c.risk_score().0,
            })),
            "auto_weight": learned_meta.get(&candidate.code).cloned(),
            "calibration": calibration_meta.get(&candidate.code).cloned(),
            "promotion_criteria": {
                "completed_days": SHADOW_PROMOTION_DAYS,
                "samples": SHADOW_PROMOTION_SAMPLES,
                "t1_real_wilson_low": 0.5,
                "target_5pct_wilson_low": 0.05,
            },
            "plan": build_trade_plan(
                date,
                now_cn,
                tags,
                false,
                plan_limit_up,
                candidate.price,
                sell_zone_map.get(&candidate.code).copied(),
                candidate.price * 0.98,
                candidate.price * 1.02,
            ),
        });
        sqlx::query(
            "INSERT INTO daily_pick_shadow(\
                date, experiment_id, code, name, rank, score, reasons, meta, created_at\
             ) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(&date_key)
        .bind(shadow_experiment_id)
        .bind(&candidate.code)
        .bind(&candidate.name)
        .bind((rank + 1) as i64)
        .bind(*score)
        .bind(serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()))
        .bind(meta.to_string())
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
    }
    for (code, block) in &calendar_exclusions {
        sqlx::query(
            "INSERT INTO daily_pick_exclusion(\
                date, code, stage, reason, evidence, created_at\
             ) VALUES(?, ?, 'calendar', ?, ?, ?)",
        )
        .bind(&date_key)
        .bind(code)
        .bind(&block.reason)
        .bind(
            serde_json::json!({
                "title": block.title,
                "published_date": block.published_date,
                "url": block.url,
            })
            .to_string(),
        )
        .bind(now_ms)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    // 执行时机：15:00 前生成 → 当天下午可买入；之后 → 次日开盘买入
    let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
    let now_cn = chrono::Utc::now().with_timezone(&tz);
    let mut execute_hint = if now_cn.hour() < 15 {
        "当天下午可买入".to_string()
    } else {
        "次日开盘买入（次日开盘价可能高于推荐日收盘价）".to_string()
    };
    let mut market = serde_json::json!({
        // 顶层字段兼容 §A.10 前的客户端；新客户端读取 market.us / market.cn。
        "djia": us_sentiment.as_ref().and_then(|s| s.djia_pct),
        "ixic": us_sentiment.as_ref().and_then(|s| s.ixic_pct),
        "us": {
            "djia": us_sentiment.as_ref().and_then(|s| s.djia_pct),
            "ixic": us_sentiment.as_ref().and_then(|s| s.ixic_pct),
        },
        "cn": cn_sentiment.as_ref().map(|c| serde_json::json!({
            "sh_pct": c.sh_pct,
            "sz_pct": c.sz_pct,
            "gem_pct": c.gem_pct,
            "hs300_pct": c.hs300_pct,
            "avg_pct": c.avg_pct(),
            "risk_score": c.risk_score().0,
        })),
    });
    attach_production_experiment(&mut market);
    let session = current_pick_session(date);
    let pick_codes: Vec<String> = picks
        .iter()
        .map(|(candidate, _, _)| candidate.code.clone())
        .collect();
    if session == "tail" {
        if let Some((morning_market, morning_codes)) =
            load_pick_snapshot(&state.db, date, "morning").await?
        {
            let confirmation =
                compare_intraday_snapshots(&morning_codes, &morning_market, &pick_codes, &market);
            market["confirmation"] = serde_json::json!({
                "status": confirmation.status,
                "overlap_ratio": confirmation.overlap_ratio,
                "regime_changed": confirmation.regime_changed,
                "reason": confirmation.reason.clone(),
            });
            if confirmation.status == "observe_only" {
                execute_hint = format!("仅观察 · {}；不作为尾盘可执行清单", confirmation.reason);
            }
        }
    }
    persist_pick_snapshot(&state.db, date, session, &market, &pick_codes).await?;
    persist_pick_run(&state.db, date, &market, &execute_hint, false, None).await?;
    let mut doc = list_picks(&state.db, Some(&date_key)).await?;
    doc.execute_hint = execute_hint;
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

async fn load_shadow_experiment_stats(
    db: &SqlitePool,
) -> Result<Vec<ShadowExperimentStat>, PickError> {
    let rows: Vec<(String, i64, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT experiment_id, \
            COUNT(DISTINCT CASE WHEN json_extract(meta,'$.outcome.t1_real') IS NOT NULL THEN date END), \
            COUNT(json_extract(meta,'$.outcome.t1_real')), \
            COALESCE(SUM(CASE WHEN CAST(json_extract(meta,'$.outcome.t1_real') AS REAL) > 0 THEN 1 ELSE 0 END), 0), \
            COUNT(json_extract(meta,'$.outcome.target_hit_5pct')), \
            COALESCE(SUM(CASE WHEN json_extract(meta,'$.outcome.target_hit_5pct') = 1 THEN 1 ELSE 0 END), 0) \
         FROM daily_pick_shadow GROUP BY experiment_id ORDER BY experiment_id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(experiment_id, completed_days, samples, wins, target_samples, target_hits)| {
                let (win_low, _, _) = wilson_interval(samples, wins, 1.96);
                let (target_low, _, _) = wilson_interval(target_samples, target_hits, 1.96);
                let eligible = completed_days >= SHADOW_PROMOTION_DAYS
                    && samples >= SHADOW_PROMOTION_SAMPLES
                    && win_low >= 0.5
                    && target_low >= 0.05;
                let reason = if eligible {
                    "样本外门槛已达标，等待人工审查后方可晋升".to_string()
                } else {
                    format!(
                        "继续影子观察：{completed_days}/{SHADOW_PROMOTION_DAYS}日，\
                         {samples}/{SHADOW_PROMOTION_SAMPLES}样本，胜率下界{:.0}%，5%达标下界{:.0}%",
                        win_low * 100.0,
                        target_low * 100.0
                    )
                };
                ShadowExperimentStat {
                    experiment_id,
                    status: if eligible { "qualified" } else { "shadow" }.into(),
                    completed_days,
                    samples,
                    t1_real_win_rate: ratio(wins, samples),
                    t1_real_wilson_low: win_low,
                    target_5pct_hit_rate: ratio(target_hits, target_samples),
                    target_5pct_wilson_low: target_low,
                    promotion_eligible: eligible,
                    promotion_reason: reason,
                }
            },
        )
        .collect())
}

async fn load_hard_filter_stats(
    db: &SqlitePool,
    date: &str,
) -> Result<Vec<HardFilterStat>, PickError> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT stage, reason, COUNT(*) FROM daily_pick_exclusion \
         WHERE date=? GROUP BY stage, reason ORDER BY stage, reason",
    )
    .bind(date)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(stage, reason, count)| HardFilterStat {
            stage,
            reason,
            count,
        })
        .collect())
}

fn build_pick_audit(picks: &[DailyPick]) -> PickAudit {
    let (experiment_id, rule_hash, manifest) = production_experiment();
    let mut pool_sources = std::collections::BTreeSet::new();
    let mut industries: HashMap<String, i64> = HashMap::new();
    let mut limit_up_count = 0_i64;
    for pick in picks {
        if let Some(sources) = pick.meta["pool_sources"].as_array() {
            for source in sources.iter().filter_map(Value::as_str) {
                pool_sources.insert(source.to_string());
            }
        }
        if let Some(industry) = pick.meta["industry"].as_str().filter(|item| !item.is_empty()) {
            *industries.entry(industry.to_string()).or_insert(0) += 1;
        }
        if pick.meta["is_limit_up"].as_bool().unwrap_or(false) {
            limit_up_count += 1;
        }
    }
    let max_industry = industries.values().copied().max().unwrap_or(0);
    PickAudit {
        experiment_id,
        rule_version: PRODUCTION_RULE_VERSION.into(),
        rule_hash,
        manifest,
        pool_sources: pool_sources.into_iter().collect(),
        concentration_summary: if picks.is_empty() {
            "当日无生产候选".into()
        } else {
            format!("{} 个行业，单行业最多 {max_industry} 只（上限 2）", industries.len())
        },
        limit_up_count,
    }
}

pub(crate) async fn load_strategy_health(db: &SqlitePool, through_date: &str) -> StrategyHealth {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT date, meta FROM daily_pick \
         WHERE date <= ? AND json_extract(meta,'$.outcome.t1_real') IS NOT NULL \
         ORDER BY date ASC",
    )
    .bind(through_date)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    // 每个推荐日按组合等权聚合，避免一天推荐较多时不成比例放大权重。
    let mut days: BTreeMap<String, (i64, i64, f64, String)> = BTreeMap::new();
    for (date, raw) in rows {
        let Ok(meta) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(real) = meta["outcome"]["t1_real"].as_f64() else {
            continue;
        };
        let regime = meta["cn"]["avg_pct"]
            .as_f64()
            .map(|pct| market_regime(pct).0.to_string())
            .unwrap_or_else(|| "unknown".into());
        let entry = days.entry(date).or_insert((0, 0, 0.0, regime));
        entry.0 += 1;
        entry.1 += i64::from(real > 0.0);
        entry.2 += real;
    }
    let mut daily: Vec<(String, i64, i64, f64, String)> = days
        .into_iter()
        .map(|(date, (samples, wins, sum, regime))| {
            (date, samples, wins, sum / samples as f64, regime)
        })
        .collect();
    if daily.len() > 20 {
        daily.drain(..daily.len() - 20);
    }

    let window = |size: usize| -> HealthWindow {
        let slice = if daily.len() > size {
            &daily[daily.len() - size..]
        } else {
            &daily[..]
        };
        let samples = slice.iter().map(|item| item.1).sum::<i64>();
        let wins = slice.iter().map(|item| item.2).sum::<i64>();
        let mut factor = 1.0_f64;
        let mut peak = 1.0_f64;
        let mut max_drawdown = 0.0_f64;
        for (_, _, _, pct, _) in slice {
            factor *= 1.0 + pct / 100.0;
            peak = peak.max(factor);
            max_drawdown = max_drawdown.min((factor / peak - 1.0) * 100.0);
        }
        HealthWindow {
            days: size as i64,
            completed_days: slice.len() as i64,
            samples,
            win_rate: ratio(wins, samples),
            avg_t1_real: if slice.is_empty() {
                0.0
            } else {
                slice.iter().map(|item| item.3).sum::<f64>() / slice.len() as f64
            },
            cumulative_return: (factor - 1.0) * 100.0,
            max_drawdown,
        }
    };
    let windows = vec![window(10), window(20)];
    let current_loss_streak = daily
        .iter()
        .rev()
        .take_while(|item| item.3 < 0.0)
        .count() as i64;
    let mut factor = 1.0_f64;
    let curve = daily
        .iter()
        .map(|(date, samples, _, pct, regime)| {
            factor *= 1.0 + pct / 100.0;
            HealthCurvePoint {
                date: date.clone(),
                samples: *samples,
                t1_real_pct: *pct,
                cumulative_pct: (factor - 1.0) * 100.0,
                regime: regime.clone(),
            }
        })
        .collect::<Vec<_>>();
    let first_date = daily
        .first()
        .map(|item| item.0.as_str())
        .unwrap_or(through_date);
    let pause_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT source, COUNT(*) FROM pick_pause_audit \
         WHERE date >= ? AND date <= ? GROUP BY source ORDER BY source",
    )
    .bind(first_date)
    .bind(through_date)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    let pause_counts = pause_rows
        .into_iter()
        .map(|(source, count)| PauseCount { source, count })
        .collect::<Vec<_>>();

    let ten = &windows[0];
    let twenty = &windows[1];
    let mut score = 100_i64;
    let mut alerts = Vec::new();
    if current_loss_streak >= 3 {
        score -= 40;
        alerts.push(format!("已连续亏损 {current_loss_streak} 个推荐日，策略应保持暂停"));
    }
    if ten.completed_days >= 5 && ten.win_rate < 0.45 {
        score -= 20;
        alerts.push(format!("近10日真实胜率降至 {:.0}%", ten.win_rate * 100.0));
    }
    if ten.completed_days >= 5 && ten.avg_t1_real < 0.0 {
        score -= 20;
        alerts.push(format!("近10日组合平均收益转负至 {:.2}%", ten.avg_t1_real));
    }
    if ten.max_drawdown <= -8.0 {
        score -= 25;
        alerts.push(format!("近10日累计曲线最大回撤 {:.1}%", ten.max_drawdown));
    }
    if ten.completed_days >= 8
        && twenty.completed_days >= 15
        && ten.win_rate + 0.15 < twenty.win_rate
    {
        score -= 15;
        alerts.push(format!(
            "短窗胜率较20日基线下降 {:.0} 个百分点",
            (twenty.win_rate - ten.win_rate) * 100.0
        ));
    }
    score = score.clamp(0, 100);
    let status = if daily.len() < 5 {
        "insufficient"
    } else if current_loss_streak >= 3 || ten.avg_t1_real <= -1.0 || ten.max_drawdown <= -8.0 {
        "critical"
    } else if !alerts.is_empty() {
        "watch"
    } else {
        "healthy"
    };
    StrategyHealth {
        status: status.into(),
        score,
        completed_days: daily.len() as i64,
        current_loss_streak,
        windows,
        curve,
        pause_counts,
        alerts,
    }
}

pub fn market_regime(avg_pct: f64) -> (&'static str, &'static str) {
    if avg_pct <= -2.0 {
        ("crash", "急跌")
    } else if avg_pct <= -0.8 {
        ("bear", "偏空")
    } else if avg_pct >= 0.8 {
        ("bull", "偏多")
    } else {
        ("range", "震荡")
    }
}

pub async fn list_picks(db: &SqlitePool, date: Option<&str>) -> Result<PicksDocument, PickError> {
    let date_key = match date {
        Some(d) => d.to_string(),
        None => {
            let latest: Option<String> = sqlx::query_scalar(
                "SELECT MAX(date) FROM (SELECT date FROM daily_pick UNION ALL SELECT date FROM daily_pick_run)",
            )
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
                by_regime: Vec::new(),
                execution: Default::default(),
                shadow_experiments: load_shadow_experiment_stats(db).await.unwrap_or_default(),
                hard_filter_exclusions: Vec::new(),
                health: StrategyHealth::default(),
                audit: build_pick_audit(&[]),
            },
            market: serde_json::Value::Null,
            execute_hint: String::new(),
            previous_picks: Vec::new(),
        });
    }
    let run_snapshot: Option<(String, String, i64, String)> = sqlx::query_as(
        "SELECT market, execute_hint, paused, pause_source FROM daily_pick_run WHERE date = ?",
    )
    .bind(&date_key)
    .fetch_optional(db)
    .await?;
    let paused = run_snapshot
        .as_ref()
        .map(|(_, _, paused, _)| *paused != 0)
        .unwrap_or(false);
    let rows: Vec<(String, String, String, i64, f64, String, String, String)> = sqlx::query_as(
        "SELECT date, code, name, rank, score, reasons, ai_note, meta FROM daily_pick \
         WHERE date = ? ORDER BY rank ASC",
    )
    .bind(&date_key)
    .fetch_all(db)
    .await?;
    let picks: Vec<DailyPick> = if paused { Vec::new() } else { rows }
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
    let mut regime_buckets: HashMap<String, (String, i64, i64, f64)> = HashMap::new();
    // §A.8 Wilson 95% 区间：避免「胜率 70% · 样本 10」被误读为稳定指标
    let (mut t1_low, mut t1_high, mut t1_margin, mut t1_sufficient) = confidence_bounds(0, 0);
    let (mut t5_low, mut t5_high, mut t5_margin, mut t5_sufficient) = confidence_bounds(0, 0);
    for meta in &outcomes {
        if let Ok(value) = serde_json::from_str::<Value>(meta) {
            if let Some(pct) = value["outcome"]["t1_pct"].as_f64() {
                t1_samples += 1;
                t1_sum += pct;
                if pct > 0.0 {
                    t1_win += 1.0;
                }
                if let Some(avg_pct) = value["cn"]["avg_pct"].as_f64() {
                    let (regime, label) = market_regime(avg_pct);
                    let entry = regime_buckets.entry(regime.to_string()).or_insert((
                        label.to_string(),
                        0,
                        0,
                        0.0,
                    ));
                    entry.1 += 1;
                    if pct > 0.0 {
                        entry.2 += 1;
                    }
                    entry.3 += pct;
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
    let mut by_regime = Vec::new();
    for regime in ["bull", "range", "bear", "crash"] {
        let Some((label, samples, wins, sum)) = regime_buckets.remove(regime) else {
            continue;
        };
        let (low, high, margin, sufficient) = confidence_bounds(samples, wins);
        by_regime.push(RegimeStat {
            regime: regime.to_string(),
            label,
            samples,
            win_rate: wins as f64 / samples as f64,
            win_rate_low: low,
            win_rate_high: high,
            win_rate_margin: margin,
            samples_sufficient: sufficient,
            avg_t1_pct: sum / samples as f64,
        });
    }

    // 真实执行口径统计（从 meta.outcome 提取）
    let exec_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT date, meta FROM daily_pick \
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
    let mut target_5pct_samples = 0_i64;
    let mut target_5pct_hits = 0_i64;
    let mut daily_execution: BTreeMap<String, (i64, f64, f64)> = BTreeMap::new();
    for (date, meta) in &exec_rows {
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
                if let Some(paper) = o["t1_pct"].as_f64() {
                    let entry = daily_execution.entry(date.clone()).or_insert((0, 0.0, 0.0));
                    entry.0 += 1;
                    entry.1 += paper;
                    entry.2 += r;
                }
            }
            if let Some(d) = o["max_dd"].as_f64() {
                dds.push(d);
            }
            if let Some(hit) = o["target_hit_5pct"].as_bool() {
                target_5pct_samples += 1;
                if hit {
                    target_5pct_hits += 1;
                }
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
    let (target_low, target_high, _) = wilson_interval(target_5pct_samples, target_5pct_hits, 1.96);
    let mut paper_factor = 1.0_f64;
    let mut open_factor = 1.0_f64;
    let mut curve = Vec::new();
    for (date, (samples, paper_sum, open_sum)) in daily_execution {
        let paper = paper_sum / samples as f64;
        let open = open_sum / samples as f64;
        paper_factor *= 1.0 + paper / 100.0;
        open_factor *= 1.0 + open / 100.0;
        curve.push(crate::model::pick::ExecutionCurvePoint {
            date,
            samples,
            paper_t1_pct: paper,
            open_t1_pct: open,
            paper_cumulative_pct: (paper_factor - 1.0) * 100.0,
            open_cumulative_pct: (open_factor - 1.0) * 100.0,
        });
    }
    if curve.len() > 20 {
        curve.drain(..curve.len() - 20);
    }
    let execution_drag = if t1_samples > 0 && n > 0 {
        t1_sum / t1_samples as f64 - avg(&t1_reals)
    } else {
        0.0
    };
    let execution_warning = n >= 5 && execution_drag >= 0.8;
    let execution_hint = if execution_warning {
        format!(
            "次日开盘执行较纸面回测平均少 {execution_drag:.1} 个百分点，当前清单仅作观察，不按纸面胜率推断可执行收益"
        )
    } else {
        String::new()
    };
    let mut sorted_reals = t1_reals.clone();
    sorted_reals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let expected_shortfall_10 = if sorted_reals.is_empty() {
        0.0
    } else {
        let count = ((sorted_reals.len() as f64 * 0.1).ceil() as usize).max(1);
        avg(&sorted_reals[..count])
    };
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
        avg_win: avg(&wins),
        avg_loss: avg(&losses),
        avg_max_dd: avg(&dds),
        max_drawdown: dds.iter().copied().reduce(f64::min).unwrap_or(0.0),
        expected_shortfall_10,
        target_5pct_samples,
        target_5pct_hit_rate: if target_5pct_samples > 0 {
            target_5pct_hits as f64 / target_5pct_samples as f64
        } else {
            0.0
        },
        target_5pct_wilson_low: target_low,
        target_5pct_wilson_high: target_high,
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
        curve,
        execution_drag,
        execution_warning,
        execution_hint,
    };

    // 优先读取运行快照；兼容旧数据时再由首条 pick 的 meta 组装。
    let mut market = run_snapshot
        .as_ref()
        .and_then(|(market, _, _, _)| serde_json::from_str::<Value>(market).ok())
        .or_else(|| {
            picks.first().map(|p| {
                serde_json::json!({
                    "djia": p.meta.get("us").and_then(|us| us.get("djia")).cloned().unwrap_or(Value::Null),
                    "ixic": p.meta.get("us").and_then(|us| us.get("ixic")).cloned().unwrap_or(Value::Null),
                    "us": p.meta.get("us").cloned().unwrap_or(Value::Null),
                    "cn": p.meta.get("cn").cloned().unwrap_or(Value::Null),
                })
            })
        })
        .unwrap_or(Value::Null);
    if let Some((_, _, true, source)) = run_snapshot
        .as_ref()
        .map(|(market, hint, paused, source)| (market, hint, *paused != 0, source))
    {
        if !source.is_empty() {
            market["pause_source"] = Value::String(source.clone());
        }
    }
    let pause_audit: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT source, reason, active, created_at FROM pick_pause_audit \
         WHERE date = ? ORDER BY created_at ASC, id ASC",
    )
    .bind(&date_key)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    if !pause_audit.is_empty() {
        market["pause_audit"] = Value::Array(
            pause_audit
                .into_iter()
                .map(|(source, reason, active, created_at)| {
                    serde_json::json!({
                        "source": source,
                        "reason": reason,
                        "active": active != 0,
                        "created_at": created_at,
                    })
                })
                .collect(),
        );
    }
    let execute_hint = run_snapshot
        .as_ref()
        .map(|(_, hint, _, _)| hint.clone())
        .unwrap_or_default();
    let previous_picks = fetch_previous_picks(
        db,
        NaiveDate::parse_from_str(&date_key, "%Y-%m-%d")
            .unwrap_or_else(|_| Utc::now().date_naive()),
    )
    .await
    .unwrap_or_default();
    let health = load_strategy_health(db, &date_key).await;
    let audit = build_pick_audit(&picks);
    Ok(PicksDocument {
        date: date_key.clone(),
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
            by_regime,
            execution,
            shadow_experiments: load_shadow_experiment_stats(db).await.unwrap_or_default(),
            hard_filter_exclusions: load_hard_filter_stats(db, &date_key)
                .await
                .unwrap_or_default(),
            health,
            audit,
        },
        market,
        execute_hint,
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
           (json_extract(meta,'$.plan.target_return_pct') IS NOT NULL AND \
            json_extract(meta,'$.outcome.target_hit_5pct') IS NULL) OR \
           (json_extract(meta,'$.plan.buy_price_low') IS NOT NULL AND \
            json_extract(meta,'$.outcome.entry_status') IS NULL) OR \
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
        let buy_low = meta["plan"]["buy_price_low"].as_f64().unwrap_or(0.0);
        let buy_high = meta["plan"]["buy_price_high"].as_f64().unwrap_or(0.0);
        let plan_entry_timing = meta["plan"]["entry_timing"]
            .as_str()
            .unwrap_or("next_session_open")
            .to_string();
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
                let gap = if plan_entry_timing == "today_close" {
                    0.0
                } else {
                    (entry_open - base) / base * 100.0
                };
                outcome_map.insert("entry_open".into(), serde_json::json!(entry_open));
                outcome_map.insert("entry_gap".into(), serde_json::json!(gap));
                changed = true;
            }
        }
        let entry_open = outcome_map.get("entry_open").and_then(Value::as_f64);
        if outcome_map
            .get("entry_status")
            .and_then(Value::as_str)
            .is_none()
        {
            if let Some(open) = entry_open {
                if let Some((status, label)) = classify_entry_validity(open, buy_low, buy_high) {
                    outcome_map.insert("entry_status".into(), serde_json::json!(status));
                    outcome_map.insert("entry_status_label".into(), serde_json::json!(label));
                    changed = true;
                }
            }
        }
        let entry = if plan_entry_timing == "today_close" {
            base
        } else {
            entry_open.unwrap_or(base)
        };
        if outcome_map
            .get("target_hit_5pct")
            .and_then(Value::as_bool)
            .is_none()
            && index + 1 < bars.len()
        {
            let high = bars[index + 1].high;
            let target = entry * (1.0 + TARGET_NET_RETURN + TARGET_COST_BUFFER);
            outcome_map.insert("execution_entry".into(), serde_json::json!(entry));
            outcome_map.insert(
                "execution_basis".into(),
                serde_json::json!(if plan_entry_timing == "today_close" {
                    "tail_close"
                } else {
                    "next_open"
                }),
            );
            outcome_map.insert("t1_high".into(), serde_json::json!(high));
            outcome_map.insert("target_price_5pct".into(), serde_json::json!(target));
            outcome_map.insert("target_hit_5pct".into(), serde_json::json!(high >= target));
            changed = true;
        }
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
    updated += backfill_shadow_outcomes(state, today).await?;
    Ok(updated)
}

async fn backfill_shadow_outcomes(state: &AppState, today: NaiveDate) -> Result<usize, PickError> {
    let since = (today - Duration::days(180)).format("%Y-%m-%d").to_string();
    let today_key = today.format("%Y-%m-%d").to_string();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT date, experiment_id, code FROM daily_pick_shadow \
         WHERE date >= ? AND date < ? AND (\
            json_extract(meta,'$.outcome.t1_real') IS NULL OR \
            json_extract(meta,'$.outcome.target_hit_5pct') IS NULL\
         ) ORDER BY date DESC",
    )
    .bind(&since)
    .bind(&today_key)
    .fetch_all(&state.db)
    .await?;
    let mut updated = 0usize;
    for (date, experiment_id, code) in rows {
        let Ok(bars) = fetch_days_cached(state, &code, 220).await else {
            continue;
        };
        let Some(index) = bars.iter().position(|bar| bar.date == date) else {
            continue;
        };
        if index + 1 >= bars.len() {
            continue;
        }
        let raw: String = sqlx::query_scalar(
            "SELECT meta FROM daily_pick_shadow \
             WHERE date=? AND experiment_id=? AND code=?",
        )
        .bind(&date)
        .bind(&experiment_id)
        .bind(&code)
        .fetch_one(&state.db)
        .await?;
        let mut meta: Value = serde_json::from_str(&raw).unwrap_or_default();
        let base = meta["close"].as_f64().unwrap_or(bars[index].close);
        if base <= 0.0 {
            continue;
        }
        let timing = meta["plan"]["entry_timing"]
            .as_str()
            .unwrap_or("next_session_open");
        let entry_open = bars[index + 1].open;
        let entry = if timing == "today_close" || entry_open <= 0.0 {
            base
        } else {
            entry_open
        };
        let t1_close = bars[index + 1].close;
        let t1_high = bars[index + 1].high;
        let target = entry * (1.0 + TARGET_NET_RETURN + TARGET_COST_BUFFER);
        let outcome = serde_json::json!({
            "entry_open": entry_open,
            "execution_entry": entry,
            "execution_basis": if timing == "today_close" { "tail_close" } else { "next_open" },
            "t1_close": t1_close,
            "t1_real": (t1_close - entry) / entry * 100.0,
            "t1_high": t1_high,
            "target_price_5pct": target,
            "target_hit_5pct": t1_high >= target,
        });
        if !meta.is_object() {
            meta = serde_json::json!({});
        }
        meta.as_object_mut()
            .expect("shadow meta normalized")
            .insert("outcome".into(), outcome);
        sqlx::query(
            "UPDATE daily_pick_shadow SET meta=? \
             WHERE date=? AND experiment_id=? AND code=?",
        )
        .bind(meta.to_string())
        .bind(&date)
        .bind(&experiment_id)
        .bind(&code)
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
    fn overnight_and_cn_joint_gate_requires_both_legs() {
        let us = UsSentiment {
            djia_pct: Some(-1.0),
            ixic_pct: Some(-1.6),
        };
        let weak_cn = CnSentiment {
            sh_pct: Some(-1.0),
            sz_pct: Some(-1.2),
            gem_pct: Some(-1.1),
            hs300_pct: Some(-1.0),
        };
        assert!(joint_market_should_pause(Some(&weak_cn), Some(&us)));

        let flat_cn = CnSentiment {
            sh_pct: Some(-0.3),
            sz_pct: Some(-0.5),
            gem_pct: Some(-0.4),
            hs300_pct: Some(-0.2),
        };
        assert!(!joint_market_should_pause(Some(&flat_cn), Some(&us)));
        assert!(!joint_market_should_pause(
            Some(&weak_cn),
            Some(&UsSentiment {
                djia_pct: Some(-0.5),
                ixic_pct: Some(-1.4),
            })
        ));
    }

    #[test]
    fn intraday_confirmation_detects_regime_flip_and_low_overlap() {
        let morning_codes = vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()];
        let stable_tail = vec!["a".into(), "b".into(), "c".into(), "x".into(), "y".into()];
        let range = serde_json::json!({"cn": {"avg_pct": 0.1}});
        let stable = compare_intraday_snapshots(&morning_codes, &range, &stable_tail, &range);
        assert_eq!(stable.status, "confirmed");
        assert!((stable.overlap_ratio - 0.6).abs() < 1e-9);

        let bear = serde_json::json!({"cn": {"avg_pct": -1.1}});
        let flipped = compare_intraday_snapshots(&morning_codes, &range, &stable_tail, &bear);
        assert_eq!(flipped.status, "observe_only");
        assert!(flipped.regime_changed);

        let unstable_tail = vec!["a".into(), "x".into(), "y".into(), "z".into(), "q".into()];
        let unstable = compare_intraday_snapshots(&morning_codes, &range, &unstable_tail, &range);
        assert_eq!(unstable.status, "observe_only");
        assert!(!unstable.regime_changed);
        assert!((unstable.overlap_ratio - 0.2).abs() < 1e-9);
    }

    #[test]
    fn beta_and_liquidity_metrics_are_bounded_by_available_data() {
        let mut market_close = 100.0;
        let mut stock_close = 50.0;
        let mut market = Vec::new();
        let mut stock = Vec::new();
        for day in 0..31 {
            if day > 0 {
                let market_return = if day % 2 == 0 { 0.01 } else { -0.006 };
                market_close *= 1.0 + market_return;
                stock_close *= 1.0 + market_return * 2.0;
            }
            let date = format!("2026-08-{:02}", day + 1);
            market.push(DayBar {
                code: "sh000300".into(),
                date: date.clone(),
                open: market_close,
                high: market_close,
                low: market_close,
                close: market_close,
                volume: 1,
                amount: 1_000_000_000.0,
            });
            stock.push(DayBar {
                code: "sz000001".into(),
                date,
                open: stock_close,
                high: stock_close,
                low: stock_close,
                close: stock_close,
                volume: 1,
                amount: 30_000_000.0,
            });
        }
        let risk = stock_risk_metrics(&stock, &market, Some(1.2), Some(1.0));
        assert!((risk.beta_60d.unwrap() - 2.0).abs() < 0.05);
        assert_eq!(risk.avg_amount_20d, Some(30_000_000.0));
        assert!(risk.max_drawdown_20d.unwrap() < 0.0);
        assert!(risk.expected_shortfall_10pct_20d.unwrap() < 0.0);
        assert!(risk.liquidity_blocked);

        for bar in &mut stock {
            bar.amount = 0.0;
        }
        let missing_amount = stock_risk_metrics(&stock, &market, None, None);
        assert_eq!(missing_amount.avg_amount_20d, None);
        assert!(
            !missing_amount.liquidity_blocked,
            "缺字段时应降级而不是误杀"
        );
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
                        turnover_pct: None,
                        volume_ratio: None,
                        pool_sources: vec!["momentum".into()],
                    },
                    Candidate {
                        code: format!("sz30001{i}"),
                        name: "乙".into(),
                        price: 10.0,
                        pct: if *industry == "半导体" { 3.0 } else { 0.5 },
                        industry: industry.to_string(),
                        is_limit_up: false,
                        turnover_pct: None,
                        volume_ratio: None,
                        pool_sources: vec!["momentum".into()],
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
    fn diversified_pool_includes_relative_strength_and_pullback() {
        let make = |code: &str, pct: f64, industry: &str, source: &str| Candidate {
            code: code.into(),
            name: code.into(),
            price: 10.0,
            pct,
            industry: industry.into(),
            is_limit_up: false,
            turnover_pct: Some(5.0),
            volume_ratio: Some(1.2),
            pool_sources: vec![source.into()],
        };
        let candidates = vec![
            make("sh600001", 8.0, "热点", "momentum"),
            make("sh600002", 2.0, "冷门", "active"),
            make("sh600003", 1.0, "稳健", "active"),
        ];
        let industry_avg = HashMap::from([
            ("热点".to_string(), 8.0),
            ("冷门".to_string(), 0.0),
            ("稳健".to_string(), 1.0),
        ]);
        let selected = select_diversified_candidates(&candidates, &industry_avg, Some(-1.0), 10);
        assert_eq!(selected.len(), 3);
        let relative = selected
            .iter()
            .find(|item| item.code == "sh600002")
            .unwrap();
        assert!(relative
            .pool_sources
            .iter()
            .any(|item| item == "relative_strength"));
        let pullback = selected
            .iter()
            .find(|item| item.code == "sh600003")
            .unwrap();
        assert!(pullback.pool_sources.iter().any(|item| item == "pullback"));
    }

    #[test]
    fn industry_cap_rejects_third_pick_from_same_industry() {
        let make = |code: &str, industry: &str| {
            (
                Candidate {
                    code: code.into(),
                    name: code.into(),
                    price: 10.0,
                    pct: 2.0,
                    industry: industry.into(),
                    is_limit_up: false,
                    turnover_pct: Some(5.0),
                    volume_ratio: Some(1.0),
                    pool_sources: vec!["momentum".into()],
                },
                80.0,
                vec!["放量".into()],
            )
        };
        let mut picks = Vec::new();
        assert!(push_with_industry_cap(&mut picks, &make("a", "半导体"), 2));
        assert!(push_with_industry_cap(&mut picks, &make("b", "半导体"), 2));
        assert!(!push_with_industry_cap(&mut picks, &make("c", "半导体"), 2));
        assert!(push_with_industry_cap(&mut picks, &make("d", "银行"), 2));
        assert_eq!(picks.len(), 3);
    }

    #[test]
    fn limit_up_strength_separates_strong_and_weak_boards() {
        let bars = vec![
            DayBar {
                code: "sh600001".into(),
                date: "2026-09-22".into(),
                open: 10.0,
                high: 10.1,
                low: 9.9,
                close: 10.0,
                volume: 100,
                amount: 1_000.0,
            },
            DayBar {
                code: "sh600001".into(),
                date: "2026-09-23".into(),
                open: 10.1,
                high: 11.0,
                low: 10.0,
                close: 11.0,
                volume: 200,
                amount: 2_000.0,
            },
        ];
        let strong_candidate = Candidate {
            code: "sh600001".into(),
            name: "强板".into(),
            price: 12.1,
            pct: 10.0,
            industry: "半导体".into(),
            is_limit_up: true,
            turnover_pct: Some(8.0),
            volume_ratio: Some(1.8),
            pool_sources: vec!["momentum".into()],
        };
        let strong = classify_limit_up_strength(&strong_candidate, &bars, true);
        assert_eq!(strong.tier, "strong");
        assert_eq!(strong.score_delta, 0.0, "强板只免扣分，不再盲目加分");
        assert_eq!(strong.prior_streak, 1);

        let weak_candidate = Candidate {
            code: "sh600002".into(),
            name: "弱板".into(),
            price: 11.0,
            pct: 10.0,
            industry: "冷门".into(),
            is_limit_up: true,
            turnover_pct: Some(0.5),
            volume_ratio: Some(0.4),
            pool_sources: vec!["momentum".into()],
        };
        let weak = classify_limit_up_strength(&weak_candidate, &bars[..1], false);
        assert_eq!(weak.tier, "weak");
        assert_eq!(weak.score_delta, -12.0);
        assert_eq!(weak.prior_streak, 0);
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
        let too_small = TagWindowBucket {
            recent_samples: 8,
            recent_wins: 8,
            prior_samples: 11,
            prior_wins: 11,
        }
        .performance("all");
        assert_eq!(learned_tag_delta(&too_small), 0.0, "30 样本前不调权");

        let stable_positive = TagWindowBucket {
            recent_samples: 20,
            recent_wins: 16,
            prior_samples: 40,
            prior_wins: 30,
        }
        .performance("range");
        assert!(learned_tag_delta(&stable_positive) > 0.0);

        let stable_negative = TagWindowBucket {
            recent_samples: 20,
            recent_wins: 4,
            prior_samples: 40,
            prior_wins: 10,
        }
        .performance("bear");
        assert!(learned_tag_delta(&stable_negative) < 0.0);

        let regime_flip = TagWindowBucket {
            recent_samples: 10,
            recent_wins: 2,
            prior_samples: 30,
            prior_wins: 24,
        }
        .performance("all");
        assert_eq!(learned_tag_delta(&regime_flip), 0.0, "训练/验证翻转时禁用");

        let all_wins = TagWindowBucket {
            recent_samples: 40,
            recent_wins: 40,
            prior_samples: 80,
            prior_wins: 80,
        }
        .performance("all");
        assert_eq!(learned_tag_delta(&all_wins), 10.0, "单标签上限 +10");

        let performance = HashMap::from([
            (
                "MACD金叉".to_string(),
                TagWindowBucket {
                    recent_samples: 30,
                    recent_wins: 27,
                    prior_samples: 70,
                    prior_wins: 63,
                }
                .performance("range"),
            ),
            (
                "放量".to_string(),
                TagWindowBucket {
                    recent_samples: 30,
                    recent_wins: 28,
                    prior_samples: 70,
                    prior_wins: 65,
                }
                .performance("range"),
            ),
            (
                "样本少".to_string(),
                TagWindowBucket {
                    recent_samples: 5,
                    recent_wins: 5,
                    prior_samples: 7,
                    prior_wins: 7,
                }
                .performance("all"),
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
        assert_eq!(evidence["method"], "walk_forward_wilson");
        assert_eq!(evidence["outcome_basis"], "t1_real");
    }

    #[test]
    fn calibrated_probability_abstains_only_with_sufficient_weak_evidence() {
        let sample = |won: bool| CalibrationSample {
            tags: BTreeSet::from(["MACD金叉".into(), "放量".into(), "相对强势".into()]),
            regime: Some("range".into()),
            won,
            target_hit_5pct: Some(won),
        };
        let tags = vec!["MACD金叉".into(), "放量".into(), "相对强势".into()];

        let small = (0..10).map(|index| sample(index < 2)).collect::<Vec<_>>();
        let small_result = calibrate_candidate(&tags, &small, Some("range"));
        assert_eq!(small_result.confidence_tier, "insufficient");
        assert!(!small_result.abstain, "小样本不得主动弃权");

        let weak = (0..40).map(|index| sample(index < 8)).collect::<Vec<_>>();
        let weak_result = calibrate_candidate(&tags, &weak, Some("range"));
        assert_eq!(weak_result.samples, 40);
        assert_eq!(weak_result.confidence_tier, "weak");
        assert!(weak_result.wilson_high < 0.5);
        assert!(weak_result.abstain);
        assert_eq!(weak_result.score_delta, 0.0);

        let strong = (0..40).map(|index| sample(index < 34)).collect::<Vec<_>>();
        let strong_result = calibrate_candidate(&tags, &strong, Some("range"));
        assert_eq!(strong_result.confidence_tier, "strong");
        assert!(strong_result.wilson_low > 0.5);
        assert!(!strong_result.abstain);
        assert!(strong_result.score_delta > 0.0);
        assert!(strong_result.posterior_win_probability < 0.85);
    }

    #[tokio::test]
    async fn walk_forward_weights_exclude_future_and_prefer_current_regime() {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE daily_pick(date TEXT, reasons TEXT, meta TEXT)")
            .execute(&db)
            .await
            .unwrap();

        async fn insert_samples(
            db: &SqlitePool,
            date: &str,
            samples: usize,
            won: bool,
            avg_pct: f64,
            real_outcome: bool,
        ) {
            for _ in 0..samples {
                let outcome = if real_outcome {
                    serde_json::json!({"t1_real": if won { 1.0 } else { -1.0 }})
                } else {
                    serde_json::json!({"t1_pct": if won { 1.0 } else { -1.0 }})
                };
                let meta = serde_json::json!({
                    "cn": {"avg_pct": avg_pct},
                    "outcome": outcome,
                });
                sqlx::query("INSERT INTO daily_pick(date, reasons, meta) VALUES(?, ?, ?)")
                    .bind(date)
                    .bind(r#"["MACD金叉"]"#)
                    .bind(meta.to_string())
                    .execute(db)
                    .await
                    .unwrap();
            }
        }

        // 当前震荡环境：训练窗 22 + 验证窗 8，方向稳定且刚好达到介入门槛。
        insert_samples(&db, "2026-07-10", 22, true, 0.0, true).await;
        insert_samples(&db, "2026-09-10", 8, true, 0.0, true).await;
        // 其他环境的失败样本不应污染当前震荡分层。
        insert_samples(&db, "2026-07-10", 22, false, -1.0, true).await;
        insert_samples(&db, "2026-09-10", 8, false, -1.0, true).await;
        // as_of 之后即使已有值也必须排除；只有纸面 t1_pct、未回写 t1_real 也必须排除。
        insert_samples(&db, "2026-10-01", 20, false, 0.0, true).await;
        insert_samples(&db, "2026-09-10", 20, false, 0.0, false).await;

        let stats = load_tag_performance(
            &db,
            NaiveDate::from_ymd_opt(2026, 9, 25).unwrap(),
            Some("range"),
        )
        .await
        .unwrap();
        let stat = stats.get("MACD金叉").unwrap();
        assert_eq!(stat.regime_scope, "range");
        assert_eq!(stat.samples, 30);
        assert_eq!(stat.recent_samples, 8);
        assert_eq!(stat.prior_samples, 22);
        assert_eq!(stat.win_rate, 1.0);
        assert!(learned_tag_delta(stat) > 0.0);
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
        // 买区 [9.80, 10.20]；卖价必须覆盖买区上界成交后的 5% 净目标。
        assert!((short["buy_price_low"].as_f64().unwrap() - 9.80).abs() < 1e-9);
        assert!((short["buy_price_high"].as_f64().unwrap() - 10.20).abs() < 1e-9);
        assert!((short["buy_basis_close"].as_f64().unwrap() - 10.00).abs() < 1e-9);
        let sell_low = short["sell_price_low"].as_f64().unwrap();
        let sell_high = short["sell_price_high"].as_f64().unwrap();
        assert!((sell_low - 10.73).abs() < 0.02, "sell_low={sell_low}");
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
        // 历史区间低于 5% 目标时自动抬升。
        assert!((with_base["sell_price_low"].as_f64().unwrap() - 21.46).abs() < 0.02);
        assert!((with_base["sell_price_high"].as_f64().unwrap() - 21.68).abs() < 0.02);

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
    fn entry_validity_rejects_breakdown_and_excessive_gap() {
        assert_eq!(
            classify_entry_validity(9.70, 9.80, 10.20).map(|(status, _)| status),
            Some("invalid_below")
        );
        assert_eq!(
            classify_entry_validity(10.50, 9.80, 10.20).map(|(status, _)| status),
            Some("invalid_gap")
        );
        assert_eq!(
            classify_entry_validity(10.25, 9.80, 10.20).map(|(status, _)| status),
            Some("valid")
        );
    }

    #[test]
    fn dynamic_exit_separates_profit_target_from_risk_exit() {
        let signal = NaiveDate::from_ymd_opt(2026, 9, 24).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let strong = dynamic_exit_plan(signal, today, 10.20, 10.0, Some(10.20), None, None)
            .unwrap();
        assert_eq!(strong.open_strength, "strong");
        assert_eq!(strong.action, "hold_strength");
        assert!((strong.sell_low - 10.73).abs() < 0.02);
        assert!(strong.sell_high > strong.sell_low * 1.015);

        let weak = dynamic_exit_plan(signal, today, 9.80, 10.0, Some(9.80), None, None).unwrap();
        assert_eq!(weak.open_strength, "weak");
        assert_eq!(weak.action, "risk_control");
        assert!(weak.sell_high < weak.sell_low * 1.006);
        assert!(weak.risk_stop < weak.sell_low);
        assert!((weak.sell_low / 9.80 - 1.052).abs() < 0.002);

        let breached = dynamic_exit_plan(
            signal,
            today,
            10.0,
            10.0,
            Some(9.9),
            Some(9.6),
            None,
        )
        .unwrap();
        assert_eq!(breached.action, "risk_exit");
        assert_eq!(breached.target_reached, Some(false));

        let timeout = dynamic_exit_plan(
            signal,
            today,
            10.0,
            10.0,
            Some(10.0),
            Some(10.2),
            None,
        )
        .unwrap();
        assert_eq!(timeout.action, "t1_timeout");
        assert!(timeout.action_label.contains("T+1"));
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
    fn calendar_filter_blocks_realized_events_without_future_leakage() {
        use chrono::TimeZone;
        let cn = FixedOffset::east_opt(8 * 3600).unwrap();
        let as_of = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let make = |title: &str, day: u32| crate::model::NewsItem {
            code: "sz300623".into(),
            title: title.into(),
            summary: String::new(),
            media: "交易所".into(),
            url: "https://news.example.com/event".into(),
            published_at: cn
                .with_ymd_and_hms(2026, 9, day, 10, 0, 0)
                .unwrap()
                .with_timezone(&Utc),
        };

        assert!(calendar_hard_block(&[make("股东减持计划公告", 25)], as_of).is_none());
        let block = calendar_hard_block(&[make("股东累计减持达到1%", 24)], as_of).unwrap();
        assert_eq!(block.reason, "减持实施");
        assert!(calendar_hard_block(&[make("公司被证监会立案调查", 20)], as_of).is_some());
        assert!(calendar_hard_block(&[make("限售股上市流通公告", 17)], as_of).is_none());
        assert!(calendar_hard_block(&[make("公司股票复牌公告", 23)], as_of).is_none());
        assert!(calendar_hard_block(&[make("公司股票复牌公告", 25)], as_of).is_some());
        assert!(calendar_hard_block(&[make("业绩预告大幅下修并首亏", 24)], as_of).is_some());
        assert!(calendar_hard_block(&[make("公司被证监会立案调查", 26)], as_of).is_none());
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
        assert!(
            low < 0.5,
            "下界应低于 0.5（说明 70% 胜率无统计意义）：{low}"
        );

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

    // §A.10 大盘情绪解析：四条指数 + risk_score 分档
    #[test]
    fn cn_index_parse_and_risk_score() {
        // 真实接口字段顺序：[0]市场类型 [1]名称 [2]代码 [3..6]价格三件套
        // pct 在字段索引 30。构造 31 字段，第 30 位为 pct。
        fn build(code: &str, pct: f64) -> String {
            let mut s = format!("v_x={}1~X~{}~100~100~100~vol", '"', code);
            // 字段 7..29 (23 个零填充位)
            for _ in 0..23 {
                s.push_str("~0");
            }
            // 字段 30 = pct
            s.push_str(&format!("~{pct}{}", '"'));
            s
        }
        let body = format!(
            "{}\n{}\n{}\n{}\n",
            build("000001", -2.05),
            build("399001", -2.10),
            build("399006", -2.30),
            build("000300", -2.15),
        );
        let cn = parse_cn_index(&body);
        assert!(
            cn.sh_pct.is_some(),
            "sh_pct 应解析：{:?} / body={body}",
            cn.sh_pct
        );
        assert!((cn.sh_pct.unwrap() - (-2.05)).abs() < 0.001);
        let avg = cn.avg_pct().unwrap();
        assert!(avg < -2.0, "avg 应 < -2.0：{avg}");
        assert!(cn.should_pause(), "三指数均值 ≤ -2.0 应暂停");
        let (delta, tag) = cn.risk_score();
        assert_eq!(delta, -30.0, "≤-2% 应打 -30：{delta}");
        assert_eq!(tag.as_deref(), Some("大盘大跌"));
    }

    #[test]
    fn cn_index_risk_score_tiers() {
        // -1% 区间 → -15 偏弱
        let cn = CnSentiment {
            sh_pct: Some(-1.0),
            sz_pct: Some(-1.0),
            gem_pct: Some(-1.0),
            hs300_pct: Some(-1.0),
        };
        let (d, t) = cn.risk_score();
        assert_eq!(d, -15.0);
        assert_eq!(t.as_deref(), Some("大盘偏弱"));
        // +1.2% → +10 偏多
        let cn = CnSentiment {
            sh_pct: Some(1.2),
            sz_pct: Some(1.0),
            gem_pct: Some(1.5),
            hs300_pct: Some(1.0),
        };
        let (d, t) = cn.risk_score();
        assert_eq!(d, 10.0);
        assert_eq!(t.as_deref(), Some("大盘偏多"));
        // 全空 → 0
        let cn = CnSentiment::default();
        assert_eq!(cn.risk_score(), (0.0, None));
        assert_eq!(cn.avg_pct(), None);
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
