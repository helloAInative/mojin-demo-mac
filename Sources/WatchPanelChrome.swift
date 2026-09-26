import SwiftUI

/// §I.4–I.5 盯盘面板的小型 chrome 组件：图例 / 三态价位行 / 密度切换胶囊。
/// ContentView 仍持有主体状态（store / settings / 半自动委托条等），
/// 这里只负责视觉与本地交互，不引入新的业务依赖。

/// §I.5 图表图例 hover 联动：本组件记录 hover 系列，外层把该集合透传到
/// MinuteChart.dimmedSeries；hover 的那一颗保留全色，未 hover 的系列淡化。
final class WatchLegendState: ObservableObject {
    @Published var hovered: String? = nil
}

struct WatchLegendView: View {
    @ObservedObject var state: WatchLegendState
    let avgColor: Color
    let trendUp: Color
    let trendDown: Color
    @State private var pinned: String? = nil
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        HStack(spacing: UITokens.stackNormal) {
            legendDot(key: "avg", color: avgColor, text: "均价")
            legendDot(key: "prev", color: Color.secondary.opacity(0.6), text: "昨收", dashed: true)
            legendDot(key: "resistance", color: trendUp, text: "阻力")
            legendDot(key: "support", color: trendDown, text: "支撑")
            Spacer(minLength: 0)
        }
        .font(.system(size: UITokens.microSize))
        .onChange(of: state.hovered) { new in
            if new == nil, pinned != nil { state.hovered = pinned }
        }
    }

    @ViewBuilder
    private func legendDot(key: String, color: Color, text: String, dashed: Bool = false) -> some View {
        let active = state.hovered == nil || state.hovered == key
        HStack(spacing: 3) {
            if dashed {
                Path { p in
                    p.move(to: CGPoint(x: 0, y: 4))
                    p.addLine(to: CGPoint(x: 10, y: 4))
                }
                .stroke(color, style: StrokeStyle(lineWidth: 1, dash: [2, 2]))
                .frame(width: 10, height: 8)
            } else {
                Capsule().fill(color).frame(width: 10, height: 3)
            }
            Text(text)
                .foregroundStyle(active ? Color.secondary : UITokens.adaptiveTertiary(for: colorScheme))
                .fontWeight(pinned == key ? .semibold : .regular)
        }
        .padding(.horizontal, 2)
        .background(
            pinned == key ? Color.accentColor.opacity(0.10) : Color.clear,
            in: RoundedRectangle(cornerRadius: 4)
        )
        .opacity(active ? 1.0 : 0.55)
        .onHover { hovering in
            if hovering {
                state.hovered = key
            } else if state.hovered == key {
                state.hovered = pinned
            }
        }
        .onTapGesture {
            if pinned == key {
                pinned = nil
                state.hovered = nil
            } else {
                pinned = key
                state.hovered = key
            }
        }
        .help(pinned == key
              ? "\(text) · 已钉住高亮（点击取消）"
              : "悬停 / 点击高亮\(text)系列，其它系列淡化")
    }
}

/// §I.5 关键价位三态（命中 / 等待 / 失效）。判定依据见 LevelRole。
enum LevelStatus: Equatable {
    case hit
    case waiting
    case missed
    case unknown

    var icon: String {
        switch self {
        case .hit: return "checkmark.circle.fill"
        case .waiting: return "dot.circle"
        case .missed: return "xmark.octagon.fill"
        case .unknown: return "questionmark.circle"
        }
    }

    var label: String {
        switch self {
        case .hit: return "命中"
        case .waiting: return "等待"
        case .missed: return "失效"
        case .unknown: return "—"
        }
    }

    var signal: UITokens.Signal {
        switch self {
        case .hit: return .buy
        case .waiting: return .observe
        case .missed: return .danger
        case .unknown: return .neutral
        }
    }
}

enum LevelRole { case base, resistance, support, neutral

    /// 命中/失效阈值按价位角色差异化：
    ///  - 阻力（向上看）：现价 ≥ 阻力 - hitPct% 视作命中；
    ///  - 支撑 / 基准（向下看）：现价 ≤ 价 + hitPct% 视作命中，
    ///    且跌破支撑 ≥ missPct% 视作失效。
    static func evaluate(value: Double, lastPrice: Double, hitPct: Double, missPct: Double, role: LevelRole) -> LevelStatus {
        guard value > 0, lastPrice > 0 else { return .unknown }
        let distPct = (lastPrice - value) / value * 100
        switch role {
        case .resistance:
            if distPct >= -hitPct { return .hit }
            return .waiting
        case .support:
            if distPct <= hitPct && distPct >= -missPct { return .hit }
            if distPct < -missPct { return .missed }
            return .waiting
        case .base, .neutral:
            if abs(distPct) <= hitPct { return .hit }
            return .waiting
        }
    }
}

struct WatchLevelRowView: View {
    let base: Double
    let support: Double
    let resistance: Double
    let lastPrice: Double
    /// 偏离百分比阈值，0.3 表示 ±0.3% 内为「命中」
    let hitPct: Double = 0.3
    /// 已跌破支撑线 ≥0.5% 视为「失效」
    let missPct: Double = 0.5

