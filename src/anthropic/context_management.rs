//! Anthropic context-management beta（"remote compact"）的客户端裁剪
//!
//! Kiro 后端不原生支持服务端裁剪（已对照 kiro-gateway / AIClient-2-API 确认生态无人实现）。
//! 本模块在 proxy 侧"模拟"应用 `clear_tool_uses_20250605` 指令：
//! 当客户端发来 `context_management: {edits: [...]}` 且 input_tokens 触发阈值时，
//! 把早期的 `tool_use` / `tool_result` 块从 messages 历史里删除，保留最近 N 条
//! 与 `exclude_tools` 列表里的工具，从而显著降低发给 Kiro 的 token 数。
//!
//! ## 支持的 edit 类型
//! - `clear_tool_uses_20250605`：清理工具调用历史。字段：
//!   - `trigger.value`：input_tokens 达到此值才触发（缺省 = 立即应用）
//!   - `keep.value`：保留最后 N 条 tool_use（缺省 3）
//!   - `exclude_tools`：永不清理的工具名列表
//! - `clear_thinking_20251015`：清理 thinking 块（推理 token 摊在 output，
//!   重复占 context 没意义）。字段：
//!   - `trigger.value` / `keep.value`：同上（缺省保留最后 3 条 thinking）
//!
//! 其它 edit 类型当前不支持，遇到时 log warning 跳过。
//!
//! ## 限制
//! - 不在响应里上报 `applied_edits`（Anthropic 原生 API 会，提示客户端哪些被清理）。
//!   多数客户端只关心副作用，不依赖此回执；如确需，可在 v22+ 扩展。

use crate::anthropic::types::MessagesRequest;
use std::collections::HashSet;

/// 应用结果（便于日志/响应展示）
#[derive(Debug, Clone, Default)]
pub struct AppliedResult {
    /// 已清理的 tool_use 数（不含 exclude_tools）
    pub cleared_tool_uses: usize,
    /// 已清理的 tool_result 数（与上面 1-1 对应通常）
    pub cleared_tool_results: usize,
    /// 已清理的 thinking 块数
    pub cleared_thinking: usize,
    /// 是否真的应用了任何 edit（false = 触发条件不满足或无可清理）
    pub applied: bool,
}

