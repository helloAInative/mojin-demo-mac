import Foundation

struct SignalEvent: Codable, Identifiable, Equatable {
    var id: UUID
    var at: Date
    var kind: String      // alert | ai | strategy | diverge | level | posAlert
    var title: String
    var body: String
    var code: String
    var price: Double
    var source: String
    var evidence: String
    var why: String
    /// 扩展元数据（levelKey/tier/levelPrice/hit/leadSec …）
    /// 旧数据缺字段时按空字典解码，保证向前兼容。
    var meta: [String: String] = [:]

    var clock: String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "MM-dd HH:mm:ss"
        return f.string(from: at)
    }

    var kindLabel: String {
        switch kind {
        case "ai": return "AI"
        case "strategy": return "策略"
        case "diverge": return "分歧"
        case "level": return "价位预警"
        case "posAlert": return "持仓预警"
        default: return "预警"
        }
    }

    enum CodingKeys: String, CodingKey {
        case id, at, kind, title, body, code, price, source, evidence, why, meta
    }

    init(id: UUID, at: Date, kind: String, title: String, body: String,
         code: String, price: Double, source: String, evidence: String,
         why: String = "", meta: [String: String] = [:]) {
        self.id = id
        self.at = at
        self.kind = kind
        self.title = title
        self.body = body
        self.code = code
        self.price = price
        self.source = source
        self.evidence = evidence
        self.why = why
        self.meta = meta
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.id        = try c.decode(UUID.self,   forKey: .id)
        self.at        = try c.decode(Date.self,   forKey: .at)
        self.kind      = try c.decode(String.self, forKey: .kind)
        self.title     = try c.decode(String.self, forKey: .title)
        self.body      = try c.decode(String.self, forKey: .body)
        self.code      = try c.decode(String.self, forKey: .code)
        self.price     = try c.decode(Double.self, forKey: .price)
        self.source    = try c.decode(String.self, forKey: .source)
        self.evidence  = try c.decode(String.self, forKey: .evidence)
        self.why       = (try? c.decode(String.self, forKey: .why)) ?? ""
        self.meta      = (try? c.decode([String: String].self, forKey: .meta)) ?? [:]
    }
}

@MainActor
enum SignalTimeline {
    private static let maxKeep = 200
    private static var cache: [SignalEvent]?

    private static func fileURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = base.appendingPathComponent("MojinPrince", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("signal-timeline.json")
    }

    static func load() -> [SignalEvent] {
        if let cache { return cache }
        guard let data = try? Data(contentsOf: fileURL()),
              let list = try? JSONDecoder().decode([SignalEvent].self, from: data) else {
            cache = []
            return []
        }
        cache = list
        return list
    }

    @discardableResult
    static func append(
        kind: String,
        title: String,
        body: String,
        code: String,
        price: Double,
        source: String,
        evidence: String,
        why: String = "",
        meta: [String: String] = [:]
    ) -> SignalEvent {
        var list = load()
        let ev = SignalEvent(
            id: UUID(),
            at: Date(),
            kind: kind,
            title: title,
            body: body,
            code: code,
            price: price,
            source: source,
            evidence: evidence,
            why: why,
            meta: meta
        )
        list.insert(ev, at: 0)
        if list.count > maxKeep { list = Array(list.prefix(maxKeep)) }
        cache = list
        if let data = try? JSONEncoder().encode(list) {
            try? data.write(to: fileURL(), options: .atomic)
        }
        Task { try? await GatewayMarketClient.putSignal(ev) }
        return ev
    }

    /// 局部更新某条事件的 meta（例如把 hit 写回），保持文件最新。
    static func updateMeta(id: UUID, mutating: (inout [String: String]) -> Void) {
        var list = load()
        guard let idx = list.firstIndex(where: { $0.id == id }) else { return }
        var m = list[idx].meta
        mutating(&m)
        list[idx].meta = m
        cache = list
        if let data = try? JSONEncoder().encode(list) {
            try? data.write(to: fileURL(), options: .atomic)
        }
        let updated = list[idx]
        Task { try? await GatewayMarketClient.putSignal(updated) }
    }

    /// 服务端与本地缓存按 id 合并；同 id 以服务端为准，断网时不触碰本地数据。
    static func mergeRemote(_ remote: [SignalEvent]) -> [SignalEvent] {
        var merged = Dictionary(uniqueKeysWithValues: load().map { ($0.id, $0) })
        for event in remote { merged[event.id] = event }
        let list = Array(merged.values.sorted { $0.at > $1.at }.prefix(maxKeep))
        cache = list
        if let data = try? JSONEncoder().encode(list) {
            try? data.write(to: fileURL(), options: .atomic)
        }
        return list
    }

    static func clear() {
        cache = []
        try? FileManager.default.removeItem(at: fileURL())
        Task { try? await GatewayMarketClient.deleteSignals() }
    }
}
