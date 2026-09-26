import SwiftUI

/// B.1：当日 level 事件的分时图右侧栏标记。
struct LevelMark: Equatable, Identifiable {
    /// 分钟 "HHmm"
    var minute: String
    var state: State
    var title: String
    var leadSec: Int?
    var id: String { minute + title }

    enum State {
        case hit
        case missed
        case waiting
    }
}

struct MinuteChart: View {
    var bars: [MinuteBar]
    var prev: Double
    var base: Double
    var support: Double
    var resistance: Double
    var aiSignal: AILatestSignal? = nil
    /// 放大模式展示 AI 事件文字；常规模式只保留三角提示。
    var detailed: Bool = false
    /// 局部区间模式：将传入 bars 铺满整个画布，用于 30/60/120 分钟放大。
    var fitToBars: Bool = false
    /// bars.first 在全日 session 中的索引，供 AI 事件对齐局部时间轴。
    var sessionStartIndex: Int = 0
    /// 关键价位预警阈值（%），来自 settings.notifyConfig
    var levelLightPct: Double = 0.30
    var levelDeepPct: Double = 0.15
    /// 持仓成本（0 表示未持仓，不画成本线）
    var cost: Double = 0
    /// 现价相对成本线预警阈值（%），默认 1.0
    var costLightPct: Double = 1.0
    /// B.1：当日 level 事件右侧栏（按时间戳竖列，颜色分 hit / 失效 / 等待中）
    var levelMarks: [LevelMark] = []
    /// §I.5 图例 hover 联动：图例点 hover 后会把未在集合里的系列淡化。
    /// 支持 key："price" / "avg" / "prev" / "support" / "resistance" / "base"。
    var dimmedSeries: Set<String> = []
    /// 单击图表时打开详细视图；详细视图本身不传入，避免重复弹出。
    var onOpenDetail: (() -> Void)? = nil
    /// 详细视图可保留最后一次十字光标，并将选中分钟同步到外部数据条。
    var keepsSelection: Bool = false
    var selectedMinuteID: Binding<String?>? = nil

    @State private var hoverIndex: Int? = nil
    @State private var hoverPoint: CGPoint = .zero
    @Environment(\.displayScale) private var displayScale

    static let session: [String] = {
        var out: [String] = []
        func push(_ h0: Int, _ m0: Int, _ h1: Int, _ m1: Int) {
            var t = h0 * 60 + m0
            let end = h1 * 60 + m1
            while t <= end {
                let hh = t / 60, mm = t % 60
                out.append(String(format: "%02d%02d", hh, mm))
                t += 1
            }
        }
        push(9, 30, 11, 30)
        push(13, 0, 15, 0)
        return out
    }()

