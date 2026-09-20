//! 默认 system prompt 与常用 prompt 模板。
//!
//! 集中放这里，方便统一调整语气 / 长度上限 / JSON 输出格式。
//!
//! 与 Swift 端 `AIService.swift` 历史默认 prompt 保持一致；
//! 后续接入新版模型只需改本文件。

/// 默认盯盘助手 prompt：要求输出 JSON 字段（summary / action / riskLevel）。
pub const SYSTEM_PROMPT: &str = "你是 A 股实时盯盘助手，必须根据用户给的价格、量能、消息回答。\
输出严格遵循以下 JSON schema，不要任何额外文字、不要 markdown 包裹：\
{\"summary\":\"<一句话结论>\",\"action\":\"<hold|buy|sell|watch>\",\"riskLevel\":\"<low|mid|high>\",\"reasons\":[\"<要点1>\",\"<要点2>\"]}";
