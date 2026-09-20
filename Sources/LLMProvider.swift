import Foundation

/// LLM Provider 抽象。后续接入本地（Ollama / llama.cpp）只需新增实现，
/// 不必改动 MarketStore.analyzeWithAI 的调用路径。
///
/// 设计原则：
/// 1. 输入只有 system + user 两段；与具体协议（OpenAI / Claude / Ollama）解耦。
/// 2. 同步走 async throws；调用方用 await 即可。
/// 3. provider 由 providerId 字符串路由，由 ProviderRegistry 解析。
protocol LLMProvider: Sendable {
    /// provider 唯一标识（remote / openai / ollama-stub …）
    var id: String { get }
    /// 单次对话；返回完整文本（含 markdown / JSON 均可，原样回传）。
    func chat(system: String, user: String) async throws -> String
}

enum LLMError: LocalizedError {
    case notConfigured
    case badURL
    case http(Int, String)
    case empty
    case decode(String)
    var errorDescription: String? {
        switch self {
        case .notConfigured:  return "AI 未配置（缺 Key 或 URL）"
        case .badURL:         return "AI baseURL 不合法"
        case .http(let c, let body):
            if body.isEmpty { return "HTTP \(c)" }
            return "HTTP \(c): \(body.prefix(200))"
        case .empty:          return "AI 返回空内容"
        case .decode(let s):  return "AI 返回解析失败：\(s.prefix(200))"
        }
    }
}

/// 工厂：当前默认走 AIService.chat()（OpenAI-compatible），即 RemoteProvider。
@MainActor
enum ProviderRegistry {
    static func resolve(id: String, config: AIConfig, apiKey: String) -> LLMProvider {
        switch id {
        case "openai", "remote", "":
            return OpenAICompatibleProvider(config: config, apiKey: apiKey)
        default:
            // 未知 id 兜底走 OpenAI 兼容；避免把不存在的 id 当作"无 provider"
            return OpenAICompatibleProvider(config: config, apiKey: apiKey)
        }
    }
}

/// OpenAI / 兼容协议（OpenAI、阿里 Maas、火山、DeepSeek、月之暗面 等）。
/// 这是当前默认 provider，内部实际调用 AIService.chat() 保持向后兼容。
struct OpenAICompatibleProvider: LLMProvider {
    let id: String = "openai"
    let config: AIConfig
    let apiKey: String

    func chat(system: String, user: String) async throws -> String {
        // 复用既有 AIService.chat：保持现有鉴权/超时/重试/降级逻辑不动
        return try await AIService.chat(
            config: config, apiKey: apiKey,
            system: system, user: user
        )
    }
}