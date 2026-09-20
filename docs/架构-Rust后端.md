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
│   └── 0003_positions.sql
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

// 数据接入（§F）
GET  /api/v1/news/{code}                  // 新闻流
GET  /api/v1/reports/{code}               // 研报
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
```

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

### 阶段 3：信号 / 持仓 / 设置（2 天）

- 把 `SignalTimeline` / `AppSettings` / `PositionNote` 的 JSON 落库迁到 SQLite + HTTP API。
- 前端加 5 分钟本地缓存（启动拉一次，断网用本地兜底）。
- `SignalEvent.meta` 仍按 JSON 存，迁移成本最低。

### 阶段 4：推送 + 调度 + 数据接入（2–3 天）

- WebSocket 推送行情变化 / 信号命中。
- 后端 scheduler 跑收盘复盘 / 周报导出。
- 接新闻 / 研报 / 板块数据（§F.3–F.4）。
- 至此 §F 数据接入全部走 Rust，前端只剩 UI。

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

---

> 维护：每完成一个阶段，更新 §7 路线状态 + §9 前端改动点的实际数字。
> 与 [`未来演进方向.md`](未来演进方向.md) §F / §G 联动；与 [`按ROI排序.md`](按ROI排序.md) 联动（独立服务 + 跨设备 → 长期价值提升）。
