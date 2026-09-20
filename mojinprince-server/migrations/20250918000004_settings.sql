-- 0004_settings.sql
-- 通用 KV 设置表（架构文档 §6）

CREATE TABLE IF NOT EXISTS settings (
    key         TEXT PRIMARY KEY,
    value       TEXT,            -- JSON
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ai_settings (
    key         TEXT PRIMARY KEY,
    value       TEXT,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS notify_config (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    level_light_pct REAL    NOT NULL DEFAULT 1.0,
    level_deep_pct  REAL    NOT NULL DEFAULT 0.3,
    cost_light_pct  REAL    NOT NULL DEFAULT 1.5,
    daily_quota     INTEGER NOT NULL DEFAULT 30,
    cool_down_min   INTEGER NOT NULL DEFAULT 30,
    updated_at      INTEGER NOT NULL
);

-- 单条 1=种子；写一次后不再创建
INSERT OR IGNORE INTO notify_config(id, updated_at) VALUES(1, CAST(strftime('%s','now') AS INTEGER)*1000);