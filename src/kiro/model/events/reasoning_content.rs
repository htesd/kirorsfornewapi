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
/// 包含模型推理过程的流式片段。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningContentEvent {
    /// 推理内容片段
    #[serde(default)]
    pub text: String,

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
}
