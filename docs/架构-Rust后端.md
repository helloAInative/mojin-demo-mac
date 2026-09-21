# 摸金小王子 · Rust 后端架构设计

> 本文档定义前后端分离方案。**决策**：独立服务（NAS/旧 Mac）+ SQLite + 渐进式 4 阶段。
> 与 [`未来演进方向.md`](未来演进方向.md) §F 数据接入、§G 跨设备 紧密相关。

---

## 1. 决策摘要

| 维度 | 选择 |
| --- | --- |
| 部署模式 | **独立服务** —— 后端跑在 NAS / 旧 Mac，监听 `0.0.0.0:8732`；前端设置里填服务器地址 |
| 数据库 | **SQLite**（rusqlite + sqlx）—— 单用户够用，零运维；想换 PostgreSQL 改连接串即可 |
| 迁移节奏 | **渐进式 4 阶段** —— 行情 → AI → 数据 → 推送，逐步上线 |
| Web 框架 | Actix-web 4 |
| 异步运行时 | tokio |
| HTTP 客户端 | reqwest |
| WebSocket | actix-web-actors |
| ORM | sqlx（编译期类型检查） |
| 序列化 | serde |
| 日志 | tracing + tracing-subscriber |
| API 文档 | utoipa + Swagger UI（自带调试界面） |
| 鉴权 | JWT（规划中；当前代码尚未实现，只用于本机或受信任局域网） |
| 打包 | `cargo build --release` → 单二进制，NAS 用 systemd / launchd 托管 |

---

## 2. 架构总览

```
┌──────────────────────────────────────────────────────────────┐
│  macOS Menu Bar App (SwiftUI)                                │
│  - UI / Chart 渲染 / 用户交互                                │
│  - 本地缓存（启动拉一次，断网用本地兜底）                       │
│  - 系统通知（NSUserNotification，仍走前端）                    │
│  - 菜单栏图标状态                                            │
└────────────────────────┬─────────────────────────────────────┘
                         │ HTTPS / WebSocket（局域网）
┌────────────────────────┴─────────────────────────────────────┐
│  Rust 后端（mojinprince-server，部署在 NAS / 旧 Mac）         │
│  ├─ HTTP API（JSON）                                          │
│  ├─ WebSocket（行情推送 / 通知）                              │
│  ├─ LLM Gateway（云端 / Ollama 路由）                          │
│  ├─ Scheduler（定时分析 / 收盘复盘）                           │
│  └─ SQLite（rusqlite + sqlx）                                  │
└─┬───────────┬───────────────┬───────────────┬─────────────────┘
  │           │               │               │
  ▼           ▼               ▼               ▼
行情源      AI Provider 新闻/公告       日志/监控
新浪/腾讯   阿里云 / 东财 /           tracing
东财 WebSocket Ollama       同花顺        JSON 文件
同花顺                    朝阳研报
```

---

## 3. 模块拆分

```
mojinprince-server/
├── Cargo.toml
├── migrations/                  # sqlx 迁移
│   ├── 0001_init.sql
│   ├── 0002_signals.sql
│   ├── 0003_positions.sql
│   └── …                        # 见 §6 表结构（含 0006_ingest.sql）
├── src/
│   ├── main.rs                  # 启动 + 路由注册
│   ├── config.rs                # 配置加载（config + dotenvy）
│   ├── error.rs                 # AppError → HTTP 状态码
│   ├── state.rs                 # AppState (db, http, ai, broadcaster)
│   ├── api/
│   │   ├── mod.rs
│   │   ├── quote.rs
│   │   ├── ai.rs
│   │   ├── signal.rs
│   │   ├── position.rs
│   │   ├── settings.rs
│   │   └── ws.rs
│   ├── service/
│   │   ├── quote/
│   │   │   ├── mod.rs
│   │   │   ├── sina.rs
│   │   │   ├── tencent.rs
│   │   │   ├── eastmoney.rs
│   │   │   └── failover.rs      # 主源 → 备份自动切换
│   │   ├── ai/
│   │   │   ├── mod.rs           # Provider trait
│   │   │   ├── openai.rs
│   │   │   ├── ollama.rs
│   │   │   └── governor.rs      # 冷却 / 配额 / 成本
│   │   ├── ingest.rs            # §F.3–F.4 东财新闻 / 研报 / 板块（拉取 + 落库）
│   │   ├── scheduler.rs         # 行情轮询 / 复盘自动生成 / 每日数据接入刷新
│   │   └── notify.rs            # 推送给前端（WebSocket）
│   ├── repo/                    # 数据库访问
│   │   ├── mod.rs
│   │   ├── quote.rs
│   │   ├── signal.rs
│   │   ├── position.rs
│   │   └── settings.rs
│   └── model/                   # DTO / Domain
│       ├── mod.rs
│       ├── quote.rs
│       ├── signal.rs
│       ├── ingest.rs            # §F.3–F.4 NewsItem / ResearchReport / SectorBoard
│       └── ai.rs
└── tests/
```

