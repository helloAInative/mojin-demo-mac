import Foundation
import Security

struct AIConfig: Codable, Equatable {
    var enabled: Bool = false
    var autoAnalyze: Bool = false
    /// OpenAI 兼容。阿里 Token Plan 请用专属域名，勿用 dashscope 通用地址。
    var baseURL: String = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
    var model: String = "qwen3.7-plus"
    /// 最近一次调用成功的模型，切换失败时可回退提示
    var lastGoodModel: String = ""
    /// 自动分析最短间隔（秒）
    var intervalSec: Int = 120
    var maxTokens: Int = 600
    /// LLM provider id（路由到 LLMProvider）。默认 "openai"（=OpenAI 兼容协议）。
    /// 旧配置解码时缺省为 "openai"，保持向后兼容。
    var providerId: String = "openai"

    enum CodingKeys: String, CodingKey {
        case enabled, autoAnalyze, baseURL, model, lastGoodModel
        case intervalSec, maxTokens, providerId
    }

    init(enabled: Bool = false, autoAnalyze: Bool = false,
         baseURL: String = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
         model: String = "qwen3.7-plus",
         lastGoodModel: String = "",
         intervalSec: Int = 120, maxTokens: Int = 600,
         providerId: String = "openai") {
        self.enabled = enabled
        self.autoAnalyze = autoAnalyze
        self.baseURL = baseURL
        self.model = model
        self.lastGoodModel = lastGoodModel
        self.intervalSec = intervalSec
        self.maxTokens = maxTokens
        self.providerId = providerId
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.enabled        = (try? c.decode(Bool.self,   forKey: .enabled))        ?? false
        self.autoAnalyze    = (try? c.decode(Bool.self,   forKey: .autoAnalyze))    ?? false
        self.baseURL        = (try? c.decode(String.self, forKey: .baseURL))        ?? "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
        self.model          = (try? c.decode(String.self, forKey: .model))          ?? "qwen3.7-plus"
        self.lastGoodModel  = (try? c.decode(String.self, forKey: .lastGoodModel))  ?? ""
        self.intervalSec    = (try? c.decode(Int.self,    forKey: .intervalSec))    ?? 120
        self.maxTokens      = (try? c.decode(Int.self,    forKey: .maxTokens))      ?? 600
        self.providerId     = (try? c.decode(String.self, forKey: .providerId))     ?? "openai"
    }
}

/// 阿里云 Token Plan 官方列出的模型（个人版 ∪ 团队版）。
struct TokenPlanModel: Identifiable, Hashable {
    var id: String { modelID }
    let brand: String
    let modelID: String
    let capability: String
    /// 可用于 chat/completions 实时分析
    let chat: Bool
    var menuLabel: String {
        chat ? modelID : "\(modelID)  · \(capability)"
    }
}

