# 更新记录

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构；正式版本计划使用语义化版本号。

## Unreleased

### Added

- SwiftUI macOS 菜单栏行情、指标、预警、策略、回测与复盘功能
- Rust Actix Web 行情网关，支持三数据源 failover、熔断和 SQLite 持久化
- 分时与前复权日 K API、Swagger UI、冒烟测试和并发基准脚本
- Swift 前端网关设置、连接测试与临时直连回退
- 本机 LaunchAgent 安装和卸载脚本
- 持仓浮盈浮亏详情与一键复制摘要
- Rust AI 网关：OpenAI 兼容 / Ollama 路由、用量账本、命中率、冷却与熔断 Governor；Swift `GatewayFirstProvider` 网关优先 + Ollama 直连兜底
- 信号 / 持仓 / 自选 / 设置 HTTP API 与 Swift 双向同步；断网回退 UserDefaults / 本地 JSON 缓存
- WebSocket 实时行情推送 `/api/v1/ws/quote` 与自选股交易时段轮询调度；Swift 首推降频、断线重连与 HTTP 兜底
- 收盘复盘 / 周报生成 API（`/api/v1/reviews`、`/api/v1/reviews/run`），按 `(kind, period_key)` 幂等落库
- Swift 复盘页接入后端报告：`ReportClient` 调用 `/api/v1/reviews*`，支持手动生成日报 / 周报与历史列表
- 收盘 / 周末自动触发报告：`spawn_report_scheduler` 60s 心跳按北京时间补生成缺失的日报 / 周报基线版，已存在记录不覆盖，手动刷新仍走 `/api/v1/reviews/run`
- 新闻 / 研报 / 概念板块数据接入（§F.3–F.4）：`GET /api/v1/news/{code}`、`/api/v1/reports/{code}`、`/api/v1/sector/{code}` 按需拉取东财并 UPSERT 落 `news_item` / `research_report` / `sector_board` 缓存表
- `POST /api/v1/ai/analyze` 新增 `include_news`：开启后把近 24h 新闻（≤5 条）拼进 prompt，拉取失败只告警不阻断分析
- `spawn_ingest_scheduler`：工作日 16:00 后为自选股（≤60 只，300ms 间隔）增量刷新新闻 / 研报 / 板块，同日只跑一次
- 评级信号化（§F.4）：每日刷新研报时把近 7 天且评级明确的研报写成 `kind=report` 的 `signal_event`（机构看多 / 看空），id 由 `(code, info_code)` 派生幂等不重复，经既有 signals 同步自动到 Swift（`kindLabel`「机构研报」）
- Swift 盯盘页「资讯 · 研报 · 板块」面板（§F.3–F.4）：`GatewayMarketClient` 新增 `news / reports / sector` 拉取，切换自选自动加载；板块涨跌色块（主营优先）、研报评级行与近 72h 新闻流，点击打开东财原文；网关不可用降级为提示并支持手动重试

### Fixed

- 修复集成测试偶发 `_sqlx_migrations UNIQUE` 失败：临时 SQLite 路径加入线程 id，避免同进程并行测试在同一时钟粒度内撞库
