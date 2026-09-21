import SwiftUI
import AppKit
import UniformTypeIdentifiers

private let trendUp = Color(red: 1, green: 0.27, blue: 0.23)
private let trendDown = Color(red: 0.2, green: 0.84, blue: 0.29)

struct ContentView: View {
    @ObservedObject var store: MarketStore
    @ObservedObject var settings: AppSettings
    @State private var showSettings = false
    @State private var showChangelog = false
    @State private var tab: PanelTab = .watch
    @State private var draftRule = ComboStrategy(name: "新策略", conditions: [
        StrategyCond(kind: .pctAbove, value: 3),
        StrategyCond(kind: .volRatioAbove, value: 2)
    ], feePct: BacktestPreset.honest.fee, slipPct: BacktestPreset.honest.slip, oosRatio: BacktestPreset.honest.oos)
    @State private var backtestText = ""
    @State private var newGroup = "默认"
    @State private var diaryQuery = ""
    @State private var strategyWhyText = ""
    @State private var signalKindFilter = "全部"
    @State private var signalCodeFilter = "全部"
    @State private var signalTodayOnly = false
    @State private var showHealth = false
    @State private var addCode = ""
    @State private var addName = ""
    @State private var addGroup = "默认"
    @State private var addHint = ""
    @State private var batchImportText = ""
    @State private var showBatchImport = false
    @State private var tradeStoryText = ""
    @State private var showPositionDetails = false
    @State private var copiedPositionSummary = ""
    /// 阶段 4：复盘页后端报告相关 UI 状态。
    @State private var serverReport: ScheduledReport?
    @State private var reviewHint: String = ""
    @State private var reviewHistory: [ScheduledReport] = []
    @State private var reviewHistoryKind: ReviewKindFilter = .all
    var embeddedInMenu: Bool = false

    enum ReviewKindFilter: Hashable {
        case all, daily, weekly

        var asReviewKind: ReviewKind? {
            switch self {
            case .all: return nil
            case .daily: return .daily
            case .weekly: return .weekly
            }
        }
    }

    private func refreshReviewHistory() async {
        let kind = reviewHistoryKind.asReviewKind
        let list = await store.loadReviewHistory(kind: kind, limit: 14)
        reviewHistory = list
    }

    enum PanelTab: String, CaseIterable {
        case watch = "盯盘"
        case list = "自选"
        case trade = "委托"
        case ai = "AI"
        case strat = "策略"
        case review = "信号"
        var tip: String {
            switch self {
            case .watch: return "行情与指标"
            case .list: return "自选与分组"
            case .trade: return "半自动委托条 · 国盛照抄"
            case .ai: return "实时分析"
            case .strat: return "条件与回测"
            case .review: return "复盘与时间线"
            }
        }
    }

    private var trendColor: Color {
        if store.quote.change > 0 { return trendUp }
        if store.quote.change < 0 { return trendDown }
        return .primary
    }

