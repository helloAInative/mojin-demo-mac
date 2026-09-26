-- §A.12 事件日历硬过滤审计：记录为什么候选没有进入生产或影子清单。
CREATE TABLE IF NOT EXISTS daily_pick_exclusion (
    date       TEXT NOT NULL,
    code       TEXT NOT NULL,
    stage      TEXT NOT NULL,
    reason     TEXT NOT NULL,
    evidence   TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (date, code, stage, reason)
);

CREATE INDEX IF NOT EXISTS idx_daily_pick_exclusion_date
    ON daily_pick_exclusion(date DESC, stage);
