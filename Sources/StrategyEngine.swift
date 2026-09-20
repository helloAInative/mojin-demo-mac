import Foundation

enum CondKind: String, Codable, CaseIterable, Identifiable {
    case priceAbove = "现价≥"
    case priceBelow = "现价≤"
    case pctAbove = "涨幅%≥"
    case pctBelow = "涨幅%≤"
    case volRatioAbove = "量比≥"
    case rsiAbove = "RSI≥"
    case rsiBelow = "RSI≤"
    case macdGolden = "MACD金叉"
    case macdDeath = "MACD死叉"
    case kdjJAbove = "J≥"
    case kdjJBelow = "J≤"

    var id: String { rawValue }
    var needsValue: Bool {
        switch self {
        case .macdGolden, .macdDeath: return false
        default: return true
        }
    }
}

enum StrategyScope: String, Codable, CaseIterable, Identifiable {
    case current = "当前盯盘"
    case group = "指定分组"
    case all = "全部自选"
    case codes = "指定代码"
    var id: String { rawValue }
}

struct StrategyCond: Codable, Equatable, Identifiable {
    var id: UUID
    var kind: CondKind
    var value: Double

    init(id: UUID = UUID(), kind: CondKind, value: Double = 0) {
        self.id = id
        self.kind = kind
        self.value = value
    }
}

struct ComboStrategy: Codable, Equatable, Identifiable {
    var id: UUID
    var name: String
    var enabled: Bool
    /// 兼容旧字段：非空则视为指定代码
    var code: String
    var scope: StrategyScope
    var scopeGroup: String
    var requireAll: Bool
    var conditions: [StrategyCond]
    /// 单边手续费 %，往返计 2 次
    var feePct: Double
    /// 单边滑点 %
    var slipPct: Double
    /// 样本外占比 0.2~0.5
    var oosRatio: Double

    init(
        id: UUID = UUID(),
        name: String,
        enabled: Bool = true,
        code: String = "",
        scope: StrategyScope = .current,
        scopeGroup: String = "默认",
        requireAll: Bool = true,
        conditions: [StrategyCond] = [],
        feePct: Double = 0.05,
        slipPct: Double = 0.10,
        oosRatio: Double = 0.30
    ) {
        self.id = id
        self.name = name
        self.enabled = enabled
        self.code = code
        self.scope = scope
        self.scopeGroup = scopeGroup
        self.requireAll = requireAll
        self.conditions = conditions
        self.feePct = feePct
        self.slipPct = slipPct
        self.oosRatio = oosRatio
    }

    enum CodingKeys: String, CodingKey {
        case id, name, enabled, code, scope, scopeGroup, requireAll, conditions, feePct, slipPct, oosRatio
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? "策略"
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        code = try c.decodeIfPresent(String.self, forKey: .code) ?? ""
        requireAll = try c.decodeIfPresent(Bool.self, forKey: .requireAll) ?? true
        conditions = try c.decodeIfPresent([StrategyCond].self, forKey: .conditions) ?? []
        feePct = try c.decodeIfPresent(Double.self, forKey: .feePct) ?? 0.05
        slipPct = try c.decodeIfPresent(Double.self, forKey: .slipPct) ?? 0.10
        oosRatio = try c.decodeIfPresent(Double.self, forKey: .oosRatio) ?? 0.30
        scopeGroup = try c.decodeIfPresent(String.self, forKey: .scopeGroup) ?? "默认"
        if let s = try c.decodeIfPresent(StrategyScope.self, forKey: .scope) {
            scope = s
        } else if !code.isEmpty {
            scope = .codes
        } else {
            scope = .current
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(name, forKey: .name)
        try c.encode(enabled, forKey: .enabled)
        try c.encode(code, forKey: .code)
        try c.encode(scope, forKey: .scope)
        try c.encode(scopeGroup, forKey: .scopeGroup)
        try c.encode(requireAll, forKey: .requireAll)
        try c.encode(conditions, forKey: .conditions)
        try c.encode(feePct, forKey: .feePct)
        try c.encode(slipPct, forKey: .slipPct)
        try c.encode(oosRatio, forKey: .oosRatio)
    }

    func targetCodes(current: String, symbols: [WatchSymbol]) -> [String] {
        switch scope {
        case .current:
            return [current]
        case .all:
            return symbols.map(\.code)
        case .group:
            let g = scopeGroup
            return symbols.filter { $0.group == g }.map(\.code)
        case .codes:
            if !code.isEmpty { return [code.lowercased()] }
            return [current]
        }
    }

    var scopeLabel: String {
        switch scope {
        case .current: return "当前"
        case .all: return "全部自选"
        case .group: return "组:\(scopeGroup)"
        case .codes: return code.isEmpty ? "指定" : code
        }
    }
}

struct SplitStats: Equatable {
    var signals: Int
    var nextUp: Int
    var avgNetPct: Double

