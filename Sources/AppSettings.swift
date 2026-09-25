import Foundation
import ServiceManagement
import SwiftUI

struct SymbolLevels: Codable, Equatable {
    var base: Double
    var support: Double
    var resistance: Double
}

struct WatchSymbol: Codable, Equatable, Identifiable, Hashable {
    var id: String { code }
    var code: String
    var name: String
    var pinned: Bool
    var group: String

    init(code: String, name: String, pinned: Bool = false, group: String = "默认") {
        self.code = code
        self.name = name
        self.pinned = pinned
        self.group = group.isEmpty ? "默认" : group
    }

    enum CodingKeys: String, CodingKey { case code, name, pinned, group }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = try c.decode(String.self, forKey: .code)
        name = try c.decode(String.self, forKey: .name)
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
        group = try c.decodeIfPresent(String.self, forKey: .group) ?? "默认"
    }

    var marketPrefix: String {
        let c = code.lowercased()
        if c.hasPrefix("sh") { return "sh" }
        if c.hasPrefix("bj") { return "bj" }
        return "sz"
    }

    var bareCode: String {
        let c = code.lowercased()
        if c.hasPrefix("sz") || c.hasPrefix("sh") || c.hasPrefix("bj") { return String(c.dropFirst(2)) }
        return c
    }

    var shortName: String {
        let n = name
        if n.count <= 2 { return n }
        return String(n.prefix(2))
    }

    var tencentCode: String { code.lowercased() }

    var eastMoneySecid: String {
        // 沪=1 深=0 北=0
        let p = marketPrefix == "sh" ? "1" : "0"
        return "\(p).\(bareCode)"
    }

    /// 规范化代码：300623 / sz300623 / 300623.SZ / SH600519
    static func normalizeCode(_ raw: String) -> String? {
        var c = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        c = c.replacingOccurrences(of: " ", with: "")
        c = c.replacingOccurrences(of: "　", with: "")
        if c.contains(".") {
            let parts = c.split(separator: ".").map(String.init)
            if parts.count == 2 {
                let a = parts[0], b = parts[1]
                if a.count == 6, a.allSatisfy(\.isNumber), ["sh", "sz", "bj"].contains(b) {
                    c = b + a
                } else if b.count == 6, b.allSatisfy(\.isNumber), ["sh", "sz", "bj"].contains(a) {
                    c = a + b
                }
            }
        }
        if c.hasPrefix("sh") || c.hasPrefix("sz") || c.hasPrefix("bj") {
            // ok
        } else if c.count == 6, c.allSatisfy(\.isNumber) {
            if c.hasPrefix("6") || c.hasPrefix("5") || c.hasPrefix("9") {
                c = "sh" + c
            } else if c.hasPrefix("4") || c.hasPrefix("8") {
                c = "bj" + c
            } else {
                c = "sz" + c
            }
        } else {
            return nil
        }
        guard c.count == 8 || c.count == 10 else { return nil }
        let prefix = String(c.prefix(2))
        guard ["sh", "sz", "bj"].contains(prefix) else { return nil }
        let bare = String(c.dropFirst(2))
        guard bare.count == 6, bare.allSatisfy(\.isNumber) else { return nil }
        return prefix + bare
    }

    var sinaCode: String { code.lowercased() }
}

struct PositionNote: Codable, Equatable {
    var cost: Double
    var shares: Double
    /// 止损价（与到价下联动可选）
    var stopLoss: Double
    /// 止盈价（与到价上联动可选；服务端 position.take_profit 已有列）
    var takeProfit: Double
    /// 计划仓位占总资金比例 %（笔记用）
    var positionPct: Double

    init(cost: Double = 0, shares: Double = 0, stopLoss: Double = 0,
         takeProfit: Double = 0, positionPct: Double = 0) {
        self.cost = cost
        self.shares = shares
        self.stopLoss = stopLoss
        self.takeProfit = takeProfit
        self.positionPct = positionPct
    }

    enum CodingKeys: String, CodingKey { case cost, shares, stopLoss, takeProfit, positionPct }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        cost = try c.decodeIfPresent(Double.self, forKey: .cost) ?? 0
        shares = try c.decodeIfPresent(Double.self, forKey: .shares) ?? 0
        stopLoss = try c.decodeIfPresent(Double.self, forKey: .stopLoss) ?? 0
        takeProfit = try c.decodeIfPresent(Double.self, forKey: .takeProfit) ?? 0
        positionPct = try c.decodeIfPresent(Double.self, forKey: .positionPct) ?? 0
    }
}