    var body: some View {
        // TimelineView 驱动 Canvas 周期性重绘，用于关键价位闪烁
        TimelineView(.animation(minimumInterval: 1.0 / 30.0, paused: false)) { tl in
            GeometryReader { geo in
                ZStack(alignment: .topLeading) {
                    Canvas { context, size in
                        // 把 tl.date 转成 0..1 的相位（2s 周期），作为闪烁驱动
                        let t = tl.date.timeIntervalSinceReferenceDate
                        let phase = (sin(t * .pi) + 1) / 2  // 0..1
                        draw(context: context, size: size,
                          flashPhase: phase,
                          flashPhaseDeep: (sin(t * 2 * .pi) + 1) / 2)
                    }
                    .gesture(
                        DragGesture(minimumDistance: 0)
                            .onChanged { value in
                                updateHover(at: value.location, in: geo.size)
                            }
                            .onEnded { value in
                                if !keepsSelection {
                                    hoverIndex = nil
                                    selectedMinuteID?.wrappedValue = nil
                                }
                                if abs(value.translation.width) < 3,
                                   abs(value.translation.height) < 3 {
                                    onOpenDetail?()
                                }
                            }
                    )

                    if let i = hoverIndex, i < bars.count {
                        let b = bars[i]
                        let tip = String(format: "%@  价%.2f  均%.2f  量%.0f",
                                         fmtHM(b.minute), b.price, b.avg, b.vol)
                        // 检查该 minute 是否落在某条 AI 事件的 ±1 分钟窗口内
                        let hit = hoverEventNote(forMinute: b.minute)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tip)
                                .font(.system(size: 10, weight: .medium))
                                .monospacedDigit()
                            if let hit {
                                Text("⚡ AI·" + hit)
                                    .font(.system(size: 10, weight: .semibold))
                                    .foregroundColor(.orange)
                            }
                        }
                        .padding(.horizontal, 7)
                        .padding(.vertical, 4)
                        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 6))
                        .position(
                            x: min(max(hoverPoint.x, 70), geo.size.width - 70),
                            y: max(14, hoverPoint.y - 18)
                        )
                        .allowsHitTesting(false)
                    }
                }
            }
        }
        .onChange(of: selectedMinuteID?.wrappedValue) { _, minuteID in
            guard keepsSelection else { return }
            if let minuteID,
               let index = bars.firstIndex(where: { $0.id == minuteID }) {
                hoverIndex = index
            } else {
                hoverIndex = nil
            }
        }
    }

    /// 根据 bar 的 minute 字符串（如 "0935"）找 ±1 分钟内的最近事件 note。
    private func hoverEventNote(forMinute minute: String) -> String? {
        guard let sig = aiSignal, !sig.events.isEmpty,
              let n = Int(minute), minute.count == 4 else { return nil }
        let hh = n / 100, mm = n % 100
        let cur = hh * 60 + mm
        // 排序后取第一个 minuteOffset 在 ±1 min 内的事件
        for ev in sig.events {
            if abs(ev.minuteOffset - cur) <= 1 {
                if !ev.note.isEmpty { return ev.note }
                return ev.kind == .volSpike ? "放量" : (ev.isUpArrow ? "上行信号" : "下行信号")
            }
        }
        return nil
    }

    private func fmtHM(_ m: String) -> String {
        guard m.count >= 4 else { return m }
        return "\(m.prefix(2)):\(m.suffix(2))"
    }

    private func updateHover(at point: CGPoint, in size: CGSize) {
        guard !bars.isEmpty else { return }
        let padL: CGFloat = 4
        let padR: CGFloat = min(92, max(76, size.width * 0.14))
        let slots = CGFloat(fitToBars ? max(bars.count - 1, 1) : max(Self.session.count - 1, 1))
        let ratio = min(max((point.x - padL) / max(size.width - padL - padR, 1), 0), 1)
        let idx = Int(round(ratio * slots))
        let safeIndex = max(0, min(bars.count - 1, idx))
        hoverIndex = safeIndex
        hoverPoint = point
        selectedMinuteID?.wrappedValue = bars[safeIndex].id
    }

    private func draw(context: GraphicsContext, size: CGSize, flashPhase: Double = 0, flashPhaseDeep: Double = 0) {
        guard !bars.isEmpty else {
            let text = Text("等待分时…").font(.system(size: 11)).foregroundColor(.secondary)
            context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
            return
        }

        let prices = bars.map(\.price)
        var minP = min(prices.min() ?? prev, prev, support > 0 ? support : prev)
        var maxP = max(prices.max() ?? prev, prev, resistance > 0 ? resistance : prev, base > 0 ? base : prev)
        let pad = max((maxP - minP) * 0.1, 0.05)
        minP -= pad
        maxP += pad

        // 为右侧价格轴保留独立栏，避免价签覆盖走势图和 AI 标记。
        let padL: CGFloat = 4
        let padR: CGFloat = min(92, max(76, size.width * 0.14))
        let padT: CGFloat = 6
        let padB: CGFloat = 16
        let slots = CGFloat(fitToBars ? max(bars.count - 1, 1) : max(Self.session.count - 1, 1))

        func snap(_ value: CGFloat) -> CGFloat {
            (value * displayScale).rounded() / max(displayScale, 1)
        }

        func x(_ i: Int) -> CGFloat {
            snap(padL + CGFloat(i) / slots * (size.width - padL - padR))
        }
        func y(_ v: Double) -> CGFloat {
            snap(padT + (1 - (v - minP) / (maxP - minP)) * (size.height - padT - padB))
        }

        drawGrid(context: context,
                 xLeft: padL, xRight: size.width - padR,
                 yTop: padT, yBottom: size.height - padB)

        var prevLine = Path()
        prevLine.move(to: CGPoint(x: padL, y: y(prev)))
        prevLine.addLine(to: CGPoint(x: size.width - padR, y: y(prev)))
        let prevAlpha: Double = dimmedSeries.isEmpty || dimmedSeries.contains("prev") ? 0.25 : 0.07
        context.stroke(prevLine, with: .color(.white.opacity(prevAlpha)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))

        // 关键价位线（base / support / resistance）：支持接近现价时闪烁
        // 提示带 + 加粗线体——以 drawAlertLevels 集中绘制
        let lastPrice = bars.last?.price ?? prev
        drawAlertLevels(context: context,
                        lastPrice: lastPrice,
                        flashPhase: flashPhase,
                        flashPhaseDeep: flashPhaseDeep,
                        base: base, support: support, resistance: resistance,
                        xLeft: padL, xRight: size.width - padR,
                        y: y,
                        lightPct: levelLightPct,
                        deepPct: levelDeepPct,
                        dimKeys: ["base": "base", "resistance": "resistance", "support": "support"],
                        dimmedSeries: dimmedSeries)

        let up = (lastPrice) >= prev
        let color: Color = up ? Color(red: 1, green: 0.27, blue: 0.23) : Color(red: 0.2, green: 0.84, blue: 0.29)

        var area = Path()
        for (i, b) in bars.enumerated() {
            let pt = CGPoint(x: x(i), y: y(b.price))
            if i == 0 { area.move(to: pt) } else { area.addLine(to: pt) }
        }
        area.addLine(to: CGPoint(x: x(bars.count - 1), y: size.height - padB))
        area.addLine(to: CGPoint(x: x(0), y: size.height - padB))
        area.closeSubpath()
        context.fill(area, with: .linearGradient(
            Gradient(colors: [color.opacity(0.28), color.opacity(0)]),
            startPoint: CGPoint(x: 0, y: padT),
            endPoint: CGPoint(x: 0, y: size.height)
        ))

        var line = Path()
        for (i, b) in bars.enumerated() {
            let pt = CGPoint(x: x(i), y: y(b.price))
            if i == 0 { line.move(to: pt) } else { line.addLine(to: pt) }
        }
        context.stroke(line, with: .color(color.opacity(dimmedSeries.isEmpty || dimmedSeries.contains("price") ? 1 : 0.18)), lineWidth: 1.6)

        // B.3：均价线（VWAP，腾讯分钟数据的 avg）——黄细线，hover 亦有数值
        var avgLine = Path()
        var started = false
        for (i, b) in bars.enumerated() where b.avg > 0 {
            let pt = CGPoint(x: x(i), y: y(b.avg))
            if started {
                avgLine.addLine(to: pt)
            } else {
                avgLine.move(to: pt)
                started = true
            }
        }
        let avgAlpha: Double = dimmedSeries.isEmpty || dimmedSeries.contains("avg") ? 0.65 : 0.12
        context.stroke(avgLine, with: .color(Color(red: 1, green: 0.84, blue: 0.04).opacity(avgAlpha)), lineWidth: 1)

        if let i = hoverIndex, i < bars.count {
            let hx = x(i)
            let hy = y(bars[i].price)
            var cross = Path()
            cross.move(to: CGPoint(x: hx, y: padT))
            cross.addLine(to: CGPoint(x: hx, y: size.height - padB))
            cross.move(to: CGPoint(x: padL, y: hy))
            cross.addLine(to: CGPoint(x: size.width - padR, y: hy))
            context.stroke(cross, with: .color(.cyan.opacity(0.55)), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
            context.fill(Path(ellipseIn: CGRect(x: hx - 3, y: hy - 3, width: 6, height: 6)), with: .color(.cyan))
        }

        // AI 关键价位标记（▲阻力/基，▼支撑/止）；每个标记在最右侧贴标签
        if let sig = aiSignal {
            drawAIMarkers(context: context,
                          markers: sig.markers,
                          xRight: size.width - padR,
                          y: y,
                          area: CGRect(x: padL, y: padT,
                                        width: size.width - padL - padR,
                                        height: size.height - padT - padB))
            // 事件流（突破 / 跌破 / 量能异动 / 假突破 / 反转）
            drawAIEvents(context: context,
                         events: sig.events,
                         chartW: size.width - padL - padR,
                         padL: padL, padR: padR,
                         y: y, yMin: padT, yMax: size.height - padB,
                         prev: prev,
                         showLabels: detailed,
                         domainStart: fitToBars ? sessionStartIndex : 0,
                         domainSlots: Int(slots))
        }

        // Y 轴价格刻度（右贴文本，关键价位都用不同颜色）
        drawYAxisTicks(context: context,
                       y: y,
                       plotRight: size.width - padR,
                       canvasRight: size.width - 4,
                       yMin: padT,
                       yMax: size.height - padB,
                       bars: bars,
                       lastPrice: lastPrice,
                       prev: prev,
                       base: base,
                       support: support,
                       resistance: resistance)

        // 持仓成本线 + 「到本」角标（接近现价 ±costLightPct% 时闪烁）
        if cost > 0 {
            drawCostLine(context: context,
                         y: y,
                         chartL: padL, chartR: size.width - padR,
                         chartT: padT, chartB: size.height - padB,
                         lastPrice: bars.last?.price ?? prev,
                         flashPhase: flashPhase)
        }

        // B.1：当日 level 事件右侧栏（竖列时间轴：上=开盘，下=收盘）
        if !levelMarks.isEmpty {
            drawLevelRail(context: context,
                          padL: padL, padR: padR,
                          yTop: padT, yBottom: size.height - padB,
                          totalW: size.width - padL - padR)
        }

        // X 轴时间刻度
        drawXAxisTicks(context: context,
                       padL: padL, padR: padR,
                       yBottom: size.height - padB,
                       totalW: size.width - padL - padR,
                       visibleBars: bars,
                       fitted: fitToBars)
    }

    /// 轻量网格只帮助判断时间和价位，不与关键价位线抢视觉层级。
    private func drawGrid(
        context: GraphicsContext,
        xLeft: CGFloat, xRight: CGFloat,
        yTop: CGFloat, yBottom: CGFloat
    ) {
        for step in 1...3 {
            let py = yTop + CGFloat(step) / 4 * (yBottom - yTop)
            var line = Path()
            line.move(to: CGPoint(x: xLeft, y: py))
            line.addLine(to: CGPoint(x: xRight, y: py))
            context.stroke(line, with: .color(.white.opacity(0.055)), lineWidth: 0.5)
        }
        let middle = (xLeft + xRight) / 2
        var divider = Path()
        divider.move(to: CGPoint(x: middle, y: yTop))
        divider.addLine(to: CGPoint(x: middle, y: yBottom))
        context.stroke(divider, with: .color(.white.opacity(0.09)),
                       style: StrokeStyle(lineWidth: 0.6, dash: [2, 3]))
    }

    /// 右侧栏：宽 ~20pt 竖列，每个事件按时间戳落位；hit=绿✓、失效=红×、等待=灰•。
    private func drawLevelRail(
        context: GraphicsContext,
        padL: CGFloat, padR: CGFloat,
        yTop: CGFloat, yBottom: CGFloat,
        totalW: CGFloat
    ) {
        let railW: CGFloat = 20
        let railX = padL + totalW - railW / 2
        // 栏底线
        var rail = Path()
        rail.move(to: CGPoint(x: railX - railW / 2, y: yTop))
        rail.addLine(to: CGPoint(x: railX - railW / 2, y: yBottom))
        context.stroke(rail, with: .color(.white.opacity(0.12)), lineWidth: 1)
        // 交易时段映射：09:30-15:00（含午休折叠——用 session slot 而非线性时间）。
        // 同一分钟先合并，相邻像素位置再聚类，避免密集事件叠成绿色长条和白字团。
        let slots = CGFloat(max(Self.session.count - 1, 1))
        struct RailItem {
            var y: CGFloat
            var state: LevelMark.State
            var leadSec: Int?
            var count: Int
        }
        func statePriority(_ state: LevelMark.State) -> Int {
            switch state {
            case .hit: return 3
            case .missed: return 2
            case .waiting: return 1
            }
        }
        var items: [RailItem] = []
        for (minute, marks) in Dictionary(grouping: levelMarks, by: \.minute) {
            guard minute.count == 4,
                  let h = Int(minute.prefix(2)),
                  let m = Int(minute.suffix(2)) else { continue }
            let timeMinutes = h * 60 + m
            let slot = Self.session.firstIndex(of: minute)
            let ratio: CGFloat
            if let slot {
                ratio = CGFloat(slot) / slots
            } else {
                // 非交易时段（开盘前 / 午休夹缝）：线性夹到栏内
                let open = 9 * 60 + 30, close = 15 * 60
                ratio = CGFloat(max(min(timeMinutes, close), open) - open) / CGFloat(close - open)
            }
            let strongest = marks.max {
                statePriority($0.state) < statePriority($1.state)
            }?.state ?? .waiting
            let lead = marks.compactMap(\.leadSec).filter { $0 > 0 }.min()
            items.append(RailItem(y: yTop + ratio * (yBottom - yTop),
                                  state: strongest,
                                  leadSec: lead,
                                  count: marks.count))
        }

        // 相距不足 11pt 的事件簇合并为一个带数量的状态标记。
        var clusters: [RailItem] = []
        for item in items.sorted(by: { $0.y < $1.y }) {
            if var last = clusters.last, item.y - last.y < 11 {
                clusters.removeLast()
                let combinedCount = last.count + item.count
                last.y = (last.y * CGFloat(last.count) + item.y * CGFloat(item.count)) / CGFloat(combinedCount)
                last.count = combinedCount
                if statePriority(item.state) > statePriority(last.state) { last.state = item.state }
                if let lead = item.leadSec {
                    last.leadSec = min(last.leadSec ?? lead, lead)
                }
                clusters.append(last)
            } else {
                clusters.append(item)
            }
        }

        for item in clusters {
            let py = min(max(item.y, yTop + 6), yBottom - 6)
            let color: Color = {
                switch item.state {
                case .hit: return Color(red: 0.2, green: 0.84, blue: 0.29)
                case .missed: return Color(red: 1, green: 0.35, blue: 0.3)
                case .waiting: return .gray
                }
            }()
            let glyph: String = {
                switch item.state {
                case .hit: return "✓"
                case .missed: return "×"
                case .waiting: return "…"
                }
            }()
            let badgeText = glyph + (item.count > 1 ? "\(item.count)" : "")
            let label = Text(badgeText)
                .font(.system(size: 7, weight: .heavy, design: .rounded))
                .foregroundColor(color)
            let resolved = context.resolve(label)
            let size = resolved.measure(in: CGSize(width: 22, height: 10))
            let badgeRect = CGRect(x: railX - size.width / 2,
                                   y: py - size.height / 2,
                                   width: size.width,
                                   height: size.height)
            let badge = Path(roundedRect: badgeRect.insetBy(dx: -3, dy: -1), cornerRadius: 4)
            context.fill(badge, with: .color(.black.opacity(0.82)))
            context.stroke(badge, with: .color(color.opacity(0.7)), lineWidth: 0.7)
            context.draw(label, at: CGPoint(x: badgeRect.midX, y: badgeRect.midY))

            if case .hit = item.state, let lead = item.leadSec, lead > 0 {
                let leadText = Text(lead >= 60 ? "\(lead / 60)m" : "\(lead)s")
                    .font(.system(size: 7, weight: .semibold, design: .monospaced))
                    .foregroundColor(.white.opacity(0.82))
                let leadResolved = context.resolve(leadText)
                let leadSize = leadResolved.measure(in: CGSize(width: 24, height: 10))
                let leadRect = CGRect(x: railX - railW / 2 - leadSize.width - 4,
                                      y: py - leadSize.height / 2,
                                      width: leadSize.width,
                                      height: leadSize.height)
                let leadBG = Path(roundedRect: leadRect.insetBy(dx: -2, dy: -1), cornerRadius: 3)
                context.fill(leadBG, with: .color(.black.opacity(0.78)))
                context.draw(leadText, at: CGPoint(x: leadRect.midX, y: leadRect.midY))
            }
        }
    }

    /// 在分时图上画 AI 事件流（折角箭头 + 标签）。
    private func drawAIEvents(
        context: GraphicsContext,
        events: [AIEvent],
        chartW: CGFloat,
        padL: CGFloat, padR: CGFloat,
        y: (Double) -> CGFloat,
        yMin: CGFloat, yMax: CGFloat,
        prev: Double,
        showLabels: Bool,
        domainStart: Int,
        domainSlots: Int
    ) {
        guard !events.isEmpty else { return }
        let slots = CGFloat(max(domainSlots, 1))
        let domainEnd = domainStart + domainSlots
        for ev in events.prefix(showLabels ? 10 : 6) {
            guard ev.minuteOffset >= domainStart, ev.minuteOffset <= domainEnd else { continue }
            // minuteOffset ∈ [0, session.count-1]；越界夹紧
            let offset = CGFloat(ev.minuteOffset - domainStart)
            let ratio = offset / slots
            let px = padL + ratio * chartW
            let priceForY = ev.price > 0 ? ev.price : prev
            let py = min(max(y(priceForY), yMin + 10), yMax - 10)

            // 颜色：上=暖橙，下=冷青
            let color: Color = ev.isUpArrow
                ? Color(red: 1, green: 0.55, blue: 0.18)
                : Color(red: 0.36, green: 0.78, blue: 1)

            // 箭头：▲ / ▼
            var tri = Path()
            if ev.isUpArrow {
                tri.move(to: CGPoint(x: px, y: py - 5))
                tri.addLine(to: CGPoint(x: px - 4, y: py + 3))
                tri.addLine(to: CGPoint(x: px + 4, y: py + 3))
            } else {
                tri.move(to: CGPoint(x: px, y: py + 5))
                tri.addLine(to: CGPoint(x: px - 4, y: py - 3))
                tri.addLine(to: CGPoint(x: px + 4, y: py - 3))
            }
            tri.closeSubpath()
            context.fill(tri, with: .color(color))
            context.stroke(tri, with: .color(.black.opacity(0.4)), lineWidth: 0.5)

            guard showLabels else { continue }

            // 标签贴在顶部/底部，并加短虚线接到价位
            let tagText = ev.label
            let tag = Text(tagText).font(.system(size: 9, weight: .bold)).foregroundColor(color)
            let resolved = context.resolve(tag)
            let tagSize = resolved.measure(in: CGSize(width: 24, height: 12))
            let tagY = ev.isUpArrow ? yMin + 2 : yMax - tagSize.height - 2
            let tagRect = CGRect(x: px - tagSize.width / 2,
                                 y: tagY,
                                 width: tagSize.width,
                                 height: tagSize.height)
            let bg = Path(roundedRect: tagRect.insetBy(dx: -2, dy: 0), cornerRadius: 2)
            context.fill(bg, with: .color(.black.opacity(0.55)))
            context.draw(tag, at: CGPoint(x: tagRect.midX, y: tagRect.midY))

            // 短虚线
            var stem = Path()
            stem.move(to: CGPoint(x: px, y: tagRect.maxY + (ev.isUpArrow ? 1 : -1)))
            stem.addLine(to: CGPoint(x: px, y: py + (ev.isUpArrow ? -5 : 5)))
            context.stroke(stem, with: .color(color.opacity(0.5)),
                           style: StrokeStyle(lineWidth: 0.6, dash: [2, 2]))
        }
    }

    /// Y 轴右侧价格刻度：高/低/昨收/基准/支撑/阻力，按对应颜色。
    /// 与 AI marker 错开：在 marker 的对面（xRight - 70）作为对照列。
    private func drawYAxisTicks(
        context: GraphicsContext,
        y: (Double) -> CGFloat,
        plotRight: CGFloat,
        canvasRight: CGFloat,
        yMin: CGFloat, yMax: CGFloat,
        bars: [MinuteBar],
        lastPrice: Double,
        prev: Double,
        base: Double, support: Double, resistance: Double
    ) {
        let high = bars.map(\.price).max() ?? prev
        let low  = bars.map(\.price).min() ?? prev

        struct Tick { let value: Double; let label: String; let color: Color }
        var ticks: [Tick] = []
        if lastPrice > 0  { ticks.append(.init(value: lastPrice, label: "现", color: .yellow)) }
        if prev > 0       { ticks.append(.init(value: prev, label: "昨", color: .white.opacity(0.85))) }
        if high > prev    { ticks.append(.init(value: high, label: "高", color: Color(red: 1, green: 0.45, blue: 0.3))) }
        if low > 0 && low != prev { ticks.append(.init(value: low, label: "低", color: Color(red: 0.45, green: 0.92, blue: 0.5))) }
        if base > 0       { ticks.append(.init(value: base,       label: "基", color: .cyan)) }
        if resistance > 0 { ticks.append(.init(value: resistance, label: "阻", color: .orange)) }
        if support > 0    { ticks.append(.init(value: support,    label: "支", color: .green)) }

        // 去重 + 排序：价位接近（<0.02）合并；按 value 升序
        let merged = ticks.reduce(into: [Tick]()) { acc, t in
            if let i = acc.firstIndex(where: { abs($0.value - t.value) < 0.02 }) {
                acc[i] = Tick(value: t.value,
                              label: acc[i].label + "·" + t.label,
                              color: acc[i].color)
            } else {
                acc.append(t)
            }
        }.sorted { $0.value < $1.value }

        // 右侧独立价格轴。先按目标 y 排序，再上下两遍避让，确保标签不重叠。
        let rowHeight: CGFloat = 13
        let minCenter = yMin + rowHeight / 2
        let maxCenter = yMax - rowHeight / 2
        var laidOut = merged.map { tick in
            (tick: tick, center: min(max(y(tick.value), minCenter), maxCenter))
        }.sorted { $0.center < $1.center }
        if laidOut.count > 1 {
            for i in 1..<laidOut.count {
                laidOut[i].center = max(laidOut[i].center, laidOut[i - 1].center + rowHeight)
            }
            laidOut[laidOut.count - 1].center = min(laidOut.last!.center, maxCenter)
            for i in stride(from: laidOut.count - 2, through: 0, by: -1) {
                laidOut[i].center = min(laidOut[i].center, laidOut[i + 1].center - rowHeight)
            }
        }
        let colX = plotRight + 5
        let maxLabelWidth = max(canvasRight - colX, 44)
        for item in laidOut {
            let t = item.tick
            let yPos = item.center
            let text = String(format: "%@%.2f", t.label, t.value)
            let attr = Text(text)
                .font(.system(size: 10, weight: .bold, design: .monospaced))
                .monospacedDigit()
                .foregroundColor(t.color)
            let resolved = context.resolve(attr)
            let size = resolved.measure(in: CGSize(width: maxLabelWidth, height: rowHeight))
            let rect = CGRect(x: colX, y: yPos - size.height / 2,
                              width: min(size.width, maxLabelWidth), height: size.height)
            let bg = Path(roundedRect: rect.insetBy(dx: -3, dy: -1), cornerRadius: 3)
            context.fill(bg, with: .color(.black.opacity(0.84)))
            context.stroke(bg, with: .color(t.color.opacity(0.22)), lineWidth: 0.5)
            context.draw(attr, at: CGPoint(x: rect.midX, y: rect.midY))
            // 价位横虚线接到左轴（仅在 base/support/resistance 时画，避免画面嘈杂）
            if t.label.contains("基") || t.label.contains("阻") || t.label.contains("支") {
                var line = Path()
                let actualY = y(t.value)
                line.move(to: CGPoint(x: 0, y: actualY))
                line.addLine(to: CGPoint(x: plotRight, y: actualY))
                context.stroke(line,
                               with: .color(t.color.opacity(0.25)),
                               style: StrokeStyle(lineWidth: 0.5, dash: [2, 3]))
            }
        }
    }

    /// X 轴底部时间刻度。午休在分时轴上折叠，因此中点合并显示。
    private func drawXAxisTicks(
        context: GraphicsContext,
        padL: CGFloat, padR: CGFloat,
        yBottom: CGFloat,
        totalW: CGFloat,
        visibleBars: [MinuteBar],
        fitted: Bool
    ) {
        let markers: [(label: String, ratio: CGFloat)]
        if fitted, let first = visibleBars.first, let last = visibleBars.last {
            let middle = visibleBars[visibleBars.count / 2]
            markers = [
                (fmtHM(first.minute), 0),
                (fmtHM(middle.minute), 0.5),
                (fmtHM(last.minute), 1)
            ]
        } else {
            markers = [
                ("9:30", 0),
                ("11:30/13:00", 0.5),
                ("15:00", 1)
            ]
        }
        for m in markers {
            let x = padL + m.ratio * totalW
            // 短竖线
            var tick = Path()
            tick.move(to: CGPoint(x: x, y: yBottom))
            tick.addLine(to: CGPoint(x: x, y: yBottom + 3))
            context.stroke(tick, with: .color(.white.opacity(0.4)), lineWidth: 0.6)
            // 文字
            let attr = Text(m.label)
                .font(.system(size: 9))
                .monospacedDigit()
                .foregroundColor(.white.opacity(0.65))
            let resolved = context.resolve(attr)
            let size = resolved.measure(in: CGSize(width: 72, height: 10))
            let labelX = min(max(x - size.width / 2, padL), padL + totalW - size.width)
            context.draw(attr, at: CGPoint(x: labelX + size.width / 2,
                                           y: yBottom + 12))
        }
    }

    /// 关键价位线：base / support / resistance。
    /// 现价接近 ±0.3% 时进入一级闪态——线体加粗 + 透明度正弦变化 + 横向辉光带。
    /// 进入 ±0.15% 二级时再叠加：辉光带变宽 + 双频脉动 + 红色描边。
    /// - flashPhase: 0..1, 0.5Hz（一级闪烁）
    /// - flashPhaseDeep: 0..1, 1.0Hz（二级更快闪烁）
    private func drawAlertLevels(
        context: GraphicsContext,
        lastPrice: Double,
        flashPhase: Double,
        flashPhaseDeep: Double,
        base: Double,
        support: Double,
        resistance: Double,
        xLeft: CGFloat,
        xRight: CGFloat,
        y: (Double) -> CGFloat,
        lightPct: Double,
        deepPct: Double,
        dimKeys: [String: String] = ["base": "base", "resistance": "resistance", "support": "support"],
        dimmedSeries: Set<String> = []
    ) {
        struct Level { let value: Double; let color: Color; let label: String; let key: String }
        var levels: [Level] = []
        if base > 0 {
            levels.append(.init(value: base, color: .cyan, label: "基准", key: dimKeys["base"] ?? "base"))
        }
        if resistance > 0 {
            levels.append(.init(value: resistance, color: .orange, label: "阻力", key: dimKeys["resistance"] ?? "resistance"))
        }
        if support > 0 {
            levels.append(.init(value: support, color: .green, label: "支撑", key: dimKeys["support"] ?? "support"))
        }
        for lv in levels {
            let isDimmed = !dimmedSeries.isEmpty && !dimmedSeries.contains(lv.key)
            let dimFactor: Double = isDimmed ? 0.18 : 1.0
            let dist = lastPrice > 0 ? abs(lastPrice - lv.value) / max(lv.value, 0.0001) * 100 : 999
            let tier: MarketStore.LevelAlertTier = lastPrice > 0
                ? .classify(distPct: dist,
                            lightPct: lightPct,
                            deepPct: deepPct)
                : .none
            let isAlert = tier != .none
            let isDeep = tier == .deep

            // 静态态
            let baseAlpha: Double = (isAlert ? 0.45 + 0.45 * flashPhase : 0.45) * dimFactor
            let lineWidth: CGFloat = isAlert ? (1.4 + CGFloat(flashPhase) * 0.8) : 1.0

            var p = Path()
            p.move(to: CGPoint(x: xLeft, y: y(lv.value)))
            p.addLine(to: CGPoint(x: xRight, y: y(lv.value)))

            if isAlert {
                // 辉光带：二级比一级更宽、更亮
                let bandH: CGFloat = isDeep ? 16 : 10
                let bandRect = CGRect(
                    x: xLeft,
                    y: y(lv.value) - bandH / 2,
                    width: xRight - xLeft,
                    height: bandH
                )
                let bandPath = Path(roundedRect: bandRect, cornerRadius: 3)
                let bandAlpha: Double = isDeep
                    ? (0.18 + 0.22 * flashPhaseDeep)   // 二级脉动频率 1Hz
                    : (0.10 + 0.15 * flashPhase)
                context.fill(bandPath, with: .color(lv.color.opacity(bandAlpha * dimFactor)))

                // 主虚线
                context.stroke(
                    p,
                    with: .color(lv.color.opacity(baseAlpha)),
                    style: StrokeStyle(lineWidth: lineWidth, dash: [4, 3])
                )
                // 加粗实线（高亮叠层）
                var solid = Path()
                solid.move(to: CGPoint(x: xLeft, y: y(lv.value)))
                solid.addLine(to: CGPoint(x: xRight, y: y(lv.value)))
                let solidAlpha: Double = isDeep
                    ? (0.65 + 0.35 * flashPhaseDeep)
                    : (0.55 + 0.4 * flashPhase)
                let solidWidth: CGFloat = isDeep
                    ? (1.4 + 0.6 * CGFloat(flashPhaseDeep))
                    : 1.0
                context.stroke(
                    solid,
                    with: .color(lv.color.opacity(solidAlpha * dimFactor)),
                    style: StrokeStyle(lineWidth: solidWidth)
                )

                // 二级预警：再叠一层红色描边外圈，提示「最危险」
                if isDeep {
                    var ring = Path()
                    ring.move(to: CGPoint(x: xLeft, y: y(lv.value)))
                    ring.addLine(to: CGPoint(x: xRight, y: y(lv.value)))
                    context.stroke(
                        ring,
                        with: .color(.red.opacity((0.35 + 0.45 * flashPhaseDeep) * dimFactor)),
                        style: StrokeStyle(lineWidth: 0.8)
                    )
                }

                // 价位与距离已在右侧独立价格轴及上方状态区展示；图内不再重复贴
                // 大块预警文字，避免小高度下覆盖行情线。
            } else {
                context.stroke(
                    p,
                    with: .color(lv.color.opacity(baseAlpha)),
                    style: StrokeStyle(lineWidth: lineWidth, dash: [4, 3])
                )
            }
        }
    }

    /// 持仓成本线 + 「到本」角标。
    /// 常规为半透灰虚线；现价接近 ±costLightPct% 时变橙红闪烁 + 角标高亮。
    private func drawCostLine(
        context: GraphicsContext,
        y: (Double) -> CGFloat,
        chartL: CGFloat, chartR: CGFloat,
        chartT: CGFloat, chartB: CGFloat,
        lastPrice: Double,
        flashPhase: Double
    ) {
        guard lastPrice > 0 else { return }
        let yC = y(cost)
        guard yC >= chartT - 1 && yC <= chartB + 1 else { return }
        let distPct = abs(lastPrice - cost) / cost * 100
        let near = distPct <= costLightPct
        // 主线（虚线）：常态灰；接近时橙红，闪烁 alpha 用 flashPhase 调制
        let lineColor: Color = near
            ? Color.orange.opacity(0.55 + 0.35 * flashPhase)
            : Color.gray.opacity(0.35)
        var path = Path()
        path.move(to: CGPoint(x: chartL, y: yC))
        path.addLine(to: CGPoint(x: chartR, y: yC))
        context.stroke(
            path,
            with: .color(lineColor),
            style: StrokeStyle(lineWidth: near ? 1.4 : 1.0,
                               lineCap: .butt,
                               dash: [3, 3])
        )
        // 「到本」角标：在右侧轴区，紧贴成本线右端
        let labelText = String(format: "本 %.2f", cost)
        let badge = Text(labelText)
            .font(.system(size: 9, weight: .bold))
            .foregroundColor(near ? .white : Color.gray)
        let resolved = context.resolve(badge)
        let tw = resolved.measure(in: CGSize(width: 120, height: 14)).width
        let bx = chartR - tw - 4
        let by = yC - 6
        if near {
            // 接近时：橙红填充矩形 + 闪烁 alpha
            let rect = CGRect(x: bx - 4, y: by - 1, width: tw + 8, height: 12 + 2)
            let bgColor = Color.orange.opacity(0.55 + 0.35 * flashPhase)
            context.fill(Path(rect), with: .color(bgColor))
        }
        context.draw(resolved, at: CGPoint(x: bx + tw / 2, y: yC))
    }

    /// 在走势图最右侧绘制 AI 价位三角。具体价格由右侧价格轴统一展示，
    /// 避免 AI 标签与支撑/阻力/高低价重复堆叠。
    private func drawAIMarkers(
        context: GraphicsContext,
        markers: [AIMarker],
        xRight: CGFloat,
        y: (Double) -> CGFloat,
        area: CGRect
    ) {
        guard !markers.isEmpty else { return }
        for m in markers {
            let isAbove = m.side == .above
            let color: Color = isAbove
                ? Color(red: 1, green: 0.55, blue: 0.18)   // 阻力/基 暖色
                : Color(red: 0.36, green: 0.78, blue: 1)   // 支撑/止 冷色
            // 在价位对应的纵坐标画一个清晰的小三角。
            let triX = xRight - 6
            let triY = min(max(y(m.price), area.minY + 5), area.maxY - 5)
            var tri = Path()
            if isAbove {
                tri.move(to: CGPoint(x: triX, y: triY - 4))
                tri.addLine(to: CGPoint(x: triX - 4, y: triY))
                tri.addLine(to: CGPoint(x: triX + 4, y: triY))
            } else {
                tri.move(to: CGPoint(x: triX, y: triY + 4))
                tri.addLine(to: CGPoint(x: triX - 4, y: triY))
                tri.addLine(to: CGPoint(x: triX + 4, y: triY))
            }
            tri.closeSubpath()
            context.fill(tri, with: .color(color))
            context.stroke(tri, with: .color(.black.opacity(0.7)), lineWidth: 0.5)
        }
    }
}

