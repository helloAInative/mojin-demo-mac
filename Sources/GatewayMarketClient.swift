import Foundation

struct GatewaySettingsSnapshot: Codable {
    var currentCode: String?
    var levelsByCode: [String: SymbolLevels]?
    var strategies: [String: StrategyNote]?
    var comboStrategies: [ComboStrategy]?
    var diaries: [String: String]?
    var alertsEnabled: Bool?
    var openCloseBrief: Bool?
    var theme: AppTheme?
    var menuBarFormat: MenuBarFormat?
    var fontSize: FontSizePref?
    var indicator: String?
    var notifyConfig: NotifyGovernor.Config?
    var aiConfig: AIConfig?
    var boardShowHS300: Bool?
    var boardShowBJ50: Bool?
    var smartPicksPaused: Bool?

    var isEmpty: Bool {
        currentCode == nil && levelsByCode == nil && strategies == nil &&
        comboStrategies == nil && diaries == nil && alertsEnabled == nil &&
        openCloseBrief == nil && theme == nil && menuBarFormat == nil &&
        fontSize == nil && indicator == nil && notifyConfig == nil &&
        aiConfig == nil && boardShowHS300 == nil && boardShowBJ50 == nil &&
        smartPicksPaused == nil
    }
}

struct GatewayPosition: Codable {
    var code: String
    var cost: Double
    var shares: Double
    var stopLoss: Double
    var takeProfit: Double
    var positionPct: Double
    var note: String?
    var updatedAt: Int64?
}

struct GatewayWatchItem: Codable {
    var code: String
    var name: String
    var market: String
    var pinned: Bool
    var group: String
    var addedAt: Int64?
    var updatedAt: Int64?
}

struct GatewayRemoteState {
    var positions: [GatewayPosition]
    var watchlist: [GatewayWatchItem]
    var settings: GatewaySettingsSnapshot

    var isEmpty: Bool { positions.isEmpty && watchlist.isEmpty && settings.isEmpty }
}

struct GatewayQuoteUpdate {
    var code: String
    var quote: Quote
}

/// §F.3：个股新闻（东财，经网关缓存）。
struct GatewayNewsItem: Codable, Equatable, Identifiable {
    var code: String
    var title: String
    var summary: String
    var media: String
    var url: String
    var publishedAt: Date

    var id: String { url }

    enum CodingKeys: String, CodingKey {
        case code, title, summary, media, url
        case publishedAt = "published_at"
    }

    /// 北京时间的 "MM-dd HH:mm"。
    var cnClock: String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: publishedAt)
    }
}

/// §F.4：机构研报（东财研报库）。
struct GatewayResearchReport: Codable, Equatable, Identifiable {
    var code: String
    var title: String
    var org: String
    var publishDate: String
    var rating: String
    var lastRating: String
    var ratingChange: Int?
    var researcher: String
    var industry: String
    var aimPriceHigh: Double?
    var aimPriceLow: Double?
    var url: String

    var id: String { url }

    /// 东财评级文案 → 涨跌色：买入 / 增持类红，卖出 / 减持类绿。
    var isBullish: Bool? {
        switch rating {
        case "买入", "增持", "强买", "推荐", "强烈推荐": return true
        case "卖出", "减持", "回避": return false
        default: return nil
        }
    }

    var ratingChangeText: String {
        switch ratingChange {
        case 1: return "上调"
        case 2: return "下调"
        case 3: return "维持"
        default: return lastRating.isEmpty ? "新覆盖" : "续评"
        }
    }

    /// 目标价摘要（详情 tooltip 用）。
    var aimPriceText: String {
        switch (aimPriceLow, aimPriceHigh) {
        case let (low?, high?) where high > 0:
            return String(format: "%.2f-%.2f", min(low, high), max(low, high))
        case let (high?, _) where high > 0:
            return String(format: "%.2f", high)
        default: return "未给出"
        }
    }

    enum CodingKeys: String, CodingKey {
        case code, title, org, rating, researcher, industry, url
        case publishDate = "publish_date"
        case lastRating = "last_rating"
        case ratingChange = "rating_change"
        case aimPriceHigh = "aim_price_high"
        case aimPriceLow = "aim_price_low"
    }
}

/// §F.4：个股所属概念 / 行业板块（含板块指数与涨跌幅）。
struct GatewaySectorBoard: Codable, Equatable, Identifiable {
    var code: String
    var boardCode: String
    var name: String
    var isPrecise: Bool
    var reason: String
    var price: Double?
    var changePct: Double?

    var id: String { boardCode }

    enum CodingKeys: String, CodingKey {
        case code, name, reason, price
        case boardCode = "board_code"
        case isPrecise = "is_precise"
        case changePct = "change_pct"
    }
}

/// B.4：逐笔成交（服务端 TickItem）。
struct GatewayTick: Codable, Equatable, Identifiable {
    var ts: Date
    var price: Double
    /// 手
    var volume: Int
    /// 1 买盘 / 2 卖盘 / 4 中性（0 集合竞价）
    var direction: Int

    var id: Date { ts }
}

/// 逐笔按分钟聚合桶（B.4：分时下方柱状 + 下钻数据）。
struct TickBucket: Equatable, Identifiable {
    /// "HHmm"
    var minute: String
    var volume: Int
    var buyVolume: Int
    var sellVolume: Int
    var ticks: [GatewayTick]
    var id: String { minute }

    /// 净买卖决定柱色（红买绿卖）
    var netBuy: Bool { buyVolume >= sellVolume }
}

