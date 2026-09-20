-- 0001_init.sql
-- 行情网关阶段 1 用到的三张表
-- 设计参考 docs/架构-Rust后端.md §6

CREATE TABLE IF NOT EXISTS quote (
    code        TEXT NOT NULL,
    ts          INTEGER NOT NULL,        -- unix epoch ms
    name        TEXT,
    price       REAL,
    prev        REAL,
    open        REAL,
    high        REAL,
    low         REAL,
    volume      INTEGER,
    amount      REAL,
    source      TEXT,                    -- sina / tencent / eastmoney
    PRIMARY KEY (code, ts)
);
CREATE INDEX IF NOT EXISTS idx_quote_code_ts ON quote(code, ts DESC);

CREATE TABLE IF NOT EXISTS minute_bar (
    code        TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    price       REAL,
    avg_price   REAL,
    volume      INTEGER,
    amount      REAL,
    PRIMARY KEY (code, ts)
);

CREATE TABLE IF NOT EXISTS day_bar (
    code        TEXT NOT NULL,
    date        TEXT NOT NULL,           -- YYYY-MM-DD
    open        REAL,
    high        REAL,
    low         REAL,
    close       REAL,
    volume      INTEGER,
    amount      REAL,
    PRIMARY KEY (code, date)
);

-- 阶段 2 起需要：信号事件表（架构文档 §6）
CREATE TABLE IF NOT EXISTS signal_event (
    id          TEXT PRIMARY KEY,        -- UUID
    at          INTEGER NOT NULL,
    kind        TEXT NOT NULL,           -- alert / ai / level / posAlert
    code        TEXT NOT NULL,
    title       TEXT,
    body        TEXT,
    price       REAL,
    source      TEXT,
    evidence    TEXT,
    why         TEXT,
    meta        TEXT                     -- JSON
);
CREATE INDEX IF NOT EXISTS idx_signal_at  ON signal_event(at DESC);
CREATE INDEX IF NOT EXISTS idx_signal_code ON signal_event(code, at DESC);