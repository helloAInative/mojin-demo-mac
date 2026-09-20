# mojinprince-server

> 摸金小王子 · 行情网关（阶段 1）
> Rust + Actix-web 4 + SQLite，独立服务部署。

完整架构见 [`docs/架构-Rust后端.md`](../docs/架构-Rust后端.md)。

---

## 状态（2026-09-18）

- ✅ 阶段 1：行情网关代码（实时报价 + 分时 + 日 K）
  - 三个数据源：新浪（主力）/ 腾讯 / 东财，自动 failover
  - 熔断：单源连续失败 ≥3 次 → 跳过 60s
  - SQLite 落库（quote / minute_bar / day_bar 三表，WAL 模式）
  - 分时、前复权日 K 实时拉取腾讯接口并写入 SQLite；Swift 前端优先走网关，故障时临时直连回退
  - 集成测试：5 项 mock failover / 2 项真实外网（`#[ignore]`），3 项单元测试
  - Swagger UI：`/swagger-ui/`（已挂载，bundled assets，build 时无需联网）
  - 联调脚本：`dev-up.sh` / `smoke.sh` / `load-test.sh`
  - BadCode 在 HTTP 层返回 `400 bad_request`（不再混入 502）
  - 本机联调通过：smoke 全部 ✅，200 并发 P95 < 200ms（待目标设备验证）
- ⏳ 阶段 2-4 待办：AI 网关 / 数据迁移 / WebSocket 推送

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
│   └── 20250918000001_init.sql   # quote / minute_bar / day_bar
├── scripts/
│   ├── dev-up.sh                 # 后台启动 + 等待就绪
│   ├── smoke.sh                  # 冒烟
│   ├── load-test.sh              # 延迟基准
│   └── verify.sh                 # 一键端到端：起 + 冒烟 + 压测
├── src/
│   ├── lib.rs                    # 业务模块汇总（让 tests 可用）
│   ├── bin/mojinprince-server.rs # 启动入口
│   ├── config.rs
│   ├── state.rs
│   ├── error.rs
│   ├── model/quote.rs
│   ├── api/
│   │   ├── mod.rs
│   │   ├── health.rs
│   │   ├── quote.rs
│   │   └── openapi.rs            # ApiDoc 汇总
│   └── service/
│       ├── mod.rs
│       └── quote/
│           ├── mod.rs            # enum 派发 + normalize_code
│           ├── sina.rs           # 默认 https://hq.sinajs.cn
│           ├── tencent.rs        # 默认 http://qt.gtimg.cn
│           ├── eastmoney.rs      # 默认 https://push2.eastmoney.com
│           ├── history.rs        # 腾讯分时 / 前复权日 K
│           └── failover.rs       # 顺序调度 + 熔断
├── tests/
│   └── quote_integration.rs      # 5 项 mock + 2 项 live
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
