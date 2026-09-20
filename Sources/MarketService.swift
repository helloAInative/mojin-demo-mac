import Foundation

enum MarketService {
    private static let session: URLSession = {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.timeoutIntervalForRequest = 8
        cfg.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: cfg)
    }()

    /// 腾讯 → 东财 → 新浪
    static func fetchQuote(symbol: WatchSymbol) async throws -> Quote {
        if let quote = try? await GatewayMarketClient.quote(symbol: symbol) { return quote }
        await RateLimiter.shared.wait(bucket: "quote")
        var last: Error = MarketError.badData
        do { return try await fetchQuoteTencent(symbol) }
        catch { last = error; CrashLog.append("quote tencent fail \(symbol.code): \(error)") }
        await RateLimiter.shared.wait(bucket: "quote")
        do { return try await fetchQuoteEastMoney(symbol) }
        catch { last = error; CrashLog.append("quote eastmoney fail \(symbol.code): \(error)") }
        await RateLimiter.shared.wait(bucket: "quote")
        do { return try await fetchQuoteSina(symbol) }
        catch { last = error; CrashLog.append("quote sina fail \(symbol.code): \(error)") }
        throw last
    }

    struct VoteResult {
        var quote: Quote
        var sources: [Quote]
        /// 最高价与最低价相对主源价的分歧 %；≥0.25 视为分歧
        var divergencePct: Double
        var primarySource: String
        var divergent: Bool { divergencePct >= 0.25 && sources.count >= 2 }
        var badge: String {
            if sources.count < 2 { return primarySource }
            if divergent {
                return String(format: "%@ · 分歧%.2f%%", primarySource, divergencePct)
            }
            return String(format: "%@ · %d源一致", primarySource, sources.count)
        }
    }

    /// 多源并行拉价：现价始终用主源（腾讯>东财>新浪），分歧只做角标，不改中位数假价
    static func fetchQuoteVoted(symbol: WatchSymbol) async -> VoteResult {
        if let quote = try? await GatewayMarketClient.quote(symbol: symbol) {
            return VoteResult(quote: quote, sources: [quote], divergencePct: 0,
                              primarySource: quote.source)
        }
        return await withTaskGroup(of: Quote?.self) { group in
            group.addTask { try? await fetchQuoteTencent(symbol) }
            group.addTask { try? await fetchQuoteEastMoney(symbol) }
            group.addTask { try? await fetchQuoteSina(symbol) }
            var got: [Quote] = []
            for await q in group {
                if let q, q.price > 0 { got.append(q) }
            }
            if got.isEmpty {
                if let q = try? await fetchQuote(symbol: symbol) {
                    return VoteResult(quote: q, sources: [q], divergencePct: 0, primarySource: q.source)
                }
                return VoteResult(quote: Quote(), sources: [], divergencePct: 0, primarySource: "")
            }
            let order = ["腾讯", "东财", "新浪"]
            let primary = order.compactMap { name in got.first(where: { $0.source == name }) }.first ?? got[0]
            let prices = got.map(\.price)
            let hi = prices.max() ?? primary.price
            let lo = prices.min() ?? primary.price
            let div = primary.price > 0 ? (hi - lo) / primary.price * 100 : 0
            var q = primary
            // 源字段保留主源名；UI 用 badge 显示分歧
            q.source = primary.source
            return VoteResult(quote: q, sources: got, divergencePct: div,
                              primarySource: "直连·\(primary.source)")
        }
    }

    /// 弱网批量：并发有限拉取自选报价
    static func fetchQuotesBatch(symbols: [WatchSymbol], concurrency: Int = 3) async -> [String: Quote] {
        var out: [String: Quote] = [:]
        var i = 0
        while i < symbols.count {
            let end = min(i + concurrency, symbols.count)
            let chunk = Array(symbols[i..<end])
            await withTaskGroup(of: (String, Quote?).self) { group in
                for s in chunk {
                    group.addTask {
                        await RateLimiter.shared.wait(bucket: "batch")
                        let q = try? await fetchQuote(symbol: s)
                        return (s.code, q)
                    }
                }
                for await (code, q) in group {
                    if let q, q.price > 0 { out[code] = q }
                }
            }
            i = end
        }
        return out
    }

    /// 大盘：核心三项 + 可选沪深300 / 北证50
    static let boardIndexDefs: [(code: String, short: String, name: String, optional: Bool)] = [
        ("sh000001", "上证", "上证指数", false),
        ("sz399001", "深成", "深证成指", false),
        ("sz399006", "创业", "创业板指", false),
        ("sh000300", "沪深300", "沪深300", true),
        ("bj899050", "北证50", "北证50", true)
    ]

    static func fetchBoardIndices(includeHS300: Bool, includeBJ50: Bool) async -> (ok: [IndexQuote], fails: [SourceProbe]) {
        let defs = boardIndexDefs.filter { def in
            if !def.optional { return true }
            if def.code == "sh000300" { return includeHS300 }
            if def.code == "bj899050" { return includeBJ50 }
            return false
        }
        var gatewayIndices: [IndexQuote] = []
        var gatewayProbes: [SourceProbe] = []
        for def in defs {
            let started = Date()
            let symbol = WatchSymbol(code: def.code, name: def.name)
            if let quote = try? await GatewayMarketClient.quote(symbol: symbol) {
                gatewayIndices.append(IndexQuote(code: def.code, shortName: def.short, quote: quote))
                gatewayProbes.append(SourceProbe(name: def.short, ok: true,
                    latencyMs: Int(Date().timeIntervalSince(started) * 1000), price: quote.price,
                    detail: String(format: "%.2f%% · 网关", quote.pct)))
            } else {
                gatewayProbes.append(SourceProbe(name: def.short, ok: false,
                    latencyMs: Int(Date().timeIntervalSince(started) * 1000), price: 0,
                    detail: "网关失败"))
            }
        }
        if gatewayIndices.count == defs.count { return (gatewayIndices, gatewayProbes) }
        await RateLimiter.shared.wait(bucket: "index")
        let codes = defs.map(\.code).joined(separator: ",")
        let url = URL(string: "https://qt.gtimg.cn/q=\(codes)&_=\(stamp())")!
        var fails: [SourceProbe] = []
        do {
            let t0 = Date()
            let (data, _) = try await session.data(from: url)
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            guard let raw = decodeGBK(data) ?? String(data: data, encoding: .utf8) else {
                let fb = await fetchBoardIndicesFallback(defs: defs)
                return (fb.ok, fb.fails)
            }
            var map: [String: Quote] = [:]
            for chunk in raw.split(separator: ";") {
                let s = String(chunk).trimmingCharacters(in: .whitespacesAndNewlines)
                guard !s.isEmpty else { continue }
                guard let eq = s.firstIndex(of: "="),
                      let qStart = s.firstIndex(of: "\""),
                      let qEnd = s.lastIndex(of: "\""),
                      qStart < qEnd else { continue }
                var key = String(s[..<eq])
                if key.hasPrefix("v_") { key = String(key.dropFirst(2)) }
                let body = String(s[s.index(after: qStart)..<qEnd])
                let a = body.split(separator: "~", omittingEmptySubsequences: false).map(String.init)
                guard a.count > 32,
                      let price = Double(a[3]), price > 0,
                      let prev = Double(a[4]), prev > 0 else { continue }
                let change = Double(a[31]) ?? (price - prev)
                let pct = Double(a[32]) ?? (change / prev * 100)
                map[key.lowercased()] = Quote(
                    name: a[1],
                    price: price,
                    prev: prev,
                    open: Double(a[5]) ?? prev,
                    high: a.count > 33 ? (Double(a[33]) ?? price) : price,
                    low: a.count > 34 ? (Double(a[34]) ?? price) : price,
                    change: change,
                    pct: pct,
                    timeText: a.count > 30 ? formatTime(a[30]) : "--",
                    source: "腾讯"
                )
            }
            var out: [IndexQuote] = []
            for def in defs {
                if let q = map[def.code] {
                    out.append(IndexQuote(code: def.code, shortName: def.short, quote: q))
                    fails.append(SourceProbe(name: def.short, ok: true, latencyMs: ms, price: q.price, detail: String(format: "%.2f", q.pct) + "%"))
                } else {
                    fails.append(SourceProbe(name: def.short, ok: false, latencyMs: ms, price: 0, detail: "无数据"))
                }
            }
            if out.isEmpty {
                return await fetchBoardIndicesFallback(defs: defs)
            }
            return (out, fails)
        } catch {
            CrashLog.append("board index: \(error)")
            return await fetchBoardIndicesFallback(defs: defs)
        }
    }

    private static func fetchBoardIndicesFallback(defs: [(code: String, short: String, name: String, optional: Bool)]) async -> (ok: [IndexQuote], fails: [SourceProbe]) {
        var out: [IndexQuote] = []
        var fails: [SourceProbe] = []
        for def in defs {
            let t0 = Date()
            let sym = WatchSymbol(code: def.code, name: def.name)
            if let q = try? await fetchQuote(symbol: sym), q.price > 0 {
                let ms = Int(Date().timeIntervalSince(t0) * 1000)
                out.append(IndexQuote(code: def.code, shortName: def.short, quote: q))
                fails.append(SourceProbe(name: def.short, ok: true, latencyMs: ms, price: q.price, detail: "单拉OK"))
            } else {
                let ms = Int(Date().timeIntervalSince(t0) * 1000)
                fails.append(SourceProbe(name: def.short, ok: false, latencyMs: ms, price: 0, detail: "失败"))
            }
        }
        return (out, fails)
    }

    /// 用行情解析名称（搜索补全）
    static func resolveSymbolName(code raw: String) async -> (code: String, name: String)? {
        guard let c = WatchSymbol.normalizeCode(raw) else { return nil }
        let sym = WatchSymbol(code: c, name: "")
        if let q = try? await fetchQuote(symbol: sym), q.price > 0 {
            return (c, q.name.isEmpty ? c : q.name)
        }
        return nil
    }

    static func fetchMinute(symbol: WatchSymbol) async throws -> [MinuteBar] {
        if let bars = try? await GatewayMarketClient.minutes(symbol: symbol) { return bars }
        await RateLimiter.shared.wait(bucket: "minute")
        let code = symbol.tencentCode
        let url = URL(string: "https://web.ifzq.gtimg.cn/appstock/app/minute/query?code=\(code)&_=\(stamp())")!
        let (data, _) = try await session.data(from: url)
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let root = json?["data"] as? [String: Any],
              let stock = root[code] as? [String: Any],
              let dataObj = stock["data"] as? [String: Any],
              let rows = dataObj["data"] as? [String] else { throw MarketError.badData }

        var out: [MinuteBar] = []
        var prevVol = 0.0
        for row in rows {
            let p = row.split(whereSeparator: { $0 == " " || $0 == "\t" }).map(String.init)
            guard p.count >= 4,
                  let price = Double(p[1]), price > 0,
                  let cumVol = Double(p[2]),
                  let cumAmt = Double(p[3]) else { continue }
            let vol = max(0, cumVol - prevVol)
            prevVol = cumVol
            let avg = cumVol > 0 ? cumAmt / cumVol / 100.0 : price
            out.append(MinuteBar(minute: p[0], price: price, avg: avg, vol: vol))
        }
        return fillMinuteGaps(out)
    }

    /// 分钟缺口：相邻间隔 >1 分钟时用前值线性插值补齐（最多补 5 根）
    static func fillMinuteGaps(_ bars: [MinuteBar]) -> [MinuteBar] {
        guard bars.count >= 2 else { return bars }
        func toMin(_ s: String) -> Int? {
            let d = s.filter(\.isNumber)
            guard d.count >= 4 else { return nil }
            let t = String(d.suffix(4))
            guard let h = Int(t.prefix(2)), let m = Int(t.suffix(2)) else { return nil }
            return h * 60 + m
        }
        func fmt(_ mins: Int) -> String {
            String(format: "%02d%02d", mins / 60, mins % 60)
        }
        var out: [MinuteBar] = [bars[0]]
        for i in 1..<bars.count {
            let a = bars[i - 1], b = bars[i]
            if let ma = toMin(a.minute), let mb = toMin(b.minute), mb > ma + 1 {
                let gap = min(mb - ma - 1, 5)
                for k in 1...gap {
                    let t = ma + k
                    let ratio = Double(k) / Double(mb - ma)
                    let price = a.price + (b.price - a.price) * ratio
                    let avg = a.avg + (b.avg - a.avg) * ratio
                    out.append(MinuteBar(minute: fmt(t), price: price, avg: avg, vol: 0))
                }
            }
            out.append(b)
        }
        return out
    }

    static func fetchDaily(symbol: WatchSymbol, limit: Int = 120) async throws -> [DayBar] {
        if let bars = try? await GatewayMarketClient.days(symbol: symbol, limit: limit) { return bars }
        await RateLimiter.shared.wait(bucket: "daily")
        let code = symbol.tencentCode
        let url = URL(string: "https://web.ifzq.gtimg.cn/appstock/app/fqkline/get?param=\(code),day,,,\(limit),qfq&_=\(stamp())")!
        let (data, _) = try await session.data(from: url)
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let root = json?["data"] as? [String: Any],
              let stock = root[code] as? [String: Any] else { throw MarketError.badData }
        let rows = (stock["qfqday"] as? [[Any]]) ?? (stock["day"] as? [[Any]]) ?? []
        var out: [DayBar] = []
        for r in rows {
            guard r.count >= 3,
                  let date = r[0] as? String,
                  let closeNum = doubleValue(r[2]), closeNum > 0 else { continue }
            let open = r.count > 1 ? (doubleValue(r[1]) ?? closeNum) : closeNum
            let high = r.count > 3 ? (doubleValue(r[3]) ?? closeNum) : closeNum
            let low = r.count > 4 ? (doubleValue(r[4]) ?? closeNum) : closeNum
            let vol = r.count > 5 ? (doubleValue(r[5]) ?? 0) : 0
            out.append(DayBar(date: date, open: open, close: closeNum, high: high, low: low, volume: vol))
        }
        return out
    }

    /// 弱网体检：分别探测三源延迟
    static func probeSources(symbol: WatchSymbol) async -> [SourceProbe] {
        let started = Date()
        if let quote = try? await GatewayMarketClient.quote(symbol: symbol) {
            return [SourceProbe(name: "行情网关", ok: true,
                latencyMs: Int(Date().timeIntervalSince(started) * 1000), price: quote.price,
                detail: quote.source)]
        }
        async let t = timed("腾讯") { try await fetchQuoteTencent(symbol) }
        async let e = timed("东财") { try await fetchQuoteEastMoney(symbol) }
        async let s = timed("新浪") { try await fetchQuoteSina(symbol) }
        return await [t, e, s]
    }

    private static func timed(_ name: String, _ work: () async throws -> Quote) async -> SourceProbe {
        let t0 = Date()
        do {
            let q = try await work()
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            return SourceProbe(name: name, ok: q.price > 0, latencyMs: ms, price: q.price, detail: q.price > 0 ? String(format: "%.2f", q.price) : "空")
        } catch {
            let ms = Int(Date().timeIntervalSince(t0) * 1000)
            return SourceProbe(name: name, ok: false, latencyMs: ms, price: 0, detail: error.localizedDescription)
        }
    }

    // MARK: - Sources

    private static func fetchQuoteTencent(_ symbol: WatchSymbol) async throws -> Quote {
        let code = symbol.tencentCode
        let url = URL(string: "https://qt.gtimg.cn/q=\(code)&_=\(stamp())")!
        let (data, _) = try await session.data(from: url)
        guard let raw = decodeGBK(data) ?? String(data: data, encoding: .utf8) else {
            throw MarketError.badData
        }
        guard let start = raw.firstIndex(of: "\""),
              let end = raw.lastIndex(of: "\""),
              start < end else { throw MarketError.badData }
        let body = String(raw[raw.index(after: start)..<end])
        let a = body.split(separator: "~", omittingEmptySubsequences: false).map(String.init)
        guard a.count > 34,
              let price = Double(a[3]), price > 0,
              let prev = Double(a[4]), prev > 0 else { throw MarketError.badData }
        let change = Double(a[31]) ?? (price - prev)
        let pct = Double(a[32]) ?? (change / prev * 100)
        return Quote(
            name: a[1].isEmpty ? symbol.name : a[1],
            price: price,
            prev: prev,
            open: Double(a[5]) ?? prev,
            high: Double(a[33]) ?? price,
            low: Double(a[34]) ?? price,
            change: change,
            pct: pct,
            timeText: formatTime(a[30]),
            source: "腾讯"
        )
    }

    private static func fetchQuoteEastMoney(_ symbol: WatchSymbol) async throws -> Quote {
        // f43现价*100 f44最高 f45最低 f46开盘 f60昨收 f58名称 f169涨跌额*100 f170涨跌幅*100
        let url = URL(string: "https://push2.eastmoney.com/api/qt/stock/get?secid=\(symbol.eastMoneySecid)&fields=f43,f44,f45,f46,f57,f58,f60,f169,f170&_=\(stamp())")!
        var req = URLRequest(url: url)
        req.setValue("https://quote.eastmoney.com", forHTTPHeaderField: "Referer")
        let (data, _) = try await session.data(for: req)
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let d = json?["data"] as? [String: Any] else { throw MarketError.badData }
        func scaled(_ key: String) -> Double? {
            guard let v = doubleValue(d[key] as Any), v != 0 else { return nil }
            // 价格类多为 *100 整数
            if ["f43", "f44", "f45", "f46", "f60", "f169"].contains(key) { return v / 100.0 }
            if key == "f170" { return v / 100.0 }
            return v
        }
        guard let price = scaled("f43"), price > 0,
              let prev = scaled("f60"), prev > 0 else { throw MarketError.badData }
        let change = scaled("f169") ?? (price - prev)
        let pct = scaled("f170") ?? (change / prev * 100)
        let name = (d["f58"] as? String) ?? symbol.name
        return Quote(
            name: name,
            price: price,
            prev: prev,
            open: scaled("f46") ?? prev,
            high: scaled("f44") ?? price,
            low: scaled("f45") ?? price,
            change: change,
            pct: pct,
            timeText: "--",
            source: "东财"
        )
    }

    private static func fetchQuoteSina(_ symbol: WatchSymbol) async throws -> Quote {
        let url = URL(string: "https://hq.sinajs.cn/list=\(symbol.sinaCode)&_=\(stamp())")!
        var req = URLRequest(url: url)
        req.setValue("https://finance.sina.com.cn", forHTTPHeaderField: "Referer")
        let (data, _) = try await session.data(for: req)
        guard let raw = decodeGBK(data) ?? String(data: data, encoding: .utf8) else {
            throw MarketError.badData
        }
        // var hq_str_sz300623="名称,今开,昨收,现价,最高,最低,..."
        guard let qStart = raw.firstIndex(of: "\""),
              let qEnd = raw.lastIndex(of: "\""),
              qStart < qEnd else { throw MarketError.badData }
        let body = String(raw[raw.index(after: qStart)..<qEnd])
        let a = body.split(separator: ",", omittingEmptySubsequences: false).map(String.init)
        guard a.count > 8,
              let open = Double(a[1]),
              let prev = Double(a[2]), prev > 0,
              let price = Double(a[3]), price > 0 else { throw MarketError.badData }
        let high = Double(a[4]) ?? price
        let low = Double(a[5]) ?? price
        let change = price - prev
        let pct = change / prev * 100
        let tm = a.count > 31 ? a[31] : (a.count > 30 ? a[30] : "--")
        return Quote(
            name: a[0].isEmpty ? symbol.name : a[0],
            price: price,
            prev: prev,
            open: open > 0 ? open : prev,
            high: high,
            low: low,
            change: change,
            pct: pct,
            timeText: tm,
            source: "新浪"
        )
    }

    private static func stamp() -> Int { Int(Date().timeIntervalSince1970 * 1000) }

    private static func doubleValue(_ any: Any?) -> Double? {
        guard let any else { return nil }
        if let d = any as? Double { return d }
        if let n = any as? NSNumber { return n.doubleValue }
        if let s = any as? String { return Double(s) }
        if let i = any as? Int { return Double(i) }
        return nil
    }

    private static func decodeGBK(_ data: Data) -> String? {
        let cfEnc = CFStringEncoding(CFStringEncodings.GB_18030_2000.rawValue)
        let nsEnc = CFStringConvertEncodingToNSStringEncoding(cfEnc)
        return String(data: data, encoding: String.Encoding(rawValue: nsEnc))
    }

    private static func formatTime(_ t: String) -> String {
        guard t.count == 14 else { return t.isEmpty ? "--" : t }
        let chars = Array(t)
        return "\(chars[8])\(chars[9]):\(chars[10])\(chars[11]):\(chars[12])\(chars[13])"
    }
}
