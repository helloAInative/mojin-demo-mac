import Foundation

struct GatewaySettingsSnapshot: Codable {
    var currentCode: String?
    var levelsByCode: [String: SymbolLevels]?
    var strategies: [String: StrategyNote]?
    var comboStrategies: [ComboStrategy]?
    var diaries: [String: String]?
    var alertsEnabled: Bool?
    var openCloseBrief: Bool?
    var theme: AppTheme?
    var menuBarFormat: MenuBarFormat?
    var fontSize: FontSizePref?
    var indicator: String?
    var notifyConfig: NotifyGovernor.Config?
    var aiConfig: AIConfig?
    var boardShowHS300: Bool?
    var boardShowBJ50: Bool?

    var isEmpty: Bool {
        currentCode == nil && levelsByCode == nil && strategies == nil &&
        comboStrategies == nil && diaries == nil && alertsEnabled == nil &&
        openCloseBrief == nil && theme == nil && menuBarFormat == nil &&
        fontSize == nil && indicator == nil && notifyConfig == nil &&
        aiConfig == nil && boardShowHS300 == nil && boardShowBJ50 == nil
    }
}

struct GatewayPosition: Codable {
    var code: String
    var cost: Double
    var shares: Double
    var stopLoss: Double
    var takeProfit: Double
    var positionPct: Double
    var note: String?
    var updatedAt: Int64?
}

struct GatewayWatchItem: Codable {
    var code: String
    var name: String
    var market: String
    var pinned: Bool
    var group: String
    var addedAt: Int64?
    var updatedAt: Int64?
}

struct GatewayRemoteState {
    var positions: [GatewayPosition]
    var watchlist: [GatewayWatchItem]
    var settings: GatewaySettingsSnapshot

    var isEmpty: Bool { positions.isEmpty && watchlist.isEmpty && settings.isEmpty }
}

struct GatewayQuoteUpdate {
    var code: String
    var quote: Quote
}

/// §F.3：个股新闻（东财，经网关缓存）。
struct GatewayNewsItem: Codable, Equatable, Identifiable {
    var code: String
    var title: String
    var summary: String
    var media: String
    var url: String
    var publishedAt: Date

    var id: String { url }

    enum CodingKeys: String, CodingKey {
        case code, title, summary, media, url
        case publishedAt = "published_at"
    }

    /// 北京时间的 "MM-dd HH:mm"。
    var cnClock: String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: publishedAt)
    }
}

/// §F.4：机构研报（东财研报库）。
struct GatewayResearchReport: Codable, Equatable, Identifiable {
    var code: String
    var title: String
    var org: String
    var publishDate: String
    var rating: String
    var lastRating: String
    var ratingChange: Int?
    var researcher: String
    var industry: String
    var aimPriceHigh: Double?
    var aimPriceLow: Double?
    var url: String

    var id: String { url }

    /// 东财评级文案 → 涨跌色：买入 / 增持类红，卖出 / 减持类绿。
    var isBullish: Bool? {
        switch rating {
        case "买入", "增持", "强买", "推荐", "强烈推荐": return true
        case "卖出", "减持", "回避": return false
        default: return nil
        }
    }

    var ratingChangeText: String {
        switch ratingChange {
        case 1: return "上调"
        case 2: return "下调"
        case 3: return "维持"
        default: return lastRating.isEmpty ? "新覆盖" : "续评"
        }
    }

    /// 目标价摘要（详情 tooltip 用）。
    var aimPriceText: String {
        switch (aimPriceLow, aimPriceHigh) {
        case let (low?, high?) where high > 0:
            return String(format: "%.2f-%.2f", min(low, high), max(low, high))
        case let (high?, _) where high > 0:
            return String(format: "%.2f", high)
        default: return "未给出"
        }
    }