enum TickBucketer {
    /// 按北京时间分钟分桶（升序）；direction 1/0 归买盘、2 归卖盘、4 中性不计入买卖。
    static func buckets(from ticks: [GatewayTick]) -> [TickBucket] {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "HHmm"
        var order: [String] = []
        var map: [String: TickBucket] = [:]
        for tick in ticks.sorted(by: { $0.ts < $1.ts }) {
            let minute = formatter.string(from: tick.ts)
            guard minute.count == 4 else { continue }
            var bucket = map[minute] ?? TickBucket(minute: minute, volume: 0,
                                                   buyVolume: 0, sellVolume: 0, ticks: [])
            bucket.volume += tick.volume
            switch tick.direction {
            case 2: bucket.sellVolume += tick.volume
            default: bucket.buyVolume += tick.volume   // 1 买 / 0 竞价 / 4 中性偏买侧计数
            }
            bucket.ticks.append(tick)
            if map[minute] == nil { order.append(minute) }
            map[minute] = bucket
        }
        return order.map { map[$0]! }
    }
}

/// A 股池智能推荐：单条（服务端 daily_pick 行）。
struct GatewayPick: Codable, Equatable, Identifiable {
    var date: String
    var code: String
    var name: String
    var rank: Int
    var score: Double
    /// 量化 / 消息面标签
    var reasons: [String]
    /// AI 一句话理由（纯量化版为空）
    var aiNote: String
    var meta: Meta

    var id: String { "\(date)-\(code)" }

    struct Meta: Codable, Equatable {
        var close: Double?
        var pct: Double?
        var industry: String?
        var outcome: Outcome?
        var autoWeight: AutoWeight?
        var plan: TradePlan?
        /// 今日已涨停（T 日买不进，需次日开盘）
        var isLimitUp: Bool?
        /// §A.9 卖出价区间基础数据：基于该票近 30 天 outcome.t1_pct
        var sellZoneBasis: SellZoneBasis?
        /// §A 负面信号细分（减持/问询/立案/...）
        var negativeSignals: [NegativeSignal]?
        var risk: RiskMetrics?

        enum CodingKeys: String, CodingKey {
            case close, pct, industry, outcome, plan, risk
            case autoWeight = "auto_weight"
            case isLimitUp = "is_limit_up"
            case sellZoneBasis = "sell_zone_basis"
            case negativeSignals = "negative_signals"
        }
    }

    struct RiskMetrics: Codable, Equatable {
        var beta60d: Double?
        var avgAmount20d: Double?
        var maxDrawdown20d: Double?
        var expectedShortfall10pct20d: Double?
        var t1Reach5pctRate20d: Double?
        var t1Reach5pctWilsonLow: Double?
        var targetStable: Bool?
        var return5d: Double?
        var crowdingPenalty: Double?
        var stressLossMarketDown2pct: Double?

        enum CodingKeys: String, CodingKey {
            case beta60d = "beta_60d"
            case avgAmount20d = "avg_amount_20d"
            case maxDrawdown20d = "max_drawdown_20d"
            case expectedShortfall10pct20d = "expected_shortfall_10pct_20d"
            case t1Reach5pctRate20d = "t1_reach_5pct_rate_20d"
            case t1Reach5pctWilsonLow = "t1_reach_5pct_wilson_low"
            case targetStable = "target_stable"
            case return5d = "return_5d"
            case crowdingPenalty = "crowding_penalty"
            case stressLossMarketDown2pct = "stress_loss_market_down_2pct"
        }
    }

    struct SellZoneBasis: Codable, Equatable {
        var samples: Int
        var winRate: Double?
        var avgT1Pct: Double?
        var winRateFactor: Double?

        enum CodingKeys: String, CodingKey {
            case samples
            case winRate = "win_rate"
            case avgT1Pct = "avg_t1_pct"
            case winRateFactor = "win_rate_factor"
        }
    }

    struct NegativeSignal: Codable, Equatable {
        var category: String
        var title: String
    }

    struct TradePlan: Codable, Equatable {
        var signalDate: String?
        var target: String?
        var objective: String?
        var entryTiming: String?
        var entryLabel: String?
        var entryWindow: String?
        var strategy: String?
        var hasBasePosition: Bool?
        var exitRule: String?
        /// §A.9 买入价下界（现价 ±2%）
        var buyPriceLow: Double?
        /// §A.9 买入价上界
        var buyPriceHigh: Double?
        /// §A.9 卖出价下界（基于 T+1 历史均值 × 胜率因子 ±1.5%）
        var sellPriceLow: Double?
        /// §A.9 卖出价上界
        var sellPriceHigh: Double?
        var profitTargetPrice: Double?
        var targetReturnPct: Double?
        /// §A.9 买入价基准（现价 close，用于回溯）
        var buyBasisClose: Double?

        enum CodingKeys: String, CodingKey {
            case target, objective, strategy
            case signalDate = "signal_date"
            case entryTiming = "entry_timing"
            case entryLabel = "entry_label"
            case entryWindow = "entry_window"
            case hasBasePosition = "has_base_position"
            case exitRule = "exit_rule"
            case buyPriceLow = "buy_price_low"
            case buyPriceHigh = "buy_price_high"
            case sellPriceLow = "sell_price_low"
            case sellPriceHigh = "sell_price_high"
            case profitTargetPrice = "profit_target_price"
            case targetReturnPct = "target_return_pct"
            case buyBasisClose = "buy_basis_close"
        }
    }

    struct AutoWeight: Codable, Equatable {
        var adjustment: Double?
        var tags: [WeightEvidence]?
        var minimumSamples: Int?
        var lookbackDays: Int?

        enum CodingKeys: String, CodingKey {
            case adjustment, tags
            case minimumSamples = "minimum_samples"
            case lookbackDays = "lookback_days"
        }
    }

    struct WeightEvidence: Codable, Equatable {
        var tag: String
        var samples: Int
        var winRate: Double
        var delta: Double

        enum CodingKeys: String, CodingKey {
            case tag, samples, delta
            case winRate = "win_rate"
        }
    }

