-- 0005_scheduled_report.sql
-- 阶段 4：收盘复盘 / 周报，按 period_key 幂等生成。

CREATE TABLE IF NOT EXISTS scheduled_report (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,          -- daily / weekly
    period_key  TEXT NOT NULL,          -- YYYY-MM-DD / YYYY-Www
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',
    created_at  INTEGER NOT NULL,
    UNIQUE(kind, period_key)
);
CREATE INDEX IF NOT EXISTS idx_scheduled_report_created
    ON scheduled_report(created_at DESC);
