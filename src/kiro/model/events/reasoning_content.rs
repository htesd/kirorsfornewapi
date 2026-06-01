//! 推理内容事件
//!
//! 处理 reasoningContentEvent 类型的事件 —— Kiro 上游在开启 thinking 时
//! 通过这个**独立事件通道**流式下发模型的推理过程（不混在 assistantResponseEvent
//! 的正文里，也不占用正文的 `<thinking>` 标签）。
//!
//! payload 形如 `{"text": " Let me reconsider..."}`，逐片下发。
//! 我们把它累积/转发为 Anthropic 的 `thinking` 内容块回给客户端。

use serde::Deserialize;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// 推理内容事件
///
/// 包含模型推理过程的流式片段。Kiro 在 thinking 流的**最后一帧**会单独下发
/// `{"signature": "..."}`（无 text），这是 Anthropic 原生 thinking 签名（protobuf
/// 编码，含模型代号/通道信息）。反代需把它透传到 Anthropic thinking 块的 `signature`
/// 字段，否则签名为空会被检测平台判为"签名校验失败"。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningContentEvent {
    /// 推理内容片段
    #[serde(default)]
    pub text: String,

    /// thinking 签名（仅最后一帧携带，protobuf base64）。透传给 Anthropic thinking 块。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// 捕获其他未使用的字段，确保反序列化兼容性
    #[serde(flatten)]
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    extra: serde_json::Value,
}

impl EventPayload for ReasoningContentEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        frame.payload_as_json()
    }
}

impl Default for ReasoningContentEvent {
    fn default() -> Self {
        Self {
            text: String::new(),
            signature: None,
            extra: serde_json::Value::Null,
        }
    }
}

impl std::fmt::Display for ReasoningContentEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_field() {
        let json = r#"{"text":" Let me think"}"#;
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.text, " Let me think");
    }

    #[test]
    fn tolerates_extra_fields() {
        let json = r#"{"text":"x","someOtherField":42}"#;
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.text, "x");
    }

    #[test]
    fn defaults_empty_when_text_missing() {
        let json = r#"{"someOtherField":1}"#;
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.text, "");
    }

    #[test]
    fn parses_signature_frame() {
        // thinking 流最后一帧：只有 signature，没有 text
        let json = r#"{"signature":"EtMBCmMIDhABGAIqQBhQ"}"#;
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.text, "");
        assert_eq!(event.signature.as_deref(), Some("EtMBCmMIDhABGAIqQBhQ"));
    }

    #[test]
    fn signature_absent_in_text_frames() {
        let json = r#"{"text":" calculating"}"#;
        let event: ReasoningContentEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.text, " calculating");
        assert!(event.signature.is_none());
    }
}