struct StrategyNote: Codable, Equatable {
    var above: Double
    var below: Double
    var drawdownPct: Double
    /// 回撤触发后冷静期（分钟），期内不再重复报警
    var coolDownMin: Int
    /// 止损价同步写入「到价下」
    var linkStopToBelow: Bool
    /// 止盈价同步写入「到价上」
    var linkTakeToAbove: Bool

    init(above: Double = 0, below: Double = 0, drawdownPct: Double = 0, coolDownMin: Int = 30,
         linkStopToBelow: Bool = true, linkTakeToAbove: Bool = true) {
        self.above = above
        self.below = below
        self.drawdownPct = drawdownPct
        self.coolDownMin = coolDownMin
        self.linkStopToBelow = linkStopToBelow
        self.linkTakeToAbove = linkTakeToAbove
    }

    enum CodingKeys: String, CodingKey { case above, below, drawdownPct, coolDownMin, linkStopToBelow, linkTakeToAbove }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        above = try c.decodeIfPresent(Double.self, forKey: .above) ?? 0
        below = try c.decodeIfPresent(Double.self, forKey: .below) ?? 0
        drawdownPct = try c.decodeIfPresent(Double.self, forKey: .drawdownPct) ?? 0
        coolDownMin = try c.decodeIfPresent(Int.self, forKey: .coolDownMin) ?? 30
        linkStopToBelow = try c.decodeIfPresent(Bool.self, forKey: .linkStopToBelow) ?? true
        linkTakeToAbove = try c.decodeIfPresent(Bool.self, forKey: .linkTakeToAbove) ?? true
    }
}

enum AppTheme: String, Codable, CaseIterable, Identifiable {
    case system = "跟随系统"
    case light = "浅色"
    case dark = "深色"
    var id: String { rawValue }
    var scheme: ColorScheme? {
        switch self {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }
}

enum MenuBarFormat: String, Codable, CaseIterable, Identifiable {
    case price = "只显示价"
    case pricePct = "价+涨跌幅"
    case shortName = "短名+价"
    case pctOnly = "只涨跌幅"
    case carousel = "自选轮播"
    var id: String { rawValue }
}

enum FontSizePref: String, Codable, CaseIterable, Identifiable {
    case sm = "小"
    case md = "中"
    case lg = "大"
    var id: String { rawValue }
    var scale: CGFloat {
        switch self {
        case .sm: return 0.92
        case .md: return 1.0
        case .lg: return 1.12
        }
    }
}

/// §I.4 盯盘密度模式：盘中默认专注，收盘后默认全量，用户可手动锁定。
/// `auto` 表示按当前是否为交易时段自动切换；
/// `focus` 仅报价头 / 分时 / 价位 / 持仓盈亏；`standard` 为现状；`full` 含资讯研报逐笔。
enum WatchDensity: String, Codable, CaseIterable, Identifiable {
    case auto = "自动"
    case focus = "专注"
    case standard = "标准"
    case full = "全量"
    var id: String { rawValue }

    /// 在 `auto` 模式下根据当前北京时间判定盘内 / 盘外。
    static func resolve(_ mode: WatchDensity) -> WatchDensity {
        if mode != .auto { return mode }
        let cn = TimeZone(identifier: "Asia/Shanghai") ?? .current
        let parts = Calendar(identifier: .gregorian).dateComponents(in: cn, from: Date())
        let weekday = parts.weekday ?? 1  // 1=周日
        if (2...6).contains(weekday) {
            let m = (parts.hour ?? 0) * 60 + (parts.minute ?? 0)
            let open = 9 * 60 + 30
            let close = 15 * 60
            return (open...close).contains(m) ? .focus : .full
        }
        return .full
    }
}

@MainActor
final class AppSettings: ObservableObject {
    static let shared = AppSettings()