    var body: some View {
        HStack(spacing: 6) {
            levelPill("基准", base, role: .base)
            levelPill("阻力", resistance, role: .resistance)
            levelPill("支撑", support, role: .support)
            Spacer()
        }
    }

    @ViewBuilder
    private func levelPill(_ title: String, _ value: Double, role: LevelRole) -> some View {
        let status = LevelRole.evaluate(value: value, lastPrice: lastPrice, hitPct: hitPct, missPct: missPct, role: role)
        let color = UITokens.color(status.signal)
        let bg = UITokens.background(status.signal)
        HStack(spacing: 3) {
            Image(systemName: status.icon)
                .font(.system(size: 8, weight: .bold))
                .foregroundStyle(color)
            Text(title)
                .font(.system(size: 9, weight: .semibold))
                .foregroundStyle(.secondary)
            Text(value > 0 ? String(format: "%.2f", value) : "--")
                .font(.system(size: 10, weight: .semibold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(value > 0 ? .primary : .secondary)
        }
        .padding(.horizontal, 7)
        .padding(.vertical, 3)
        .background(bg, in: Capsule())
        .overlay(Capsule().stroke(color.opacity(0.55), lineWidth: 0.6))
        .help(levelHelp(title: title, status: status, value: value))
    }

    private func levelHelp(title: String, status: LevelStatus, value: Double) -> String {
        guard value > 0, lastPrice > 0 else { return "\(title)未设置" }
        let pct = (lastPrice - value) / value * 100
        return String(format: "%@ %.2f · 现价 %+.2f%% · %@",
                      title, value, pct, status.label)
    }
}

/// §I.4 密度 chip 升级：补「今日生效说明」与「自动窗口」两行小字，
/// 让用户立刻知道此刻为何采用这个密度。
struct WatchDensityChipView: View {
    @ObservedObject var settings: AppSettings
    let resolved: WatchDensity
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: UITokens.stackTight) {
                Image(systemName: iconName)
                    .font(.system(size: UITokens.metaSize))
                    .foregroundStyle(iconColor)
                Text(densityLabel)
                    .font(.system(size: UITokens.metaSize, weight: .semibold))
                    .foregroundStyle(.secondary)
                Text(activeBadge)
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(badgeColor)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(badgeColor.opacity(0.12), in: Capsule())
                Spacer()
                ForEach(WatchDensity.allCases) { mode in
                    let isActive = settings.watchDensity == mode
                    Button {
                        settings.setWatchDensity(mode)
                    } label: {
                        Text(mode.rawValue)
                            .font(.system(size: UITokens.microSize, weight: .semibold))
                            .foregroundStyle(isActive ? .white : .secondary)
                            .padding(.horizontal, UITokens.pillHPad)
                            .padding(.vertical, 1)
                            .background(isActive ? Color.accentColor : Color.secondary.opacity(0.12), in: Capsule())
                    }
                    .buttonStyle(.plain)
                    .help(densityHelp(mode))
                }
            }
            Text(effectiveHint)
                .font(.system(size: 8))
                .foregroundStyle(UITokens.adaptiveTertiary(for: colorScheme))
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var iconName: String {
        switch resolved {
        case .focus: return "scope"
        case .standard: return "rectangle.split.3x1"
        case .full: return "square.grid.2x2"
        case .auto: return "circle.lefthalf.filled"
        }
    }

    private var iconColor: Color {
        switch resolved {
        case .focus: return UITokens.color(.buy)
        case .full: return UITokens.color(.observe)
        case .auto, .standard: return .secondary
        }
    }

    private var activeBadge: String {
        settings.watchDensity == .auto ? "自动" : "手动"
    }

    private var badgeColor: Color {
        settings.watchDensity == .auto ? UITokens.color(.observe) : .secondary
    }

    private var densityLabel: String {
        switch (settings.watchDensity, resolved) {
        case (.auto, .focus): return "盘中自动 · 专注"
        case (.auto, .standard): return "盘前盘后 · 标准"
        case (.auto, .full): return "盘外自动 · 全量"
        case (.auto, .auto): return "自动"
        case (.focus, _): return "专注"
        case (.standard, _): return "标准"
        case (.full, _): return "全量"
        }
    }

    private func densityHelp(_ mode: WatchDensity) -> String {
        switch mode {
        case .auto: return "自动：盘中 09:30–15:00 切专注，其余时段全量"
        case .focus: return "专注：仅报价头 / 分时 / 价位 / 持仓盈亏"
        case .standard: return "标准：增加指数条"
        case .full: return "全量：含资讯 · 研报 · 止损卡 · OHLC · 指标"
        }
    }

    private var effectiveHint: String {
        switch (settings.watchDensity, resolved) {
        case (.auto, .focus):
            return "今日生效：专注（盘中 09:30–15:00 · A 股交易日）"
        case (.auto, .standard):
            return "今日生效：标准（午休 / 盘前 09:00–09:30 / 盘后 15:00 后）"
        case (.auto, .full):
            return "今日生效：全量（非交易时段或周末）"
        case (.auto, .auto):
            return "今日生效：随时间窗口自动切换（09:30–15:00 专注，其余全量）"
        case (.focus, _):
            return "今日生效：专注（手动锁定）"
        case (.standard, _):
            return "今日生效：标准（手动锁定）"
        case (.full, _):
            return "今日生效：全量（手动锁定）"
        }
    }
}