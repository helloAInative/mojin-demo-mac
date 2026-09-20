import Foundation
import UserNotifications

/// 通知治理：冷却、聚合、免打扰
@MainActor
final class NotifyGovernor {
    static let shared = NotifyGovernor()

    struct Config: Codable, Equatable {
        var cooldownSec: Int = 300          // 同类通知冷却
        var aggregateSec: Int = 90          // 聚合窗口
        var dndEnabled: Bool = false
        var dndStartMin: Int = 22 * 60      // 22:00
        var dndEndMin: Int = 8 * 60         // 08:00 次日
        var maxPerHour: Int = 12
        /// 关键价位一级预警阈值（%），距离 ≤ 此值触发一级闪 + 一级通知
        var levelLightPct: Double = 0.30
        /// 关键价位二级预警阈值（%），距离 ≤ 此值触发二级闪 + 二级通知
        /// 必须严格小于 levelLightPct，否则 classify 会全归 deep。
        var levelDeepPct: Double = 0.15
        /// 持仓成本线预警阈值（%）：现价相对持仓成本距离 ≤ 此值时绘制/闪烁成本线 + 触发通知。
        /// 默认 1.0（±1%）。
        var costLightPct: Double = 1.0

        enum CodingKeys: String, CodingKey {
            case cooldownSec, aggregateSec, dndEnabled, dndStartMin, dndEndMin
            case maxPerHour, levelLightPct, levelDeepPct, costLightPct
        }

        init(cooldownSec: Int = 300, aggregateSec: Int = 90,
             dndEnabled: Bool = false,
             dndStartMin: Int = 22 * 60, dndEndMin: Int = 8 * 60,
             maxPerHour: Int = 12,
             levelLightPct: Double = 0.30, levelDeepPct: Double = 0.15,
             costLightPct: Double = 1.0) {
            self.cooldownSec = cooldownSec
            self.aggregateSec = aggregateSec
            self.dndEnabled = dndEnabled
            self.dndStartMin = dndStartMin
            self.dndEndMin = dndEndMin
            self.maxPerHour = maxPerHour
            self.levelLightPct = levelLightPct
            self.levelDeepPct = levelDeepPct
            self.costLightPct = costLightPct
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            self.cooldownSec   = (try? c.decode(Int.self,    forKey: .cooldownSec))   ?? 300
            self.aggregateSec  = (try? c.decode(Int.self,    forKey: .aggregateSec))  ?? 90
            self.dndEnabled    = (try? c.decode(Bool.self,   forKey: .dndEnabled))    ?? false
            self.dndStartMin   = (try? c.decode(Int.self,    forKey: .dndStartMin))   ?? 22 * 60
            self.dndEndMin     = (try? c.decode(Int.self,    forKey: .dndEndMin))     ?? 8 * 60
            self.maxPerHour    = (try? c.decode(Int.self,    forKey: .maxPerHour))    ?? 12
            self.levelLightPct = (try? c.decode(Double.self, forKey: .levelLightPct)) ?? 0.30
            self.levelDeepPct  = (try? c.decode(Double.self, forKey: .levelDeepPct))  ?? 0.15
            self.costLightPct  = (try? c.decode(Double.self, forKey: .costLightPct))  ?? 1.0
        }
    }

    var config = Config()
    private var lastFire: [String: Date] = [:]
    private var pending: [(title: String, body: String, id: String, code: String, userInfo: [String: String], at: Date)] = []
    private var hourBucket: [Date] = []
    private var flushTask: Task<Void, Never>?
    private var lastBlockedReason: String = ""
    /// 点击通知后由 UI 消费
    static var pendingOpenCode: String?
    static var pendingAction: String? // open | reanalyze | diary | stratNote

    struct QuotaSnapshot: Equatable {
        var inDND: Bool
        var hourUsed: Int
        var hourMax: Int
        var hourLeft: Int
        var pendingCount: Int
        var aggregateSec: Int
        var cooldownSec: Int
        var soonestCooldownLeft: Int
        var lastBlocked: String
        var line: String
    }

    func configure(_ c: Config) { config = c }