    @Published var symbols: [WatchSymbol]
    @Published var currentCode: String
    @Published var levelsByCode: [String: SymbolLevels]
    @Published var positions: [String: PositionNote]
    @Published var strategies: [String: StrategyNote]
    @Published var comboStrategies: [ComboStrategy]
    @Published var diaries: [String: String]
    @Published var launchAtLogin: Bool
    @Published var alertsEnabled: Bool
    @Published var menuBarOnly: Bool
    @Published var openCloseBrief: Bool
    @Published var theme: AppTheme
    @Published var menuBarFormat: MenuBarFormat
    @Published var fontSize: FontSizePref
    @Published var indicator: IndicatorKind
    @Published var lastOpenBriefDay: String
    @Published var lastCloseBriefDay: String
    /// 收盘复盘通知当日去重（本地设备级，不参与网关同步）
    @Published var lastReviewNotifyDay: String
    @Published var notifyConfig: NotifyGovernor.Config
    @Published var seenChangelogVersion: String
    @Published var aiConfig: AIConfig
    /// UI 绑定用，保存时写入 Keychain
    @Published var aiAPIKey: String = ""
    /// 大盘可选指数
    @Published var boardShowHS300: Bool = false
    @Published var boardShowBJ50: Bool = false
    @Published var marketGatewayEnabled: Bool = true
    @Published var marketServerURL: String = "http://127.0.0.1:8732"
    /// §I.4 盯盘密度模式
    @Published var watchDensity: WatchDensity = .auto

    private let defaults = UserDefaults.standard
    private enum Key {
        static let symbols = "mojin.symbols"
        static let current = "mojin.currentCode"
        static let levels = "mojin.levels"
        static let pos = "mojin.positions"
        static let strat = "mojin.strategies"
        static let combo = "mojin.comboStrategies"
        static let diary = "mojin.diaries"
        static let launch = "mojin.launchAtLogin"
        static let alerts = "mojin.alertsEnabled"
        static let menuOnly = "mojin.menuBarOnly"
        static let brief = "mojin.openCloseBrief"
        static let theme = "mojin.theme"
        static let menuFmt = "mojin.menuBarFormat"
        static let font = "mojin.fontSize"
        static let ind = "mojin.indicator"
        static let openDay = "mojin.lastOpenBriefDay"
        static let closeDay = "mojin.lastCloseBriefDay"
        static let reviewNotifyDay = "mojin.lastReviewNotifyDay"
        static let notify = "mojin.notifyConfig"
        static let changelog = "mojin.seenChangelog"
        static let ai = "mojin.aiConfig"
        static let boardHS300 = "mojin.boardHS300"
        static let boardBJ50 = "mojin.boardBJ50"
        static let marketGatewayEnabled = "mojin.marketGatewayEnabled"
        static let marketServerURL = "mojin.marketServerURL"
        static let watchDensity = "mojin.watchDensity"
    }

    static let defaultSymbol = WatchSymbol(code: "sz300623", name: "捷捷微电", pinned: true)
    static let defaultLevels = SymbolLevels(base: 31.13, support: 29.4, resistance: 31.3)

    var currentSymbol: WatchSymbol {
        symbols.first(where: { $0.code == currentCode }) ?? Self.defaultSymbol
    }

    var levels: SymbolLevels {
        levelsByCode[currentCode] ?? Self.defaultLevels
    }

    var position: PositionNote {
        positions[currentCode] ?? PositionNote(cost: 0, shares: 0)
    }

    var strategy: StrategyNote {
        strategies[currentCode] ?? StrategyNote(above: 0, below: 0, drawdownPct: 0)
    }

    /// 置顶优先，再按列表顺序
    var sortedSymbols: [WatchSymbol] {
        let pinned = symbols.filter(\.pinned)
        let rest = symbols.filter { !$0.pinned }
        return pinned + rest
    }

    var groups: [String] {
        Array(Set(symbols.map(\.group))).sorted()
    }

