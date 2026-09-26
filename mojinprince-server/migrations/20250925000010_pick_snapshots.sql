-- §A.13 盘中二次确认：保留 10:00 观察版与 14:45 执行版，避免尾盘覆盖后失去对照。
CREATE TABLE IF NOT EXISTS daily_pick_snapshot (
    date       TEXT NOT NULL,
    session    TEXT NOT NULL, -- morning / tail
    market     TEXT NOT NULL DEFAULT '{}',
    picks      TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (date, session)
);