---

## 4. API 设计（核心路由）

```rust
// 健康
GET  /api/v1/health

// 行情
GET  /api/v1/quote/{code}                 // 单标的实时报价
GET  /api/v1/minutes/{code}               // 分时
GET  /api/v1/days/{code}                  // 日 K
GET  /api/v1/ws/quote                     // WebSocket 推送

// AI
POST /api/v1/ai/analyze                   // 触发分析
POST /api/v1/ai/reflect                   // 反思 prompt（多步 agent）
GET  /api/v1/ai/usage                     // 用量账本
GET  /api/v1/ai/accuracy                  // 命中率

// 信号 / 预警
GET  /api/v1/signals                      // SignalEvent 列表
POST /api/v1/signals/{id}/feedback        // 用户反馈（采纳/忽略）

// 持仓 / 自选
GET  /api/v1/positions
PUT  /api/v1/positions/{code}
GET  /api/v1/watchlist
POST /api/v1/watchlist
DELETE /api/v1/watchlist/{code}

// 设置
GET  /api/v1/settings
PUT  /api/v1/settings

// 复盘 / 周报（已实现：按需触发 + 收盘 / 周末自动补缺 + 幂等落库）
GET  /api/v1/reviews                      // 列出已生成报告（?kind=&limit=）
POST /api/v1/reviews/run                  // 生成日报 / 周报（?kind=daily|weekly）

// 数据接入（§F.3–F.4 已实现：东财按需拉取 + UPSERT 落库，每日收盘后增量刷新自选股）
GET  /api/v1/news/{code}                  // 新闻流（?limit=&hours=）
GET  /api/v1/reports/{code}               // 研报（?limit=&days=）
GET  /api/v1/sector/{code}                // 板块行情

// 鉴权（公网模式才打开）
POST /api/v1/auth/login
POST /api/v1/auth/refresh
```

完整 Swagger UI 自动挂在 `/swagger-ui/`。

---

## 5. 前后端边界

### Rust 后端负责

- 网络 IO（行情 / WebSocket / AI API）
- 跨源聚合（failover / 拼接分时 / K 线复权）
- 调度（自动分析 / 收盘复盘 / 周报导出）
- 持久化（SQLite 替代现在的 JSON 文件）
- 鉴权 / 限流 / 成本统计 / 命中率计算
- 推送（行情变化 / 信号命中 → WebSocket）
- 数据接入（§F 新闻 / 研报 / 板块）

### Swift 前端负责

- UI 渲染（Chart / 菜单栏）
- 本地配置缓存（启动时拉一次，断网用本地兜底）
- 系统通知（NSUserNotification，仍走前端，便于跟随系统免打扰）
- 菜单栏图标状态
- 不再做 HTTP 拉行情 / AI —— `MarketService.swift` / `AIService.swift` 大半可删

预期前端代码减少 ~1000 行（`MarketService.swift` ~600 行 + `AIService.swift` ~250 行）。

---

## 6. 数据库 schema（初版）