    var winRate: Double {
        signals > 0 ? Double(nextUp) / Double(signals) * 100 : 0
    }

    var line: String {
        if signals == 0 { return "无触发" }
        return String(format: "%d次 胜率%.0f%% 净%.2f%%", signals, winRate, avgNetPct)
    }
}

struct BacktestResult: Equatable {
    var signals: Int
    var nextUp: Int
    var nextDown: Int
    var avgNextPct: Double
    var sampleDays: Int
    var summary: String
    var inSample: SplitStats
    var outSample: SplitStats
    var trades: [BacktestTrade] = []

    /// 样本外净收益是否高于样本内
    var oosBeatsIS: Bool? {
        guard inSample.signals > 0, outSample.signals > 0 else { return nil }
        return outSample.avgNetPct >= inSample.avgNetPct
    }

    /// 尾部连续亏损次数（按交易顺序）
    var trailingLosses: Int {
        var n = 0
        for t in trades.reversed() {
            if t.netPct < 0 { n += 1 } else { break }
        }
        return n
    }

    /// 过拟合/诚实一句话
    var honestyWarning: String {
        if signals == 0 { return "无触发，无法评价稳定性。" }
        if outSample.signals == 0 {
            return "⚠ 样本外无触发：样本内再好看也难信。"
        }
        if let beat = oosBeatsIS, !beat,
           inSample.avgNetPct - outSample.avgNetPct >= 0.5 {
            return "⚠ 疑似过拟合：样本内净\(String(format: "%.2f", inSample.avgNetPct))% > 样本外\(String(format: "%.2f", outSample.avgNetPct))%。"
        }
        if trailingLosses >= 3 {
            return "⚠ 尾部连亏 \(trailingLosses) 次，近期失效风险升高。"
        }
        if let beat = oosBeatsIS, beat {
            return "✓ 样本外未弱于样本内，可信度相对更高。"
        }
        return "样本外有触发；请结合胜率与成本再看。"
    }

    var honestyBadge: String {
        guard let beat = oosBeatsIS else {
            return outSample.signals == 0 ? "样本外空" : "—"
        }
        return beat ? "样本外≥内" : "样本外弱于内"
    }

    static func merge(labeled: [(code: String, result: BacktestResult)]) -> BacktestResult {
        let empty = SplitStats(signals: 0, nextUp: 0, avgNetPct: 0)
        guard !labeled.isEmpty else {
            return BacktestResult(
                signals: 0, nextUp: 0, nextDown: 0, avgNextPct: 0, sampleDays: 0,
                summary: "无标的可回测", inSample: empty, outSample: empty, trades: []
            )
        }
        var allTrades: [BacktestTrade] = []
        var sig = 0, up = 0, days = 0
        var isSig = 0, isUp = 0, isSum = 0.0
        var oosSig = 0, oosUp = 0, oosSum = 0.0
        var lines: [String] = ["作用域多标的回测（各股独立日线缓存）："]
        for item in labeled {
            let r = item.result
            sig += r.signals
            up += r.nextUp
            days = max(days, r.sampleDays)
            isSig += r.inSample.signals
            isUp += r.inSample.nextUp
            isSum += r.inSample.avgNetPct * Double(r.inSample.signals)
            oosSig += r.outSample.signals
            oosUp += r.outSample.nextUp
            oosSum += r.outSample.avgNetPct * Double(r.outSample.signals)
            allTrades.append(contentsOf: r.trades.map {
                BacktestTrade(
                    signalDate: "\(item.code)|\($0.signalDate)",
                    entry: $0.entry, exit: $0.exit, rawPct: $0.rawPct, netPct: $0.netPct
                )
            })
            lines.append("[\(item.code)] \(r.signals)次 净\(String(format: "%.2f", r.avgNextPct))% · IS \(r.inSample.line) · OOS \(r.outSample.line)")
        }
        allTrades.sort { $0.signalDate < $1.signalDate }
        let avg = sig > 0
            ? labeled.reduce(0.0) { $0 + $1.result.avgNextPct * Double($1.result.signals) } / Double(sig)
            : 0
        let ins = SplitStats(signals: isSig, nextUp: isUp, avgNetPct: isSig > 0 ? isSum / Double(isSig) : 0)
        let oos = SplitStats(signals: oosSig, nextUp: oosUp, avgNetPct: oosSig > 0 ? oosSum / Double(oosSig) : 0)
        var summary = lines.joined(separator: "\n")
        summary += "\n合计 \(sig) 次 · 净均 \(String(format: "%.2f", avg))% · \(labeled.count) 只"
        var result = BacktestResult(
            signals: sig, nextUp: up, nextDown: sig - up, avgNextPct: avg,
            sampleDays: days, summary: summary, inSample: ins, outSample: oos, trades: allTrades
        )
        if sig > 0 {
            result.summary += "\n\(result.honestyBadge) · \(result.honestyWarning)"
        }
        return result
    }

