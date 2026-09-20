//! §F.3–F.4 数据接入 DTO：新闻 / 研报 / 概念板块。
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 个股相关新闻（东财全文搜索按时间倒序）
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NewsItem {
    /// 关联股票代码（统一 sh/sz/bj 前缀）
    pub code: String,
    /// 标题
    pub title: String,
    /// 摘要
    pub summary: String,
    /// 媒体来源
    pub media: String,
    /// 原文链接
    pub url: String,
    /// 发布时间
    pub published_at: DateTime<Utc>,
}

/// 机构研报（东财研报库）
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResearchReport {
    pub code: String,
    /// 研报标题
    pub title: String,
    /// 机构简称
    pub org: String,
    /// 发布日期 YYYY-MM-DD
    pub publish_date: String,
    /// 东财评级（买入 / 增持 / 中性 …）
    pub rating: String,
    /// 上次评级
    pub last_rating: String,
    /// 评级变动（东财编码：1 上调 / 2 下调 / 3 维持）
    pub rating_change: Option<i64>,
    /// 分析师
    pub researcher: String,
    /// 所属行业
    pub industry: String,
    /// 目标价上限
    pub aim_price_high: Option<f64>,
    /// 目标价下限
    pub aim_price_low: Option<f64>,
    /// 研报详情页
    pub url: String,
}

/// 概念 / 行业板块（个股所属，含最新指数与涨跌幅）
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SectorBoard {
    /// 关联股票代码（统一 sh/sz/bj 前缀）
    pub code: String,
    /// 板块代码，如 BK0977
    pub board_code: String,
    /// 板块名称
    pub name: String,
    /// 是否主营相关（东财 IS_PRECISE）
    pub is_precise: bool,
    /// 入选理由（东财给出）
    pub reason: String,
    /// 板块最新指数
    pub price: Option<f64>,
    /// 板块涨跌幅（%）
    pub change_pct: Option<f64>,
}