    enum CodingKeys: String, CodingKey {
        case code, title, org, rating, researcher, industry, url
        case publishDate = "publish_date"
        case lastRating = "last_rating"
        case ratingChange = "rating_change"
        case aimPriceHigh = "aim_price_high"
        case aimPriceLow = "aim_price_low"
    }
}

/// §F.4：个股所属概念 / 行业板块（含板块指数与涨跌幅）。
struct GatewaySectorBoard: Codable, Equatable, Identifiable {
    var code: String
    var boardCode: String
    var name: String
    var isPrecise: Bool
    var reason: String
    var price: Double?
    var changePct: Double?

    var id: String { boardCode }

    enum CodingKeys: String, CodingKey {
        case code, name, reason, price
        case boardCode = "board_code"
        case isPrecise = "is_precise"
        case changePct = "change_pct"
    }
}

/// B.4：逐笔成交（服务端 TickItem）。
struct GatewayTick: Codable, Equatable, Identifiable {
    var ts: Date
    var price: Double
    /// 手
    var volume: Int
    /// 1 买盘 / 2 卖盘 / 4 中性（0 集合竞价）
    var direction: Int

    var id: Date { ts }
}

/// 逐笔按分钟聚合桶（B.4：分时下方柱状 + 下钻数据）。
struct TickBucket: Equatable, Identifiable {
    /// "HHmm"
    var minute: String
    var volume: Int
    var buyVolume: Int
    var sellVolume: Int
    var ticks: [GatewayTick]
    var id: String { minute }

    /// 净买卖决定柱色（红买绿卖）
    var netBuy: Bool { buyVolume >= sellVolume }
}

enum TickBucketer {
    /// 按北京时间分钟分桶（升序）；direction 1/0 归买盘、2 归卖盘、4 中性不计入买卖。
    static func buckets(from ticks: [GatewayTick]) -> [TickBucket] {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "HHmm"
        var order: [String] = []
        var map: [String: TickBucket] = [:]
        for tick in ticks.sorted(by: { $0.ts < $1.ts }) {
            let minute = formatter.string(from: tick.ts)
            guard minute.count == 4 else { continue }
            var bucket = map[minute] ?? TickBucket(minute: minute, volume: 0,
                                                   buyVolume: 0, sellVolume: 0, ticks: [])
            bucket.volume += tick.volume
            switch tick.direction {
            case 2: bucket.sellVolume += tick.volume
            default: bucket.buyVolume += tick.volume   // 1 买 / 0 竞价 / 4 中性偏买侧计数
            }
            bucket.ticks.append(tick)
            if map[minute] == nil { order.append(minute) }
            map[minute] = bucket
        }
        return order.map { map[$0]! }
    }
}

/// A 股池智能推荐：单条（服务端 daily_pick 行）。
struct GatewayPick: Codable, Equatable, Identifiable {
    var date: String
    var code: String
    var name: String
    var rank: Int
    var score: Double
    /// 量化 / 消息面标签
    var reasons: [String]
    /// AI 一句话理由（纯量化版为空）
    var aiNote: String
    var meta: Meta

    var id: String { "\(date)-\(code)" }

    struct Meta: Codable, Equatable {
        var close: Double?
        var pct: Double?
        var industry: String?
        var outcome: Outcome?
        var autoWeight: AutoWeight?
        var plan: TradePlan?

        enum CodingKeys: String, CodingKey {
            case close, pct, industry, outcome, plan
            case autoWeight = "auto_weight"
        }
    }

    struct TradePlan: Codable, Equatable {
        var signalDate: String?
        var target: String?
        var objective: String?
        var entryTiming: String?
        var entryLabel: String?
        var entryWindow: String?
        var strategy: String?
        var hasBasePosition: Bool?
        var exitRule: String?

        enum CodingKeys: String, CodingKey {
            case target, objective, strategy
            case signalDate = "signal_date"
            case entryTiming = "entry_timing"
            case entryLabel = "entry_label"
            case entryWindow = "entry_window"
            case hasBasePosition = "has_base_position"
            case exitRule = "exit_rule"
        }
    }

