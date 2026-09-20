-- 0006_ingest.sql
-- §F.3–F.4：新闻 / 研报 / 概念板块数据接入。
-- 三个表都是"按 code 的缓存"：接口按需拉取后 UPSERT，调度器每日增量刷新。

CREATE TABLE IF NOT EXISTS news_item (
    code         TEXT NOT NULL,          -- sh600460
    url          TEXT NOT NULL,          -- 原文链接（去重键）
    title        TEXT NOT NULL,
    summary      TEXT NOT NULL DEFAULT '',
    media        TEXT NOT NULL DEFAULT '',
    published_at INTEGER NOT NULL,       -- 发布时间（ms, UTC）
    fetched_at   INTEGER NOT NULL,
    PRIMARY KEY (code, url)
);
CREATE INDEX IF NOT EXISTS idx_news_item_code_published
    ON news_item(code, published_at DESC);

CREATE TABLE IF NOT EXISTS research_report (
    code          TEXT NOT NULL,
    info_code     TEXT NOT NULL,         -- 东财研报编号（详情页 URL 的一部分）
    title         TEXT NOT NULL,
    org           TEXT NOT NULL DEFAULT '',
    publish_date  TEXT NOT NULL DEFAULT '',   -- YYYY-MM-DD
    rating        TEXT NOT NULL DEFAULT '',
    last_rating   TEXT NOT NULL DEFAULT '',
    rating_change INTEGER,               -- 东财编码：1 上调 / 2 下调 / 3 维持
    researcher    TEXT NOT NULL DEFAULT '',
    industry      TEXT NOT NULL DEFAULT '',
    aim_price_high REAL,
    aim_price_low  REAL,
    url           TEXT NOT NULL DEFAULT '',
    fetched_at    INTEGER NOT NULL,
    PRIMARY KEY (code, info_code)
);
CREATE INDEX IF NOT EXISTS idx_research_report_code_date
    ON research_report(code, publish_date DESC);

CREATE TABLE IF NOT EXISTS sector_board (
    code       TEXT NOT NULL,
    board_code TEXT NOT NULL,            -- BK0977
    board_name TEXT NOT NULL,
    is_precise INTEGER NOT NULL DEFAULT 1,  -- 1 = 主营相关（东财 IS_PRECISE）
    reason     TEXT NOT NULL DEFAULT '',
    price      REAL,                     -- 板块最新指数
    change_pct REAL,                     -- 涨跌幅（%）
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (code, board_code)
);
CREATE INDEX IF NOT EXISTS idx_sector_board_code
    ON sector_board(code);
