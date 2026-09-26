-- §A.10 每次推荐生成的市场状态与暂停原因。
-- `daily_pick` 无法表达「当天正常执行过，但策略决定零推荐」，因此单独记运行快照，
-- 让后续 GET /picks 不会回退并误展示上一交易日的旧清单。
CREATE TABLE IF NOT EXISTS daily_pick_run (
    date         TEXT PRIMARY KEY,
    market       TEXT NOT NULL DEFAULT '{}',
    execute_hint TEXT NOT NULL DEFAULT '',
    paused       INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);