struct MACDChart: View {
    var days: [DayBar]
    var points: [MACD.Point]
    var limit: Int = 60
    var aiSignal: AILatestSignal? = nil

    var body: some View {
        Canvas { context, size in
            let paired: [(DayBar, MACD.Point)] = zip(days, points).compactMap { d, p in
                guard p.hist != nil else { return nil }
                return (d, p)
            }
            let view = Array(paired.suffix(limit))
            guard !view.isEmpty else {
                let text = Text("等待 MACD…").font(.system(size: 11)).foregroundColor(.secondary)
                context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
                return
            }

            let padL: CGFloat = 2
            let padR: CGFloat = 2
            let padT: CGFloat = 12
            let padB: CGFloat = 14
            var vals: [Double] = []
            for (_, p) in view {
                if let d = p.dif { vals.append(d) }
                if let e = p.dea { vals.append(e) }
                if let h = p.hist { vals.append(h) }
            }
            var minV = vals.min() ?? -1
            var maxV = vals.max() ?? 1
            let span = max(maxV - minV, 0.01)
            minV -= span * 0.08
            maxV += span * 0.08

            func x(_ idx: Int) -> CGFloat {
                padL + CGFloat(idx) / CGFloat(max(view.count - 1, 1)) * (size.width - padL - padR)
            }
            func y(_ v: Double) -> CGFloat {
                padT + (1 - (v - minV) / (maxV - minV)) * (size.height - padT - padB)
            }

            if minV < 0 && maxV > 0 {
                var zero = Path()
                zero.move(to: CGPoint(x: padL, y: y(0)))
                zero.addLine(to: CGPoint(x: size.width - padR, y: y(0)))
                context.stroke(zero, with: .color(.white.opacity(0.2)), lineWidth: 1)
            }

            let bw = max(1.5, (size.width - padL - padR) / CGFloat(view.count) * 0.55)
            for (idx, item) in view.enumerated() {
                guard let hist = item.1.hist else { continue }
                let y0 = y(0)
                let y1 = y(hist)
                let rect = CGRect(x: x(idx) - bw / 2, y: min(y0, y1), width: bw, height: max(abs(y1 - y0), 1))
                let c: Color = hist >= 0
                    ? Color(red: 1, green: 0.27, blue: 0.23).opacity(0.75)
                    : Color(red: 0.2, green: 0.84, blue: 0.29).opacity(0.75)
                context.fill(Path(rect), with: .color(c))
            }

            var difPath = Path()
            var deaPath = Path()
            for (idx, item) in view.enumerated() {
                if let d = item.1.dif {
                    let pt = CGPoint(x: x(idx), y: y(d))
                    if difPath.isEmpty { difPath.move(to: pt) } else { difPath.addLine(to: pt) }
                }
                if let e = item.1.dea {
                    let pt = CGPoint(x: x(idx), y: y(e))
                    if deaPath.isEmpty { deaPath.move(to: pt) } else { deaPath.addLine(to: pt) }
                }
            }
            context.stroke(difPath, with: .color(Color(red: 1, green: 0.84, blue: 0.04)), lineWidth: 1.4)
            context.stroke(deaPath, with: .color(Color(red: 0.39, green: 0.82, blue: 1)), lineWidth: 1.4)

            let legendDIF = Text("DIF").font(.system(size: 9, weight: .semibold)).foregroundColor(Color(red: 1, green: 0.84, blue: 0.04))
            let legendDEA = Text("DEA").font(.system(size: 9, weight: .semibold)).foregroundColor(Color(red: 0.39, green: 0.82, blue: 1))
            context.draw(legendDIF, at: CGPoint(x: 8, y: 8))
            context.draw(legendDEA, at: CGPoint(x: 36, y: 8))

            if let first = view.first?.0.date, first.count >= 10 {
                let a = Text(String(first.dropFirst(5))).font(.system(size: 9)).foregroundColor(.secondary)
                context.draw(a, at: CGPoint(x: 6, y: size.height - 4))
            }
            if let last = view.last?.0.date, last.count >= 10 {
                let b = Text(String(last.dropFirst(5))).font(.system(size: 9)).foregroundColor(.secondary)
                context.draw(b, at: CGPoint(x: size.width - 34, y: size.height - 4))
            }

            // AI 价位标签（MACD 纵轴是 DIF/DEA，无法直接按价格定位；
            // 这里只画右侧竖排标签，避免误导价位在 MACD 区间内的位置）。
            if let sig = aiSignal {
                drawMACDMarkers(context: context, markers: sig.markers,
                                area: CGRect(x: padL, y: padT,
                                              width: size.width - padL - padR,
                                              height: size.height - padT - padB))
            }
        }
        // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
    }

