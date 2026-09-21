import Foundation

/// 智能止损 / 止盈建议：纯量化锚点推导，可解释、可单测。
/// 建议只到「预警 + 一键委托草稿」为止，不自动下单（决策权留给用户）。
enum StopTakeAdvisor {

    struct Params {
        /// 止损波动锚系数：现价 − k×ATR14
        var atrStopMultiple: Double = 2
        /// 止盈波动锚系数：现价 + m×ATR14
        var atrTakeMultiple: Double = 3
        /// 盈亏比硬门槛：(止盈−现价)/(现价−止损) 低于该值不给一键应用
        var minRatio: Double = 1.5
        /// 结构锚回看天数（swing low / high）
        var swingLookback: Int = 20
        /// 建议止损距现价小于该百分比视为「过近」
        var minStopDistPct: Double = 2
        /// 成本锚兜底回撤（%），与策略页 drawdownPct 取大
        var maxAcceptableDrawdownPct: Double = 5
    }

    struct Advice: Equatable {
        /// 建议止损价（0 = 数据不足）
        var stop: Double
        /// 建议止盈价（0 = 数据不足）
        var take: Double
        /// 盈亏比 (止盈−现价)/(现价−止损)；锚点不全时为 0
        var ratio: Double
        /// 止损锚点明细（可解释）
        var stopAnchors: [String]
        var takeAnchors: [String]
        /// 止损距现价百分比（锚点有效时）
        var stopDistPct: Double
        var warning: String
        var actionable: Bool

        static let insufficient = Advice(
            stop: 0, take: 0, ratio: 0, stopAnchors: [], takeAnchors: [],
            stopDistPct: 0, warning: "日线数据不足，暂无法给建议", actionable: false
        )
    }

    /// ATR（平均真实波幅）。TR = max(高−低, |高−昨收|, |低−昨收|)，取近 `period` 根均值。
    static func atr(days: [DayBar], period: Int = 14) -> Double? {
        guard days.count >= period + 1 else { return nil }
        let window = Array(days.suffix(period + 1))
        var trSum = 0.0
        for i in 1...period {
            let bar = window[i]
            let prevClose = window[i - 1].close
            guard bar.high > 0, bar.low > 0, prevClose > 0 else { return nil }
            let tr = max(
                bar.high - bar.low,
                abs(bar.high - prevClose),
                abs(bar.low - prevClose)
            )
            trSum += tr
        }
        return trSum / Double(period)
    }

    /// 三锚点推导：止损取保守（最高价），止盈取先到（最低价）。
    /// - 波动锚：ATR 倍数（趋势跟踪止损的常规做法）
    /// - 结构锚：近 N 日 swing low / high（跌破结构低点 = 走势破坏）
    /// - 成本锚 / 阻力锚：可承受回撤与既有阻力位
    static func advise(
        days: [DayBar],
        price: Double,
        cost: Double,
        resistance: Double,
        drawdownPct: Double,
        params: Params = Params()
    ) -> Advice {
        guard price > 0, days.count >= 2 else { return .insufficient }
        // suffix 自动截断：数据不足回看天数时按现有窗口算，结构锚文案标注实际窗口
        let lookback = max(2, params.swingLookback)
        return adviseInternal(days: Array(days.suffix(lookback)), price: price, cost: cost,
                              resistance: resistance, drawdownPct: drawdownPct, params: params)
    }

    private static func adviseInternal(
        days: [DayBar],
        price: Double,
        cost: Double,
        resistance: Double,
        drawdownPct: Double,
        params: Params
    ) -> Advice {
        let atrValue = atr(days: days, period: 14)
        let swingLow = days.map(\.low).filter { $0 > 0 }.min() ?? 0
        let swingHigh = days.map(\.high).filter { $0 > 0 }.max() ?? 0

        var stopAnchors: [String] = []
        var stopCandidates: [Double] = []
        if let atrValue {
            let anchor = price - params.atrStopMultiple * atrValue
            if anchor > 0, anchor < price {
                stopCandidates.append(anchor)
                stopAnchors.append(String(format: "波动锚 现价−%.0f×ATR14(%.2f) = %.2f",
                                          params.atrStopMultiple, atrValue, anchor))
            }
        }
        if swingLow > 0, swingLow < price {
            stopCandidates.append(swingLow)
            stopAnchors.append(String(format: "结构锚 近%lu日低点 = %.2f", days.count, swingLow))
        }
        if cost > 0 {
            let dd = max(drawdownPct, params.maxAcceptableDrawdownPct)
            let anchor = cost * (1 - dd / 100)
            if anchor > 0, anchor < price {
                stopCandidates.append(anchor)
                stopAnchors.append(String(format: "成本锚 成本%.3f×(1−%.0f%%) = %.2f", cost, dd, anchor))
            }
        }

        var takeAnchors: [String] = []
        var takeCandidates: [Double] = []
        if let atrValue {
            let anchor = price + params.atrTakeMultiple * atrValue
            if anchor > price {
                takeCandidates.append(anchor)
                takeAnchors.append(String(format: "波动锚 现价+%.0f×ATR14(%.2f) = %.2f",
                                          params.atrTakeMultiple, atrValue, anchor))
            }
        }
        if swingHigh > price {
            takeCandidates.append(swingHigh)
            takeAnchors.append(String(format: "结构锚 近%lu日高点 = %.2f", days.count, swingHigh))
        }
        if resistance > price {
            takeCandidates.append(resistance)
            takeAnchors.append(String(format: "阻力锚 关键阻力位 = %.2f", resistance))
        }

        let stop = stopCandidates.max() ?? 0
        let take = takeCandidates.min() ?? 0
        var ratio = 0.0
        if stop > 0, take > 0 {
            ratio = (take - price) / (price - stop)
        }
        let stopDistPct = stop > 0 ? (price - stop) / price * 100 : 0

        var warnings: [String] = []
        if stop > 0, take > 0 {
            if ratio < params.minRatio {
                warnings.append(String(format: "盈亏比 %.2f < %.1f，建议观望不动", ratio, params.minRatio))
            }
            if stopDistPct < params.minStopDistPct {
                warnings.append(String(format: "止损距现价仅 %.1f%%，过近易被日内波动扫到，考虑减仓而非清仓",
                                       stopDistPct))
            }
        } else if stop > 0 {
            warnings.append("止盈锚点不足（无上方结构 / 阻力），仅给止损参考")
        } else if take > 0 {
            warnings.append("止损锚点不足，仅给止盈参考")
        } else {
            warnings.append("锚点均无效，暂无法给建议")
        }

        return Advice(
            stop: stop, take: take, ratio: ratio,
            stopAnchors: stopAnchors, takeAnchors: takeAnchors,
            stopDistPct: stopDistPct,
            warning: warnings.joined(separator: "；"),
            actionable: warnings.isEmpty && stop > 0 && take > 0
        )
    }
}
