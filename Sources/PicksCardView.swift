import SwiftUI

/// §I.1 + §I.3：精选卡三层折叠视图。
///
/// L1 决策层（默认）：能不能买？买哪几只？买/卖价在哪？
/// L2 证据层（点「详情」）：regime 胜率、真实执行、期望短缺、标签胜率、调权。
/// L3 审计层（点「元数据」）：池子来源、行业集中度、涨停分档、experiment_id。
///
/// 复用 ContentView 的数据 / store / settings，仅重组渲染层级；
/// 不动 API，不改 regime 判定逻辑，所有结论来自 `doc`。
struct PicksCardView: View {
    @ObservedObject var store: MarketStore
    @ObservedObject var settings: AppSettings
    /// 切到该标的并跳盯盘（ContentView 提供回调）。
    var onOpenSymbol: (String, String) -> Void

    @ObservedObject private var ui = PicksCardUIState()

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: UITokens.stackNormal) {
                header
                if let doc = store.picksDoc {
                    marketSentimentBanner(doc)
                    intradayConfirmationBanner(doc)
                    if doc.picks.isEmpty {
                        emptyState(doc)
                    } else {
                        evidenceSummary(doc)
                        if ui.showDetail { evidenceDetail(doc) }
                        if ui.showAudit { auditDetail(doc) }
                        picksList(doc)
                        tailBuySection(doc)
                        previousPicksSection(doc)
                    }
                } else if !store.picksHint.isEmpty {
                    Text(store.picksHint)
                        .font(.system(size: UITokens.metaSize))
                        .foregroundStyle(.tertiary)
                } else {
                    Text("点「AI 精排」生成，或等收盘后自动产出")
                        .font(.system(size: UITokens.metaSize))
                        .foregroundStyle(.tertiary)
                }
                if !store.picksHint.isEmpty, store.picksDoc?.picks.isEmpty == false {
                    Text(store.picksHint)
                        .font(.system(size: UITokens.microSize))
                        .foregroundStyle(UITokens.microText)
                }
            }
            .padding(4)
        }
    }

    // MARK: - 头部

    private var header: some View {
        HStack(spacing: UITokens.stackTight) {
            Text("尾盘智能推荐 · 目标 T+1")
                .font(.system(size: UITokens.titleSize, weight: UITokens.titleWeight))
            if let doc = store.picksDoc, !doc.picks.isEmpty {
                Text(doc.date)
                    .font(.system(size: UITokens.metaSize))
                    .foregroundStyle(.tertiary)
            }
            Spacer()
            if store.picksBusy { ProgressView().controlSize(.mini) }
            if let doc = store.picksDoc, !doc.picks.isEmpty {
                ToggleChip(label: "详情", isOn: ui.showDetail) {
                    ui.showDetail.toggle()
                }
                ToggleChip(label: "元数据", isOn: ui.showAudit) {
                    ui.showAudit.toggle()
                }
            }
            Button("AI 精排") { Task { await store.runPicks() } }
                .controlSize(.mini).disabled(store.picksBusy)
                .help("重新生成当日推荐：涨幅榜 → 量化 → 研报/新闻 → AI 精排；通常需要 30–120 秒（AI 未配置则纯量化）")
            Button("刷新") { Task { await store.loadPicks() } }
                .controlSize(.mini).disabled(store.picksBusy)
        }
    }

    private func toggleButton(_ label: String, isOn: Binding<Bool>) -> some View {
        Button(label) { isOn.wrappedValue.toggle() }
            .controlSize(.mini)
            .buttonStyle(.borderless)
            .foregroundStyle(isOn.wrappedValue ? Color.accentColor : .secondary)
    }

    /// 等价于 `toggleButton`，但接受只读 isOn + 显式 action（避免依赖宏展开）。
    private struct ToggleChip: View {
        let label: String
        let isOn: Bool
        let action: () -> Void
        var body: some View {
            Button(action: action) {
                Text(label)
                    .font(.system(size: UITokens.metaSize, weight: .semibold))
                    .foregroundStyle(isOn ? Color.accentColor : .secondary)
                    .padding(.horizontal, UITokens.pillHPad)
                    .padding(.vertical, 1)
                    .background(
                        (isOn ? Color.accentColor : Color.secondary).opacity(isOn ? 0.14 : 0.08),
                        in: Capsule()
                    )
            }
            .buttonStyle(.borderless)
            .controlSize(.mini)
        }
    }

    // MARK: - 空清单专门态（暂停 / 无候选）

    @ViewBuilder
    private func emptyState(_ doc: GatewayPicksDocument) -> some View {
        if let hint = doc.executeHint, !hint.isEmpty {
            // §I.1.2：大红灯 + 暂停原因；不再混用「暂无数据」灰字。
            HStack(alignment: .top, spacing: UITokens.stackTight) {
                Image(systemName: "exclamationmark.octagon.fill")
                    .font(.system(size: UITokens.bodySize + 2))
                    .foregroundStyle(UITokens.color(.danger))
                VStack(alignment: .leading, spacing: 2) {
                    Text("今日暂停推荐")
                        .font(.system(size: UITokens.titleSize, weight: .bold))
                        .foregroundStyle(UITokens.color(.danger))
                    Text(hint)
                        .font(.system(size: UITokens.metaSize))
                        .foregroundStyle(.primary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(UITokens.stackNormal)
            .background(UITokens.background(.danger), in: RoundedRectangle(cornerRadius: 6))
            // §I.8 VoiceOver：把「暂停 + 原因」合并为一句可朗读的语义
            .accessibilityElement(children: .combine)
            .accessibilityLabel(Text("今日暂停推荐，\(hint)"))
            .accessibilityAddTraits(.isHeader)
        } else {
            Text("暂无推荐。工作日 14:45 自动生成尾盘候选，或点「AI 精排」立即刷新。")
                .font(.system(size: UITokens.metaSize))
                .foregroundStyle(UITokens.microText)
                .accessibilityLabel(Text("暂无推荐"))
        }
    }

    // MARK: - L1：胜率 / 执行一行摘要（默认可见的最小证据）

    @ViewBuilder
    private func evidenceSummary(_ doc: GatewayPicksDocument) -> some View {
        if doc.samples > 0 {
            HStack(spacing: UITokens.stackRelaxed) {
                regimeBadge(doc)
                if let exec = doc.execution {
                    executionBadge(doc, exec: exec)
                }
                if let hint = doc.executeHint, !hint.isEmpty {
                    hintPill(hint)
                }
                Spacer(minLength: 0)
                if let ixic = doc.market?.ixic {
                    Text(String(format: "隔夜纳指 %+.1f%%", ixic))
                        .font(.system(size: UITokens.metaSize))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.trend(ixic))
                        .help("隔夜美股情绪（道指/纳指），影响当日推荐的全局分")
                }
            }
        } else {
            HStack(spacing: UITokens.stackTight) {
                Text("T+1 样本积累中（下一交易日收盘后回写）")
                    .font(.system(size: UITokens.metaSize))
                    .foregroundStyle(.tertiary)
                if let hint = doc.executeHint, !hint.isEmpty {
                    hintPill(hint)
                }
                Spacer(minLength: 0)
            }
        }
    }

    /// §I.3.1 胜率默认格式：`偏空日 · 52% [41–63] · n=28`；样本不足时主色降为橙并前置 ⚠️。
    private func regimeBadge(_ doc: GatewayPicksDocument) -> some View {
        let regime = doc.currentRegimeStat
        let visibleWinRate = regime?.winRate ?? doc.t1WinRate
        let visibleSamples = regime?.samples ?? doc.samples
        let visibleAvg = regime?.avgT1Pct ?? doc.avgT1Pct
        let visibleSufficient = regime?.samplesSufficient ?? doc.t1SamplesSufficient
        let lowPct = (regime?.winRateLow ?? doc.t1WinRateLow) * 100
        let highPct = (regime?.winRateHigh ?? doc.t1WinRateHigh) * 100
        let intervalLabel = String(format: "[%.0f–%.0f%%]", lowPct, highPct)
        let warningIcon = visibleSufficient ? "" : "⚠️ "
        let label = regime?.label ?? "全市场"
        let warnSignal: UITokens.Signal = visibleSufficient
            ? (visibleWinRate >= 0.5 ? .buy : .observe)
            : .observe
        return HStack(spacing: 4) {
            Text("\(warningIcon)\(label)日")
                .font(.system(size: UITokens.metaSize, weight: .semibold))
                .foregroundStyle(UITokens.color(warnSignal))
            Text("T+1 胜率")
                .font(.system(size: UITokens.microSize))
                .foregroundStyle(.secondary)
            Text(String(format: "%.0f%%", visibleWinRate * 100))
                .font(.system(size: UITokens.metaSize, weight: .bold))
                .monospacedDigit()
                .foregroundStyle(UITokens.color(warnSignal))
            Text("\(intervalLabel) · n=\(visibleSamples) · 平均 \(String(format: "%+.1f", visibleAvg))%")
                .font(.system(size: UITokens.microSize, design: .monospaced))
                .foregroundStyle(.secondary)
        }
        .help("默认按当前大盘环境分桶；尾盘推荐基准价 vs 下一交易日收盘，近 30 天口径。区间为 Wilson 95% 置信区间。")
    }

    private func executionBadge(_ doc: GatewayPicksDocument, exec: ExecutionStats) -> some View {
        let execInterval = String(format: "[%.0f–%.0f%%]",
                                  exec.t1RealWinRateLow * 100,
                                  exec.t1RealWinRateHigh * 100)
        let signal: UITokens.Signal = exec.t1RealWinRate >= 0.5 ? .buy : .observe
        return Text(String(format: "次日开盘 胜率 %.0f%%%@ · 溢价 %+.1f%%",
                           exec.t1RealWinRate * 100, execInterval, exec.avgEntryGap))
            .font(.system(size: UITokens.metaSize))
            .monospacedDigit()
            .foregroundStyle(UITokens.color(signal))
            .help("Wilson 95% 区间；样本 < 30 时只看方向，不看数字")
    }

    private func hintPill(_ hint: String) -> some View {
        Text(hint)
            .font(.system(size: UITokens.metaSize, weight: .bold))
            .foregroundStyle(UITokens.color(.observe))
            .padding(.horizontal, UITokens.pillHPad + 1)
            .padding(.vertical, UITokens.pillVPad + 1)
            .background(UITokens.background(.observe), in: Capsule())
            .help("推荐基于当日行情；涨停已过滤，确保有买入窗口")
    }

    // MARK: - L2：证据细节（regime 警告 / 真实执行 / 曲线 / 标签 / 调权）

    @ViewBuilder
    private func evidenceDetail(_ doc: GatewayPicksDocument) -> some View {
        VStack(alignment: .leading, spacing: UITokens.stackTight) {
            if doc.samples > 0, let exec = doc.execution {
                executionDetail(exec)
            }
            if doc.samples > 0, let regime = doc.currentRegimeStat, !regime.samplesSufficient {
                Text("⚠️ \(regime.label)样本不足 \(regime.samples)，胜率仅供方向参考")
                    .font(.system(size: UITokens.microSize, weight: .semibold))
                    .foregroundStyle(UITokens.color(.observe))
            }
            if doc.t5Samples > 0 {
                let t5Low = doc.t5WinRateLow * 100
                let t5High = doc.t5WinRateHigh * 100
                Text(String(format: "T+5 胜率 %.0f%% [%.0f–%.0f%%] · n=%d",
                            doc.t5WinRate * 100, t5Low, t5High, doc.t5Samples))
                    .font(.system(size: UITokens.microSize))
                    .foregroundStyle(UITokens.microText)
                    .help("T+5 中线参考 · Wilson 95% 区间")
            }
            if !doc.tags.isEmpty {
                HStack(spacing: UITokens.stackTight) {
                    ForEach(doc.tags.prefix(5)) { t in
                        tagPill(t)
                    }
                    Spacer(minLength: 0)
                }
            }
            if doc.picks.contains(where: { abs($0.meta.autoWeight?.adjustment ?? 0) >= 0.05 }) {
                Label("已启用复盘学习调权（近30天 · 单标签≥30样本）",
                      systemImage: "brain.head.profile")
                    .font(.system(size: UITokens.microSize, weight: .semibold))
                    .foregroundStyle(UITokens.color(.audit))
            }
        }
    }

    private func tagPill(_ t: GatewayPicksDocument.TagStat) -> some View {
        let signal: UITokens.Signal = t.winRate >= 0.5 ? .buy : .observe
        return Text(String(format: "%@ %.0f%%", t.tag, t.winRate * 100))
            .font(.system(size: UITokens.microSize))
            .monospacedDigit()
            .foregroundStyle(UITokens.color(signal))
            .padding(.horizontal, UITokens.pillHPad - 1)
            .padding(.vertical, UITokens.pillVPad)
            .background(UITokens.background(signal), in: Capsule())
            .help("该标签近 30 天 T+1 胜率 · \(t.samples) 样本")
    }

    @ViewBuilder
    private func executionDetail(_ exec: ExecutionStats) -> some View {
        VStack(alignment: .leading, spacing: UITokens.stackTight) {
            HStack(spacing: UITokens.stackNormal) {
                if exec.avgMaxDd < 0 {
                    Text(String(format: "回撤 均%.1f%% / 最差%.1f%%", exec.avgMaxDd, exec.maxDrawdown))
                        .font(.system(size: UITokens.metaSize))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.color(.observe))
                }
                if exec.expectedShortfall10 < 0 {
                    Text(String(format: "期望短缺 %.1f%%", exec.expectedShortfall10))
                        .font(.system(size: UITokens.metaSize, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.color(.danger))
                }
                if exec.target5pctSamples > 0 {
                    let signal: UITokens.Signal = exec.target5pctWilsonLow >= 0.1 ? .buy : .observe
                    Text(String(format: "达成5%% %.0f%% [%.0f–%.0f%%] · %d样本",
                                exec.target5pctHitRate * 100,
                                exec.target5pctWilsonLow * 100,
                                exec.target5pctWilsonHigh * 100,
                                exec.target5pctSamples))
                        .font(.system(size: UITokens.metaSize, weight: .semibold))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.color(signal))
                }
                if exec.winLossRatio > 0 {
                    let signal: UITokens.Signal = exec.winLossRatio >= 2 ? .buy : .neutral
                    Text(String(format: "盈亏比 %.1f", exec.winLossRatio))
                        .font(.system(size: UITokens.metaSize))
                        .monospacedDigit()
                        .foregroundStyle(UITokens.color(signal))
                }
                Spacer(minLength: 0)
            }
            .help("次日开盘实际买入价 vs 推荐日收盘价的统计——更贴近真实收益")
            if !exec.curve.isEmpty {
                PickExecutionCurveTokenView(points: exec.curve)
                    .frame(height: 76)
            }
            if exec.executionWarning {
                Label(exec.executionHint, systemImage: "exclamationmark.triangle.fill")
                    .font(.system(size: UITokens.metaSize, weight: .semibold))
                    .foregroundStyle(UITokens.color(.observe))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    // MARK: - L3：审计（池子来源 / 集中度 / 涨停分档）

    @ViewBuilder
    private func auditDetail(_ doc: GatewayPicksDocument) -> some View {
        VStack(alignment: .leading, spacing: UITokens.stackTight) {
            let limitUpCount = doc.picks.filter { ($0.meta.isLimitUp ?? false) }.count
            let negatives = doc.picks.compactMap { $0.meta.negativeSignals }.flatMap { $0 }
            let betas = doc.picks.compactMap { $0.meta.risk?.beta60d }
            if limitUpCount > 0 || doc.picks.isEmpty == false {
                Text(String(format: "Top %d · 涨停 %d · 行业平均 %.1f 只/行业",
                            doc.picks.count, limitUpCount, averageIndustryPerPick(doc)))
                    .font(.system(size: UITokens.microSize, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            if !negatives.isEmpty {
                Text("负向信号：\(negatives.prefix(3).map { "\($0.title)" }.joined(separator: "；"))")
                    .font(.system(size: UITokens.microSize))
                    .foregroundStyle(UITokens.color(.observe))
            }
            if let avgBeta = averageBeta(betas) {
                Text(String(format: "β60d 均值 %.2f（≥1.3 高 β / ≤0.8 低 β）", avgBeta))
                    .font(.system(size: UITokens.microSize))
                    .foregroundStyle(avgBeta >= 1.3 ? UITokens.color(.observe) : .secondary)
            }
            Text("L3 元数据受服务端字段约束：池子来源 / 集中度 / experiment_id 等 §A.14 落地后即透出")
                .font(.system(size: UITokens.microSize))
                .foregroundStyle(.tertiary)
        }
        .padding(UITokens.stackTight)
        .background(Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 4))
    }

    private func averageIndustryPerPick(_ doc: GatewayPicksDocument) -> Double {
        let industries = Set(doc.picks.compactMap { $0.meta.industry }.filter { !$0.isEmpty })
        guard !industries.isEmpty else { return 0 }
        return Double(doc.picks.count) / Double(industries.count)
    }

    private func averageBeta(_ betas: [Double]) -> Double? {
        guard !betas.isEmpty else { return nil }
        return betas.reduce(0, +) / Double(betas.count)
    }

    // MARK: - Top5 列表

    private func picksList(_ doc: GatewayPicksDocument) -> some View {
        VStack(alignment: .leading, spacing: UITokens.stackTight) {
            ForEach(doc.picks) { pick in
                pickRow(pick)
            }
        }
    }

    private func pickRow(_ pick: GatewayPick) -> some View {
        let entry = pickEntryStatus(pick)
        return Button {
            if !settings.symbols.contains(where: { $0.code == pick.code }) {
                _ = settings.addSymbol(code: pick.code, name: pick.name, group: "观察")
            }
            onOpenSymbol(pick.code, pick.name)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                // L1 默认：名称 / 排名 / 涨跌幅 / 入场时机 / 策略 / 买区 / 一句话风险（ES·DD 收进 tooltip）
                HStack(alignment: .top, spacing: UITokens.stackTight) {
                    Text("#\(pick.rank)")
                        .font(.system(size: UITokens.bodySize, weight: .bold, design: .rounded))
                        .monospacedDigit()
                        .foregroundStyle(Color.accentColor)
                        .frame(width: 20, alignment: .leading)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: UITokens.stackTight) {
                            Text(pick.name)
                                .font(.system(size: UITokens.bodySize, weight: .semibold))
                                .lineLimit(1)
                            if let industry = pick.meta.industry, !industry.isEmpty {
                                Text(industry)
                                    .font(.system(size: UITokens.microSize))
                                    .foregroundStyle(.tertiary)
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 0)
                            if let pct = pick.meta.pct {
                                Text(String(format: "%@%.1f%%", pct >= 0 ? "+" : "", pct))
                                    .font(.system(size: UITokens.bodySize, weight: .semibold))
                                    .monospacedDigit()
                                    .foregroundStyle(UITokens.trend(pct))
                            }
                        }
                        HStack(spacing: UITokens.stackTight) {
                            pill(text: entry.text, signal: entry.signal)
                            let strategy = pick.meta.plan?.strategy ?? "短线"
                            pill(text: strategy, signal: strategySignal(strategy))
                            Text(pickTargetLabel(pick))
                                .font(.system(size: UITokens.microSize, weight: .semibold))
                                .foregroundStyle(.secondary)
                            Spacer(minLength: 0)
                            if let window = pick.meta.plan?.entryWindow, entry.text.contains("尾盘") {
                                Text(window)
                                    .font(.system(size: UITokens.microSize, design: .monospaced))
                                    .foregroundStyle(.tertiary)
                            }
                        }
                        // 买/卖区间（§I.3.2 等宽数字 + 浅底）
                        if let plan = pick.meta.plan,
                           let bLow = plan.buyPriceLow,
                           let bHigh = plan.buyPriceHigh,
                           let sLow = plan.sellPriceLow,
                           let sHigh = plan.sellPriceHigh {
                            priceRangeRow(basis: pick.meta.sellZoneBasis,
                                          bLow: bLow, bHigh: bHigh,
                                          sLow: sLow, sHigh: sHigh)
                        }
                        // L2（默认隐藏）：标签 / 风险 / 学习分 / T+1 — 仅 showDetail 时显示
                        if ui.showDetail {
                            HStack(spacing: UITokens.stackTight) {
                                ForEach(pick.reasons.prefix(3), id: \.self) { tag in
                                    Text(tag)
                                        .font(.system(size: UITokens.microSize))
                                        .foregroundStyle(Color.accentColor.opacity(0.9))
                                        .padding(.horizontal, UITokens.pillHPad - 1)
                                        .padding(.vertical, UITokens.pillVPad)
                                        .background(Color.accentColor.opacity(0.1), in: Capsule())
                                }
                                if let risk = pick.meta.risk,
                                   let shortfall = risk.expectedShortfall10pct20d {
                                    Text(String(format: "ES %.1f%% · DD %.1f%%",
                                                shortfall, risk.maxDrawdown20d ?? 0))
                                        .font(.system(size: UITokens.microSize, design: .monospaced))
                                        .foregroundStyle(shortfall < -3 ? UITokens.color(.danger) : .secondary)
                                }
                                if let t1 = pick.meta.outcome?.t1Pct {
                                    Text(String(format: "T+1 %+.1f%%", t1))
                                        .font(.system(size: UITokens.microSize))
                                        .monospacedDigit()
                                        .foregroundStyle(UITokens.trend(t1))
                                }
                                if let adjustment = pick.meta.autoWeight?.adjustment,
                                   abs(adjustment) >= 0.5 {
                                    Text(String(format: "学习 %@%.1f", adjustment >= 0 ? "+" : "", adjustment))
                                        .font(.system(size: UITokens.microSize, weight: .semibold))
                                        .monospacedDigit()
                                        .foregroundStyle(adjustment >= 0 ? UITokens.color(.audit) : UITokens.color(.observe))
                                        .padding(.horizontal, UITokens.pillHPad - 1)
                                        .padding(.vertical, UITokens.pillVPad)
                                        .background(
                                            (adjustment >= 0 ? UITokens.color(.audit) : UITokens.color(.observe)).opacity(0.1),
                                            in: Capsule()
                                        )
                                        .help(pickHelp_AutoWeight(pick.meta.autoWeight))
                                }
                                Spacer(minLength: 0)
                                Text(String(format: "%.0f分", pick.score))
                                    .font(.system(size: UITokens.microSize))
                                    .monospacedDigit()
                                    .foregroundStyle(.tertiary)
                            }
                        }
                    }
                }
            }
            .padding(.vertical, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(pickHelp(pick))
    }

    private func priceRangeRow(basis: GatewayPick.SellZoneBasis?,
                               bLow: Double, bHigh: Double,
                               sLow: Double, sHigh: Double) -> some View {
        HStack(spacing: UITokens.stackNormal) {
            HStack(spacing: 3) {
                Text("买")
                    .font(.system(size: UITokens.microSize, weight: .bold))
                    .foregroundStyle(UITokens.color(.buy))
                Text(String(format: "%.2f–%.2f", bLow, bHigh))
                    .font(.system(size: UITokens.metaSize, design: .monospaced))
                    .monospacedDigit()
                    .foregroundStyle(.primary)
                    .padding(.horizontal, 4).padding(.vertical, 1)
                    .background(UITokens.color(.buy).opacity(0.08), in: RoundedRectangle(cornerRadius: 3))
            }
            HStack(spacing: 3) {
                Text("卖")
                    .font(.system(size: UITokens.microSize, weight: .bold))
                    .foregroundStyle(UITokens.color(.observe))
                Text(String(format: "%.2f–%.2f（≥5%%）", sLow, sHigh))
                    .font(.system(size: UITokens.metaSize, design: .monospaced))
                    .monospacedDigit()
                    .foregroundStyle(.primary)
                    .padding(.horizontal, 4).padding(.vertical, 1)
                    .background(UITokens.color(.observe).opacity(0.08), in: RoundedRectangle(cornerRadius: 3))
            }
            Spacer(minLength: 0)
            if let basis, basis.samples > 0, let avgT1 = basis.avgT1Pct {
                Text(String(format: "%d 样本 · %+.1f%% T+1", basis.samples, avgT1))
                    .font(.system(size: UITokens.microSize))
                    .foregroundStyle(.tertiary)
                    .help("基于该票近 30 天已回写 outcome.t1_pct；样本 < 30 时仅作方向参考")
            } else {
                Text("默认 1.2% T+1 期望")
                    .font(.system(size: UITokens.microSize))
                    .foregroundStyle(.tertiary)
                    .help("无历史 outcome，按市场平均 T+1 收益 1.2% 推算")
            }
        }
    }

    // MARK: - 情绪灯 / 盘中二次确认

    @ViewBuilder
    private func marketSentimentBanner(_ doc: GatewayPicksDocument) -> some View {
        if let cn = doc.market?.cn, let avg = cn.avgPct {
            let paused = doc.picks.isEmpty && avg <= -2.0
            let weak = avg <= -0.8
            let bullish = avg >= 1.0
            let signal: UITokens.Signal = paused ? .danger : (weak ? .observe : (bullish ? .buy : .neutral))
            let icon = paused ? "exclamationmark.octagon.fill" :
                (weak ? "arrow.down.right.circle.fill" :
                    (bullish ? "arrow.up.right.circle.fill" : "minus.circle.fill"))
            let state = paused ? "今日暂停推荐" : (weak ? "偏弱" : (bullish ? "偏多" : "震荡"))
            HStack(spacing: UITokens.stackNormal - 2) {
                Image(systemName: icon)
                Text(String(format: "大盘 %+.2f%% · %@", avg, state))
                    .font(.system(size: UITokens.bodySize, weight: .bold))
                    .monospacedDigit()
                Spacer(minLength: 0)
                if let sh = cn.shPct, let sz = cn.szPct, let gem = cn.gemPct {
                    Text(String(format: "沪 %+.1f · 深 %+.1f · 创 %+.1f", sh, sz, gem))
                        .font(.system(size: UITokens.microSize, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
            }
            .foregroundStyle(UITokens.color(signal))
            .padding(.horizontal, UITokens.stackNormal + 2)
            .padding(.vertical, UITokens.pillVPad + 2)
            .background(UITokens.background(signal), in: RoundedRectangle(cornerRadius: 6))
            .help("A 股主流指数当日平均涨跌幅；≤ -2% 暂停推荐，≤ -0.8% 全候选降权，≥ +1% 加权")
            // §I.8 VoiceOver
            .accessibilityElement(children: .combine)
            .accessibilityLabel(Text(String(format: "大盘 %+.2f%% · %@", avg, state)))
        }
    }

    @ViewBuilder
    private func intradayConfirmationBanner(_ doc: GatewayPicksDocument) -> some View {
        if let confirmation = doc.market?.confirmation {
            let observeOnly = confirmation.status == "observe_only"
            let signal: UITokens.Signal = observeOnly ? .observe : .buy
            Label(
                observeOnly
                    ? "盘中二次确认未通过 · \(confirmation.reason) · 仅观察"
                    : "盘中二次确认通过 · \(confirmation.reason)",
                systemImage: observeOnly ? "eye.trianglebadge.exclamationmark" : "checkmark.shield.fill"
            )
            .font(.system(size: UITokens.metaSize, weight: .semibold))
            .foregroundStyle(UITokens.color(signal))
            .padding(.horizontal, UITokens.stackNormal + 2)
            .padding(.vertical, UITokens.pillVPad + 2)
            .background(UITokens.background(signal), in: RoundedRectangle(cornerRadius: 6))
            // §I.8 VoiceOver
            .accessibilityElement(children: .combine)
            .accessibilityLabel(Text(observeOnly
                ? "盘中二次确认未通过，\(confirmation.reason)，仅观察"
                : "盘中二次确认通过，\(confirmation.reason)"))
        }
    }

    // MARK: - 尾盘买进 / 昨日卖出

    @ViewBuilder
    private func tailBuySection(_ doc: GatewayPicksDocument) -> some View {
        let today = doc.tailBuyPicks
        let nextOpen = doc.nextOpenPicks
        if today.isEmpty && nextOpen.isEmpty {
            EmptyView()
        } else {
            VStack(alignment: .leading, spacing: UITokens.stackTight) {
                if !today.isEmpty {
                    HStack(spacing: UITokens.stackTight) {
                        Image(systemName: "clock.badge.checkmark")
                            .font(.system(size: UITokens.metaSize))
                            .foregroundStyle(UITokens.color(.buy))
                        Text("尾盘买进清单 · 14:45–14:57 窗口")
                            .font(.system(size: UITokens.metaSize, weight: .semibold))
                            .foregroundStyle(UITokens.color(.buy))
                        Spacer(minLength: 0)
                    }
                    ForEach(today) { pick in pickRow(pick) }
                }
                if !nextOpen.isEmpty {
                    HStack(spacing: UITokens.stackTight) {
                        Image(systemName: "sun.max")
                            .font(.system(size: UITokens.metaSize))
                            .foregroundStyle(UITokens.color(.observe))
                        Text("次盘新股 · 09:30-09:35 集合竞价优先")
                            .font(.system(size: UITokens.metaSize, weight: .semibold))
                            .foregroundStyle(UITokens.color(.observe))
                        Spacer(minLength: 0)
                    }
                    .padding(.top, 2)
                    ForEach(nextOpen) { pick in pickRow(pick) }
                }
            }
        }
    }

    @ViewBuilder
    private func previousPicksSection(_ doc: GatewayPicksDocument) -> some View {
        if let prev = doc.previousPicks, !prev.isEmpty {
            VStack(alignment: .leading, spacing: UITokens.stackTight) {
                Divider().padding(.vertical, 2)
                HStack(spacing: UITokens.stackNormal - 2) {
                    Image(systemName: "calendar.badge.clock")
                        .font(.system(size: UITokens.bodySize))
                        .foregroundStyle(.indigo)
                    Text("上一交易日推荐 → 今日卖出参考")
                        .font(.system(size: UITokens.bodySize, weight: .bold))
                    Text("\(prev.first?.date ?? "") · \(prev.count) 只")
                        .font(.system(size: UITokens.metaSize))
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 0)
                }
                ForEach(prev) { p in
                    previousPickRow(p)
                }
            }
        }
    }

    private func previousPickRow(_ prev: GatewayPreviousPick) -> some View {
        Button {
            if !settings.symbols.contains(where: { $0.code == prev.code }) {
                _ = settings.addSymbol(code: prev.code, name: prev.name, group: "观察")
            }
            onOpenSymbol(prev.code, prev.name)
        } label: {
            HStack(alignment: .top, spacing: UITokens.stackTight) {
                Text("#\(prev.rank)")
                    .font(.system(size: UITokens.bodySize, weight: .bold, design: .rounded))
                    .monospacedDigit()
                    .foregroundStyle(Color.indigo)
                    .frame(width: 20, alignment: .leading)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: UITokens.stackTight) {
                        Text(prev.name)
                            .font(.system(size: UITokens.bodySize, weight: .semibold))
                            .lineLimit(1)
                        if let t1Pct = prev.t1Pct {
                            Text(String(format: "T+1 %@%.1f%%", t1Pct >= 0 ? "+" : "", t1Pct))
                                .font(.system(size: UITokens.metaSize, weight: .semibold))
                                .monospacedDigit()
                                .foregroundStyle(UITokens.trend(t1Pct))
                        }
                        if let real = prev.t1RealPct {
                            Text(String(format: "实 %@%.1f%%", real >= 0 ? "+" : "", real))
                                .font(.system(size: UITokens.metaSize))
                                .monospacedDigit()
                                .foregroundStyle(UITokens.trend(real))
                        }
                        Spacer(minLength: 0)
                    }
                    HStack(spacing: UITokens.stackTight) {
                        if let entryLabel = prev.entryLabel {
                            Text(entryLabel)
                                .font(.system(size: UITokens.microSize, weight: .bold))
                                .foregroundStyle(.indigo)
                                .padding(.horizontal, UITokens.pillHPad)
                                .padding(.vertical, UITokens.pillVPad)
                                .background(Color.indigo.opacity(UITokens.pillOpacity), in: Capsule())
                        }
                        if let entryStatus = prev.entryStatus,
                           entryStatus.hasPrefix("invalid") {
                            pill(text: prev.entryStatusLabel ?? "买入失效 · 勿追", signal: .danger)
                        }
                        if let sLow = prev.sellPriceLow, let sHigh = prev.sellPriceHigh {
                            HStack(spacing: 3) {
                                Text("卖")
                                    .font(.system(size: UITokens.microSize, weight: .bold))
                                    .foregroundStyle(UITokens.color(.observe))
                                Text(String(format: "%.2f–%.2f（≥5%%）", sLow, sHigh))
                                    .font(.system(size: UITokens.bodySize, weight: .semibold, design: .monospaced))
                                    .monospacedDigit()
                                    .foregroundStyle(.primary)
                                    .padding(.horizontal, 4).padding(.vertical, 1)
                                    .background(UITokens.color(.observe).opacity(0.08), in: RoundedRectangle(cornerRadius: 3))
                            }
                        }
                        if let basis = prev.sellBasisPrice {
                            Text(String(format: "基准 %.2f", basis))
                                .font(.system(size: UITokens.microSize))
                                .monospacedDigit()
                                .foregroundStyle(.tertiary)
                        }
                        Spacer(minLength: 0)
                    }
                    .help("卖出价按真实/计划买入成本计算，最低覆盖 5% 盈利目标并预留约 0.2% 成本；不使用 T+1 收盘价事后反推")
                }
            }
            .padding(.vertical, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: - 小工具

    private func pill(text: String, signal: UITokens.Signal) -> some View {
        Text(text)
            .font(.system(size: UITokens.microSize, weight: .bold))
            .foregroundStyle(UITokens.color(signal))
            .padding(.horizontal, UITokens.pillHPad)
            .padding(.vertical, UITokens.pillVPad)
            .background(UITokens.background(signal), in: Capsule())
    }

    private func pickEntryStatus(_ pick: GatewayPick) -> (text: String, signal: UITokens.Signal) {
        if let status = pick.meta.outcome?.entryStatus, status.hasPrefix("invalid") {
            return (pick.meta.outcome?.entryStatusLabel ?? "买入失效 · 勿追", .danger)
        }
        guard pick.date == todayCNDateKey else {
            return ("上一交易日推荐", .neutral)
        }
        let calendar = Calendar(identifier: .gregorian)
        let cn = TimeZone(identifier: "Asia/Shanghai") ?? .current
        let parts = calendar.dateComponents(in: cn, from: Date())
        let minute = (parts.hour ?? 0) * 60 + (parts.minute ?? 0)
        if minute < 15 * 60 {
            return ("今日尾盘", .buy)
        }
        if pick.meta.plan?.entryTiming == "next_session_pullback" {
            return ("明日回踩", .observe)
        }
        return ("今日已收盘", .observe)
    }

    private func strategySignal(_ strategy: String) -> UITokens.Signal {
        switch strategy {
        case "做T": return .audit
        case "中线": return .neutral
        default: return .observe
        }
    }

    private func pickTargetLabel(_ pick: GatewayPick) -> String {
        if pick.date == todayCNDateKey { return "目标：下一交易日上涨" }
        if pick.meta.outcome?.t1Pct != nil { return "T+1 已验证" }
        return "目标：下一交易日 T+1"
    }

    private func pickHelp(_ pick: GatewayPick) -> String {
        let plan = pick.meta.plan
        let entry = pickEntryStatus(pick).text
        let strategy = plan?.strategy ?? "短线"
        let exit = plan?.exitRule ?? "T+1 观察，不承诺次日上涨"
        return "\(pick.name) \(pick.code) · \(entry) · \(strategy)\n目标：T+1 收盘正收益（概率筛选，非保证）\n计划：\(exit)\n\(pick.reasons.joined(separator: " / ")) · 综合分 \(Int(pick.score))\(pick.aiNote.isEmpty ? "" : "\nAI：\(pick.aiNote)")\n\(pickHelp_AutoWeight(pick.meta.autoWeight))\n点击加入自选并去盯盘（仅关注建议，不构成投资建议）"
    }

    private func pickHelp_AutoWeight(_ weight: GatewayPick.AutoWeight?) -> String {
        guard let weight,
              let adjustment = weight.adjustment,
              abs(adjustment) >= 0.05 else {
            return "历史样本未达自动调权门槛"
        }
        let evidence = (weight.tags ?? []).map {
            String(format: "%@ %d样本/胜率%.0f%%/%@%.1f分",
                   $0.tag, $0.samples, $0.winRate * 100,
                   $0.delta >= 0 ? "+" : "", $0.delta)
        }.joined(separator: "；")
        return String(format: "复盘学习调权 %@%.1f分\n%@",
                      adjustment >= 0 ? "+" : "", adjustment,
                      evidence.isEmpty ? "无可展示标签证据" : evidence)
    }

    private var todayCNDateKey: String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: Date())
    }
}

/// §I.1 三层折叠的本地折叠状态（详情 / 元数据）。
final class PicksCardUIState: ObservableObject {
    @Published var showDetail: Bool = false
    @Published var showAudit: Bool = false
}

/// PicksCardView 的执行曲线视图，与卡片用色一致（§I.2）。
struct PickExecutionCurveTokenView: View {
    let points: [ExecutionStats.CurvePoint]

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 10) {
                Label("纸面", systemImage: "minus")
                    .foregroundStyle(.blue)
                Label("次日开盘", systemImage: "minus")
                    .foregroundStyle(UITokens.color(.observe))
                Spacer(minLength: 0)
                if let last = points.last {
                    Text(String(format: "累计 %+.1f%% / %+.1f%%",
                                last.paperCumulativePct, last.openCumulativePct))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
            }
            .font(.system(size: 8, weight: .semibold))
            Canvas { context, size in
                let values: [Double] = points.flatMap {
                    [$0.paperCumulativePct, $0.openCumulativePct]
                } + [0.0]
                let rawMin = values.min() ?? 0
                let rawMax = values.max() ?? 0
                let spread = max(rawMax - rawMin, 1)
                let minY = rawMin - spread * 0.12
                let maxY = rawMax + spread * 0.12
                func location(_ index: Int, _ value: Double) -> CGPoint {
                    let x = points.count <= 1 ? size.width / 2 :
                        CGFloat(index) / CGFloat(points.count - 1) * size.width
                    let ratio = (value - minY) / max(maxY - minY, 0.001)
                    return CGPoint(x: x, y: size.height * (1 - ratio))
                }
                if minY <= 0, maxY >= 0 {
                    var zero = Path()
                    let y = location(0, 0).y
                    zero.move(to: CGPoint(x: 0, y: y))
                    zero.addLine(to: CGPoint(x: size.width, y: y))
                    context.stroke(zero, with: .color(.secondary.opacity(0.25)),
                                   style: StrokeStyle(lineWidth: 0.7, dash: [3, 3]))
                }
                func draw(_ keyPath: KeyPath<ExecutionStats.CurvePoint, Double>, color: Color) {
                    var path = Path()
                    for (index, point) in points.enumerated() {
                        let p = location(index, point[keyPath: keyPath])
                        if index == 0 { path.move(to: p) } else { path.addLine(to: p) }
                    }
                    context.stroke(path, with: .color(color),
                                   style: StrokeStyle(lineWidth: 1.6, lineJoin: .round))
                }
                draw(\.paperCumulativePct, color: .blue)
                draw(\.openCumulativePct, color: UITokens.color(.observe))
            }
        }
        .help("同一批推荐按日等权聚合：蓝线以推荐日收盘为纸面买入基准，橙线以次日真实开盘价为基准")
    }
}
