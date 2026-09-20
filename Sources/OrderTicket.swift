import Foundation
import AppKit

enum OrderSide: String, Codable, CaseIterable, Identifiable {
    case buy = "买入"
    case sell = "卖出"
    var id: String { rawValue }
}

enum OrderPriceMode: String, Codable, CaseIterable, Identifiable {
    case limit = "限价"
    case last = "现价"
    case bidAsk = "对手价感"
    var id: String { rawValue }
}

/// 半自动委托条：只生成国盛可照抄的委托，不下真单
struct SemiOrderTicket: Codable, Identifiable, Equatable {
    var id: UUID
    var at: Date
    var code: String
    var name: String
    var side: OrderSide
    var price: Double
    var quantity: Int
    var priceMode: OrderPriceMode
    var note: String
    var copied: Bool
    var brokerHint: String
    /// 用户勾选「已在国盛成交」
    var filled: Bool
    var source: String

    var bareCode: String {
        WatchSymbol(code: code, name: name).bareCode
    }

    var amount: Double { price * Double(max(0, quantity)) }

    var lotOK: Bool {
        guard quantity > 0 else { return false }
        if code.lowercased().hasPrefix("sh688") {
            return quantity % 200 == 0
        }
        return quantity % 100 == 0
    }

    /// 国盛/同花顺照抄文本
    var clipboardText: String {
        """
        【半自动委托 · 请在国盛客户端确认】
        方向：\(side.rawValue)
        代码：\(bareCode)
        名称：\(name.isEmpty ? code : name)
        价格：\(String(format: "%.2f", price))（\(priceMode.rawValue)）
        数量：\(quantity) 股
        约额：\(String(format: "%.0f", amount)) 元
        \(note.isEmpty ? "" : "备注：\(note)\n")来源：\(source.isEmpty ? "手动" : source)
        通道：\(brokerHint) · 本 App 不报单
        """
    }

    var oneLine: String {
        "\(side.rawValue) \(bareCode) \(quantity)股 @\(String(format: "%.2f", price))"
    }

    static func normalizeLot(_ qty: Int, code: String) -> Int {
        let step = code.lowercased().hasPrefix("sh688") ? 200 : 100
        guard qty > 0 else { return step }
        return max(step, (qty / step) * step)
    }

    enum CodingKeys: String, CodingKey {
        case id, at, code, name, side, price, quantity, priceMode, note, copied, brokerHint, filled, source
    }

    init(
        id: UUID = UUID(), at: Date = Date(), code: String, name: String,
        side: OrderSide, price: Double, quantity: Int, priceMode: OrderPriceMode,
        note: String, copied: Bool, brokerHint: String, filled: Bool = false, source: String = ""
    ) {
        self.id = id; self.at = at; self.code = code; self.name = name
        self.side = side; self.price = price; self.quantity = quantity
        self.priceMode = priceMode; self.note = note; self.copied = copied
        self.brokerHint = brokerHint; self.filled = filled; self.source = source
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        at = try c.decode(Date.self, forKey: .at)
        code = try c.decode(String.self, forKey: .code)
        name = try c.decode(String.self, forKey: .name)
        side = try c.decode(OrderSide.self, forKey: .side)
        price = try c.decode(Double.self, forKey: .price)
        quantity = try c.decode(Int.self, forKey: .quantity)
        priceMode = try c.decode(OrderPriceMode.self, forKey: .priceMode)
        note = try c.decode(String.self, forKey: .note)
        copied = try c.decode(Bool.self, forKey: .copied)
        brokerHint = try c.decode(String.self, forKey: .brokerHint)
        filled = try c.decodeIfPresent(Bool.self, forKey: .filled) ?? false
        source = try c.decodeIfPresent(String.self, forKey: .source) ?? ""
    }
}

@MainActor
enum OrderTicketLedger {
    private static let maxKeep = 80
    private static var cache: [SemiOrderTicket]?

    private static func fileURL() -> URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = base.appendingPathComponent("MojinPrince", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("semi-orders.json")
    }

    static func load() -> [SemiOrderTicket] {
        if let cache { return cache }
        guard let data = try? Data(contentsOf: fileURL()),
              let list = try? JSONDecoder().decode([SemiOrderTicket].self, from: data) else {
            cache = []
            return []
        }
        cache = list
        return list
    }

    private static func persist(_ list: [SemiOrderTicket]) {
        cache = list
        if let data = try? JSONEncoder().encode(list) {
            try? data.write(to: fileURL(), options: .atomic)
        }
    }

    @discardableResult
    static func append(_ ticket: SemiOrderTicket) -> SemiOrderTicket {
        var list = load()
        list.insert(ticket, at: 0)
        if list.count > maxKeep { list = Array(list.prefix(maxKeep)) }
        persist(list)
        return ticket
    }

    static func update(_ ticket: SemiOrderTicket) {
        var list = load()
        if let i = list.firstIndex(where: { $0.id == ticket.id }) {
            list[i] = ticket
            persist(list)
        }
    }

    static func clear() {
        cache = []
        try? FileManager.default.removeItem(at: fileURL())
    }
}
