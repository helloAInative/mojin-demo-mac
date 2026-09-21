# mojinprince-server

> 摸金小王子 · 行情、AI 与数据网关（阶段 4 进行中）
> Rust + Actix-web 4 + SQLite，独立服务部署。

完整架构见 [`docs/架构-Rust后端.md`](../docs/架构-Rust后端.md)。

---

## 状态（2026-09-20）

- ✅ 阶段 1：行情网关代码（实时报价 + 分时 + 日 K）
  - 三个数据源：新浪（主力）/ 腾讯 / 东财，自动 failover
  - 熔断：单源连续失败 ≥3 次 → 跳过 60s
  - SQLite 落库（quote / minute_bar / day_bar 三表，WAL 模式）
  - 分时、前复权日 K 实时拉取腾讯接口并写入 SQLite；Swift 前端优先走网关，故障时临时直连回退
  - 集成测试：5 项 mock failover / 2 项真实外网（`#[ignore]`），本阶段单元测试 3 项
  - Swagger UI：`/swagger-ui/`（已挂载，bundled assets，build 时无需联网）
  - 联调脚本：`dev-up.sh` / `smoke.sh` / `load-test.sh`
  - BadCode 在 HTTP 层返回 `400 bad_request`（不再混入 502）
  - 本机联调通过：smoke 全部 ✅，200 并发 P95 < 200ms（待目标设备验证）
- ✅ 阶段 2：AI 网关（OpenAI 兼容 / Ollama、用量、命中率、熔断）
- ✅ 阶段 3：信号 / 持仓 / 自选 / 设置 API 与 Swift 双向同步
- 🚧 阶段 4：WebSocket 行情推送 + 自选股交易时段调度 + 收盘复盘 / 周报生成（按需 API + 收盘 / 周末自动触发，按 `(kind, period_key)` 幂等，9 项集成测试）已完成，Swift 端已接入复盘历史 / 手动生成；**新闻 / 研报 / 板块数据接入（§F.3–F.4）已完成**（按需 API + 每日收盘后增量刷新 + 评级信号化 `kind=report`；Swift 盯盘页已接入「资讯 · 研报 · 板块」面板）；**A 股池智能推荐（服务端）已完成**：涨幅榜→量化→消息面→AI 精排四层漏斗 + T+5 回测闭环（Swift 展示页待接）
- ✅ 测试全绿（2026-09-21 复跑 `cargo test --offline`）：单元 31 项 + 集成 45 项（行情 5 / AI 5+6 / 数据 3 / 复盘 9 / 报告调度 4 / 数据接入 9 / 智能推荐 3 / 导出 1），另有 5 项真实外网用例默认 `#[ignore]`；3 项数据接入 live 用例已联网验证通过

---

## 构建

```bash
cd mojinprince-server
CARGO_TARGET_DIR=./target cargo build --release
```

二进制路径：`./target/release/mojinprince-server`

> ⚠️ `CARGO_TARGET_DIR` 必须显式指向 `./target`，否则 cargo 会被环境劫持到 `/var/folders/...`。
> 工程内已配 `.cargo/config.toml`，再次构建时 `cargo build` 即可命中本地 `target/`。

> ⚠️ Swagger UI 用 `utoipa-swagger-ui` 的 `vendored` 特性，bundled assets 编译进二进制，**build 时不需要联网下载**。

## 启动

### 推荐：开发模式（debug 构建，省时间）

```bash
./scripts/dev-up.sh             # 后台启动，监听 127.0.0.1:8732，日志在 logs/server.log
./scripts/dev-up.sh STOP=1      # 停掉后台实例
```

### 本机常驻（登录后自动启动）

```bash
./scripts/install-local-service.sh
```

安装器会将 release 二进制、数据库和日志放到 `~/Library/Application Support/MojinPrinceServer/`，避免 macOS 对 Desktop 的后台访问限制。停用服务：`./scripts/uninstall-local-service.sh`；数据默认保留。

### 直接启（生产 / 自管 systemd）

```bash
DATABASE_URL="sqlite://$(pwd)/data/mojinprince.db" \
BIND_ADDR=0.0.0.0:8732 \
RUST_LOG=info \
./target/release/mojinprince-server
```

