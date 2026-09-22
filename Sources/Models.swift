import Foundation

struct Quote: Equatable, Codable {
    var name: String = ""
    var price: Double = 0
    var prev: Double = 0
    var open: Double = 0
    var high: Double = 0
    var low: Double = 0
    var change: Double = 0
    var pct: Double = 0
    var timeText: String = "--"
    var source: String = ""
}

/// 盯盘顶部大盘指数一行
struct IndexQuote: Identifiable, Equatable {
    var id: String { code }
    var code: String
    var shortName: String
    var quote: Quote
}

struct MinuteBar: Identifiable, Equatable, Codable {
    var id: String { minute }
    var minute: String
    var price: Double
    var avg: Double
    var vol: Double
}

struct DayBar: Identifiable, Equatable, Codable {
    var id: String { date }
    var date: String
    var open: Double = 0
    var close: Double
    var high: Double = 0
    var low: Double = 0
    var volume: Double = 0
}

/// B.6 热力图单元格数据。
struct WatchHeatCell: Identifiable, Equatable {
    var code: String
    var name: String
    var pct: Double
    var id: String { code }
}

enum MarketError: Error {
    case badData
}

/// AI 最近一次分析的结构化结论。仅暴露给 UI 画标记用，
/// 文本版展示仍走 aiText。
struct AILatestSignal: Equatable {
    enum Verdict: String { case bull, bear, range }

    var verdict: Verdict = .range
    var summary: String = ""
    var keyPoints: [String] = []
    var risk: String = ""

    /// AI 给出的关键价位（基准 / 支撑 / 阻力 / 止损）。
    /// 数值 <= 0 视为未给出。
    var base: Double = 0
    var support: Double = 0
    var resistance: Double = 0
    var stop: Double = 0

    /// AI 给出的事件流（突破 / 假突破 / 量能异动 / 政策命中）。
    /// 价 0 或方向缺失会被 UI 忽略。
    var events: [AIEvent] = []

    /// 警戒线（量比阈值等），画在 VolumeChart 顶部。
    /// volumeRatio > 0 视为有效；UI 会按当前 maxV 自适应。
    var volumeWarnRatio: Double = 0

    var updatedAt: Date = Date()

    /// 走势图标需要的「侧 + 价位 + 标签」。
    var markers: [AIMarker] {
        var out: [AIMarker] = []
        if resistance > 0 { out.append(.init(side: .above, price: resistance, label: "AI 阻")) }
        if support > 0   { out.append(.init(side: .below, price: support,   label: "AI 支")) }
        if stop > 0      { out.append(.init(side: .below, price: stop,      label: "AI 止")) }
        if base > 0      { out.append(.init(side: .above, price: base,      label: "AI 基")) }
        return out
    }
}

/// 走势图上的标记点：方向 + 价格 + 文字标签。
struct AIMarker: Equatable, Hashable {
    enum Side { case above, below }
    var side: Side
    var price: Double
    var label: String
}

/// AI 给出的盘中事件（画在分时图上）。
struct AIEvent: Equatable, Hashable {
    enum Kind: String, Codable {
        case breakout    // 突破
        case breakdown   // 跌破
        case volSpike    // 量能异动
        case fakeout     // 假突破
        case reversal    // 反转
    }
    enum Dir: String, Codable { case up, down }

    var kind: Kind
    var dir: Dir
    /// 分时分钟数（距 9:30 的偏移，0..=240）。
    /// 找不到对应 bar 时由 UI 决定是否回落到「现在」。
    var minuteOffset: Int = 0
    var price: Double = 0
    var note: String = ""

    /// 给图表用的箭头方向 + 标签。
    var marker: AIMarker {
        let isUp = dir == .up
        return .init(side: isUp ? .above : .below,
                     price: price,
                     label: label)
    }

    var label: String {
        switch kind {
        case .breakout:  return isUpArrow ? "突" : "跌"
        case .breakdown: return "破"
        case .volSpike:  return "量"
        case .fakeout:   return "假"
        case .reversal:  return isUpArrow ? "反" : "反"
        }
    }

    var isUpArrow: Bool { dir == .up }
}

struct Snapshot: Codable {
    var quote: Quote
    var minutes: [MinuteBar]
    var days: [DayBar]
    var savedAt: Date
}

enum SnapshotCache {
    private static func dir() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let d = base.appendingPathComponent("MojinPrince/cache", isDirectory: true)
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    private static func file(_ code: String) -> URL {
        dir().appendingPathComponent(code.replacingOccurrences(of: "/", with: "_") + ".json")
    }

    static func save(code: String, quote: Quote, minutes: [MinuteBar], days: [DayBar]) {
        let snap = Snapshot(quote: quote, minutes: minutes, days: days, savedAt: Date())
        guard let data = try? JSONEncoder().encode(snap) else { return }
        try? data.write(to: file(code), options: .atomic)
    }

    static func load(code: String) -> Snapshot? {
        guard let data = try? Data(contentsOf: file(code)) else { return nil }
        return try? JSONDecoder().decode(Snapshot.self, from: data)
    }

    static func cacheAgeSec(code: String) -> Int? {
        guard let snap = load(code: code) else { return nil }
        return max(0, Int(Date().timeIntervalSince(snap.savedAt)))
    }

    static func saveDays(code: String, days: [DayBar], quote: Quote = Quote(), minutes: [MinuteBar] = []) {
        let existing = load(code: code)
        save(
            code: code,
            quote: quote.price > 0 ? quote : (existing?.quote ?? Quote()),
            minutes: minutes.isEmpty ? (existing?.minutes ?? []) : minutes,
            days: days
        )
    }
}
