//! 智能推荐 DTO。
use serde::{Deserialize, Serialize};

/// 单条推荐（落库行 / API 返回同构）。
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DailyPick {
    /// 推荐基准日（最近已收盘交易日，北京时间）
    pub date: String,
    pub code: String,
    pub name: String,
    /// 1 起
    pub rank: i64,
    /// 量化 + 消息面总分
    pub score: f64,
    /// 标签（MACD金叉 / 放量 / 研报看多 …）
    pub reasons: Vec<String>,
    /// AI 一句话理由（无 AI 精排时为空）
    #[serde(default)]
    pub ai_note: String,
    /// close / pct / industry / boost / outcome{t1_pct,t5_pct}
    #[serde(default)]
    pub meta: serde_json::Value,
}

/// 推荐文档：当日清单 + 近 30 天回测统计。
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PicksDocument {
    pub date: String,
    pub picks: Vec<DailyPick>,
    /// 有 outcome 的样本统计；无样本时各值为 0
    pub stats: PickStats,
    /// 生成时的市场环境（隔夜美股涨跌等）；缺省 {}
    #[serde(default)]
    pub market: serde_json::Value,
    /// 执行时机提示：「当天下午可买入」或「次日开盘买入」
    #[serde(default)]
    pub execute_hint: String,
    /// §A.9 上一交易日推荐（只含近 30 天内已有 T+1 outcome 回写的票，
    /// 用于在「AI 精选」卡下方展示「昨日推荐 → 卖出参考」）。
    #[serde(default)]
    pub previous_picks: Vec<PreviousPick>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PickStats {
    pub samples: i64,
    /// 主目标：T+1 收盘价高于推荐日尾盘基准价的比例（0..1）。
    #[serde(default)]
    pub t1_win_rate: f64,
    /// Wilson 95% 置信区间下界（样本 < 30 时宽度大，避免误读）
    #[serde(default)]
    pub t1_win_rate_low: f64,
    /// Wilson 95% 置信区间上界
    #[serde(default)]
    pub t1_win_rate_high: f64,
    /// 置信区间半宽（绝对值）。前端可直接读 `t1_win_rate ± t1_win_rate_margin`
    #[serde(default)]
    pub t1_win_rate_margin: f64,
    /// 「样本是否足以给出有意义胜率」的便捷阈值。默认 false 表示样本 < 30
    #[serde(default)]
    pub t1_samples_sufficient: bool,
    #[serde(default)]
    pub avg_t1_pct: f64,
    /// 兼容中线观察的 T+5 口径。
    #[serde(default)]
    pub t5_samples: i64,
    pub t5_win_rate: f64,
    #[serde(default)]
    pub t5_win_rate_low: f64,
    #[serde(default)]
    pub t5_win_rate_high: f64,
    #[serde(default)]
    pub t5_win_rate_margin: f64,
    #[serde(default)]
    pub t5_samples_sufficient: bool,
    pub avg_t5_pct: f64,
    /// 标签级 T+1 回测：哪个因子更适合隔日目标。
    #[serde(default)]
    pub tags: Vec<PickTagStat>,
    /// §A.11 按推荐日 A 股大盘环境分桶的 T+1 统计。
    #[serde(default)]
    pub by_regime: Vec<RegimeStat>,
    /// 真实执行口径（次日开盘买入，非收盘价）
    #[serde(default)]
    pub execution: ExecutionStats,
    /// §A.14 影子实验只展示统计，不混入正式 picks。
    #[serde(default)]
    pub shadow_experiments: Vec<ShadowExperimentStat>,
    /// 当日硬过滤汇总，便于解释“为什么今天少推/不推”。
    #[serde(default)]
    pub hard_filter_exclusions: Vec<HardFilterStat>,
    /// §A.14 近 10/20 个已完成推荐日的滚动策略健康度。
    #[serde(default)]
    pub health: StrategyHealth,
    /// §A.16 已落地概率预测的样本外校准质量。
    #[serde(default)]
    pub calibration_quality: CalibrationQuality,
    /// §A.14 生产规则版本与关键阈值清单，用于复现实验。
    #[serde(default)]
    pub audit: PickAudit,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CalibrationQuality {
    /// reliable / degraded / insufficient
    pub status: String,
    pub samples: i64,
    pub brier_score: f64,
    pub expected_calibration_error: f64,
    pub log_loss: f64,
    pub auc: Option<f64>,
    /// 只有足量历史预测且质量达标时，概率证据才可产生正向加分。
    pub positive_boost_enabled: bool,
    pub reason: String,
    #[serde(default)]
    pub bins: Vec<CalibrationBin>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CalibrationBin {
    pub lower: f64,
    pub upper: f64,
    pub samples: i64,
    pub average_probability: f64,
    pub observed_win_rate: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PickAudit {
    pub experiment_id: String,
    pub rule_version: String,
    pub rule_hash: String,
    #[serde(default)]
    pub manifest: serde_json::Value,
    #[serde(default)]
    pub pool_sources: Vec<String>,
    pub concentration_summary: String,
    pub limit_up_count: i64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StrategyHealth {
    /// healthy / watch / critical / insufficient
    pub status: String,
    pub score: i64,
    pub completed_days: i64,
    pub current_loss_streak: i64,
    #[serde(default)]
    pub windows: Vec<HealthWindow>,
    #[serde(default)]
    pub curve: Vec<HealthCurvePoint>,
    #[serde(default)]
    pub pause_counts: Vec<PauseCount>,
    #[serde(default)]
    pub alerts: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthWindow {
    pub days: i64,
    pub completed_days: i64,
    pub samples: i64,
    pub win_rate: f64,
    pub avg_t1_real: f64,
    pub cumulative_return: f64,
    pub max_drawdown: f64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HealthCurvePoint {
    pub date: String,
    pub samples: i64,
    pub t1_real_pct: f64,
    pub cumulative_pct: f64,
    pub regime: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PauseCount {
    pub source: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct HardFilterStat {
    pub stage: String,
    pub reason: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ShadowExperimentStat {
    pub experiment_id: String,
    pub status: String,
    pub completed_days: i64,
    pub samples: i64,
    pub t1_real_win_rate: f64,
    pub t1_real_wilson_low: f64,
    pub target_5pct_hit_rate: f64,
    pub target_5pct_wilson_low: f64,
    pub promotion_eligible: bool,
    pub promotion_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RegimeStat {
    /// bull / range / bear / crash
    pub regime: String,
    pub label: String,
    pub samples: i64,
    pub win_rate: f64,
    pub win_rate_low: f64,
    pub win_rate_high: f64,
    pub win_rate_margin: f64,
    pub samples_sufficient: bool,
    pub avg_t1_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PickTagStat {
    pub tag: String,
    pub samples: i64,
    /// 该标签下 T+1 为正的比例（0..1）
    pub win_rate: f64,
    #[serde(default)]
    pub win_rate_low: f64,
    #[serde(default)]
    pub win_rate_high: f64,
    #[serde(default)]
    pub win_rate_margin: f64,
    #[serde(default)]
    pub samples_sufficient: bool,
}

/// 真实执行口径统计（从次日开盘价计算，不是推荐日收盘价）。
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema, Default)]
pub struct ExecutionStats {
    /// 平均执行溢价（次日开盘 vs 推荐日收盘，%）——涨停股高开吃利润
    pub avg_entry_gap: f64,
    /// 真实 T+1 胜率（从次日开盘价算）
    pub t1_real_win_rate: f64,
    #[serde(default)]
    pub t1_real_win_rate_low: f64,
    #[serde(default)]
    pub t1_real_win_rate_high: f64,
    #[serde(default)]
    pub t1_real_win_rate_margin: f64,
    #[serde(default)]
    pub t1_real_samples_sufficient: bool,
    /// 真实 T+1 平均收益
    pub avg_t1_real: f64,
    /// 盈利样本平均收益 / 亏损样本平均收益（后者为负）。
    #[serde(default)]
    pub avg_win: f64,
    #[serde(default)]
    pub avg_loss: f64,
    /// 平均最大回撤（持有期间最低价 vs 买入价）
    pub avg_max_dd: f64,
    /// 样本中最差的持有期最大回撤。
    #[serde(default)]
    pub max_drawdown: f64,
    /// T+1 真实收益最差 10% 样本的均值（CVaR / 期望短缺）。
    #[serde(default)]
    pub expected_shortfall_10: f64,
    /// 实际执行成本下，T+1 盘中达到净盈利 5% 目标的统计。
    #[serde(default)]
    pub target_5pct_samples: i64,
    #[serde(default)]
    pub target_5pct_hit_rate: f64,
    #[serde(default)]
    pub target_5pct_wilson_low: f64,
    #[serde(default)]
    pub target_5pct_wilson_high: f64,
    /// 盈亏比（平均盈利 / |平均亏损|）
    pub win_loss_ratio: f64,
    /// 按推荐日聚合的纸面基准 vs 次日开盘真实执行累计曲线（最近 20 个推荐日）。
    #[serde(default)]
    pub curve: Vec<ExecutionCurvePoint>,
    /// 纸面平均 T+1 - 开盘执行平均 T+1（百分点）；正值表示纸面高估。
    #[serde(default)]
    pub execution_drag: f64,
    /// 执行偏差过大时为 true，客户端应降低“可执行”文案强度。
    #[serde(default)]
    pub execution_warning: bool,
    #[serde(default)]
    pub execution_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExecutionCurvePoint {
    pub date: String,
    pub samples: i64,
    pub paper_t1_pct: f64,
    pub open_t1_pct: f64,
    pub paper_cumulative_pct: f64,
    pub open_cumulative_pct: f64,
}

/// §A.9 上一交易日推荐：仅展示已有 T+1 outcome 回写的票，用于在
/// 「AI 精选」卡下方提供「昨日推荐 → 今日卖点」参考。
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PreviousPick {
    pub date: String,
    pub code: String,
    pub name: String,
    pub rank: usize,
    /// 推荐日的收盘价（meta.close），用于反推卖出价
    #[serde(default)]
    pub close: Option<f64>,
    /// 真实执行买入价（次日开盘价 = meta.outcome.t1_open_basis）
    /// 如果回写里有，反推卖点更准
    #[serde(default)]
    pub t1_open: Option<f64>,
    /// T+1 收盘价（如果已回写）
    #[serde(default)]
    pub t1_close: Option<f64>,
    /// T+1 收益（%）
    #[serde(default)]
    pub t1_pct: Option<f64>,
    /// T+1 真实执行收益（%）
    #[serde(default)]
    pub t1_real_pct: Option<f64>,
    /// 推荐日开盘 vs 收盘的偏移率（%），用于推断缺口
    #[serde(default)]
    pub entry_gap: Option<f64>,
    /// §A.9 卖出价区间下界（基于 t1_close / t1_open × sell_zone_factor ±1.5%）
    #[serde(default)]
    pub sell_price_low: Option<f64>,
    /// §A.9 卖出价区间上界
    #[serde(default)]
    pub sell_price_high: Option<f64>,
    /// §A.9 卖出价基准价
    #[serde(default)]
    pub sell_basis_price: Option<f64>,
    /// §A.9 卖出价基准类型："t1_close"（T+1 收盘价）/ "t1_open"（真实执行买入价）
    #[serde(default)]
    pub sell_basis_kind: Option<String>,
    /// strong / neutral / weak，按真实次日开盘相对推荐日收盘计算。
    #[serde(default)]
    pub open_strength: Option<String>,
    /// hold_strength / take_profit / risk_control / risk_exit / t1_timeout / entry_invalid。
    #[serde(default)]
    pub sell_action: Option<String>,
    #[serde(default)]
    pub sell_action_label: Option<String>,
    /// 与盈利目标分离的风险退出价；不得标记为“盈利5%卖点”。
    #[serde(default)]
    pub risk_stop_price: Option<f64>,
    #[serde(default)]
    pub target_reached: Option<bool>,
    /// 推荐时的 entry_timing（today_close / next_session_open / ...）
    #[serde(default)]
    pub entry_timing: Option<String>,
    /// 推荐时的 entry_label
    #[serde(default)]
    pub entry_label: Option<String>,
    /// 次日开盘相对推荐买入区间的校验：valid / invalid_below / invalid_gap。
    #[serde(default)]
    pub entry_status: Option<String>,
    #[serde(default)]
    pub entry_status_label: Option<String>,
    /// 推荐时附带的 score / reasons 便于复用 UI
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// `POST /api/v1/picks/run` 可选 body：客户端透传 AI 配置做精排。
/// 密钥只在本次请求内使用，不落库（Keychain-only 约束）。
#[derive(Debug, Clone, Default, Deserialize, utoipa::ToSchema)]
pub struct PicksRunRequest {
    #[serde(default)]
    pub ai: Option<AiRankConfig>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct AiRankConfig {
    pub provider: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub model: String,
}