    private func pctColor(_ pct: Double) -> Color {
        if pct > 0 { return trendUp }
        if pct < 0 { return trendDown }
        return .secondary
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            tabBar
            Group {
                switch tab {
                case .watch: watchTab
                case .list: listTab
                case .trade: tradeTab
                case .ai: aiTab
                case .strat: strategyTab
                case .review: reviewTab
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            footer
        }
        .padding(12)
        .frame(width: embeddedInMenu ? 400 : 420, height: embeddedInMenu ? 720 : nil, alignment: .top)
        .preferredColorScheme(settings.theme.scheme)
        .sheet(isPresented: $showSettings) {
            SettingsView(store: store, settings: settings)
        }
        .sheet(isPresented: $showChangelog) {
            ChangelogView(settings: settings)
        }
        .sheet(isPresented: $showHealth) {
            HealthSheet(store: store)
        }
        .onAppear {
            if settings.needsChangelog { showChangelog = true }
            consumePendingAction()
            store.refreshNotifyQuota()
            store.reloadAIUsage()
        }
        .onChange(of: store.pendingUIAction) { _ in
            consumePendingAction()
        }
    }

    private var tabBar: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                ForEach(PanelTab.allCases, id: \.self) { t in
                    Button {
                        tab = t
                        if t == .review { store.refreshNotifyQuota() }
                        if t == .trade {
                            store.syncTicketFromMarket(forcePrice: store.ticketPrice <= 0)
                            store.reloadTicketHistory()
                        }
                    } label: {
                        Text(t.rawValue)
                            .font(.system(size: 10, weight: tab == t ? .semibold : .regular))
                            .foregroundStyle(tab == t ? Color.primary : .secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 6)
                            .background(
                                tab == t
                                ? Color.accentColor.opacity(0.16)
                                : Color.primary.opacity(0.04),
                                in: RoundedRectangle(cornerRadius: 7, style: .continuous)
                            )
                    }
                    .buttonStyle(.plain)
                    .help(t.tip)
                }
            }
            Text(tab.tip)
                .font(.system(size: 9))
                .foregroundStyle(.tertiary)
        }
    }

    private func consumePendingAction() {
        guard let act = store.pendingUIAction else { return }
        store.pendingUIAction = nil
        switch act {
        case "reanalyze":
            tab = .ai
            store.analyzeWithAI(force: true)
        case "diary":
            tab = .review
        case "stratNote":
            tab = .strat
            strategyWhyText = store.lastStrategyWhy
        case "orderTicket":
            tab = .trade
            store.syncTicketFromMarket(forcePrice: true)
        case "openLevel":
            // 通知点击价位：appDelegate 已经把 payload 推到 store.pendingLevelPayload
            // 这里切到交易 tab，让用户看到草稿委托。
            store.consumePendingLevelPayload()
            tab = .trade
        default:
            break
        }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text("摸金小王子")
                    .font(.system(size: 15, weight: .semibold))
                Text((store.quote.name.isEmpty ? settings.currentSymbol.name : store.quote.name)
                     + " · " + settings.currentSymbol.bareCode)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 2) {
                HStack(spacing: 5) {
                    Circle()
                        .fill(store.liveOK ? Color.green : (store.usingCache ? Color.orange : Color.gray))
                        .frame(width: 6, height: 6)
                    Text(store.quote.price > 0 ? String(format: "%.2f", store.quote.price) : "--")
                        .font(.system(size: 26, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(trendColor)
                    if store.quoteDivergence >= 0.25 {
                        Text(String(format: "分歧%.2f%%", store.quoteDivergence))
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(.orange)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 2)
                            .background(Color.orange.opacity(0.15), in: Capsule())
                            .help(store.quoteSourceDetail)
                    }
                }
                Text(store.quote.price > 0
                     ? String(format: "%@%.2f  %@%.2f%%",
                              store.quote.change >= 0 ? "+" : "", store.quote.change,
                              store.quote.pct >= 0 ? "+" : "", store.quote.pct)
                     : "等待行情…")
                    .font(.system(size: 11, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(trendColor)
                if !store.quoteVoteBadge.isEmpty {
                    Text(store.quoteVoteBadge)
                        .font(.system(size: 9))
                        .foregroundStyle(.secondary)
                        .help(store.quoteSourceDetail)
                }
            }
        }
    }

    private var watchTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                boardIndexStrip
                positionRow
                ohlc
                minutePanel
                indicatorPanel
                levels
                codeInfoPanel
                Button {
                    store.syncTicketFromMarket(forcePrice: true)
                    tab = .trade
                } label: {
                    HStack {
                        Image(systemName: "doc.on.clipboard")
                        Text("半自动委托条")
                        Spacer()
                        Text(store.quote.price > 0
                             ? String(format: "%.2f · 去照抄", store.quote.price)
                             : "去填写")
                            .foregroundStyle(.secondary)
                    }
                    .font(.system(size: 11, weight: .semibold))
                    .padding(8)
                    .background(Color.accentColor.opacity(0.12), in: RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
            }
            .padding(.bottom, 4)
        }
        .onAppear { store.ensureCodeInfo() }
    }

    /// §F.3–F.4：当前标的的概念板块 / 机构研报 / 近期新闻（走网关，点击打开东财原文）。
    private var codeInfoPanel: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 6) {
                Text("资讯 · 研报 · 板块")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)
                if store.codeInfoBusy {
                    ProgressView().controlSize(.mini)
                }
                if let error = store.codeInfoError {
                    Text(error)
                        .font(.system(size: 9))
                        .foregroundStyle(trendUp)
                        .lineLimit(1)
                }
                Spacer()
                Button {
                    store.reloadCodeInfo()
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.plain)
                .controlSize(.mini)
                .help("重新拉取当前标的的新闻 / 研报 / 板块")
            }

            if !store.sectorBoards.isEmpty {
                let boards = store.sectorBoards.sorted {
                    if $0.isPrecise != $1.isPrecise { return $0.isPrecise }
                    return ($0.changePct ?? -99) > ($1.changePct ?? -99)
                }
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 86), spacing: 4)],
                          alignment: .leading, spacing: 4) {
                    ForEach(boards.prefix(9)) { board in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(board.name)
                                .font(.system(size: 9, weight: .semibold))
                                .lineLimit(1)
                            if let pct = board.changePct {
                                Text(String(format: "%@%.2f%%", pct >= 0 ? "+" : "", pct))
                                    .font(.system(size: 9))
                                    .monospacedDigit()
                                    .foregroundStyle(pctColor(pct))
                            } else {
                                Text("—")
                                    .font(.system(size: 9))
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .padding(.horizontal, 5)
                        .padding(.vertical, 3)
                        .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 5))
                        .help(board.reason.isEmpty ? board.name : "\(board.name) · \(board.reason)")
                    }
                }
            }

            if !store.researchReports.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(store.researchReports.prefix(3)) { report in
                        Button {
                            if let url = URL(string: report.url) { NSWorkspace.shared.open(url) }
                        } label: {
                            HStack(spacing: 5) {
                                Text(report.publishDate)
                                    .font(.system(size: 9))
                                    .foregroundStyle(.secondary)
                                Text("\(report.org)·\(report.rating)")
                                    .font(.system(size: 9, weight: .semibold))
                                    .foregroundStyle(
                                        report.isBullish == true ? trendUp :
                                        report.isBullish == false ? trendDown : .secondary)
                                Text(report.ratingChangeText)
                                    .font(.system(size: 8))
                                    .foregroundStyle(.secondary)
                                    .padding(.horizontal, 3)
                                    .padding(.vertical, 1)
                                    .background(Color.primary.opacity(0.06), in: Capsule())
                                Text(report.title)
                                    .font(.system(size: 9))
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                            }
                        }
                        .buttonStyle(.plain)
                        .help("\(report.title)\n\(report.researcher)\(report.industry.isEmpty ? "" : "（\(report.industry)）") · 目标价 \(report.aimPriceText)\n\(report.url)")
                    }
                }
            }

            if !store.newsItems.isEmpty {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(store.newsItems.prefix(4)) { item in
                        Button {
                            if let url = URL(string: item.url) { NSWorkspace.shared.open(url) }
                        } label: {
                            HStack(spacing: 5) {
                                Text(item.cnClock)
                                    .font(.system(size: 9))
                                    .foregroundStyle(.secondary)
                                Text(item.media.isEmpty ? "新闻" : item.media)
                                    .font(.system(size: 9, weight: .semibold))
                                    .foregroundStyle(Color.accentColor)
                                    .lineLimit(1)
                                Text(item.title)
                                    .font(.system(size: 9))
                                    .lineLimit(1)
                                Spacer(minLength: 0)
                            }
                        }
                        .buttonStyle(.plain)
                        .help(item.summary.isEmpty ? item.title : "\(item.title)\n\(item.summary)")
                    }
                }
            }

            if !store.codeInfoBusy, store.codeInfoError == nil,
               store.newsItems.isEmpty, store.researchReports.isEmpty, store.sectorBoards.isEmpty {
                Text("暂无资讯 · 东财无数据或网关未启动")
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(8)
        .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 8))
    }

    private var boardIndexStrip: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Toggle("沪深300", isOn: Binding(
                    get: { settings.boardShowHS300 },
                    set: {
                        settings.setBoardShowHS300($0)
                        Task { await store.refreshBoardIndices() }
                    }
                ))
                .toggleStyle(.checkbox)
                .font(.system(size: 9))
                Toggle("北证50", isOn: Binding(
                    get: { settings.boardShowBJ50 },
                    set: {
                        settings.setBoardShowBJ50($0)
                        Task { await store.refreshBoardIndices() }
                    }
                ))
                .toggleStyle(.checkbox)
                .font(.system(size: 9))
                Spacer()
                if !store.boardAlignBadge.isEmpty, store.quote.price > 0 {
                    Text(store.boardAlignBadge)
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(
                            store.boardAlignBadge.contains("同向") ? Color.accentColor :
                                store.boardAlignBadge == "背离" ? trendUp : .secondary
                        )
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Color.primary.opacity(0.06), in: Capsule())
                        .help("个股涨跌与大盘多数方向对比")
                }
            }

            if store.boardIndices.isEmpty {
                HStack {
                    Text(store.indexFailStreak > 0 ? "大盘失败×\(store.indexFailStreak)" : "大盘加载中…")
                        .font(.system(size: 10))
                        .foregroundStyle(store.indexFailStreak > 0 ? trendUp : Color.secondary)
                    Spacer()
                    Button {
                        Task { await store.refreshBoardIndices() }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .buttonStyle(.plain)
                    .controlSize(.mini)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8))
            } else {
                HStack(spacing: 0) {
                    ForEach(Array(store.boardIndices.enumerated()), id: \.element.id) { idx, item in
                        if idx > 0 {
                            Divider().frame(height: 28)
                        }
                        Button {
                            store.selectBoardIndex(item.code)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 3) {
                                    Text(item.shortName)
                                        .font(.system(size: 9, weight: .semibold))
                                        .foregroundStyle(.secondary)
                                    if store.selectedIndexCode == item.code {
                                        Image(systemName: "chart.xyaxis.line")
                                            .font(.system(size: 8))
                                            .foregroundStyle(Color.accentColor)
                                    }
                                }
                                Text(String(format: "%.2f", item.quote.price))
                                    .font(.system(size: 11, weight: .bold, design: .rounded))
                                    .monospacedDigit()
                                    .foregroundStyle(pctColor(item.quote.pct))
                                Text(String(format: "%@%.2f%%", item.quote.pct >= 0 ? "+" : "", item.quote.pct))
                                    .font(.system(size: 9, weight: .semibold))
                                    .monospacedDigit()
                                    .foregroundStyle(pctColor(item.quote.pct))
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 6)
                            .background(
                                store.selectedIndexCode == item.code
                                ? Color.accentColor.opacity(0.12)
                                : Color.clear
                            )
                        }
                        .buttonStyle(.plain)
                        .help("\(item.quote.name) · 点击看迷你分时")
                    }
                }
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8))
                .contextMenu {
                    Button("刷新大盘") {
                        Task { await store.refreshBoardIndices() }
                    }
                }
            }

            if let code = store.selectedIndexCode {
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("指数分时 · \(store.boardIndices.first(where: { $0.code == code })?.shortName ?? code)")
                            .font(.system(size: 9, weight: .bold))
                            .foregroundStyle(.secondary)
                        Spacer()
                        if store.indexMinuteBusy { ProgressView().controlSize(.mini) }
                        Button("收起") { store.selectedIndexCode = nil; store.indexMinutes = [] }
                            .controlSize(.mini)
                    }
                    MinuteChart(
                        bars: store.indexMinutes,
                        prev: store.boardIndices.first(where: { $0.code == code })?.quote.prev
                            ?? (store.indexMinutes.first?.price ?? 0),
                        base: 0, support: 0, resistance: 0
                    )
                    .frame(height: 72)
                    .background(Color.black.opacity(0.18), in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }

    private var tradeTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Text("半自动委托 · 国盛照抄")
                    .font(.system(size: 12, weight: .bold))
                Text("本页只生成委托文本并复制到剪贴板，不会向券商报单。请到国盛通/同花顺确认。")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: 8) {
                    Picker("方向", selection: Binding(
                        get: { store.ticketSide },
                        set: { store.setTicketSide($0) }
                    )) {
                        ForEach(OrderSide.allCases) { s in
                            Text(s.rawValue).tag(s)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                HStack {
                    Text("标的")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(width: 36, alignment: .leading)
                    Text("\(store.quote.name.isEmpty ? settings.currentSymbol.name : store.quote.name) · \(settings.currentSymbol.bareCode)")
                        .font(.system(size: 12, weight: .semibold))
                    Spacer()
                    Button("跟现价") {
                        store.syncTicketFromMarket(forcePrice: true)
                    }
                    .controlSize(.mini)
                }

                HStack(spacing: 4) {
                    Text("一键").font(.caption2).foregroundStyle(.secondary)
                    Button("到价上") { store.fillTicketFromLevel("above") }.controlSize(.mini)
                    Button("到价下") { store.fillTicketFromLevel("below") }.controlSize(.mini)
                    Button("止损") { store.fillTicketFromLevel("stop") }.controlSize(.mini)
                    Button("阻力") { store.fillTicketFromLevel("resist") }.controlSize(.mini)
                    Button("支撑") { store.fillTicketFromLevel("support") }.controlSize(.mini)
                }
                HStack(spacing: 4) {
                    Text("草稿").font(.caption2).foregroundStyle(.secondary)
                    Button("策略命中") { store.draftTicketFromStrategy() }.controlSize(.mini)
                    Button("AI结论") { store.draftTicketFromAI() }.controlSize(.mini)
                    if !store.suggestedOrderLine.isEmpty {
                        Text(store.suggestedOrderLine)
                            .font(.system(size: 9))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }

                HStack(spacing: 8) {
                    Picker("价", selection: Binding(
                        get: { store.ticketPriceMode },
                        set: { store.applyTicketPriceMode($0) }
                    )) {
                        ForEach(OrderPriceMode.allCases) { m in
                            Text(m.rawValue).tag(m)
                        }
                    }
                    .frame(width: 120)
                    TextField("价格", value: $store.ticketPrice, format: .number.precision(.fractionLength(2)))
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 88)
                    Text("元").font(.caption2).foregroundStyle(.secondary)
                }

                HStack(spacing: 8) {
                    Text("数量")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .frame(width: 36, alignment: .leading)
                    TextField("股数", value: $store.ticketQty, format: .number)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 88)
                    Button("−100") {
                        store.ticketQty = SemiOrderTicket.normalizeLot(max(100, store.ticketQty - 100), code: settings.currentCode)
                    }
                    .controlSize(.mini)
                    Button("+100") {
                        store.ticketQty = SemiOrderTicket.normalizeLot(store.ticketQty + 100, code: settings.currentCode)
                    }
                    .controlSize(.mini)
                    if store.ticketSide == .sell, settings.position.shares > 0 {
                        Button("持仓") {
                            store.setTicketSide(.sell)
                        }
                        .controlSize(.mini)
                    }
                }

                let preview = store.buildCurrentTicket()
                VStack(alignment: .leading, spacing: 4) {
                    Text(preview.oneLine)
                        .font(.system(size: 13, weight: .semibold))
                    Text(String(format: "约额 %.0f 元 · %@", preview.amount, preview.lotOK ? "手数正常" : "请检查手数（科创200/其他100）"))
                        .font(.caption2)
                        .foregroundStyle(preview.lotOK ? .secondary : trendUp)
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8))

                TextField("备注（可选）", text: $store.ticketNote)
                    .textFieldStyle(.roundedBorder)

                HStack {
                    Button {
                        _ = store.copyTicketForBroker()
                    } label: {
                        Label("复制委托条", systemImage: "doc.on.clipboard")
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(store.ticketPrice <= 0 || store.ticketQty <= 0)

                    Button("整手纠正") {
                        store.ticketQty = SemiOrderTicket.normalizeLot(store.ticketQty, code: settings.currentCode)
                    }
                    .controlSize(.small)
                    Spacer()
                }

                Text(store.ticketHint)
                    .font(.caption2)
                    .foregroundStyle(.secondary)

                if !store.ticketHistory.isEmpty {
                    Divider()
                    HStack {
                        Text("最近照抄")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("清空", role: .destructive) {
                            store.clearTicketHistory()
                        }
                        .controlSize(.mini)
                    }
                    ForEach(store.ticketHistory.prefix(8)) { t in
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(t.oneLine)
                                    .font(.system(size: 11, weight: .semibold))
                                Text("\(t.at, style: .time)\(t.filled ? " · 已成交" : "")\(t.source.isEmpty ? "" : " · \(t.source)")")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            if !t.filled {
                                Button("已成交") {
                                    store.markTicketFilled(t.id)
                                }
                                .controlSize(.mini)
                                .help("勾选后回写持仓成本/股数")
                            }
                            Button("再复制") {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(t.clipboardText, forType: .string)
                                store.ticketHint = "已再复制 · \(t.oneLine)"
                            }
                            .controlSize(.mini)
                        }
                        .padding(.vertical, 2)
                    }
                }
            }
            .padding(.bottom, 8)
        }
    }

    private var aiTab: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("实时分析")
                        .font(.system(size: 13, weight: .semibold))
                    Text(store.aiStatus)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Button {
                    store.analyzeWithAI(force: true)
                } label: {
                    if store.aiAnalyzing {
                        ProgressView().controlSize(.small)
                    } else {
                        Text("立即分析")
                    }
                }
                .disabled(store.aiAnalyzing || !settings.aiConfig.enabled || !TokenPlanCatalog.isChat(settings.aiConfig.model))
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .help(aiAnalyzeHelp)
            }

            TokenPlanModelPicker(selection: Binding(
                get: { settings.aiConfig.model },
                set: { settings.setAIModel($0) }
            ), chatOnly: true)

            HStack(spacing: 8) {
                Toggle("自动", isOn: Binding(
                    get: { settings.aiConfig.autoAnalyze },
                    set: { on in
                        var c = settings.aiConfig
                        c.autoAnalyze = on
                        settings.setAIConfig(c)
                    }
                ))
                .disabled(!settings.aiConfig.enabled)
                .toggleStyle(.checkbox)
                Text("间隔 \(settings.aiConfig.intervalSec)s · 变动≥0.3%")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Spacer()
                if !settings.aiConfig.lastGoodModel.isEmpty {
                    Text("上次成功 \(settings.aiConfig.lastGoodModel)")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }

            if !settings.aiConfig.enabled || settings.aiAPIKey.isEmpty {
                Text("在「设置 → AI 接入」配置 Base URL / Token，并开启 AI。模型可在上方直接切换。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("打开设置") { showSettings = true }
                    .controlSize(.small)
            }

            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    if !store.aiEvidence.isEmpty {
                        Text(store.aiEvidence)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(.secondary)
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(6)
                            .background(Color.primary.opacity(0.03))
                            .clipShape(RoundedRectangle(cornerRadius: 6))
                    }
                    if store.aiUsedFallback {
                        HStack {
                            Text("已降级为规则摘要（模型失败）")
                                .font(.caption2)
                                .foregroundStyle(.orange)
                            Spacer()
                            if !settings.aiConfig.lastGoodModel.isEmpty,
                               settings.aiConfig.lastGoodModel != settings.aiConfig.model {
                                Button("切回 \(settings.aiConfig.lastGoodModel)") {
                                    store.switchToLastGoodModel()
                                    store.analyzeWithAI(force: true)
                                }
                                .controlSize(.mini)
                                .buttonStyle(.borderedProminent)
                            }
                        }
                    }
                    Text(store.aiText.isEmpty
                         ? "点「立即分析」或开启自动分析后，会把现价、支撑阻力、MACD/KDJ/RSI/量比打包发给模型。"
                         : store.aiText)
                        .font(.system(size: 12))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(8)
                        .background(Color.primary.opacity(0.04))
                        .clipShape(RoundedRectangle(cornerRadius: 8))

                    aiAccuracyRow
                }
            }
            .frame(minHeight: 120, maxHeight: 200)

            aiUsagePanel
        }
    }

    /// AI 命中回测汇总（基于价位预警事件）：命中率 / 平均领先秒 / 最大偏离
    private var aiAccuracyRow: some View {
        let acc = store.aiAccuracy
        return HStack(spacing: 10) {
            Text("AI 命中")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(.secondary)
            metricChip(label: "命中率",
                       value: acc.total == 0 ? "—"
                              : String(format: "%.0f%% (%d/%d)",
                                       acc.hitRate * 100, acc.hit, acc.total),
                       tint: acc.hitRate >= 0.6 ? .green : (acc.total == 0 ? .secondary : .orange))
            metricChip(label: "平均领先",
                       value: acc.avgLeadSec.map { formatLead($0) } ?? "—",
                       tint: .secondary)
            metricChip(label: "最大偏离",
                       value: acc.total == 0 ? "—"
                              : String(format: "%.2f%%", acc.maxDrawdownPct),
                       tint: acc.maxDrawdownPct > 1 ? .red : .secondary)
        }
        .padding(.top, 4)
    }

    private func metricChip(label: String, value: String, tint: Color) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.system(size: 9))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(tint)
        }
        .padding(.horizontal, 6).padding(.vertical, 3)
        .background(Color.primary.opacity(0.05))
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }

    private func formatLead(_ sec: Int) -> String {
        if sec < 60 { return "\(sec)s" }
        let m = sec / 60, s = sec % 60
        return s == 0 ? "\(m)m" : "\(m)m\(s)s"
    }

    private var aiUsagePanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("用量账本")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)
                Text(String(format: "今日感 %.1f", store.aiTodayFeel))
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .help("非官方账单，按输出长度粗算的 Credits 体感")
                Spacer()
                if !settings.aiConfig.lastGoodModel.isEmpty {
                    Button("成功模型") {
                        store.switchToLastGoodModel()
                    }
                    .controlSize(.mini)
                    .help("切到上次成功的 \(settings.aiConfig.lastGoodModel)")
                }
            }
            if store.aiUsage.isEmpty {
                Text("每次成功/降级会记模型、耗时与 Credits 感。")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            } else {
                ForEach(store.aiUsage.prefix(5)) { e in
                    HStack(spacing: 6) {
                        Circle()
                            .fill(e.ok ? Color.green.opacity(0.8) : Color.orange.opacity(0.8))
                            .frame(width: 6, height: 6)
                        Text("\(e.clock) \(e.model)")
                            .font(.system(size: 10))
                            .lineLimit(1)
                        Spacer()
                        Text("\(e.elapsedMs)ms")
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundStyle(.secondary)
                        Text(String(format: "%.1f", e.creditFeel))
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundStyle(.tertiary)
                    }
                }
            }
        }
        .padding(8)
        .background(Color.primary.opacity(0.03), in: RoundedRectangle(cornerRadius: 8))
    }

    private var aiAnalyzeHelp: String {
        if !settings.aiConfig.enabled { return "请先在设置中开启 AI" }
        if settings.aiAPIKey.isEmpty { return "请先配置 API Token" }
        if !TokenPlanCatalog.isChat(settings.aiConfig.model) { return "当前模型不能做文本分析，请换对话模型" }
        if store.aiAnalyzing { return "分析进行中" }
        return "打包现价与指标发给模型"
    }

    private var positionRow: some View {
        let p = settings.position
        return HStack(spacing: 6) {
            if p.cost > 0, p.shares > 0, let pnl = store.pnl {
                Button {
                    showPositionDetails = true
                } label: {
                    pill(positionResultLabel(pnl.amount),
                         String(format: "¥%.2f", abs(pnl.amount)),
                         color: pctColor(pnl.amount))
                }
                .buttonStyle(.plain)
                .help("查看持仓盈亏详情")
                .popover(isPresented: $showPositionDetails, arrowEdge: .bottom) {
                    positionDetails(p: p, pnl: pnl)
                }
                pill("收益率", String(format: "%@%.2f%%", pnl.pct >= 0 ? "+" : "", pnl.pct),
                     color: pnl.pct >= 0 ? trendUp : trendDown)
                Button {
                    copyPositionSummary(p: p, pnl: pnl)
                } label: {
                    Image(systemName: copiedPositionSummary == positionSummary(p: p, pnl: pnl)
                          ? "checkmark" : "doc.on.doc")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .padding(5)
                        .background(.quaternary.opacity(0.35), in: Circle())
                }
                .buttonStyle(.plain)
                .help("一键复制持仓盈亏")
            } else {
                Button("未填仓位 · 去设置") { showSettings = true }
                    .font(.system(size: 10))
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .help("填写成本与股数后显示持仓盈亏")
            }
            if let d = store.toSupport, settings.levels.support > 0 {
                let mao = d * 10
                pill("距支撑", String(format: "%@%.1f毛", mao >= 0 ? "+" : "", mao),
                     color: d >= 0 ? trendDown : trendUp)
            }
            Spacer()
        }
    }

    private func positionDetails(p: PositionNote, pnl: (amount: Double, pct: Double)) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("当前\(positionResultLabel(pnl.amount))")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.secondary)
            Text(String(format: "¥%.2f", abs(pnl.amount)))
                .font(.system(size: 24, weight: .bold, design: .rounded))
                .monospacedDigit()
                .foregroundStyle(pctColor(pnl.amount))
            Text(String(format: "收益率 %+.2f%%", pnl.pct))
                .font(.system(size: 12, weight: .semibold))
                .monospacedDigit()
            Text(String(format: "现价 ¥%.2f · 成本 ¥%.3f · 持仓 %.0f 股",
                        store.quote.price, p.cost, p.shares))
                .font(.system(size: 10))
                .foregroundStyle(.secondary)
            if store.usingCache {
                Text("按缓存报价估算")
                    .font(.system(size: 10))
                    .foregroundStyle(.orange)
            }
            Button {
                copyPositionSummary(p: p, pnl: pnl)
            } label: {
                Label(copiedPositionSummary == positionSummary(p: p, pnl: pnl) ? "已复制" : "复制盈亏摘要",
                      systemImage: "doc.on.doc")
            }
            .font(.system(size: 11))
        }
        .padding(12)
        .frame(width: 250, alignment: .leading)
    }

    private func positionResultLabel(_ amount: Double) -> String {
        if amount > 0 { return "浮盈" }
        if amount < 0 { return "浮亏" }
        return "持平"
    }

    private func positionSummary(p: PositionNote, pnl: (amount: Double, pct: Double)) -> String {
        let symbol = settings.currentSymbol
        let name = store.quote.name.isEmpty ? symbol.name : store.quote.name
        let quoteNote = store.usingCache ? "（缓存报价）" : ""
        return String(format: "%@（%@）\n现价 ¥%.2f%@ · 成本 ¥%.3f · 持仓 %.0f 股\n%@ ¥%.2f（%+.2f%%）",
                      name, symbol.bareCode, store.quote.price, quoteNote, p.cost, p.shares,
                      positionResultLabel(pnl.amount), abs(pnl.amount), pnl.pct)
    }

    private func copyPositionSummary(p: PositionNote, pnl: (amount: Double, pct: Double)) {
        let summary = positionSummary(p: p, pnl: pnl)
        NSPasteboard.general.clearContents()
        if NSPasteboard.general.setString(summary, forType: .string) {
            copiedPositionSummary = summary
        }
    }

    private var ohlc: some View {
        HStack(spacing: 6) {
            metaCell("今开", store.quote.open)
            metaCell("最高", store.quote.high)
            metaCell("最低", store.quote.low)
            metaCell("昨收", store.quote.prev)
        }
    }

    private func metaCell(_ title: String, _ value: Double) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 10)).foregroundStyle(.secondary)
            Text(value > 0 ? String(format: "%.2f", value) : "--")
                .font(.system(size: 12, weight: .semibold))
                .monospacedDigit()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(8)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
    }

    private var minutePanel: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("分时").font(.system(size: 10, weight: .bold)).foregroundStyle(.secondary)
                Spacer()
                Text(store.minutes.isEmpty
                     ? (store.liveOK ? "暂无分时" : "等待分时…")
                     : "\(store.minutes.count) 点 · 拖动")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
            MinuteChart(
                bars: store.minutes,
                prev: store.quote.prev > 0 ? store.quote.prev : (store.minutes.first?.price ?? 0),
                base: store.base,
                support: store.support,
                resistance: store.resistance,
                aiSignal: store.aiSignal,
                levelLightPct: settings.notifyConfig.levelLightPct,
                levelDeepPct: settings.notifyConfig.levelDeepPct,
                cost: settings.position.cost,
                costLightPct: settings.notifyConfig.costLightPct
            )
            .frame(height: 96)
            .background(Color.black.opacity(0.18), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .padding(8)
        .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var indicatorPanel: some View {
        VStack(alignment: .leading, spacing: 6) {
            Picker("指标", selection: Binding(
                get: { settings.indicator },
                set: { settings.setIndicator($0) }
            )) {
                ForEach(IndicatorKind.allCases) { k in
                    Text(k.rawValue).tag(k)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            Group {
                switch settings.indicator {
                case .macd:
                    MACDChart(days: store.days, points: store.macd, aiSignal: store.aiSignal)
                case .kdj:
                    LineTripleChart(
                        dates: store.days.map(\.date),
                        a: store.kdj.map(\.k), b: store.kdj.map(\.d), c: store.kdj.map(\.j),
                        nameA: "K", nameB: "D", nameC: "J",
                        colorA: Color(red: 1, green: 0.84, blue: 0.04),
                        colorB: Color(red: 0.39, green: 0.82, blue: 1),
                        colorC: trendUp,
                        aiSignal: store.aiSignal
                    )
                case .rsi:
                    LineTripleChart(
                        dates: store.days.map(\.date),
                        a: store.rsi.map(\.value), b: [], c: [],
                        nameA: "RSI", nameB: "", nameC: "",
                        colorA: Color.purple, colorB: .clear, colorC: .clear,
                        aiSignal: store.aiSignal
                    )
                case .vol:
                    VolumeChart(days: store.days, aiSignal: store.aiSignal)
                }
            }
            .frame(height: 108)
            .background(Color.black.opacity(0.18), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            indicatorFooter
        }
        .padding(8)
        .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    @ViewBuilder
    private var indicatorFooter: some View {
        switch settings.indicator {
        case .macd:
            let last = store.lastMACD
            let sig = store.signalText
            VStack(spacing: 6) {
                HStack(spacing: 4) {
                    macdStat("DIF", last?.dif, Color(red: 1, green: 0.84, blue: 0.04))
                    macdStat("DEA", last?.dea, Color(red: 0.39, green: 0.82, blue: 1))
                    macdStat("MACD", last?.hist, (last?.hist ?? 0) >= 0 ? trendUp : trendDown)
                }
                HStack {
                    Text(sig.detail).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1)
                    Spacer()
                    Text(sig.title)
                        .font(.system(size: 10, weight: .bold))
                        .padding(.horizontal, 8).padding(.vertical, 3)
                        .background(sig.title == "金叉" || sig.title == "多头" ? Color(red: 1, green: 0.84, blue: 0.04) : Color(red: 0.37, green: 0.36, blue: 0.9), in: Capsule())
                        .foregroundStyle(sig.title == "金叉" || sig.title == "多头" ? Color.black.opacity(0.85) : Color.white)
                }
            }
        case .kdj:
            let p = store.lastKDJ
            HStack {
                macdStat("K", p?.k, Color(red: 1, green: 0.84, blue: 0.04))
                macdStat("D", p?.d, Color(red: 0.39, green: 0.82, blue: 1))
                macdStat("J", p?.j, trendUp)
            }
        case .rsi:
            HStack {
                macdStat("RSI14", store.lastRSI, Color.purple)
                Text((store.lastRSI ?? 50) >= 70 ? "超买" : ((store.lastRSI ?? 50) <= 30 ? "超卖" : "中性"))
                    .font(.system(size: 11)).foregroundStyle(.secondary)
                Spacer()
            }
        case .vol:
            HStack {
                if let r = store.volRatio {
                    macdStat("量比", r, r >= 2 ? trendUp : .primary)
                    Text(r >= 2 ? "放量异动" : "量能正常")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                } else {
                    Text("量能不足").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                Spacer()
            }
        }
    }

    private func macdStat(_ title: String, _ value: Double?, _ color: Color, text: String? = nil) -> some View {
        VStack(spacing: 2) {
            Text(title).font(.system(size: 9)).foregroundStyle(.secondary)
            Text(text ?? (value.map { String(format: "%.3f", $0) } ?? "--"))
                .font(.system(size: 11, weight: .bold))
                .monospacedDigit()
                .foregroundStyle(color)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 5)
        .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }

    private var levels: some View {
        HStack(spacing: 6) {
            pill("基准", store.base > 0 ? String(format: "%.2f", store.base) : "--")
            pill("阻力", store.resistance > 0 ? String(format: "%.2f", store.resistance) : "--")
            pill("支撑", store.support > 0 ? String(format: "%.2f", store.support) : "--")
            Button {
                showSettings = true
            } label: {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                    .padding(6)
                    .background(.quaternary.opacity(0.35), in: Circle())
            }
            .buttonStyle(.plain)
            .help("设置")
        }
    }

    private func pill(_ title: String, _ value: String, color: Color = .secondary) -> some View {
        Text("\(title) \(value)")
            .font(.system(size: 10))
            .monospacedDigit()
            .foregroundStyle(color)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(.quaternary.opacity(0.35), in: Capsule())
    }

    private var listTab: some View {
        VStack(alignment: .leading, spacing: 8) {
            GroupBox {
                VStack(alignment: .leading, spacing: 6) {
                    Text("自定义添加")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(.secondary)
                    HStack(spacing: 6) {
                        TextField("代码 300623", text: $addCode)
                            .textFieldStyle(.roundedBorder)
                            .frame(minWidth: 90)
                            .onChange(of: addCode) { new in
                                guard addName.isEmpty,
                                      let c = WatchSymbol.normalizeCode(new),
                                      c.count >= 8 else { return }
                                Task {
                                    if let r = await MarketService.resolveSymbolName(code: c) {
                                        if addName.isEmpty { addName = r.name }
                                        addHint = "补全：\(r.name) · \(r.code)"
                                    }
                                }
                            }
                        TextField("名称(可空)", text: $addName)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 72)
                        TextField("分组", text: $addGroup)
                            .textFieldStyle(.roundedBorder)
                            .frame(width: 52)
                        Button("添加") { addCustomSymbol() }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.small)
                            .disabled(addCode.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    HStack {
                        Button(showBatchImport ? "收起批量" : "批量导入") {
                            showBatchImport.toggle()
                        }
                        .controlSize(.mini)
                        if !store.staleSymbolHints.isEmpty {
                            Text("可能失效：\(store.staleSymbolHints.prefix(4).joined(separator: ","))")
                                .font(.caption2)
                                .foregroundStyle(trendUp)
                                .lineLimit(1)
                        }
                    }
                    if showBatchImport {
                        TextEditor(text: $batchImportText)
                            .font(.system(size: 11, design: .monospaced))
                            .frame(height: 72)
                            .scrollContentBackground(.hidden)
                            .padding(4)
                            .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 6))
                        Button("导入") {
                            let r = settings.batchImportSymbols(batchImportText, defaultGroup: addGroup)
                            addHint = r.line
                            if !r.invalid.isEmpty {
                                addHint += " · 无效:" + r.invalid.prefix(3).joined(separator: ",")
                            }
                            batchImportText = ""
                            showBatchImport = false
                            Task { await store.refreshWatchlistNow() }
                        }
                        .controlSize(.small)
                        Text("每行：代码 或 代码,名称,分组")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }
                    Text(addHint.isEmpty ? "支持 6 位码 / sz300623 / 300623.SZ；名称空则拉行情补全" : addHint)
                        .font(.caption2)
                        .foregroundStyle(addHint.contains("失败") || addHint.contains("无效") ? trendUp : .secondary)
                        .lineLimit(2)
                }
                .padding(4)
            }

            HStack {
                Text("\(settings.symbols.count) 只自选")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Button("刷新") {
                    Task { await store.refreshWatchlistNow() }
                }
                .controlSize(.small)
                .help("弱网批量刷新报价")
            }
            List {
                ForEach(settings.groups, id: \.self) { g in
                    Section {
                        if !store.collapsedGroups.contains(g) {
                            ForEach(settings.symbols.filter { $0.group == g }) { s in
                                symbolRow(s)
                                    .opacity(store.staleSymbolHints.contains(s.code) ? 0.55 : 1)
                                    .contextMenu {
                                        Button("盯盘") {
                                            store.switchSymbol(s.code)
                                            tab = .watch
                                        }
                                        Button("委托条") {
                                            store.switchSymbol(s.code)
                                            tab = .trade
                                        }
                                        Button("删除", role: .destructive) {
                                            settings.removeSymbol(s.code)
                                            store.switchSymbol(settings.currentCode)
                                            addHint = "已删除 \(s.name)"
                                        }
                                        .disabled(settings.symbols.count <= 1)
                                    }
                            }
                            .onMove { from, to in
                                settings.moveWithinGroup(g, from: from, to: to)
                            }
                            .onDelete { idx in
                                let list = settings.symbols.filter { $0.group == g }
                                for i in idx {
                                    guard list.indices.contains(i) else { continue }
                                    settings.removeSymbol(list[i].code)
                                }
                                store.switchSymbol(settings.currentCode)
                            }
                        }
                    } header: {
                        Button {
                            if store.collapsedGroups.contains(g) {
                                store.collapsedGroups.remove(g)
                            } else {
                                store.collapsedGroups.insert(g)
                            }
                        } label: {
                            HStack {
                                Image(systemName: store.collapsedGroups.contains(g) ? "chevron.right" : "chevron.down")
                                Text(g)
                                Text("(\(settings.symbols.filter { $0.group == g }.count))")
                                    .foregroundStyle(.secondary)
                                Spacer()
                            }
                            .font(.system(size: 11, weight: .semibold))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .help("点击折叠/展开分组；拖动手柄排序；星标置顶")
                    }
                }
            }
            .listStyle(.inset)
            .frame(minHeight: 240)
        }
    }

    private func addCustomSymbol() {
        let outcome = settings.addSymbol(code: addCode, name: addName, group: addGroup)
        switch outcome {
        case .invalid:
            addHint = "无效代码，请用 6 位或带 sh/sz/bj 前缀"
        case .exists(let name):
            addHint = "已在自选：\(name)"
            store.switchSymbol(settings.currentCode)
            Task { await store.refreshWatchlistNow() }
        case .added(let code):
            addHint = "已添加 \(code)"
            addCode = ""
            addName = ""
            store.switchSymbol(code)
            Task { await store.refreshWatchlistNow() }
        }
    }

    private func symbolRow(_ s: WatchSymbol) -> some View {
        let q = store.watchQuotes[s.code]
        let on = s.code == settings.currentCode
        return HStack(spacing: 8) {
            Button {
                settings.togglePin(s.code)
            } label: {
                Image(systemName: s.pinned ? "star.fill" : "star")
                    .foregroundStyle(s.pinned ? Color.yellow : .secondary)
            }
            .buttonStyle(.plain)
            .help(s.pinned ? "取消置顶" : "置顶")

            Button {
                store.switchSymbol(s.code)
                tab = .watch
            } label: {
                HStack {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(s.name).font(.system(size: 12, weight: .semibold))
                        Text(s.code).font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if let q, q.price > 0 {
                        VStack(alignment: .trailing, spacing: 2) {
                            Text(String(format: "%.2f", q.price))
                                .font(.system(size: 13, weight: .bold)).monospacedDigit()
                            Text(String(format: "%@%.2f%%", q.pct >= 0 ? "+" : "", q.pct))
                                .font(.system(size: 10))
                                .foregroundStyle(pctColor(q.pct))
                        }
                    } else {
                        Text("--").foregroundStyle(.secondary)
                    }
                }
            }
            .buttonStyle(.plain)

            Menu {
                ForEach(["默认", "核心", "观察", "银行", "消费", "科技"], id: \.self) { g in
                    Button(g) { settings.setGroup(s.code, group: g) }
                }
            } label: {
                Image(systemName: "folder")
                    .foregroundStyle(.secondary)
            }
            .help("移动到分组")
        }
        .listRowBackground(on ? Color.accentColor.opacity(0.12) : Color.clear)
        .padding(.vertical, 2)
    }

    private func moveItem(code: String, delta: Int) {
        guard let i = settings.symbols.firstIndex(where: { $0.code == code }) else { return }
        let j = i + delta
        guard settings.symbols.indices.contains(j) else { return }
        settings.symbols.swapAt(i, j)
        settings.save()
    }

    private var strategyTab: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                Text("条件组合（价+量+指标）· 盘后回测")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)

                TextField("策略名", text: $draftRule.name)
                    .textFieldStyle(.roundedBorder)

                Picker("作用域", selection: $draftRule.scope) {
                    ForEach(StrategyScope.allCases) { s in Text(s.rawValue).tag(s) }
                }
                if draftRule.scope == .group {
                    Picker("分组", selection: $draftRule.scopeGroup) {
                        ForEach(settings.groups, id: \.self) { g in Text(g).tag(g) }
                    }
                }
                if draftRule.scope == .codes {
                    TextField("代码 sz300623", text: $draftRule.code)
                        .textFieldStyle(.roundedBorder)
                }

                Toggle("全部满足(AND) / 关闭则任一(OR)", isOn: $draftRule.requireAll)

                // 条件树可视化
                VStack(alignment: .leading, spacing: 4) {
                    Text("条件树 · \(draftRule.requireAll ? "AND" : "OR")")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(.secondary)
                    ForEach(Array(draftRule.conditions.enumerated()), id: \.element.id) { idx, c in
                        HStack(spacing: 4) {
                            Text(idx == 0 ? "┌" : (idx == draftRule.conditions.count - 1 ? "└" : "├"))
                                .foregroundStyle(.tertiary)
                            Text(c.kind.rawValue + (c.kind.needsValue ? String(format: " %.2f", c.value) : ""))
                                .font(.system(size: 11, design: .monospaced))
                        }
                    }
                    // 命中热力
                    let snap = store.currentStrategySnapshot()
                    let hits = StrategyEngine.evaluateHits(draftRule, snap: snap)
                    if store.quote.price > 0, !hits.isEmpty {
                        HStack(spacing: 4) {
                            Text("热力").font(.system(size: 10)).foregroundStyle(.secondary)
                            ForEach(hits, id: \.cond.id) { item in
                                Circle()
                                    .fill(item.hit ? Color.green.opacity(0.85) : Color.gray.opacity(0.35))
                                    .frame(width: 10, height: 10)
                                    .help(item.cond.kind.rawValue)
                            }
                            Text(StrategyEngine.evaluate(draftRule, snap: snap) ? "触发中" : "未触发")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(StrategyEngine.evaluate(draftRule, snap: snap) ? trendUp : .secondary)
                        }
                        Button("AI 对照为何触发") {
                            strategyWhyText = StrategyEngine.whyTriggered(draftRule, snap: snap)
                            store.lastStrategyWhy = strategyWhyText
                            if settings.aiConfig.enabled {
                                store.analyzeWithAI(force: true)
                                tab = .ai
                            }
                        }
                        .font(.system(size: 11))
                    }
                    if !strategyWhyText.isEmpty {
                        Text(strategyWhyText)
                            .font(.system(size: 10, design: .monospaced))
                            .padding(6)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(.quaternary.opacity(0.2), in: RoundedRectangle(cornerRadius: 6))
                    }
                }

                HStack {
                    Text("成本档").font(.system(size: 10)).foregroundStyle(.secondary)
                    ForEach(BacktestPreset.allCases) { p in
                        Button(p.rawValue) {
                            draftRule.feePct = p.fee
                            draftRule.slipPct = p.slip
                            draftRule.oosRatio = p.oos
                        }
                        .controlSize(.mini)
                    }
                }

                HStack {
                    Text("手续费%").font(.system(size: 10)).foregroundStyle(.secondary)
                    TextField("0.05", value: $draftRule.feePct, format: .number)
                        .textFieldStyle(.roundedBorder).frame(width: 50)
                    Text("滑点%").font(.system(size: 10)).foregroundStyle(.secondary)
                    TextField("0.10", value: $draftRule.slipPct, format: .number)
                        .textFieldStyle(.roundedBorder).frame(width: 50)
                    Text("样本外").font(.system(size: 10)).foregroundStyle(.secondary)
                    TextField("0.3", value: $draftRule.oosRatio, format: .number)
                        .textFieldStyle(.roundedBorder).frame(width: 44)
                }

                ForEach($draftRule.conditions) { $c in
                    HStack {
                        Picker("", selection: $c.kind) {
                            ForEach(CondKind.allCases) { k in Text(k.rawValue).tag(k) }
                        }
                        .frame(width: 110)
                        if c.kind.needsValue {
                            TextField("值", value: $c.value, format: .number)
                                .textFieldStyle(.roundedBorder)
                                .frame(width: 70)
                        }
                        Button(role: .destructive) {
                            draftRule.conditions.removeAll { $0.id == c.id }
                        } label: { Image(systemName: "minus.circle") }
                        .buttonStyle(.plain)
                    }
                }

                HStack {
                    Button("加条件") {
                        draftRule.conditions.append(StrategyCond(kind: .rsiAbove, value: 70))
                    }
                    Button("保存策略") {
                        settings.upsertCombo(draftRule)
                        draftRule = ComboStrategy(name: "新策略", conditions: [
                            StrategyCond(kind: .pctAbove, value: 3),
                            StrategyCond(kind: .volRatioAbove, value: 2)
                        ], feePct: BacktestPreset.honest.fee, slipPct: BacktestPreset.honest.slip, oosRatio: BacktestPreset.honest.oos)
                    }
                    Spacer()
                }
                HStack {
                    Button("回测作用域") {
                        Task {
                            let r = await store.runBacktestScoped(draftRule)
                            backtestText = r.summary
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
                    .disabled(store.backtestBusy)
                    if store.backtestBusy {
                        ProgressView().controlSize(.small)
                        Text("拉日线…").font(.caption2).foregroundStyle(.secondary)
                    }
                    Button("导出 CSV") {
                        exportBacktestCSV()
                    }
                    .controlSize(.small)
                    .disabled(store.lastBacktest == nil)
                    Spacer()
                }
                .font(.system(size: 11))
                Text("按策略作用域拉各股日线缓存后回测，不再只用当前盯盘。")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)

                if !backtestText.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        if let r = store.lastBacktest {
                            HStack(spacing: 6) {
                                Text(r.honestyBadge)
                                    .font(.system(size: 10, weight: .bold))
                                    .foregroundStyle(
                                        (r.oosBeatsIS == true) ? trendDown :
                                            (r.oosBeatsIS == false || r.outSample.signals == 0) ? trendUp : .secondary
                                    )
                                    .padding(.horizontal, 6)
                                    .padding(.vertical, 3)
                                    .background(.quaternary.opacity(0.4), in: Capsule())
                                if r.trailingLosses >= 3 {
                                    Text("连亏\(r.trailingLosses)")
                                        .font(.system(size: 10, weight: .bold))
                                        .foregroundStyle(trendUp)
                                }
                                Spacer()
                            }
                            Text(r.honestyWarning)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(r.honestyWarning.hasPrefix("⚠") ? trendUp : .secondary)
                        }
                        Text(backtestText)
                            .font(.system(size: 11))
                    }
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 8))
                }

                Divider()
                Text("已保存").font(.system(size: 10, weight: .bold)).foregroundStyle(.secondary)
                ForEach(settings.comboStrategies) { rule in
                    HStack {
                        Toggle(rule.name + " · " + rule.scopeLabel, isOn: Binding(
                            get: { rule.enabled },
                            set: { on in
                                var r = rule
                                r.enabled = on
                                settings.upsertCombo(r)
                            }
                        ))
                        Spacer()
                        Button("回测") {
                            draftRule = rule
                            Task {
                                let r = await store.runBacktestScoped(rule)
                                backtestText = r.summary
                            }
                        }
                        .disabled(store.backtestBusy)
                        Button("编辑") { draftRule = rule }
                        Button("删", role: .destructive) { settings.removeCombo(rule.id) }
                    }
                    .font(.system(size: 11))
                }
            }
            .padding(.bottom, 8)
        }
        .frame(minHeight: 280, alignment: .top)
    }

    private var reviewTab: some View {
        let day = MarketStore.todayString()
        let hits = settings.searchDiaries(code: nil, query: diaryQuery, limit: 8)
        return ScrollView {
            VStack(alignment: .leading, spacing: 8) {
            Text("关键价位  基准 \(fmt(store.base))  阻力 \(fmt(store.resistance))  支撑 \(fmt(store.support))")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Text("日记 · \(day) · \(settings.currentSymbol.name)")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(.secondary)
            TextEditor(text: Binding(
                get: { settings.diaryText(day: day) },
                set: { settings.setDiary(day: day, text: $0) }
            ))
            .font(.system(size: 12))
            .frame(height: 80)
            .scrollContentBackground(.hidden)
            .padding(6)
            .background(.quaternary.opacity(0.2), in: RoundedRectangle(cornerRadius: 8))

            GroupBox {
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("复盘工作台 · 交易故事")
                            .font(.system(size: 10, weight: .bold))
                        Spacer()
                        Button("生成今日") {
                            store.submitReview(kind: .daily) { result in
                                switch result {
                                case .success(let report):
                                    serverReport = report
                                    tradeStoryText = report.body
                                    reviewHint = "已落库 · \(report.title) · \(report.periodKey)"
                                    Task { await refreshReviewHistory() }
                                case .failure(let err):
                                    reviewHint = "生成失败：\(err.localizedDescription)"
                                }
                            }
                        }
                        .controlSize(.mini)
                        Button("生成本周") {
                            store.submitReview(kind: .weekly) { result in
                                switch result {
                                case .success(let report):
                                    serverReport = report
                                    tradeStoryText = report.body
                                    reviewHint = "已落库 · \(report.title) · \(report.periodKey)"
                                    Task { await refreshReviewHistory() }
                                case .failure(let err):
                                    reviewHint = "生成失败：\(err.localizedDescription)"
                                }
                            }
                        }
                        .controlSize(.mini)
                        Button("本地预览") {
                            tradeStoryText = store.tradeStoryText(day: day)
                            reviewHint = "本地拼接（未上后端）"
                        }
                        .controlSize(.mini)
                        Button("复制") {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(tradeStoryText, forType: .string)
                        }
                        .controlSize(.mini)
                        .disabled(tradeStoryText.isEmpty)
                    }
                    if !reviewHint.isEmpty {
                        Text(reviewHint)
                            .font(.system(size: 9))
                            .foregroundStyle(.tertiary)
                    }
                    Text(tradeStoryText.isEmpty ? "点「生成今日 / 生成本周」走后端，或「本地预览」拼一份" : tradeStoryText)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(tradeStoryText.isEmpty ? .tertiary : .primary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .frame(minHeight: 60)
                }
                .padding(4)
            }

            // 复盘历史（从后端拉，按 kind 过滤）
            GroupBox {
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("复盘历史 · 后端持久化")
                            .font(.system(size: 10, weight: .bold))
                        Spacer()
                        Picker("", selection: $reviewHistoryKind) {
                            Text("全部").tag(ReviewKindFilter.all)
                            Text("日报").tag(ReviewKindFilter.daily)
                            Text("周报").tag(ReviewKindFilter.weekly)
                        }
                        .pickerStyle(.segmented)
                        .frame(width: 160)
                        Button("刷新") {
                            Task { await refreshReviewHistory() }
                        }
                        .controlSize(.mini)
                    }
                    if reviewHistory.isEmpty {
                        Text("暂无报告。先点「生成今日 / 生成本周」")
                            .font(.system(size: 10))
                            .foregroundStyle(.tertiary)
                    } else {
                        ForEach(reviewHistory) { r in
                            HStack(alignment: .top, spacing: 8) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(r.title)
                                        .font(.system(size: 10, weight: .semibold))
                                    Text("\(r.kind) · \(r.periodKey) · \(r.createdAt.formatted(date: .abbreviated, time: .shortened))")
                                        .font(.system(size: 9))
                                        .foregroundStyle(.secondary)
                                    Text(r.body.prefix(140).description)
                                        .font(.system(size: 9, design: .monospaced))
                                        .foregroundStyle(.secondary)
                                        .lineLimit(3)
                                }
                                Spacer()
                                Button("展开") {
                                    tradeStoryText = r.body
                                    serverReport = r
                                    reviewHint = "已加载 · \(r.title)"
                                }
                                .controlSize(.mini)
                            }
                            .padding(.vertical, 2)
                        }
                    }
                }
                .padding(4)
            }
            .onAppear { Task { await refreshReviewHistory() } }

            TextField("检索复盘（标的/日期/关键词）", text: $diaryQuery)
                .textFieldStyle(.roundedBorder)
            if !hits.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(hits.enumerated()), id: \.offset) { _, h in
                        Button {
                            store.switchSymbol(h.code)
                            let cur = settings.diaryText(day: day)
                            let ref = "参考 \(h.day)：\(String(h.text.prefix(80)))"
                            settings.setDiary(day: day, text: cur.isEmpty ? ref : cur + "\n" + ref)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("\(h.day) · \(h.code)")
                                    .font(.system(size: 10, weight: .semibold))
                                Text(String(h.text.prefix(100)))
                                    .font(.system(size: 10))
                                    .foregroundStyle(.secondary)
                                    .lineLimit(2)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .buttonStyle(.plain)
                    }
                }
                .frame(maxHeight: 80)
            }

            HStack {
                Button("导出今日分时 CSV") { exportCSV() }
                Button("喂给 AI 对照") {
                    tab = .ai
                    store.analyzeWithAI(force: true)
                }
                .font(.system(size: 11))
                Spacer()
                Text("\(store.minutes.count) 根")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }

            Divider()
            signalTimelinePanel
            }
            .padding(.bottom, 8)
        }
        .frame(minHeight: 280, alignment: .top)
    }

    private var signalTimelinePanel: some View {
        let codes = Array(Set(store.signalEvents.map(\.code))).sorted()
        let filtered = store.signalEvents.filter { ev in
            if signalTodayOnly {
                let f = DateFormatter()
                f.locale = Locale(identifier: "en_US_POSIX")
                f.timeZone = TimeZone(identifier: "Asia/Shanghai")
                f.dateFormat = "yyyy-MM-dd"
                if f.string(from: ev.at) != MarketStore.todayString() { return false }
            }
            if signalKindFilter != "全部" {
                switch signalKindFilter {
                case "AI":
                    if ev.kind != "ai" { return false }
                case "策略":
                    if ev.kind != "strategy" { return false }
                case "分歧":
                    if ev.kind != "diverge" { return false }
                case "委托":
                    if ev.kind != "order" { return false }
                case "预警":
                    if ["ai", "strategy", "diverge", "order"].contains(ev.kind) { return false }
                default: break
                }
            }
            if signalCodeFilter != "全部", ev.code != signalCodeFilter { return false }
            return true
        }
        return VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("信号时间线")
                    .font(.system(size: 10, weight: .bold))
                    .foregroundStyle(.secondary)
                Spacer()
                Text(store.notifyQuotaLine.isEmpty ? "通知配额…" : store.notifyQuotaLine)
                    .font(.system(size: 9))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                Button("清空", role: .destructive) {
                    SignalTimeline.clear()
                    store.signalEvents = []
                    store.selectedSignalID = nil
                }
                .controlSize(.mini)
            }
            HStack(spacing: 6) {
                Picker("类型", selection: $signalKindFilter) {
                    ForEach(["全部", "预警", "AI", "策略", "分歧", "委托"], id: \.self) { Text($0).tag($0) }
                }
                .frame(width: 88)
                Picker("标的", selection: $signalCodeFilter) {
                    Text("全部").tag("全部")
                    ForEach(codes, id: \.self) { Text($0).tag($0) }
                }
                .frame(maxWidth: .infinity)
                Toggle("今日", isOn: $signalTodayOnly)
                    .toggleStyle(.checkbox)
                    .font(.system(size: 10))
            }
            .controlSize(.mini)

            if filtered.isEmpty {
                Text(store.signalEvents.isEmpty
                      ? "预警 / AI / 策略触发会落在这里。"
                      : "当前筛选无结果。")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(filtered.prefix(40)) { ev in
                            Button {
                                store.selectedSignalID = (store.selectedSignalID == ev.id) ? nil : ev.id
                            } label: {
                                HStack(alignment: .top, spacing: 6) {
                                    Text(ev.kindLabel)
                                        .font(.system(size: 9, weight: .bold))
                                        .foregroundStyle(.white)
                                        .padding(.horizontal, 4)
                                        .padding(.vertical, 1)
                                        .background(ev.kind == "ai" ? Color.blue : (ev.kind == "strategy" ? Color.purple : Color.orange), in: Capsule())
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text("\(ev.clock) · \(ev.title)")
                                            .font(.system(size: 10, weight: .semibold))
                                            .lineLimit(1)
                                        Text(String(format: "%@  %.2f  %@", ev.code, ev.price, ev.source))
                                            .font(.system(size: 9))
                                            .foregroundStyle(.secondary)
                                    }
                                    Spacer(minLength: 0)
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if store.selectedSignalID == ev.id {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text("当时为什么报")
                                        .font(.system(size: 9, weight: .bold))
                                        .foregroundStyle(.secondary)
                                    Text(ev.why.isEmpty ? ev.body : ev.why)
                                        .font(.system(size: 10))
                                        .textSelection(.enabled)
                                    Text(ev.evidence)
                                        .font(.system(size: 9, design: .monospaced))
                                        .foregroundStyle(.secondary)
                                        .textSelection(.enabled)
                                    Button("跳回盯盘 · 当时价 \(String(format: "%.2f", ev.price))") {
                                        if !ev.code.isEmpty { store.switchSymbol(ev.code) }
                                        tab = .watch
                                    }
                                    .controlSize(.small)
                                    .buttonStyle(.borderedProminent)
                                }
                                .padding(6)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 6))
                            }
                        }
                    }
                }
                .frame(maxHeight: 180)
            }
        }
    }

    private func fmt(_ v: Double) -> String { v > 0 ? String(format: "%.2f", v) : "--" }

    private func exportCSV() {
        guard let tmp = store.exportMinutesCSV() else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.commaSeparatedText]
        panel.nameFieldStringValue = tmp.lastPathComponent
        if panel.runModal() == .OK, let dest = panel.url {
            try? FileManager.default.removeItem(at: dest)
            try? FileManager.default.copyItem(at: tmp, to: dest)
        }
    }

    private func exportBacktestCSV() {
        guard let tmp = store.exportBacktestCSV(ruleName: draftRule.name) else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.commaSeparatedText]
        panel.nameFieldStringValue = tmp.lastPathComponent
        if panel.runModal() == .OK, let dest = panel.url {
            try? FileManager.default.removeItem(at: dest)
            try? FileManager.default.copyItem(at: tmp, to: dest)
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Button {
                showHealth = true
                Task { await store.probeNetworkHealth() }
            } label: {
                VStack(alignment: .leading, spacing: 1) {
                    Text(store.quote.timeText)
                        .font(.system(size: 10, design: .monospaced))
                    Text(store.healthReport?.line ?? shortStatus)
                        .font(.system(size: 9))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            .buttonStyle(.plain)
            .help("点开弱网体检：三源延迟 / 失败 / 缓存年龄")
            Spacer(minLength: 4)
            Button {
                tab = .review
                store.refreshNotifyQuota()
            } label: {
                Image(systemName: "clock.arrow.circlepath")
            }
            .controlSize(.small)
            .help("信号时间线 / 复盘")
            Button {
                showSettings = true
            } label: {
                Image(systemName: "gearshape")
            }
            .controlSize(.small)
            .help("设置")
            Button {
                NSApp.terminate(nil)
            } label: {
                Image(systemName: "xmark.circle")
            }
            .controlSize(.small)
            .help("退出")
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.secondary)
    }

    private var shortStatus: String {
        var s = store.status
        if s.count > 28 { s = String(s.prefix(26)) + "…" }
        return s
    }
}

