import Foundation

enum MACD {
    struct Point {
        var dif: Double?
        var dea: Double?
        var hist: Double?
    }

    static func ema(_ values: [Double], period: Int) -> [Double?] {
        guard period > 0, !values.isEmpty else { return [] }
        let k = 2.0 / Double(period + 1)
        var out = Array<Double?>(repeating: nil, count: values.count)
        var seed = 0.0
        for i in 0..<values.count {
            if i < period - 1 {
                seed += values[i]
            } else if i == period - 1 {
                seed += values[i]
                out[i] = seed / Double(period)
            } else if let prev = out[i - 1] {
                out[i] = values[i] * k + prev * (1 - k)
            }
        }
        return out
    }

    static func compute(closes: [Double], fast: Int = 12, slow: Int = 26, signal: Int = 9) -> [Point] {
        let eFast = ema(closes, period: fast)
        let eSlow = ema(closes, period: slow)
        var dif: [Double?] = Array(repeating: nil, count: closes.count)
        for i in 0..<closes.count {
            if let a = eFast[i], let b = eSlow[i] { dif[i] = a - b }
        }
        var difNums: [Double] = []
        var difIdx: [Int] = []
        for (i, v) in dif.enumerated() {
            if let v { difNums.append(v); difIdx.append(i) }
        }
        let deaNums = ema(difNums, period: signal)
        var dea: [Double?] = Array(repeating: nil, count: closes.count)
        for (j, idx) in difIdx.enumerated() { dea[idx] = deaNums[j] }
        return (0..<closes.count).map { i in
            guard let d = dif[i], let e = dea[i] else {
                return Point(dif: nil, dea: nil, hist: nil)
            }
            return Point(dif: d, dea: e, hist: 2 * (d - e))
        }
    }
}

enum KDJ {
    struct Point {
        var k: Double?
        var d: Double?
        var j: Double?
    }

    static func compute(days: [DayBar], n: Int = 9) -> [Point] {
        guard !days.isEmpty else { return [] }
        var out = Array(repeating: Point(k: nil, d: nil, j: nil), count: days.count)
        var prevK = 50.0
        var prevD = 50.0
        for i in 0..<days.count {
            let from = max(0, i - n + 1)
            let slice = days[from...i]
            let hh = slice.map { $0.high > 0 ? $0.high : $0.close }.max() ?? days[i].close
            let ll = slice.map { $0.low > 0 ? $0.low : $0.close }.min() ?? days[i].close
            let den = max(hh - ll, 0.0001)
            let rsv = (days[i].close - ll) / den * 100
            let k = (2.0 / 3.0) * prevK + (1.0 / 3.0) * rsv
            let d = (2.0 / 3.0) * prevD + (1.0 / 3.0) * k
            let j = 3 * k - 2 * d
            out[i] = Point(k: k, d: d, j: j)
            prevK = k
            prevD = d
        }
        return out
    }
}

enum RSI {
    struct Point { var value: Double? }

    static func compute(closes: [Double], period: Int = 14) -> [Point] {
        guard closes.count > period else {
            return Array(repeating: Point(value: nil), count: closes.count)
        }
        var out = Array(repeating: Point(value: nil), count: closes.count)
        var gain = 0.0, loss = 0.0
        for i in 1...period {
            let ch = closes[i] - closes[i - 1]
            if ch >= 0 { gain += ch } else { loss += -ch }
        }
        var avgG = gain / Double(period)
        var avgL = loss / Double(period)
        func rsi(_ g: Double, _ l: Double) -> Double {
            if l == 0 { return 100 }
            let rs = g / l
            return 100 - 100 / (1 + rs)
        }
        out[period] = Point(value: rsi(avgG, avgL))
        if period + 1 < closes.count {
            for i in (period + 1)..<closes.count {
                let ch = closes[i] - closes[i - 1]
                let g = max(ch, 0)
                let l = max(-ch, 0)
                avgG = (avgG * Double(period - 1) + g) / Double(period)
                avgL = (avgL * Double(period - 1) + l) / Double(period)
                out[i] = Point(value: rsi(avgG, avgL))
            }
        }
        return out
    }
}

enum VolumeSpike {
    /// 当日量 / 近20日均量
    static func ratio(days: [DayBar]) -> Double? {
        guard days.count >= 6 else { return nil }
        let last = days.last!.volume
        guard last > 0 else { return nil }
        let prev = Array(days.dropLast().suffix(20)).map(\.volume).filter { $0 > 0 }
        guard !prev.isEmpty else { return nil }
        let avg = prev.reduce(0, +) / Double(prev.count)
        guard avg > 0 else { return nil }
        return last / avg
    }
}

enum IndicatorKind: String, CaseIterable, Identifiable {
    case macd = "MACD"
    case kdj = "KDJ"
    case rsi = "RSI"
    case vol = "量能"
    var id: String { rawValue }
}