    func quotaSnapshot() -> QuotaSnapshot {
        let now = Date()
        pruneHour(now)
        let used = hourBucket.count
        let left = max(0, config.maxPerHour - used)
        let dnd = inDND()
        var soonest = 0
        for (_, t) in lastFire {
            let remain = Int(Double(config.cooldownSec) - now.timeIntervalSince(t))
            if remain > soonest { soonest = remain }
        }
        var parts: [String] = []
        if dnd {
            parts.append("免打扰中")
        } else {
            parts.append("本时\(left)/\(config.maxPerHour)")
        }
        if pending.count > 0 {
            parts.append("聚合中\(pending.count)条/\(config.aggregateSec)s")
        }
        if soonest > 0 {
            parts.append("冷却剩\(soonest)s")
        }
        if !lastBlockedReason.isEmpty {
            parts.append("拦:\(lastBlockedReason)")
        }
        return QuotaSnapshot(
            inDND: dnd,
            hourUsed: used,
            hourMax: config.maxPerHour,
            hourLeft: left,
            pendingCount: pending.count,
            aggregateSec: config.aggregateSec,
            cooldownSec: config.cooldownSec,
            soonestCooldownLeft: soonest,
            lastBlocked: lastBlockedReason,
            line: parts.joined(separator: " · ")
        )
    }

    @discardableResult
    func enqueue(title: String, body: String, id: String, code: String = "",
                 userInfo: [String: String] = [:]) -> Bool {
        if inDND() {
            lastBlockedReason = "免打扰"
            return false
        }

        let key = id.components(separatedBy: "-").first ?? id
        let now = Date()
        if let last = lastFire[key], now.timeIntervalSince(last) < Double(config.cooldownSec) {
            let left = Int(Double(config.cooldownSec) - now.timeIntervalSince(last))
            lastBlockedReason = "冷却\(left)s"
            return false
        }

        pruneHour(now)
        if hourBucket.count >= config.maxPerHour {
            lastBlockedReason = "小时额度满"
            return false
        }

        lastBlockedReason = ""
        pending.append((title, body, id, code, userInfo, now))
        scheduleFlush()
        return true
    }

    /// 与 enqueue 共用冷静/额度判断，不真正入队
    func canFire(id: String) -> Bool {
        if inDND() { return false }
        let key = id.components(separatedBy: "-").first ?? id
        let now = Date()
        if let last = lastFire[key], now.timeIntervalSince(last) < Double(config.cooldownSec) {
            return false
        }
        pruneHour(now)
        return hourBucket.count < config.maxPerHour
    }

    func flushNow() {
        flushTask?.cancel()
        flushTask = nil
        deliverPending()
    }

    private func scheduleFlush() {
        flushTask?.cancel()
        flushTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(max(self?.config.aggregateSec ?? 1, 1)) * 1_000_000_000)
            await MainActor.run { self?.deliverPending() }
        }
    }

    private func deliverPending() {
        guard !pending.isEmpty else { return }
        let batch = pending
        pending.removeAll()
        let now = Date()

        if batch.count == 1 {
            let one = batch[0]
            let key = one.id.components(separatedBy: "-").first ?? one.id
            lastFire[key] = now
            hourBucket.append(now)
            AlertService.deliver(title: one.title, body: one.body,
                                 id: one.id, code: one.code,
                                 userInfo: one.userInfo)
            return
        }

        for item in batch {
            let key = item.id.components(separatedBy: "-").first ?? item.id
            lastFire[key] = now
        }
        hourBucket.append(now)
        let lines = batch.prefix(8).map { item -> String in
            let tag = item.code.isEmpty ? "" : "[\(item.code)] "
            return tag + item.body
        }
        let tapCode = batch.first(where: { !$0.code.isEmpty })?.code ?? ""
        let tapInfo = batch.first(where: { !$0.userInfo.isEmpty })?.userInfo ?? [:]
        AlertService.deliver(
            title: "摸金小王子 · \(batch.count) 条提醒",
            body: lines.joined(separator: "\n"),
            id: "agg-\(Int(now.timeIntervalSince1970))",
            code: tapCode,
            userInfo: tapInfo
        )
    }

    private func inDND() -> Bool {
        guard config.dndEnabled else { return false }
        let m = TradingSession.shanghaiMinutes()
        let s = config.dndStartMin
        let e = config.dndEndMin
        if s == e { return false }
        if s < e { return m >= s && m < e }
        // 跨午夜
        return m >= s || m < e
    }

    private func pruneHour(_ now: Date) {
        hourBucket = hourBucket.filter { now.timeIntervalSince($0) < 3600 }
    }
}

