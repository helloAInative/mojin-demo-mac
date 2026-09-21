import Foundation
import Combine
import AppKit

@MainActor
final class MarketStore: ObservableObject {
    @Published var quote = Quote()
    @Published var minutes: [MinuteBar] = []
    @Published var days: [DayBar] = []
    @Published var macd: [MACD.Point] = []
    @Published var kdj: [KDJ.Point] = []
    @Published var rsi: [RSI.Point] = []
    @Published var status = "启动中…"
    @Published var liveOK = false
    @Published var usingCache = false
    @Published var sessionLabel = TradingSession.current().label
    @Published var menuFlash = false
    @Published var watchQuotes: [String: Quote] = [:]
    @Published var aiText = ""
    @Published var aiEvidence = ""
    @Published var aiStatus = "未分析"
    @Published var aiAnalyzing = false
    @Published var aiUpdatedAt: Date?
    @Published var aiUsedFallback = false
    @Published var aiSignal: AILatestSignal?
    @Published var quoteDivergence: Double = 0
    @Published var quoteVoteBadge = ""
    @Published var quoteSourceDetail = ""
    @Published var lastStrategyWhy = ""
    @Published var lastBacktest: BacktestResult?
    @Published var pendingUIAction: String? // reanalyze | diary | stratNote | open | openLevel | orderTicket | review
    /// 价位预警通知点击后载荷：{ levelKey, tier, price, code }
    @Published var pendingLevelPayload: [String: String] = [:]
    @Published var collapsedGroups: Set<String> = []
    @Published var signalEvents: [SignalEvent] = []
    @Published var selectedSignalID: UUID?

    /// AI 命中回测汇总：基于 SignalTimeline 中 kind=="level" 的事件。
    /// 命中率 = 已 hit / 总数；平均领先秒数；最大回撤（按当前价相对 levelPrice 距离）。
    var aiAccuracy: AIAccuracy {
        let levelEvents = signalEvents.filter { $0.kind == "level" }
        let total = levelEvents.count
        let hit = levelEvents.filter { $0.meta["hit"] == "1" }
        let leads = hit.compactMap { Int($0.meta["leadSec"] ?? "") }
        let avgLead: Int? = leads.isEmpty
            ? nil
            : Int(Double(leads.reduce(0, +)) / Double(leads.count))
        let hitRate: Double = total == 0 ? 0 : Double(hit.count) / Double(total)
        let pendingHits = levelEvents.filter { $0.meta["hit"] != "1" }
        // 最大回撤：当前未命中的事件中 levelPrice 距离现价最远的那一笔百分比
        let px = quote.price
        var maxDD: Double = 0
        for e in pendingHits {
            guard let s = e.meta["levelPrice"], let v = Double(s), v > 0, px > 0 else { continue }
            let dd = abs(px - v) / v * 100
            if dd > maxDD { maxDD = dd }
        }
        return AIAccuracy(
            total: total,
            hit: hit.count,
            hitRate: hitRate,
            avgLeadSec: avgLead,
            maxDrawdownPct: maxDD
        )
    }
    @Published var notifyQuotaLine = ""
    @Published var backtestBusy = false
    @Published var aiUsage: [AIUsageEntry] = []
    @Published var aiTodayFeel: Double = 0
    @Published var healthReport: HealthReport?
    @Published var healthProbing = false
    @Published var boardIndices: [IndexQuote] = []
    @Published var indexFailStreak = 0
    @Published var lastIndexProbes: [SourceProbe] = []
    @Published var selectedIndexCode: String?
    @Published var indexMinutes: [MinuteBar] = []
    @Published var indexMinuteBusy = false
    @Published var staleSymbolHints: [String] = []
    @Published var suggestedOrderLine: String = ""
    @Published var ticketSource: String = ""

    /// 智能止损 / 止盈建议（refreshDaily / 持仓变更时重算，不逐 tick）
    @Published var stopTakeAdvice: StopTakeAdvisor.Advice?

    /// 命中率时序（ROI #5）：近 30 个日历日按日聚合 level 事件 + 7 日滚动命中率。
    var accuracyTimeline: [AccuracyTimeline.Day] {
        AccuracyTimeline.build(events: signalEvents)
    }

    // §F.3–F.4：当前标的的新闻 / 研报 / 板块（走网关，失败只降级为提示）
    @Published var newsItems: [GatewayNewsItem] = []
    @Published var researchReports: [GatewayResearchReport] = []
    @Published var sectorBoards: [GatewaySectorBoard] = []
    @Published var codeInfoBusy = false
    @Published var codeInfoError: String?
    /// 已加载过的标的，切换自选时按此去重
    private var codeInfoLoadedCode: String?
    private var codeInfoTask: Task<Void, Never>?

    // 半自动委托条（国盛照抄，不报单）
    @Published var ticketSide: OrderSide = .buy
    @Published var ticketPrice: Double = 0
    @Published var ticketQty: Int = 100
    @Published var ticketPriceMode: OrderPriceMode = .last
    @Published var ticketNote: String = ""
    @Published var ticketHint: String = "不会向券商下单，仅生成照抄文本"
    @Published var ticketHistory: [SemiOrderTicket] = []
    @Published var lastCopiedTicketID: UUID?

    let settings: AppSettings

    private var quoteTask: Task<Void, Never>?
    private var quoteStreamTask: Task<Void, Never>?
    private var minuteTask: Task<Void, Never>?
    private var dailyTask: Task<Void, Never>?
    private var listTask: Task<Void, Never>?
    private var indexTask: Task<Void, Never>?
    private var retryTask: Task<Void, Never>?
    private var aiTask: Task<Void, Never>?
    /// 收盘复盘通知轮询（独立于行情 loop：行情 15:05 后降频停摆，复盘 15:05 后才生成）
    private var reviewNotifyTask: Task<Void, Never>?
    private var started = false
    private var quoteStreamConnected = false
    private var pendingRetry = 0

    private var lastPriceForLevel: Double?
    private var lastMacdCross: String?
    private var lastAboveFired = false
    private var lastBelowFired = false
    private var lastDrawdownFired = false
    private var lastDrawdownAt: Date?
    private var lastDivergenceWarn: Date?
    private var failStreak = 0
    private var lastAIAt: Date?
    private var lastAIPrice: Double?

    var base: Double { settings.levels.base }
    var support: Double { settings.levels.support }
    var resistance: Double { settings.levels.resistance }

    /// 关键价位预警档位（用于通知 + UI 双层闪烁）。
    enum LevelAlertTier: Equatable {
        case none       // 距离 > 一级阈值
        case light      // 距离 ∈ (二级阈值, 一级阈值]
        case deep       // 距离 ≤ 二级阈值

        /// distPct：相对距离（%）。
        /// lightPct / deepPct：来自 settings.notifyConfig。
        static func classify(distPct: Double,
                             lightPct: Double, deepPct: Double) -> LevelAlertTier {
            // deep 阈值需 ≤ light 阈值，否则全归 deep（防呆）
            let deep = min(lightPct, max(deepPct, 0))
            if distPct <= deep { return .deep }
            if distPct <= lightPct { return .light }
            return .none
        }
    }

    /// 当前每条价位线的预警档位（lazy 算，不存）。
    func levelAlert(base b: Double) -> LevelAlertTier {
        let p = quote.price
        guard p > 0, b > 0 else { return .none }
        let d = abs(p - b) / b * 100
        return LevelAlertTier.classify(distPct: d,
                                       lightPct: settings.notifyConfig.levelLightPct,
                                       deepPct: settings.notifyConfig.levelDeepPct)
    }
    func levelAlert(support s: Double) -> LevelAlertTier {
        let p = quote.price
        guard p > 0, s > 0 else { return .none }
        let d = abs(p - s) / s * 100
        return LevelAlertTier.classify(distPct: d,
                                       lightPct: settings.notifyConfig.levelLightPct,
                                       deepPct: settings.notifyConfig.levelDeepPct)
    }
    func levelAlert(resistance r: Double) -> LevelAlertTier {
        let p = quote.price
        guard p > 0, r > 0 else { return .none }
        let d = abs(p - r) / r * 100
        return LevelAlertTier.classify(distPct: d,
                                       lightPct: settings.notifyConfig.levelLightPct,
                                       deepPct: settings.notifyConfig.levelDeepPct)
    }

    /// 每条价位线的当前预警档位（用于决定是否发通知）。
    /// key: "base" | "support" | "resistance"
    private var lastLevelAlert: [String: LevelAlertTier] = [:]
    /// 持仓到本预警状态：当前是否在 costLightPct 内（防重复）
    private var lastPositionAlertState: Bool = false

    var lastMACD: MACD.Point? { macd.last(where: { $0.hist != nil }) }
    var lastKDJ: KDJ.Point? { kdj.last }
    var lastRSI: Double? { rsi.last(where: { $0.value != nil })?.value }
    var volRatio: Double? { VolumeSpike.ratio(days: days) }

    var prevMACD: MACD.Point? {
        let valid = macd.filter { $0.hist != nil }
        guard valid.count >= 2 else { return nil }
        return valid[valid.count - 2]
    }

    var signalText: (title: String, detail: String) {
        guard let cur = lastMACD, let dif = cur.dif, let dea = cur.dea, let hist = cur.hist else {
            return ("--", "等待日线…")
        }
        if let prev = prevMACD, let pd = prev.dif, let pe = prev.dea {
            if pd <= pe && dif > dea { return ("金叉", "DIF 上穿 DEA · 偏多留意") }
            if pd >= pe && dif < dea { return ("死叉", "DIF 下穿 DEA · 偏空留意") }
        }
        if dif > dea {
            return ("多头", "DIF 在 DEA 上方 · 柱\(hist >= 0 ? "放红" : "缩绿")")
        }
        return ("空头", "DIF 在 DEA 下方 · 柱\(hist >= 0 ? "缩红" : "放绿")")
    }