    private init() {
        if let data = defaults.data(forKey: Key.symbols),
           let decoded = try? JSONDecoder().decode([WatchSymbol].self, from: data),
           !decoded.isEmpty {
            symbols = decoded
        } else {
            symbols = [
                Self.defaultSymbol,
                WatchSymbol(code: "sz000001", name: "平安银行", group: "银行"),
                WatchSymbol(code: "sh600519", name: "贵州茅台", group: "消费")
            ]
        }
        currentCode = defaults.string(forKey: Key.current) ?? Self.defaultSymbol.code
        levelsByCode = Self.decode(Key.levels, defaults) ?? [Self.defaultSymbol.code: Self.defaultLevels]
        positions = Self.decode(Key.pos, defaults) ?? [:]
        strategies = Self.decode(Key.strat, defaults) ?? [:]
        comboStrategies = Self.decode(Key.combo, defaults) ?? []
        diaries = Self.decode(Key.diary, defaults) ?? [:]
        launchAtLogin = defaults.object(forKey: Key.launch) as? Bool ?? false
        alertsEnabled = defaults.object(forKey: Key.alerts) as? Bool ?? true
        menuBarOnly = defaults.object(forKey: Key.menuOnly) as? Bool ?? true
        openCloseBrief = defaults.object(forKey: Key.brief) as? Bool ?? true
        theme = AppTheme(rawValue: defaults.string(forKey: Key.theme) ?? "") ?? .system
        menuBarFormat = MenuBarFormat(rawValue: defaults.string(forKey: Key.menuFmt) ?? "") ?? .pricePct
        fontSize = FontSizePref(rawValue: defaults.string(forKey: Key.font) ?? "") ?? .md
        indicator = IndicatorKind(rawValue: defaults.string(forKey: Key.ind) ?? "") ?? .macd
        lastOpenBriefDay = defaults.string(forKey: Key.openDay) ?? ""
        lastCloseBriefDay = defaults.string(forKey: Key.closeDay) ?? ""
        lastReviewNotifyDay = defaults.string(forKey: Key.reviewNotifyDay) ?? ""
        notifyConfig = Self.decode(Key.notify, defaults) ?? NotifyGovernor.Config()
        seenChangelogVersion = defaults.string(forKey: Key.changelog) ?? ""
        aiConfig = Self.decode(Key.ai, defaults) ?? AIConfig()
        aiAPIKey = KeychainStore.load()
        boardShowHS300 = defaults.object(forKey: Key.boardHS300) as? Bool ?? false
        boardShowBJ50 = defaults.object(forKey: Key.boardBJ50) as? Bool ?? false
        marketGatewayEnabled = defaults.object(forKey: Key.marketGatewayEnabled) as? Bool ?? true
        marketServerURL = defaults.string(forKey: Key.marketServerURL) ?? "http://127.0.0.1:8732"
        watchDensity = WatchDensity(rawValue: defaults.string(forKey: Key.watchDensity) ?? "") ?? .auto
        if levelsByCode[currentCode] == nil {
            levelsByCode[currentCode] = Self.defaultLevels
        }
        NotifyGovernor.shared.configure(notifyConfig)
    }

    private static func decode<T: Decodable>(_ key: String, _ defaults: UserDefaults) -> T? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private func persistLocal() {
        if let data = try? JSONEncoder().encode(symbols) { defaults.set(data, forKey: Key.symbols) }
        defaults.set(currentCode, forKey: Key.current)
        if let data = try? JSONEncoder().encode(levelsByCode) { defaults.set(data, forKey: Key.levels) }
        if let data = try? JSONEncoder().encode(positions) { defaults.set(data, forKey: Key.pos) }
        if let data = try? JSONEncoder().encode(strategies) { defaults.set(data, forKey: Key.strat) }
        if let data = try? JSONEncoder().encode(comboStrategies) { defaults.set(data, forKey: Key.combo) }
        if let data = try? JSONEncoder().encode(diaries) { defaults.set(data, forKey: Key.diary) }
        if let data = try? JSONEncoder().encode(notifyConfig) { defaults.set(data, forKey: Key.notify) }
        if let data = try? JSONEncoder().encode(aiConfig) { defaults.set(data, forKey: Key.ai) }
        KeychainStore.save(aiAPIKey)
        defaults.set(launchAtLogin, forKey: Key.launch)
        defaults.set(alertsEnabled, forKey: Key.alerts)
        defaults.set(menuBarOnly, forKey: Key.menuOnly)
        defaults.set(openCloseBrief, forKey: Key.brief)
        defaults.set(theme.rawValue, forKey: Key.theme)
        defaults.set(menuBarFormat.rawValue, forKey: Key.menuFmt)
        defaults.set(fontSize.rawValue, forKey: Key.font)
        defaults.set(indicator.rawValue, forKey: Key.ind)
        defaults.set(lastOpenBriefDay, forKey: Key.openDay)
        defaults.set(lastCloseBriefDay, forKey: Key.closeDay)
        defaults.set(lastReviewNotifyDay, forKey: Key.reviewNotifyDay)
        defaults.set(seenChangelogVersion, forKey: Key.changelog)
        defaults.set(boardShowHS300, forKey: Key.boardHS300)
        defaults.set(boardShowBJ50, forKey: Key.boardBJ50)
        defaults.set(marketGatewayEnabled, forKey: Key.marketGatewayEnabled)
        defaults.set(marketServerURL, forKey: Key.marketServerURL)
        defaults.set(watchDensity.rawValue, forKey: Key.watchDensity)
        NotifyGovernor.shared.configure(notifyConfig)
    }

