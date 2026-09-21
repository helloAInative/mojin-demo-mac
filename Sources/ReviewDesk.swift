import Foundation

/// 复盘工作台：按日拼接信号 + 委托照抄 + 日记
enum TradeStoryDesk {
    static func dayString(_ date: Date = Date()) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: date)
    }

    static func clock(_ date: Date) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "HH:mm"
        return f.string(from: date)
    }

    static func build(
        day: String,
        code: String,
        name: String,
        signals: [SignalEvent],
        tickets: [SemiOrderTicket],
        diary: String
    ) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "Asia/Shanghai")
        f.dateFormat = "yyyy-MM-dd"

        let sigs = signals.filter { f.string(from: $0.at) == day && (code.isEmpty || $0.code == code) }
            .sorted { $0.at < $1.at }
        let ords = tickets.filter { f.string(from: $0.at) == day && (code.isEmpty || $0.code == code) }
            .sorted { $0.at < $1.at }

        var lines: [String] = []
        lines.append("## 交易故事 · \(day) · \(name.isEmpty ? code : name) (\(code))")
        lines.append("")

        if !diary.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            lines.append("### 日记")
            lines.append(diary.trimmingCharacters(in: .whitespacesAndNewlines))
            lines.append("")
        }

        if !sigs.isEmpty {
            lines.append("### 信号时间线")
            for s in sigs {
                lines.append("- \(clock(s.at)) [\(s.kind)] \(s.title) — \(s.body)")
                if !s.why.isEmpty {
                    lines.append("  为何：\(s.why)")
                }
            }
            lines.append("")
        }

        if !ords.isEmpty {
            lines.append("### 半自动委托照抄")
            for o in ords {
                let flag = o.filled ? "已勾成交" : (o.copied ? "已复制" : "草稿")
                lines.append("- \(clock(o.at)) \(o.oneLine) · \(flag)")
                if !o.note.isEmpty { lines.append("  注：\(o.note)") }
            }
            lines.append("")
        }

        if diary.isEmpty && sigs.isEmpty && ords.isEmpty {
            lines.append("_当日暂无日记 / 信号 / 委托记录_")
        } else {
            lines.append("---")
            lines.append("共 \(sigs.count) 条信号 · \(ords.count) 条委托 · 日记\(diary.isEmpty ? "无" : "有")")
        }
        return lines.joined(separator: "\n")
    }
}

// ============================================================================
// 阶段 4：与后端 `/api/v1/reviews*` 联调的 DTO + ReportClient
// ============================================================================

/// 服务端返回的报告（camelCase 反序列化）。
struct ScheduledReport: Codable, Equatable, Identifiable {
    var id: String
    var kind: String
    var periodKey: String
    var title: String
    var body: String
    var payload: ReportPayload?
    var createdAt: Date
}

/// payload 里客户端关心的子结构（context / summary）。
struct ReportPayload: Codable, Equatable {
    var context: ReportContextPayload?
    var summary: ReportSummary?
}

struct ReportContextPayload: Codable, Equatable {
    var diary: String?
    var tickets: [TicketPayload]?
    var signals: [ClientSignalPayload]?
    var focusCodes: [String]?
}

struct TicketPayload: Codable, Equatable {
    var code: String
    var at: Date
    var summary: String
    var status: String
    var note: String?
    /// buy / sell（供服务端复盘做止损止盈执行对照）
    var side: String?
    var price: Double?
}

struct ClientSignalPayload: Codable, Equatable {
    var at: Date
    var kind: String
    var code: String
    var title: String
    var body: String?
    var why: String?
}

struct ReportSummary: Codable, Equatable {
    var signalsTotal: Int?
    var levelHit: Int?
    var levelTotal: Int?
    var aiCallsSuccess: Int?
    var aiCallsTotal: Int?
    var tokensIn: Int?
    var tokensOut: Int?
    var costUsd: Double?
}

/// 客户端 POST 给后端的上下文（camelCase 编码，匹配 Rust serde）。
struct ReviewContextPayload: Codable, Equatable {
    var diary: String?
    var tickets: [TicketPayload]
    var signals: [ClientSignalPayload]
    var focusCodes: [String]

