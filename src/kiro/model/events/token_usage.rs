//! Token 使用事件
//!
//! 处理 tokenUsageEvent 类型的事件
//!
//! Kiro 后端在流末端会下发 tokenUsageEvent,字段对齐 Amazon Q 的 TokenUsage:
//! ```json
//! {
//!   "uncachedInputTokens": 12345,
//!   "outputTokens": 678,
//!   "totalTokens": 13023,
//!   "cacheReadInputTokens": 0,
//!   "cacheWriteInputTokens": 0
//! }
//! ```
//!
//! 这是 Kiro **精确**的 token 计量(包含 thinking 输出 + 缓存命中/写入明细)。
//! 此前我们不认这个事件、被当 Unknown 丢弃,导致只能估算 thinking、估算缓存。

use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

/// Token 使用事件
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageEvent {
    /// 未命中缓存的输入 token(等于 fresh input + 头一次写入缓存的部分)
    #[serde(default)]
    pub uncached_input_tokens: i64,
    /// 输出 token(精确,**含 thinking**)
    #[serde(default)]
    pub output_tokens: i64,
    /// 总 token(uncached + cache_read + output)
    #[serde(default)]
    pub total_tokens: i64,
    /// 命中缓存读取的 token(按 0.1× 计费的部分)
    #[serde(default)]
    pub cache_read_input_tokens: Option<i64>,
    /// 写入缓存的 token(按 1.25× 计费的部分)
    #[serde(default)]
    pub cache_write_input_tokens: Option<i64>,
    /// 容错:Kiro 后续可能加新字段,全部塞这里不丢
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl EventPayload for TokenUsageEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        // 偶发空 payload 时回退到 default,避免整条流崩
        match frame.payload_as_json::<Self>() {
            Ok(ev) => Ok(ev),
            Err(_) => Ok(Self::default()),
        }
    }
}