enum TokenPlanCatalog {
    static let all: [TokenPlanModel] = [
        .init(brand: "千问", modelID: "qwen3.8-max", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen3.8-flash", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen3.7-max", capability: "推理 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen3.7-plus", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen3.6-plus", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen3.6-flash", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "千问", modelID: "qwen-image-3.0-pro", capability: "图片生成", chat: false),
        .init(brand: "千问", modelID: "qwen-image-2.0-pro", capability: "图片生成", chat: false),
        .init(brand: "千问", modelID: "qwen-image-2.0", capability: "图片生成", chat: false),
        .init(brand: "千问", modelID: "qwen-audio-3.0-tts-plus", capability: "语音合成", chat: false),
        .init(brand: "千问", modelID: "qwen-audio-3.0-realtime-plus", capability: "实时语音", chat: false),
        .init(brand: "千问", modelID: "qwen-audio-3.0-asr-flash", capability: "语音识别", chat: false),
        .init(brand: "万相", modelID: "wan2.7-image-pro", capability: "图片生成", chat: false),
        .init(brand: "万相", modelID: "wan2.7-image", capability: "图片生成", chat: false),
        .init(brand: "DeepSeek", modelID: "deepseek-v4.1-flash", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "DeepSeek", modelID: "deepseek-v4-pro", capability: "推理 / 文本", chat: true),
        .init(brand: "DeepSeek", modelID: "deepseek-v4-pro-0813", capability: "推理 / 文本", chat: true),
        .init(brand: "DeepSeek", modelID: "deepseek-v4-flash", capability: "推理 / 文本", chat: true),
        .init(brand: "DeepSeek", modelID: "deepseek-v4-flash-0731", capability: "推理 / 文本", chat: true),
        .init(brand: "DeepSeek", modelID: "deepseek-v3.2", capability: "推理 / 文本", chat: true),
        .init(brand: "月之暗面", modelID: "kimi-k2.7-code", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "月之暗面", modelID: "kimi-k2.6", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "月之暗面", modelID: "kimi-k2.5", capability: "推理 / 视觉 / 文本", chat: true),
        .init(brand: "智谱 AI", modelID: "glm-5.3", capability: "推理 / 文本", chat: true),
        .init(brand: "智谱 AI", modelID: "glm-5.2", capability: "推理 / 文本", chat: true),
        .init(brand: "智谱 AI", modelID: "glm-5.1", capability: "推理 / 文本", chat: true),
        .init(brand: "智谱 AI", modelID: "glm-5", capability: "推理 / 文本", chat: true),
        .init(brand: "MiniMax", modelID: "MiniMax-M2.5", capability: "推理 / 文本", chat: true),
        .init(brand: "MiniMax", modelID: "MiniMax-M3", capability: "推理 / 文本", chat: true),
        .init(brand: "HappyHorse", modelID: "happyhorse-1.1-i2v", capability: "视频生成", chat: false),
        .init(brand: "HappyHorse", modelID: "happyhorse-1.1-t2v", capability: "视频生成", chat: false),
        .init(brand: "HappyHorse", modelID: "happyhorse-1.1-r2v", capability: "视频生成", chat: false),
    ]

    static var chat: [TokenPlanModel] { all.filter(\.chat) }

    static var grouped: [(brand: String, models: [TokenPlanModel])] {
        var order: [String] = []
        var map: [String: [TokenPlanModel]] = [:]
        for m in all {
            if map[m.brand] == nil { order.append(m.brand) }
            map[m.brand, default: []].append(m)
        }
        return order.map { (brand: $0, models: map[$0] ?? []) }
    }

    static func contains(_ id: String) -> Bool {
        all.contains { $0.modelID == id }
    }

    static func isChat(_ id: String) -> Bool {
        all.first(where: { $0.modelID == id })?.chat ?? true
    }
}

enum KeychainStore {
    private static let service = "com.zhangpengxuan.mojinprince.ai"
    private static let account = "apiKey"

    static func save(_ value: String) {
        let data = Data(value.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account
        ]
        SecItemDelete(query as CFDictionary)
        guard !value.isEmpty else { return }
        var add = query
        add[kSecValueData as String] = data
        add[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlocked
        SecItemAdd(add as CFDictionary, nil)
    }

    static func load() -> String {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data,
              let s = String(data: data, encoding: .utf8) else { return "" }
        return s
    }
}

enum AIService {
    enum AIError: LocalizedError {
        case notConfigured
        case badURL
        case http(Int, String)
        case empty

        var errorDescription: String? {
            switch self {
            case .notConfigured: return "请先配置 API Token"
            case .badURL: return "Base URL 无效"
            case .http(let c, let b): return "HTTP \(c): \(b.prefix(120))"
            case .empty: return "模型返回为空"
            }
        }
    }