```sql
-- 0001_init.sql
CREATE TABLE quote (
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
CREATE INDEX idx_quote_code_ts ON quote(code, ts DESC);

CREATE TABLE minute_bar (
    code        TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    price       REAL,
    avg_price   REAL,
    volume      INTEGER,
    amount      REAL,
    PRIMARY KEY (code, ts)
);

CREATE TABLE day_bar (
    code        TEXT NOT NULL,
    date        TEXT NOT NULL,           -- YYYY-MM-DD
    open        REAL, high REAL, low REAL, close REAL,
    volume      INTEGER, amount REAL,
    PRIMARY KEY (code, date)
);

-- 0002_signals.sql
CREATE TABLE signal_event (
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
CREATE INDEX idx_signal_at ON signal_event(at DESC);
CREATE INDEX idx_signal_code ON signal_event(code, at DESC);

-- 0003_positions.sql
CREATE TABLE position (
    code        TEXT PRIMARY KEY,
    cost        REAL,
    shares      REAL,
    stop_loss   REAL,
    take_profit REAL,
    position_pct REAL,
    updated_at  INTEGER
);

CREATE TABLE watchlist (
    code        TEXT PRIMARY KEY,
    name        TEXT,
    market      TEXT,
    added_at    INTEGER
);

CREATE TABLE settings (
    key         TEXT PRIMARY KEY,
    value       TEXT,                    -- JSON
    updated_at  INTEGER
);

CREATE TABLE ai_usage (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,
    provider    TEXT,                    -- openai / ollama
    model       TEXT,
    tokens_in   INTEGER,
    tokens_out  INTEGER,
    cost        REAL,
    duration_ms INTEGER,
    success     INTEGER,                 -- 0/1
    error       TEXT
);
CREATE INDEX idx_ai_usage_at ON ai_usage(at DESC);

-- 0005_scheduled_report.sql
CREATE TABLE scheduled_report (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,          -- daily / weekly
    period_key  TEXT NOT NULL,          -- YYYY-MM-DD / YYYY-Www
    title       TEXT NOT NULL,
    body        TEXT NOT NULL,
    payload     TEXT NOT NULL DEFAULT '{}',
    created_at  INTEGER NOT NULL,
    UNIQUE(kind, period_key)
);
CREATE INDEX idx_scheduled_report_created ON scheduled_report(created_at DESC);

-- 0006_ingest.sql
CREATE TABLE IF NOT EXISTS news_item (
    code         TEXT NOT NULL,         -- sh600460
    url          TEXT NOT NULL,         -- 原文链接（去重键）
    title        TEXT NOT NULL,
    summary      TEXT NOT NULL DEFAULT '',
    media        TEXT NOT NULL DEFAULT '',
    published_at INTEGER NOT NULL,      -- 毫秒时间戳（UTC）
    fetched_at   INTEGER NOT NULL,
    PRIMARY KEY(code, url)
);
CREATE INDEX idx_news_item_code_published ON news_item(code, published_at DESC);

CREATE TABLE IF NOT EXISTS research_report (
    code            TEXT NOT NULL,
    info_code       TEXT NOT NULL,      -- 东财研报编号（详情页 URL 的一部分）
    title           TEXT NOT NULL,
    org             TEXT NOT NULL DEFAULT '',
    publish_date    TEXT NOT NULL DEFAULT '',  -- YYYY-MM-DD
    rating          TEXT NOT NULL DEFAULT '',
    last_rating     TEXT NOT NULL DEFAULT '',
    rating_change   INTEGER,            -- 1 上调 / 2 下调 / 3 维持
    researcher      TEXT NOT NULL DEFAULT '',
    industry        TEXT NOT NULL DEFAULT '',
    aim_price_high  REAL,
    aim_price_low   REAL,
    url             TEXT NOT NULL DEFAULT '',
    fetched_at      INTEGER NOT NULL,
    PRIMARY KEY(code, info_code)
);
CREATE INDEX idx_research_report_code_date ON research_report(code, publish_date DESC);

CREATE TABLE IF NOT EXISTS sector_board (
    code        TEXT NOT NULL,
    board_code  TEXT NOT NULL,          -- BK0977
    board_name  TEXT NOT NULL,
    is_precise  INTEGER NOT NULL DEFAULT 1,  -- 1 = 主营相关（东财 IS_PRECISE）
    reason      TEXT NOT NULL DEFAULT '',
    price       REAL,                   -- 板块指数（停牌 / 无数据为 NULL）
    change_pct  REAL,
    fetched_at  INTEGER NOT NULL,
    PRIMARY KEY(code, board_code)
);
CREATE INDEX idx_sector_board_code ON sector_board(code);
```

### 6.5 复盘表 + 幂等生成

`scheduled_report` 的 `UNIQUE(kind, period_key)` 是幂等保证的基石：

- `period_key`：daily 用 `YYYY-MM-DD`（Asia/Shanghai），weekly 用 ISO 周 `YYYY-Www`
- 重复执行同一 `(kind, period_key)` 走 `INSERT ... ON CONFLICT(kind, period_key) DO UPDATE SET title/body/payload`，**只刷新正文，`created_at` 保留首次写入时间**。Swift 端可以用 `id`（基于 kind+period_key 哈希，稳定不变）做缓存键
- 报告正文包含两部分：
  - **后端摘要**（服务端拼）：signal_event 时间线、AI 用量、level 命中
  - **客户端上下文**（Swift 主动 POST）：日记 / 委托 / 信号 — 后端原样落到 `payload.context`
  - **止损止盈执行对照**（ROI #2，2026-09-21）：委托 context 带 `side` / `price` 后，服务端把「已成交卖出」与 `position` 表的止损 / 止盈价对比，正文输出偏差金额与百分比（`+0.10（+0.3%）`），未设价位的持仓给出纪律提示；买入与草稿不参与
  - **失效归因**（ROI #9，2026-09-21，仅周报）：本周 `kind=level` 信号按 `day_bar` 的 high/low 判定失败原因——穿越未确认（发射日后曾触及价位但 6h 内未回写 hit，多为假突破扫损）、未到价（最近偏离度 + 已过天数，取 TOP3）、窗口未满（发射 < 6h）、无日线；`ai_feedback` 中 `ignore` 计数一并汇总；`payload.summary` 带 `level_crossed_unconfirmed` / `level_never_reached` / `level_window_open` / `level_no_bars` / `ai_feedback_ignored`
