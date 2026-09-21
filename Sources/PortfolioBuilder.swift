import Foundation

/// 多标的组合视图（ROI #10）：跨持仓聚合 + 单行明细。
/// 纯函数、独立可测；UI 层只做布局，不改数值。
struct PortfolioRow: Equatable, Identifiable {
    let code: String
    let name: String
    /// 股数
    let shares: Double
    /// 单价成本（0 = 未填）
    let cost: Double
    /// 现价（0 = 无报价）
    let price: Double
    /// 昨收（当日盈亏基准；0 = 未知）
    let prev: Double
    let hasStop: Bool
    let hasTake: Bool

    var id: String { code }

    /// 有报价才算市值（无报价沉底展示成本）
    var quoted: Bool { price > 0 }
    var market: Double { quoted ? shares * price : 0 }
    var costValue: Double { cost > 0 ? shares * cost : 0 }
    /// 浮盈（无报价或无成本为 0）
    var pnl: Double { quoted && cost > 0 ? shares * (price - cost) : 0 }
    /// 浮盈 %（相对成本）
    var pnlPct: Double { quoted && cost > 0 ? (price - cost) / cost * 100 : 0 }
    /// 当日盈亏（昨收未知为 0）
    var dayPnl: Double { quoted && prev > 0 ? shares * (price - prev) : 0 }
    var dayPct: Double { quoted && prev > 0 ? (price - prev) / prev * 100 : 0 }
}

struct PortfolioSummary: Equatable {
    /// 有报价持仓的市值合计
    let market: Double
    /// 有报价持仓的成本合计（pnl 分母同口径）
    let cost: Double
    let pnl: Double
    /// 当日盈亏合计（只含有昨收的）
    let dayPnl: Double
    /// 无报价持仓数
    let unquoted: Int

    var pnlPct: Double { cost > 0 ? pnl / cost * 100 : 0 }
    var isEmpty: Bool { market == 0 && cost == 0 && dayPnl == 0 && unquoted == 0 }
}

enum PortfolioBuilder {
    /// 按市值降序；无报价沉底（按成本额排）。shares ≤ 0 的脏数据直接滤掉。
    static func build(
        positions: [String: PositionNote],
        quotes: [String: Quote],
        names: [String: String]
    ) -> (rows: [PortfolioRow], summary: PortfolioSummary) {
        let rows: [PortfolioRow] = positions
            .filter { $0.value.shares > 0 }
            .map { code, note in
                let quote = quotes[code]
                return PortfolioRow(
                    code: code,
                    name: names[code] ?? quote?.name ?? code,
                    shares: note.shares,
                    cost: note.cost,
                    price: quote?.price ?? 0,
                    prev: quote?.prev ?? 0,
                    hasStop: note.stopLoss > 0,
                    hasTake: note.takeProfit > 0
                )
            }
            .sorted { a, b in
                if a.quoted != b.quoted { return a.quoted }
                if a.market != b.market { return a.market > b.market }
                return a.costValue > b.costValue
            }
        let quotedRows = rows.filter { $0.quoted }
        let market = quotedRows.reduce(0.0) { $0 + $1.market }
        let cost = quotedRows.reduce(0.0) { $0 + $1.costValue }
        let pnl = quotedRows.reduce(0.0) { $0 + $1.pnl }
        let dayPnl = rows.reduce(0.0) { $0 + $1.dayPnl }
        let unquoted = rows.count - quotedRows.count
        return (
            rows,
            PortfolioSummary(market: market, cost: cost, pnl: pnl, dayPnl: dayPnl, unquoted: unquoted)
        )
    }
}