    struct AutoWeight: Codable, Equatable {
        var adjustment: Double?
        var tags: [WeightEvidence]?
        var minimumSamples: Int?
        var lookbackDays: Int?

        enum CodingKeys: String, CodingKey {
            case adjustment, tags
            case minimumSamples = "minimum_samples"
            case lookbackDays = "lookback_days"
        }
    }

    struct WeightEvidence: Codable, Equatable {
        var tag: String
        var samples: Int
        var winRate: Double
        var delta: Double

        enum CodingKeys: String, CodingKey {
            case tag, samples, delta
            case winRate = "win_rate"
        }
    }

    struct Outcome: Codable, Equatable {
        var t1Pct: Double?
        var t5Pct: Double?

        enum CodingKeys: String, CodingKey {
            case t1Pct = "t1_pct"
            case t5Pct = "t5_pct"
        }
    }

    enum CodingKeys: String, CodingKey {
        case date, code, name, rank, score, reasons, meta
        case aiNote = "ai_note"
    }
}

/// 推荐文档：当日尾盘清单 + 近 30 天 T+1 主回测（T+5 中线参考）。
struct GatewayPicksDocument: Decodable, Equatable {
    var date: String
    var picks: [GatewayPick]
    var samples: Int
    var t1WinRate: Double
    var avgT1Pct: Double
    var t5Samples: Int
    var t5WinRate: Double
    var avgT5Pct: Double

    private enum StatsKeys: String, CodingKey {
        case samples
        case t1WinRate = "t1_win_rate"
        case avgT1Pct = "avg_t1_pct"
        case t5Samples = "t5_samples"
        case t5WinRate = "t5_win_rate"
        case avgT5Pct = "avg_t5_pct"
        case tags
    }

    private enum CodingKeys: String, CodingKey {
        case date, picks, stats, market
        case executeHint = "execute_hint"
    }

    /// 标签级 T+1 回测（哪个因子更适合隔日目标，按样本数降序 ≤6）
    var tags: [TagStat]
    /// 隔夜美股（djia / ixic 涨跌 %）
    var market: MarketInfo?
    /// 执行时机：「当天下午可买入」或「次日开盘买入…」
    var executeHint: String?

    struct TagStat: Decodable, Equatable, Identifiable {
        var tag: String
        var samples: Int
        var winRate: Double
        var id: String { tag }

        enum CodingKeys: String, CodingKey {
            case tag, samples
            case winRate = "win_rate"
        }
    }

    struct MarketInfo: Decodable, Equatable {
        var djia: Double?
        var ixic: Double?
    }

    init(date: String, picks: [GatewayPick], samples: Int, t5WinRate: Double, avgT5Pct: Double,
         tags: [TagStat] = [], market: MarketInfo? = nil,
         t1WinRate: Double = 0, avgT1Pct: Double = 0, t5Samples: Int = 0) {
        self.date = date
        self.picks = picks
        self.samples = samples
        self.t1WinRate = t1WinRate
        self.avgT1Pct = avgT1Pct
        self.t5Samples = t5Samples
        self.t5WinRate = t5WinRate
        self.avgT5Pct = avgT5Pct
        self.tags = tags
        self.market = market
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        date = try c.decode(String.self, forKey: .date)
        picks = try c.decode([GatewayPick].self, forKey: .picks)
        let stats = try c.nestedContainer(keyedBy: StatsKeys.self, forKey: .stats)
        samples = (try? stats.decodeIfPresent(Int.self, forKey: .samples)) ?? 0
        t1WinRate = (try? stats.decodeIfPresent(Double.self, forKey: .t1WinRate)) ?? 0
        avgT1Pct = (try? stats.decodeIfPresent(Double.self, forKey: .avgT1Pct)) ?? 0
        t5Samples = (try? stats.decodeIfPresent(Int.self, forKey: .t5Samples)) ?? 0
        t5WinRate = (try? stats.decodeIfPresent(Double.self, forKey: .t5WinRate)) ?? 0
        avgT5Pct = (try? stats.decodeIfPresent(Double.self, forKey: .avgT5Pct)) ?? 0
        tags = (try? stats.decodeIfPresent([TagStat].self, forKey: .tags)) ?? []
        market = try? c.decodeIfPresent(MarketInfo.self, forKey: .market)
        executeHint = try? c.decodeIfPresent(String.self, forKey: .executeHint)
    }
}