- 自动调度：`spawn_report_scheduler` 60s 心跳；工作日 15:05 后补日报，周五 / 周末补周报，**只补缺失的基线版**（已存在的跳过）；Swift 端"生成今日 / 生成本周"按钮相当于在已存在的记录上覆盖正文（含客户端 context），不会改 `created_at`

---

## 7. 渐进式迁移路线

### 阶段 1：行情网关（3–4 天）

- **代码状态（2026-09-18）**：Rust 实时报价、分时、日 K 已实现；Swift 优先调用网关，服务故障时临时直连回退。目标设备部署与局域网联调待完成。
- 把 `MarketService.swift` 的新浪 / 腾讯 / 东财 HTTP 抽到 Rust。
- Swift 端只调 `GET /api/v1/quote/{code}` / `/minutes` / `/days`。
- 加 failover（主源连续 3 次失败 → 切备份）。
- 收益：failover 在 Rust 端做更稳；未来接 WebSocket 顺带做。
- **风险最低**，建议先做。

### 阶段 2：AI 网关（2 天）

- 把 `AIService.chat` + `ProviderRegistry` 迁到 Rust。
- 加 `Provider` trait + `OpenAIProvider` / `OllamaProvider`。
- 加 `/api/v1/ai/usage` 把 `AIUsageLedger` 落 SQLite。
- 加 `/api/v1/ai/accuracy` 服务端算命中率。
- 收益：本地 LLM（§A.3）和降级（§A.3 配套）天然在 Rust 做。
- **代码状态（2026-09-20）**：路由 / DTO / Governor / 用量账本 / 集成测试 / smoke test 全部已绿；Swift 端 `GatewayFirstProvider` 网关优先 + Ollama 直连兜底实现完毕。剩余：Swift 端实际联调（菜单「测试连接」跑通网关）+ 目标设备局域网联调报告。

### 阶段 3：信号 / 持仓 / 设置（2 天）

- 把 `SignalTimeline` / `AppSettings` / `PositionNote` 的 JSON 落库迁到 SQLite + HTTP API。
- 前端加 5 分钟本地缓存（启动拉一次，断网用本地兜底）。
- `SignalEvent.meta` 仍按 JSON 存，迁移成本最低。
- **代码状态（2026-09-20）**：`signals / positions / watchlist / settings` HTTP API、OpenAPI 和 3 项集成测试已完成；Swift 采用“本地先写、服务端后写”，启动时服务端优先、空库自动用本地数据播种。断网继续使用 UserDefaults / 本地 JSON；API Token 始终仅存 Keychain，不参与同步。

### 阶段 4：推送 + 调度 + 数据接入（2–3 天）

