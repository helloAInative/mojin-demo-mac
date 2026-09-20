-- 0003_signal_position.sql
-- 阶段 3：信号 / 持仓 / 自选 落库（架构文档 §6）

CREATE TABLE IF NOT EXISTS position (
    code            TEXT PRIMARY KEY,
    cost            REAL    NOT NULL DEFAULT 0,
    shares          REAL    NOT NULL DEFAULT 0,
    stop_loss       REAL    NOT NULL DEFAULT 0,
    take_profit     REAL    NOT NULL DEFAULT 0,
    position_pct    REAL    NOT NULL DEFAULT 0,
    note            TEXT,
    updated_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS watchlist (
    code        TEXT PRIMARY KEY,
    name        TEXT,
    market      TEXT,
    pinned      INTEGER NOT NULL DEFAULT 0,
    grp         TEXT,
    added_at    INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_watchlist_pinned_added ON watchlist(pinned DESC, added_at DESC);

CREATE TABLE IF NOT EXISTS levels (
    code        TEXT PRIMARY KEY,
    base        REAL NOT NULL DEFAULT 0,
    support     REAL NOT NULL DEFAULT 0,
    resistance  REAL NOT NULL DEFAULT 0,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS strategy_note (
    code              TEXT PRIMARY KEY,
    above             REAL NOT NULL DEFAULT 0,
    below             REAL NOT NULL DEFAULT 0,
    drawdown_pct      REAL NOT NULL DEFAULT 0,
    cool_down_min     INTEGER NOT NULL DEFAULT 30,
    link_stop_to_below INTEGER NOT NULL DEFAULT 1,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS diary (
    code        TEXT NOT NULL,
    day         TEXT NOT NULL,        -- YYYY-MM-DD
    text        TEXT,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (code, day)
);
CREATE INDEX IF NOT EXISTS idx_diary_day ON diary(day DESC);

-- 用户对单次 level 信号的命中（前端 checkLevelHits 已写过 meta.hit=1，
-- 这里额外存行式方便 JOIN / 统计；触发逻辑由前端保持）。
CREATE TABLE IF NOT EXISTS level_hit (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    signal_id       TEXT NOT NULL,
    code            TEXT NOT NULL,
    level_key       TEXT NOT NULL,    -- base/support/resistance
    level_price     REAL NOT NULL,
    hit_price       REAL NOT NULL,
    lead_sec        INTEGER NOT NULL,
    fired_at        INTEGER NOT NULL,
    hit_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_level_hit_code_at ON level_hit(code, fired_at DESC);