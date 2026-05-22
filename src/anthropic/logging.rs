//! Handler 层的请求日志胶水
//!
//! 把 [`RequestRecordBuilder`] 的创建、事件观察、落库流程集中到这里，
//! 主 handler 文件只负责调用入口/出口两个 hook。
//!
//! ## V1 范围（当前）
//!
//! - 非流式：完整捕获 metering / context_usage / 输出 token
//! - 流式：仅捕获调度成功时的基础信号（account / attempts / http）
//! - 错误路径：所有 5 类失败都记 error_kind
//!
//! 流式响应内的 metering 实时捕获留给 V2（见 `create_sse_stream`）。

use crate::db::{LogRecorder, RequestRecordBuilder, RequestStatus};
use crate::kiro::model::events::Event;
use crate::kiro::provider::CallOutcome;

use super::types::MessagesRequest;

/// 从 Anthropic 请求构造 builder
///
/// 抽取请求形状信息：messages 数 / tools 数 / system 长度 / 是否带 cache_control。
pub fn begin(endpoint: &str, payload: &MessagesRequest) -> RequestRecordBuilder {
    let messages_count = payload.messages.len() as i32;
    let tools_count = payload.tools.as_ref().map(|v| v.len() as i32).unwrap_or(0);
    let system_prompt_len = payload
        .system
        .as_ref()
        .map(|v| serde_json::to_string(v).map(|s| s.len()).unwrap_or(0) as i32)
        .unwrap_or(0);

    // has_cache_control：扫描 system / messages 序列化后是否含 "cache_control" 字串
    // 简单快速，比逐字段递归判断便宜
    let has_cache_control = serialized_has_cache_control(payload);

    RequestRecordBuilder::begin(
        endpoint,
        &payload.model,
        payload.stream,
        messages_count,
        tools_count,
        has_cache_control,
        system_prompt_len,
    )
}

fn serialized_has_cache_control(payload: &MessagesRequest) -> bool {
    if let Ok(sys) = serde_json::to_string(&payload.system) {
        if sys.contains("cache_control") {
            return true;
        }
    }
    for msg in &payload.messages {
        if let Ok(content_str) = serde_json::to_string(&msg.content) {
            if content_str.contains("cache_control") {
                return true;
            }
        }
    }
    false
}

/// 把调度层返回的 CallOutcome 信息塞进 builder
pub fn observe_dispatch(builder: &mut RequestRecordBuilder, outcome: &CallOutcome) {
    builder.set_account(&outcome.account_id, outcome.account_label.clone());
    builder.set_http_status(outcome.http_status);
    builder.set_attempts(outcome.attempts as i32);
}

/// 把 Kiro 流事件里的关键信号填到 builder
///
/// 当前关注：meteringEvent.usage / contextUsageEvent.context_usage_percentage。
pub fn observe_event(builder: &mut RequestRecordBuilder, event: &Event) {
    match event {
        Event::Metering(m) => {
            builder.set_metering(m.unit.clone(), m.usage);
        }
        Event::ContextUsage(c) => {
            builder.set_context_usage_pct(c.context_usage_percentage);
        }
        _ => {}
    }
}

/// 终态：把 builder 落库（如果 recorder 存在）
///
/// 调用方负责显式给 `status` —— 成功 / 错误 / 取消。
pub fn finish(
    recorder: Option<&LogRecorder>,
    builder: RequestRecordBuilder,
    status: RequestStatus,
) {
    if let Some(rec) = recorder {
        let record = builder.build(status);
        rec.record(record);
    }
}

/// 错误的便捷封装
pub fn finish_with_error(
    recorder: Option<&LogRecorder>,
    mut builder: RequestRecordBuilder,
    kind: &str,
    stage: &str,
    code: impl Into<String>,
    message: impl Into<String>,
) {
    builder.set_error(kind, stage, code, message);
    finish(recorder, builder, RequestStatus::Error);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::{MessagesRequest};
    use serde_json::json;

    fn mk_request(with_cc: bool) -> MessagesRequest {
        // 走 JSON 反序列化构造，避开 MessagesRequest 字段细节
        let messages = if with_cc {
            json!([{
                "role": "user",
                "content": [
                    {"type": "text", "text": "hi", "cache_control": {"type": "ephemeral"}}
                ]
            }])
        } else {
            json!([{"role": "user", "content": "hi"}])
        };

        serde_json::from_value(json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": messages,
            "stream": true,
        }))
        .unwrap()
    }

    #[test]
    fn begin_detects_cache_control() {
        let req = mk_request(true);
        let b = begin("/v1/messages", &req);
        let rec = b.build(RequestStatus::Success);
        assert!(rec.has_cache_control);
        assert_eq!(rec.endpoint, "/v1/messages");
        assert!(rec.is_stream);
        assert_eq!(rec.messages_count, 1);
    }

    #[test]
    fn begin_no_cache_control_when_absent() {
        let req = mk_request(false);
        let b = begin("/v1/messages", &req);
        let rec = b.build(RequestStatus::Success);
        assert!(!rec.has_cache_control);
    }

    #[test]
    fn observe_event_metering() {
        use crate::kiro::model::events::MeteringEvent;
        let mut b = RequestRecordBuilder::begin("/v1/messages", "m", false, 1, 0, false, 0);
        let ev = Event::Metering(MeteringEvent {
            unit: Some("credit".into()),
            unit_plural: Some("credits".into()),
            usage: 0.5,
            extra: Default::default(),
        });
        observe_event(&mut b, &ev);
        let rec = b.build(RequestStatus::Success);
        assert_eq!(rec.metering_usage, Some(0.5));
        assert_eq!(rec.metering_unit.as_deref(), Some("credit"));
    }

    #[test]
    fn observe_event_context_usage() {
        use crate::kiro::model::events::ContextUsageEvent;
        let mut b = RequestRecordBuilder::begin("/v1/messages", "m", false, 1, 0, false, 0);
        let ev = Event::ContextUsage(ContextUsageEvent {
            context_usage_percentage: 42.5,
        });
        observe_event(&mut b, &ev);
        let rec = b.build(RequestStatus::Success);
        assert_eq!(rec.context_usage_pct, Some(42.5));
    }
}
