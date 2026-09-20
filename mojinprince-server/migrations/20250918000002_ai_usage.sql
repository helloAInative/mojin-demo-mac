-- 0002_ai_usage.sql
-- 阶段 2：AI 网关用量账本
-- 设计参考 docs/架构-Rust后端.md §6

CREATE TABLE IF NOT EXISTS ai_usage (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,        -- unix epoch ms
    provider    TEXT NOT NULL,           -- openai / ollama
    model       TEXT NOT NULL,
    tokens_in   INTEGER NOT NULL DEFAULT 0,
    tokens_out  INTEGER NOT NULL DEFAULT 0,
    cost        REAL    NOT NULL DEFAULT 0,    -- 美元，按 provider + model 估算；0 表示未知
    duration_ms INTEGER NOT NULL DEFAULT 0,
    success     INTEGER NOT NULL,        -- 0/1
    fallback    INTEGER NOT NULL DEFAULT 0,    -- 0/1（规则摘要兜底）
    error       TEXT,
    note        TEXT,                    -- 简短上下文（标的 / prompt hash 等）
    prompt_chars INTEGER NOT NULL DEFAULT 0,
    output_chars INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_ai_usage_at       ON ai_usage(at DESC);
CREATE INDEX IF NOT EXISTS idx_ai_usage_model_at ON ai_usage(model, at DESC);

-- 用户对单次 AI 结论的反馈（采纳 / 忽略）。回填进 accuracy 统计。
CREATE TABLE IF NOT EXISTS ai_feedback (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,
    usage_id    INTEGER,                 -- 关联 ai_usage.id
    signal_id   TEXT,                    -- 关联 signal_event.id
    code        TEXT NOT NULL,
    sentiment   TEXT NOT NULL,           -- accept / ignore / partial
    note        TEXT,
    FOREIGN KEY (usage_id)  REFERENCES ai_usage(id)  ON DELETE SET NULL,
    FOREIGN KEY (signal_id) REFERENCES signal_event(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_ai_feedback_code_at ON ai_feedback(code, at DESC);