- WebSocket 推送行情变化 / 信号命中。
- 后端 scheduler 跑收盘复盘 / 周报导出。
- 接新闻 / 研报 / 板块数据（§F.3–F.4）。
- 至此 §F 数据接入全部走 Rust，前端只剩 UI。
- **代码状态（2026-09-20；09-21 补评级信号化 + Swift 展示）**：`/api/v1/ws/quote`、广播 Hub、自选股交易时段轮询调度和 Swift 断线重连 / HTTP 回退已完成；收盘复盘 / 周报生成按需 API（`POST /api/v1/reviews/run` + `GET /api/v1/reviews`，按 `(kind, period_key)` UPSERT 幂等，落 `scheduled_report` 表，7 项集成测试）与**自动触发**（`spawn_report_scheduler` 60s 心跳：工作日 15:05 后补日报，周五 15:05 后与周末补周报，仅补缺失的基线版，2 项集成测试）均已落地，Swift 端 `ReportClient` 已接入复盘历史与手动生成；**新闻 / 研报 / 板块数据（§F.3–F.4）已完成**：三个只读端点按需拉东财并 UPSERT 落 `news_item` / `research_report` / `sector_board` 三张缓存表，`POST /api/v1/ai/analyze` 新增 `include_news` 可把近 24h 新闻拼进 prompt，`spawn_ingest_scheduler` 工作日 16:00 后为自选股（≤60 只）增量刷新一次（9 项集成测试 + 3 项 live 测试 `#[ignore]`）；**评级信号化**：刷新研报时把近 7 天且评级明确的写成 `kind=report` 的 `signal_event`（买入 / 增持 → 机构看多，卖出 / 减持 → 机构看空；id 由 `(code, info_code)` 派生 + `INSERT OR IGNORE` 幂等），经既有 signals 同步通道自动到 Swift；**Swift 盯盘页**已加「资讯 · 研报 · 板块」面板（板块涨跌色块、研报评级行、近 72h 新闻流，点击打开东财原文，切换自选自动加载）；**收盘复盘通知**（ROI #6）：工作日 15:05 后独立轮询（60s，生成窗口后降为 10 分钟兜底）当日日报，生成即发 macOS 通知一次（按 `lastReviewNotifyDay` 当日去重），点击跳复盘页。

### 阶段 5（可选）：跨设备

- iOS / Watch / Telegram Bot 直接对后端 HTTP。
- 配置多客户端 → JWT 鉴权打开。

---

## 8. 部署

### 启动脚本（NAS / 旧 Mac）

```bash
# /opt/mojinprince/start.sh
#!/bin/bash
cd /opt/mojinprince
export DATABASE_URL=sqlite:///var/lib/mojinprince/data.db
export BIND_ADDR=0.0.0.0:8732
export JWT_SECRET=$(cat /etc/mojinprince/jwt.secret)
exec ./mojinprince-server
```

### systemd unit

```ini
# /etc/systemd/system/mojinprince.service
[Unit]
Description=MojinPrince Server
After=network-online.target

[Service]
Type=simple
User=mojin
ExecStart=/opt/mojinprince/start.sh
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### launchd plist（macOS）

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
 "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.mojinprince.server</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/mojinprince/mojinprince-server</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/var/log/mojinprince/server.log</string>
  <key>StandardErrorPath</key><string>/var/log/mojinprince/server.err</string>
</dict>
</plist>
```

### 前端连接配置

设置面板新增「服务器地址」：

- 默认 `http://127.0.0.1:8732`（嵌入式 fallback）
- 局域网 `http://192.168.x.x:8732`
- 公网 `https://mojin.example.com`（需 JWT）

---

## 9. 与现有代码的对接

### 前端改动点（按阶段）

| 阶段 | 改动文件 | 预期减少 |
| --- | --- | --- |
| 1 | `MarketService.swift` → 改为薄壳调 HTTP；删除 sina/tencent/eastmoney 拼接代码 | ~600 行 |
| 2 | `AIService.swift` → 改为 HTTP 调用；删除 OpenAI 兼容协议实现 | ~250 行 |
| 3 | `SignalTimeline.swift` / `AppSettings.swift` 的文件读写 → 改为 API 调用 | ~150 行 |
| 4 | 删除本地 `quoteVoteBadge` / `maybeAutoAIAnalyze` 等调度逻辑 | ~100 行 |

### Rust 端复用逻辑

| 原 Swift 逻辑 | Rust 落点 |
| --- | --- |
| `MarketService.fetchQuote` 多源拼接 | `service/quote/failover.rs` |
| `AIService.chat` OpenAI 协议 | `service/ai/openai.rs` |
| `AIUsageLedger` JSON 文件 | `repo/ai_usage.rs` |
| `SignalTimeline.updateMeta` | `repo/signal_event.rs` |
| `NotifyGovernor` 冷却 | `service/ai/governor.rs` |
| `checkLevelAlerts` / `checkLevelHits` / `checkPositionAlert` | `service/notify.rs` |

### 不动的前端代码

- `Charts.swift` —— 纯绘制，不涉及网络
- `ContentView.swift` —— 仅改 HTTP 调用入口，UI 结构不动
- `AlertService.swift` —— 系统通知仍由前端触发（macOS API 限制）

---

## 10. 关键风险与对策

