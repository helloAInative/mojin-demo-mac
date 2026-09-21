-- 0007_picks.sql
-- 智能推荐：四层漏斗（涨幅榜候选 → 技术指标粗筛 → 研报/新闻加分 → AI 精排）
-- 每日一份，按 (date, code) 主键；重复生成走 DELETE+INSERT 刷新。
-- meta: {close, pct, industry, boost} + 回测回写 {t1_pct, t5_pct}

CREATE TABLE IF NOT EXISTS daily_pick (
    date       TEXT NOT NULL,           -- 推荐基准日 YYYY-MM-DD（北京，最近已收盘交易日）
    code       TEXT NOT NULL,
    name       TEXT NOT NULL DEFAULT '',
    rank       INTEGER NOT NULL,
    score      REAL NOT NULL DEFAULT 0,
    reasons    TEXT NOT NULL DEFAULT '[]',  -- JSON 字符串数组（标签）
    ai_note    TEXT NOT NULL DEFAULT '',
    meta       TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (date, code)
);
CREATE INDEX IF NOT EXISTS idx_daily_pick_date ON daily_pick(date DESC);
