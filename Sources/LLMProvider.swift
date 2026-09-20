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
        let direct: any LLMProvider
        switch id {
        case "openai", "remote", "":
            direct = OpenAICompatibleProvider(config: config, apiKey: apiKey)
        case "ollama":
            direct = OllamaProvider(config: config, apiKey: apiKey)
        default:
            // 未知 id 兜底走 OpenAI 兼容；避免把不存在的 id 当作"无 provider"
            direct = OpenAICompatibleProvider(config: config, apiKey: apiKey)
        }
        return GatewayFirstProvider(config: config, apiKey: apiKey, direct: direct)
    }
}

/// 优先把 AI 调用交给 Rust 网关；网关不可用时保留原来的 Swift 直连能力。
struct GatewayFirstProvider: LLMProvider {
    let id: String = "gateway"
    let config: AIConfig
    let apiKey: String
    let direct: any LLMProvider

    func chat(system: String, user: String) async throws -> String {
        do {
            return try await GatewayAIClient.chat(
                config: config, apiKey: apiKey, system: system, user: user
            )
        } catch {
            return try await direct.chat(system: system, user: user)
        }
    }
}

enum GatewayAIClient {
    private static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 70
        configuration.timeoutIntervalForResource = 75
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        return URLSession(configuration: configuration)
    }()

    private struct RequestBody: Encodable {
        let provider: String
        let baseURL: String
        let apiKey: String
        let model: String
        let system: String
        let user: String
        let maxTokens: Int
        let temperature: Double

        enum CodingKeys: String, CodingKey {
            case provider, model, system, user, temperature
            case baseURL = "base_url"
            case apiKey = "api_key"
            case maxTokens = "max_tokens"
        }
    }

    private struct ResponseBody: Decodable {
        let content: String
    }

    static func chat(config: AIConfig, apiKey: String, system: String, user: String) async throws -> String {
        let enabled = await AppSettings.shared.marketGatewayEnabled
        let address = await AppSettings.shared.marketServerURL
        guard enabled else { throw LLMError.notConfigured }
        let base = address.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/api/v1/ai/chat"),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              url.host != nil else { throw LLMError.badURL }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 70
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(RequestBody(
            provider: config.providerId,
            baseURL: config.baseURL,
            apiKey: apiKey,
            model: config.model,
            system: system,
            user: user,
            maxTokens: config.maxTokens,
            temperature: 0.4
        ))
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            let raw = String(data: data, encoding: .utf8) ?? ""
            throw LLMError.http(status, raw)
        }
        let decoded = try JSONDecoder().decode(ResponseBody.self, from: data)
        let content = decoded.content.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty else { throw LLMError.empty }
        return content
    }
}

/// Rust 网关关闭时的 Ollama 直连回退。
struct OllamaProvider: LLMProvider {
    let id: String = "ollama"
    let config: AIConfig
    let apiKey: String

    private struct Message: Codable {
        let role: String
        let content: String
    }

    private struct RequestBody: Encodable {
        let model: String
        let stream: Bool
        let messages: [Message]
        let options: Options
    }

    private struct Options: Encodable {
        let temperature: Double
        let numPredict: Int

        enum CodingKeys: String, CodingKey {
            case temperature
            case numPredict = "num_predict"
        }
    }

    private struct ResponseBody: Decodable {
        let message: Message?
    }

    func chat(system: String, user: String) async throws -> String {
        let base = config.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/api/chat"),
              ["http", "https"].contains(url.scheme?.lowercased() ?? ""),
              url.host != nil else { throw LLMError.badURL }
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.timeoutInterval = 70
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if !apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        }
        request.httpBody = try JSONEncoder().encode(RequestBody(
            model: config.model,
            stream: false,
            messages: [
                Message(role: "system", content: system),
                Message(role: "user", content: user)
            ],
            options: Options(temperature: 0.4, numPredict: config.maxTokens)
        ))
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            throw LLMError.http(status, String(data: data, encoding: .utf8) ?? "")
        }
        let decoded = try JSONDecoder().decode(ResponseBody.self, from: data)
        let content = decoded.message?.content.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard !content.isEmpty else { throw LLMError.empty }
        return content
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