struct HealthSheet: View {
    @ObservedObject var store: MarketStore
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("弱网体检").font(.headline)
                Spacer()
                Button("重测") {
                    Task { await store.probeNetworkHealth() }
                }
                .disabled(store.healthProbing)
                Button("关闭") { dismiss() }
            }
            if store.healthProbing {
                ProgressView("探测三源…")
            } else if let r = store.healthReport {
                Text(r.line)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                ForEach(r.probes) { p in
                    HStack {
                        Circle()
                            .fill(p.ok ? Color.green : Color.orange)
                            .frame(width: 8, height: 8)
                        Text(p.name).font(.system(size: 12, weight: .semibold))
                        Spacer()
                        Text("\(p.latencyMs) ms")
                            .font(.system(size: 11, design: .monospaced))
                        Text(p.detail)
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Divider()
                Text("盯盘 \(r.cacheCode) · 个股缓存 \(r.cacheAgeSec.map { "\($0)s" } ?? "无") · 直播\(r.liveOK ? "OK" : "否") · 用缓存\(r.usingCache ? "是" : "否") · 失败连击 \(r.failStreak)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                if !r.indexProbes.isEmpty {
                    Text("大盘探测 · 失败连击 \(r.indexFailStreak) · 指数缓存 \(r.indexCacheAgeSec.map { "\($0)s" } ?? "无")")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    ForEach(r.indexProbes) { p in
                        HStack {
                            Circle()
                                .fill(p.ok ? Color.green : Color.orange)
                                .frame(width: 8, height: 8)
                            Text(p.name).font(.system(size: 12, weight: .semibold))
                            Spacer()
                            Text("\(p.latencyMs) ms")
                                .font(.system(size: 11, design: .monospaced))
                            Text(p.detail)
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }
                    }
                }
            } else {
                Text("点击重测开始。")
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(16)
        .frame(width: 420, height: 360)
        .onAppear {
            if store.healthReport == nil {
                Task { await store.probeNetworkHealth() }
            }
        }
    }
}

struct SettingsView: View {
    @ObservedObject var store: MarketStore
    @ObservedObject var settings: AppSettings
    @Environment(\.dismiss) private var dismiss

    @State private var baseText = ""
    @State private var supportText = ""
    @State private var resistText = ""
    @State private var costText = ""
    @State private var sharesText = ""
    @State private var stopText = ""
    @State private var posPctText = ""
    @State private var aboveText = ""
    @State private var belowText = ""
    @State private var ddText = ""
    @State private var coolText = "30"
    @State private var newCode = ""
    @State private var newName = ""
    @State private var newGroupText = "默认"
    @State private var addSymbolHint = ""
    @State private var cooldownText = "300"
    @State private var aggregateText = "90"
    @State private var dndStart = "22:00"
    @State private var dndEnd = "08:00"
    @State private var levelLightDraft: Double = 0.30
    @State private var levelDeepDraft: Double = 0.15
    @State private var costLightDraft: Double = 1.0
    @State private var showLog = false
    @State private var aiBase = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    @State private var aiModel = "qwen3.7-plus"
    @State private var aiProvider = "openai"
    @State private var aiKey = ""
    @State private var aiInterval = "120"
    @State private var aiMaxTokens = "600"
    @State private var gatewayEnabled = true
    @State private var serverURLDraft = "http://127.0.0.1:8732"
    @State private var gatewayTestStatus = ""
    @State private var gatewayTesting = false
    @State private var section: SettingsSection = .ai

    enum SettingsSection: String, CaseIterable, Identifiable {
        case ai = "AI"
        case quote = "行情"
        case notify = "通知"
        case look = "外观"
        var id: String { rawValue }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("摸金设置").font(.headline)
                Spacer()
                Button("完成") { persist(); dismiss() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(gatewayEnabled && !validServerURL)
            }
            Picker("", selection: $section) {
                ForEach(SettingsSection.allCases) { s in
                    Text(s.rawValue).tag(s)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()

            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    switch section {
                    case .ai:
                        aiSettingsBox
                    case .quote:
                        quoteSettingsBoxes
                    case .notify:
                        notifySettingsBox
                    case .look:
                        lookSettingsBoxes
                    }
                }
            }
        }
        .padding(16)
        .frame(width: 460, height: 560)
        .onAppear { load() }
        .sheet(isPresented: $showLog) {
            VStack(alignment: .leading) {
                HStack {
                    Text("运行日志").font(.headline)
                    Spacer()
                    Button("关闭") { showLog = false }
                }
                ScrollView {
                    Text(CrashLog.recent())
                        .font(.system(size: 10, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding()
            .frame(width: 520, height: 400)
        }
    }

    private var aiSettingsBox: some View {
        GroupBox("AI 接入") {
            VStack(alignment: .leading, spacing: 8) {
                Toggle("启用 AI 分析", isOn: Binding(
                    get: { settings.aiConfig.enabled },
                    set: { on in
                        var c = settings.aiConfig
                        c.enabled = on
                        settings.setAIConfig(c)
                    }
                ))
                Toggle("盘中自动分析", isOn: Binding(
                    get: { settings.aiConfig.autoAnalyze },
                    set: { on in
                        var c = settings.aiConfig
                        c.autoAnalyze = on
                        settings.setAIConfig(c)
                    }
                ))
                Picker("Provider", selection: $aiProvider) {
                    Text("OpenAI 兼容").tag("openai")
                    Text("Ollama 本地").tag("ollama")
                }
                .pickerStyle(.segmented)
                .onChange(of: aiProvider) { _, provider in
                    if provider == "ollama", !aiBase.contains(":11434") {
                        aiBase = "http://127.0.0.1:11434"
                        aiModel = "qwen2.5:1.5b"
                    } else if provider == "openai", aiBase.contains(":11434") {
                        aiBase = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
                        aiModel = "qwen3.7-plus"
                    }
                }
                TextField(aiProvider == "ollama" ? "Ollama URL（默认 http://127.0.0.1:11434）" : "Base URL", text: $aiBase)
                    .textFieldStyle(.roundedBorder)
                if aiProvider == "ollama" {
                    TextField("模型（例如 qwen2.5:1.5b）", text: $aiModel)
                        .textFieldStyle(.roundedBorder)
                } else {
                    TokenPlanModelPicker(selection: Binding(
                        get: { aiModel },
                        set: { new in
                            aiModel = new
                            settings.setAIModel(new)
                        }
                    ), chatOnly: false)
                }
                if aiProvider != "ollama" && !TokenPlanCatalog.isChat(aiModel) {
                    Text("图片 / 语音 / 视频模型不能做盯盘分析，请选「推理 / 文本」类。")
                        .font(.caption2)
                        .foregroundStyle(.orange)
                }
                SecureField(aiProvider == "ollama" ? "API Token（通常留空）" : "API Token", text: $aiKey)
                    .textFieldStyle(.roundedBorder)
                HStack {
                    field("间隔秒", $aiInterval)
                    field("max_tokens", $aiMaxTokens)
                }
                Text("AI 请求优先经 Rust 网关并记录用量；网关不可用时自动直连回退。")
                    .font(.caption2).foregroundStyle(.secondary)
                Button("测试连接") {
                    persistAI()
                    store.analyzeWithAI(force: true)
                }
                .disabled(aiProvider != "ollama" && aiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }.padding(6)
        }
    }

    @ViewBuilder
    private var quoteSettingsBoxes: some View {
        GroupBox("行情网关") {
            VStack(alignment: .leading, spacing: 8) {
                Toggle("优先使用 Rust 网关", isOn: $gatewayEnabled)
                TextField("服务器地址", text: $serverURLDraft)
                    .textFieldStyle(.roundedBorder)
                    .disabled(!gatewayEnabled)
                Text("本机默认 http://127.0.0.1:8732；服务不可用时自动回退直连。")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                if gatewayEnabled && !validServerURL {
                    Text("请输入有效的 http:// 或 https:// 地址")
                        .font(.caption2)
                        .foregroundStyle(.orange)
                }
                HStack {
                    Button(gatewayTesting ? "测试中…" : "测试连接") {
                        testGatewayConnection()
                    }
                    .disabled(!gatewayEnabled || !validServerURL || gatewayTesting)
                    if !gatewayTestStatus.isEmpty {
                        Text(gatewayTestStatus)
                            .font(.caption2)
                            .foregroundStyle(gatewayTestStatus.hasPrefix("连接成功") ? .green : .orange)
                    }
                }
            }.padding(6)
        }
        GroupBox("当前标的 · 基准线") {
            VStack(alignment: .leading, spacing: 8) {
                Text(settings.currentSymbol.name + " · " + settings.currentCode)
                    .font(.caption).foregroundStyle(.secondary)
                HStack {
                    field("基准", $baseText)
                    field("支撑", $supportText)
                    field("阻力", $resistText)
                }
            }.padding(6)
        }
        GroupBox("仓位笔记") {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    field("成本价", $costText)
                    field("股数", $sharesText)
                }
                HStack {
                    field("止损价", $stopText)
                    field("仓位%", $posPctText)
                }
                Text("止损可联动「到价下」；仓位%仅作笔记。")
                    .font(.caption2).foregroundStyle(.secondary)
            }.padding(6)
        }
        GroupBox("轻量策略") {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    field("到价上", $aboveText)
                    field("到价下", $belowText)
                    field("回撤%", $ddText)
                }
                HStack {
                    field("回撤冷静分", $coolText)
                    Toggle("止损→到价下", isOn: Binding(
                        get: { settings.strategy.linkStopToBelow },
                        set: { on in
                            var st = settings.strategy
                            settings.updateStrategy(
                                above: st.above, below: st.below, drawdownPct: st.drawdownPct,
                                coolDownMin: st.coolDownMin, linkStopToBelow: on
                            )
                        }
                    ))
                }
                Toggle("开盘/收盘快报", isOn: Binding(
                    get: { settings.openCloseBrief },
                    set: { settings.setOpenCloseBrief($0) }
                ))
            }.padding(6)
        }
        GroupBox("添加标的") {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    TextField("代码 300623 / sz300623", text: $newCode).textFieldStyle(.roundedBorder)
                    TextField("名称(可空)", text: $newName).textFieldStyle(.roundedBorder).frame(width: 80)
                    TextField("分组", text: $newGroupText).textFieldStyle(.roundedBorder).frame(width: 60)
                    Button("添加") {
                        switch settings.addSymbol(code: newCode, name: newName, group: newGroupText) {
                        case .invalid:
                            addSymbolHint = "无效代码"
                        case .exists(let n):
                            addSymbolHint = "已存在：\(n)"
                            store.switchSymbol(settings.currentCode)
                        case .added(let c):
                            addSymbolHint = "已添加 \(c)"
                            newCode = ""; newName = ""
                            store.switchSymbol(c)
                            Task { await store.refreshWatchlistNow() }
                        }
                        load()
                    }
                    .disabled(newCode.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
                Text(addSymbolHint.isEmpty ? "名称空则刷新行情后自动补全" : addSymbolHint)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }.padding(6)
        }
        Button("删除当前标的", role: .destructive) {
            settings.removeCurrentIfPossible()
            store.switchSymbol(settings.currentCode)
            load()
        }
        .disabled(settings.symbols.count <= 1)
    }

    private var notifySettingsBox: some View {
        GroupBox("通知治理") {
            VStack(alignment: .leading, spacing: 8) {
                Toggle("启用通知", isOn: Binding(
                    get: { settings.alertsEnabled },
                    set: { settings.setAlertsEnabled($0) }
                ))
                HStack {
                    field("冷却秒", $cooldownText)
                    field("聚合秒", $aggregateText)
                }
                Toggle("免打扰", isOn: Binding(
                    get: { settings.notifyConfig.dndEnabled },
                    set: { on in
                        var c = settings.notifyConfig
                        c.dndEnabled = on
                        settings.setNotifyConfig(c)
                    }
                ))
                HStack {
                    TextField("开始 HH:mm", text: $dndStart).textFieldStyle(.roundedBorder)
                    TextField("结束 HH:mm", text: $dndEnd).textFieldStyle(.roundedBorder)
                }
                Text(store.notifyQuotaLine.isEmpty ? "跨午夜可用，如 22:00–08:00" : "当前 · \(store.notifyQuotaLine)")
                    .font(.caption2).foregroundStyle(.secondary)

                Divider().padding(.vertical, 2)

                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("价位一级预警").font(.system(size: 11, weight: .medium))
                        Spacer()
                        Text(String(format: "%.2f%%", levelLightDraft))
                            .font(.system(size: 11, weight: .semibold).monospacedDigit())
                            .foregroundStyle(.orange)
                    }
                    Slider(value: Binding(
                        get: { levelLightDraft },
                        set: { levelLightDraft = $0; levelDeepDraft = min(levelDeepDraft, $0 - 0.02) }
                    ), in: 0.10...1.50, step: 0.05)
                    Text("现价相对支撑/阻力/基准的距离 ≤ 此值触发一级闪烁 + 系统通知")
                        .font(.caption2).foregroundStyle(.secondary)
                }

                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("价位二级预警").font(.system(size: 11, weight: .medium))
                        Spacer()
                        Text(String(format: "%.2f%%", levelDeepDraft))
                            .font(.system(size: 11, weight: .semibold).monospacedDigit())
                            .foregroundStyle(.red)
                    }
                    Slider(value: Binding(
                        get: { levelDeepDraft },
                        set: { levelDeepDraft = min($0, levelLightDraft - 0.02) }
                    ), in: 0.05...1.50, step: 0.05)
                    Text("更深一级：1Hz 红色脉动 + 红色描边外圈 + 二级系统通知")
                        .font(.caption2).foregroundStyle(.secondary)

                    Divider().padding(.vertical, 4)

                    HStack {
                        Text("持仓到本预警").font(.system(size: 11, weight: .medium))
                        Spacer()
                        Text(String(format: "%.2f%%", costLightDraft))
                            .font(.system(size: 11, weight: .semibold).monospacedDigit())
                            .foregroundStyle(.purple)
                    }
                    Slider(value: $costLightDraft, in: 0.10...3.00, step: 0.05)
                    Text("现价相对持仓成本距离 ≤ 此值时，MinuteChart 成本线闪烁 + 角标高亮 + 系统通知")
                        .font(.caption2).foregroundStyle(.secondary)
                }
            }.padding(6)
        }
    }

    @ViewBuilder
    private var lookSettingsBoxes: some View {
        GroupBox("外观") {
            VStack(alignment: .leading, spacing: 8) {
                Picker("主题", selection: Binding(get: { settings.theme }, set: { settings.setTheme($0) })) {
                    ForEach(AppTheme.allCases) { t in Text(t.rawValue).tag(t) }
                }
                Picker("字号", selection: Binding(get: { settings.fontSize }, set: { settings.setFontSize($0) })) {
                    ForEach(FontSizePref.allCases) { t in Text(t.rawValue).tag(t) }
                }
                Text("字号影响阅读偏好记录；面板已取消整体缩放以免裁切。")
                    .font(.caption2).foregroundStyle(.secondary)
                Picker("菜单栏", selection: Binding(get: { settings.menuBarFormat }, set: { settings.setMenuBarFormat($0) })) {
                    ForEach(MenuBarFormat.allCases) { t in Text(t.rawValue).tag(t) }
                }
            }.padding(6)
        }
        GroupBox("行为") {
            Toggle("开机自启", isOn: Binding(
                get: { settings.launchAtLogin },
                set: { settings.setLaunchAtLogin($0) }
            ))
            Toggle("纯菜单栏模式", isOn: Binding(
                get: { settings.menuBarOnly },
                set: { on in
                    settings.setMenuBarOnly(on)
                    NSApp.setActivationPolicy(on ? .accessory : .regular)
                }
            ))
        }
        GroupBox("稳定性 / 版本") {
            VStack(alignment: .leading, spacing: 6) {
                Text("版本 \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "—")")
                    .font(.caption)
                Button("查看更新说明") {
                    NSApp.activate(ignoringOtherApps: true)
                    let alert = NSAlert()
                    alert.messageText = "摸金小王子 · 更新说明"
                    alert.informativeText = AppChangelog.text
                    alert.runModal()
                    settings.markChangelogSeen()
                }
                Button("查看崩溃/运行日志") { showLog = true }
                Button("清空日志", role: .destructive) { CrashLog.clear() }
            }.padding(6)
        }
    }

    private func field(_ title: String, _ text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            TextField(title, text: text)
                .textFieldStyle(.roundedBorder)
                .monospacedDigit()
        }
    }

    private func persist() {
        settings.setMarketGateway(enabled: gatewayEnabled, address: serverURLDraft)
        settings.updateLevels(base: Double(baseText) ?? 0, support: Double(supportText) ?? 0, resistance: Double(resistText) ?? 0)
        settings.updatePosition(
            cost: Double(costText) ?? 0,
            shares: Double(sharesText) ?? 0,
            stopLoss: Double(stopText) ?? 0,
            positionPct: Double(posPctText) ?? 0
        )
        settings.updateStrategy(
            above: Double(aboveText) ?? 0,
            below: Double(belowText) ?? 0,
            drawdownPct: Double(ddText) ?? 0,
            coolDownMin: Int(coolText) ?? 30,
            linkStopToBelow: settings.strategy.linkStopToBelow
        )
        var c = settings.notifyConfig
        c.cooldownSec = Int(cooldownText) ?? 300
        c.aggregateSec = Int(aggregateText) ?? 90
        c.dndStartMin = parseHM(dndStart) ?? (22 * 60)
        c.dndEndMin = parseHM(dndEnd) ?? (8 * 60)
        c.levelLightPct = max(0.05, levelLightDraft)
        c.levelDeepPct  = max(0.02, min(levelDeepDraft, c.levelLightPct - 0.02))
        c.costLightPct  = max(0.10, costLightDraft)
        settings.setNotifyConfig(c)
        persistAI()
    }

    private func persistAI() {
        var a = settings.aiConfig
        a.baseURL = aiBase.trimmingCharacters(in: .whitespacesAndNewlines)
        a.model = aiModel.trimmingCharacters(in: .whitespacesAndNewlines)
        a.providerId = aiProvider
        a.intervalSec = max(30, Int(aiInterval) ?? 120)
        a.maxTokens = max(128, Int(aiMaxTokens) ?? 600)
        settings.setAIConfig(a)
        settings.setAIAPIKey(aiKey)
    }

    private func parseHM(_ s: String) -> Int? {
        let p = s.split(separator: ":")
        guard p.count == 2, let h = Int(p[0]), let m = Int(p[1]), (0...23).contains(h), (0...59).contains(m) else { return nil }
        return h * 60 + m
    }

    private func load() {
        gatewayEnabled = settings.marketGatewayEnabled
        serverURLDraft = settings.marketServerURL
        let lv = settings.levels
        baseText = lv.base > 0 ? String(format: "%.2f", lv.base) : ""
        supportText = lv.support > 0 ? String(format: "%.2f", lv.support) : ""
        resistText = lv.resistance > 0 ? String(format: "%.2f", lv.resistance) : ""
        let p = settings.position
        costText = p.cost > 0 ? String(format: "%.3f", p.cost) : ""
        sharesText = p.shares > 0 ? String(format: "%.0f", p.shares) : ""
        stopText = p.stopLoss > 0 ? String(format: "%.2f", p.stopLoss) : ""
        posPctText = p.positionPct > 0 ? String(format: "%.0f", p.positionPct) : ""
        let st = settings.strategy
        aboveText = st.above > 0 ? String(format: "%.2f", st.above) : ""
        belowText = st.below > 0 ? String(format: "%.2f", st.below) : ""
        ddText = st.drawdownPct > 0 ? String(format: "%.1f", st.drawdownPct) : ""
        coolText = String(st.coolDownMin)
        cooldownText = String(settings.notifyConfig.cooldownSec)
        aggregateText = String(settings.notifyConfig.aggregateSec)
        dndStart = String(format: "%02d:%02d", settings.notifyConfig.dndStartMin / 60, settings.notifyConfig.dndStartMin % 60)
        dndEnd = String(format: "%02d:%02d", settings.notifyConfig.dndEndMin / 60, settings.notifyConfig.dndEndMin % 60)
        levelLightDraft = settings.notifyConfig.levelLightPct
        levelDeepDraft = settings.notifyConfig.levelDeepPct
        costLightDraft = settings.notifyConfig.costLightPct
        aiBase = settings.aiConfig.baseURL
        aiModel = settings.aiConfig.model
        aiProvider = settings.aiConfig.providerId
        aiKey = settings.aiAPIKey
        aiInterval = String(settings.aiConfig.intervalSec)
        aiMaxTokens = String(settings.aiConfig.maxTokens)
    }

    private var validServerURL: Bool {
        guard let url = URLComponents(string: serverURLDraft.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            return false
        }
        return ["http", "https"].contains(url.scheme?.lowercased() ?? "") && url.host != nil
    }

    private func testGatewayConnection() {
        gatewayTesting = true
        gatewayTestStatus = ""
        let address = serverURLDraft
        Task {
            do {
                gatewayTestStatus = try await GatewayMarketClient.health(baseURL: address)
            } catch {
                gatewayTestStatus = "连接失败 · \(error.localizedDescription)"
            }
            gatewayTesting = false
        }
    }
}