    struct Outcome: Codable, Equatable {
        var t1Pct: Double?
        var t5Pct: Double?
        var entryStatus: String?
        var entryStatusLabel: String?

        enum CodingKeys: String, CodingKey {
            case t1Pct = "t1_pct"
            case t5Pct = "t5_pct"
            case entryStatus = "entry_status"
            case entryStatusLabel = "entry_status_label"
        }
    }

    enum CodingKeys: String, CodingKey {
        case date, code, name, rank, score, reasons, meta
        case aiNote = "ai_note"
    }
}

/// 推荐文档：当日尾盘清单 + 近 30 天 T+1 主回测（T+5 中线参考）。
struct GatewayPicksDocument: Decodable, Equatable {
    struct Audit: Decodable, Equatable {
        var poolSources: [String]
        var concentration: Concentration?
        var limitUpBreakdown: String?
        var experimentId: String?

        struct Concentration: Decodable, Equatable {
            var maxPerIndustry: Int
            var summary: String
        }

        enum CodingKeys: String, CodingKey {
            case poolSources = "pool_sources"
            case concentration
            case limitUpBreakdown = "limit_up_breakdown"
            case experimentId = "experiment_id"
        }
    }

    var date: String
    var picks: [GatewayPick]
    var samples: Int
    var t1WinRate: Double
    /// Wilson 95% 区间下界：避免「胜率 70% · 样本 10」被误读为稳定指标
    var t1WinRateLow: Double
    /// Wilson 95% 区间上界
    var t1WinRateHigh: Double
    /// Wilson 区间半宽（绝对值），便于直接显示 ±margin
    var t1WinRateMargin: Double
    /// true = 样本 ≥ 30，胜率可参考；false = 样本不足，仅参考方向
    var t1SamplesSufficient: Bool
    var avgT1Pct: Double
    var t5Samples: Int
    var t5WinRate: Double
    var t5WinRateLow: Double
    var t5WinRateHigh: Double
    var t5WinRateMargin: Double
    var t5SamplesSufficient: Bool
    var avgT5Pct: Double

    private enum StatsKeys: String, CodingKey {
        case samples
        case t1WinRate = "t1_win_rate"
        case t1WinRateLow = "t1_win_rate_low"
        case t1WinRateHigh = "t1_win_rate_high"
        case t1WinRateMargin = "t1_win_rate_margin"
        case t1SamplesSufficient = "t1_samples_sufficient"
        case avgT1Pct = "avg_t1_pct"
        case t5Samples = "t5_samples"
        case t5WinRate = "t5_win_rate"
        case t5WinRateLow = "t5_win_rate_low"
        case t5WinRateHigh = "t5_win_rate_high"
        case t5WinRateMargin = "t5_win_rate_margin"
        case t5SamplesSufficient = "t5_samples_sufficient"
        case avgT5Pct = "avg_t5_pct"
        case tags
        case byRegime = "by_regime"
        case audit
    }

    private enum CodingKeys: String, CodingKey {
        case date, picks, stats, market, execution
        case executeHint = "execute_hint"
        case previousPicks = "previous_picks"
    }

    /// 标签级 T+1 回测（哪个因子更适合隔日目标，按样本数降序 ≤6）
    var tags: [TagStat]
    /// §A.11 按推荐日市场环境分桶的 T+1 胜率。
    var byRegime: [RegimeStat]
    /// 生成时市场环境：隔夜美股 + A 股大盘情绪。
    var market: MarketInfo?
    /// 执行时机：「当天下午可买入」或「次日开盘买入…」
    var executeHint: String?
    /// 真实执行口径（次日开盘买入统计）
    var execution: ExecutionStats?
    /// §A.14 规则版本与候选池审计信息；旧服务端缺失时为 nil。
    var audit: Audit?

    /// §A.9 上一交易日推荐（已有 T+1 outcome 回写），用于展示「昨日推荐 → 今日卖点」
    var previousPicks: [GatewayPreviousPick]?

    /// 今日可尾盘买进子集：
    /// 1. entryTiming == today_close（服务端 14:45-15:00 窗口标的）
    /// 2. 未涨停（meta.is_limit_up != true）
    ///    —— 已涨停的票服务端会改成 entry_timing=next_session_open，不计入此列表。
    var tailBuyPicks: [GatewayPick] {
        picks.filter { pick in
            pick.meta.plan?.entryTiming == "today_close"
                && (pick.meta.isLimitUp ?? false) == false
        }
    }

    /// 今日被识别为涨停、需要次日开盘买入的子集
    var nextOpenPicks: [GatewayPick] {
        picks.filter { ($0.meta.isLimitUp ?? false) == true }
    }

    var currentRegimeStat: RegimeStat? {
        guard let avg = market?.cn?.avgPct else { return nil }
        let key: String
        if avg <= -2.0 { key = "crash" }
        else if avg <= -0.8 { key = "bear" }
        else if avg >= 0.8 { key = "bull" }
        else { key = "range" }
        return byRegime.first(where: { $0.regime == key })
    }

    struct TagStat: Decodable, Equatable, Identifiable {
        var tag: String
        var samples: Int
        var winRate: Double
        var winRateLow: Double
        var winRateHigh: Double
        var winRateMargin: Double
        var samplesSufficient: Bool
        var id: String { tag }

        enum CodingKeys: String, CodingKey {
            case tag, samples
            case winRate = "win_rate"
            case winRateLow = "win_rate_low"
            case winRateHigh = "win_rate_high"
            case winRateMargin = "win_rate_margin"
            case samplesSufficient = "samples_sufficient"
        }
    }

    struct RegimeStat: Decodable, Equatable, Identifiable {
        var regime: String
        var label: String
        var samples: Int
        var winRate: Double
        var winRateLow: Double
        var winRateHigh: Double
        var winRateMargin: Double
        var samplesSufficient: Bool
        var avgT1Pct: Double
        var id: String { regime }