    func save() {
        persistLocal()
        let snapshot = gatewaySnapshot()
        Task { try? await GatewayMarketClient.putSettings(snapshot) }
    }

    private func gatewaySnapshot() -> GatewaySettingsSnapshot {
        GatewaySettingsSnapshot(
            currentCode: currentCode,
            levelsByCode: levelsByCode,
            strategies: strategies,
            comboStrategies: comboStrategies,
            diaries: diaries,
            alertsEnabled: alertsEnabled,
            openCloseBrief: openCloseBrief,
            theme: theme,
            menuBarFormat: menuBarFormat,
            fontSize: fontSize,
            indicator: indicator.rawValue,
            notifyConfig: notifyConfig,
            aiConfig: aiConfig,
            boardShowHS300: boardShowHS300,
            boardShowBJ50: boardShowBJ50
        )
    }

    /// 启动时服务端优先；服务端为空则把现有本地缓存作为迁移种子上传。
    /// 网关不可用时直接返回，本地 UserDefaults 保持可用。
    func syncGatewayData() async {
        guard marketGatewayEnabled else { return }
        guard let remote = try? await GatewayMarketClient.remoteState() else { return }
        if remote.isEmpty {
            await seedGatewayFromLocal()
            return
        }

        if !remote.watchlist.isEmpty {
            symbols = remote.watchlist.map {
                WatchSymbol(code: $0.code, name: $0.name, pinned: $0.pinned, group: $0.group)
            }
        } else {
            for symbol in symbols { try? await GatewayMarketClient.putWatchSymbol(symbol) }
        }
        if !remote.positions.isEmpty {
            positions = Dictionary(uniqueKeysWithValues: remote.positions.map {
                ($0.code, PositionNote(cost: $0.cost, shares: $0.shares, stopLoss: $0.stopLoss,
                                       takeProfit: $0.takeProfit, positionPct: $0.positionPct))
            })
        } else {
            for (code, position) in positions {
                try? await GatewayMarketClient.putPosition(code: code, position: position)
            }
        }
        if remote.settings.isEmpty {
            try? await GatewayMarketClient.putSettings(gatewaySnapshot())
        } else {
            apply(remote.settings)
        }
        if !symbols.contains(where: { $0.code == currentCode }), let first = symbols.first {
            currentCode = first.code
        }
        persistLocal()
    }

    private func seedGatewayFromLocal() async {
        try? await GatewayMarketClient.putSettings(gatewaySnapshot())
        for symbol in symbols { try? await GatewayMarketClient.putWatchSymbol(symbol) }
        for (code, position) in positions {
            try? await GatewayMarketClient.putPosition(code: code, position: position)
        }
    }

    private func apply(_ remote: GatewaySettingsSnapshot) {
        if let value = remote.currentCode { currentCode = value }
        if let value = remote.levelsByCode { levelsByCode = value }
        if let value = remote.strategies { strategies = value }
        if let value = remote.comboStrategies { comboStrategies = value }
        if let value = remote.diaries { diaries = value }
        if let value = remote.alertsEnabled { alertsEnabled = value }
        if let value = remote.openCloseBrief { openCloseBrief = value }
        if let value = remote.theme { theme = value }
        if let value = remote.menuBarFormat { menuBarFormat = value }
        if let value = remote.fontSize { fontSize = value }
        if let value = remote.indicator, let parsed = IndicatorKind(rawValue: value) { indicator = parsed }
        if let value = remote.notifyConfig { notifyConfig = value }
        if let value = remote.aiConfig { aiConfig = value }
        if let value = remote.boardShowHS300 { boardShowHS300 = value }
        if let value = remote.boardShowBJ50 { boardShowBJ50 = value }
    }