    var pnl: (amount: Double, pct: Double)? {
        let p = settings.position
        guard p.cost > 0, p.shares > 0, quote.price > 0 else { return nil }
        let amt = (quote.price - p.cost) * p.shares
        let pct = (quote.price - p.cost) / p.cost * 100
        return (amt, pct)
    }

    /// 现价相对支撑，正数=在支撑上方（还差几毛跌破）
    var toSupport: Double? {
        guard support > 0, quote.price > 0 else { return nil }
        return quote.price - support
    }

    init(settings: AppSettings) {
        self.settings = settings
    }

    private var lastComboFire: [UUID: Date] = [:]

    func start() {
        if started { return }
        started = true
        CrashLog.install()
        AlertService.requestPermission()
        signalEvents = SignalTimeline.load()
        aiUsage = AIUsageLedger.load()
        aiTodayFeel = AIUsageLedger.todayFeel()
        ticketHistory = OrderTicketLedger.load()
        refreshNotifyQuota()
        loadCache(for: settings.currentCode)
        syncTicketFromMarket(forcePrice: true)
        restartLoops()
        Task { await prefetchWatchlistDaily() }
        reviewNotifyTask = Task { [weak self] in
            await self?.pollReviewNotify()
        }
        Task { [weak self] in
            let previousCode = self?.settings.currentCode
            await self?.settings.syncGatewayData()
            if let self, previousCode != self.settings.currentCode {
                self.switchSymbol(self.settings.currentCode)
            }
            guard let remote = try? await GatewayMarketClient.signals() else { return }
            self?.signalEvents = SignalTimeline.mergeRemote(remote)
        }
    }

