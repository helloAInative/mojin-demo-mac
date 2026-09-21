import Foundation
import Combine

/// AI 智能降级（ROI #7 / §A.3 配套）：连续失败后熔断短路自动分析。
///
/// 既有链路「网关 → 直连 → 规则摘要」不动；本组件只决定**这次要不要发起真请求**：
/// - 普通失败连败 ≥3 次 → 熔断（10 分钟起步，连败每多一次翻倍，封顶 60 分钟）
/// - 429 限流 → 30 分钟；401/403 鉴权、402 欠费 → 24 小时（修好配置后手动试探即恢复）
/// - 熔断期内**自动分析直接走规则摘要**（不再白等满超时），手动「立即分析」永远放行
///   （相当于半开试探，成功即清零恢复）
/// - 一次成功调用清空全部状态
@MainActor
final class AIDegradeGovernor: ObservableObject {
    enum ErrorKind: Equatable {
        /// 429：上游限流，长冷静
        case rateLimit
        /// 401 / 403：Key 失效，等用户修配置
        case auth
        /// 402：欠费
        case quota
        /// 超时 / 连接失败 / 网关不可达
        case network
        case other

        var label: String {
            switch self {
            case .rateLimit: return "限流(429)"
            case .auth: return "鉴权失败(401/403)"
            case .quota: return "欠费(402)"
            case .network: return "网络/超时"
            case .other: return "其他错误"
            }
        }
    }

    /// 普通错误连败多少次后进入熔断
    let streakThreshold = 3
    /// 普通熔断起步时长（分钟）；连败每多一次翻倍，封顶 maxCooldownMin
    let baseCooldownMin = 10
    let maxCooldownMin = 60
    /// 429 限流熔断时长（分钟）
    let rateLimitCooldownMin = 30
    /// 鉴权 / 欠费熔断时长（小时）：实质上是停掉自动分析，等用户修配置
    let authCooldownHours = 24.0

    @Published private(set) var failStreak = 0
    @Published private(set) var lastKind: ErrorKind?
    @Published private(set) var circuitUntil: Date?

    func inCircuit(now: Date = Date()) -> Bool {
        circuitUntil.map { now < $0 } ?? false
    }

    /// 熔断剩余时间描述（"8 分钟" / "1.2 小时"），不在熔断期返回 nil。
    func circuitHoldText(now: Date = Date()) -> String? {
        guard let until = circuitUntil, now < until else { return nil }
        let sec = Int(until.timeIntervalSince(now))
        if sec >= 3600 { return String(format: "%.1f 小时", Double(sec) / 3600) }
        if sec >= 60 { return "\(max(sec / 60, 1)) 分钟" }
        return "\(max(sec, 1)) 秒"
    }

    /// 自动分析是否短路（熔断期内直接规则摘要，不发网络请求）。
    /// 手动 force 调用不走这里——点「立即分析」即半开试探。
    func shouldShortCircuitAuto(now: Date = Date()) -> Bool {
        inCircuit(now: now)
    }

    func recordFailure(kind: ErrorKind, now: Date = Date()) {
        failStreak += 1
        lastKind = kind
        switch kind {
        case .auth, .quota:
            circuitUntil = now.addingTimeInterval(authCooldownHours * 3600)
        case .rateLimit:
            circuitUntil = now.addingTimeInterval(Double(rateLimitCooldownMin * 60))
        case .network, .other:
            guard failStreak >= streakThreshold else { return }
            // 10 / 20 / 40 / 60 分钟：指数翻倍，封顶 maxCooldownMin
            let exp = min(Double(failStreak - streakThreshold), 3.0)
            let cooldown = min(
                Double(baseCooldownMin) * pow(2, exp) * 60,
                Double(maxCooldownMin * 60)
            )
            circuitUntil = now.addingTimeInterval(cooldown)
        }
    }

    func recordSuccess() {
        failStreak = 0
        lastKind = nil
        circuitUntil = nil
    }

    /// 从调用链错误分类（网关与直连都抛 LLMError；超时是 URLError）。
    nonisolated static func classify(_ error: Error) -> ErrorKind {
        if let llm = error as? LLMError, case LLMError.http(let status, _) = llm {
            switch status {
            case 429: return .rateLimit
            case 401, 403: return .auth
            case 402: return .quota
            default: return .other
            }
        }
        if error is URLError { return .network }
        return .other
    }
}