    func setBoardShowHS300(_ on: Bool) {
        boardShowHS300 = on
        save()
    }

    func setBoardShowBJ50(_ on: Bool) {
        boardShowBJ50 = on
        save()
    }

    func setMarketGateway(enabled: Bool, address: String) {
        marketGatewayEnabled = enabled
        marketServerURL = address.trimmingCharacters(in: .whitespacesAndNewlines)
        save()
    }

    func updateLevels(base: Double, support: Double, resistance: Double) {
        levelsByCode[currentCode] = SymbolLevels(base: base, support: support, resistance: resistance)
        save()
    }

    func updatePosition(cost: Double, shares: Double, stopLoss: Double = 0,
                        takeProfit: Double = 0, positionPct: Double = 0) {
        let note = PositionNote(cost: cost, shares: shares, stopLoss: stopLoss,
                                takeProfit: takeProfit, positionPct: positionPct)
        positions[currentCode] = note
        // 止损联动到价下、止盈联动到价上（对称）
        var st = strategies[currentCode] ?? StrategyNote()
        var stDirty = false
        if st.linkStopToBelow, stopLoss > 0, st.below != stopLoss {
            st.below = stopLoss
            stDirty = true
        }
        if st.linkTakeToAbove, takeProfit > 0, st.above != takeProfit {
            st.above = takeProfit
            stDirty = true
        }
        if stDirty { strategies[currentCode] = st }
        save()
        let code = currentCode
        Task { try? await GatewayMarketClient.putPosition(code: code, position: note) }
    }

    func updateStrategy(above: Double, below: Double, drawdownPct: Double, coolDownMin: Int = 30,
                        linkStopToBelow: Bool = true, linkTakeToAbove: Bool = true) {
        strategies[currentCode] = StrategyNote(
            above: above, below: below, drawdownPct: drawdownPct,
            coolDownMin: coolDownMin, linkStopToBelow: linkStopToBelow, linkTakeToAbove: linkTakeToAbove
        )
        save()
    }

    func diaryKey(code: String? = nil, day: String) -> String {
        (code ?? currentCode) + "|" + day
    }

    func diaryText(day: String, code: String? = nil) -> String {
        diaries[diaryKey(code: code, day: day)] ?? ""
    }

    func setDiary(day: String, text: String, code: String? = nil) {
        diaries[diaryKey(code: code, day: day)] = text
        save()
    }

    /// 按标的检索复盘日记，最近优先
    func searchDiaries(code: String? = nil, query: String = "", limit: Int = 12) -> [(day: String, code: String, text: String)] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let want = (code ?? "").lowercased()
        var rows: [(day: String, code: String, text: String)] = []
        for (k, text) in diaries {
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { continue }
            let parts = k.split(separator: "|", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { continue }
            let c = parts[0], day = parts[1]
            if !want.isEmpty, c != want { continue }
            if !q.isEmpty, !text.lowercased().contains(q), !day.contains(q), !c.contains(q) { continue }
            rows.append((day, c, text))
        }
        rows.sort { $0.day > $1.day }
        return Array(rows.prefix(limit))
    }

    func appendDiaryNote(day: String, note: String, code: String? = nil) {
        let cur = diaryText(day: day, code: code)
        let stamp = {
            let f = DateFormatter()
            f.locale = Locale(identifier: "en_US_POSIX")
            f.timeZone = TimeZone(identifier: "Asia/Shanghai")
            f.dateFormat = "HH:mm"
            return f.string(from: Date())
        }()
        let line = "[\(stamp)] \(note)"
        setDiary(day: day, text: cur.isEmpty ? line : cur + "\n" + line, code: code)
    }

    func selectSymbol(_ code: String) {
        currentCode = code.lowercased()
        if levelsByCode[currentCode] == nil {
            levelsByCode[currentCode] = SymbolLevels(base: 0, support: 0, resistance: 0)
        }
        save()
    }

    enum AddSymbolOutcome: Equatable {
        case added(String)
        case exists(String)
        case invalid
    }