> ⚠️ **`DATABASE_URL` 必须用绝对路径**。相对路径 `./data/test.db` 在 sandbox / systemd / launchd 不同执行环境下 cwd 不一样，会出现 `unable to open database file` 但目录又确实存在的怪现象。

## 联调脚本

### 一键端到端（推荐）

```bash
./scripts/verify.sh
#   == smoke ==      12/12 ✅
#   == load ==       200 并发，P95 85ms / success 200/200
```

`verify.sh` 内部自起服务、自跑 smoke + load-test、然后清理。适合 CI 与首次环境验收。

### 分步手动

```bash
# 1. 启动
./scripts/dev-up.sh

# 2. 冒烟（health / quote / minutes / days / 错误码 / Swagger UI）
./scripts/smoke.sh

# 3. 延迟基准（默认 N=200 并发 8）
./scripts/load-test.sh
#   samples: 200
#   success: 200
#   P50:    35.9ms
#   P95:    85.0ms
#   max:   277.5ms

# 4. 停掉
./scripts/dev-up.sh STOP=1
```

> ⚠️ Cursor 的 shell 沙盒会把 `.cargo/config.toml` 的 `target-dir` 劫持到 `/var/folders/.../cargo-target/`，工程内 `target/debug/mojinprince-server` 可能是旧的。`verify.sh` 会自动探测并挑最新的 binary，无需关心。

## 测试

```bash
# 单元 + 集成（mock）
cargo test

# 真实外网（默认忽略）
cargo test -- --ignored

# 全部
cargo test -- --include-ignored
```

集成测试覆盖（`tests/quote_integration.rs`）：

| 用例 | 覆盖目标 |
| --- | --- |
| `failover_succeeds_on_first_source` | 主源（新浪）成功 → 200 |
| `failover_switches_when_primary_fails` | 主源失败 → 切备源（腾讯）成功，整体 ≤ 3s |
| `failover_returns_error_when_all_fail` | 三源全败 → 502 + 结构化错误 |
| `breaker_skips_after_threshold` | 连续失败 N 次后熔断，跳过主源 |
| `bad_code_is_rejected_without_http` | 非法代码 → 400，**不发任何 HTTP 请求** |
| `live_sina_sh600460`（`#[ignore]`） | 真实外网冒烟 |
| `live_tencent_sz000001`（`#[ignore]`） | 真实外网冒烟 |

## API

### `GET /health`

```json
{"status":"ok","db":"ok"}
```

### `GET /api/v1/quote/{code}`

`code` 支持：纯数字（自动识别 sh/sz/bj）、或带前缀（`sh600460` / `sz000001` / `bj835899`）。

```bash
$ curl http://127.0.0.1:8732/api/v1/quote/sh600460
{"code":"sh600460","name":"士兰微","price":32.61,"prev":31.99,"open":32.5,"high":32.75,"low":31.77,"volume":55092183,"amount":1779905404.0,"source":"sina","ts":"2026-09-18T02:40:06.768634Z"}
```

### `GET /api/v1/quote/{code}/minutes?limit=N`

返回当日分时点，按时间升序；每次请求拉取腾讯分时并写入 `minute_bar`。`volume` 为当分钟成交手数。

### `GET /api/v1/quote/{code}/days?limit=N`

返回前复权日 K，按日期升序；每次请求拉取腾讯日 K 并写入 `day_bar`。

### Swagger UI

```bash
# 浏览器打开
http://127.0.0.1:8732/swagger-ui/

# OpenAPI JSON
http://127.0.0.1:8732/api-docs/openapi.json
```

### 阶段 3 数据接口

- `GET|POST|DELETE /api/v1/signals`、`POST /api/v1/signals/{id}/feedback`
- `GET /api/v1/positions`、`PUT /api/v1/positions/{code}`
- `GET|POST /api/v1/watchlist`、`DELETE /api/v1/watchlist/{code}`
- `GET|PUT /api/v1/settings`（按 key 合并，不会覆盖请求中未提供的设置）

### WebSocket 实时行情

连接 `ws://127.0.0.1:8732/api/v1/ws/quote` 后，会收到自选股报价：

```json
{"type":"quote","quote":{"code":"sh600460","price":32.61,"source":"sina","ts":"2026-09-20T08:00:00Z"}}
```