    func csvString(ruleName: String) -> String {
        var lines = [
            "rule,fee_slip_note,sample_days,signals,win,avg_net_pct,is_signals,is_win_pct,is_avg,oos_signals,oos_win_pct,oos_avg,oos_beats_is,trailing_losses,honesty",
            String(format: "%@,see_summary,%d,%d,%d,%.4f,%d,%.1f,%.4f,%d,%.1f,%.4f,%@,%d,\"%@\"",
                   ruleName.replacingOccurrences(of: ",", with: "_"),
                   sampleDays, signals, nextUp, avgNextPct,
                   inSample.signals, inSample.winRate, inSample.avgNetPct,
                   outSample.signals, outSample.winRate, outSample.avgNetPct,
                   oosBeatsIS.map { $0 ? "yes" : "no" } ?? "na",
                   trailingLosses,
                   honestyWarning.replacingOccurrences(of: "\"", with: "'"))
        ]
        lines.append("signal_date,entry_open,exit_close,raw_pct,net_pct")
        for t in trades {
            lines.append(String(format: "%@,%.4f,%.4f,%.4f,%.4f", t.signalDate, t.entry, t.exit, t.rawPct, t.netPct))
        }
        lines.append("")
        lines.append("\"\(summary.replacingOccurrences(of: "\"", with: "'"))\"")
        return lines.joined(separator: "\n")
    }
}

struct BacktestTrade: Equatable {
    var signalDate: String
    var entry: Double
    var exit: Double
    var rawPct: Double
    var netPct: Double
}

enum BacktestPreset: String, CaseIterable, Identifiable {
    case honest = "诚实档"
    case light = "轻成本"
    case heavy = "重摩擦"
    var id: String { rawValue }
    var fee: Double {
        switch self {
        case .honest: return 0.05
        case .light: return 0.03
        case .heavy: return 0.08
        }
    }
    var slip: Double {
        switch self {
        case .honest: return 0.10
        case .light: return 0.05
        case .heavy: return 0.20
        }
    }
    var oos: Double { 0.30 }
}

enum StrategyEngine {
    struct Snapshot {
        var price: Double
        var pct: Double
        var volRatio: Double?
        var rsi: Double?
        var macdTitle: String
        var kdjJ: Double?
    }

    static func evaluate(_ rule: ComboStrategy, snap: Snapshot) -> Bool {
        guard rule.enabled, !rule.conditions.isEmpty else { return false }
        let hits = rule.conditions.map { match($0, snap: snap) }
        return rule.requireAll ? hits.allSatisfy { $0 } : hits.contains(true)
    }

    /// 条件命中明细，用于热力与「为何触发」
    static func evaluateHits(_ rule: ComboStrategy, snap: Snapshot) -> [(cond: StrategyCond, hit: Bool)] {
        rule.conditions.map { ($0, match($0, snap: snap)) }
    }

    static func whyTriggered(_ rule: ComboStrategy, snap: Snapshot) -> String {
        let hits = evaluateHits(rule, snap: snap)
        guard !hits.isEmpty else { return "无条件" }
        let mode = rule.requireAll ? "AND" : "OR"
        let lines = hits.map { item -> String in
            let v = item.cond.kind.needsValue ? String(format: " %.2f", item.cond.value) : ""
            return "\(item.hit ? "✓" : "✗") \(item.cond.kind.rawValue)\(v)"
        }
        let ok = evaluate(rule, snap: snap)
        return "[\(mode)] \(ok ? "触发" : "未触发")\n" + lines.joined(separator: "\n")
    }

    static func match(_ c: StrategyCond, snap: Snapshot) -> Bool {
        switch c.kind {
        case .priceAbove: return snap.price >= c.value
        case .priceBelow: return snap.price <= c.value && snap.price > 0
        case .pctAbove: return snap.pct >= c.value
        case .pctBelow: return snap.pct <= c.value
        case .volRatioAbove: return (snap.volRatio ?? 0) >= c.value
        case .rsiAbove: return (snap.rsi ?? -1) >= c.value
        case .rsiBelow: return (snap.rsi ?? 999) <= c.value
        case .macdGolden: return snap.macdTitle == "金叉"
        case .macdDeath: return snap.macdTitle == "死叉"
        case .kdjJAbove: return (snap.kdjJ ?? -999) >= c.value
        case .kdjJBelow: return (snap.kdjJ ?? 999) <= c.value
        }
    }

