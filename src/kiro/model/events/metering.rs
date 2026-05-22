//! 计费事件 (meteringEvent)
//!
//! Kiro 在 `generateAssistantResponse` 流末端会下发一个 `meteringEvent`，
//! 包含本次请求消耗的积分 / token 计量。原本 `kiro.rs` 把这个事件丢了
//! （`Metering(())`），但 `debug.rs` 里残留的代码暴露了原始结构：
//!
//! ```ignore
//! Event::Metering(e) => {
//!     println!("  unit: {:?}", e.unit);
//!     println!("  unit_plural: {:?}", e.unit_plural);
//!     println!("  usage: {}", e.usage);
//! }
//! ```
//!
//! 救回来落库后，是判断「kiro 服务端有没有应用缓存折扣」的关键信号。
//! 详细背景见 README §请求日志 / Phase 2 数据分析。

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// 计费事件
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MeteringEvent {
    /// 计量单位（如 "credit"）
    #[serde(default)]
    pub unit: Option<String>,
    /// 计量单位复数形式
    #[serde(default)]
    pub unit_plural: Option<String>,
    /// 本次请求消耗的数量
    #[serde(default)]
    pub usage: f64,
    /// 容错：Kiro 后续可能加新字段，全部塞这里不丢
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl EventPayload for MeteringEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        // Kiro 偶尔可能下发空 payload 的 meteringEvent，回退到 default
        match frame.payload_as_json::<Self>() {
            Ok(ev) => Ok(ev),
            Err(_) => Ok(Self::default()),
        }
    }
}

impl MeteringEvent {
    /// 用复数单位（如 "credits"）的展示文本
    pub fn pretty(&self) -> String {
        let unit = self
            .unit_plural
            .as_deref()
            .or(self.unit.as_deref())
            .unwrap_or("units");
        format!("{:.4} {}", self.usage, unit)
    }
}

impl std::fmt::Display for MeteringEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.pretty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_with_unit_plural() {
        let ev = MeteringEvent {
            unit: Some("credit".into()),
            unit_plural: Some("credits".into()),
            usage: 0.0125,
            extra: Default::default(),
        };
        assert_eq!(ev.pretty(), "0.0125 credits");
    }

    #[test]
    fn pretty_fallback_to_unit() {
        let ev = MeteringEvent {
            unit: Some("token".into()),
            unit_plural: None,
            usage: 100.0,
            extra: Default::default(),
        };
        assert_eq!(ev.pretty(), "100.0000 token");
    }

    #[test]
    fn deserialize_from_kiro_json() {
        let json = r#"{"unit":"credit","unitPlural":"credits","usage":0.0125}"#;
        let ev: MeteringEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.usage, 0.0125);
        assert_eq!(ev.unit.as_deref(), Some("credit"));
        assert_eq!(ev.unit_plural.as_deref(), Some("credits"));
    }

    #[test]
    fn deserialize_with_extra_fields() {
        let json = r#"{"unit":"credit","usage":1.5,"futureField":"abc","another":42}"#;
        let ev: MeteringEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.usage, 1.5);
        assert_eq!(ev.extra.len(), 2);
        assert!(ev.extra.contains_key("futureField"));
    }
}