| 风险 | 对策 |
| --- | --- |
| 网络抖动导致前端空白 | 前端保留 5 分钟本地缓存（SignalTimeline / Quote） |
| 后端挂了 | launchd 自动拉起；前端检测到 `503` 时降级到只读模式 |
| SQLite 并发写入瓶颈 | 启用 WAL 模式；写操作走单一 tokio 任务串行化 |
| NAS 资源有限 | 行情轮询 + AI 调用都加节流；持久化用 SQLite 不用 PostgreSQL |
| 鉴权漏掉导致公网裸奔 | 当前 `JWT_SECRET` 仅是预留配置，公网部署须等鉴权实现并验证后再开放 |

---

## 11. 实施 checklist

### 阶段 1 启动前

- [x] 选 NAS / 旧 Mac 部署目标（推荐 N305 / J4125 之类低功耗小主机）
- [x] 安装 Rust 工具链（`rustup`）
- [x] 创建 `mojinprince-server` Cargo 项目
- [x] 写 `migrations/0001_init.sql`
- [x] 写 `service/quote/sina.rs` / `tencent.rs` / `eastmoney.rs` 三个 provider
- [x] 写 `service/quote/failover.rs`（超时 3s / 主源失败 3 次切备）
- [x] 写 `api/quote.rs` 三个 GET 接口
- [x] 挂载 Swagger UI（`utoipa` + `utoipa-swagger-ui` vendored）
- [x] 写 `tests/quote_integration.rs`（5 项 mock + 2 项 live `#[ignore]`）
- [x] BadCode 在 HTTP 层返回 400（之前混在 502 是 bug）
- [x] 联调脚本：`dev-up.sh` / `smoke.sh` / `load-test.sh`
- [x] 改 Swift `MarketService.swift` 调用后端（已在 `GatewayMarketClient.swift` 实现）
- [x] 联调通过后保留旧实现 1 周作为回退开关
- [ ] 选目标设备并部署 → 局域网 P95 延迟验收
- [ ] 联调报告（fastfetch / NAS 上跑 load-test.sh 结果）补到本节

### 验收标准

- 行情接口 P95 延迟 < 200ms（局域网）—— 本机 200 并发 P95 已绿；待目标设备复验
- failover 触发 ≤ 3s —— 集成测试 `failover_switches_when_primary_fails` 断言已绿
- 前端无网络时显示「离线模式」+ 本地缓存数据
- 不引入新依赖到前端（`MarketService.swift` 改完后行数减少 60%+）

### 阶段 2 启动前 / 进行中（2026-09-20）

- [x] 写 `migrations/20250918000002_ai_usage.sql`（`ai_usage` + `ai_feedback` 两张表）
- [x] `service/ai/{mod,openai,ollama,governor,prompt}.rs` —— `Provider` trait + OpenAI 兼容 + Ollama 原生 + 冷却 / 配额 / 熔断 governor + 默认 system prompt
- [x] `model/ai.rs` —— `AiChatRequest` / `AiChatResponse` / `AiUsageRecord` / `AiUsageSummary`
- [x] `api/ai.rs` —— `POST /api/v1/ai/chat` / `analyze` / `reflect` + `GET /api/v1/ai/usage` / `accuracy` + `POST /api/v1/ai/feedback`
- [x] `state.rs` / `config.rs` 增加 `ai_http` 客户端 + `AI_TIMEOUT_MS` 环境变量（默认 65s）
- [x] `error.rs` 增加 `AiUpstream`（502）/ `TooManyRequests`（429）/ `Unavailable`（503）
- [x] `openapi.rs` 升级到 `0.2.0`，把 AI schema + 路由纳入 Swagger UI
- [x] Swift `LLMProvider.swift`：`GatewayFirstProvider` 网关优先 + 直连兜底；`OllamaProvider` 本地直连
- [x] Swift `ContentView.swift`：设置面板「Provider」二选一（OpenAI 兼容 / Ollama 本地），自动切换默认 base + model
- [x] Swift `MarketStore.swift`：ollama provider 不再强制要求 API Token
- [x] `tests/ai_integration.rs`：5 项 mock 用例覆盖 /chat 成功 / 上游 5xx / 非法 provider / 非法 base_url / Ollama 协议
- [x] `tests/ai_phase2_features.rs`：6 项覆盖 /analyze 落库 signal_event、/reflect 多步反思、/accuracy 命中率、/feedback 写入 ai_feedback、Governor 熔断
- [x] unit tests：`Governor::check` 冷却、`fail_streak` 熔断、`resolve` 路由
- [x] smoke test：`/health`、`/api/v1/ai/usage`、`POST /api/v1/ai/chat` 非法参数均按预期返回
- [ ] Swift 端联调：菜单「测试连接」→ GatewayFirstProvider 命中网关
- [ ] 局域网联调报告（fastfetch / NAS 端到端 P95 延迟）补到本节