        enum CodingKeys: String, CodingKey {
            case regime, label, samples
            case winRate = "win_rate"
            case winRateLow = "win_rate_low"
            case winRateHigh = "win_rate_high"
            case winRateMargin = "win_rate_margin"
            case samplesSufficient = "samples_sufficient"
            case avgT1Pct = "avg_t1_pct"
        }
    }

    struct MarketInfo: Decodable, Equatable {
        var us: US?
        var cn: CN?
        var confirmation: Confirmation?

        /// 兼容 §A.10 之前服务端直接返回 `{djia, ixic}` 的历史文档。
        var djia: Double? { us?.djia }
        var ixic: Double? { us?.ixic }

        struct US: Decodable, Equatable {
            var djia: Double?
            var ixic: Double?
        }

        struct CN: Decodable, Equatable {
            var shPct: Double?
            var szPct: Double?
            var gemPct: Double?
            var hs300Pct: Double?
            var avgPct: Double?
            var riskScore: Double?

            enum CodingKeys: String, CodingKey {
                case shPct = "sh_pct"
                case szPct = "sz_pct"
                case gemPct = "gem_pct"
                case hs300Pct = "hs300_pct"
                case avgPct = "avg_pct"
                case riskScore = "risk_score"
            }
        }

        struct Confirmation: Decodable, Equatable {
            var status: String
            var overlapRatio: Double
            var regimeChanged: Bool
            var reason: String

            enum CodingKeys: String, CodingKey {
                case status, reason
                case overlapRatio = "overlap_ratio"
                case regimeChanged = "regime_changed"
            }
        }

        private enum CodingKeys: String, CodingKey {
            case us, cn, confirmation, djia, ixic
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            us = try c.decodeIfPresent(US.self, forKey: .us)
            cn = try c.decodeIfPresent(CN.self, forKey: .cn)
            confirmation = try c.decodeIfPresent(Confirmation.self, forKey: .confirmation)
            if us == nil {
                let legacyDJIA = try c.decodeIfPresent(Double.self, forKey: .djia)
                let legacyIXIC = try c.decodeIfPresent(Double.self, forKey: .ixic)
                if legacyDJIA != nil || legacyIXIC != nil {
                    us = US(djia: legacyDJIA, ixic: legacyIXIC)
                }
            }
        }
    }

    init(date: String, picks: [GatewayPick], samples: Int, t5WinRate: Double, avgT5Pct: Double,
         tags: [TagStat] = [], market: MarketInfo? = nil,
         t1WinRate: Double = 0, avgT1Pct: Double = 0, t5Samples: Int = 0,
         t1WinRateLow: Double = 0, t1WinRateHigh: Double = 0, t1WinRateMargin: Double = 0,
         t1SamplesSufficient: Bool = false,
         t5WinRateLow: Double = 0, t5WinRateHigh: Double = 0, t5WinRateMargin: Double = 0,
         t5SamplesSufficient: Bool = false) {
        self.date = date
        self.picks = picks
        self.samples = samples
        self.t1WinRate = t1WinRate
        self.t1WinRateLow = t1WinRateLow
        self.t1WinRateHigh = t1WinRateHigh
        self.t1WinRateMargin = t1WinRateMargin
        self.t1SamplesSufficient = t1SamplesSufficient
        self.avgT1Pct = avgT1Pct
        self.t5Samples = t5Samples
        self.t5WinRate = t5WinRate
        self.t5WinRateLow = t5WinRateLow
        self.t5WinRateHigh = t5WinRateHigh
        self.t5WinRateMargin = t5WinRateMargin
        self.t5SamplesSufficient = t5SamplesSufficient
        self.avgT5Pct = avgT5Pct
        self.tags = tags
        self.byRegime = []
        self.market = market
        self.executeHint = nil
        self.execution = nil
        self.audit = nil
        self.previousPicks = nil
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        date = try c.decode(String.self, forKey: .date)
        picks = try c.decode([GatewayPick].self, forKey: .picks)
        let stats = try c.nestedContainer(keyedBy: StatsKeys.self, forKey: .stats)
        samples = (try? stats.decodeIfPresent(Int.self, forKey: .samples)) ?? 0
        t1WinRate = (try? stats.decodeIfPresent(Double.self, forKey: .t1WinRate)) ?? 0
        t1WinRateLow = (try? stats.decodeIfPresent(Double.self, forKey: .t1WinRateLow)) ?? 0
        t1WinRateHigh = (try? stats.decodeIfPresent(Double.self, forKey: .t1WinRateHigh)) ?? 0
        t1WinRateMargin = (try? stats.decodeIfPresent(Double.self, forKey: .t1WinRateMargin)) ?? 0
        t1SamplesSufficient = (try? stats.decodeIfPresent(Bool.self, forKey: .t1SamplesSufficient)) ?? false
        avgT1Pct = (try? stats.decodeIfPresent(Double.self, forKey: .avgT1Pct)) ?? 0
        t5Samples = (try? stats.decodeIfPresent(Int.self, forKey: .t5Samples)) ?? 0
        t5WinRate = (try? stats.decodeIfPresent(Double.self, forKey: .t5WinRate)) ?? 0
        t5WinRateLow = (try? stats.decodeIfPresent(Double.self, forKey: .t5WinRateLow)) ?? 0
        t5WinRateHigh = (try? stats.decodeIfPresent(Double.self, forKey: .t5WinRateHigh)) ?? 0
        t5WinRateMargin = (try? stats.decodeIfPresent(Double.self, forKey: .t5WinRateMargin)) ?? 0
        t5SamplesSufficient = (try? stats.decodeIfPresent(Bool.self, forKey: .t5SamplesSufficient)) ?? false
        avgT5Pct = (try? stats.decodeIfPresent(Double.self, forKey: .avgT5Pct)) ?? 0
        tags = (try? stats.decodeIfPresent([TagStat].self, forKey: .tags)) ?? []
        byRegime = (try? stats.decodeIfPresent([RegimeStat].self, forKey: .byRegime)) ?? []
        audit = try? stats.decodeIfPresent(Audit.self, forKey: .audit)
        market = try? c.decodeIfPresent(MarketInfo.self, forKey: .market)
        executeHint = try? c.decodeIfPresent(String.self, forKey: .executeHint)
        execution = try? c.decodeIfPresent(ExecutionStats.self, forKey: .execution)
        previousPicks = try? c.decodeIfPresent([GatewayPreviousPick].self, forKey: .previousPicks)
    }
}