    /// MACD 专用：竖排 AI 价位标签，无价位纵坐标轴，因此只画标签。
    private func drawMACDMarkers(
        context: GraphicsContext,
        markers: [AIMarker],
        area: CGRect
    ) {
        guard !markers.isEmpty else { return }
        let tagFont = Font.system(size: 9, weight: .semibold)
        let rowH: CGFloat = 11
        var y = area.minY + 2
        let xRight = area.maxX
        for m in markers {
            let isAbove = m.side == .above
            let color: Color = isAbove
                ? Color(red: 1, green: 0.55, blue: 0.18)
                : Color(red: 0.36, green: 0.78, blue: 1)
            let labelText = "\(isAbove ? "▲" : "▼") \(m.label) \(String(format: "%.2f", m.price))"
            let attrText = Text(labelText).font(tagFont).foregroundColor(color)
            let resolved = context.resolve(attrText)
            let size = resolved.measure(in: CGSize(width: 120, height: rowH))
            let rect = CGRect(x: xRight - size.width - 4,
                              y: y,
                              width: size.width,
                              height: size.height)
            let bg = Path(roundedRect: rect.insetBy(dx: -3, dy: -1), cornerRadius: 3)
            context.fill(bg, with: .color(.black.opacity(0.55)))
            context.draw(attrText, at: CGPoint(x: rect.midX, y: rect.midY))
            y += rowH + 1
            if y > area.maxY - rowH { break }
        }
    }
}