    struct BatchImportResult: Equatable {
        var added: [String]
        var exists: [String]
        var invalid: [String]
        var line: String {
            "新增\(added.count) · 已有\(exists.count) · 无效\(invalid.count)"
        }
    }

    @discardableResult
    func addSymbol(code raw: String, name: String, group: String = "默认") -> AddSymbolOutcome {
        guard let c = WatchSymbol.normalizeCode(raw) else { return .invalid }
        let g = group.trimmingCharacters(in: .whitespacesAndNewlines)
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if let exist = symbols.first(where: { $0.code == c }) {
            selectSymbol(c)
            return .exists(exist.name.isEmpty ? c : exist.name)
        }
        symbols.append(WatchSymbol(code: c, name: n.isEmpty ? c : n, group: g.isEmpty ? "默认" : g))
        let added = symbols.last!
        selectSymbol(c)
        save()
        Task { try? await GatewayMarketClient.putWatchSymbol(added) }
        return .added(c)
    }

    /// 批量导入：每行一个代码，可选 `代码,名称,分组` 或空格分隔
    func batchImportSymbols(_ text: String, defaultGroup: String = "默认") -> BatchImportResult {
        var added: [String] = [], exists: [String] = [], invalid: [String] = []
        let lines = text.components(separatedBy: CharacterSet.newlines)
        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty, !trimmed.hasPrefix("#") else { continue }
            let parts = trimmed
                .replacingOccurrences(of: "，", with: ",")
                .replacingOccurrences(of: "\t", with: ",")
                .components(separatedBy: CharacterSet(charactersIn: ", "))
                .map { $0.trimmingCharacters(in: .whitespaces) }
                .filter { !$0.isEmpty }
            guard let raw = parts.first else { continue }
            let name = parts.count > 1 ? parts[1] : ""
            let group = parts.count > 2 ? parts[2] : defaultGroup
            switch addSymbol(code: raw, name: name, group: group) {
            case .added(let c): added.append(c)
            case .exists(let n): exists.append(n)
            case .invalid: invalid.append(raw)
            }
        }
        return BatchImportResult(added: added, exists: exists, invalid: invalid)
    }

    func removeSymbol(_ code: String) {
        guard symbols.count > 1 else { return }
        let c = code.lowercased()
        guard symbols.contains(where: { $0.code == c }) else { return }
        symbols.removeAll { $0.code == c }
        if currentCode == c {
            currentCode = symbols[0].code
        }
        save()
        Task { try? await GatewayMarketClient.deleteWatchSymbol(code: c) }
    }

    func removeCurrentIfPossible() {
        removeSymbol(currentCode)
    }

    /// 成交回写持仓笔记（半自动勾选）
    func applyFillToPosition(side: OrderSide, price: Double, quantity: Int, code: String? = nil) {
        let c = (code ?? currentCode).lowercased()
        var pos = positions[c] ?? PositionNote()
        let q = Double(max(0, quantity))
        guard q > 0, price > 0 else { return }
        switch side {
        case .buy:
            let oldCost = pos.cost
            let oldShares = pos.shares
            let newShares = oldShares + q
            if newShares > 0 {
                pos.cost = (oldCost * oldShares + price * q) / newShares
            }
            pos.shares = newShares
        case .sell:
            pos.shares = max(0, pos.shares - q)
            if pos.shares <= 0 {
                pos.shares = 0
                pos.cost = 0
            }
        }
        positions[c] = pos
        save()
        Task { try? await GatewayMarketClient.putPosition(code: c, position: pos) }
    }

    func moveWithinGroup(_ group: String, from: IndexSet, to: Int) {
        var codes = symbols.filter { $0.group == group }.map(\.code)
        codes.move(fromOffsets: from, toOffset: to)
        var rebuilt: [WatchSymbol] = []
        var gi = 0
        for s in symbols {
            if s.group == group {
                if gi < codes.count, let ns = symbols.first(where: { $0.code == codes[gi] }) {
                    rebuilt.append(ns)
                }
                gi += 1
            } else {
                rebuilt.append(s)
            }
        }
        symbols = rebuilt
        save()
    }

    func moveSymbol(from: IndexSet, to: Int) {
        symbols.move(fromOffsets: from, toOffset: to)
        save()
    }