    static func chat(config: AIConfig, apiKey: String, system: String, user: String) async throws -> String {
        let key = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else { throw AIError.notConfigured }
        var base = config.baseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        if base.hasSuffix("/") { base.removeLast() }
        guard let url = URL(string: base + "/chat/completions") else { throw AIError.badURL }

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.timeoutInterval = 60
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")

        let body: [String: Any] = [
            "model": config.model,
            "temperature": 0.4,
            "max_tokens": config.maxTokens,
            "enable_thinking": false,
            "messages": [
                ["role": "system", "content": system],
                ["role": "user", "content": user]
            ]
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, resp) = try await URLSession.shared.data(for: req)
        let code = (resp as? HTTPURLResponse)?.statusCode ?? 0
        if !(200...299).contains(code) {
            let raw = String(data: data, encoding: .utf8) ?? ""
            throw AIError.http(code, raw)
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let choices = json?["choices"] as? [[String: Any]]
        let msg = choices?.first?["message"] as? [String: Any]
        if let content = msg?["content"] as? String, !content.isEmpty {
            return content.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        if let reasoning = msg?["reasoning_content"] as? String, !reasoning.isEmpty {
            return reasoning.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        if let text = choices?.first?["text"] as? String, !text.isEmpty {
            return text.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        throw AIError.empty
    }

    static let systemPrompt = """
    你是A股短线盯盘助手「摸金小王子」。根据用户提供的实时行情与技术指标，用中文给出简洁可执行的观察结论。

    【必须】严格返回一个 JSON 对象，不要任何额外说明，不要使用 Markdown 代码块包装：
    {
      "summary": "一句话总判断（偏多/偏空/震荡，<=30 字）",
      "verdict": "bull" | "bear" | "range",
      "levels": {
        "base":      数字或 null,   // AI 认为今日合理的基准价（昨收附近的强弱分水岭）
        "support":   数字或 null,   // AI 识别的短线支撑位
        "resistance":数字或 null,   // AI 识别的短线阻力位
        "stop":      数字或 null    // AI 给出的失效/止损位（可与支撑相同）
      },
      "events": [                                  // 今日已发生 / 即将可能发生的盘中事件（最多 5 条）
        {
          "kind":   "breakout" | "breakdown" | "volSpike" | "fakeout" | "reversal",
          "dir":    "up" | "down",
          "minuteOffset": 数字（距 9:30 的分钟数，0..240），
          "price":   数字,
          "note":    "短说明（<=12 字，可省略）"
        }
      ],
      "volumeWarnRatio": 数字或 null,              // 量比警戒倍数；超过即视为放量异动
      "keyPoints": ["要点1", "要点2", "要点3"],   // 3~5 条短句
      "risk": "一段风险与无效条件的话（<=40 字）"
    }

    要求：
    1. 价位字段务必与上下文中已提供的支撑/阻力/基准/止损保持一致；若证据不充分则填 null，不要编造数字。
    2. events 必须真实反映当前行情：未确认的不要写；若是预测性事件，dir 反向并 note 注明「若」。
    3. summary 与 keyPoints 用中文短句，不要 emoji；控制在 220 字以内。
    4. 不要做收益承诺；不要解释 JSON 格式。
    """
}

/// 从模型返回文本里提取 JSON 子串。优先匹配首个完整花括号对象。
enum AIJSONExtractor {
    static func extractObject(from text: String) -> [String: Any]? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let start = trimmed.firstIndex(of: "{") else { return nil }
        var depth = 0
        var inString = false
        var escape = false
        var idx = start
        while idx < trimmed.endIndex {
            let ch = trimmed[idx]
            if escape { escape = false; idx = trimmed.index(after: idx); continue }
            if ch == "\\" { escape = true; idx = trimmed.index(after: idx); continue }
            if ch == "\"" { inString.toggle() }
            else if !inString {
                if ch == "{" { depth += 1 }
                else if ch == "}" {
                    depth -= 1
                    if depth == 0 {
                        let slice = String(trimmed[start...idx])
                        if let data = slice.data(using: .utf8),
                           let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
                            return obj
                        }
                        return nil
                    }
                }
            }
            idx = trimmed.index(after: idx)
        }
        return nil
    }
}