struct LineTripleChart: View {
    var dates: [String]
    var a: [Double?]
    var b: [Double?]
    var c: [Double?]
    var nameA: String
    var nameB: String
    var nameC: String
    var colorA: Color
    var colorB: Color
    var colorC: Color
    var limit: Int = 60
    var aiSignal: AILatestSignal? = nil

    var body: some View {
        Canvas { context, size in
            let n = min(dates.count, a.count)
            guard n > 0 else { return }
            let start = max(0, n - limit)
            let count = n - start
            var vals: [Double] = []
            for i in start..<n {
                if let v = a[i] { vals.append(v) }
                if i < b.count, let v = b[i] { vals.append(v) }
                if i < c.count, let v = c[i] { vals.append(v) }
            }
            guard !vals.isEmpty else {
                let text = Text("等待数据…").font(.system(size: 11)).foregroundColor(.secondary)
                context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
                return
            }
            let padL: CGFloat = 2, padR: CGFloat = 2, padT: CGFloat = 12, padB: CGFloat = 14
            var minV = vals.min() ?? 0
            var maxV = vals.max() ?? 1
            let span = max(maxV - minV, 0.01)
            minV -= span * 0.08
            maxV += span * 0.08
            func x(_ idx: Int) -> CGFloat {
                padL + CGFloat(idx) / CGFloat(max(count - 1, 1)) * (size.width - padL - padR)
            }
            func y(_ v: Double) -> CGFloat {
                padT + (1 - (v - minV) / (maxV - minV)) * (size.height - padT - padB)
            }
            // KDJ/RSI 阈值线：80/20（仅在 KDJ 时画，避免污染 RSI）
            if nameA == "K" {
                let hi = y(80), lo = y(20)
                var hiP = Path(); hiP.move(to: CGPoint(x: padL, y: hi)); hiP.addLine(to: CGPoint(x: size.width - padR, y: hi))
                var loP = Path(); loP.move(to: CGPoint(x: padL, y: lo)); loP.addLine(to: CGPoint(x: size.width - padR, y: lo))
                context.stroke(hiP, with: .color(.red.opacity(0.35)), style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
                context.stroke(loP, with: .color(.green.opacity(0.35)), style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
            }
            // RSI 30/70 阈值线
            if nameA == "RSI" {
                let hi = y(70), lo = y(30)
                var hiP = Path(); hiP.move(to: CGPoint(x: padL, y: hi)); hiP.addLine(to: CGPoint(x: size.width - padR, y: hi))
                var loP = Path(); loP.move(to: CGPoint(x: padL, y: lo)); loP.addLine(to: CGPoint(x: size.width - padR, y: lo))
                context.stroke(hiP, with: .color(.orange.opacity(0.4)), style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
                context.stroke(loP, with: .color(.cyan.opacity(0.4)), style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
            }
            func stroke(_ series: [Double?], _ color: Color) {
                guard series.count > start else { return }
                var p = Path()
                var started = false
                for i in 0..<count {
                    let idx = start + i
                    guard idx < series.count, let v = series[idx] else { continue }
                    let pt = CGPoint(x: x(i), y: y(v))
                    if started { p.addLine(to: pt) } else { p.move(to: pt); started = true }
                }
                context.stroke(p, with: .color(color), lineWidth: 1.4)
            }
            stroke(a, colorA)
            stroke(b, colorB)
            stroke(c, colorC)
            context.draw(Text(nameA).font(.system(size: 9, weight: .semibold)).foregroundColor(colorA), at: CGPoint(x: 10, y: 8))
            context.draw(Text(nameB).font(.system(size: 9, weight: .semibold)).foregroundColor(colorB), at: CGPoint(x: 36, y: 8))
            context.draw(Text(nameC).font(.system(size: 9, weight: .semibold)).foregroundColor(colorC), at: CGPoint(x: 62, y: 8))

            // AI 价位竖排标签（KDJ/RSI 纵轴不是价格，仅做提示）
            if let sig = aiSignal, !sig.markers.isEmpty {
                drawSideLevelTags(context: context,
                                  markers: sig.markers,
                                  area: CGRect(x: padL, y: padT,
                                                width: size.width - padL - padR,
                                                height: size.height - padT - padB))
            }
        }
        // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
    }

    /// 顶部竖排 AI 价位标签（KDJ/RSI 顶部的小行）。
    private func drawSideLevelTags(
        context: GraphicsContext,
        markers: [AIMarker],
        area: CGRect
    ) {
        let tagFont = Font.system(size: 9, weight: .semibold)
        let rowH: CGFloat = 11
        var y = area.minY + 1
        let xRight = area.maxX
        for m in markers.prefix(4) {
            let isAbove = m.side == .above
            let color: Color = isAbove
                ? Color(red: 1, green: 0.55, blue: 0.18)
                : Color(red: 0.36, green: 0.78, blue: 1)
            let labelText = "\(isAbove ? "▲" : "▼") \(m.label) \(String(format: "%.2f", m.price))"
            let attrText = Text(labelText).font(tagFont).foregroundColor(color)
            let resolved = context.resolve(attrText)
            let size = resolved.measure(in: CGSize(width: 120, height: rowH))
            // 顶部一行：与图例同行，靠左放避免和 nameA/B/C 重叠
            let rect = CGRect(x: xRight - size.width - 4, y: y,
                              width: size.width, height: size.height)
            let bg = Path(roundedRect: rect.insetBy(dx: -3, dy: -1), cornerRadius: 3)
            context.fill(bg, with: .color(.black.opacity(0.55)))
            context.draw(attrText, at: CGPoint(x: rect.midX, y: rect.midY))
            y += rowH + 1
            if y > area.minY + 80 { break }
        }
    }
}

struct VolumeChart: View {
    var days: [DayBar]
    var limit: Int = 60
    var aiSignal: AILatestSignal? = nil

    /// 均量阈值（默认 5 日均量）。AI 给定 volumeWarnRatio 后会用它。
    private func avgVolume(_ view: [DayBar]) -> Double {
        guard !view.isEmpty else { return 0 }
        let sum = view.reduce(0.0) { $0 + $1.volume }
        return sum / Double(view.count)
    }

    var body: some View {
        Canvas { context, size in
            let view = Array(days.suffix(limit))
            guard !view.isEmpty else {
                let text = Text("等待量能…").font(.system(size: 11)).foregroundColor(.secondary)
                context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
                return
            }
            let maxV = max(view.map(\.volume).max() ?? 1, 1)
            let padL: CGFloat = 2, padR: CGFloat = 2, padT: CGFloat = 12, padB: CGFloat = 8
            let bw = max(1.5, (size.width - padL - padR) / CGFloat(view.count) * 0.6)

            // 量能警戒线：AI 给 volumeWarnRatio 时按 (均量 * ratio) 画线；
            // 否则用 5 日均量作为隐含警戒线（参考值，不强提示）。
            let avg = avgVolume(view)
            let warnRatio: Double = (aiSignal?.volumeWarnRatio ?? 0) > 0 ? aiSignal!.volumeWarnRatio : 0
            if avg > 0, warnRatio > 0 {
                let warnV = avg * warnRatio
                let warnY = padT + (1 - CGFloat(warnV / maxV)) * (size.height - padT - padB)
                var path = Path()
                path.move(to: CGPoint(x: padL, y: warnY))
                path.addLine(to: CGPoint(x: size.width - padR, y: warnY))
                context.stroke(path,
                               with: .color(.yellow.opacity(0.6)),
                               style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                let label = Text(String(format: "AI 量比警戒 %.1f×", warnRatio))
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(.yellow)
                context.draw(label, at: CGPoint(x: size.width - 90, y: warnY - 6))
            }

            for (i, d) in view.enumerated() {
                let x = padL + CGFloat(i) / CGFloat(max(view.count - 1, 1)) * (size.width - padL - padR)
                let h = CGFloat(d.volume / maxV) * (size.height - padT - padB)
                let rect = CGRect(x: x - bw / 2, y: size.height - padB - h, width: bw, height: max(h, 1))
                let up = d.close >= d.open
                let c: Color = up
                    ? Color(red: 1, green: 0.27, blue: 0.23).opacity(0.7)
                    : Color(red: 0.2, green: 0.84, blue: 0.29).opacity(0.7)
                context.fill(Path(rect), with: .color(c))
                // 单日放量标记（量 > 5日均 × 警戒倍数 且 AI 给出了方向）
                if avg > 0, warnRatio > 0, d.volume > avg * warnRatio, let sig = aiSignal {
                    let tagColor: Color = sig.verdict == .bull
                        ? Color(red: 1, green: 0.78, blue: 0.15)
                        : (sig.verdict == .bear
                            ? Color(red: 0.36, green: 0.78, blue: 1)
                            : .white)
                    let labelText = Text("放量")
                        .font(.system(size: 9, weight: .heavy))
                        .foregroundColor(tagColor)
                    let resolved = context.resolve(labelText)
                    let tagSize = resolved.measure(in: CGSize(width: 36, height: 12))
                    // 圆角背景：黑色半透明 + 描边
                    let bgRect = CGRect(
                        x: x - tagSize.width / 2 - 3,
                        y: padT + 1,
                        width: tagSize.width + 6,
                        height: tagSize.height + 2
                    )
                    let bg = Path(roundedRect: bgRect, cornerRadius: 3)
                    context.fill(bg, with: .color(.black.opacity(0.75)))
                    context.stroke(bg,
                                   with: .color(tagColor.opacity(0.85)),
                                   style: StrokeStyle(lineWidth: 0.6))
                    context.draw(labelText,
                                 at: CGPoint(x: bgRect.midX, y: bgRect.midY))
                }
            }
        }
        // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
    }
}

/// 命中率时序曲线（ROI #5 / D.1 升级）：
/// 上区 7 日滚动命中率折线（0–100%），下区每日 level 信号数柱（命中部分着色）。
/// 悬停看单日明细与滚动值；空档日一目了然。
struct AccuracyTrendChart: View {
    var days: [AccuracyTimeline.Day]

    @State private var hoverIndex: Int? = nil

    private let hitColor = Color(red: 0.2, green: 0.84, blue: 0.29)
    private let missColor = Color.primary.opacity(0.14)
    private let lineColor = Color.accentColor

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                Canvas { context, size in
                    draw(context: context, size: size)
                }
                // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            hoverIndex = index(atX: value.location.x, in: geo.size)
                        }
                        .onEnded { _ in
                            hoverIndex = nil
                        }
                )

                if let i = hoverIndex, i < days.count, i >= 0 {
                    let d = days[i]
                    let rate = d.total > 0 ? Double(d.hit) / Double(d.total) * 100 : 0
                    let rolling = d.rolling.map { String(format: "%.0f%%", $0 * 100) } ?? "—"
                    VStack(alignment: .leading, spacing: 1) {
                        Text(String(format: "%@ · 信号 %d · 命中 %d", shortDate(d.date), d.total, d.hit))
                        Text(String(format: "当日 %.0f%% · 近7日 %@", rate, rolling))
                    }
                    .font(.system(size: 9, weight: .semibold))
                    .padding(.horizontal, 6).padding(.vertical, 3)
                    .background(Color.black.opacity(0.8), in: RoundedRectangle(cornerRadius: 4))
                    .foregroundColor(.white)
                    .offset(x: min(geo.size.width - 150, max(0, xFor(i, in: geo.size) - 60)),
                            y: 2)
                }
            }
        }
    }