后台只在北京时间工作日 `09:15–11:30`、`13:00–15:05` 拉取自选股；服务端每 15s 发一次 ping，45s 内无 pong 则断开。Swift 收到首条推送后自动降低 HTTP 轮询频率（≥15s 一次），断线后每 5 秒重连并恢复 HTTP 兜底。

### A 股池智能推荐

`GET /api/v1/picks?date=YYYY-MM-DD`（缺省最近一天）：当日 Top5 推荐 + 近 30 天 T+5 回测统计（样本数 / 胜率 / 平均涨幅）。

`POST /api/v1/picks/run`：重新生成（同日 DELETE+INSERT 全量刷新）。可选 body 透传 AI 配置做精排：

```json
{"ai": {"provider": "openai", "base_url": "…/v1", "api_key": "…", "model": "qwen3.7-plus"}}
```

六层漏斗（v2 多因子）：
1. 东财涨幅榜 Top100 候选（push2delay，剔除 ST/退市/次新/一字板/RSI 超买）
2. 技术指标量化打分：MACD 金叉 30 / 多头 10（零上金叉额外 +5）、RSI 健康区 15、放量 20、20 日箱体 15 + 近高 10、均线多头 10、KDJ 金叉 10 / 超买 −10、BOLL 中轨上方 5 / 突破上轨 8、BIAS5 超涨 −8
3. 板块动量：候选池行业聚合，强势行业（Top5 且均值 ≥2%）+15、弱势（均值 ≤0）−10
4. 隔夜美股（腾讯 usDJI/usIXIC）：情绪 ±10，纳指跌 >1% 时半导体/电子类额外 −15
5. 消息面：研报评级上调 +25 / 下调 −20；当日新闻拉取落库 + 关键词（利好 +5 / 利空 −8，±25 封顶）+ 热度
6. AI 精排（密钥只在请求内透传不落库，失败降级量化序）；综合分 <60 不入选

工作日 15:30 后调度器自动生成纯量化版；T+1/T+5 收盘价自动回写 `meta.outcome`；`stats.tags` 输出**标签级胜率**（哪个因子真的有效）。

### 收盘复盘 / 周报

- `POST /api/v1/reviews/run?kind=daily|weekly`：生成一份报告。可选 JSON body `{tickets, diary, signals, focusCodes}` 补全 Swift 端内存数据。按 `(kind, period_key)` UPSERT 幂等——重复调用刷新同一条记录（`created_at` 仅首次写入）。`period_key`：daily 为 `YYYY-MM-DD`，weekly 为 `YYYY-Www`（ISO 周，周一为周首日）。
- `GET /api/v1/reviews?kind=daily|weekly&limit=N`：按 `created_at` 倒序列出已落库报告。

报告 body 汇总后端落库的信号事件、level 命中率、AI 调用成功 / 总数与 token / 成本，并拼接客户端传入的日记、委托与信号时间线，落 `scheduled_report` 表。委托带 `side` / `price` 时（ROI #2），正文还会把「已成交卖出」与 `position` 表的止损 / 止盈价做执行对照（偏差金额与百分比），未设价位的给出纪律提示。周报（ROI #9）额外带「失效归因（level 预警）」小节：本周 level 信号按 `day_bar` 判定 命中 / 穿越未确认 / 未到价（偏离度 TOP3）/ 窗口未满 / 无日线，并汇总被忽略的 AI 反馈；`payload.summary` 输出结构化计数。

Swift 复盘页通过 `ReportClient` 调用这两个接口：手动生成日报 / 周报 + 拉取历史列表。

自动触发：`spawn_report_scheduler` 独立 60s 心跳（与 3s 行情轮询分离）。北京时间工作日 15:05 后补当日日报，周五 15:05 后与周六 / 周日补周报（覆盖周五晚间服务器未开机的情况）。仅当 `(kind, period_key)` 不存在时生成一份无客户端 context 的基线版，已存在则跳过；之后仍可手动 `POST /reviews/run` 带日记 / 委托刷新同一条记录。

### 新闻 / 研报 / 概念板块（§F.3–F.4）

三个端点都是"按需拉取东财 + UPSERT 落库"：上游成功即写 SQLite（`news_item` / `research_report` / `sector_board`），客户端可反复读、断网可离线读库。

