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
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PickStats {
    pub samples: i64,
    /// 主目标：T+1 收盘价高于推荐日尾盘基准价的比例（0..1）。
    #[serde(default)]
    pub t1_win_rate: f64,
    #[serde(default)]
    pub avg_t1_pct: f64,
    /// 兼容中线观察的 T+5 口径。
    #[serde(default)]
    pub t5_samples: i64,
    pub t5_win_rate: f64,
    pub avg_t5_pct: f64,
    /// 标签级 T+1 回测：哪个因子更适合隔日目标。
    #[serde(default)]
    pub tags: Vec<PickTagStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PickTagStat {
    pub tag: String,
    pub samples: i64,
    /// 该标签下 T+1 为正的比例（0..1）
    pub win_rate: f64,
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
