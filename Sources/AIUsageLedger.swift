import Foundation

struct AIUsageEntry: Codable, Identifiable, Equatable {
    var id: UUID
    var at: Date
    var model: String
    var ok: Bool
    var fallback: Bool
    var elapsedMs: Int
    /// 粗算「Credits 感」：输出字数/100 + 固定起步，仅作体感非账单
    var creditFeel: Double
    var note: String
    /// D.4：失败原因（ok=false 时截取错误前 60 字），旧数据解码容错
    var failureReason: String = ""

    var clock: String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "HH:mm:ss"
        return f.string(from: at)
    }
}

@MainActor
enum AIUsageLedger {
    private static let maxKeep = 100
    private static var cache: [AIUsageEntry]?

    private static func fileURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = base.appendingPathComponent("MojinPrince", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("ai-usage.json")
    }

    static func load() -> [AIUsageEntry] {
        if let cache { return cache }
        guard let data = try? Data(contentsOf: fileURL()),
              let list = try? JSONDecoder().decode([AIUsageEntry].self, from: data) else {
            cache = []
            return []
        }
        cache = list
        return list
    }

    @discardableResult
    static func append(model: String, ok: Bool, fallback: Bool, elapsedMs: Int, outputChars: Int,
                       note: String, failureReason: String = "") -> AIUsageEntry {
        let feel = 0.3 + Double(max(outputChars, 1)) / 100.0 + (fallback ? 0 : 0.2)
        var list = load()
        let e = AIUsageEntry(
            id: UUID(), at: Date(), model: model, ok: ok, fallback: fallback,
            elapsedMs: elapsedMs, creditFeel: feel, note: note, failureReason: failureReason
        )
        list.insert(e, at: 0)
        if list.count > maxKeep { list = Array(list.prefix(maxKeep)) }
        cache = list
        if let data = try? JSONEncoder().encode(list) {
            try? data.write(to: fileURL(), options: .atomic)
        }
        return e
    }

    static func todayFeel() -> Double {
        let day = {
            let f = DateFormatter()
            f.locale = Locale(identifier: "en_US_POSIX")
            f.timeZone = TimeZone(identifier: "Asia/Shanghai")
            f.dateFormat = "yyyy-MM-dd"
            return f.string(from: Date())
        }()
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"
        return load().filter { f.string(from: $0.at) == day }.reduce(0) { $0 + $1.creditFeel }
    }

    static func clear() {
        cache = []
        try? FileManager.default.removeItem(at: fileURL())
    }
}

struct SourceProbe: Equatable, Identifiable {
    var id: String { name }
    var name: String
    var ok: Bool
    var latencyMs: Int
    var price: Double
    var detail: String
}

struct HealthReport: Equatable {
    var at: Date
    var probes: [SourceProbe]
    var failStreak: Int
    var cacheAgeSec: Int?
    var cacheCode: String
    var liveOK: Bool
    var usingCache: Bool
    var indexProbes: [SourceProbe] = []
    var indexFailStreak: Int = 0
    var indexCacheAgeSec: Int? = nil

    var line: String {
        let okN = probes.filter(\.ok).count
        let idxOK = indexProbes.filter(\.ok).count
        let cache = cacheAgeSec.map { "个股缓存\($0)s" } ?? "个股无缓存"
        let icache = indexCacheAgeSec.map { "指数缓存\($0)s" } ?? "指数无缓存"
        let idx = indexProbes.isEmpty ? "" : " · 大盘\(idxOK)/\(indexProbes.count)"
        return "源\(okN)/\(probes.count)\(idx) · 失败连\(failStreak) · \(cache) · \(icache)"
    }
}