/// §A.9 上一交易日推荐（昨日 / 最近一次有 outcome 回写）
struct GatewayPreviousPick: Codable, Equatable, Identifiable {
    var date: String
    var code: String
    var name: String
    var rank: Int
    var close: Double?
    var t1Open: Double?
    var t1Close: Double?
    var t1Pct: Double?
    var t1RealPct: Double?
    var entryGap: Double?
    var sellPriceLow: Double?
    var sellPriceHigh: Double?
    var sellBasisPrice: Double?
    var sellBasisKind: String?
    var openStrength: String?
    var sellAction: String?
    var sellActionLabel: String?
    var riskStopPrice: Double?
    var targetReached: Bool?
    var entryTiming: String?
    var entryLabel: String?
    var entryStatus: String?
    var entryStatusLabel: String?
    var reasons: [String]

    var id: String { "\(date)-\(code)" }

    enum CodingKeys: String, CodingKey {
        case date, code, name, rank, close, reasons
        case t1Open = "t1_open"
        case t1Close = "t1_close"
        case t1Pct = "t1_pct"
        case t1RealPct = "t1_real_pct"
        case entryGap = "entry_gap"
        case sellPriceLow = "sell_price_low"
        case sellPriceHigh = "sell_price_high"
        case sellBasisPrice = "sell_basis_price"
        case sellBasisKind = "sell_basis_kind"
        case openStrength = "open_strength"
        case sellAction = "sell_action"
        case sellActionLabel = "sell_action_label"
        case riskStopPrice = "risk_stop_price"
        case targetReached = "target_reached"
        case entryTiming = "entry_timing"
        case entryLabel = "entry_label"
        case entryStatus = "entry_status"
        case entryStatusLabel = "entry_status_label"
    }
}

/// 真实执行口径统计（从次日开盘价计算）。
struct ExecutionStats: Decodable, Equatable {
    /// 平均执行溢价（次日开盘 vs 推荐日收盘，%）
    var avgEntryGap: Double
    var t1RealWinRate: Double
    /// Wilson 95% 区间下界（真实执行口径）
    var t1RealWinRateLow: Double
    /// Wilson 95% 区间上界
    var t1RealWinRateHigh: Double
    /// Wilson 区间半宽
    var t1RealWinRateMargin: Double
    /// 样本是否足以给出有意义的胜率（≥30）
    var t1RealSamplesSufficient: Bool
    var avgT1Real: Double
    var avgWin: Double
    var avgLoss: Double
    var avgMaxDd: Double
    var maxDrawdown: Double
    var expectedShortfall10: Double
    var target5pctSamples: Int
    var target5pctHitRate: Double
    var target5pctWilsonLow: Double
    var target5pctWilsonHigh: Double
    var winLossRatio: Double
    var curve: [CurvePoint]
    var executionDrag: Double
    var executionWarning: Bool
    var executionHint: String

    struct CurvePoint: Decodable, Equatable, Identifiable {
        var date: String
        var samples: Int
        var paperT1Pct: Double
        var openT1Pct: Double
        var paperCumulativePct: Double
        var openCumulativePct: Double
        var id: String { date }

        enum CodingKeys: String, CodingKey {
            case date, samples
            case paperT1Pct = "paper_t1_pct"
            case openT1Pct = "open_t1_pct"
            case paperCumulativePct = "paper_cumulative_pct"
            case openCumulativePct = "open_cumulative_pct"
        }
    }