/// 每日数据归档（ROI #12 回放）：与服务端 `data/archives/{date}.json` 同构。
struct GatewayDayExport: Decodable, Equatable {
    var date: String
    var signals: [Signal]
    var codes: [String: Code]

    struct Signal: Decodable, Equatable, Identifiable {
        var id: String
        /// 发射时间（unix ms）
        var at: Double
        var kind: String
        var code: String
        var title: String
    }

    struct Code: Decodable, Equatable {
        var name: String?
        var quote: Quote?
        /// [ts(ms), price, avg, volume] 紧凑数组
        var minutes: [[Double]]?

        struct Quote: Decodable, Equatable {
            var close: Double?
            var prev: Double?
            var open: Double?
            var high: Double?
            var low: Double?
        }
    }

    /// 当前标的的回放分时（ts ms → 北京时间 HHmm）。
    func minuteBars(code: String) -> (bars: [MinuteBar], prev: Double) {
        guard let node = codes[code], let rows = node.minutes, !rows.isEmpty else {
            guard let node = codes[code] else { return ([], 0) }
            return ([], node.quote?.prev ?? 0)
        }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "HHmm"
        let bars: [MinuteBar] = rows.compactMap { row in
            guard row.count >= 4, row[0] > 0, row[1] > 0 else { return nil }
            let clock = formatter.string(from: Date(timeIntervalSince1970: row[0] / 1000.0))
            guard clock.count == 4 else { return nil }
            return MinuteBar(minute: clock, price: row[1], avg: row[2], vol: row[3])
        }
        return (bars, node.quote?.prev ?? 0)
    }
}