    private func shortDate(_ d: String) -> String {
        String(d.suffix(5)) // yyyy-MM-dd → MM-dd
    }

    private func xFor(_ i: Int, in size: CGSize) -> CGFloat {
        let padL: CGFloat = 2, padR: CGFloat = 2
        let count = max(days.count, 1)
        return padL + (CGFloat(i) + 0.5) / CGFloat(count) * (size.width - padL - padR)
    }

    private func index(atX x: CGFloat, in size: CGSize) -> Int? {
        guard days.count > 0, size.width > 4 else { return nil }
        let count = CGFloat(days.count)
        let width = size.width - 4
        let i = Int(((x - 2) / width) * count)
        return min(max(i, 0), days.count - 1)
    }

    private func draw(context: GraphicsContext, size: CGSize) {
        guard days.count > 1 else { return }
        let padL: CGFloat = 2, padR: CGFloat = 2, padT: CGFloat = 6, padB: CGFloat = 13
        let drawable = CGSize(width: size.width - padL - padR, height: size.height - padT - padB)
        let rateH = drawable.height * 0.62
        let barH = drawable.height * 0.34
        let gap = drawable.height * 0.04
        let barTop = padT + rateH + gap
        let maxTotal = max(days.map(\.total).max() ?? 1, 1)
        let barW = max(1.5, drawable.width / CGFloat(days.count) * 0.55)

        // 50% 参考线（命中率区）
        let fiftyY = padT + (1 - 0.5) * rateH
        var grid = Path()
        grid.move(to: CGPoint(x: padL, y: fiftyY))
        grid.addLine(to: CGPoint(x: size.width - padR, y: fiftyY))
        context.stroke(grid, with: .color(Color.primary.opacity(0.12)),
                       style: StrokeStyle(lineWidth: 0.8, dash: [3, 3]))
        let fiftyLabel = Text("50%").font(.system(size: 8)).foregroundColor(.secondary)
        context.draw(fiftyLabel, at: CGPoint(x: size.width - 12, y: fiftyY - 5))

        // 每日信号柱：命中部分绿色，未命中灰色叠加
        for (i, d) in days.enumerated() {
            guard d.total > 0 else { continue }
            let x = xFor(i, in: size)
            let full = CGFloat(d.total) / CGFloat(maxTotal) * barH
            let hitPart = CGFloat(d.hit) / CGFloat(maxTotal) * barH
            let baseY = padT + drawable.height
            if d.hit > 0 {
                let hitRect = CGRect(x: x - barW / 2, y: baseY - hitPart, width: barW, height: max(hitPart, 1))
                context.fill(Path(hitRect), with: .color(hitColor.opacity(0.75)))
            }
            if d.hit < d.total {
                let missRect = CGRect(x: x - barW / 2, y: baseY - full, width: barW,
                                      height: max(full - hitPart, 1))
                context.fill(Path(missRect), with: .color(missColor))
            }
        }

        // 7 日滚动命中率折线（只在有定义的相邻点之间连线）
        var line = Path()
        var started = false
        for (i, d) in days.enumerated() {
            guard let rate = d.rolling else { started = false; continue }
            let x = xFor(i, in: size)
            let y = padT + (1 - CGFloat(rate)) * rateH
            if started {
                line.addLine(to: CGPoint(x: x, y: y))
            } else {
                line.move(to: CGPoint(x: x, y: y))
                started = true
            }
        }
        context.stroke(line, with: .color(lineColor), style: StrokeStyle(lineWidth: 1.4, lineJoin: .round))
        for (i, d) in days.enumerated() {
            guard let rate = d.rolling, d.total > 0 else { continue }
            let x = xFor(i, in: size)
            let y = padT + (1 - CGFloat(rate)) * rateH
            let dot = CGRect(x: x - 1.5, y: y - 1.5, width: 3, height: 3)
            context.fill(Path(ellipseIn: dot), with: .color(lineColor))
        }

        // X 轴稀疏日期
        for i in [0, days.count / 2, days.count - 1] {
            let label = Text(shortDate(days[i].date))
                .font(.system(size: 8))
                .foregroundColor(.secondary)
            let x = xFor(i, in: size)
            context.draw(label, at: CGPoint(x: x, y: size.height - 5))
        }
    }
}