- `GET /api/v1/news/{code}?limit=20&hours=72`：东财全文搜索个股新闻，按发布时间倒序；`hours` 为响应的时间窗（落库的是全量，窗口只作用于返回）。条目缺少链接会被丢弃。
- `GET /api/v1/reports/{code}?limit=20&days=365`：东财研报库，含机构 / 评级 / 上次评级 / 评级变动 / 分析师 / 目标价上下限，`url` 指向东财研报详情页。
- `GET /api/v1/sector/{code}`：先拉成分列表（`RPT_F10_CORETHEME_BOARDTYPE`，最多 50 个板块），再用 `ulist.np` 批量补板块指数与涨跌幅；停牌 / 无数据时 `price` / `change_pct` 为 `null`。

`POST /api/v1/ai/analyze` 新增可选 `include_news: true`（需要 `code`）：开启后把近 24h 新闻（≤5 条）拼到 prompt 末尾（`近 24h 新闻：` + 每行 `[媒体] 标题 (MM-DD HH:MM)`），并顺带把用到的新闻落库；拉取失败只记 `warn`，不影响分析本身。默认关闭。

自动刷新：`spawn_ingest_scheduler` 独立 5 分钟心跳，北京时间工作日 16:00 之后为自选股（≤60 只，逐只间隔 300ms）增量刷新新闻 / 研报 / 板块，同一天只跑一次；单只标的某个源失败只告警，不中断其余标的。

`GET /api/v1/export/day?date=YYYY-MM-DD`：返回某交易日的归档 JSON（与 `data/archives/{date}.json` 完全同构，缺省为最近已收盘交易日）——Swift 端「历史回放」与导出共用。

每日数据归档（ROI #12）：`spawn_report_scheduler` 顺带把**最近已收盘交易日**（工作日 15:10 后为当日；盘中 / 周末 / 周一自动回补上一交易日）的自选股分时（minute_bar）、当日收盘快照（quote 最后一条）、当日信号、AI 用量汇总与持仓快照写成 `{数据库目录}/archives/{date}.json`——同一天文件已存在即跳过，长期资产按日沉淀。归档组装三处复用：自动 dump、HTTP 导出、Swift 回放。

评级信号化（§F.4）：每日刷新研报后，把「近 7 天发布且评级明确」的研报转成 `kind=report` 的 `signal_event`（买入 / 增持 → `机构看多`，卖出 / 减持 → `机构看空`，中性 / 持有不出信号）。信号 id 由 `(code, info_code)` 派生（UUID 形态）+ `INSERT OR IGNORE`，同一研报永远只出一条；`at` 取研报发布日的北京时间 0 点，正文含分析师 / 行业 / 目标价，`meta` 携带 `reportId` / 机构 / 评级。信号经既有 `GET /api/v1/signals` 同步通道自动到达 Swift 端（`kindLabel` 显示「机构研报」）。注意：客户端删除后 7 天窗口内会被刷新补回。

### 错误响应

```json
{"error":"upstream_exhausted","message":"upstream failed after 3 tries: bad code: notacode"}
```

| HTTP | error code | 含义 |
| --- | --- | --- |
| 400 | bad_request | 代码非法 |
| 502 | upstream_exhausted | 所有行情源失败 |
| 502 | upstream_parse | 行情源返回数据无法解析 |
| 500 | db_error / internal | 内部错误 |

---

## 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `BIND_ADDR` | `127.0.0.1:8732` | 监听地址；当前版本只建议本机或受信任局域网 |
| `DATABASE_URL` | `sqlite://./data/mojinprince.db` | SQLite 连接串（推荐绝对路径） |
| `QUOTE_TIMEOUT_MS` | `3000` | 单次行情请求超时 |
| `QUOTE_FAIL_THRESHOLD` | `3` | 单源连续失败次数达到该值后熔断 60s |
| `QUOTE_SOURCES` | `sina,tencent,eastmoney` | 主源顺序 |
| `API_PREFIX` | `/api/v1` | API 路径前缀 |
| `JWT_SECRET` | `""` | 预留字段；当前版本尚未实现鉴权，不能仅靠设置此值公开部署 |

---

## 目录结构