    /// 收盘出信号 → 次日开盘买入 → 次日收盘卖出（不计隔夜）
    /// 净收益扣除往返手续费+滑点。
    static func backtest(rule: ComboStrategy, days: [DayBar], closes: [Double]) -> BacktestResult {
        let empty = SplitStats(signals: 0, nextUp: 0, avgNetPct: 0)
        guard days.count >= 40, closes.count == days.count else {
            return BacktestResult(
                signals: 0, nextUp: 0, nextDown: 0, avgNextPct: 0, sampleDays: days.count,
                summary: "日线不足，无法分段回测（需≥40根）",
                inSample: empty, outSample: empty, trades: []
            )
        }
        let macd = MACD.compute(closes: closes)
        let kdj = KDJ.compute(days: days)
        let rsi = RSI.compute(closes: closes)
        let cost = 2 * (rule.feePct + rule.slipPct)
        let oos = min(max(rule.oosRatio, 0.15), 0.5)
        let split = max(30, Int(Double(days.count) * (1 - oos)))

        func collect(from: Int, to: Int) -> (SplitStats, [BacktestTrade]) {
            var n = 0, up = 0, sum = 0.0
            var trades: [BacktestTrade] = []
            let hi = min(to, days.count - 1)
            guard from < hi else { return (empty, []) }
            for i in from..<hi {
                let prevClose = i > 0 ? closes[i - 1] : closes[i]
                let pct = prevClose > 0 ? (closes[i] - prevClose) / prevClose * 100 : 0
                let volRatio: Double? = {
                    let a = max(0, i - 20)
                    let slice = days[a..<i].map(\.volume).filter { $0 > 0 }
                    guard !slice.isEmpty, days[i].volume > 0 else { return nil }
                    let avg = slice.reduce(0, +) / Double(slice.count)
                    return avg > 0 ? days[i].volume / avg : nil
                }()
                var macdTitle = "--"
                if i > 0, let d0 = macd[i - 1].dif, let e0 = macd[i - 1].dea,
                   let d1 = macd[i].dif, let e1 = macd[i].dea {
                    if d0 <= e0 && d1 > e1 { macdTitle = "金叉" }
                    else if d0 >= e0 && d1 < e1 { macdTitle = "死叉" }
                    else if d1 > e1 { macdTitle = "多头" }
                    else { macdTitle = "空头" }
                }
                let snap = Snapshot(
                    price: closes[i], pct: pct, volRatio: volRatio,
                    rsi: rsi[i].value, macdTitle: macdTitle, kdjJ: kdj[i].j
                )
                guard evaluate(rule, snap: snap) else { continue }
                let nxt = days[i + 1]
                let o = nxt.open > 0 ? nxt.open : nxt.close
                guard o > 0, nxt.close > 0 else { continue }
                let raw = (nxt.close - o) / o * 100
                let net = raw - cost
                n += 1
                sum += net
                if net >= 0 { up += 1 }
                trades.append(BacktestTrade(signalDate: days[i].date, entry: o, exit: nxt.close, rawPct: raw, netPct: net))
            }
            return (SplitStats(signals: n, nextUp: up, avgNetPct: n > 0 ? sum / Double(n) : 0), trades)
        }

        let (ins, tIS) = collect(from: 26, to: split)
        let (oosS, tOOS) = collect(from: split, to: days.count - 1)
        let allTrades = tIS + tOOS
        let allN = ins.signals + oosS.signals
        let allUp = ins.nextUp + oosS.nextUp
        let allAvg: Double = {
            if allN == 0 { return 0 }
            return (ins.avgNetPct * Double(ins.signals) + oosS.avgNetPct * Double(oosS.signals)) / Double(allN)
        }()
        let summary: String
        if allN == 0 {
            summary = "样本期内无触发（开盘后净收益、扣费\(String(format: "%.2f", cost))%）"
        } else {
            summary = """
            口径：收盘信号 → 次日开盘买入 → 当日收盘卖出；往返成本 \(String(format: "%.2f", cost))%（费\(String(format: "%.2f", rule.feePct))+滑点\(String(format: "%.2f", rule.slipPct))）。
            样本内：\(ins.line)
            样本外：\(oosS.line)
            合计 \(allN) 次 · 净均 \(String(format: "%.2f", allAvg))%
            """
        }
        var result = BacktestResult(
            signals: allN, nextUp: allUp, nextDown: allN - allUp, avgNextPct: allAvg,
            sampleDays: days.count, summary: summary, inSample: ins, outSample: oosS, trades: allTrades
        )
        if allN > 0 {
            result.summary += "\n\(result.honestyBadge) · \(result.honestyWarning)"
        }
        return result
    }
}