/// B.2：日 K 多周期——蜡烛图 + MA5/10/20 + 底部成交量条 + hover 十字线。
/// 与分时图上下叠放构成多周期视角（时间轴各自独立：分钟 vs 交易日）。
/// C.2：回测命中标记（策略页跑完回测后画到日 K 上）。
struct DayMark: Equatable, Identifiable {
    var date: String
    /// netPct ≥ 0 → 盈利 ▲；< 0 → 亏损 ✕
    var netPct: Double
    var entry: Double
    var exit: Double
    var id: String { date }
}

struct DayChart: View {
    var days: [DayBar]
    /// 展示最近 N 根（默认 90）
    var limit: Int = 90
    /// C.2：回测命中 / 失效点
    var marks: [DayMark] = []
    /// 顶部预留（最高价标注）
    private let padT: CGFloat = 10
    private let padB: CGFloat = 12
    private let padL: CGFloat = 4
    private let padR: CGFloat = 4
    /// 成交量区占图高比例
    private let volRatio: CGFloat = 0.24

    @State private var hoverIndex: Int? = nil

    private let upColor = Color(red: 1, green: 0.27, blue: 0.23)
    private let downColor = Color(red: 0.2, green: 0.84, blue: 0.29)
    private let maColors: [(Int, Color)] = [
        (5, Color(red: 1, green: 0.84, blue: 0.04)),
        (10, Color(red: 0.39, green: 0.82, blue: 1)),
        (20, Color(red: 0.8, green: 0.5, blue: 1)),
    ]

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                Canvas { context, size in
                    draw(context: context, size: size)
                }
                // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            hoverIndex = index(atX: value.location.x, in: geo.size)
                        }
                        .onEnded { _ in
                            hoverIndex = nil
                        }
                )

                if let i = hoverIndex, i < view.count {
                    let bar = view[i]
                    let up = bar.close >= bar.open
                    VStack(alignment: .leading, spacing: 1) {
                        Text(bar.date)
                        Text(String(format: "开%.2f 高%.2f", bar.open, bar.high))
                        Text(String(format: "低%.2f 收%.2f", bar.low, bar.close))
                        Text(String(format: "量%.0f MA5%@ MA10%@ MA20%@",
                                     bar.volume,
                                     maText(i, period: 5), maText(i, period: 10), maText(i, period: 20)))
                        if let mark = marks.first(where: { $0.date == bar.date }) {
                            Text(String(format: "回测 入%.2f 出%.2f 净%+.1f%%",
                                        mark.entry, mark.exit, mark.netPct))
                                .foregroundColor(mark.netPct >= 0 ? Color(red: 1, green: 0.27, blue: 0.23)
                                                              : Color(red: 0.2, green: 0.84, blue: 0.29))
                        }
                    }
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(.white)
                    .padding(.horizontal, 6).padding(.vertical, 4)
                    .background(Color.black.opacity(0.8), in: RoundedRectangle(cornerRadius: 4))
                    .offset(x: min(geo.size.width - 190, max(0, xFor(i, in: geo.size) - 80)),
                            y: 2)
                    .id(up) // 防布局抖动
                }
            }
        }
    }

    private var view: [DayBar] {
        Array(days.suffix(max(limit, 20)))
    }

    /// 简单移动平均：前 period-1 个为 nil。
    private func maSeries(_ values: [Double], period: Int) -> [Double?] {
        guard values.count >= period else { return values.map { _ in nil } }
        var out: [Double?] = Array(repeating: nil, count: period - 1)
        var sum = values.prefix(period).reduce(0, +)
        out.append(sum / Double(period))
        for i in period..<values.count {
            sum += values[i] - values[i - period]
            out.append(sum / Double(period))
        }
        return out
    }

    private func maText(_ i: Int, period: Int) -> String {
        let closes = view.map(\.close)
        let series = maSeries(closes, period: period)
        guard i < series.count, let v = series[i] else { return "--" }
        return String(format: "%.2f", v)
    }

    private func xFor(_ i: Int, in size: CGSize) -> CGFloat {
        let count = max(view.count, 1)
        return padL + (CGFloat(i) + 0.5) / CGFloat(count) * (size.width - padL - padR)
    }

    private func index(atX x: CGFloat, in size: CGSize) -> Int? {
        guard view.count > 0, size.width > 8 else { return nil }
        let width = size.width - padL - padR
        let i = Int(((x - padL) / width) * CGFloat(view.count))
        return min(max(i, 0), view.count - 1)
    }

    private func draw(context: GraphicsContext, size: CGSize) {
        let view = self.view
        guard !view.isEmpty else {
            let text = Text("等待日 K…").font(.system(size: 11)).foregroundColor(.secondary)
            context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
            return
        }
        let count = view.count
        let chartW = size.width - padL - padR
        let chartH = size.height - padT - padB
        let volH = chartH * volRatio
        let priceH = chartH - volH - 6

        // 价格区间（含 MA 极值，避免线出界）
        let closes = view.map(\.close)
        var hi = view.map(\.high).max() ?? 1
        var lo = view.map(\.low).min() ?? 0
        for (period, _) in maColors {
            for v in maSeries(closes, period: period).compactMap({ $0 }) {
                hi = max(hi, v)
                lo = min(lo, v)
            }
        }
        guard hi > lo else { return }
        let pad = (hi - lo) * 0.02
        hi += pad
        lo -= pad
        func y(_ price: Double) -> CGFloat {
            padT + (1 - CGFloat((price - lo) / (hi - lo))) * priceH
        }

        // 网格：昨收虚线（仅最后一根前收）+ 成交量区分隔
        let last = view.last!
        let lastUp = last.close >= last.open
        var prevLine = Path()
        prevLine.move(to: CGPoint(x: padL, y: y(last.close)))
        prevLine.addLine(to: CGPoint(x: size.width - padR, y: y(last.close)))
        context.stroke(prevLine, with: .color(.white.opacity(0.2)), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
        // 最新收盘价标注（右上）
        let lastLabel = Text(String(format: "%.2f", last.close))
            .font(.system(size: 9, weight: .bold))
            .foregroundColor(lastUp ? upColor : downColor)
        context.draw(lastLabel, at: CGPoint(x: size.width - 26, y: y(last.close) - 6))

        let candleW = max(1.5, chartW / CGFloat(count) * 0.66)

        // 蜡烛：红涨绿跌（A 股），wick 同色
        for (i, bar) in view.enumerated() {
            let x = xFor(i, in: size)
            let up = bar.close >= bar.open
            let color = up ? upColor : downColor
            var wick = Path()
            wick.move(to: CGPoint(x: x, y: y(bar.high)))
            wick.addLine(to: CGPoint(x: x, y: y(bar.low)))
            context.stroke(wick, with: .color(color.opacity(0.85)), lineWidth: 1)
            let top = y(max(bar.open, bar.close))
            let bottom = y(min(bar.open, bar.close))
            let body = CGRect(x: x - candleW / 2, y: top, width: candleW,
                              height: max(bottom - top, 1))
            // 阴线空心（A 股习惯），阳线实心
            if up {
                context.fill(Path(body), with: .color(color.opacity(0.9)))
            } else {
                context.stroke(Path(body), with: .color(color.opacity(0.9)), lineWidth: 1)
                context.fill(Path(body), with: .color(color.opacity(0.12)))
            }
        }

        // MA5 / 10 / 20
        for (period, color) in maColors {
            let series = maSeries(closes, period: period)
            var line = Path()
            var started = false
            for (i, v) in series.enumerated() {
                guard let v else { continue }
                let pt = CGPoint(x: xFor(i, in: size), y: y(v))
                if started {
                    line.addLine(to: pt)
                } else {
                    line.move(to: pt)
                    started = true
                }
            }
            context.stroke(line, with: .color(color.opacity(0.9)), lineWidth: 1.1)
        }

        // 成交量条（底部 24%，随涨跌着色）
        let maxVol = view.map(\.volume).max() ?? 1
        let volTop = padT + priceH + 6
        for (i, bar) in view.enumerated() {
            let x = xFor(i, in: size)
            let up = bar.close >= bar.open
            let h = CGFloat(bar.volume / maxVol) * volH
            let rect = CGRect(x: x - candleW / 2, y: size.height - padB - h,
                              width: candleW, height: max(h, 0.5))
            context.fill(Path(rect), with: .color((up ? upColor : downColor).opacity(0.55)))
        }

        // X 轴稀疏日期（首 / 中 / 尾）
        for i in [0, count / 2, count - 1] {
            let text = Text(String(view[i].date.suffix(5)))
                .font(.system(size: 8))
                .foregroundColor(.secondary)
            context.draw(text, at: CGPoint(x: xFor(i, in: size), y: size.height - 5))
        }

        // C.2：回测命中标记（▲ 盈利红 / ✕ 亏损绿，画在蜡烛高点上方）
        if !marks.isEmpty {
            let markMap = Dictionary(uniqueKeysWithValues: marks.map { ($0.date, $0) })
            for (i, bar) in view.enumerated() {
                guard let mark = markMap[bar.date] else { continue }
                let x = xFor(i, in: size)
                let yTop = y(bar.high) - 8
                let win = mark.netPct >= 0
                let color = win ? upColor : downColor
                let glyph = win ? "▲" : "✕"
                let text = Text(glyph)
                    .font(.system(size: 8, weight: .bold))
                    .foregroundColor(color)
                context.draw(text, at: CGPoint(x: x, y: yTop))
            }
        }

        // hover 十字线
        if let i = hoverIndex, i < count {
            let x = xFor(i, in: size)
            let hy = y(view[i].close)
            var cross = Path()
            cross.move(to: CGPoint(x: x, y: padT))
            cross.addLine(to: CGPoint(x: x, y: size.height - padB))
            cross.move(to: CGPoint(x: padL, y: hy))
            cross.addLine(to: CGPoint(x: size.width - padR, y: hy))
            context.stroke(cross, with: .color(.cyan.opacity(0.55)), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
        }
    }

}


/// B.4：逐笔分钟桶柱状图——x 对齐分时时段槽位，柱高 ∝ 分钟成交量，
/// 净买红 / 净卖绿；点击柱下钻该分钟逐笔（由父视图展示）。
struct TickChart: View {
    var buckets: [TickBucket]
    /// 点选的分钟（"HHmm"）
    @Binding var selectedMinute: String?

    private let upColor = Color(red: 1, green: 0.27, blue: 0.23)
    private let downColor = Color(red: 0.2, green: 0.84, blue: 0.29)
    private let padT: CGFloat = 6
    private let padB: CGFloat = 10
    private let padL: CGFloat = 4
    private let padR: CGFloat = 4

    private var slotIndex: [String: Int] {
        Dictionary(uniqueKeysWithValues: MinuteChart.session.enumerated().map { ($1, $0) })
    }

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                Canvas { context, size in
                    draw(context: context, size: size)
                }
                // drawingGroup：离屏光栅化在 Retina 下会糊，Canvas 直绘即可
                .gesture(
                    DragGesture(minimumDistance: 0)
                        .onChanged { value in
                            selectedMinute = minute(atX: value.location.x, in: geo.size)
                        }
                        .onEnded { _ in }
                )
                if let sel = selectedMinute, let bucket = buckets.first(where: { $0.minute == sel }) {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(String(sel.prefix(2)) + ":" + String(sel.suffix(2)))
                        Text(String(format: "%d 手 · 买 %d / 卖 %d",
                                    bucket.volume, bucket.buyVolume, bucket.sellVolume))
                    }
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundColor(.white)
                    .padding(.horizontal, 5).padding(.vertical, 3)
                    .background(Color.black.opacity(0.8), in: RoundedRectangle(cornerRadius: 4))
                    .offset(x: min(geo.size.width - 110, max(0, xFor(sel, in: geo.size) - 40)), y: 2)
                }
            }
        }
    }

    private func xFor(_ minute: String, in size: CGSize) -> CGFloat {
        let slots = CGFloat(max(MinuteChart.session.count - 1, 1))
        let ratio = slotIndex[minute].map { CGFloat($0) / slots } ?? 0
        return padL + ratio * (size.width - padL - padR)
    }

    private func minute(atX x: CGFloat, in size: CGSize) -> String? {
        guard !buckets.isEmpty, size.width > 8 else { return nil }
        let width = size.width - padL - padR
        let ratio = (x - padL) / width
        let target = ratio * CGFloat(MinuteChart.session.count)
        // 找离目标槽位最近的桶
        var best: (String, Double)?
        for bucket in buckets {
            guard let slot = slotIndex[bucket.minute] else { continue }
            let d = abs(CGFloat(slot) - target)
            if best == nil || d < best!.1 {
                best = (bucket.minute, d)
            }
        }
        return best?.0
    }

    private func draw(context: GraphicsContext, size: CGSize) {
        guard !buckets.isEmpty else {
            let text = Text("暂无逐笔").font(.system(size: 10)).foregroundColor(.secondary)
            context.draw(text, at: CGPoint(x: 8, y: size.height / 2))
            return
        }
        let maxVol = buckets.map(\.volume).max() ?? 1
        let chartH = size.height - padT - padB
        let chartW = size.width - padL - padR
        let barW = max(2.0, chartW / CGFloat(MinuteChart.session.count) * 0.6)
        for bucket in buckets {
            let x = xFor(bucket.minute, in: size)
            let h = CGFloat(Double(bucket.volume) / Double(maxVol)) * chartH
            let color = bucket.netBuy ? upColor : downColor
            let rect = CGRect(x: x - barW / 2, y: size.height - padB - h,
                              width: barW, height: max(h, 0.5))
            context.fill(Path(rect), with: .color(color.opacity(0.65)))
            if bucket.minute == selectedMinute {
                context.stroke(Path(rect), with: .color(.cyan.opacity(0.9)), lineWidth: 1)
            }
        }
        // 选中参考线
        if let sel = selectedMinute {
            let x = xFor(sel, in: size)
            var line = Path()
            line.move(to: CGPoint(x: x, y: padT))
            line.addLine(to: CGPoint(x: x, y: size.height - padB))
            context.stroke(line, with: .color(.cyan.opacity(0.4)),
                           style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
        }
    }
}