```
mojinprince-server/
├── Cargo.toml
├── .cargo/config.toml            # 锁定 target-dir
├── migrations/
│   ├── 20250918000001_init.sql           # quote / minute_bar / day_bar
│   ├── 20250918000002_ai_usage.sql       # ai_usage / ai_feedback
│   ├── 20250918000003_signal_position.sql# signal_event / position / watchlist
│   ├── 20250918000004_settings.sql       # settings
│   ├── 20250918000005_scheduled_report.sql # scheduled_report（复盘 / 周报）
│   └── 20250918000006_ingest.sql         # news_item / research_report / sector_board
├── scripts/
│   ├── dev-up.sh                 # 后台启动 + 等待就绪
│   ├── smoke.sh                  # 冒烟
│   ├── load-test.sh              # 延迟基准
│   └── verify.sh                 # 一键端到端：起 + 冒烟 + 压测
├── src/
│   ├── lib.rs                    # 业务模块汇总（让 tests 可用）
│   ├── bin/mojinprince-server.rs # 启动入口 + 路由注册 + 三个 scheduler 接线
│   ├── config.rs
│   ├── state.rs
│   ├── error.rs
│   ├── model/                    # quote / ai / data / review / ingest DTO
│   ├── api/
│   │   ├── mod.rs
│   │   ├── health.rs
│   │   ├── quote.rs
│   │   ├── ai.rs                 # 阶段 2：chat / analyze / reflect / usage / accuracy / feedback
│   │   ├── data.rs               # 阶段 3：signals / positions / watchlist / settings
│   │   ├── review.rs             # 阶段 4：reviews / reviews/run
│   │   ├── ws.rs                 # 阶段 4：/ws/quote 推送
│   │   ├── ingest.rs             # §F.3–F.4：news / reports / sector
│   │   └── openapi.rs            # ApiDoc 汇总
│   ├── repo/                     # signal / settings 数据访问
│   └── service/
│       ├── mod.rs
│       ├── scheduler.rs          # 阶段 4：交易时段轮询 + 收盘 / 周末自动补报告 + 每日数据接入刷新
│       ├── ingest.rs             # §F.3–F.4：东财新闻 / 研报 / 板块 provider + 落库
│       ├── ai/                   # mod / openai / ollama / governor / prompt
│       └── quote/
│           ├── mod.rs            # enum 派发 + normalize_code
│           ├── sina.rs           # 默认 https://hq.sinajs.cn
│           ├── tencent.rs        # 默认 http://qt.gtimg.cn
│           ├── eastmoney.rs      # 默认 https://push2.eastmoney.com
│           ├── history.rs        # 腾讯分时 / 前复权日 K
│           └── failover.rs       # 顺序调度 + 熔断
├── tests/
│   ├── quote_integration.rs      # 5 项 mock + 2 项 live
│   ├── ai_integration.rs         # 阶段 2 mock
│   ├── ai_phase2_features.rs     # 阶段 2 analyze / reflect / accuracy / feedback / governor
│   ├── data_phase3.rs            # 阶段 3 数据接口
│   ├── review_phase4.rs          # 阶段 4 复盘 / 周报幂等
│   ├── report_scheduler.rs       # 阶段 4：自动触发报告（基线版 / ISO 周 key）
│   └── ingest_integration.rs     # §F.3–F.4：8 项 mock + 3 项 live（新闻 / 研报 / 板块 / analyze 注入）
└── target/                       # 构建产物（gitignore）
```

---

## 阶段 1 验收标准

- ✅ 三源代码完成且 enum 派发正常
- ✅ 错误响应结构化（HTTP 状态码 + JSON error/message，BadCode → 400）
- ✅ SQLite WAL 模式 + 同步创建父目录
- ✅ 前端优先调用后端；支持设置地址，失败时回退旧直连
- ✅ Swagger UI 自带，proc 可在 `/swagger-ui/` 直接试调
- ✅ 集成测试 mock + failover + 熔断 + BadCode 全绿
- ✅ 联调脚本化：`dev-up.sh` / `smoke.sh` / `load-test.sh`
- ⏳ P95 延迟 < 200ms（局域网，目标设备联调时验证）—— 本机 200 并发 P95 已绿
- ⏳ failover 触发 ≤ 3s（目标设备联调时验证）—— 集成测试断言已绿
- ⏳ 在目标 NAS / 旧 Mac 上部署并联调，完成局域网延迟验收

---

> 前端设置 → 行情中填写服务器地址。默认 `http://127.0.0.1:8732`，Rust 服务需单独启动。阶段 2 再迁移 AI。