/// Rust 行情网关的 Swift DTO 适配层。这里不访问第三方行情源。
enum GatewayMarketClient {
    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 3.5
        config.timeoutIntervalForResource = 4
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: config)
    }()
    /// 选股会串行拉取候选日 K、消息面并调用模型，不能复用行情的 4 秒超时。
    private static let picksSession: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 240
        config.timeoutIntervalForResource = 300
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.waitsForConnectivity = true
        return URLSession(configuration: config)
    }()
    private static let circuit = GatewayCircuit()

    private struct RemoteQuote: Decodable {
        let name: String
        let price: Double
        let prev: Double
        let open: Double
        let high: Double
        let low: Double
        let source: String
        let ts: String
    }

    private struct RemoteMinute: Decodable {
        let ts: String
        let price: Double
        let avg_price: Double
        let volume: Double
    }

    private struct RemoteDay: Decodable {
        let date: String
        let open: Double
        let close: Double
        let high: Double
        let low: Double
        let volume: Double
    }

    enum GatewayError: Error {
        case disabled
        case invalidURL
        case unavailable
        case badResponse
    }

    private struct HealthResponse: Decodable {
        let status: String
        let db: String
    }

    static func health(baseURL: String) async throws -> String {
        let base = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let endpoint = URL(string: base + "/health"),
              ["http", "https"].contains(endpoint.scheme?.lowercased() ?? ""),
              endpoint.host != nil else { throw GatewayError.invalidURL }
        let started = Date()
        let (data, response) = try await session.data(from: endpoint)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        let health = try JSONDecoder().decode(HealthResponse.self, from: data)
        guard health.status == "ok", health.db == "ok" else { throw GatewayError.unavailable }
        return "连接成功 · \(Int(Date().timeIntervalSince(started) * 1000)) ms · 数据库正常"
    }

    static func quote(symbol: WatchSymbol) async throws -> Quote {
        let remote: RemoteQuote = try await get("quote/\(symbol.code)")
        return try quote(from: remote, fallbackName: symbol.name)
    }

    private static func quote(from remote: RemoteQuote, fallbackName: String = "") throws -> Quote {
        guard remote.price > 0, remote.prev > 0 else { throw GatewayError.badResponse }
        let change = remote.price - remote.prev
        return Quote(
            name: remote.name.isEmpty ? fallbackName : remote.name,
            price: remote.price,
            prev: remote.prev,
            open: remote.open,
            high: remote.high,
            low: remote.low,
            change: change,
            pct: change / remote.prev * 100,
            timeText: chinaTime(remote.ts, format: "HH:mm:ss"),
            source: "网关·\(sourceName(remote.source))"
        )
    }

    /// WebSocket 行情流。断线重连由 MarketStore 负责，HTTP 轮询始终作为兜底。
    static func quoteUpdates() async throws -> AsyncThrowingStream<GatewayQuoteUpdate, Error> {
        struct Event: Decodable {
            var type: String
            var quote: RemoteQuoteWithCode
        }
        struct RemoteQuoteWithCode: Decodable {
            var code: String
            var name: String
            var price: Double
            var prev: Double
            var open: Double
            var high: Double
            var low: Double
            var source: String
            var ts: String
        }

        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard enabled, var components = URLComponents(string: base) else {
            throw GatewayError.disabled
        }
        switch components.scheme?.lowercased() {
        case "http": components.scheme = "ws"
        case "https": components.scheme = "wss"
        default: throw GatewayError.invalidURL
        }
        components.path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            + "/api/v1/ws/quote"
        components.query = nil
        guard let url = components.url else { throw GatewayError.invalidURL }

        return AsyncThrowingStream { continuation in
            let socket = session.webSocketTask(with: url)
            socket.resume()
            let receiveTask = Task {
                do {
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message {
                        case .data(let value): data = value
                        case .string(let value): data = Data(value.utf8)
                        @unknown default: continue
                        }
                        let event = try JSONDecoder().decode(Event.self, from: data)
                        guard event.type == "quote" else { continue }
                        let remote = event.quote
                        let legacy = RemoteQuote(name: remote.name, price: remote.price, prev: remote.prev,
                                                 open: remote.open, high: remote.high, low: remote.low,
                                                 source: remote.source, ts: remote.ts)
                        let value = try quote(from: legacy)
                        continuation.yield(GatewayQuoteUpdate(code: remote.code, quote: value))
                    }
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in
                receiveTask.cancel()
                socket.cancel(with: .goingAway, reason: nil)
            }
        }
    }

    static func minutes(symbol: WatchSymbol) async throws -> [MinuteBar] {
        let remote: [RemoteMinute] = try await get("quote/\(symbol.code)/minutes?limit=240")
        guard !remote.isEmpty else { throw GatewayError.badResponse }
        let bars = remote.compactMap { bar -> MinuteBar? in
            guard bar.price > 0 else { return nil }
            let minute = chinaTime(bar.ts, format: "HHmm")
            guard minute.count == 4 else { return nil }
            return MinuteBar(minute: minute, price: bar.price, avg: bar.avg_price, vol: bar.volume)
        }
        guard !bars.isEmpty else { throw GatewayError.badResponse }
        return MarketService.fillMinuteGaps(bars)
    }

    static func days(symbol: WatchSymbol, limit: Int) async throws -> [DayBar] {
        let remote: [RemoteDay] = try await get("quote/\(symbol.code)/days?limit=\(limit)")
        let bars = remote.filter { $0.close > 0 }.map { bar in
            DayBar(date: bar.date, open: bar.open, close: bar.close,
                   high: bar.high, low: bar.low, volume: bar.volume)
        }
        guard !bars.isEmpty else { throw GatewayError.badResponse }
        return bars
    }

    /// 阶段 3：服务端信号时间线。调用失败时上层继续使用本地 JSON 缓存。
    static func signals(limit: Int = 200) async throws -> [SignalEvent] {
        try await getData("signals?limit=\(max(1, min(limit, 1000)))")
    }

    /// §F.3：个股新闻（`hours` 为响应时间窗；落库的始终是全量）。
    static func news(code: String, limit: Int = 10, hours: Int = 72) async throws -> [GatewayNewsItem] {
        try await getData("news/\(code)?limit=\(max(1, min(limit, 50)))&hours=\(max(1, min(hours, 720)))")
    }

    /// §F.4：机构研报（近 `days` 天，按发布日倒序）。
    static func reports(code: String, limit: Int = 10, days: Int = 90) async throws -> [GatewayResearchReport] {
        try await getData("reports/\(code)?limit=\(max(1, min(limit, 50)))&days=\(max(1, min(days, 1095)))")
    }

    /// §F.4：所属概念 / 行业板块（板块指数 + 涨跌幅）。
    static func sector(code: String) async throws -> [GatewaySectorBoard] {
        try await getData("sector/\(code)")
    }

    /// B.4：逐笔成交明细（东财 push2delay details，按需拉取）。
    static func ticks(code: String, limit: Int = 2000) async throws -> [GatewayTick] {
        try await getData("quote/\(code)/ticks?limit=\(max(50, min(limit, 5000)))")
    }

    /// A 股池智能推荐：最近一份（含 T+5 回测统计）。
    static func picks() async throws -> GatewayPicksDocument {
        try await getData("picks")
    }

    /// 每日数据归档原始 JSON（存档 / 回放共用）。
    static func dayExportData(date: String?) async throws -> Data {
        let path = date.map { "export/day?date=\($0)" } ?? "export/day"
        let request = try await dataRequest(path: path, method: "GET")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        return data
    }

    /// 回放用：归档解码为结构化文档。
    static func dayExport(date: String?) async throws -> GatewayDayExport {
        try JSONDecoder().decode(GatewayDayExport.self, from: await dayExportData(date: date))
    }

    /// 重新生成当日推荐；`ai` 透传本机 AI 配置做精排（密钥只在请求内使用）。
    static func runPicks(ai: AiRankConfig?) async throws -> GatewayPicksDocument {
        struct Body: Encodable {
            var ai: AiRankConfig?
        }
        var request = try await dataRequest(path: "picks/run", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Body(ai: ai))
        request.timeoutInterval = 240
        let (data, response) = try await picksSession.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        return try JSONDecoder().decode(GatewayPicksDocument.self, from: data)
    }

    /// POST /picks/run 的 AI 精排配置（snake_case 对齐服务端 serde）。
    struct AiRankConfig: Codable, Equatable {
        var provider: String
        var baseURL: String
        var apiKey: String
        var model: String

        enum CodingKeys: String, CodingKey {
            case provider, model
            case baseURL = "base_url"
            case apiKey = "api_key"
        }
    }

    /// 本地先写、服务端后写；网络错误不会影响盯盘主流程。
    static func putSignal(_ event: SignalEvent) async throws {
        try await sendData("signals", method: "POST", body: event)
    }

    static func deleteSignals() async throws {
        let request = try await dataRequest(path: "signals", method: "DELETE")
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 204 else { throw GatewayError.badResponse }
    }

    static func remoteState() async throws -> GatewayRemoteState {
        struct SettingsDocument: Decodable { var values: GatewaySettingsSnapshot }
        async let positions: [GatewayPosition] = getData("positions")
        async let watchlist: [GatewayWatchItem] = getData("watchlist")
        async let settings: SettingsDocument = getData("settings")
        return try await GatewayRemoteState(
            positions: positions,
            watchlist: watchlist,
            settings: settings.values
        )
    }

    static func putSettings(_ settings: GatewaySettingsSnapshot) async throws {
        try await sendData("settings", method: "PUT", body: settings)
    }

    static func putPosition(code: String, position: PositionNote) async throws {
        struct Body: Encodable {
            var cost: Double
            var shares: Double
            var stopLoss: Double
            var takeProfit: Double = 0
            var positionPct: Double
        }
        try await sendData(
            "positions/\(code)", method: "PUT",
            body: Body(cost: position.cost, shares: position.shares,
                       stopLoss: position.stopLoss, takeProfit: position.takeProfit,
                       positionPct: position.positionPct)
        )
    }

    static func putWatchSymbol(_ symbol: WatchSymbol) async throws {
        struct Body: Encodable {
            var code: String
            var name: String
            var market: String
            var pinned: Bool
            var group: String
        }
        try await sendData(
            "watchlist", method: "POST",
            body: Body(code: symbol.code, name: symbol.name, market: symbol.marketPrefix,
                       pinned: symbol.pinned, group: symbol.group)
        )
    }

    static func deleteWatchSymbol(code: String) async throws {
        let request = try await dataRequest(path: "watchlist/\(code)", method: "DELETE")
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 204 else { throw GatewayError.badResponse }
    }

    private static func get<T: Decodable>(_ path: String) async throws -> T {
        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard enabled else { throw GatewayError.disabled }
        guard let url = URL(string: base),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              url.host != nil else { throw GatewayError.invalidURL }
        guard await circuit.canTry(base) else { throw GatewayError.unavailable }
        guard let endpoint = URL(string: base.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            + "/api/v1/" + path) else { throw GatewayError.invalidURL }
        do {
            let (data, response) = try await session.data(from: endpoint)
            guard let response = response as? HTTPURLResponse,
                  (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
            let result = try JSONDecoder().decode(T.self, from: data)
            await circuit.succeeded(base)
            return result
        } catch {
            await circuit.failed(base)
            throw error
        }
    }

    private static func getData<T: Decodable>(_ path: String) async throws -> T {
        let request = try await dataRequest(path: path, method: "GET")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(T.self, from: data)
    }

    private static func sendData<T: Encodable>(_ path: String, method: String, body: T) async throws {
        var request = try await dataRequest(path: path, method: method)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        request.httpBody = try encoder.encode(body)
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
    }

    private static func dataRequest(path: String, method: String) async throws -> URLRequest {
        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard enabled else { throw GatewayError.disabled }
        guard let endpoint = URL(string: base + "/api/v1/" + path),
              ["http", "https"].contains(endpoint.scheme?.lowercased() ?? ""),
              endpoint.host != nil else { throw GatewayError.invalidURL }
        var request = URLRequest(url: endpoint)
        request.httpMethod = method
        return request
    }

    private static func sourceName(_ source: String) -> String {
        switch source {
        case "sina": return "新浪"
        case "tencent": return "腾讯"
        case "eastmoney": return "东财"
        default: return source
        }
    }

    private static func chinaTime(_ timestamp: String, format: String) -> String {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = parser.date(from: timestamp) ?? {
            parser.formatOptions = [.withInternetDateTime]
            return parser.date(from: timestamp)
        }()
        guard let date else { return "--" }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = format
        return formatter.string(from: date)
    }
}

private actor GatewayCircuit {
    private var address = ""
    private var retryAfter = Date.distantPast

    func canTry(_ base: String) -> Bool {
        if address != base {
            address = base
            retryAfter = .distantPast
        }
        return Date() >= retryAfter
    }

    func failed(_ base: String) {
        guard address == base else { return }
        retryAfter = Date().addingTimeInterval(10)
    }

    func succeeded(_ base: String) {
        guard address == base else { return }
        retryAfter = .distantPast
    }
}
