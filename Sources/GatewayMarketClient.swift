import Foundation

/// Rust 行情网关的 Swift DTO 适配层。这里不访问第三方行情源。
enum GatewayMarketClient {
    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 3.5
        config.timeoutIntervalForResource = 4
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
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
        guard remote.price > 0, remote.prev > 0 else { throw GatewayError.badResponse }
        let change = remote.price - remote.prev
        return Quote(
            name: remote.name.isEmpty ? symbol.name : remote.name,
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
