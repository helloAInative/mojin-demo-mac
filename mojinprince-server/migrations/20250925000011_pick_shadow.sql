-- §A.14 影子盘：挑战者规则只记账、不进入正式推荐。
-- outcome 继续写入 meta，便于与 daily_pick 使用同一套真实执行统计口径。
CREATE TABLE IF NOT EXISTS daily_pick_shadow (
    date          TEXT NOT NULL,
    experiment_id TEXT NOT NULL,
    code          TEXT NOT NULL,
    name          TEXT NOT NULL DEFAULT '',
    rank          INTEGER NOT NULL,
    score         REAL NOT NULL DEFAULT 0,
    reasons       TEXT NOT NULL DEFAULT '[]',
    meta          TEXT NOT NULL DEFAULT '{}',
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (date, experiment_id, code)
);

CREATE INDEX IF NOT EXISTS idx_daily_pick_shadow_experiment_date
    ON daily_pick_shadow(experiment_id, date DESC);