    func togglePin(_ code: String) {
        guard let i = symbols.firstIndex(where: { $0.code == code }) else { return }
        symbols[i].pinned.toggle()
        let symbol = symbols[i]
        save()
        Task { try? await GatewayMarketClient.putWatchSymbol(symbol) }
    }

    func setGroup(_ code: String, group: String) {
        guard let i = symbols.firstIndex(where: { $0.code == code }) else { return }
        symbols[i].group = group.isEmpty ? "默认" : group
        let symbol = symbols[i]
        save()
        Task { try? await GatewayMarketClient.putWatchSymbol(symbol) }
    }

    func upsertCombo(_ rule: ComboStrategy) {
        if let i = comboStrategies.firstIndex(where: { $0.id == rule.id }) {
            comboStrategies[i] = rule
        } else {
            comboStrategies.append(rule)
        }
        save()
    }

    func removeCombo(_ id: UUID) {
        comboStrategies.removeAll { $0.id == id }
        save()
    }

    func setNotifyConfig(_ c: NotifyGovernor.Config) {
        notifyConfig = c
        save()
    }

    func setAIConfig(_ c: AIConfig) {
        aiConfig = c
        save()
    }

    func setAIModel(_ id: String) {
        var c = aiConfig
        c.model = id
        aiConfig = c
        save()
    }

    func setAIAPIKey(_ key: String) {
        aiAPIKey = key
        KeychainStore.save(key)
    }

    func markChangelogSeen() {
        seenChangelogVersion = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "1.4"
        save()
    }

    var needsChangelog: Bool {
        let v = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? ""
        return !v.isEmpty && v != seenChangelogVersion
    }

    func setLaunchAtLogin(_ on: Bool) {
        launchAtLogin = on
        save()
        do {
            if on { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
        } catch {}
    }

    func setAlertsEnabled(_ on: Bool) { alertsEnabled = on; save() }
    func setMenuBarOnly(_ on: Bool) { menuBarOnly = on; save() }
    func setWatchDensity(_ d: WatchDensity) { watchDensity = d; save() }
    func setOpenCloseBrief(_ on: Bool) { openCloseBrief = on; save() }
    func setTheme(_ t: AppTheme) { theme = t; save() }
    func setMenuBarFormat(_ f: MenuBarFormat) { menuBarFormat = f; save() }
    func setFontSize(_ f: FontSizePref) { fontSize = f; save() }
    func setIndicator(_ k: IndicatorKind) { indicator = k; save() }
}

enum TradingSession {
    case open, lunch, closed

    static func current(now: Date = Date()) -> TradingSession {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Asia/Shanghai") ?? .current
        let wd = cal.component(.weekday, from: now)
        if wd == 1 || wd == 7 { return .closed }
        let mins = cal.component(.hour, from: now) * 60 + cal.component(.minute, from: now)
        if (mins >= 9 * 60 + 15 && mins <= 11 * 60 + 30) || (mins >= 13 * 60 && mins <= 15 * 60 + 5) {
            return .open
        }
        if mins > 11 * 60 + 30 && mins < 13 * 60 { return .lunch }
        return .closed
    }

    static func shanghaiMinutes(_ now: Date = Date()) -> Int {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Asia/Shanghai") ?? .current
        return cal.component(.hour, from: now) * 60 + cal.component(.minute, from: now)
    }

    static func isShanghaiWeekday(_ now: Date = Date()) -> Bool {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "Asia/Shanghai") ?? .current
        let wd = cal.component(.weekday, from: now)
        return wd != 1 && wd != 7
    }

    var quoteInterval: UInt64 {
        switch self {
        case .open: return 3_000_000_000
        case .lunch: return 30_000_000_000
        case .closed: return 120_000_000_000
        }
    }

    var minuteInterval: UInt64 {
        switch self {
        case .open: return 15_000_000_000
        case .lunch: return 60_000_000_000
        case .closed: return 300_000_000_000
        }
    }

    var dailyInterval: UInt64 {
        switch self {
        case .open: return 60_000_000_000
        case .lunch: return 120_000_000_000
        case .closed: return 600_000_000_000
        }
    }

    var label: String {
        switch self {
        case .open: return "盘中"
        case .lunch: return "午休"
        case .closed: return "休市"
        }
    }
}