    static func from(
        diary: String?,
        tickets: [SemiOrderTicket],
        signals: [SignalEvent]
    ) -> ReviewContextPayload {
        let ticketDTOs: [TicketPayload] = tickets.map { t in
            let status: String = t.filled ? "filled" : (t.copied ? "copied" : "draft")
            return TicketPayload(
                code: t.code,
                at: t.at,
                summary: t.oneLine,
                status: status,
                note: t.note.isEmpty ? nil : t.note,
                side: t.side == .sell ? "sell" : "buy",
                price: t.price
            )
        }
        let signalDTOs: [ClientSignalPayload] = signals.map { s in
            ClientSignalPayload(
                at: s.at,
                kind: s.kind,
                code: s.code,
                title: s.title,
                body: s.body.isEmpty ? nil : s.body,
                why: s.why.isEmpty ? nil : s.why
            )
        }
        let codes = Set(tickets.map(\.code)).union(signals.map(\.code))
        return ReviewContextPayload(
            diary: diary?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == true ? nil : diary,
            tickets: ticketDTOs,
            signals: signalDTOs,
            focusCodes: Array(codes).sorted()
        )
    }
}

enum ReviewKind: String, Codable {
    case daily
    case weekly
}

/// 调用后端 `/api/v1/reviews*` 的客户端。
final class ReportClient {
    let baseURL: URL
    let session: URLSession

    init(baseURL: URL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// 拉取历史报告（按 kind 过滤，可选 limit）。
    func list(kind: ReviewKind? = nil, limit: Int = 20) async throws -> [ScheduledReport] {
        var components = URLComponents(
            url: baseURL.appendingPathComponent("/api/v1/reviews"),
            resolvingAgainstBaseURL: false
        )!
        var items: [URLQueryItem] = [URLQueryItem(name: "limit", value: String(limit.clamped(1, 200)))]
        if let kind {
            items.append(URLQueryItem(name: "kind", value: kind.rawValue))
        }
        components.queryItems = items
        var req = URLRequest(url: components.url!)
        req.httpMethod = "GET"
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: req)
        try Self.check(response, data: data)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode([ScheduledReport].self, from: data)
    }

    /// 提交一份报告；服务端按 `(kind, period_key)` 幂等落库。
    /// `context` 可传 nil —— 后端会从自己数据库（信号 / AI 用量）拼一份。
    @discardableResult
    func run(kind: ReviewKind = .daily, context: ReviewContextPayload?) async throws -> ScheduledReport {
        let url = baseURL.appendingPathComponent("/api/v1/reviews/run")
        var components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "kind", value: kind.rawValue)]
        var req = URLRequest(url: components.url!)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Accept")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        // 即使 context 为空也发一个空 JSON 对象，让服务端 schema 解析走 Option<web::Json<...>>
        req.httpBody = try encoder.encode(context ?? ReviewContextPayload(diary: nil, tickets: [], signals: [], focusCodes: []))
        let (data, response) = try await session.data(for: req)
        try Self.check(response, data: data)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(ScheduledReport.self, from: data)
    }

    private static func check(_ response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw ReportClientError.network(message: "无效的 HTTP 响应")
        }
        guard (200..<300).contains(http.statusCode) else {
            let body = String(data: data, encoding: .utf8) ?? ""
            throw ReportClientError.server(status: http.statusCode, body: body)
        }
    }
}

enum ReportClientError: LocalizedError {
    case network(message: String)
    case server(status: Int, body: String)

    var errorDescription: String? {
        switch self {
        case .network(let message):
            return "网络错误：\(message)"
        case .server(let status, let body):
            if body.isEmpty { return "服务端错误（HTTP \(status)）" }
            return "服务端错误（HTTP \(status)）：\(body.prefix(200))"
        }
    }
}

private extension Comparable {
    func clamped(_ lower: Self, _ upper: Self) -> Self {
        min(max(self, lower), upper)
    }
}