/// 把 `context_management.edits` 应用到 messages（原地修改）。
///
/// `current_input_tokens` 是 trigger 比较用，由调用方在 convert 前估算。
pub fn apply(payload: &mut MessagesRequest, current_input_tokens: i32) -> AppliedResult {
    let mut result = AppliedResult::default();
    let Some(cm) = payload.context_management.clone() else {
        return result;
    };
    let Some(edits) = cm.get("edits").and_then(|e| e.as_array()) else {
        return result;
    };

    for edit in edits {
        let edit_type = edit.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match edit_type {
            "clear_tool_uses_20250605" => {
                let trigger_tokens = edit
                    .get("trigger")
                    .and_then(|t| t.get("value"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                if trigger_tokens > 0 && current_input_tokens < trigger_tokens {
                    tracing::debug!(
                        trigger = trigger_tokens,
                        current = current_input_tokens,
                        "context_management: trigger 未达阈值，跳过"
                    );
                    continue;
                }
                let keep_count = edit
                    .get("keep")
                    .and_then(|k| k.get("value"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3) as usize;
                let exclude_tools: HashSet<String> = edit
                    .get("exclude_tools")
                    .and_then(|e| e.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let r = clear_tool_uses(&mut payload.messages, keep_count, &exclude_tools);
                result.cleared_tool_uses += r.cleared_tool_uses;
                result.cleared_tool_results += r.cleared_tool_results;
                if r.cleared_tool_uses + r.cleared_tool_results > 0 {
                    result.applied = true;
                }
            }
            "clear_thinking_20251015" => {
                let trigger_tokens = edit
                    .get("trigger")
                    .and_then(|t| t.get("value"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                if trigger_tokens > 0 && current_input_tokens < trigger_tokens {
                    tracing::debug!(
                        trigger = trigger_tokens,
                        current = current_input_tokens,
                        "context_management(thinking): trigger 未达阈值，跳过"
                    );
                    continue;
                }
                let keep_count = edit
                    .get("keep")
                    .and_then(|k| k.get("value"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(3) as usize;

                let cleared = clear_thinking(&mut payload.messages, keep_count);
                result.cleared_thinking += cleared;
                if cleared > 0 {
                    result.applied = true;
                }
            }
            other => {
                tracing::warn!(edit_type = other, "未知 context_management edit 类型，跳过");
            }
        }
    }

    if result.applied {
        tracing::info!(
            cleared_tool_uses = result.cleared_tool_uses,
            cleared_tool_results = result.cleared_tool_results,
            cleared_thinking = result.cleared_thinking,
            "已应用 context_management 客户端裁剪"
        );
    }
    result
}

/// 清理工具历史：保留最后 `keep` 条 tool_use（按出现顺序）及 `exclude_tools` 里的工具，
/// 其余 tool_use 与配对的 tool_result 块从 messages 内容里移除。
///
/// 配对方式：tool_result 通过 `tool_use_id` 关联到 tool_use 的 `id`。
fn clear_tool_uses(
    messages: &mut Vec<crate::anthropic::types::Message>,
    keep: usize,
    exclude_tools: &HashSet<String>,
) -> AppliedResult {
    // 1) 扫描收集所有候选清理的 tool_use id（按出现顺序，跳过 exclude_tools）
    let mut all_clearable_ids: Vec<String> = Vec::new();
    for msg in messages.iter() {
        if let Some(arr) = msg.content.as_array() {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !id.is_empty() && !exclude_tools.contains(name) {
                        all_clearable_ids.push(id.to_string());
                    }
                }
            }
        }
    }

    // 2) 保留最后 `keep` 条，前面的进入 to_clear
    let to_clear_count = all_clearable_ids.len().saturating_sub(keep);
    if to_clear_count == 0 {
        return AppliedResult::default();
    }
    let to_clear: HashSet<String> = all_clearable_ids
        .into_iter()
        .take(to_clear_count)
        .collect();

    // 3) 走一遍 messages，从每条 content 数组里删 to_clear 的 tool_use 和对应 tool_result
    let mut cleared_uses = 0;
    let mut cleared_results = 0;
    for msg in messages.iter_mut() {
        let Some(arr) = msg.content.as_array() else {
            continue;
        };
        let filtered: Vec<serde_json::Value> = arr
            .iter()
            .filter(|block| {
                let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if bt == "tool_use" {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    if to_clear.contains(id) {
                        cleared_uses += 1;
                        return false;
                    }
                } else if bt == "tool_result" {
                    let tuid = block
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("");
                    if to_clear.contains(tuid) {
                        cleared_results += 1;
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        msg.content = serde_json::Value::Array(filtered);
    }

    // 4) 删除内容已空的 messages（仅有 tool_use/tool_result 的中间消息会变空）
    messages.retain(|m| match &m.content {
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true,
    });

    AppliedResult {
        cleared_tool_uses: cleared_uses,
        cleared_tool_results: cleared_results,
        cleared_thinking: 0,
        applied: cleared_uses + cleared_results > 0,
    }
}

/// 清理 thinking 块：保留最后 `keep` 条（按出现顺序），其余从 assistant 消息内容里移除。
///
/// thinking 块没有 `id`，纯按位置识别。返回清理数量。
fn clear_thinking(
    messages: &mut Vec<crate::anthropic::types::Message>,
    keep: usize,
) -> usize {
    // 1) 数 thinking 总数
    let mut total: usize = 0;
    for msg in messages.iter() {
        if msg.role != "assistant" {
            continue;
        }
        if let Some(arr) = msg.content.as_array() {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    total += 1;
                }
            }
        }
    }
    let to_clear_count = total.saturating_sub(keep);
    if to_clear_count == 0 {
        return 0;
    }

    // 2) 按出现顺序删前 `to_clear_count` 个 thinking
    let mut remaining_to_clear = to_clear_count;
    let mut cleared = 0;
    for msg in messages.iter_mut() {
        if msg.role != "assistant" {
            continue;
        }
        let Some(arr) = msg.content.as_array() else {
            continue;
        };
        let filtered: Vec<serde_json::Value> = arr
            .iter()
            .filter(|block| {
                if remaining_to_clear == 0 {
                    return true;
                }
                if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    remaining_to_clear -= 1;
                    cleared += 1;
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        msg.content = serde_json::Value::Array(filtered);
        if remaining_to_clear == 0 {
            break;
        }
    }

    // 3) 删空 messages
    messages.retain(|m| match &m.content {
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true,
    });

    cleared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::{Message, MessagesRequest};
    use serde_json::json;

    fn mk_request(messages: Vec<Message>, cm: Option<serde_json::Value>) -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 100,
            messages,
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
            context_management: cm,
        }
    }

    fn assistant_with_tool(id: &str, name: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type": "text", "text": "ok"},
                {"type": "tool_use", "id": id, "name": name, "input": {}}
            ]),
        }
    }

    fn user_with_result(tuid: &str, text: &str) -> Message {
        Message {
            role: "user".to_string(),
            content: json!([
                {"type": "tool_result", "tool_use_id": tuid, "content": text}
            ]),
        }
    }

    #[test]
    fn no_cm_no_op() {
        let mut req = mk_request(vec![], None);
        let r = apply(&mut req, 100_000);
        assert!(!r.applied);
    }

    #[test]
    fn keeps_last_n_tool_uses() {
        let messages = vec![
            assistant_with_tool("t1", "Bash"),
            user_with_result("t1", "r1"),
            assistant_with_tool("t2", "Bash"),
            user_with_result("t2", "r2"),
            assistant_with_tool("t3", "Bash"),
            user_with_result("t3", "r3"),
            assistant_with_tool("t4", "Bash"),
            user_with_result("t4", "r4"),
        ];
        let cm = json!({"edits": [{"type": "clear_tool_uses_20250605", "keep": {"value": 2}}]});
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 200_000);
        assert!(r.applied);
        assert_eq!(r.cleared_tool_uses, 2); // t1, t2
        assert_eq!(r.cleared_tool_results, 2);
        // 剩下应该只有 t3, t4 的 tool_use（带文本"ok"的 assistant）与对应 tool_result
        let remaining_ids: Vec<String> = req
            .messages
            .iter()
            .flat_map(|m| m.content.as_array().cloned().unwrap_or_default())
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    b.get("id").and_then(|i| i.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(remaining_ids, vec!["t3", "t4"]);
    }

    #[test]
    fn trigger_threshold_blocks() {
        let messages = vec![
            assistant_with_tool("t1", "Bash"),
            assistant_with_tool("t2", "Bash"),
            assistant_with_tool("t3", "Bash"),
        ];
        let cm = json!({
            "edits": [{"type": "clear_tool_uses_20250605", "trigger": {"value": 100000}, "keep": {"value": 1}}]
        });
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 50_000);
        assert!(!r.applied, "current < trigger, nothing applied");
    }

    #[test]
    fn exclude_tools_protected() {
        let messages = vec![
            assistant_with_tool("t1", "str_replace_editor"),
            assistant_with_tool("t2", "Bash"),
            assistant_with_tool("t3", "Bash"),
        ];
        let cm = json!({
            "edits": [{"type": "clear_tool_uses_20250605", "keep": {"value": 1}, "exclude_tools": ["str_replace_editor"]}]
        });
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 100_000);
        assert!(r.applied);
        // t1 受保护，t2 被清，t3 是最后一条保留 → 剩 t1 + t3
        let remaining_ids: Vec<String> = req
            .messages
            .iter()
            .flat_map(|m| m.content.as_array().cloned().unwrap_or_default())
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    b.get("id").and_then(|i| i.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(remaining_ids, vec!["t1", "t3"]);
    }

    #[test]
    fn empty_message_removed_after_clear() {
        // user 消息只含一个 tool_result，清掉后该消息变空 → 应整条移除
        let messages = vec![
            assistant_with_tool("t1", "Bash"),
            user_with_result("t1", "r1"), // 这条会被清空
            assistant_with_tool("t2", "Bash"),
        ];
        let cm = json!({"edits": [{"type": "clear_tool_uses_20250605", "keep": {"value": 1}}]});
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 200_000);
        assert!(r.applied);
        // 起初 3 条 → t1 的 assistant 还剩 text "ok"（不空，留下），user 的 tool_result 没了变空（删除）
        // assistant t2 留下
        assert_eq!(req.messages.len(), 2);
    }

    #[test]
    fn unknown_edit_type_logged_but_safe() {
        let messages = vec![assistant_with_tool("t1", "Bash")];
        let cm = json!({"edits": [{"type": "unknown_edit_99", "value": 1}]});
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 100_000);
        assert!(!r.applied);
    }

    fn assistant_with_thinking(label: &str) -> Message {
        Message {
            role: "assistant".to_string(),
            content: json!([
                {"type": "thinking", "thinking": format!("internal reasoning {}", label)},
                {"type": "text", "text": format!("response {}", label)}
            ]),
        }
    }

    #[test]
    fn clear_thinking_keeps_last_n() {
        let messages = vec![
            assistant_with_thinking("a"),
            Message { role: "user".to_string(), content: json!("more please") },
            assistant_with_thinking("b"),
            Message { role: "user".to_string(), content: json!("again") },
            assistant_with_thinking("c"),
            Message { role: "user".to_string(), content: json!("once more") },
            assistant_with_thinking("d"),
        ];
        let cm = json!({"edits": [{"type": "clear_thinking_20251015", "keep": {"value": 2}}]});
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 200_000);
        assert!(r.applied);
        assert_eq!(r.cleared_thinking, 2); // a, b cleared; c, d kept

        // 收集剩下 thinking 内容
        let remaining_thinking: Vec<String> = req
            .messages
            .iter()
            .flat_map(|m| m.content.as_array().cloned().unwrap_or_default())
            .filter_map(|b| {
                if b.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    b.get("thinking").and_then(|t| t.as_str()).map(String::from)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(remaining_thinking, vec!["internal reasoning c", "internal reasoning d"]);

        // text 块应仍在所有原 assistant 消息里（thinking 被删后 text 留下）
        let text_count = req
            .messages
            .iter()
            .flat_map(|m| m.content.as_array().cloned().unwrap_or_default())
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .count();
        assert_eq!(text_count, 4, "所有 4 个 assistant 的 text 块都应保留");
    }

    #[test]
    fn clear_thinking_trigger_blocks() {
        let messages = vec![
            assistant_with_thinking("a"),
            assistant_with_thinking("b"),
            assistant_with_thinking("c"),
        ];
        let cm = json!({
            "edits": [{"type": "clear_thinking_20251015", "trigger": {"value": 100000}, "keep": {"value": 1}}]
        });
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 50_000);
        assert!(!r.applied, "current < trigger，不应裁剪");
    }

    #[test]
    fn clear_thinking_and_tool_uses_combined() {
        // 两个 edit 同时存在：tool_uses keep 1 + thinking keep 1
        let messages = vec![
            assistant_with_tool("t1", "Bash"),
            user_with_result("t1", "r1"),
            assistant_with_thinking("a"),
            assistant_with_thinking("b"),
            assistant_with_tool("t2", "Bash"),
            user_with_result("t2", "r2"),
        ];
        let cm = json!({"edits": [
            {"type": "clear_tool_uses_20250605", "keep": {"value": 1}},
            {"type": "clear_thinking_20251015", "keep": {"value": 1}}
        ]});
        let mut req = mk_request(messages, Some(cm));
        let r = apply(&mut req, 200_000);
        assert!(r.applied);
        assert_eq!(r.cleared_tool_uses, 1, "t1 被清，t2 留");
        assert_eq!(r.cleared_thinking, 1, "a 被清，b 留");
    }
}
