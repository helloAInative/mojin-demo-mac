-- §A.13 统一暂停来源与审计。daily_pick_run 保存当天最终状态，
-- pick_pause_audit 保存每次首次触发的结构化原因，供复盘追溯。
ALTER TABLE daily_pick_run ADD COLUMN pause_source TEXT NOT NULL DEFAULT '';

CREATE TABLE IF NOT EXISTS pick_pause_audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    date       TEXT NOT NULL,
    source     TEXT NOT NULL,
    reason     TEXT NOT NULL,
    active     INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    UNIQUE(date, source, reason)
);

CREATE INDEX IF NOT EXISTS idx_pick_pause_audit_date
    ON pick_pause_audit(date DESC, source);