    enum CodingKeys: String, CodingKey {
        case avgEntryGap = "avg_entry_gap"
        case t1RealWinRate = "t1_real_win_rate"
        case t1RealWinRateLow = "t1_real_win_rate_low"
        case t1RealWinRateHigh = "t1_real_win_rate_high"
        case t1RealWinRateMargin = "t1_real_win_rate_margin"
        case t1RealSamplesSufficient = "t1_real_samples_sufficient"
        case avgT1Real = "avg_t1_real"
        case avgWin = "avg_win"
        case avgLoss = "avg_loss"
        case avgMaxDd = "avg_max_dd"
        case maxDrawdown = "max_drawdown"
        case expectedShortfall10 = "expected_shortfall_10"
        case target5pctSamples = "target_5pct_samples"
        case target5pctHitRate = "target_5pct_hit_rate"
        case target5pctWilsonLow = "target_5pct_wilson_low"
        case target5pctWilsonHigh = "target_5pct_wilson_high"
        case winLossRatio = "win_loss_ratio"
        case curve
        case executionDrag = "execution_drag"
        case executionWarning = "execution_warning"
        case executionHint = "execution_hint"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        avgEntryGap = try c.decodeIfPresent(Double.self, forKey: .avgEntryGap) ?? 0
        t1RealWinRate = try c.decodeIfPresent(Double.self, forKey: .t1RealWinRate) ?? 0
        t1RealWinRateLow = try c.decodeIfPresent(Double.self, forKey: .t1RealWinRateLow) ?? 0
        t1RealWinRateHigh = try c.decodeIfPresent(Double.self, forKey: .t1RealWinRateHigh) ?? 0
        t1RealWinRateMargin = try c.decodeIfPresent(Double.self, forKey: .t1RealWinRateMargin) ?? 0
        t1RealSamplesSufficient = try c.decodeIfPresent(Bool.self, forKey: .t1RealSamplesSufficient) ?? false
        avgT1Real = try c.decodeIfPresent(Double.self, forKey: .avgT1Real) ?? 0
        avgWin = try c.decodeIfPresent(Double.self, forKey: .avgWin) ?? 0
        avgLoss = try c.decodeIfPresent(Double.self, forKey: .avgLoss) ?? 0
        avgMaxDd = try c.decodeIfPresent(Double.self, forKey: .avgMaxDd) ?? 0
        maxDrawdown = try c.decodeIfPresent(Double.self, forKey: .maxDrawdown) ?? 0
        expectedShortfall10 = try c.decodeIfPresent(Double.self, forKey: .expectedShortfall10) ?? 0
        target5pctSamples = try c.decodeIfPresent(Int.self, forKey: .target5pctSamples) ?? 0
        target5pctHitRate = try c.decodeIfPresent(Double.self, forKey: .target5pctHitRate) ?? 0
        target5pctWilsonLow = try c.decodeIfPresent(Double.self, forKey: .target5pctWilsonLow) ?? 0
        target5pctWilsonHigh = try c.decodeIfPresent(Double.self, forKey: .target5pctWilsonHigh) ?? 0
        winLossRatio = try c.decodeIfPresent(Double.self, forKey: .winLossRatio) ?? 0
        curve = try c.decodeIfPresent([CurvePoint].self, forKey: .curve) ?? []
        executionDrag = try c.decodeIfPresent(Double.self, forKey: .executionDrag) ?? 0
        executionWarning = try c.decodeIfPresent(Bool.self, forKey: .executionWarning) ?? false
        executionHint = try c.decodeIfPresent(String.self, forKey: .executionHint) ?? ""
    }
}

/// 每日数据归档（ROI #12 回放）：与服务端 `data/archives/{date}.json` 同构。
struct GatewayDayExport: Decodable, Equatable {
    var date: String
    var signals: [Signal]
    var codes: [String: Code]

    struct Signal: Decodable, Equatable, Identifiable {
        var id: String
        /// 发射时间（unix ms）
        var at: Double
        var kind: String
        var code: String
        var title: String
    }

    struct Code: Decodable, Equatable {
        var name: String?
        var quote: Quote?
        /// [ts(ms), price, avg, volume] 紧凑数组
        var minutes: [[Double]]?

        struct Quote: Decodable, Equatable {
            var close: Double?
            var prev: Double?
            var open: Double?
            var high: Double?
            var low: Double?
        }
    }

    /// 当前标的的回放分时（ts ms → 北京时间 HHmm）。
    func minuteBars(code: String) -> (bars: [MinuteBar], prev: Double) {
        guard let node = codes[code], let rows = node.minutes, !rows.isEmpty else {
            guard let node = codes[code] else { return ([], 0) }
            return ([], node.quote?.prev ?? 0)
        }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "HHmm"
        let bars: [MinuteBar] = rows.compactMap { row in
            guard row.count >= 4, row[0] > 0, row[1] > 0 else { return nil }
            let clock = formatter.string(from: Date(timeIntervalSince1970: row[0] / 1000.0))
            guard clock.count == 4 else { return nil }
            return MinuteBar(minute: clock, price: row[1], avg: row[2], vol: row[3])
        }
        return (bars, node.quote?.prev ?? 0)
    }
}