enum AlertService {
    static func requestPermission() {
        registerCategories()
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    static func notify(title: String, body: String, id: String, code: String = "",
                      userInfo: [String: String] = [:]) {
        Task { @MainActor in
            _ = NotifyGovernor.shared.enqueue(
                title: title, body: body, id: id, code: code, userInfo: userInfo
            )
        }
    }

    static func deliver(title: String, body: String, id: String, code: String = "",
                        userInfo: [String: String] = [:]) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.categoryIdentifier = "MOJIN_QUOTE"
        var info = userInfo
        if !code.isEmpty { info["code"] = code }
        content.userInfo = info
        let req = UNNotificationRequest(
            identifier: id + "-\(Int(Date().timeIntervalSince1970))",
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(req, withCompletionHandler: nil)
    }

    static func registerCategories() {
        let open = UNNotificationAction(identifier: "OPEN_SYMBOL", title: "查看该股", options: [.foreground])
        let re = UNNotificationAction(identifier: "REANALYZE", title: "再分析", options: [.foreground])
        let diary = UNNotificationAction(identifier: "DIARY", title: "记复盘", options: [.foreground])
        let note = UNNotificationAction(identifier: "STRAT_NOTE", title: "策略备注", options: [.foreground])
        let order = UNNotificationAction(identifier: "SUGGEST_ORDER", title: "建议委托", options: [.foreground])
        // 价位预警专用：跳到价位 + 草稿委托
        let level = UNNotificationAction(identifier: "OPEN_LEVEL", title: "跳到价位+草稿委托", options: [.foreground])
        let cat = UNNotificationCategory(
            identifier: "MOJIN_QUOTE",
            actions: [open, level, order, re, diary, note],
            intentIdentifiers: []
        )
        UNUserNotificationCenter.current().setNotificationCategories([cat])
    }
}

/// 简单全局限流，避免打爆行情接口
actor RateLimiter {
    static let shared = RateLimiter()
    private var last: [String: Date] = [:]
    private let minGap: TimeInterval = 0.4

    func wait(bucket: String = "default") async {
        let now = Date()
        if let t = last[bucket] {
            let left = minGap - now.timeIntervalSince(t)
            if left > 0 {
                try? await Task.sleep(nanoseconds: UInt64(left * 1_000_000_000))
            }
        }
        last[bucket] = Date()
    }
}

enum CrashLog {
    private static var path: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        let dir = base.appendingPathComponent("MojinPrince/logs", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("app.log")
    }

    static func install() {
        NSSetUncaughtExceptionHandler { ex in
            let msg = "EXCEPTION \(ex.name.rawValue): \(ex.reason ?? "")\n\(ex.callStackSymbols.prefix(12).joined(separator: "\n"))"
            CrashLog.append(msg)
        }
        append("—— 启动 \(ISO8601DateFormatter().string(from: Date())) v\(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "?") ——")
    }

    static func append(_ line: String) {
        let stamp = DateFormatter.localizedString(from: Date(), dateStyle: .short, timeStyle: .medium)
        let text = "[\(stamp)] \(line)\n"
        let url = path
        if let data = text.data(using: .utf8) {
            if FileManager.default.fileExists(atPath: url.path) {
                if let handle = try? FileHandle(forWritingTo: url) {
                    defer { try? handle.close() }
                    try? handle.seekToEnd()
                    try? handle.write(contentsOf: data)
                }
            } else {
                try? data.write(to: url)
            }
        }
        // 裁剪 > 200KB
        if let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
           let size = attrs[.size] as? NSNumber, size.intValue > 200_000,
           let all = try? String(contentsOf: url, encoding: .utf8) {
            let kept = String(all.suffix(100_000))
            try? kept.write(to: url, atomically: true, encoding: .utf8)
        }
    }

    static func recent(maxChars: Int = 8000) -> String {
        guard let s = try? String(contentsOf: path, encoding: .utf8) else { return "暂无日志" }
        return String(s.suffix(maxChars))
    }

    static func clear() {
        try? FileManager.default.removeItem(at: path)
    }
}

enum AppChangelog {
    static let text = """
    ## 2.2
    - 委托条：到价/止损一键填、策略/AI 草稿、国盛成交回写持仓
    - 大盘：沪深300/北证50 开关、点指数迷你分时、同向角标
    - 自选：名称补全、批量导入、失效代码提示
    - 体检：大盘失败计入；个股/指数缓存年龄分列
    - 策略命中推建议委托；通知动作「建议委托」
    - 复盘工作台：日记+信号+委托拼成交易故事

    ## 2.1
    - 半自动委托条：生成国盛照抄文本（买/卖/价/量），一键复制，不报单
    - 盯盘顶部大盘：上证 / 深成 / 创业，约 15 秒刷新

    ## 2.0
    - 多标的真回测、信号筛选、AI 账本、弱网体检

    ## 1.9 – 1.2
    - 打磨 / 时间线 / AI / 策略 / 自选演进
    """
}