### 阶段 2 验收标准

- AI 网关 P95 延迟 < 70s（默认超时 65s + 远端调用开销）—— 单机 mock 已绿；真实环境待验
- ai_usage 表每条调用都落账（成功 / 失败两条路径）
- failover：OpenAI 失败自动回退到本地 Ollama 由 Swift `GatewayFirstProvider` 兜底（不在网关层做）
- Governor 连续失败触发熔断（默认 5 次失败 → 60s 冷却）
- 前端只通过 `POST /api/v1/ai/chat` 上行，不再直连 OpenAI 协议

### 阶段 3 / 4 进行中（2026-09-20）

- [x] `migrations/20250918000003_signal_position.sql` / `20250918000004_settings.sql` / `20250918000005_scheduled_report.sql`
- [x] `api/data.rs`：signals / positions / watchlist / settings HTTP API + OpenAPI
- [x] `tests/data_phase3.rs`：3 项（信号往返与 feedback 合并 meta、持仓 / 自选 upsert、settings 按 key 合并不丢旧值）
- [x] Swift 双向同步：本地先写、服务端后写；启动服务端优先、空库用本地播种；断网回退 UserDefaults / 本地 JSON
- [x] `api/ws.rs` + 广播 Hub：`/api/v1/ws/quote` 推送自选股报价；服务端 15s ping / 45s 无 pong 断开
- [x] `service/scheduler.rs`：`spawn_quote_scheduler` 交易时段轮询 + 落库 + 广播
- [x] Swift `MarketStore.quoteStreamTask` 消费 `GatewayMarketClient.quoteUpdates()`：WebSocket 接入、连上后 HTTP 轮询降频到 ≥15s、断线 5s 重试且 HTTP 轮询始终兜底
- [x] `service/scheduler.rs`：`generate_daily_report` / `generate_weekly_report` 按 `(kind, period_key)` 幂等生成
- [x] `api/review.rs`：`GET /api/v1/reviews` + `POST /api/v1/reviews/run` + OpenAPI
- [x] `tests/review_phase4.rs`：7 项（日 / 周独立、幂等、默认 context、非法 kind 400、body 含 AI 用量与信号、列表按 kind 过滤与 limit、created_at ISO8601）
- [x] `service/scheduler.rs`：`spawn_report_scheduler` 60s 心跳收盘 / 周末**自动触发**报告生成（`report_kinds_due` 纯函数决策 + `ensure_report` 仅补缺失基线版，已存在则跳过）
- [x] `tests/report_scheduler.rs`：2 项（基线版只生成一次且不覆盖手动刷新、周报 ISO 周 period_key）
- [x] Swift 端消费 `/api/v1/reviews`：`Sources/ReviewDesk.swift` 新增 `ReportClient`（list / run），`MarketStore.submitReview` 手动生成日报 / 周报、`MarketStore.loadReviewHistory` 拉取历史，复盘子页展示近 14 天报告
- [x] `migrations/20250918000006_ingest.sql`：`news_item` / `research_report` / `sector_board` 三张缓存表（按 `(code,url)` / `(code,info_code)` / `(code,board_code)` 主键 UPSERT）
- [x] `service/ingest.rs`：东财新闻（全文搜索）/ 研报（研报库）/ 板块（成分 + 批量行情）三个 provider，北京时间统一转 UTC
- [x] `api/ingest.rs`：`GET /api/v1/news/{code}`（?limit=&hours=）、`GET /api/v1/reports/{code}`（?limit=&days=）、`GET /api/v1/sector/{code}` + OpenAPI
- [x] `tests/ingest_integration.rs`：9 项（新闻解析 / 时间窗 / 重复拉取幂等、研报字段与详情页 URL、板块成分与行情合并、非法代码 400 不打上游、上游 5xx → 502、analyze 注入近 24h 新闻、默认不注入、评级信号化幂等）+ 3 项 live 测试（`#[ignore]`，直连东财）
- [x] `api/ai.rs`：`POST /api/v1/ai/analyze` 新增 `include_news`（默认 false），命中近 24h 新闻（≤5 条）拼进 prompt 并顺带落库；拉取失败只 warn 不阻断分析
- [x] `service/scheduler.rs`：`spawn_ingest_scheduler` 工作日 16:00 后为自选股（≤60 只，300ms 间隔）增量刷新新闻 / 研报 / 板块，同日只跑一次
- [x] `service/ingest.rs::persist_report_signals`：评级信号化 —— 刷新研报时把近 7 天且评级明确的写成 `kind=report` 的 `signal_event`（看多 / 看空 / 新覆盖 / 上调 / 下调标题；id 由 `(code, info_code)` 派生为 UUID 形态 + `INSERT OR IGNORE` 幂等；`meta` 携带 reportId / 机构 / 评级，`at` 取发布日北京 0 点）
- [x] `Sources/SignalTimeline.swift`：`kindLabel` 加 `report → 机构研报`，服务端信号经既有同步通道直接展示
- [x] Swift 盯盘页「资讯 · 研报 · 板块」面板：`GatewayMarketClient` 新增 `news/reports/sector` 三个拉取方法（snake_case CodingKeys + iso8601），`MarketStore.ensureCodeInfo` 切换自选自动加载（同标的只拉一次、失败降级为提示），板块涨跌色块（主营优先）、研报评级行 / 新闻流点击打开东财原文
- [x] 收盘复盘通知（ROI #6）：`MarketStore.pollReviewNotify` 工作日 15:05 后独立轮询当日日报（`GET /reviews?kind=daily&limit=1`，`periodKey == 今日` 即通知，`lastReviewNotifyDay` 本地去重），`flashAndNotify` 透传 userInfo，通知默认点击 `kind=review → review` 动作跳复盘页
- [x] 智能止损 / 止盈（ROI #2，2026-09-21）：`Sources/StopTakeAdvisor.swift` ATR14 + 近 20 日结构高低 + 成本回撤 / 阻力三锚点（止损取保守、止盈取先到），盈亏比 < 1.5 / 止损过近自动降级提示；盯盘页建议卡（锚点 tooltip + 手动覆盖标记）一键应用 + 止损/止盈委托草稿预填
- [x] 持仓止盈链路：`PositionNote.takeProfit` 建模 + `PUT /positions` 同步 + `linkTakeToAbove` 对称联动；跌破止损 / 触达止盈专属提醒（`kind=stopTake`，同价时抑制通用到价提醒，通知点击按 side 预填委托草稿）
- [x] `scheduler.rs::build_report` 止损止盈执行对照：`TicketSummary` 加 `side` / `price`（`#[serde(default)]` 兼容旧客户端），已成交卖出 vs `position` 止损 / 止盈价输出偏差；`tests/review_phase4.rs` +1、`tests/data_phase3.rs` 补 takeProfit 断言
- [x] 失效归因周报（ROI #9）：`scheduler.rs::attribute_level`（纯函数单测）+ `build_level_attribution`（周报专用，仅 weekly）——level 信号按 `day_bar` 判定 命中 / 穿越未确认 / 未到价（偏离度 TOP3）/ 窗口未满 / 无日线 + `ai_feedback` ignore 计数，`payload.summary` 带结构化字段；`tests/review_phase4.rs` +1（含周一凌晨时间夹取防周界 flake）
- [x] 多标的组合视图（ROI #10）：`Sources/PortfolioBuilder.swift`（纯函数：市值降序、无报价沉底、名称三级回退 自选名→报价名→code、脏数据过滤）+ `MarketStore.portfolio`（settings.positions × watchQuotes 实时聚合）+ 自选页顶部「持仓组合」卡（总市值 / 浮盈 / 当日盈亏 / 无报价提示，持仓行带占比条与止损止盈标记，点击跳盯盘）；独立编译 19 项断言
- [x] 收盘自动数据归档（ROI #12 M1，2026-09-21）：`scheduler.rs::maybe_dump_daily_archive` 挂 `spawn_report_scheduler`——为**最近已收盘交易日**（`last_closed_trading_day`：工作日 15:10 后为当日，否则回退上一工作日，周末 / 周一自动补周五）dump `archives/{date}.json`（自选股 minute_bar 分时 + quote 收盘快照 + 当日 signal_event + ai_usage 汇总 + position 快照，minutes 为 `[ts,price,avg,volume]` 紧凑数组）；文件存在即跳过幂等；目录从 `database_url` 推导（dev `data/archives/`，常驻安装 Application Support）；`tests/report_scheduler.rs` +2（盘中补昨日 / 收盘 dump 内容与幂等 / 周末目标周五）；导出端点与回放 UI 待做
- [ ] 目标设备 / 局域网端到端联调报告

---

> 维护：每完成一个阶段，更新 §7 路线状态 + §9 前端改动点的实际数字。
> 与 [`未来演进方向.md`](未来演进方向.md) §F / §G 联动；与 [`按ROI排序.md`](按ROI排序.md) 联动（独立服务 + 跨设备 → 长期价值提升）。
