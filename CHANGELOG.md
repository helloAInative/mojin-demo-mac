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
- 收盘复盘通知（ROI #6）：工作日 15:05 后轮询当日日报，生成即发 macOS 通知（当日去重），点击跳复盘页
- 智能止损 / 止盈（ROI #2）：`StopTakeAdvisor` 按 ATR14 + 近 20 日结构高低 + 成本回撤 / 阻力三锚点推导，盈亏比 < 1.5 或止损过近自动降级为提示；盯盘页建议卡一键应用（联动到价上/下 + 网关同步）与止损/止盈委托草稿预填
- 持仓止盈价全链路：`PositionNote.takeProfit` 建模（向后兼容解码）、`PUT /positions` 停止硬编码 0、设置页可编辑、跌破止损 / 触达止盈专属提醒（`kind=stopTake`，通知点击按 side 预填对应委托草稿）
- AI 点评止损止盈位：`analyze` prompt 注入建议数值与锚点依据，AI 只做叙事复核不另给价
- 复盘「止损止盈执行对照」：日报正文把已成交卖出与持仓止损 / 止盈价对比（偏差金额与百分比），未设价位的给出纪律提示；委托 context 新增 `side` / `price` 字段
- 命中率时序曲线（ROI #5 / D.1）：AI 页三件套下方新增「命中率曲线 · 近 30 天」——按日历日分桶 level 事件（空档直观），7 日滚动命中率折线 + 每日信号数柱（命中着色），悬停看单日明细与滚动值
- AI 智能降级熔断（ROI #7 / §A.3 配套）：`AIDegradeGovernor` 连败 ≥3 次熔断（10 分钟起步指数翻倍封顶 60 分钟）、429 限流 30 分钟、401/403/402 长 24 小时；熔断期内自动分析直接规则摘要（不再白等超时），手动「立即分析」半开放行，成功清零；状态条展示熔断剩余 / 连败计数 / 错误分类
- 失效归因周报（ROI #9）：周报正文新增「失效归因（level 预警）」——本周 level 信号按 `day_bar` 判定为 命中 / 穿越未确认（曾到价但 6h 未确认，多为假突破扫损）/ 未到价（最近偏离度 + 已过天数，TOP3）/ 窗口未满 / 无日线，另汇总被忽略的 AI 反馈计数；`payload.summary` 增加对应结构化字段
- 多标的组合视图（ROI #10）：自选页顶部「持仓组合」卡——总市值 / 总浮盈（收益率）/ 当日盈亏聚合 + 每只持仓行（现价、当日涨跌、浮盈、市值占比条、止损止盈标记、无报价提示），点击行跳盯盘；`PortfolioBuilder` 纯函数聚合（脏数据过滤、无报价沉底、名称三级回退）
- 周报导出（ROI #11）：复盘历史每条报告可「导出」为 .md（标题 + 导出时间 + 正文，临时文件 + 保存面板，与分时 CSV 同款交互），右键可复制全文 Markdown
- A 股池智能推荐（Swift 复盘页）：「智能推荐 · A 股池」卡——Top5 行（排名、行业、当日涨跌、量化标签胶囊、T+5 回测着色、量化分）+ 近 30 天 T+5 胜率统计行；「AI 精排」按钮透传本机 Keychain 密钥触发服务端精排（密钥不落库），未配置 AI 自动降级纯量化；行点击自动加入「观察」组并跳盯盘
- 每日数据导出 + 历史回放（ROI #12 收尾）：`GET /export/day?date=` 返回当日归档（与收盘自动 dump 同构，缺省最近已收盘交易日）；Swift 复盘页新增「历史回放」区——最近 7 日选择 → 归档分时（北京时间 HHmm 转换）喂 MinuteChart（叠加当前关键位）+ 当日信号列表，换标的自动重取对应分时；「导出 .json」一键存档完整数据
- 推荐候选池 fallback：东财 push2 断连时自动切新浪 A 股涨幅榜（同属既有三源；剔除北交所 / ST，无行业字段时板块动量因子自动降级）——push2delay 网络抖动不再阻断推荐生成
- 构建修复：`[profile.release]` 的 `strip = true` 会剥坏 proc-macro dylib（dlopen 报 mis-aligned LINKEDIT），改为 `strip = false` + 安装脚本只 strip 最终可执行文件
- 常驻服务数据迁移修复：阶段 1 时代的旧库手工补 0001 signal_event 段并校正校验和，使 0002–0007 正常应用（已先备份 `mojinprince.db.bak-20260921`）
- A 股池智能推荐 v2（多因子高胜率逻辑）：**板块动量**（候选池行业聚合，强势行业 +15 / 弱势 −10）+ **隔夜美股**（腾讯 usDJI/usIXIC：情绪 ±10，纳指跌 >1% 半导体类额外 −15，meta 与文档级 market 记录）+ **新闻深化**（Top20 候选拉当日新闻落库，关键词利好 +5/利空 −8 封顶 ±25）+ **技术指标扩充**（MACD 零上金叉 +5、KDJ 金叉 +10 / 超买 −10、BOLL 中轨上方 +5 / 突破上轨 +8、BIAS5 超涨 −8）+ **综合分 <60 不入选**（宁缺毋滥）；**标签级回测**——stats.tags 输出每个因子的 T+5 胜率，Swift 推荐卡展示标签胜率胶囊与隔夜纳指
- A 股池智能推荐（新增需求，服务端）：四层漏斗——东财涨幅榜 Top100 候选（push2delay，剔除 ST/退市/次新）→ 技术指标量化粗筛（MACD 金叉/多头、RSI 45–65、量比 ≥1.5、20 日箱体位置、均线多头；一字板与超买硬淘汰）→ 研报评级上调 +25/下调 −20、新闻热度加分 → AI 精排（`POST /picks/run` 客户端透传 AI 配置，密钥不落库，失败降级量化序）；`daily_pick` 按日幂等（DELETE+INSERT 刷新），收盘调度 15:30 后自动生成纯量化版并回写 T+1/T+5 收盘对照（`meta.outcome`），`GET /picks` 附近 30 天 T+5 胜率统计
- 收盘自动数据归档（ROI #12 M1）：报告调度器顺带把**最近已收盘交易日**（工作日 15:10 后为当日，盘中 / 周末 / 周一自动回补上一交易日）的自选股分时、收盘快照、当日信号、AI 用量与持仓 dump 成 `data/archives/{date}.json`，文件存在即跳过幂等；目录跟着数据库走（dev / 常驻安装各自隔离）

### Fixed

- 修复集成测试偶发 `_sqlx_migrations UNIQUE` 失败：临时 SQLite 路径加入线程 id，避免同进程并行测试在同一时钟粒度内撞库