    func restartLoops() {
        quoteTask?.cancel()
        quoteStreamTask?.cancel()
        minuteTask?.cancel()
        dailyTask?.cancel()
        listTask?.cancel()
        indexTask?.cancel()
        retryTask?.cancel()
        lastPriceForLevel = nil
        lastMacdCross = nil
        lastAboveFired = false
        lastBelowFired = false
        lastDrawdownFired = false
        failStreak = 0
        pendingRetry = 0
        loadCache(for: settings.currentCode)

        quoteTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshQuote()
                self?.sessionLabel = TradingSession.current().label
                let interval = self?.quoteStreamConnected == true
                    ? max(TradingSession.current().quoteInterval, 15_000_000_000)
                    : TradingSession.current().quoteInterval
                try? await Task.sleep(nanoseconds: interval)
            }
        }
        quoteStreamTask = Task { [weak self] in
            while !Task.isCancelled {
                do {
                    let updates = try await GatewayMarketClient.quoteUpdates()
                    for try await update in updates {
                        guard !Task.isCancelled else { break }
                        self?.quoteStreamConnected = true
                        self?.applyPushedQuote(update)
                    }
                } catch {
                    self?.quoteStreamConnected = false
                }
                try? await Task.sleep(nanoseconds: 5_000_000_000)
            }
        }
        minuteTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshMinute()
                try? await Task.sleep(nanoseconds: TradingSession.current().minuteInterval)
            }
        }
        dailyTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshDaily()
                try? await Task.sleep(nanoseconds: TradingSession.current().dailyInterval)
            }
        }
        listTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshWatchlist()
                try? await Task.sleep(nanoseconds: 20_000_000_000)
            }
        }
        indexTask = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshBoardIndices()
                try? await Task.sleep(nanoseconds: 15_000_000_000)
            }
        }
        retryTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 8_000_000_000)
                guard let self, self.pendingRetry > 0 else { continue }
                await self.refreshQuote()
                await self.refreshMinute()
                await self.refreshDaily()
            }
        }
    }

    func stop() {
        started = false
        quoteTask?.cancel()
        quoteStreamTask?.cancel()
        minuteTask?.cancel()
        dailyTask?.cancel()
        listTask?.cancel()
        indexTask?.cancel()
        retryTask?.cancel()
    }

    func switchSymbol(_ code: String) {
        settings.selectSymbol(code)
        restartLoops()
        syncTicketFromMarket(forcePrice: true)
        ensureCodeInfo()
        // days/quote 还是旧标的的：先清掉，restartLoops→loadCache / refreshDaily 会重算
        stopTakeAdvice = nil
    }

    /// 用现价/持仓刷新委托条
    func syncTicketFromMarket(forcePrice: Bool = false) {
        let code = settings.currentCode
        if forcePrice || ticketPriceMode == .last || ticketPrice <= 0 {
            if quote.price > 0 {
                ticketPrice = quote.price
                ticketPriceMode = .last
            }
        }
        if ticketQty <= 0 {
            ticketQty = SemiOrderTicket.normalizeLot(100, code: code)
        }
        if ticketSide == .sell {
            let shares = Int(settings.position.shares)
            if shares > 0 {
                ticketQty = SemiOrderTicket.normalizeLot(shares, code: code)
            }
        }
    }

    func applyTicketPriceMode(_ mode: OrderPriceMode) {
        ticketPriceMode = mode
        guard quote.price > 0 else { return }
        switch mode {
        case .last:
            ticketPrice = quote.price
        case .limit:
            break
        case .bidAsk:
            // 无盘口时用现价轻微偏移作「对手价感」
            let raw = ticketSide == .buy ? quote.price * 1.001 : quote.price * 0.999
            ticketPrice = (raw * 100).rounded() / 100
        }
    }

    func setTicketSide(_ side: OrderSide) {
        ticketSide = side
        if side == .sell {
            let shares = Int(settings.position.shares)
            if shares > 0 {
                ticketQty = SemiOrderTicket.normalizeLot(shares, code: codeForTicket)
            }
        } else if ticketQty < 100 {
            ticketQty = SemiOrderTicket.normalizeLot(100, code: codeForTicket)
        }
    }

    private var codeForTicket: String { settings.currentCode }

    func buildCurrentTicket() -> SemiOrderTicket {
        let sym = settings.currentSymbol
        let name = quote.name.isEmpty ? sym.name : quote.name
        let qty = SemiOrderTicket.normalizeLot(ticketQty, code: sym.code)
        return SemiOrderTicket(
            id: UUID(),
            at: Date(),
            code: sym.code,
            name: name,
            side: ticketSide,
            price: ticketPrice,
            quantity: qty,
            priceMode: ticketPriceMode,
            note: ticketNote.trimmingCharacters(in: .whitespacesAndNewlines),
            copied: false,
            brokerHint: "国盛证券客户端",
            filled: false,
            source: ticketSource
        )
    }

    /// 到价上 / 到价下 / 止损 一键填入
    func fillTicketFromLevel(_ kind: String) {
        ticketPriceMode = .limit
        switch kind {
        case "above":
            let p = settings.strategy.above
            guard p > 0 else { ticketHint = "未设置到价上"; return }
            ticketSide = .sell
            ticketPrice = p
            ticketSource = "到价上"
            ticketNote = "到价上 \(String(format: "%.2f", p))"
        case "below":
            let p = settings.strategy.below
            guard p > 0 else { ticketHint = "未设置到价下"; return }
            ticketSide = .buy
            ticketPrice = p
            ticketSource = "到价下"
            ticketNote = "到价下 \(String(format: "%.2f", p))"
        case "stop":
            let p = settings.position.stopLoss
            guard p > 0 else { ticketHint = "未设置止损价"; return }
            ticketSide = .sell
            ticketPrice = p
            let shares = Int(settings.position.shares)
            if shares > 0 {
                ticketQty = SemiOrderTicket.normalizeLot(shares, code: settings.currentCode)
            }
            ticketSource = "止损"
            ticketNote = "止损 \(String(format: "%.2f", p))"
        case "take":
            let p = settings.position.takeProfit
            guard p > 0 else { ticketHint = "未设置止盈价"; return }
            ticketSide = .sell
            ticketPrice = p
            let shares = Int(settings.position.shares)
            if shares > 0 {
                ticketQty = SemiOrderTicket.normalizeLot(shares, code: settings.currentCode)
            }
            ticketSource = "止盈"
            ticketNote = "止盈 \(String(format: "%.2f", p))（可分批，先卖一半）"
        case "resist":
            let p = settings.levels.resistance
            guard p > 0 else { ticketHint = "未设置阻力"; return }
            ticketSide = .sell
            ticketPrice = p
            ticketSource = "阻力"
            ticketNote = "阻力位 \(String(format: "%.2f", p))"
        case "support":
            let p = settings.levels.support
            guard p > 0 else { ticketHint = "未设置支撑"; return }
            ticketSide = .buy
            ticketPrice = p
            ticketSource = "支撑"
            ticketNote = "支撑位 \(String(format: "%.2f", p))"
        default:
            break
        }
        ticketHint = "已填入 · \(ticketSource)"
    }

    /// 从策略为何触发生成草稿
    func draftTicketFromStrategy() {
        let why = lastStrategyWhy
        guard !why.isEmpty || !settings.comboStrategies.isEmpty else {
            ticketHint = "暂无策略命中说明"
            return
        }
        syncTicketFromMarket(forcePrice: true)
        ticketSource = "策略命中"
        ticketNote = why.isEmpty ? "策略建议委托" : String(why.prefix(80))
        if why.contains("死叉") || why.contains("跌") || why.contains("空") {
            ticketSide = .sell
        } else {
            ticketSide = .buy
        }
        suggestedOrderLine = buildCurrentTicket().oneLine
        ticketHint = "已从策略生成草稿 · \(suggestedOrderLine)"
    }

    /// 从 AI 结论文本粗解析方向
    func draftTicketFromAI() {
        let text = aiText
        guard !text.isEmpty else {
            ticketHint = "暂无 AI 结论"
            return
        }
        syncTicketFromMarket(forcePrice: true)
        ticketSource = "AI结论"
        let lower = text.lowercased()
        if text.contains("卖出") || text.contains("减仓") || text.contains("止损") || lower.contains("sell") {
            ticketSide = .sell
        } else if text.contains("买入") || text.contains("加仓") || text.contains("建仓") || lower.contains("buy") {
            ticketSide = .buy
        }
        ticketNote = "AI：" + String(text.prefix(60)).replacingOccurrences(of: "\n", with: " ")
        suggestedOrderLine = buildCurrentTicket().oneLine
        ticketHint = "已从 AI 生成草稿 · \(suggestedOrderLine)"
    }

    /// 策略命中后推建议委托（与通知共用冷静/配额键）
    func pushSuggestedOrder(rule: ComboStrategy, code: String, snap: StrategyEngine.Snapshot) {
        let notifyOK = NotifyGovernor.shared.canFire(id: "combo-\(String(rule.id.uuidString.prefix(8)))")
        // 无论通知是否发出，本地都生成建议
        if code != settings.currentCode {
            settings.selectSymbol(code)
        }
        syncTicketFromMarket(forcePrice: true)
        ticketSource = "策略·\(rule.name)"
        ticketNote = StrategyEngine.whyTriggered(rule, snap: snap)
        let bearish = ticketNote.contains("死叉") || ticketNote.contains("空头") || snap.pct < -2
        ticketSide = bearish ? .sell : .buy
        if ticketSide == .sell, settings.position.shares > 0 {
            ticketQty = SemiOrderTicket.normalizeLot(Int(settings.position.shares), code: code)
        }
        suggestedOrderLine = buildCurrentTicket().oneLine
        if !notifyOK {
            ticketHint = "建议委托已就绪（通知冷却中）· \(suggestedOrderLine)"
        } else {
            ticketHint = "建议委托 · \(suggestedOrderLine)"
        }
    }

    @discardableResult
    func copyTicketForBroker() -> SemiOrderTicket? {
        guard ticketPrice > 0, ticketQty > 0 else {
            ticketHint = "请填写有效价格和数量"
            return nil
        }
        var t = buildCurrentTicket()
        t.copied = true
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(t.clipboardText, forType: .string)
        OrderTicketLedger.append(t)
        ticketHistory = OrderTicketLedger.load()
        lastCopiedTicketID = t.id
        ticketHint = "已复制 · 请到国盛 App/同花顺确认下单"
        logSignal(
            kind: "order",
            title: "半自动委托 · \(t.side.rawValue)",
            body: t.oneLine,
            evidence: t.clipboardText,
            why: "仅剪贴板照抄，未向券商报单 · 来源\(t.source.isEmpty ? "手动" : t.source)"
        )
        return t
    }

    /// 勾选已在国盛成交 → 回写持仓
    func markTicketFilled(_ id: UUID) {
        guard var t = ticketHistory.first(where: { $0.id == id }) else { return }
        guard !t.filled else { return }
        t.filled = true
        OrderTicketLedger.update(t)
        ticketHistory = OrderTicketLedger.load()
        if t.code != settings.currentCode {
            settings.selectSymbol(t.code)
        }
        settings.applyFillToPosition(side: t.side, price: t.price, quantity: t.quantity, code: t.code)
        let day = Self.todayString()
        settings.appendDiaryNote(day: day, note: "国盛成交 \(t.oneLine)", code: t.code)
        ticketHint = "已回写持仓 · \(t.oneLine)"
        logSignal(
            kind: "order",
            title: "国盛成交勾选",
            body: t.oneLine,
            why: "手动确认已在国盛成交，回写成本/股数"
        )
    }

    func reloadTicketHistory() {
        ticketHistory = OrderTicketLedger.load()
    }

    func clearTicketHistory() {
        OrderTicketLedger.clear()
        ticketHistory = []
        ticketHint = "历史已清空"
    }

    func tradeStoryText(day: String? = nil) -> String {
        let d = day ?? Self.todayString()
        return TradeStoryDesk.build(
            day: d,
            code: settings.currentCode,
            name: quote.name.isEmpty ? settings.currentSymbol.name : quote.name,
            signals: signalEvents,
            tickets: ticketHistory,
            diary: settings.diaryText(day: d)
        )
    }

    /// 阶段 4：调后端生成复盘 / 周报。
    /// - `kind=daily` 按当日 date 幂等；`kind=weekly` 按 ISO 周幂等。
    /// - 客户端把当日的日记 + 委托 + 信号打包到 `payload.context`，
    ///   后端再补上"AI 调用 / level 命中 / 后端信号时间线"。
    /// - 返回的报告已经写入数据库，前端只需展示。
    func submitReview(
        kind: ReviewKind,
        day: String? = nil,
        completion: ((Result<ScheduledReport, Error>) -> Void)? = nil
    ) {
        let d = day ?? Self.todayString()
        let diary = settings.diaryText(day: d)
        let ctx = ReviewContextPayload.from(
            diary: diary,
            tickets: ticketHistory,
            signals: signalEvents
        )
        guard let url = URL(string: settings.marketServerURL) else {
            completion?(.failure(ReportClientError.network(message: "marketServerURL 非法：\(settings.marketServerURL)")))
            return
        }
        let client = ReportClient(baseURL: url)
        status = "正在生成\(kind == .weekly ? "周报" : "日报")…"
        Task { [weak self] in
            do {
                let report = try await client.run(kind: kind, context: ctx)
                await MainActor.run {
                    self?.status = "复盘已生成 · \(report.title)"
                    completion?(.success(report))
                }
            } catch {
                await MainActor.run {
                    self?.status = "复盘生成失败：\(error.localizedDescription)"
                    completion?(.failure(error))
                }
            }
        }
    }

    /// 拉取后端已落库的复盘历史。
    func loadReviewHistory(kind: ReviewKind? = nil, limit: Int = 7) async -> [ScheduledReport] {
        guard let url = URL(string: settings.marketServerURL) else { return [] }
        let client = ReportClient(baseURL: url)
        do {
            return try await client.list(kind: kind, limit: limit)
        } catch {
            status = "拉取复盘历史失败：\(error.localizedDescription)"
            return []
        }
    }

    /// §F.3–F.4：当前标的切换或首次进入盯盘页时加载资讯；同标的只拉一次。
    func ensureCodeInfo() {
        let code = settings.currentCode
        guard codeInfoLoadedCode != code else { return }
        codeInfoTask?.cancel()
        codeInfoLoadedCode = code
        newsItems = []
        researchReports = []
        sectorBoards = []
        codeInfoError = nil
        codeInfoTask = Task { [weak self] in
            await self?.loadCodeInfo(code: code)
        }
    }

    /// 强制刷新当前标的的资讯（按钮触发，即使同标的）。
    func reloadCodeInfo() {
        codeInfoLoadedCode = nil
        ensureCodeInfo()
    }

    private func loadCodeInfo(code: String) async {
        codeInfoBusy = true
        defer { codeInfoBusy = false }
        do {
            async let news = GatewayMarketClient.news(code: code, limit: 6, hours: 72)
            async let reports = GatewayMarketClient.reports(code: code, limit: 5, days: 180)
            async let boards = GatewayMarketClient.sector(code: code)
            let (n, r, b) = try await (news, reports, boards)
            guard settings.currentCode == code, !Task.isCancelled else { return }
            newsItems = n
            researchReports = r
            sectorBoards = b
            codeInfoError = nil
        } catch {
            guard settings.currentCode == code, !Task.isCancelled else { return }
            // 网关未启用 / 不可达时只降级为提示，不刷时间线、不发通知
            codeInfoError = Self.describeGatewayError(error)
        }
    }

    private static func describeGatewayError(_ error: Error) -> String {
        if case GatewayMarketClient.GatewayError.disabled = error { return "网关未启用" }
        if case GatewayMarketClient.GatewayError.invalidURL = error { return "网关地址非法" }
        return "拉取失败：\(error.localizedDescription)"
    }

    func selectBoardIndex(_ code: String) {
        if selectedIndexCode == code {
            selectedIndexCode = nil
            indexMinutes = []
            return
        }
        selectedIndexCode = code
        Task { await refreshIndexMinutes() }
    }

    func refreshIndexMinutes() async {
        guard let code = selectedIndexCode else { return }
        indexMinuteBusy = true
        defer { indexMinuteBusy = false }
        let name = boardIndices.first(where: { $0.code == code })?.quote.name ?? code
        let sym = WatchSymbol(code: code, name: name)
        do {
            indexMinutes = try await MarketService.fetchMinute(symbol: sym)
            SnapshotCache.saveDays(code: "idx-\(code)", days: [], quote: boardIndices.first(where: { $0.code == code })?.quote ?? Quote(), minutes: indexMinutes)
        } catch {
            CrashLog.append("index minute \(code): \(error)")
            if let cached = SnapshotCache.load(code: "idx-\(code)") {
                indexMinutes = cached.minutes
            }
        }
    }

    /// 个股与大盘同向角标
    var boardAlignBadge: String {
        guard quote.price > 0, !boardIndices.isEmpty else { return "" }
        let stockUp = quote.pct > 0.05
        let stockDown = quote.pct < -0.05
        guard stockUp || stockDown else { return "震荡" }
        let ups = boardIndices.filter { $0.quote.pct > 0.05 }.count
        let downs = boardIndices.filter { $0.quote.pct < -0.05 }.count
        if stockUp && ups >= downs { return "同向偏多" }
        if stockDown && downs >= ups { return "同向偏空" }
        return "背离"
    }

    func exportMinutesCSV() -> URL? {
        let header = "time,price,avg,vol\n"
        let rows = minutes.map {
            "\($0.minute),\($0.price),\($0.avg),\($0.vol)"
        }.joined(separator: "\n")
        let day = Self.todayString()
        let name = "\(settings.currentSymbol.bareCode)-\(day)-minute.csv"
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(name)
        try? (header + rows).write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    private func loadCache(for code: String) {
        guard let snap = SnapshotCache.load(code: code) else { return }
        if quote.price == 0 { quote = snap.quote }
        if minutes.isEmpty { minutes = snap.minutes }
        if days.isEmpty {
            days = snap.days
            recomputeIndicators()
            refreshStopTakeAdvice()
        }
        usingCache = quote.price > 0
        if usingCache && !liveOK {
            status = "缓存 · 弱网"
        }
    }

    private func persist() {
        guard quote.price > 0 || !minutes.isEmpty else { return }
        SnapshotCache.save(code: settings.currentCode, quote: quote, minutes: minutes, days: days)
    }

    private func recomputeIndicators() {
        let closes = days.map(\.close)
        macd = MACD.compute(closes: closes)
        kdj = KDJ.compute(days: days)
        rsi = RSI.compute(closes: closes)
    }

    private func refreshQuote() async {
        let symbol = settings.currentSymbol
        let voted = await MarketService.fetchQuoteVoted(symbol: symbol)
        let q = voted.quote
        guard q.price > 0 else {
            failStreak += 1
            pendingRetry = min(pendingRetry + 1, 8)
            liveOK = false
            loadCache(for: symbol.code)
            if quote.price > 0 {
                usingCache = true
                status = "缓存 · 重试#\(pendingRetry)"
            } else {
                status = failStreak >= 2 ? "报价失败 · 排队重试" : "切换备用源…"
            }
            return
        }
        if !q.name.isEmpty, let idx = settings.symbols.firstIndex(where: { $0.code == symbol.code }) {
            if settings.symbols[idx].name != q.name {
                var list = settings.symbols
                list[idx].name = q.name
                settings.symbols = list
                settings.save()
            }
        }
        quoteDivergence = voted.divergencePct
        quoteVoteBadge = voted.badge
        quoteSourceDetail = voted.sources
            .map { "\($0.source)=\(String(format: "%.2f", $0.price))" }
            .joined(separator: " ")
        if voted.divergent {
            let now = Date()
            if lastDivergenceWarn == nil || now.timeIntervalSince(lastDivergenceWarn!) > 600 {
                lastDivergenceWarn = now
                flashAndNotify(
                    title: "报价分歧 \(String(format: "%.2f", voted.divergencePct))%",
                    body: "[\(symbol.code)] 主源\(voted.primarySource) \(String(format: "%.2f", q.price)) · \(quoteSourceDetail)",
                    id: "diverge",
                    kind: "diverge",
                    why: "多源价差相对主源 ≥0.25%，现价仍用主源，未取中位数。"
                )
            }
        }
        evaluateAlerts(old: lastPriceForLevel, new: q)
        evaluateComboStrategies()
        lastPriceForLevel = q.price
        quote = q
        watchQuotes[symbol.code] = q
        liveOK = true
        usingCache = false
        failStreak = 0
        pendingRetry = 0
        status = "\(sessionLabel) · \(voted.badge)"
        patchTodayClose(q.price)
        persist()
        maybeSessionBriefs(q)
        maybeAutoAIAnalyze()
        checkLevelAlerts()
        checkLevelHits()
        checkPositionAlert()
        refreshNotifyQuota()
    }

    private func applyPushedQuote(_ update: GatewayQuoteUpdate) {
        let q = update.quote
        guard q.price > 0 else { return }
        watchQuotes[update.code] = q
        guard update.code == settings.currentCode else { return }
        if !q.name.isEmpty, let index = settings.symbols.firstIndex(where: { $0.code == update.code }),
           settings.symbols[index].name != q.name {
            settings.symbols[index].name = q.name
            settings.save()
        }
        evaluateAlerts(old: lastPriceForLevel, new: q)
        lastPriceForLevel = q.price
        quote = q
        evaluateComboStrategies()
        liveOK = true
        usingCache = false
        failStreak = 0
        pendingRetry = 0
        status = "\(sessionLabel) · WebSocket · \(q.source)"
        patchTodayClose(q.price)
        persist()
        maybeSessionBriefs(q)
        maybeAutoAIAnalyze()
        checkLevelAlerts()
        checkLevelHits()
        checkPositionAlert()
        refreshNotifyQuota()
    }

    private func refreshMinute() async {
        do {
            minutes = try await MarketService.fetchMinute(symbol: settings.currentSymbol)
            persist()
        } catch {
            pendingRetry += 1
            loadCache(for: settings.currentCode)
        }
    }

    private func refreshDaily() async {
        do {
            var d = try await MarketService.fetchDaily(symbol: settings.currentSymbol)
            if quote.price > 0 {
                let ds = Self.todayString()
                if let last = d.last, last.date == ds {
                    d[d.count - 1].close = quote.price
                    if quote.high > 0 { d[d.count - 1].high = max(d[d.count - 1].high, quote.high) }
                    if quote.low > 0 { d[d.count - 1].low = min(d[d.count - 1].low == 0 ? quote.low : d[d.count - 1].low, quote.low) }
                } else if let last = d.last, last.date < ds {
                    d.append(DayBar(date: ds, open: quote.open, close: quote.price, high: quote.high, low: quote.low, volume: 0))
                }
            }
            days = d
            recomputeIndicators()
            refreshStopTakeAdvice()
            evaluateMacdAlerts()
            evaluateComboStrategies()
            persist()
        } catch {
            pendingRetry += 1
            CrashLog.append("daily fail: \(error)")
            loadCache(for: settings.currentCode)
        }
    }

    func refreshWatchlistNow() async {
        await refreshWatchlist()
    }

    func refreshBoardIndices() async {
        let result = await MarketService.fetchBoardIndices(
            includeHS300: settings.boardShowHS300,
            includeBJ50: settings.boardShowBJ50
        )
        lastIndexProbes = result.fails
        if result.ok.isEmpty {
            indexFailStreak += 1
        } else {
            indexFailStreak = 0
            boardIndices = result.ok
            // 缓存年龄：写一份合成快照
            if let first = result.ok.first {
                SnapshotCache.save(
                    code: "board-meta",
                    quote: first.quote,
                    minutes: [],
                    days: []
                )
            }
        }
        if let sel = selectedIndexCode, !boardIndices.contains(where: { $0.code == sel }) {
            selectedIndexCode = nil
            indexMinutes = []
        }
    }

    private func refreshWatchlist() async {
        let others = settings.symbols.filter { $0.code != settings.currentCode }
        let batch = await MarketService.fetchQuotesBatch(symbols: others, concurrency: 3)
        for (code, q) in batch {
            watchQuotes[code] = q
            if !q.name.isEmpty, let idx = settings.symbols.firstIndex(where: { $0.code == code }) {
                if settings.symbols[idx].name != q.name {
                    var list = settings.symbols
                    list[idx].name = q.name
                    settings.symbols = list
                    settings.save()
                }
            }
        }
        var stale: [String] = []
        for s in settings.symbols where s.code != settings.currentCode {
            if batch[s.code] == nil {
                stale.append(s.code)
            }
        }
        staleSymbolHints = stale
        watchQuotes[settings.currentCode] = quote
    }

    private func patchTodayClose(_ price: Double) {
        guard !days.isEmpty else { return }
        let ds = Self.todayString()
        if days[days.count - 1].date == ds {
            days[days.count - 1].close = price
            recomputeIndicators()
            evaluateMacdAlerts()
        }
    }

    private func evaluateAlerts(old: Double?, new: Quote) {
        let name = new.name.isEmpty ? settings.currentSymbol.name : new.name
        evaluateLevelAlerts(old: old, new: new.price, name: name)
        evaluatePriceTargets(old: old, new: new.price, name: name)
        evaluatePositionStops(old: old, new: new.price, name: name)
        evaluateDrawdown(new, name: name)
    }

    private func evaluateLevelAlerts(old: Double?, new: Double, name: String) {
        guard settings.alertsEnabled, let old, old > 0, new > 0 else { return }
        let lv = settings.levels
        if lv.support > 0, old >= lv.support, new < lv.support {
            flashAndNotify(title: "跌破支撑", body: "[\(settings.currentCode)] \(name) \(String(format: "%.2f", new)) < 支撑 \(String(format: "%.2f", lv.support))", id: "support")
        }
        if lv.resistance > 0, old <= lv.resistance, new > lv.resistance {
            flashAndNotify(title: "突破阻力", body: "[\(settings.currentCode)] \(name) \(String(format: "%.2f", new)) > 阻力 \(String(format: "%.2f", lv.resistance))", id: "resist")
        }
    }

    private func evaluatePriceTargets(old: Double?, new: Double, name: String) {
        guard settings.alertsEnabled, let old, old > 0 else { return }
        let st = settings.strategy
        let pos = settings.position
        // 止损/止盈已联动写入到价下/上：同价时跳过通用到价提醒，由 posStop / posTake 专用提醒覆盖
        if st.above > 0, old < st.above, new >= st.above, !lastAboveFired,
           !(pos.takeProfit > 0 && abs(st.above - pos.takeProfit) < 0.005) {
            lastAboveFired = true
            flashAndNotify(title: "到价(上)", body: "\(name) \(String(format: "%.2f", new)) ≥ \(String(format: "%.2f", st.above))", id: "above")
        }
        if st.below > 0, old > st.below, new <= st.below, !lastBelowFired,
           !(pos.stopLoss > 0 && abs(st.below - pos.stopLoss) < 0.005) {
            lastBelowFired = true
            flashAndNotify(title: "到价(下)", body: "\(name) \(String(format: "%.2f", new)) ≤ \(String(format: "%.2f", st.below))", id: "below")
        }
    }

    /// 跌破止损 / 触达止盈（持仓专属提醒，点击通知跳止损/止盈委托草稿）。
    private func evaluatePositionStops(old: Double?, new: Double, name: String) {
        guard settings.alertsEnabled, let old, old > 0, new > 0 else { return }
        let pos = settings.position
        guard pos.shares > 0 else { return }
        if pos.stopLoss > 0, old > pos.stopLoss, new <= pos.stopLoss {
            flashAndNotify(
                title: "跌破止损",
                body: "\(name) \(String(format: "%.2f", new)) ≤ 止损 \(String(format: "%.2f", pos.stopLoss)) · 持仓 \(Int(pos.shares)) 股",
                id: "pos-stop",
                kind: "stopTake",
                why: "价格跌破持仓止损位，按纪律执行或复核锚点（波动 / 结构 / 成本）",
                userInfo: ["side": "stop"]
            )
        }
        if pos.takeProfit > 0, old < pos.takeProfit, new >= pos.takeProfit {
            flashAndNotify(
                title: "达到止盈",
                body: "\(name) \(String(format: "%.2f", new)) ≥ 止盈 \(String(format: "%.2f", pos.takeProfit)) · 可分批止盈",
                id: "pos-take",
                kind: "stopTake",
                why: "价格触及持仓止盈位，按计划分批卖出或上移止盈",
                userInfo: ["side": "take"]
            )
        }
    }

    private func evaluateDrawdown(_ q: Quote, name: String) {
        guard settings.alertsEnabled else { return }
        let st = settings.strategy
        let pct = st.drawdownPct
        guard pct > 0, q.high > 0, q.price > 0 else { return }
        let dd = (q.high - q.price) / q.high * 100
        let cool = Double(max(5, st.coolDownMin)) * 60
        if dd >= pct {
            let cooled = lastDrawdownAt.map { Date().timeIntervalSince($0) >= cool } ?? true
            if !lastDrawdownFired || cooled {
                lastDrawdownFired = true
                lastDrawdownAt = Date()
                flashAndNotify(
                    title: "回撤 \(String(format: "%.1f", dd))%",
                    body: "\(name) 自高 \(String(format: "%.2f", q.high)) 回落至 \(String(format: "%.2f", q.price)) · 冷静\(st.coolDownMin)分",
                    id: "dd"
                )
            }
        }
        if dd < pct * 0.5 { lastDrawdownFired = false }
    }

    private func evaluateMacdAlerts() {
        guard settings.alertsEnabled else { return }
        let sig = signalText.title
        guard sig == "金叉" || sig == "死叉" else { return }
        if lastMacdCross == sig { return }
        lastMacdCross = sig
        let name = quote.name.isEmpty ? settings.currentSymbol.name : quote.name
        flashAndNotify(title: "MACD \(sig)", body: "\(name) · \(signalText.detail)", id: "macd-\(sig)")
    }

    private func maybeSessionBriefs(_ q: Quote) {
        guard settings.openCloseBrief, settings.alertsEnabled else { return }
        let day = Self.todayString()
        let mins = TradingSession.shanghaiMinutes()
        let name = q.name.isEmpty ? settings.currentSymbol.name : q.name
        let body = String(format: "%@  %.2f  %@%.2f%%  开%.2f 高%.2f 低%.2f",
                          name, q.price, q.pct >= 0 ? "+" : "", q.pct, q.open, q.high, q.low)
        if mins >= 9 * 60 + 25 && mins <= 9 * 60 + 40, settings.lastOpenBriefDay != day {
            settings.lastOpenBriefDay = day
            settings.save()
            flashAndNotify(title: "开盘快报", body: body, id: "open-brief")
        }
        if mins >= 15 * 60 && mins <= 15 * 60 + 10, settings.lastCloseBriefDay != day {
            settings.lastCloseBriefDay = day
            settings.save()
            flashAndNotify(title: "收盘快报", body: body, id: "close-brief")
        }
    }

    /// 收盘复盘通知：工作日 15:05 后盯服务端日报生成（60s 心跳，生成即通知一次）。
    /// 行情 loop 收盘后停摆，所以独立成任务；点击通知跳复盘页。
    private func pollReviewNotify() async {
        while !Task.isCancelled {
            var sleepSec: UInt64 = 600_000_000_000
            if settings.marketGatewayEnabled, settings.alertsEnabled,
               TradingSession.isShanghaiWeekday(),
               TradingSession.shanghaiMinutes() >= 15 * 60 + 5 {
                await checkDailyReviewReady()
                // 15:05–16:05 是服务端刚生成的窗口，盯紧一点；之后每 10 分钟兜底（覆盖晚间才开 App）
                sleepSec = TradingSession.shanghaiMinutes() <= 16 * 60 + 5
                    ? 60_000_000_000 : 600_000_000_000
            }
            try? await Task.sleep(nanoseconds: sleepSec)
        }
    }

    private func checkDailyReviewReady() async {
        let today = Self.todayString()
        guard settings.lastReviewNotifyDay != today else { return }
        guard let url = URL(string: settings.marketServerURL) else { return }
        guard let latest = try? await ReportClient(baseURL: url).list(kind: .daily, limit: 1).first
        else { return }
        guard latest.periodKey == today else { return }
        settings.lastReviewNotifyDay = today
        settings.save()
        flashAndNotify(
            title: "收盘复盘已生成",
            body: "\(latest.title) · 点击查看复盘",
            id: "review-ready",
            kind: "review",
            why: "服务端收盘后自动生成当日日报，已落库"
        )
    }

    // ============================================================================
    // 智能止损 / 止盈（ROI #2）
    // ============================================================================

    /// 重算建议（挂 refreshDaily / loadCache / 持仓变更，纯数学 O(20)，不做逐 tick）。
    func refreshStopTakeAdvice() {
        guard quote.price > 0, days.count >= 2 else {
            stopTakeAdvice = nil
            return
        }
        stopTakeAdvice = StopTakeAdvisor.advise(
            days: days,
            price: quote.price,
            cost: settings.position.cost,
            resistance: settings.levels.resistance,
            drawdownPct: settings.strategy.drawdownPct
        )
    }

    /// 一键应用建议到持仓：写本地 + 网关 PUT（联动到价上/下），并落一条时间线记录供复盘对照。
    func applyStopTakeAdvice() {
        guard let advice = stopTakeAdvice, advice.stop > 0 || advice.take > 0 else { return }
        let pos = settings.position
        settings.updatePosition(
            cost: pos.cost, shares: pos.shares,
            stopLoss: advice.stop, takeProfit: advice.take,
            positionPct: pos.positionPct
        )
        logSignal(
            kind: "stopTake",
            title: "应用智能止损/止盈",
            body: String(format: "止损 %.2f · 止盈 %.2f · 盈亏比 %.2f",
                         advice.stop, advice.take, advice.ratio),
            why: (advice.stopAnchors + advice.takeAnchors).joined(separator: "；")
        )
    }

    private func evaluateComboStrategies() {
        guard settings.alertsEnabled else { return }
        let now = Date()
        for rule in settings.comboStrategies where rule.enabled {
            let targets = rule.targetCodes(current: settings.currentCode, symbols: settings.symbols)
            for code in targets {
                guard let snap = snapshot(for: code) else { continue }
                guard StrategyEngine.evaluate(rule, snap: snap) else { continue }
                let fireKey = rule.id
                // 策略本地冷静与通知配额共用：先看通知能否发，再叠加 10 分钟本地锁
                let notifyId = "combo-\(String(rule.id.uuidString.prefix(8)))"
                if let last = lastComboFire[fireKey], now.timeIntervalSince(last) < 600 { continue }
                lastComboFire[fireKey] = now
                let name = settings.symbols.first(where: { $0.code == code })?.name ?? code
                pushSuggestedOrder(rule: rule, code: code, snap: snap)
                let suggest = suggestedOrderLine
                flashAndNotify(
                    title: "策略 · \(rule.name)",
                    body: "[\(code)] \(name) 触发 · 建议委托 \(suggest)",
                    id: notifyId,
                    code: code,
                    kind: "strategy",
                    why: StrategyEngine.whyTriggered(rule, snap: snap)
                )
                if code == settings.currentCode {
                    lastStrategyWhy = StrategyEngine.whyTriggered(rule, snap: snap)
                }
                break // 一条策略本轮只报一只，避免刷屏
            }
        }
    }

    private func snapshot(for code: String) -> StrategyEngine.Snapshot? {
        if code == settings.currentCode, quote.price > 0 {
            return StrategyEngine.Snapshot(
                price: quote.price, pct: quote.pct, volRatio: volRatio,
                rsi: lastRSI, macdTitle: signalText.title, kdjJ: lastKDJ?.j
            )
        }
        guard let q = watchQuotes[code], q.price > 0 else { return nil }
        var rsiV: Double?
        var jV: Double?
        var macdTitle = "--"
        var volR: Double?
        if let cached = SnapshotCache.load(code: code), !cached.days.isEmpty {
            var d = cached.days
            if d[d.count - 1].date == Self.todayString() {
                d[d.count - 1].close = q.price
            }
            let closes = d.map(\.close)
            let macd = MACD.compute(closes: closes)
            let kdj = KDJ.compute(days: d)
            let rsi = RSI.compute(closes: closes)
            rsiV = rsi.last(where: { $0.value != nil })?.value
            jV = kdj.last?.j
            if macd.count >= 2, let a = macd[macd.count - 2].dif, let b = macd[macd.count - 2].dea,
               let c = macd[macd.count - 1].dif, let e = macd[macd.count - 1].dea {
                if a <= b && c > e { macdTitle = "金叉" }
                else if a >= b && c < e { macdTitle = "死叉" }
                else if c > e { macdTitle = "多头" }
                else { macdTitle = "空头" }
            }
            volR = VolumeSpike.ratio(days: d)
        }
        return StrategyEngine.Snapshot(
            price: q.price, pct: q.pct, volRatio: volR,
            rsi: rsiV, macdTitle: macdTitle, kdjJ: jV
        )
    }

    func runBacktest(_ rule: ComboStrategy) -> BacktestResult {
        let r = StrategyEngine.backtest(rule: rule, days: days, closes: days.map(\.close))
        lastBacktest = r
        return r
    }

    /// 按作用域拉取各股日线缓存后回测（真正多标的）
    func runBacktestScoped(_ rule: ComboStrategy) async -> BacktestResult {
        backtestBusy = true
        defer { backtestBusy = false }
        let codes = rule.targetCodes(current: settings.currentCode, symbols: settings.symbols)
        var labeled: [(code: String, result: BacktestResult)] = []
        for code in codes {
            if Task.isCancelled { break }
            let d = await ensureDailyBars(code: code)
            let r = StrategyEngine.backtest(rule: rule, days: d, closes: d.map(\.close))
            labeled.append((code, r))
        }
        let merged = codes.count <= 1
            ? (labeled.first?.result ?? runBacktest(rule))
            : BacktestResult.merge(labeled: labeled)
        lastBacktest = merged
        return merged
    }

    func ensureDailyBars(code: String) async -> [DayBar] {
        if code == settings.currentCode, days.count >= 40 {
            SnapshotCache.saveDays(code: code, days: days, quote: quote, minutes: minutes)
            return days
        }
        if let cached = SnapshotCache.load(code: code)?.days, cached.count >= 40 {
            // 缓存超过 36h 则刷新
            if let age = SnapshotCache.cacheAgeSec(code: code), age < 36 * 3600 {
                return cached
            }
        }
        guard let sym = settings.symbols.first(where: { $0.code == code })
                ?? (code == settings.currentCode ? settings.currentSymbol : nil) else {
            return SnapshotCache.load(code: code)?.days ?? []
        }
        do {
            let d = try await MarketService.fetchDaily(symbol: sym)
            let q = watchQuotes[code] ?? (code == settings.currentCode ? quote : Quote())
            SnapshotCache.saveDays(code: code, days: d, quote: q)
            return d
        } catch {
            CrashLog.append("daily cache \(code): \(error)")
            return SnapshotCache.load(code: code)?.days ?? []
        }
    }

    func prefetchWatchlistDaily() async {
        for s in settings.symbols {
            if Task.isCancelled { return }
            _ = await ensureDailyBars(code: s.code)
        }
    }

    func exportBacktestCSV(ruleName: String) -> URL? {
        guard let r = lastBacktest else { return nil }
        let name = "backtest-\(ruleName.replacingOccurrences(of: "/", with: "-"))-\(Self.todayString()).csv"
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(name)
        try? r.csvString(ruleName: ruleName).write(to: url, atomically: true, encoding: .utf8)
        return url
    }

    func currentStrategySnapshot() -> StrategyEngine.Snapshot {
        StrategyEngine.Snapshot(
            price: quote.price, pct: quote.pct, volRatio: volRatio,
            rsi: lastRSI, macdTitle: signalText.title, kdjJ: lastKDJ?.j
        )
    }

    func analyzeWithAI(force: Bool = true) {
        let cfg = settings.aiConfig
        guard cfg.enabled else {
            aiStatus = "请先在设置中开启 AI"
            return
        }
        guard cfg.providerId == "ollama"
                || !settings.aiAPIKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            aiStatus = "请先配置 API Token"
            return
        }
        guard quote.price > 0 else {
            aiStatus = "等待行情…"
            return
        }
        guard TokenPlanCatalog.isChat(cfg.model) else {
            aiStatus = "当前模型是\(TokenPlanCatalog.all.first(where: { $0.modelID == cfg.model })?.capability ?? "生成类")，请换对话模型再分析"
            return
        }
        if aiAnalyzing { return }
        if !force, let last = lastAIAt {
            let gap = Date().timeIntervalSince(last)
            if gap < Double(max(30, cfg.intervalSec)) { return }
        }
        let evidence = buildEvidenceSnapshot()
        aiEvidence = evidence
        aiAnalyzing = true
        aiUsedFallback = false
        aiStatus = "分析中…"
        let prompt = buildAIPrompt(evidence: evidence)
        let key = settings.aiAPIKey
        let model = cfg.model
        let t0 = Date()
        aiTask?.cancel()
        aiTask = Task { [weak self] in
            do {
                let provider = ProviderRegistry.resolve(
                    id: cfg.providerId,
                    config: cfg,
                    apiKey: key
                )
                let text = try await provider.chat(
                    system: AIService.systemPrompt, user: prompt
                )
                guard !Task.isCancelled else { return }
                let ms = Int(Date().timeIntervalSince(t0) * 1000)
                await MainActor.run {
                    guard let self else { return }
                    let parsed = Self.parseAISignal(from: text)
                    let formatted = Self.formatAIText(parsed: parsed, raw: text)
                    self.aiText = formatted
                    self.aiUpdatedAt = Date()
                    self.lastAIAt = Date()
                    self.lastAIPrice = self.quote.price
                    self.aiAnalyzing = false
                    self.aiUsedFallback = false
                    if let sig = parsed.signal {
                        self.aiSignal = sig
                        // 把 AI 给出的价位合并进当前 symbol 的关键价位
                        // （用户可手动在设置里覆盖，仅在 AI 给值且当前无值时落库，避免误清空）。
                        let cur = self.settings.levels
                        let nb = sig.base       > 0 ? sig.base       : cur.base
                        let ns = sig.support    > 0 ? sig.support    : cur.support
                        let nr = sig.resistance > 0 ? sig.resistance : cur.resistance
                        if abs(nb - cur.base) > 1e-6
                            || abs(ns - cur.support) > 1e-6
                            || abs(nr - cur.resistance) > 1e-6 {
                            self.settings.updateLevels(base: nb, support: ns, resistance: nr)
                        }
                    }
                    var status = "已更新 · \(Self.clockString()) · \(model) · \(ms)ms"
                    if parsed.signal != nil { status += " · 已落库价位" }
                    self.aiStatus = status
                    var c = self.settings.aiConfig
                    c.lastGoodModel = model
                    self.settings.setAIConfig(c)
                    AIUsageLedger.append(
                        model: model, ok: true, fallback: false, elapsedMs: ms,
                        outputChars: text.count, note: "成功"
                    )
                    self.reloadAIUsage()
                    self.logSignal(
                        kind: "ai",
                        title: "AI 分析 · \(model)",
                        body: String(formatted.prefix(160)),
                        evidence: evidence,
                        why: "模型成功返回；依据见快照。耗时 \(ms)ms"
                    )
                }
            } catch {
                guard !Task.isCancelled else { return }
                let ms = Int(Date().timeIntervalSince(t0) * 1000)
                await MainActor.run {
                    guard let self else { return }
                    let fallback = self.ruleBasedSummary()
                    self.aiText = fallback
                    self.aiUsedFallback = true
                    self.aiAnalyzing = false
                    // 模型失败时清掉旧标记，避免陈旧 AI 价位继续误导走势图
                    self.aiSignal = nil
                    var msg = error.localizedDescription
                    if msg.count > 48 { msg = String(msg.prefix(46)) + "…" }
                    self.aiStatus = "降级 · \(msg)"
                    if !self.settings.aiConfig.lastGoodModel.isEmpty,
                       self.settings.aiConfig.lastGoodModel != model {
                        self.aiStatus += " · 可切 \(self.settings.aiConfig.lastGoodModel)"
                    }
                    AIUsageLedger.append(
                        model: model, ok: false, fallback: true, elapsedMs: ms,
                        outputChars: fallback.count, note: msg
                    )
                    self.reloadAIUsage()
                    self.logSignal(
                        kind: "ai",
                        title: "AI 降级 · 规则摘要",
                        body: String(fallback.prefix(160)),
                        evidence: evidence,
                        why: "模型失败：\(error.localizedDescription) · \(ms)ms"
                    )
                }
            }
        }
    }

    func reloadAIUsage() {
        aiUsage = AIUsageLedger.load()
        aiTodayFeel = AIUsageLedger.todayFeel()
    }

    func switchToLastGoodModel() {
        let good = settings.aiConfig.lastGoodModel
        guard !good.isEmpty, good != settings.aiConfig.model else { return }
        settings.setAIModel(good)
        aiStatus = "已切回上次成功模型 · \(good)"
    }

    func probeNetworkHealth() async {
        healthProbing = true
        defer { healthProbing = false }
        async let stockProbes = MarketService.probeSources(symbol: settings.currentSymbol)
        await refreshBoardIndices()
        let probes = await stockProbes
        let age = SnapshotCache.cacheAgeSec(code: settings.currentCode)
        let idxAge = SnapshotCache.cacheAgeSec(code: "board-meta")
        healthReport = HealthReport(
            at: Date(),
            probes: probes,
            failStreak: failStreak,
            cacheAgeSec: age,
            cacheCode: settings.currentCode,
            liveOK: liveOK,
            usingCache: usingCache,
            indexProbes: lastIndexProbes,
            indexFailStreak: indexFailStreak,
            indexCacheAgeSec: idxAge
        )
    }

    private func maybeAutoAIAnalyze() {
        let cfg = settings.aiConfig
        guard cfg.enabled, cfg.autoAnalyze else { return }
        guard TradingSession.current() != .closed else { return }
        if let lastP = lastAIPrice, lastP > 0, quote.price > 0 {
            let move = abs(quote.price - lastP) / lastP * 100
            if move < 0.3, let last = lastAIAt,
               Date().timeIntervalSince(last) < Double(max(30, cfg.intervalSec)) {
                return
            }
        }
        analyzeWithAI(force: false)
    }

    /// 检查现价相对关键价位的预警档位，触发则调 AlertService.notify。
    /// 同档位内不重复发；进入更深的档位或从无→有会发；离开后再回到档位仍能再发（靠 AlertService 冷却）。
    /// 通知文案区分一级 / 二级，并在 SignalTimeline 落库便于事后回查。
    func checkLevelAlerts() {
        guard settings.alertsEnabled else { return }
        let p = quote.price
        guard p > 0 else { return }
        let sym = settings.currentSymbol
        let name = quote.name.isEmpty ? sym.name : quote.name
        let pairs: [(String, Double, String)] = [
            ("base",       base,       "基准"),
            ("support",    support,    "支撑"),
            ("resistance", resistance, "阻力")
        ]
        for (key, value, label) in pairs {
            guard value > 0 else { continue }
            let distPct = abs(p - value) / value * 100
            let newTier = LevelAlertTier.classify(
                distPct: distPct,
                lightPct: settings.notifyConfig.levelLightPct,
                deepPct: settings.notifyConfig.levelDeepPct
            )
            let oldTier = lastLevelAlert[key] ?? .none
            // 仅当档位升高（更深）或从 none 切入时发；同档内不重复发
            let shouldFire: Bool
            switch (oldTier, newTier) {
            case (_, .deep):                  shouldFire = oldTier != .deep   // 升档或首入二级
            case (.none, .light):             shouldFire = true              // 首入一级
            default:                          shouldFire = false             // 同档内不重复
            }
            lastLevelAlert[key] = newTier
            if shouldFire {
                let distStr = String(format: "%.2f%%", distPct)
                let tierStr = newTier == .deep ? "二级预警" : "一级预警"
                let title = "\(name) · \(label)\(tierStr)"
                let body  = String(format: "现价 %.2f  距 %.2f  %@",
                                   p, value, distStr)
                let nid = "alert-level-\(key)-\(newTier == .deep ? "deep" : "light")"
                AlertService.notify(
                    title: title, body: body, id: nid, code: sym.code,
                    userInfo: [
                        "kind":  "level",
                        "levelKey": key,                 // base / support / resistance
                        "tier":   newTier == .deep ? "deep" : "light",
                        "price":  String(value),
                        "code":   sym.code
                    ]
                )
                logSignal(
                    kind: "level",
                    title: title,
                    body: body,
                    why: "价位接近自动预警（\(tierStr)，距离 \(distStr)）",
                    meta: [
                        "levelKey":   key,
                        "tier":       newTier == .deep ? "deep" : "light",
                        "levelPrice": String(value),
                        "distPct":    String(format: "%.3f", distPct),
                        "hit":        "0"   // 后续价到检测写回 1
                    ]
                )
            }
        }
    }

    /// 持仓联动预警：现价相对持仓成本 cost 距离 ≤ costLightPct% 时触发
    /// 一次系统通知 + 落库 SignalTimeline (kind="posAlert") + 状态机避免重复发。
    func checkPositionAlert() {
        guard settings.alertsEnabled else { return }
        let cost = settings.position.cost
        guard cost > 0 else { return }
        let p = quote.price
        guard p > 0 else { return }
        let sym = settings.currentSymbol
        let name = quote.name.isEmpty ? sym.name : quote.name
        let distPct = abs(p - cost) / cost * 100
        let thr = settings.notifyConfig.costLightPct
        let inRange = distPct <= thr
        let prev = lastPositionAlertState
        if inRange && !prev {
            let distStr = String(format: "%.2f%%", distPct)
            let dir = p >= cost ? "上方" : "下方"
            let body = String(format: "现价 %.2f %@ 成本 %.2f  %@", p, dir, cost, distStr)
            let title = "\(name) · 到本预警"
            AlertService.notify(
                title: title, body: body, id: "alert-posAlert", code: sym.code,
                userInfo: [
                    "kind": "posAlert",
                    "cost": String(cost),
                    "price": String(p),
                    "code": sym.code
                ]
            )
            logSignal(
                kind: "posAlert",
                title: title,
                body: body,
                why: "现价距成本 \(distStr)（阈值 \(String(format: "%.2f%%", thr))）",
                meta: [
                    "cost": String(cost),
                    "distPct": String(format: "%.3f", distPct)
                ]
            )
        }
        lastPositionAlertState = inRange
    }

    /// 价到回测：扫描近 6 小时内的「价位预警」事件，若当前价已穿越/到达 levelPrice ±0.05%
    /// 且尚未标记 hit，则回写 meta（hit=1, hitAt, leadSec），刷新缓存供 UI 统计。
    /// 超时窗口（默认 6h）内没到的则视作失效（后续可在 UI 上标"失效"）。
    private static let hitWindowSec: TimeInterval = 6 * 3600
    private static let hitTolerancePct: Double = 0.05   // ±0.05%

    func checkLevelHits() {
        let p = quote.price
        guard p > 0 else { return }
        let now = Date()
        let events = SignalTimeline.load()
        var changed = false
        for ev in events where ev.kind == "level" {
            // 已命中过 / 已超时 → 跳过
            if ev.meta["hit"] == "1" { continue }
            guard let lpStr = ev.meta["levelPrice"], let lp = Double(lpStr), lp > 0 else { continue }
            if now.timeIntervalSince(ev.at) > Self.hitWindowSec { continue }
            let distPct = abs(p - lp) / lp * 100
            if distPct <= Self.hitTolerancePct {
                let leadSec = Int(now.timeIntervalSince(ev.at))
                let f = DateFormatter()
                f.locale = Locale(identifier: "en_US_POSIX")
                f.timeZone = TimeZone(identifier: "Asia/Shanghai")
                f.dateFormat = "MM-dd HH:mm:ss"
                SignalTimeline.updateMeta(id: ev.id) { m in
                    m["hit"]    = "1"
                    m["hitAt"]  = f.string(from: now)
                    m["leadSec"] = String(leadSec)
                    m["hitPrice"] = String(format: "%.2f", p)
                }
                changed = true
            }
        }
        if changed { signalEvents = SignalTimeline.load() }
    }

    func buildEvidenceSnapshot() -> String {
        let sym = settings.currentSymbol
        let name = quote.name.isEmpty ? sym.name : quote.name
        var lines: [String] = []
        lines.append("【依据快照 \(Self.clockString())】")
        lines.append("\(name) \(sym.code)")
        lines.append(String(format: "价 %.2f  涨跌%+.2f (%+.2f%%)  开%.2f 高%.2f 低%.2f 昨%.2f",
                            quote.price, quote.change, quote.pct, quote.open, quote.high, quote.low, quote.prev))
        lines.append(String(format: "基准%.2f 支撑%.2f 阻力%.2f", base, support, resistance))
        lines.append("MACD \(signalText.title)：\(signalText.detail)")
        if let m = lastMACD, let d = m.dif, let e = m.dea, let h = m.hist {
            lines.append(String(format: "DIF=%.3f DEA=%.3f HIST=%.3f", d, e, h))
        }
        if let k = lastKDJ, let kk = k.k, let dd = k.d, let jj = k.j {
            lines.append(String(format: "KDJ K=%.1f D=%.1f J=%.1f", kk, dd, jj))
        }
        if let r = lastRSI { lines.append(String(format: "RSI=%.1f", r)) }
        if let v = volRatio { lines.append(String(format: "量比=%.2f", v)) }
        if quoteDivergence > 0 {
            lines.append(String(format: "多源分歧=%.2f%% · %@", quoteDivergence, quote.source))
        } else {
            lines.append("源：\(quote.source)")
        }
        return lines.joined(separator: "\n")
    }

    func ruleBasedSummary() -> String {
        var bits: [String] = []
        let sig = signalText.title
        if sig == "金叉" { bits.append("总判断：偏多（MACD金叉）") }
        else if sig == "死叉" { bits.append("总判断：偏空（MACD死叉）") }
        else if sig == "多头" { bits.append("总判断：偏多震荡") }
        else if sig == "空头" { bits.append("总判断：偏空震荡") }
        else { bits.append("总判断：数据不足，观望") }

        if support > 0, quote.price > 0 {
            let d = quote.price - support
            bits.append(d >= 0
                        ? String(format: "· 现价在支撑上方 %.2f", d)
                        : String(format: "· 已跌破支撑 %.2f", -d))
        }
        if resistance > 0, quote.price > 0 {
            bits.append(String(format: "· 距阻力 %.2f", resistance - quote.price))
        }
        if let r = lastRSI {
            if r >= 70 { bits.append(String(format: "· RSI %.0f 偏超买", r)) }
            else if r <= 30 { bits.append(String(format: "· RSI %.0f 偏超卖", r)) }
            else { bits.append(String(format: "· RSI %.0f 中性", r)) }
        }
        if let v = volRatio {
            bits.append(v >= 2 ? String(format: "· 量比 %.1f 放量", v) : String(format: "· 量比 %.1f", v))
        }
        bits.append("· 风险：规则摘要非模型结论；破支撑/死叉则观察失效")
        bits.append("\n——\n" + aiEvidence)
        return bits.joined(separator: "\n")
    }

    private func buildAIPrompt(evidence: String) -> String {
        var lines: [String] = [evidence]
        if let p = pnl {
            lines.append(String(format: "持仓浮盈：%+.0f (%+.2f%%)", p.amount, p.pct))
        }
        let pos = settings.position
        if pos.stopLoss > 0 {
            lines.append(String(format: "止损价：%.2f  计划仓位：%.0f%%", pos.stopLoss, pos.positionPct))
        }
        if pos.takeProfit > 0 {
            lines.append(String(format: "止盈价：%.2f", pos.takeProfit))
        }
        // 智能止损/止盈建议交给 AI 点评（数值由锚点公式决定，AI 只负责叙事复核）
        if let advice = stopTakeAdvice, advice.stop > 0 || advice.take > 0 {
            lines.append(String(format: "智能止损止盈建议：止损 %.2f（距现价 %.1f%%）止盈 %.2f 盈亏比 %.2f",
                                advice.stop, advice.stopDistPct, advice.take, advice.ratio))
            let anchors = (advice.stopAnchors + advice.takeAnchors).joined(separator: "；")
            if !anchors.isEmpty { lines.append("建议依据：\(anchors)") }
            lines.append("请点评该止损止盈位的合理性（结构 / 波动 / 仓位视角，一两句），不要另给价位。")
        }
        if !lastStrategyWhy.isEmpty {
            lines.append("最近策略对照：\n\(lastStrategyWhy)")
        }
        let hist = settings.searchDiaries(code: settings.currentCode, limit: 5)
        if !hist.isEmpty {
            lines.append("历史复盘（同标的，供对照今日结构）：")
            for h in hist.prefix(3) {
                let snippet = String(h.text.prefix(120)).replacingOccurrences(of: "\n", with: " ")
                lines.append("- \(h.day)：\(snippet)")
            }
            lines.append("请对比今日与历史同结构异同（一两句）。")
        }
        lines.append("请给出当前实时观察结论。")
        return lines.joined(separator: "\n")
    }

    func consumePendingNotifyAction() {
        let code = NotifyGovernor.pendingOpenCode ?? ""
        let act = NotifyGovernor.pendingAction ?? "open"
        NotifyGovernor.pendingOpenCode = nil
        NotifyGovernor.pendingAction = nil
        if !code.isEmpty { switchSymbol(code) }
        pendingUIAction = act
    }

    /// 消费通知载荷：跳到价位 + 草稿委托。
    /// levelKey: base | support | resistance
    /// tier: light | deep（仅用于 ticketNote 标注）
    func consumePendingLevelPayload() {
        let p = pendingLevelPayload
        pendingLevelPayload = [:]
        guard let key = p["levelKey"],
              let priceStr = p["price"],
              let price = Double(priceStr), price > 0 else { return }
        let tier = p["tier"] ?? "light"
        switch key {
        case "resistance":
            settings.updateLevels(base: settings.levels.base,
                                  support: settings.levels.support,
                                  resistance: price)
            ticketPriceMode = .limit
            ticketSide = .sell
            ticketPrice = price
            ticketSource = "AI 阻力预警"
            ticketNote = "阻力 \(String(format: "%.2f", price)) · \(tier == "deep" ? "二级" : "一级")"
        case "support":
            settings.updateLevels(base: settings.levels.base,
                                  support: price,
                                  resistance: settings.levels.resistance)
            ticketPriceMode = .limit
            ticketSide = .buy
            ticketPrice = price
            ticketSource = "AI 支撑预警"
            ticketNote = "支撑 \(String(format: "%.2f", price)) · \(tier == "deep" ? "二级" : "一级")"
        case "base":
            // 基准不带方向，仅把价位同步给基准；并按价格相对现价方向草稿一个委托
            settings.updateLevels(base: price,
                                  support: settings.levels.support,
                                  resistance: settings.levels.resistance)
            let px = quote.price
            if px > 0 {
                ticketPriceMode = .limit
                if price > px {
                    ticketSide = .sell
                    ticketSource = "AI 基准（高位）"
                    ticketNote = "基准 \(String(format: "%.2f", price)) · 高于现价草稿卖出"
                } else if price < px {
                    ticketSide = .buy
                    ticketSource = "AI 基准（低位）"
                    ticketNote = "基准 \(String(format: "%.2f", price)) · 低于现价草稿买入"
                } else {
                    ticketSource = "AI 基准"
                    ticketNote = "基准 \(String(format: "%.2f", price))"
                }
                ticketPrice = price
            }
        default:
            return
        }
        ticketHint = "通知跳转 · 已落库价位并填入草稿委托"
    }

    private static func clockString() -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "HH:mm:ss"
        return f.string(from: Date())
    }

    // MARK: - AI JSON 解析

    /// 解析 AI 返回文本，输出结构化信号。
    /// - 返回的 tuple.signal 可能为 nil（解析失败或字段缺失），displayText 始终为可展示文本。
    static func parseAISignal(from text: String) -> (signal: AILatestSignal?, displayText: String) {
        guard let obj = AIJSONExtractor.extractObject(from: text) else {
            return (nil, text.trimmingCharacters(in: .whitespacesAndNewlines))
        }

        let summary = (obj["summary"] as? String) ?? ""
        let verdictStr = (obj["verdict"] as? String)?.lowercased() ?? "range"
        let verdict: AILatestSignal.Verdict
        switch verdictStr {
        case "bull": verdict = .bull
        case "bear": verdict = .bear
        default:     verdict = .range
        }
        let keyPoints = (obj["keyPoints"] as? [String]) ?? []
        let risk = (obj["risk"] as? String) ?? ""

        let levelsObj = obj["levels"] as? [String: Any] ?? [:]
        let baseV       = (levelsObj["base"]       as? NSNumber)?.doubleValue ?? 0
        let supportV    = (levelsObj["support"]    as? NSNumber)?.doubleValue ?? 0
        let resistanceV = (levelsObj["resistance"] as? NSNumber)?.doubleValue ?? 0
        let stopV       = (levelsObj["stop"]       as? NSNumber)?.doubleValue ?? 0

        // 事件流
        var events: [AIEvent] = []
        if let arr = obj["events"] as? [[String: Any]] {
            for raw in arr.prefix(5) {
                let kindStr = (raw["kind"] as? String)?.lowercased() ?? ""
                let dirStr  = (raw["dir"] as? String)?.lowercased() ?? "up"
                guard let kind = Self.eventKind(kindStr) else { continue }
                let dir: AIEvent.Dir = dirStr == "down" ? .down : .up
                let mo = (raw["minuteOffset"] as? NSNumber)?.intValue ?? 0
                let price = (raw["price"] as? NSNumber)?.doubleValue ?? 0
                let note = (raw["note"] as? String) ?? ""
                // 至少要有一个有效字段
                if mo == 0 && price == 0 && note.isEmpty { continue }
                events.append(AIEvent(kind: kind, dir: dir,
                                      minuteOffset: max(0, min(240, mo)),
                                      price: price, note: note))
            }
        }
        let volWarn = (obj["volumeWarnRatio"] as? NSNumber)?.doubleValue ?? 0

        let allZero = baseV <= 0 && supportV <= 0 && resistanceV <= 0 && stopV <= 0
        if summary.isEmpty && keyPoints.isEmpty && risk.isEmpty && allZero && events.isEmpty && volWarn <= 0 {
            return (nil, text.trimmingCharacters(in: .whitespacesAndNewlines))
        }

        let sig = AILatestSignal(
            verdict: verdict,
            summary: summary,
            keyPoints: keyPoints,
            risk: risk,
            base: baseV,
            support: supportV,
            resistance: resistanceV,
            stop: stopV,
            events: events,
            volumeWarnRatio: volWarn,
            updatedAt: Date()
        )
        return (sig, formatAIText(parsed: (sig, text), raw: text))
    }

    private static func eventKind(_ s: String) -> AIEvent.Kind? {
        switch s {
        case "breakout":   return .breakout
        case "breakdown":  return .breakdown
        case "volspike":   return .volSpike
        case "fakeout":    return .fakeout
        case "reversal":   return .reversal
        default:           return nil
        }
    }

    /// 把 AI JSON 渲染成中文短文本，用于 aiText 展示。
    static func formatAIText(parsed: (signal: AILatestSignal?, displayText: String), raw: String) -> String {
        guard let s = parsed.signal else { return parsed.displayText }
        var lines: [String] = []
        let prefix: String
        switch s.verdict {
        case .bull: prefix = "总判断：偏多"
        case .bear: prefix = "总判断：偏空"
        case .range: prefix = "总判断：震荡"
        }
        if !s.summary.isEmpty {
            lines.append("\(prefix) · \(s.summary)")
        } else {
            lines.append(prefix)
        }
        var levelBits: [String] = []
        if s.base > 0       { levelBits.append(String(format: "基%.2f", s.base)) }
        if s.resistance > 0 { levelBits.append(String(format: "阻%.2f", s.resistance)) }
        if s.support > 0    { levelBits.append(String(format: "支%.2f", s.support)) }
        if s.stop > 0       { levelBits.append(String(format: "止%.2f", s.stop)) }
        if !levelBits.isEmpty {
            lines.append("AI 价位 · " + levelBits.joined(separator: "  "))
        }
        for (i, kp) in s.keyPoints.prefix(5).enumerated() {
            lines.append("\(i + 1). \(kp)")
        }
        // 事件流摘要
        if !s.events.isEmpty {
            let parts: [String] = s.events.prefix(5).map { ev in
                let arrow = ev.isUpArrow ? "↑" : "↓"
                let price = ev.price > 0 ? String(format: "%.2f", ev.price) : ""
                let note  = ev.note.isEmpty ? "" : "·\(ev.note)"
                return "\(ev.label)\(arrow)\(price)\(note)"
            }
            lines.append("事件 · " + parts.joined(separator: "  "))
        }
        if s.volumeWarnRatio > 0 {
            lines.append(String(format: "量比警戒 %.1f×", s.volumeWarnRatio))
        }
        if !s.risk.isEmpty {
            lines.append("风险 · \(s.risk)")
        }
        // 兜底：如果解析异常导致所有字段都空，至少展示原始文本。
        if lines.count <= 1 && !raw.isEmpty {
            return raw.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        return lines.joined(separator: "\n")
    }

    func refreshNotifyQuota() {
        notifyQuotaLine = NotifyGovernor.shared.quotaSnapshot().line
    }

    func logSignal(kind: String, title: String, body: String, evidence: String? = nil, why: String = "", code: String? = nil, meta: [String: String] = [:]) {
        let ev = SignalTimeline.append(
            kind: kind,
            title: title,
            body: body,
            code: code ?? settings.currentCode,
            price: quote.price,
            source: quoteVoteBadge.isEmpty ? quote.source : quoteVoteBadge,
            evidence: evidence ?? buildEvidenceSnapshot(),
            why: why,
            meta: meta
        )
        signalEvents = SignalTimeline.load()
        selectedSignalID = ev.id
    }

    private func flashAndNotify(
        title: String,
        body: String,
        id: String,
        code: String? = nil,
        kind: String = "alert",
        why: String = "",
        userInfo: [String: String] = [:]
    ) {
        menuFlash = true
        let c = code ?? settings.currentCode
        var payload = userInfo
        if !kind.isEmpty { payload["kind"] = kind }
        AlertService.notify(title: "摸金小王子 · \(title)", body: body, id: id, code: c, userInfo: payload)
        logSignal(kind: kind, title: title, body: body, why: why.isEmpty ? body : why, code: c)
        refreshNotifyQuota()
        Task {
            try? await Task.sleep(nanoseconds: 1_200_000_000)
            menuFlash = false
            try? await Task.sleep(nanoseconds: 400_000_000)
            menuFlash = true
            try? await Task.sleep(nanoseconds: 800_000_000)
            menuFlash = false
        }
    }

    static func todayString() -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: Date())
    }
}

/// AI 命中回测汇总（统计信号时间窗 6h 内 hit 数 / 总数）
struct AIAccuracy: Equatable {
    let total: Int           // 触发的价位预警总条数
    let hit: Int             // hit=1 的条数
    let hitRate: Double      // 命中率 0~1
    let avgLeadSec: Int?     // 平均领先（从预警到命中）
    let maxDrawdownPct: Double  // 未命中里 levelPrice 相对现价最远偏离（%）
}
