import SwiftUI

/// §I.6 组件拆分：自选热力图 + 持仓组合卡从 ContentView 抽出。
///
/// 两卡共享同一组 UITokens（§I.2）+ 行情色解耦的红涨绿跌 vs 信号色；
/// 点击行统一走 onSelectSymbol 回调，ContentView 负责切 tab。
struct WatchHeatmapView: View {
    @ObservedObject var settings: AppSettings
    @ObservedObject var store: MarketStore
    var onSelectSymbol: (String) -> Void

    var body: some View {
        let cells = settings.symbols.compactMap { symbol -> WatchHeatCell? in
            let quote = store.watchQuotes[symbol.code]
            guard let quote, quote.price > 0, quote.prev > 0 else { return nil }
            return WatchHeatCell(code: symbol.code, name: symbol.name, pct: quote.pct)
        }
        if cells.isEmpty {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: UITokens.stackTight) {
                Text("自选热力 · \(cells.count) 只有报价")
                    .font(.system(size: UITokens.metaSize, weight: .bold))
                    .foregroundStyle(.secondary)
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: UITokens.stackTight), count: 4),
                          spacing: UITokens.stackTight) {
                    ForEach(cells) { cell in
                        heatCellButton(cell)
                    }
                }
            }
            .padding(UITokens.stackNormal)
            .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 8))
        }
    }

    private func heatCellButton(_ cell: WatchHeatCell) -> some View {
        Button {
            onSelectSymbol(cell.code)
        } label: {
            VStack(alignment: .leading, spacing: 1) {
                Text(cell.name)
                    .font(.system(size: UITokens.metaSize, weight: .semibold))
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Text(String(format: "%@%.2f%%", cell.pct >= 0 ? "+" : "", cell.pct))
                    .font(.system(size: UITokens.bodySize, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(.primary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, UITokens.stackNormal - 2)
            .padding(.vertical, UITokens.stackTight)
            .background(heatColor(cell.pct), in: RoundedRectangle(cornerRadius: 5))
            .help("\(cell.name) \(cell.code) · 点击去盯盘")
        }
        .buttonStyle(.plain)
    }

    /// 涨跌幅 → 背景色（A 股红涨绿跌梯度；±3% 封顶，±0.06 中性灰）。
    /// 红 / 绿来自 UITokens.trendUp/Down，与信号色解耦。
    private func heatColor(_ pct: Double) -> Color {
        let clamped = max(min(pct, 3.0), -3.0) / 3.0
        if clamped > 0.02 {
            return UITokens.trendUp.opacity(0.15 + 0.45 * clamped)
        }
        if clamped < -0.02 {
            return UITokens.trendDown.opacity(0.15 + 0.45 * (-clamped))
        }
        return Color.primary.opacity(0.06)
    }
}

/// §I.6 + ROI #10：持仓组合卡（聚合 + 占比条 + 止损止盈标记）。
struct PortfolioCardView: View {
    @ObservedObject var settings: AppSettings
    @ObservedObject var store: MarketStore
    var onSelectSymbol: (String) -> Void

    var body: some View {
        let portfolio = store.portfolio
        if portfolio.rows.isEmpty {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: UITokens.stackTight + 1) {
                summaryHeader(portfolio.summary, count: portfolio.rows.count)
                subSummary(portfolio.summary)
                ForEach(portfolio.rows) { row in
                    Button {
                        onSelectSymbol(row.code)
                    } label: {
                        PortfolioRowView(row: row,
                                         totalMarket: portfolio.summary.market,
                                         hasStop: row.hasStop ? settings.positions[row.code]?.stopLoss : nil,
                                         hasTake: row.hasTake ? settings.positions[row.code]?.takeProfit : nil)
                    }
                    .buttonStyle(.plain)
                    .help(rowTooltip(row))
                }
            }
            .padding(UITokens.stackNormal)
            .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 8))
        }
    }

    private func summaryHeader(_ summary: PortfolioSummary, count: Int) -> some View {
        HStack(spacing: UITokens.stackNormal - 2) {
            Text("持仓组合 · \(count) 只")
                .font(.system(size: UITokens.bodySize, weight: .bold))
                .foregroundStyle(.secondary)
            Spacer()
            if summary.cost > 0 || summary.market > 0 {
                Text(String(format: "总市值 ¥%.0f", summary.market))
                    .font(.system(size: UITokens.bodySize, weight: .semibold))
                    .monospacedDigit()
                Text(String(format: "%@¥%.0f（%+.1f%%）",
                            summary.pnl >= 0 ? "浮盈 +" : "浮亏 ",
                            abs(summary.pnl),
                            summary.pnlPct))
                    .font(.system(size: UITokens.bodySize, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(trendByPct(summary.pnlPct))
            }
        }
    }

    private func subSummary(_ summary: PortfolioSummary) -> some View {
        HStack(spacing: UITokens.stackNormal - 2) {
            if summary.dayPnl != 0 {
                Text(String(format: "今日 %@¥%.0f",
                            summary.dayPnl >= 0 ? "+" : "−",
                            abs(summary.dayPnl)))
                    .font(.system(size: UITokens.metaSize, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(trendByPct(summary.dayPnl))
            }
            if summary.unquoted > 0 {
                Text("\(summary.unquoted) 只无报价（点行去盯盘拉行情）")
                    .font(.system(size: UITokens.metaSize))
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
    }

    private func rowTooltip(_ row: PortfolioRow) -> String {
        var text = "\(row.name) \(row.code) · 成本 \(row.cost > 0 ? String(format: "%.3f", row.cost) : "未填") · \(Int(row.shares)) 股"
        if row.hasStop, let note = settings.positions[row.code], note.stopLoss > 0 {
            text += String(format: " · 止损 %.2f", note.stopLoss)
        }
        if row.hasTake, let note = settings.positions[row.code], note.takeProfit > 0 {
            text += String(format: " · 止盈 %.2f", note.takeProfit)
        }
        return text + "\n点击去盯盘"
    }

    private func trendByPct(_ value: Double) -> Color {
        if value > 0 { return UITokens.trendUp }
        if value < 0 { return UITokens.trendDown }
        return .secondary
    }
}

/// 单只持仓行：名称 / 股数 / 现价 / 浮盈 / 市值占比条。
struct PortfolioRowView: View {
    let row: PortfolioRow
    let totalMarket: Double
    let hasStop: Double?
    let hasTake: Double?

    var body: some View {
        HStack(spacing: UITokens.stackNormal - 2) {
            VStack(alignment: .leading, spacing: 1) {
                Text(row.name.isEmpty ? row.code : row.name)
                    .font(.system(size: UITokens.bodySize, weight: .semibold))
                    .lineLimit(1)
                HStack(spacing: 3) {
                    if let stop = hasStop, stop > 0 {
                        Image(systemName: "shield.lefthalf.filled")
                            .font(.system(size: 7))
                            .foregroundStyle(UITokens.color(.buy))
                    }
                    if let take = hasTake, take > 0 {
                        Image(systemName: "flag.fill")
                            .font(.system(size: 7))
                            .foregroundStyle(UITokens.color(.observe))
                    }
                    Text(String(format: "%.0f 股", row.shares))
                        .font(.system(size: UITokens.microSize))
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
            VStack(alignment: .trailing, spacing: 1) {
                if row.quoted {
                    Text(String(format: "%.2f", row.price))
                        .font(.system(size: UITokens.bodySize, weight: .bold, design: .rounded))
                        .monospacedDigit()
                    Text(String(format: "%@%.2f%%", row.dayPct >= 0 ? "+" : "", row.dayPct))
                        .font(.system(size: UITokens.microSize))
                        .monospacedDigit()
                        .foregroundStyle(row.prev > 0 ? UITokens.trend(row.dayPct) : .secondary)
                } else {
                    Text("无报价")
                        .font(.system(size: UITokens.metaSize))
                        .foregroundStyle(.secondary)
                    Text(row.cost > 0 ? String(format: "成本 %.3f", row.cost) : "未填成本")
                        .font(.system(size: UITokens.microSize))
                        .foregroundStyle(.secondary)
                }
            }
            VStack(alignment: .trailing, spacing: 1) {
                if row.quoted && row.cost > 0 {
                    Text(String(format: "%@¥%.0f", row.pnl >= 0 ? "+" : "−", abs(row.pnl)))
                        .font(.system(size: UITokens.bodySize, weight: .semibold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.trend(row.pnl))
                    Text(String(format: "%+.1f%%", row.pnlPct))
                        .font(.system(size: UITokens.microSize))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.trend(row.pnl))
                } else {
                    Text("—").font(.system(size: UITokens.bodySize)).foregroundStyle(.secondary)
                }
            }
            // 市值占比条（占组合总市值）
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color.primary.opacity(0.1))
                    .frame(width: 54, height: 4)
                if row.quoted, totalMarket > 0 {
                    let weight = min(row.market / totalMarket, 1)
                    Capsule()
                        .fill(Color.accentColor.opacity(0.75))
                        .frame(width: 54 * weight, height: 4)
                }
            }
            .help(row.quoted && totalMarket > 0
                  ? String(format: "市值 ¥%.0f · 占组合 %.0f%%", row.market, row.market / totalMarket * 100)
                  : "无报价")
        }
        .contentShape(Rectangle())
    }
}