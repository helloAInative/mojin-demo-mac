-- 0008_main_net.sql
-- 主力资金净流入（A 股池智能推荐 §A.6 因子）
-- 数据源：东财 push2 /api/qt/stock/get secid=1.{code} 字段 f62（主力净额 / 元）
--         + f170（主力净占比 %）+ f168（换手率 %）
-- 落库按 (code, trade_date) UPSERT；TTL 不显式建（smart picks 每次现拉 + 兜底缓存）。

CREATE TABLE IF NOT EXISTS main_net_snapshot (
    code         TEXT NOT NULL,         -- sh600460
    trade_date   TEXT NOT NULL,         -- YYYY-MM-DD 北京
    main_net     REAL NOT NULL,         -- 主力净流入 / 元（f62）
    super_net    REAL NOT NULL DEFAULT 0,  -- 超大单净流入 / 元（f63）
    big_net      REAL NOT NULL DEFAULT 0,  -- 大单净流入 / 元（f64）
    pct_ratio    REAL NOT NULL DEFAULT 0,  -- 主力净占比 %（f170）
    turnover     REAL NOT NULL DEFAULT 0,  -- 换手率 %（f168）
    prev_close   REAL NOT NULL DEFAULT 0,  -- 昨收 / 元（f60，警戒参考）
    fetched_at   INTEGER NOT NULL,
    PRIMARY KEY (code, trade_date)
);
CREATE INDEX IF NOT EXISTS idx_main_net_date ON main_net_snapshot(trade_date DESC);