struct TokenPlanModelPicker: View {
    @Binding var selection: String
    var chatOnly: Bool = false

    private var source: [TokenPlanModel] {
        chatOnly ? TokenPlanCatalog.chat : TokenPlanCatalog.all
    }

    private var groups: [(brand: String, models: [TokenPlanModel])] {
        var order: [String] = []
        var map: [String: [TokenPlanModel]] = [:]
        for m in source {
            if map[m.brand] == nil { order.append(m.brand) }
            map[m.brand, default: []].append(m)
        }
        return order.map { (brand: $0, models: map[$0] ?? []) }
    }

    private var currentLabel: String {
        if let m = TokenPlanCatalog.all.first(where: { $0.modelID == selection }) {
            return m.modelID
        }
        return selection.isEmpty ? "选择模型" : selection
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(chatOnly ? "分析模型（Token Plan）" : "模型（Token Plan 全量）")
                .font(.caption)
                .foregroundStyle(.secondary)
            Menu {
                if !TokenPlanCatalog.contains(selection), !selection.isEmpty {
                    Button(selection) { selection = selection }
                }
                ForEach(groups, id: \.brand) { g in
                    Menu(g.brand) {
                        ForEach(g.models) { m in
                            Button {
                                selection = m.modelID
                            } label: {
                                HStack {
                                    Text(m.menuLabel)
                                    if m.modelID == selection {
                                        Image(systemName: "checkmark")
                                    }
                                }
                            }
                        }
                    }
                }
            } label: {
                HStack {
                    Image(systemName: "cpu")
                    Text(currentLabel)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
                .font(.system(size: 12, weight: .medium))
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8))
            }
            .menuStyle(.borderlessButton)
            Text("共 \(source.count) 个可选 · 当前 \(selection.isEmpty ? "未选" : selection)")
                .font(.caption2)
                .foregroundStyle(.tertiary)
        }
    }
}

struct ChangelogView: View {
    @ObservedObject var settings: AppSettings
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("更新说明").font(.headline)
            ScrollView {
                Text(AppChangelog.text)
                    .font(.system(size: 12))
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Button("知道了") {
                settings.markChangelogSeen()
                dismiss()
            }
            .keyboardShortcut(.defaultAction)
        }
        .padding(16)
        .frame(width: 420, height: 360)
    }
}