/// Rust 行情网关的 Swift DTO 适配层。这里不访问第三方行情源。
enum GatewayMarketClient {
    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 3.5
        config.timeoutIntervalForResource = 4
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: config)
    }()
    /// 选股会串行拉取候选日 K、消息面并调用模型，不能复用行情的 4 秒超时。
    private static let picksSession: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 240
        config.timeoutIntervalForResource = 300
        config.requestCachePolicy = .reloadIgnoringLocalCacheData
        config.waitsForConnectivity = true
        return URLSession(configuration: config)
    }()
    private static let circuit = GatewayCircuit()

    private struct RemoteQuote: Decodable {
        let name: String
        let price: Double
        let prev: Double
        let open: Double
        let high: Double
        let low: Double
        let source: String
        let ts: String
    }

    private struct RemoteMinute: Decodable {
        let ts: String
        let price: Double
        let avg_price: Double
        let volume: Double
    }

    private struct RemoteDay: Decodable {
        let date: String
        let open: Double
        let close: Double
        let high: Double
        let low: Double
        let volume: Double
    }

    enum GatewayError: Error {
        case disabled
        case invalidURL
        case unavailable
        case badResponse
    }

    private struct HealthResponse: Decodable {
        let status: String
        let db: String
    }

    static func health(baseURL: String) async throws -> String {
        let base = baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let endpoint = URL(string: base + "/health"),
              ["http", "https"].contains(endpoint.scheme?.lowercased() ?? ""),
              endpoint.host != nil else { throw GatewayError.invalidURL }
        let started = Date()
        let (data, response) = try await session.data(from: endpoint)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        let health = try JSONDecoder().decode(HealthResponse.self, from: data)
        guard health.status == "ok", health.db == "ok" else { throw GatewayError.unavailable }
        return "连接成功 · \(Int(Date().timeIntervalSince(started) * 1000)) ms · 数据库正常"
    }

    static func quote(symbol: WatchSymbol) async throws -> Quote {
        let remote: RemoteQuote = try await get("quote/\(symbol.code)")
        return try quote(from: remote, fallbackName: symbol.name)
    }

    private static func quote(from remote: RemoteQuote, fallbackName: String = "") throws -> Quote {
        guard remote.price > 0, remote.prev > 0 else { throw GatewayError.badResponse }
        let change = remote.price - remote.prev
        return Quote(
            name: remote.name.isEmpty ? fallbackName : remote.name,
            price: remote.price,
            prev: remote.prev,
            open: remote.open,
            high: remote.high,
            low: remote.low,
            change: change,
            pct: change / remote.prev * 100,
            timeText: chinaTime(remote.ts, format: "HH:mm:ss"),
            source: "网关·\(sourceName(remote.source))"
        )
    }

    /// WebSocket 行情流。断线重连由 MarketStore 负责，HTTP 轮询始终作为兜底。
    static func quoteUpdates() async throws -> AsyncThrowingStream<GatewayQuoteUpdate, Error> {
        struct Event: Decodable {
            var type: String
            var quote: RemoteQuoteWithCode
        }
        struct RemoteQuoteWithCode: Decodable {
            var code: String
            var name: String
            var price: Double
            var prev: Double
            var open: Double
            var high: Double
            var low: Double
            var source: String
            var ts: String
        }

        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard enabled, var components = URLComponents(string: base) else {
            throw GatewayError.disabled
        }
        switch components.scheme?.lowercased() {
        case "http": components.scheme = "ws"
        case "https": components.scheme = "wss"
        default: throw GatewayError.invalidURL
        }
        components.path = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            + "/api/v1/ws/quote"
        components.query = nil
        guard let url = components.url else { throw GatewayError.invalidURL }

        return AsyncThrowingStream { continuation in
            let socket = session.webSocketTask(with: url)
            socket.resume()
            let receiveTask = Task {
                do {
                    while !Task.isCancelled {
                        let message = try await socket.receive()
                        let data: Data
                        switch message {
                        case .data(let value): data = value
                        case .string(let value): data = Data(value.utf8)
                        @unknown default: continue
                        }
                        let event = try JSONDecoder().decode(Event.self, from: data)
                        guard event.type == "quote" else { continue }
                        let remote = event.quote
                        let legacy = RemoteQuote(name: remote.name, price: remote.price, prev: remote.prev,
                                                 open: remote.open, high: remote.high, low: remote.low,
                                                 source: remote.source, ts: remote.ts)
                        let value = try quote(from: legacy)
                        continuation.yield(GatewayQuoteUpdate(code: remote.code, quote: value))
                    }
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in
                receiveTask.cancel()
                socket.cancel(with: .goingAway, reason: nil)
            }
        }
    }

    static func minutes(symbol: WatchSymbol) async throws -> [MinuteBar] {
        let remote: [RemoteMinute] = try await get("quote/\(symbol.code)/minutes?limit=240")
        guard !remote.isEmpty else { throw GatewayError.badResponse }
        let bars = remote.compactMap { bar -> MinuteBar? in
            guard bar.price > 0 else { return nil }
            let minute = chinaTime(bar.ts, format: "HHmm")
            guard minute.count == 4 else { return nil }
            return MinuteBar(minute: minute, price: bar.price, avg: bar.avg_price, vol: bar.volume)
        }
        guard !bars.isEmpty else { throw GatewayError.badResponse }
        return MarketService.fillMinuteGaps(bars)
    }

    static func days(symbol: WatchSymbol, limit: Int) async throws -> [DayBar] {
        let remote: [RemoteDay] = try await get("quote/\(symbol.code)/days?limit=\(limit)")
        let bars = remote.filter { $0.close > 0 }.map { bar in
            DayBar(date: bar.date, open: bar.open, close: bar.close,
                   high: bar.high, low: bar.low, volume: bar.volume)
        }
        guard !bars.isEmpty else { throw GatewayError.badResponse }
        return bars
    }

    /// 阶段 3：服务端信号时间线。调用失败时上层继续使用本地 JSON 缓存。
    static func signals(limit: Int = 200) async throws -> [SignalEvent] {
        try await getData("signals?limit=\(max(1, min(limit, 1000)))")
    }

    /// §F.3：个股新闻（`hours` 为响应时间窗；落库的始终是全量）。
    static func news(code: String, limit: Int = 10, hours: Int = 72) async throws -> [GatewayNewsItem] {
        try await getData("news/\(code)?limit=\(max(1, min(limit, 50)))&hours=\(max(1, min(hours, 720)))")
    }

    /// §F.4：机构研报（近 `days` 天，按发布日倒序）。
    static func reports(code: String, limit: Int = 10, days: Int = 90) async throws -> [GatewayResearchReport] {
        try await getData("reports/\(code)?limit=\(max(1, min(limit, 50)))&days=\(max(1, min(days, 1095)))")
    }

    /// §F.4：所属概念 / 行业板块（板块指数 + 涨跌幅）。
    static func sector(code: String) async throws -> [GatewaySectorBoard] {
        try await getData("sector/\(code)")
    }

    /// B.4：逐笔成交明细（东财 push2delay details，按需拉取）。
    static func ticks(code: String, limit: Int = 2000) async throws -> [GatewayTick] {
        try await getData("quote/\(code)/ticks?limit=\(max(50, min(limit, 5000)))")
    }

    /// A 股池智能推荐：最近一份（含 T+5 回测统计）。
    static func picks() async throws -> GatewayPicksDocument {
        try await getData("picks")
    }

    /// 每日数据归档原始 JSON（存档 / 回放共用）。
    static func dayExportData(date: String?) async throws -> Data {
        let path = date.map { "export/day?date=\($0)" } ?? "export/day"
        let request = try await dataRequest(path: path, method: "GET")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        return data
    }

    /// 回放用：归档解码为结构化文档。
    static func dayExport(date: String?) async throws -> GatewayDayExport {
        try JSONDecoder().decode(GatewayDayExport.self, from: await dayExportData(date: date))
    }

    /// 重新生成当日推荐；`ai` 透传本机 AI 配置做精排（密钥只在请求内使用）。
    static func runPicks(ai: AiRankConfig?) async throws -> GatewayPicksDocument {
        struct Body: Encodable {
            var ai: AiRankConfig?
        }
        var request = try await dataRequest(path: "picks/run", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(Body(ai: ai))
        request.timeoutInterval = 240
        let (data, response) = try await picksSession.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        return try JSONDecoder().decode(GatewayPicksDocument.self, from: data)
    }

    /// POST /picks/run 的 AI 精排配置（snake_case 对齐服务端 serde）。
    struct AiRankConfig: Codable, Equatable {
        var provider: String
        var baseURL: String
        var apiKey: String
        var model: String

        enum CodingKeys: String, CodingKey {
            case provider, model
            case baseURL = "base_url"
            case apiKey = "api_key"
        }
    }

    /// 本地先写、服务端后写；网络错误不会影响盯盘主流程。
    static func putSignal(_ event: SignalEvent) async throws {
        try await sendData("signals", method: "POST", body: event)
    }

    static func deleteSignals() async throws {
        let request = try await dataRequest(path: "signals", method: "DELETE")
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 204 else { throw GatewayError.badResponse }
    }

    static func remoteState() async throws -> GatewayRemoteState {
        struct SettingsDocument: Decodable { var values: GatewaySettingsSnapshot }
        async let positions: [GatewayPosition] = getData("positions")
        async let watchlist: [GatewayWatchItem] = getData("watchlist")
        async let settings: SettingsDocument = getData("settings")
        return try await GatewayRemoteState(
            positions: positions,
            watchlist: watchlist,
            settings: settings.values
        )
    }

    static func putSettings(_ settings: GatewaySettingsSnapshot) async throws {
        try await sendData("settings", method: "PUT", body: settings)
    }

    static func putPosition(code: String, position: PositionNote) async throws {
        struct Body: Encodable {
            var cost: Double
            var shares: Double
            var stopLoss: Double
            var takeProfit: Double = 0
            var positionPct: Double
        }
        try await sendData(
            "positions/\(code)", method: "PUT",
            body: Body(cost: position.cost, shares: position.shares,
                       stopLoss: position.stopLoss, takeProfit: position.takeProfit,
                       positionPct: position.positionPct)
        )
    }

    static func putWatchSymbol(_ symbol: WatchSymbol) async throws {
        struct Body: Encodable {
            var code: String
            var name: String
            var market: String
            var pinned: Bool
            var group: String
        }
        try await sendData(
            "watchlist", method: "POST",
            body: Body(code: symbol.code, name: symbol.name, market: symbol.marketPrefix,
                       pinned: symbol.pinned, group: symbol.group)
        )
    }

    static func deleteWatchSymbol(code: String) async throws {
        let request = try await dataRequest(path: "watchlist/\(code)", method: "DELETE")
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              response.statusCode == 204 else { throw GatewayError.badResponse }
    }

    private static func get<T: Decodable>(_ path: String) async throws -> T {
        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard enabled else { throw GatewayError.disabled }
        guard let url = URL(string: base),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              url.host != nil else { throw GatewayError.invalidURL }
        guard await circuit.canTry(base) else { throw GatewayError.unavailable }
        guard let endpoint = URL(string: base.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            + "/api/v1/" + path) else { throw GatewayError.invalidURL }
        do {
            let (data, response) = try await session.data(from: endpoint)
            guard let response = response as? HTTPURLResponse,
                  (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
            let result = try JSONDecoder().decode(T.self, from: data)
            await circuit.succeeded(base)
            return result
        } catch {
            await circuit.failed(base)
            throw error
        }
    }

    private static func getData<T: Decodable>(_ path: String) async throws -> T {
        let request = try await dataRequest(path: path, method: "GET")
        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(T.self, from: data)
    }

    private static func sendData<T: Encodable>(_ path: String, method: String, body: T) async throws {
        var request = try await dataRequest(path: path, method: method)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        request.httpBody = try encoder.encode(body)
        let (_, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse,
              (200..<300).contains(response.statusCode) else { throw GatewayError.badResponse }
    }

    private static func dataRequest(path: String, method: String) async throws -> URLRequest {
        let enabled = await AppSettings.shared.marketGatewayEnabled
        let base = await AppSettings.shared.marketServerURL
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard enabled else { throw GatewayError.disabled }
        guard let endpoint = URL(string: base + "/api/v1/" + path),
              ["http", "https"].contains(endpoint.scheme?.lowercased() ?? ""),
              endpoint.host != nil else { throw GatewayError.invalidURL }
        var request = URLRequest(url: endpoint)
        request.httpMethod = method
        return request
    }

    private static func sourceName(_ source: String) -> String {
        switch source {
        case "sina": return "新浪"
        case "tencent": return "腾讯"
        case "eastmoney": return "东财"
        default: return source
        }
    }

    private static func chinaTime(_ timestamp: String, format: String) -> String {
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let date = parser.date(from: timestamp) ?? {
            parser.formatOptions = [.withInternetDateTime]
            return parser.date(from: timestamp)
        }()
        guard let date else { return "--" }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = format
        return formatter.string(from: date)
    }
}

private actor GatewayCircuit {
    private var address = ""
    private var retryAfter = Date.distantPast

    func canTry(_ base: String) -> Bool {
        if address != base {
            address = base
            retryAfter = .distantPast
        }
        return Date() >= retryAfter
    }

    func failed(_ base: String) {
        guard address == base else { return }
        retryAfter = Date().addingTimeInterval(10)
    }

    func succeeded(_ base: String) {
        guard address == base else { return }
        retryAfter = .distantPast
    }
}
