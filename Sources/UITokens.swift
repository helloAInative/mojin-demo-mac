import SwiftUI

/// §I.2 设计令牌：字号 / 间距 / 语义色。
///
/// 目的：消灭散落的 `.font(.system(size: 8/9/10))` 与互不通气的胶囊配色；
/// 把「可买 / 勿追 / 仅观察 / 暂停」四种信号语义从「A 股红涨绿跌」里抽离出来，
/// 避免「红色 = 涨」与「红色 = 危险」打架（详见 docs/未来演进方向.md §I.2）。
enum UITokens {
    // MARK: - 字号（紧凑面板：四级足够，再多就抢戏）

    /// 卡 / 区段标题（"尾盘智能推荐 · 目标 T+1"、section 头等）
    static let titleSize: CGFloat = 11
    static let titleWeight: Font.Weight = .bold

    /// 行级主文字（标的名称、买区价）
    static let bodySize: CGFloat = 10
    static let bodyWeight: Font.Weight = .semibold

    /// 次级元数据（样本数、置信区间、Δ%）
    static let metaSize: CGFloat = 9
    static let metaWeight: Font.Weight = .regular

    /// 最末层细节（"T+5 中线参考"、"次盘新股"等 tooltip-only 细节）
    static let microSize: CGFloat = 8
    static let microWeight: Font.Weight = .regular

    // MARK: - 间距

    static let stackTight: CGFloat = 4
    static let stackNormal: CGFloat = 6
    static let stackRelaxed: CGFloat = 8

    static let pillHPad: CGFloat = 5
    static let pillVPad: CGFloat = 1
    static let pillOpacity: Double = 0.12

    // MARK: - 文本色（§I.7 深色对比度）

    /// micro 字号（8pt）专用：深色下 `tertiary` 几乎不可读 → 用 `secondary` 作底线。
    /// 浅色下视觉差异极小，但深色下保 4.5:1 对比度。
    static let microText: Color = .secondary

    /// meta 字号（9pt）专用：比 micro 略轻，仍比 `tertiary` 易读。
    static let metaText: Color = Color.primary.opacity(0.78)

    /// 信号色上的「不可用 / 等待」占位文本用中性灰，避免与 buy/danger 混淆。
    static let idleText: Color = Color.primary.opacity(0.55)

    // MARK: - 深色对比度修正（§I.7）

    /// SwiftUI `.tertiary` 在 macOS 深色模式下与背景对比度 <3:1，
    /// 8–9pt 字号下几乎不可读。本 modifier 在深色下升级到
    /// `secondary` 的 0.85 不透明度，保证对比度 ≥4.5:1，
    /// 浅色下保留 `.tertiary` 的视觉层次。
    ///
    /// 用法：`.foregroundStyle(.tertiary)` → `.foregroundStyle(.tertiary.adaptiveContrast())`
    /// 或更彻底的 `.modifier(UITokens.ContrastTertiaryModifier())`。
    static func adaptiveTertiary(for scheme: ColorScheme) -> Color {
        scheme == .dark ? Color.secondary.opacity(0.85) : Color.tertiary
    }

    struct ContrastTertiaryModifier: ViewModifier {
        @Environment(\.colorScheme) private var scheme
        func body(content: Content) -> some View {
            content.foregroundStyle(UITokens.adaptiveTertiary(for: scheme))
        }
    }

    /// 价位 / 价位线 / 脉动描边在深色背景下的次级提示色：
    /// 浅色用 primary 35%，深色用 primary 65%，避免「糊成一团」。
    static func levelAccent(for scheme: ColorScheme) -> Color {
        scheme == .dark ? Color.primary.opacity(0.65) : Color.primary.opacity(0.35)
    }

    // MARK: - 语义色（信号：与趋势涨跌解耦）

    enum Signal {
        /// 绿灯：可买 / 达标 / 命中。基色 .green，吸顶辨识度最高。
        case buy
        /// 红灯：买入失效、暂停推荐、危险（不论涨跌方向）。
        case danger
        /// 橙灯：仅观察、偏弱、样本不足等「先别下手但有信息」。
        case observe
        /// 中性：上一交易日推荐 / 默认策略胶囊。
        case neutral
        /// 紫：复盘学习 / 内部标记（与执行风险无关）。
        case audit
    }

    static func color(_ signal: Signal) -> Color {
        switch signal {
        case .buy: return .green
        case .danger: return .red
        case .observe: return .orange
        case .neutral: return .secondary
        case .audit: return .purple
        }
    }

    static func background(_ signal: Signal) -> Color {
        color(signal).opacity(pillOpacity)
    }

    // MARK: - 行情色（A 股惯例：红涨绿跌；与 Signal.danger 解耦）

    static let trendUp = Color(red: 1, green: 0.27, blue: 0.23)
    static let trendDown = Color(red: 0.2, green: 0.84, blue: 0.29)

    /// 根据涨跌幅返回中性 / 红 / 绿；为 0 时回退 secondary，避免全黑主文案丢失语境。
    static func trend(_ pct: Double) -> Color {
        if pct > 0 { return trendUp }
        if pct < 0 { return trendDown }
        return .secondary
    }
}