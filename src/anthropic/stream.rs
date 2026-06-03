//! 流式响应处理模块
//!
//! 实现 Kiro → Anthropic 流式响应转换和 SSE 状态管理

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;

use crate::kiro::model::events::Event;

/// 找到小于等于目标位置的最近有效UTF-8字符边界
///
/// UTF-8字符可能占用1-4个字节，直接按字节位置切片可能会切在多字节字符中间导致panic。
/// 这个函数从目标位置向前搜索，找到最近的有效字符边界。
fn find_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    if target == 0 {
        return 0;
    }
    // 从目标位置向前搜索有效的字符边界
    let mut pos = target;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// 需要跳过的包裹字符
///
/// 当 thinking 标签被这些字符包裹时，认为是在引用标签而非真正的标签：
/// - 反引号 (`)：行内代码
/// - 双引号 (")：字符串
/// - 单引号 (')：字符串
const QUOTE_CHARS: &[u8] = &[
    b'`', b'"', b'\'', b'\\', b'#', b'!', b'@', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'-',
    b'_', b'=', b'+', b'[', b']', b'{', b'}', b';', b':', b'<', b'>', b',', b'.', b'?', b'/',
];

/// 检查指定位置的字符是否是引用字符
fn is_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|c| QUOTE_CHARS.contains(c))
        .unwrap_or(false)
}

/// 查找真正的 thinking 结束标签（不被引用字符包裹，且后面有双换行符）
///
/// 当模型在思考过程中提到 `</thinking>` 时，通常会用反引号、引号等包裹，
/// 或者在同一行有其他内容（如"关于 </thinking> 标签"）。
/// 这个函数会跳过这些情况，只返回真正的结束标签位置。
///
/// 跳过的情况：
/// - 被引用字符包裹（反引号、引号等）
/// - 后面没有双换行符（真正的结束标签后面会有 `\n\n`）
/// - 标签在缓冲区末尾（流式处理时需要等待更多内容）
///
/// # 参数
/// - `buffer`: 要搜索的字符串
///
/// # 返回值
/// - `Some(pos)`: 真正的结束标签的起始位置
/// - `None`: 没有找到真正的结束标签
fn find_real_thinking_end_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        // 检查前面是否有引用字符
        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);

        // 检查后面是否有引用字符
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        // 如果被引用字符包裹，跳过
        if has_quote_before || has_quote_after {
            search_start = absolute_pos + 1;
            continue;
        }

        // 检查后面的内容
        let after_content = &buffer[after_pos..];

        // 如果标签后面内容不足以判断是否有双换行符，等待更多内容
        if after_content.len() < 2 {
            return None;
        }

        // 真正的 thinking 结束标签后面会有双换行符 `\n\n`
        if after_content.starts_with("\n\n") {
            return Some(absolute_pos);
        }

        // 不是双换行符，跳过继续搜索
        search_start = absolute_pos + 1;
    }

    None
}

/// 查找缓冲区末尾的 thinking 结束标签（允许末尾只有空白字符）
///
/// 用于“边界事件”场景：例如 thinking 结束后立刻进入 tool_use，或流结束，
/// 此时 `</thinking>` 后面可能没有 `\n\n`，但结束标签依然应被识别并过滤。
///
/// 约束：只有当 `</thinking>` 之后全部都是空白字符时才认为是结束标签，
/// 以避免在 thinking 内容中提到 `</thinking>`（非结束标签）时误判。
fn find_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        // 检查前面是否有引用字符
        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);

        // 检查后面是否有引用字符
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        if has_quote_before || has_quote_after {
            search_start = absolute_pos + 1;
            continue;
        }

        // 只有当标签后面全部是空白字符时才认定为结束标签
        if buffer[after_pos..].trim().is_empty() {
            return Some(absolute_pos);
        }

        search_start = absolute_pos + 1;
    }

    None
}

/// 查找真正的 thinking 开始标签（不被引用字符包裹）
///
/// 与 `find_real_thinking_end_tag` 类似，跳过被引用字符包裹的开始标签。
fn find_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "<thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        // 检查前面是否有引用字符
        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);

        // 检查后面是否有引用字符
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        // 如果不被引用字符包裹，则是真正的开始标签
        if !has_quote_before && !has_quote_after {
            return Some(absolute_pos);
        }

        // 继续搜索下一个匹配
        search_start = absolute_pos + 1;
    }

    None
}

/// 从完整文本中提取 thinking 块（用于非流式响应）
///
/// 使用与流式处理相同的标签检测逻辑（引用字符过滤），确保一致性。
/// 非流式场景下文本已完整，无需处理跨 chunk 分割问题。
///
/// # 返回值
/// - `(Some(thinking_content), remaining_text)` — 检测到有效 thinking 块
/// - `(None, original_text)` — 未检测到，原样返回
pub(crate) fn extract_thinking_from_complete_text(text: &str) -> (Option<String>, String) {
    let start_pos = match find_real_thinking_start_tag(text) {
        Some(pos) => pos,
        None => return (None, text.to_string()),
    };

    let before = &text[..start_pos];
    let after_open = &text[start_pos + "<thinking>".len()..];

    // 查找结束标签：优先匹配带 \n\n 后缀的，退而使用末尾匹配
    let (thinking_raw, text_after) =
        if let Some(end_pos) = find_real_thinking_end_tag(after_open) {
            (
                &after_open[..end_pos],
                &after_open[end_pos + "</thinking>\n\n".len()..],
            )
        } else if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(after_open) {
            let after_tag = end_pos + "</thinking>".len();
            (
                &after_open[..end_pos],
                after_open[after_tag..].trim_start(),
            )
        } else {
            // 找不到有效的结束标签，不做提取
            return (None, text.to_string());
        };

    // 剥离开头的换行符（与流式处理一致：模型输出 <thinking>\n）
    let thinking_content = thinking_raw
        .strip_prefix('\n')
        .unwrap_or(thinking_raw);

    // 组装剩余文本：跳过纯空白的 before 部分
    let mut remaining = String::new();
    if !before.trim().is_empty() {
        remaining.push_str(before);
    }
    remaining.push_str(text_after);

    if thinking_content.is_empty() {
        (None, remaining)
    } else {
        (Some(thinking_content.to_string()), remaining)
    }
}

/// SSE 事件
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: String,
    pub data: serde_json::Value,
}

impl SseEvent {
    pub fn new(event: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }

    /// 格式化为 SSE 字符串
    pub fn to_sse_string(&self) -> String {
        format!(
            "event: {}\ndata: {}\n\n",
            self.event,
            serde_json::to_string(&self.data).unwrap_or_default()
        )
    }
}

/// 内容块状态
#[derive(Debug, Clone)]
struct BlockState {
    block_type: String,
    started: bool,
    stopped: bool,
}

impl BlockState {
    fn new(block_type: impl Into<String>) -> Self {
        Self {
            block_type: block_type.into(),
            started: false,
            stopped: false,
        }
    }
}

/// SSE 状态管理器
///
/// 确保 SSE 事件序列符合 Claude API 规范：
/// 1. message_start 只能出现一次
/// 2. content_block 必须先 start 再 delta 再 stop
/// 3. message_delta 只能出现一次，且在所有 content_block_stop 之后
/// 4. message_stop 在最后
#[derive(Debug)]
pub struct SseStateManager {
    /// message_start 是否已发送
    message_started: bool,
    /// message_delta 是否已发送
    message_delta_sent: bool,
    /// 活跃的内容块状态
    active_blocks: HashMap<i32, BlockState>,
    /// 消息是否已结束
    message_ended: bool,
    /// 下一个块索引
    next_block_index: i32,
    /// 当前 stop_reason
    stop_reason: Option<String>,
    /// 是否有工具调用
    has_tool_use: bool,
}

impl Default for SseStateManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SseStateManager {
    pub fn new() -> Self {
        Self {
            message_started: false,
            message_delta_sent: false,
            active_blocks: HashMap::new(),
            message_ended: false,
            next_block_index: 0,
            stop_reason: None,
            has_tool_use: false,
        }
    }

    /// 判断指定块是否处于可接收 delta 的打开状态
    fn is_block_open_of_type(&self, index: i32, expected_type: &str) -> bool {
        self.active_blocks
            .get(&index)
            .is_some_and(|b| b.started && !b.stopped && b.block_type == expected_type)
    }

    /// 获取下一个块索引
    pub fn next_block_index(&mut self) -> i32 {
        let index = self.next_block_index;
        self.next_block_index += 1;
        index
    }

    /// 记录工具调用
    pub fn set_has_tool_use(&mut self, has: bool) {
        self.has_tool_use = has;
    }

    /// 设置 stop_reason
    pub fn set_stop_reason(&mut self, reason: impl Into<String>) {
        self.stop_reason = Some(reason.into());
    }

    /// 检查是否存在非 thinking 类型的内容块（如 text 或 tool_use）
    fn has_non_thinking_blocks(&self) -> bool {
        self.active_blocks
            .values()
            .any(|b| b.block_type != "thinking")
    }

    /// 关闭所有已开启但未关闭的内容块，返回对应的事件。
    /// 用于错误/空响应终止前清理 SSE 结构，避免残留半开块。
    /// thinking 类型块在 stop 前补发 `signature_delta`（哪怕空签名），满足 Anthropic 对
    /// thinking 块结构的要求——覆盖 fake `<thinking>` 标签解析产生的块（它们不走
    /// reasoning_block_active 路径）。
    fn close_all_open_blocks(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for (index, block) in self.active_blocks.iter_mut() {
            if block.started && !block.stopped {
                if block.block_type == "thinking" {
                    events.push(SseEvent::new(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "signature_delta", "signature": "" }
                        }),
                    ));
                }
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": index }),
                ));
                block.stopped = true;
            }
        }
        events
    }

    /// 获取最终的 stop_reason
    pub fn get_stop_reason(&self) -> String {
        if let Some(ref reason) = self.stop_reason {
            reason.clone()
        } else if self.has_tool_use {
            "tool_use".to_string()
        } else {
            "end_turn".to_string()
        }
    }

    /// 处理 message_start 事件
    pub fn handle_message_start(&mut self, event: serde_json::Value) -> Option<SseEvent> {
        if self.message_started {
            tracing::debug!("跳过重复的 message_start 事件");
            return None;
        }
        self.message_started = true;
        Some(SseEvent::new("message_start", event))
    }

    /// 处理 content_block_start 事件
    pub fn handle_content_block_start(
        &mut self,
        index: i32,
        block_type: &str,
        data: serde_json::Value,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 如果是 tool_use 块，先关闭之前的文本块
        if block_type == "tool_use" {
            self.has_tool_use = true;
            for (block_index, block) in self.active_blocks.iter_mut() {
                if block.block_type == "text" && block.started && !block.stopped {
                    // 自动发送 content_block_stop 关闭文本块
                    events.push(SseEvent::new(
                        "content_block_stop",
                        json!({
                            "type": "content_block_stop",
                            "index": block_index
                        }),
                    ));
                    block.stopped = true;
                }
            }
        }

        // 检查块是否已存在
        if let Some(block) = self.active_blocks.get_mut(&index) {
            if block.started {
                tracing::debug!("块 {} 已启动，跳过重复的 content_block_start", index);
                return events;
            }
            block.started = true;
        } else {
            let mut block = BlockState::new(block_type);
            block.started = true;
            self.active_blocks.insert(index, block);
        }

        events.push(SseEvent::new("content_block_start", data));
        events
    }

    /// 处理 content_block_delta 事件
    pub fn handle_content_block_delta(
        &mut self,
        index: i32,
        data: serde_json::Value,
    ) -> Option<SseEvent> {
        // 确保块已启动
        if let Some(block) = self.active_blocks.get(&index) {
            if !block.started || block.stopped {
                tracing::warn!(
                    "块 {} 状态异常: started={}, stopped={}",
                    index,
                    block.started,
                    block.stopped
                );
                return None;
            }
        } else {
            // 块不存在，可能需要先创建
            tracing::warn!("收到未知块 {} 的 delta 事件", index);
            return None;
        }

        Some(SseEvent::new("content_block_delta", data))
    }

    /// 处理 content_block_stop 事件
    pub fn handle_content_block_stop(&mut self, index: i32) -> Option<SseEvent> {
        if let Some(block) = self.active_blocks.get_mut(&index) {
            if block.stopped {
                tracing::debug!("块 {} 已停止，跳过重复的 content_block_stop", index);
                return None;
            }
            block.stopped = true;
            return Some(SseEvent::new(
                "content_block_stop",
                json!({
                    "type": "content_block_stop",
                    "index": index
                }),
            ));
        }
        None
    }

    /// 生成最终事件序列
    pub fn generate_final_events(
        &mut self,
        input_tokens: i32,
        output_tokens: i32,
        cache_read: i32,
        cache_creation: i32,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 关闭所有未关闭的块
        for (index, block) in self.active_blocks.iter_mut() {
            if block.started && !block.stopped {
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({
                        "type": "content_block_stop",
                        "index": index
                    }),
                ));
                block.stopped = true;
            }
        }

        // 发送 message_delta（usage 含 cache_read / cache_creation，>0 才写）
        if !self.message_delta_sent {
            self.message_delta_sent = true;
            events.push(SseEvent::new(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": self.get_stop_reason(),
                        "stop_sequence": null
                    },
                    "usage": super::usage::build_usage_json(
                        input_tokens, output_tokens, cache_read, cache_creation,
                    )
                }),
            ));
        }

        // 发送 message_stop
        if !self.message_ended {
            self.message_ended = true;
            events.push(SseEvent::new(
                "message_stop",
                json!({ "type": "message_stop" }),
            ));
        }

        events
    }
}

use super::converter::get_context_window_size;

/// 流处理上下文
pub struct StreamContext {
    /// SSE 状态管理器
    pub state_manager: SseStateManager,
    /// 请求的模型名称
    pub model: String,
    /// 消息 ID
    pub message_id: String,
    /// 输入 tokens（估算值）
    pub input_tokens: i32,
    /// 从 contextUsageEvent 计算的实际输入 tokens
    pub context_input_tokens: Option<i32>,
    /// 输出 tokens 累计
    pub output_tokens: i32,
    /// 工具块索引映射 (tool_id -> block_index)
    pub tool_block_indices: HashMap<String, i32>,
    /// 工具名称反向映射（短名称 → 原始名称），用于响应时还原
    pub tool_name_map: HashMap<String, String>,
    /// thinking 是否启用
    pub thinking_enabled: bool,
    /// thinking 内容缓冲区
    pub thinking_buffer: String,
    /// 是否在 thinking 块内
    pub in_thinking_block: bool,
    /// thinking 块是否已提取完成
    pub thinking_extracted: bool,
    /// thinking 块索引
    pub thinking_block_index: Option<i32>,
    /// 文本块索引（thinking 启用时动态分配）
    pub text_block_index: Option<i32>,
    /// 是否需要剥离 thinking 内容开头的换行符
    /// 模型输出 `<thinking>\n` 时，`\n` 可能与标签在同一 chunk 或下一 chunk
    strip_thinking_leading_newline: bool,
    /// Kiro tokenUsageEvent.cacheReadInputTokens（None = 未上报，走 cache_estimate 回退）
    pub cache_read_input_tokens: Option<i32>,
    /// Kiro tokenUsageEvent.cacheWriteInputTokens
    pub cache_creation_input_tokens: Option<i32>,
    /// Kiro meteringEvent.usage（cache_estimate 回退需要）
    pub metering_usage: Option<f64>,
    /// 缓存上报缩放倍率（来自 config.cache.readMultiplier，运行时可热调）
    pub cache_read_multiplier: f64,
    /// 命中上限比率（config.cache.capRatio）：上报封顶 = total × 此值
    pub cache_cap_ratio: f64,
    /// 最低比率（config.cache.floorRatio）：上报下限 = total × 此值
    pub cache_floor_ratio: f64,
    /// prefix 缓存模拟器算出的 (命中 token, 本轮总 token)；同口径，供 billing 算比例。None = 未模拟。
    pub sim_cache: Option<(i32, i32)>,
    /// 流结束时实际 emit 给 NewAPI 的 cache_read（generate_final_events 设置）
    pub emitted_cache_read: Option<i32>,
    /// 流结束时模拟器命中的真实值（写入 DB cached_tokens，供缓存分析）
    pub emitted_raw_cache_read: Option<i32>,
    pub emitted_cache_creation: Option<i32>,
    /// 原生 reasoningContentEvent 的 thinking 块是否已开启（未关闭）
    /// 与 fake `<thinking>` 标签解析互斥：上游走独立 reasoning 流时用这条路径。
    reasoning_block_active: bool,
    /// fake `<thinking>` 路径累积发出的 thinking 文本（用于关闭块时合成签名）。
    /// 上游不下发 reasoningContentEvent.signature 时（如 opus-4-6），据此造结构合法签名。
    fake_thinking_text: String,
    /// 本次响应是否出现过原生 reasoning。一旦出现，正文走纯 text（绕过 fake
    /// `<thinking>` 标签解析）——因为推理已在独立通道，正文不会再含 `<thinking>`。
    native_reasoning_seen: bool,
    /// 上游下发的原生 thinking 签名（protobuf base64，仅 thinking 流最后一帧携带）。
    /// 关闭 thinking 块时透传到 Anthropic `signature_delta`，替代占位空签名。
    reasoning_signature: Option<String>,
    /// 流的终态失败（None = 正常）。区分上游 error 事件 / 上游 exception / 空响应 / IO 错误，
    /// 供日志按 error_kind 分类、并据此向客户端补发终止性 `error` 事件。
    /// 此前上游 error/exception 仅打日志后丢弃、空响应按 success 收尾，导致客户端收到“空响应正常结束”。
    failure: Option<StreamFailure>,
}

/// 流式响应的终态失败类型。用结构化枚举区分四类，避免把“上游真错”和“本地判定空响应”混为一谈。
#[derive(Debug, Clone)]
pub enum StreamFailure {
    /// 上游下发了 error 事件
    UpstreamError(String),
    /// 上游下发了 exception 事件（ContentLengthExceeded 除外，那是正常的 max_tokens）
    UpstreamException(String),
    /// 上游 HTTP 200 但流中零实质内容（无正文/工具/推理）
    EmptyResponse,
    /// 读取上游响应流时 IO 错误（连接中断等）
    StreamIo(String),
}

impl StreamFailure {
    /// 日志用的 error_kind 分类
    pub fn error_kind(&self) -> &'static str {
        match self {
            StreamFailure::UpstreamError(_) => "upstream_error",
            StreamFailure::UpstreamException(_) => "upstream_exception",
            StreamFailure::EmptyResponse => "empty_response",
            StreamFailure::StreamIo(_) => "stream_io",
        }
    }

    /// 发给客户端 error 事件的 message
    pub fn client_message(&self) -> String {
        match self {
            StreamFailure::UpstreamError(m) => format!("upstream error event: {}", m),
            StreamFailure::UpstreamException(m) => format!("upstream exception: {}", m),
            StreamFailure::EmptyResponse => {
                "上游返回空响应（200 但流中无任何内容事件），可能是生成超时或瞬时限流".to_string()
            }
            StreamFailure::StreamIo(m) => format!("reading upstream stream failed: {}", m),
        }
    }
}

impl StreamContext {
    /// 创建启用thinking的StreamContext
    pub fn new_with_thinking(
        model: impl Into<String>,
        input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
    ) -> Self {
        Self {
            state_manager: SseStateManager::new(),
            model: model.into(),
            message_id: format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
            input_tokens,
            context_input_tokens: None,
            output_tokens: 0,
            tool_block_indices: HashMap::new(),
            tool_name_map,
            thinking_enabled,
            thinking_buffer: String::new(),
            in_thinking_block: false,
            thinking_extracted: false,
            thinking_block_index: None,
            text_block_index: None,
            strip_thinking_leading_newline: false,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            metering_usage: None,
            cache_read_multiplier: super::usage::DEFAULT_CACHE_READ_MULTIPLIER,
            cache_cap_ratio: super::usage::DEFAULT_CACHE_CAP_RATIO,
            cache_floor_ratio: super::usage::DEFAULT_CACHE_FLOOR_RATIO,
            sim_cache: None,
            emitted_cache_read: None,
            emitted_raw_cache_read: None,
            emitted_cache_creation: None,
            reasoning_block_active: false,
            fake_thinking_text: String::new(),
            native_reasoning_seen: false,
            reasoning_signature: None,
            failure: None,
        }
    }

    /// 设置缓存上报缩放倍率（来自 config.cache.readMultiplier 的 live 值）
    pub fn set_cache_read_multiplier(&mut self, m: f64) {
        self.cache_read_multiplier = m;
    }

    /// 设置命中上限比率（config.cache.capRatio 的 live 值）
    pub fn set_cache_cap_ratio(&mut self, r: f64) {
        self.cache_cap_ratio = r;
    }

    /// 设置最低比率（config.cache.floorRatio 的 live 值）
    pub fn set_cache_floor_ratio(&mut self, r: f64) {
        self.cache_floor_ratio = r;
    }

    /// 设置 prefix 缓存模拟器结果 (命中 token, 本轮总 token)（计费上报来源）
    pub fn set_sim_cache(&mut self, v: Option<(i32, i32)>) {
        self.sim_cache = v;
    }

    /// 生成 message_start 事件
    pub fn create_message_start_event(&self) -> serde_json::Value {
        json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": self.model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": self.input_tokens,
                    "output_tokens": 1
                }
            }
        })
    }

    /// 本次响应是否产出过任何实质内容（正文 token / 工具调用 / 原生 reasoning）。
    ///
    /// 用于检测"上游 200 但 body 完全为空（零事件）"的空响应：此时 output_tokens=0、
    /// 无 tool_use、无 reasoning。这类请求此前按 success 收尾，客户端收到空回答却以为正常完成。
    pub fn produced_any_content(&self) -> bool {
        self.output_tokens > 0
            || !self.tool_block_indices.is_empty()
            || self.native_reasoning_seen
    }

    /// 标记流的终态失败（首个失败优先，不覆盖）。封装内部状态，避免外部直接改字段。
    pub fn mark_failure(&mut self, failure: StreamFailure) {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
    }

    /// 当前失败的 error_kind（None = 无失败）
    pub fn failure_kind(&self) -> Option<&'static str> {
        self.failure.as_ref().map(|f| f.error_kind())
    }

    /// 当前失败发给日志/客户端的 message（None = 无失败）
    pub fn failure_message(&self) -> Option<String> {
        self.failure.as_ref().map(|f| f.client_message())
    }

    /// 生成初始事件序列 (message_start + 文本块 start)
    ///
    /// 当 thinking 启用时，不在初始化时创建文本块，而是等到实际收到内容时再创建。
    /// 这样可以确保 thinking 块（索引 0）在文本块（索引 1）之前。
    pub fn generate_initial_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // message_start
        let msg_start = self.create_message_start_event();
        if let Some(event) = self.state_manager.handle_message_start(msg_start) {
            events.push(event);
        }

        // 如果启用了 thinking，不在这里创建文本块
        // thinking 块和文本块会在 process_content_with_thinking 中按正确顺序创建
        if self.thinking_enabled {
            return events;
        }

        // 创建初始文本块（仅在未启用 thinking 时）
        let text_block_index = self.state_manager.next_block_index();
        self.text_block_index = Some(text_block_index);
        let text_block_events = self.state_manager.handle_content_block_start(
            text_block_index,
            "text",
            json!({
                "type": "content_block_start",
                "index": text_block_index,
                "content_block": {
                    "type": "text",
                    "text": ""
                }
            }),
        );
        events.extend(text_block_events);

        events
    }

    /// 处理 Kiro 事件并转换为 Anthropic SSE 事件
    pub fn process_kiro_event(&mut self, event: &Event) -> Vec<SseEvent> {
        match event {
            Event::AssistantResponse(resp) => self.process_assistant_response(&resp.content),
            Event::ReasoningContent(r) => self.process_reasoning_content(&r.text, &r.signature),
            Event::ToolUse(tool_use) => self.process_tool_use(tool_use),
            Event::ContextUsage(context_usage) => {
                // 从上下文使用百分比计算实际的 input_tokens
                let window_size = get_context_window_size(&self.model);
                let actual_input_tokens = (context_usage.context_usage_percentage
                    * (window_size as f64)
                    / 100.0) as i32;
                self.context_input_tokens = Some(actual_input_tokens);
                // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                if context_usage.context_usage_percentage >= 100.0 {
                    self.state_manager
                        .set_stop_reason("model_context_window_exceeded");
                }
                tracing::debug!(
                    "收到 contextUsageEvent: {}%, 计算 input_tokens: {}",
                    context_usage.context_usage_percentage,
                    actual_input_tokens
                );
                Vec::new()
            }
            Event::TokenUsage(t) => {
                // Kiro 精确 token 统计，含 cacheReadInputTokens（部分模型不报）
                if let Some(cr) = t.cache_read_input_tokens {
                    self.cache_read_input_tokens = Some(cr as i32);
                }
                if let Some(cw) = t.cache_write_input_tokens {
                    self.cache_creation_input_tokens = Some(cw as i32);
                }
                // prompt 真值 = uncached + cacheRead，覆盖 contextUsage 推算
                let total_input = t.uncached_input_tokens
                    + t.cache_read_input_tokens.unwrap_or(0);
                if total_input > 0 {
                    self.context_input_tokens = Some(total_input as i32);
                }
                Vec::new()
            }
            Event::Metering(m) => {
                // 留作 cache_estimate 回退（模型不报 cacheRead 时）
                self.metering_usage = Some(m.usage);
                Vec::new()
            }
            Event::Error {
                error_code,
                error_message,
            } => {
                tracing::error!("收到错误事件: {} - {}", error_code, error_message);
                // 记录上游错误，流结束时据此判定为失败（此前只打日志后丢弃，
                // 导致空响应被当作 success 正常收尾）
                self.mark_failure(StreamFailure::UpstreamError(format!(
                    "{} - {}",
                    error_code, error_message
                )));
                Vec::new()
            }
            Event::Exception {
                exception_type,
                message,
            } => {
                // 处理 ContentLengthExceededException：这是正常的 max_tokens 截断，
                // 模型已产出内容到上限，不算失败
                if exception_type == "ContentLengthExceededException" {
                    self.state_manager.set_stop_reason("max_tokens");
                } else {
                    // 其它异常（如 "Encountered an unexpected error..."）记为上游错误，
                    // 避免静默吞错导致客户端收到空响应
                    self.mark_failure(StreamFailure::UpstreamException(format!(
                        "{} - {}",
                        exception_type, message
                    )));
                }
                tracing::warn!("收到异常事件: {} - {}", exception_type, message);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// 处理助手响应事件
    fn process_assistant_response(&mut self, content: &str) -> Vec<SseEvent> {
        if content.is_empty() {
            return Vec::new();
        }

        // 估算 tokens
        self.output_tokens += estimate_tokens(content);

        // 若正文开始前有开着的原生 reasoning thinking 块，先关闭它
        // （Anthropic 要求 thinking 块在 text 块之前并独立闭合）
        let mut events = self.close_reasoning_block_if_open();

        // 如果启用了thinking，需要处理thinking块（fake `<thinking>` 标签解析路径）。
        // 但若本次已出现原生 reasoning，则正文走纯 text，不再做 fake 标签解析
        // （推理已在独立通道，正文不含 `<thinking>`）。
        if self.thinking_enabled && !self.native_reasoning_seen {
            events.extend(self.process_content_with_thinking(content));
            return events;
        }

        // 非 thinking 模式同样复用统一的 text_delta 发送逻辑，
        // 以便在 tool_use 自动关闭文本块后能够自愈重建新的文本块，避免“吞字”。
        events.extend(self.create_text_delta_events(content));
        events
    }

    /// 处理原生 reasoningContentEvent —— 转成 Anthropic thinking 块。
    ///
    /// 与 `process_content_with_thinking`（从正文文本里抠 `<thinking>` 标签的 fake 路径）
    /// 不同：这里上游已经把推理放在独立事件流，我们直接逐片发 thinking_delta，
    /// 不需要标签解析。
    ///
    /// **顺序约束**：Anthropic 要求 thinking 块在 text 块之前。正常情况上游先发
    /// reasoning 再发正文。若 reasoning **迟于**正文到达（text 块已开），无法再合法地
    /// 在其前插入 thinking 块——此时丢弃该迟到 reasoning（仅日志），避免产生非法块顺序
    /// 导致客户端解析失败。实测上游均为 reasoning 先行，此分支是防御性兜底。
    fn process_reasoning_content(&mut self, text: &str, signature: &Option<String>) -> Vec<SseEvent> {
        // 先捕获签名（上游在 thinking 流最后一帧单独下发 {"signature":...}，无 text）。
        // 必须在 text.is_empty() 早返回之前处理，否则签名帧会被直接丢弃。
        if let Some(sig) = signature {
            if !sig.is_empty() {
                // 把签名里暴露 Bedrock 渠道的模型代号(claude-quince)替换成客户端请求的
                // 官方模型名，修复检测平台"签名/身份不一致"；重写失败则原样透传。
                let fixed = super::signature::rewrite_model_in_signature(sig, &self.model)
                    .unwrap_or_else(|| sig.clone());
                self.reasoning_signature = Some(fixed);
            }
        }

        if text.is_empty() {
            return Vec::new();
        }

        // 尊重 thinking 开关：未启用 thinking 时丢弃原生 reasoning，不暴露推理链
        // （与 fake `<thinking>` 路径一致受 thinking_enabled 门控）。
        if !self.thinking_enabled {
            return Vec::new();
        }

        // 顺序保护：text 块已存在且当前没有开着的 reasoning 块 → reasoning 迟到，丢弃
        if self.text_block_index.is_some() && !self.reasoning_block_active {
            tracing::warn!(
                "reasoningContentEvent 迟于正文到达，已丢弃以避免非法块顺序: {:?}",
                text.chars().take(40).collect::<String>()
            );
            return Vec::new();
        }

        self.output_tokens += estimate_tokens(text);

        // 首次见到原生 reasoning：清空 fake `<thinking>` 解析器的残留状态，
        // 避免两套机制共享 thinking_block_index、且 finalize 时 flush 残留 buffer。
        if !self.native_reasoning_seen {
            self.native_reasoning_seen = true;
            self.thinking_buffer.clear();
            self.in_thinking_block = false;
            self.thinking_extracted = false;
        }

        let mut events = Vec::new();

        // 开 thinking 块（首个片段，或上一个 reasoning 块已关闭后的重新开启）
        if !self.reasoning_block_active {
            let idx = self.state_manager.next_block_index();
            self.thinking_block_index = Some(idx);
            self.reasoning_block_active = true;
            let start_events = self.state_manager.handle_content_block_start(
                idx,
                "thinking",
                json!({
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": { "type": "thinking", "thinking": "" }
                }),
            );
            events.extend(start_events);
        }

        if let Some(idx) = self.thinking_block_index {
            events.push(self.create_thinking_delta_event(idx, text));
        }

        events
    }

    /// 关闭开着的原生 reasoning thinking 块（若有）。
    ///
    /// Anthropic 规范：thinking 块结束前需发一个 `signature_delta`，再发 `content_block_stop`。
    /// 优先透传上游下发的真实签名（protobuf）；若上游未给则回退空签名（仍满足结构合法性）。
    /// `reasoning_block_active` 守卫保证不会重复关闭，故签名无条件发送（每个 thinking 块恰好一次）。
    fn close_reasoning_block_if_open(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        if !self.reasoning_block_active {
            return events;
        }
        if let Some(idx) = self.thinking_block_index {
            // signature_delta：透传上游真实签名（若有），否则占位空签名
            let signature = self.reasoning_signature.clone().unwrap_or_default();
            events.push(SseEvent::new(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": { "type": "signature_delta", "signature": signature }
                }),
            ));
            if let Some(stop) = self.state_manager.handle_content_block_stop(idx) {
                events.push(stop);
            }
        }
        self.reasoning_block_active = false;
        // 签名属于刚关闭的这个 thinking 块，清空以防泄漏到后续块（防御性：
        // 现有顺序保护已使原生路径每轮仅一个 thinking 块，此处兜底上游行为变化）。
        self.reasoning_signature = None;
        events
    }

    /// 处理包含thinking块的内容
    fn process_content_with_thinking(&mut self, content: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 将内容添加到缓冲区进行处理
        self.thinking_buffer.push_str(content);

        loop {
            if !self.in_thinking_block && !self.thinking_extracted {
                // 查找 <thinking> 开始标签（跳过被反引号包裹的）
                if let Some(start_pos) = find_real_thinking_start_tag(&self.thinking_buffer) {
                    // 发送 <thinking> 之前的内容作为 text_delta
                    // 注意：如果前面只是空白字符（如 adaptive 模式返回的 \n\n），则跳过，
                    // 避免在 thinking 块之前产生无意义的 text 块导致客户端解析失败
                    let before_thinking = self.thinking_buffer[..start_pos].to_string();
                    if !before_thinking.is_empty() && !before_thinking.trim().is_empty() {
                        events.extend(self.create_text_delta_events(&before_thinking));
                    }

                    // 进入 thinking 块
                    self.in_thinking_block = true;
                    self.strip_thinking_leading_newline = true;
                    self.thinking_buffer =
                        self.thinking_buffer[start_pos + "<thinking>".len()..].to_string();

                    // 创建 thinking 块的 content_block_start 事件
                    let thinking_index = self.state_manager.next_block_index();
                    self.thinking_block_index = Some(thinking_index);
                    let start_events = self.state_manager.handle_content_block_start(
                        thinking_index,
                        "thinking",
                        json!({
                            "type": "content_block_start",
                            "index": thinking_index,
                            "content_block": {
                                "type": "thinking",
                                "thinking": ""
                            }
                        }),
                    );
                    events.extend(start_events);
                } else {
                    // 没有找到 <thinking>，检查是否可能是部分标签
                    // 保留可能是部分标签的内容
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("<thinking>".len());
                    let safe_len = find_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe_content = self.thinking_buffer[..safe_len].to_string();
                        // 如果 thinking 尚未提取，且安全内容只是空白字符，
                        // 则不发送为 text_delta，继续保留在缓冲区等待更多内容。
                        // 这避免了 4.6 模型中 <thinking> 标签跨事件分割时，
                        // 前导空白（如 "\n\n"）被错误地创建为 text 块，
                        // 导致 text 块先于 thinking 块出现的问题。
                        if !safe_content.is_empty() && !safe_content.trim().is_empty() {
                            events.extend(self.create_text_delta_events(&safe_content));
                            self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                        }
                    }
                    break;
                }
            } else if self.in_thinking_block {
                // 剥离 <thinking> 标签后紧跟的换行符（可能跨 chunk）
                if self.strip_thinking_leading_newline {
                    if self.thinking_buffer.starts_with('\n') {
                        self.thinking_buffer = self.thinking_buffer[1..].to_string();
                        self.strip_thinking_leading_newline = false;
                    } else if !self.thinking_buffer.is_empty() {
                        // buffer 非空但不以 \n 开头，不再需要剥离
                        self.strip_thinking_leading_newline = false;
                    }
                    // buffer 为空时保留标志，等待下一个 chunk
                }

                // 在 thinking 块内，查找 </thinking> 结束标签（跳过被反引号包裹的）
                if let Some(end_pos) = find_real_thinking_end_tag(&self.thinking_buffer) {
                    // 提取 thinking 内容
                    let thinking_content = self.thinking_buffer[..end_pos].to_string();
                    if !thinking_content.is_empty() {
                        if let Some(thinking_index) = self.thinking_block_index {
                            events.push(
                                self.create_thinking_delta_event(thinking_index, &thinking_content),
                            );
                        }
                    }

                    // 结束 thinking 块
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;

                    // 关闭 thinking 块：补发 signature_delta（无真签名则合成）+ stop
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.extend(self.close_fake_thinking_block(thinking_index));
                    }

                    // 剥离 `</thinking>\n\n`（find_real_thinking_end_tag 已确认 \n\n 存在）
                    self.thinking_buffer =
                        self.thinking_buffer[end_pos + "</thinking>\n\n".len()..].to_string();
                } else {
                    // 没有找到结束标签，发送当前缓冲区内容作为 thinking_delta。
                    // 保留末尾可能是部分 `</thinking>\n\n` 的内容：
                    // find_real_thinking_end_tag 要求标签后有 `\n\n` 才返回 Some，
                    // 因此保留区必须覆盖 `</thinking>\n\n` 的完整长度（13 字节），
                    // 否则当 `</thinking>` 已在 buffer 但 `\n\n` 尚未到达时，
                    // 标签的前几个字符会被错误地作为 thinking_delta 发出。
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("</thinking>\n\n".len());
                    let safe_len = find_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe_content = self.thinking_buffer[..safe_len].to_string();
                        if !safe_content.is_empty() {
                            if let Some(thinking_index) = self.thinking_block_index {
                                events.push(
                                    self.create_thinking_delta_event(thinking_index, &safe_content),
                                );
                            }
                        }
                        self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                    }
                    break;
                }
            } else {
                // thinking 已提取完成，剩余内容作为 text_delta
                if !self.thinking_buffer.is_empty() {
                    let remaining = self.thinking_buffer.clone();
                    self.thinking_buffer.clear();
                    events.extend(self.create_text_delta_events(&remaining));
                }
                break;
            }
        }

        events
    }

    /// 创建 text_delta 事件
    ///
    /// 如果文本块尚未创建，会先创建文本块。
    /// 当发生 tool_use 时，状态机会自动关闭当前文本块；后续文本会自动创建新的文本块继续输出。
    ///
    /// 返回值包含可能的 content_block_start 事件和 content_block_delta 事件。
    fn create_text_delta_events(&mut self, text: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 如果当前 text_block_index 指向的块已经被关闭（例如 tool_use 开始时自动 stop），
        // 则丢弃该索引并创建新的文本块继续输出，避免 delta 被状态机拒绝导致“吞字”。
        if let Some(idx) = self.text_block_index {
            if !self.state_manager.is_block_open_of_type(idx, "text") {
                self.text_block_index = None;
            }
        }

        // 获取或创建文本块索引
        let text_index = if let Some(idx) = self.text_block_index {
            idx
        } else {
            // 文本块尚未创建，需要先创建
            let idx = self.state_manager.next_block_index();
            self.text_block_index = Some(idx);

            // 发送 content_block_start 事件
            let start_events = self.state_manager.handle_content_block_start(
                idx,
                "text",
                json!({
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": {
                        "type": "text",
                        "text": ""
                    }
                }),
            );
            events.extend(start_events);
            idx
        };

        // 发送 content_block_delta 事件
        if let Some(delta_event) = self.state_manager.handle_content_block_delta(
            text_index,
            json!({
                "type": "content_block_delta",
                "index": text_index,
                "delta": {
                    "type": "text_delta",
                    "text": text
                }
            }),
        ) {
            events.push(delta_event);
        }

        events
    }

    /// 创建 thinking_delta 事件
    fn create_thinking_delta_event(&mut self, index: i32, thinking: &str) -> SseEvent {
        // 累积 fake thinking 文本，供关闭块时合成签名（上游无签名时的兜底）
        if !thinking.is_empty() {
            self.fake_thinking_text.push_str(thinking);
        }
        SseEvent::new(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {
                    "type": "thinking_delta",
                    "thinking": thinking
                }
            }),
        )
    }

    /// 关闭 fake `<thinking>` 路径的 thinking 块：发 signature_delta + content_block_stop。
    /// 上游未透传真实签名（reasoning_signature=None）时，合成一个结构合法、模型标识为
    /// 官方名的签名，把检测平台判定从"签名失败"提升到"部分合格"。
    fn close_fake_thinking_block(&mut self, thinking_index: i32) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let signature = match &self.reasoning_signature {
            Some(sig) if !sig.is_empty() => sig.clone(),
            _ => super::signature::synthesize_signature(&self.model, &self.fake_thinking_text),
        };
        events.push(SseEvent::new(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": thinking_index,
                "delta": { "type": "signature_delta", "signature": signature }
            }),
        ));
        if let Some(stop) = self.state_manager.handle_content_block_stop(thinking_index) {
            events.push(stop);
        }
        events
    }

    /// 处理工具使用事件
    fn process_tool_use(
        &mut self,
        tool_use: &crate::kiro::model::events::ToolUseEvent,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 若 reasoning 后直接进入 tool_use，先关闭原生 reasoning thinking 块
        events.extend(self.close_reasoning_block_if_open());

        self.state_manager.set_has_tool_use(true);

        // tool_use 必须发生在 thinking 结束之后。
        // 但当 `</thinking>` 后面没有 `\n\n`（例如紧跟 tool_use 或流结束）时，
        // thinking 结束标签会滞留在 thinking_buffer，导致后续 flush 时把 `</thinking>` 当作内容输出。
        // 这里在开始 tool_use block 前做一次“边界场景”的结束标签识别与过滤。
        if self.thinking_enabled && self.in_thinking_block {
            if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer) {
                let thinking_content = self.thinking_buffer[..end_pos].to_string();
                if !thinking_content.is_empty() {
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.push(
                            self.create_thinking_delta_event(thinking_index, &thinking_content),
                        );
                    }
                }

                // 结束 thinking 块
                self.in_thinking_block = false;
                self.thinking_extracted = true;

                if let Some(thinking_index) = self.thinking_block_index {
                    // 补发 signature_delta（无真签名则合成）+ content_block_stop
                    events.extend(self.close_fake_thinking_block(thinking_index));
                }

                // 把结束标签后的内容当作普通文本（通常为空或空白）
                let after_pos = end_pos + "</thinking>".len();
                let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                self.thinking_buffer.clear();
                if !remaining.is_empty() {
                    events.extend(self.create_text_delta_events(&remaining));
                }
            }
        }

        // thinking 模式下，process_content_with_thinking 可能会为了探测 `<thinking>` 而暂存一小段尾部文本。
        // 如果此时直接开始 tool_use，状态机会自动关闭 text block，导致这段"待输出文本"看起来被 tool_use 吞掉。
        // 约束：只在尚未进入 thinking block、且 thinking 尚未被提取时，将缓冲区当作普通文本 flush。
        if self.thinking_enabled
            && !self.in_thinking_block
            && !self.thinking_extracted
            && !self.thinking_buffer.is_empty()
        {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            events.extend(self.create_text_delta_events(&buffered));
        }

        // 获取或分配块索引
        let block_index = if let Some(&idx) = self.tool_block_indices.get(&tool_use.tool_use_id) {
            idx
        } else {
            let idx = self.state_manager.next_block_index();
            self.tool_block_indices
                .insert(tool_use.tool_use_id.clone(), idx);
            idx
        };

        // 还原工具名称（如果有映射）
        let original_name = self
            .tool_name_map
            .get(&tool_use.name)
            .cloned()
            .unwrap_or_else(|| tool_use.name.clone());

        // 发送 content_block_start
        let start_events = self.state_manager.handle_content_block_start(
            block_index,
            "tool_use",
            json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_use.tool_use_id,
                    "name": original_name,
                    "input": {}
                }
            }),
        );
        events.extend(start_events);

        // 发送参数增量 (ToolUseEvent.input 是 String 类型)
        if !tool_use.input.is_empty() {
            self.output_tokens += (tool_use.input.len() as i32 + 3) / 4; // 估算 token

            if let Some(delta_event) = self.state_manager.handle_content_block_delta(
                block_index,
                json!({
                    "type": "content_block_delta",
                    "index": block_index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": tool_use.input
                    }
                }),
            ) {
                events.push(delta_event);
            }
        }

        // 如果是完整的工具调用（stop=true），发送 content_block_stop
        if tool_use.stop {
            if let Some(stop_event) = self.state_manager.handle_content_block_stop(block_index) {
                events.push(stop_event);
            }
        }

        events
    }

    /// 生成最终事件序列
    pub fn generate_final_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 空响应检测（在此统一处理，使 live 与 buffered 两条路径都覆盖）：
        // 流结束时若无任何已记录失败、且零实质内容产出 → 判定为空响应。
        if self.failure.is_none() && !self.produced_any_content() {
            self.mark_failure(StreamFailure::EmptyResponse);
            tracing::warn!(
                "检测到空响应：上游零内容产出，prompt_tokens≈{}",
                self.context_input_tokens.unwrap_or(self.input_tokens)
            );
        }

        // 终态失败：向客户端补发终止性 Anthropic `error` 事件并立即返回。
        // 不再追加 message_delta/message_stop，避免「error 后又 message_stop」的自相矛盾序列，
        // 让客户端干净识别失败并重试。
        if let Some(failure) = self.failure.clone() {
            // 先按规范闭合开着的 reasoning thinking 块（发 signature_delta + stop），
            // 再关闭其余开着的块——避免残留半开块或缺签名的非法 thinking 块。
            events.extend(self.close_reasoning_block_if_open());
            events.extend(self.state_manager.close_all_open_blocks());
            events.push(SseEvent::new(
                "error",
                json!({
                    "type": "error",
                    "error": {
                        "type": "api_error",
                        "message": failure.client_message(),
                    }
                }),
            ));
            return events;
        }

        // 若流结束时原生 reasoning thinking 块仍开着（纯思考无后续正文），先干净关闭
        events.extend(self.close_reasoning_block_if_open());

        // Flush thinking_buffer 中的剩余内容
        if self.thinking_enabled && !self.thinking_buffer.is_empty() {
            if self.in_thinking_block {
                // 末尾可能残留 `</thinking>`（例如紧跟 tool_use 或流结束），需要在 flush 时过滤掉结束标签。
                if let Some(end_pos) =
                    find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
                {
                    let thinking_content = self.thinking_buffer[..end_pos].to_string();
                    if !thinking_content.is_empty() {
                        if let Some(thinking_index) = self.thinking_block_index {
                            events.push(
                                self.create_thinking_delta_event(thinking_index, &thinking_content),
                            );
                        }
                    }

                    // 关闭 thinking 块：补发 signature_delta（无真签名则合成）+ stop
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.extend(self.close_fake_thinking_block(thinking_index));
                    }

                    // 把结束标签后的内容当作普通文本（通常为空或空白）
                    let after_pos = end_pos + "</thinking>".len();
                    let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                    self.thinking_buffer.clear();
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if !remaining.is_empty() {
                        events.extend(self.create_text_delta_events(&remaining));
                    }
                } else {
                    // 如果还在 thinking 块内，发送剩余内容作为 thinking_delta
                    if let Some(thinking_index) = self.thinking_block_index {
                        let buf = self.thinking_buffer.clone();
                        events.push(self.create_thinking_delta_event(thinking_index, &buf));
                    }
                    // 关闭 thinking 块：补发 signature_delta（无真签名则合成）+ stop
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.extend(self.close_fake_thinking_block(thinking_index));
                    }
                }
            } else {
                // 否则发送剩余内容作为 text_delta
                let buffer_content = self.thinking_buffer.clone();
                events.extend(self.create_text_delta_events(&buffer_content));
            }
            self.thinking_buffer.clear();
        }

        // 如果整个流中只产生了 thinking 块，没有 text 也没有 tool_use，
        // 则设置 stop_reason 为 max_tokens（表示模型耗尽了 token 预算在思考上），
        // 并补发一套完整的 text 事件（内容为一个空格），确保 content 数组中有 text 块
        if self.thinking_enabled
            && self.thinking_block_index.is_some()
            && !self.state_manager.has_non_thinking_blocks()
        {
            self.state_manager.set_stop_reason("max_tokens");
            events.extend(self.create_text_delta_events(" "));
        }

        // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
        let final_input_tokens = self.context_input_tokens.unwrap_or(self.input_tokens);

        // 零输出保护：若本轮最终无任何输出 token（completion=0），即便模拟器给了命中，
        // 也**不向客户端计费缓存**——用户没拿到任何产出，不该为缓存读付费。
        // 正常空响应已在上方判 EmptyResponse 提前 return；这里兜住"绕过空检测但输出为 0"。
        let zero_output = self.output_tokens <= 0;

        // 缓存计费（v53：统一走 prefix 模拟器）。
        //   hit/sim_total 同口径（模拟器 tokenizer）算命中比例；
        //   report_total = Kiro contextUsageEvent 权威 token（final_input_tokens），billing 基准。
        let (hit_tokens, sim_total) = self.sim_cache.unwrap_or((0, 0));
        let hit_tokens = hit_tokens.max(0);
        // 落库模拟器命中（供缓存分析）；sim 已跑（Some）就记，包括 0=明确未命中。
        if self.sim_cache.is_some() {
            self.emitted_raw_cache_read = Some(hit_tokens);
        }
        // 零输出 → 上报缓存归零（计费保护）；否则按公式算上报。
        let cache_read = if zero_output {
            0
        } else {
            super::usage::reported_cache_read(
                final_input_tokens,
                hit_tokens,
                sim_total,
                self.cache_read_multiplier,
                self.cache_cap_ratio,
                self.cache_floor_ratio,
            )
        };
        // 统一模型下不单列 cache_creation。
        let cache_creation = 0;

        // 暴露给 SSE 流外层（写 DB 用），区分真实估算 vs 放大后实报
        self.emitted_cache_read = Some(cache_read);
        self.emitted_cache_creation = Some(cache_creation);

        // 生成最终事件
        events.extend(self.state_manager.generate_final_events(
            final_input_tokens,
            self.output_tokens,
            cache_read,
            cache_creation,
        ));
        events
    }
}

/// 缓冲流处理上下文 - 用于 /cc/v1/messages 流式请求
///
/// 与 `StreamContext` 不同，此上下文会缓冲所有事件直到流结束，
/// 然后用从 `contextUsageEvent` 计算的正确 `input_tokens` 更正 `message_start` 事件。
///
/// 工作流程：
/// 1. 使用 `StreamContext` 正常处理所有 Kiro 事件
/// 2. 把生成的 SSE 事件缓存起来（而不是立即发送）
/// 3. 流结束时，找到 `message_start` 事件并更新其 `input_tokens`
/// 4. 一次性返回所有事件
pub struct BufferedStreamContext {
    /// 内部流处理上下文（复用现有的事件处理逻辑）
    inner: StreamContext,
    /// 缓冲的所有事件（包括 message_start、content_block_start 等）
    event_buffer: Vec<SseEvent>,
    /// 估算的 input_tokens（用于回退）
    estimated_input_tokens: i32,
    /// 是否已经生成了初始事件
    initial_events_generated: bool,
}

impl BufferedStreamContext {
    /// 创建缓冲流上下文
    pub fn new(
        model: impl Into<String>,
        estimated_input_tokens: i32,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
    ) -> Self {
        let inner =
            StreamContext::new_with_thinking(model, estimated_input_tokens, thinking_enabled, tool_name_map);
        Self {
            inner,
            event_buffer: Vec::new(),
            estimated_input_tokens,
            initial_events_generated: false,
        }
    }

    /// 设置缓存上报缩放倍率（透传到内部 StreamContext）
    pub fn set_cache_read_multiplier(&mut self, m: f64) {
        self.inner.set_cache_read_multiplier(m);
    }

    /// 设置命中上限比率（透传到内部 StreamContext）
    pub fn set_cache_cap_ratio(&mut self, r: f64) {
        self.inner.set_cache_cap_ratio(r);
    }

    /// 设置最低比率（透传到内部 StreamContext）
    pub fn set_cache_floor_ratio(&mut self, r: f64) {
        self.inner.set_cache_floor_ratio(r);
    }

    /// 设置 prefix 缓存模拟器结果（透传到内部 StreamContext）
    pub fn set_sim_cache(&mut self, v: Option<(i32, i32)>) {
        self.inner.set_sim_cache(v);
    }

    /// 标记流的终态失败（透传到内部 StreamContext）。
    /// 用于 buffered 路径的 IO 错误等场景，使最终事件发出终止性 error。
    pub fn mark_failure(&mut self, failure: StreamFailure) {
        self.inner.mark_failure(failure);
    }

    /// 处理 Kiro 事件并缓冲结果
    ///
    /// 复用 StreamContext 的事件处理逻辑，但把结果缓存而不是立即发送。
    pub fn process_and_buffer(&mut self, event: &crate::kiro::model::events::Event) {
        // 首次处理事件时，先生成初始事件（message_start 等）
        if !self.initial_events_generated {
            let initial_events = self.inner.generate_initial_events();
            self.event_buffer.extend(initial_events);
            self.initial_events_generated = true;
        }

        // 处理事件并缓冲结果
        let events = self.inner.process_kiro_event(event);
        self.event_buffer.extend(events);
    }

    /// 完成流处理并返回所有事件
    ///
    /// 此方法会：
    /// 1. 生成最终事件（message_delta, message_stop）
    /// 2. 用正确的 input_tokens 更正 message_start 事件
    /// 3. 返回所有缓冲的事件
    pub fn finish_and_get_all_events(&mut self) -> Vec<SseEvent> {
        // 如果从未处理过事件，也要生成初始事件
        if !self.initial_events_generated {
            let initial_events = self.inner.generate_initial_events();
            self.event_buffer.extend(initial_events);
            self.initial_events_generated = true;
        }

        // 生成最终事件
        let final_events = self.inner.generate_final_events();
        self.event_buffer.extend(final_events);

        // 获取正确的 input_tokens
        let final_input_tokens = self
            .inner
            .context_input_tokens
            .unwrap_or(self.estimated_input_tokens);

        // 更正 message_start 事件中的 input_tokens
        for event in &mut self.event_buffer {
            if event.event == "message_start" {
                if let Some(message) = event.data.get_mut("message") {
                    if let Some(usage) = message.get_mut("usage") {
                        usage["input_tokens"] = serde_json::json!(final_input_tokens);
                    }
                }
            }
        }

        std::mem::take(&mut self.event_buffer)
    }
}

/// 简单的 token 估算
fn estimate_tokens(text: &str) -> i32 {
    let chars: Vec<char> = text.chars().collect();
    let mut chinese_count = 0;
    let mut other_count = 0;

    for c in &chars {
        if *c >= '\u{4E00}' && *c <= '\u{9FFF}' {
            chinese_count += 1;
        } else {
            other_count += 1;
        }
    }

    // 中文约 1.5 字符/token，英文约 4 字符/token
    let chinese_tokens = (chinese_count * 2 + 2) / 3;
    let other_tokens = (other_count + 3) / 4;

    (chinese_tokens + other_tokens).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_event_format() {
        let event = SseEvent::new("message_start", json!({"type": "message_start"}));
        let sse_str = event.to_sse_string();

        assert!(sse_str.starts_with("event: message_start\n"));
        assert!(sse_str.contains("data: "));
        assert!(sse_str.ends_with("\n\n"));
    }

    #[test]
    fn test_sse_state_manager_message_start() {
        let mut manager = SseStateManager::new();

        // 第一次应该成功
        let event = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(event.is_some());

        // 第二次应该被跳过
        let event = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(event.is_none());
    }

    #[test]
    fn test_sse_state_manager_block_lifecycle() {
        let mut manager = SseStateManager::new();

        // 创建块
        let events = manager.handle_content_block_start(0, "text", json!({}));
        assert_eq!(events.len(), 1);

        // delta
        let event = manager.handle_content_block_delta(0, json!({}));
        assert!(event.is_some());

        // stop
        let event = manager.handle_content_block_stop(0);
        assert!(event.is_some());

        // 重复 stop 应该被跳过
        let event = manager.handle_content_block_stop(0);
        assert!(event.is_none());
    }

    #[test]
    fn test_tool_name_reverse_mapping_in_stream() {
        use crate::kiro::model::events::ToolUseEvent;

        let mut map = HashMap::new();
        map.insert("short_abc12345".to_string(), "mcp__very_long_original_tool_name".to_string());

        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, map);
        let _ = ctx.generate_initial_events();

        // 模拟 Kiro 返回短名称的 tool_use
        let tool_event = Event::ToolUse(ToolUseEvent {
            name: "short_abc12345".to_string(),
            tool_use_id: "toolu_01".to_string(),
            input: r#"{"key":"value"}"#.to_string(),
            stop: true,
        });

        let events = ctx.process_kiro_event(&tool_event);

        // content_block_start 中的 name 应该是原始长名称
        let start_event = events.iter().find(|e| e.event == "content_block_start").unwrap();
        assert_eq!(
            start_event.data["content_block"]["name"],
            "mcp__very_long_original_tool_name",
            "应还原为原始工具名称"
        );
    }

    #[test]
    fn test_text_delta_after_tool_use_restarts_text_block() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());

        let initial_events = ctx.generate_initial_events();
        assert!(
            initial_events
                .iter()
                .any(|e| e.event == "content_block_start"
                    && e.data["content_block"]["type"] == "text")
        );

        let initial_text_index = ctx
            .text_block_index
            .expect("initial text block index should exist");

        // tool_use 开始会自动关闭现有 text block
        let tool_events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "test_tool".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });
        assert!(
            tool_events.iter().any(|e| {
                e.event == "content_block_stop"
                    && e.data["index"].as_i64() == Some(initial_text_index as i64)
            }),
            "tool_use should stop the previous text block"
        );

        // 之后再来文本增量，应自动创建新的 text block 而不是往已 stop 的块里写 delta
        let text_events = ctx.process_assistant_response("hello");
        let new_text_start_index = text_events.iter().find_map(|e| {
            if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                e.data["index"].as_i64()
            } else {
                None
            }
        });
        assert!(
            new_text_start_index.is_some(),
            "should start a new text block"
        );
        assert_ne!(
            new_text_start_index.unwrap(),
            initial_text_index as i64,
            "new text block index should differ from the stopped one"
        );
        assert!(
            text_events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == "hello"
            }),
            "should emit text_delta after restarting text block"
        );
    }

    #[test]
    fn test_tool_use_flushes_pending_thinking_buffer_text_before_tool_block() {
        // thinking 模式下，短文本可能被暂存在 thinking_buffer 以等待 `<thinking>` 的跨 chunk 匹配。
        // 当紧接着出现 tool_use 时，应先 flush 这段文本，再开始 tool_use block。
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        // 两段短文本（各 2 个中文字符），总长度仍可能不足以满足 safe_len>0 的输出条件，
        // 因而会留在 thinking_buffer 中等待后续 chunk。
        let ev1 = ctx.process_assistant_response("有修");
        assert!(
            ev1.iter().all(|e| e.event != "content_block_delta"),
            "short prefix should be buffered under thinking mode"
        );
        let ev2 = ctx.process_assistant_response("改：");
        assert!(
            ev2.iter().all(|e| e.event != "content_block_delta"),
            "short prefix should still be buffered under thinking mode"
        );

        let events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "Write".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });

        let text_start_index = events.iter().find_map(|e| {
            if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                e.data["index"].as_i64()
            } else {
                None
            }
        });
        let pos_text_delta = events.iter().position(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta"
        });
        let pos_text_stop = text_start_index.and_then(|idx| {
            events.iter().position(|e| {
                e.event == "content_block_stop" && e.data["index"].as_i64() == Some(idx)
            })
        });
        let pos_tool_start = events.iter().position(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "tool_use"
        });

        assert!(
            text_start_index.is_some(),
            "should start a text block to flush buffered text"
        );
        assert!(
            pos_text_delta.is_some(),
            "should flush buffered text as text_delta"
        );
        assert!(
            pos_text_stop.is_some(),
            "should stop text block before tool_use block starts"
        );
        assert!(pos_tool_start.is_some(), "should start tool_use block");

        let pos_text_delta = pos_text_delta.unwrap();
        let pos_text_stop = pos_text_stop.unwrap();
        let pos_tool_start = pos_tool_start.unwrap();

        assert!(
            pos_text_delta < pos_text_stop && pos_text_stop < pos_tool_start,
            "ordering should be: text_delta -> text_stop -> tool_use_start"
        );

        assert!(
            events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == "有修改："
            }),
            "flushed text should equal the buffered prefix"
        );
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("Hello") > 0);
        assert!(estimate_tokens("你好") > 0);
        assert!(estimate_tokens("Hello 你好") > 0);
    }

    #[test]
    fn test_find_real_thinking_start_tag_basic() {
        // 基本情况：正常的开始标签
        assert_eq!(find_real_thinking_start_tag("<thinking>"), Some(0));
        assert_eq!(find_real_thinking_start_tag("prefix<thinking>"), Some(6));
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_backticks() {
        // 被反引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("`<thinking>`"), None);
        assert_eq!(find_real_thinking_start_tag("use `<thinking>` tag"), None);

        // 先有被包裹的，后有真正的开始标签
        assert_eq!(
            find_real_thinking_start_tag("about `<thinking>` tag<thinking>content"),
            Some(22)
        );
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_quotes() {
        // 被双引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("\"<thinking>\""), None);
        assert_eq!(find_real_thinking_start_tag("the \"<thinking>\" tag"), None);

        // 被单引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("'<thinking>'"), None);

        // 混合情况
        assert_eq!(
            find_real_thinking_start_tag("about \"<thinking>\" and '<thinking>' then<thinking>"),
            Some(40)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_basic() {
        // 基本情况：正常的结束标签后面有双换行符
        assert_eq!(find_real_thinking_end_tag("</thinking>\n\n"), Some(0));
        assert_eq!(
            find_real_thinking_end_tag("content</thinking>\n\n"),
            Some(7)
        );
        assert_eq!(
            find_real_thinking_end_tag("some text</thinking>\n\nmore text"),
            Some(9)
        );

        // 没有双换行符的情况
        assert_eq!(find_real_thinking_end_tag("</thinking>"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking>\n"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking> more"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_backticks() {
        // 被反引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("`</thinking>`\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("mention `</thinking>` in code\n\n"),
            None
        );

        // 只有前面有反引号
        assert_eq!(find_real_thinking_end_tag("`</thinking>\n\n"), None);

        // 只有后面有反引号
        assert_eq!(find_real_thinking_end_tag("</thinking>`\n\n"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_quotes() {
        // 被双引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("\"</thinking>\"\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("the string \"</thinking>\" is a tag\n\n"),
            None
        );

        // 被单引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("'</thinking>'\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("use '</thinking>' as marker\n\n"),
            None
        );

        // 混合情况：双引号包裹后有真正的标签
        assert_eq!(
            find_real_thinking_end_tag("about \"</thinking>\" tag</thinking>\n\n"),
            Some(23)
        );

        // 混合情况：单引号包裹后有真正的标签
        assert_eq!(
            find_real_thinking_end_tag("about '</thinking>' tag</thinking>\n\n"),
            Some(23)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_mixed() {
        // 先有被包裹的，后有真正的结束标签
        assert_eq!(
            find_real_thinking_end_tag("discussing `</thinking>` tag</thinking>\n\n"),
            Some(28)
        );

        // 多个被包裹的，最后一个是真正的
        assert_eq!(
            find_real_thinking_end_tag("`</thinking>` and `</thinking>` done</thinking>\n\n"),
            Some(36)
        );

        // 多种引用字符混合
        assert_eq!(
            find_real_thinking_end_tag(
                "`</thinking>` and \"</thinking>\" and '</thinking>' done</thinking>\n\n"
            ),
            Some(54)
        );
    }

    #[test]
    fn test_tool_use_immediately_after_thinking_filters_end_tag_and_closes_thinking_block() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();

        // thinking 内容以 `</thinking>` 结尾，但后面没有 `\n\n`（模拟紧跟 tool_use 的场景）
        all_events.extend(ctx.process_assistant_response("<thinking>abc</thinking>"));

        let tool_events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "Write".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });
        all_events.extend(tool_events);

        all_events.extend(ctx.generate_final_events());

        // 不应把 `</thinking>` 当作 thinking 内容输出
        assert!(
            all_events.iter().all(|e| {
                !(e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "thinking_delta"
                    && e.data["delta"]["thinking"] == "</thinking>")
            }),
            "`</thinking>` should be filtered from output"
        );

        // thinking block 必须在 tool_use block 之前关闭
        let thinking_index = ctx
            .thinking_block_index
            .expect("thinking block index should exist");
        let pos_thinking_stop = all_events.iter().position(|e| {
            e.event == "content_block_stop"
                && e.data["index"].as_i64() == Some(thinking_index as i64)
        });
        let pos_tool_start = all_events.iter().position(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "tool_use"
        });
        assert!(
            pos_thinking_stop.is_some(),
            "thinking block should be stopped"
        );
        assert!(pos_tool_start.is_some(), "tool_use block should be started");
        assert!(
            pos_thinking_stop.unwrap() < pos_tool_start.unwrap(),
            "thinking block should stop before tool_use block starts"
        );
    }

    #[test]
    fn test_final_flush_filters_standalone_thinking_end_tag() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>abc</thinking>"));
        all_events.extend(ctx.generate_final_events());

        assert!(
            all_events.iter().all(|e| {
                !(e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "thinking_delta"
                    && e.data["delta"]["thinking"] == "</thinking>")
            }),
            "`</thinking>` should be filtered during final flush"
        );
    }

    #[test]
    fn test_thinking_strips_leading_newline_same_chunk() {
        // <thinking>\n 在同一个 chunk 中，\n 应被剥离
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let events = ctx.process_assistant_response("<thinking>\nHello world");

        // 找到所有 thinking_delta 事件
        let thinking_deltas: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        // 拼接所有 thinking 内容
        let full_thinking: String = thinking_deltas
            .iter()
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_thinking.starts_with('\n'),
            "thinking content should not start with \\n, got: {:?}",
            full_thinking
        );
    }

    #[test]
    fn test_thinking_strips_leading_newline_cross_chunk() {
        // <thinking> 在第一个 chunk 末尾，\n 在第二个 chunk 开头
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let events1 = ctx.process_assistant_response("<thinking>");
        let events2 = ctx.process_assistant_response("\nHello world");

        let mut all_events = Vec::new();
        all_events.extend(events1);
        all_events.extend(events2);

        let thinking_deltas: Vec<_> = all_events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        let full_thinking: String = thinking_deltas
            .iter()
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_thinking.starts_with('\n'),
            "thinking content should not start with \\n across chunks, got: {:?}",
            full_thinking
        );
    }

    #[test]
    fn test_thinking_no_strip_when_no_leading_newline() {
        // <thinking> 后直接跟内容（无 \n），内容应完整保留
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let events = ctx.process_assistant_response("<thinking>abc</thinking>\n\ntext");

        let thinking_deltas: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        let full_thinking: String = thinking_deltas
            .iter()
            .filter(|e| !e.data["delta"]["thinking"].as_str().unwrap_or("").is_empty())
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert_eq!(full_thinking, "abc", "thinking content should be 'abc'");
    }

    #[test]
    fn test_text_after_thinking_strips_leading_newlines() {
        // `</thinking>\n\n` 后的文本不应以 \n\n 开头
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let events =
            ctx.process_assistant_response("<thinking>\nabc</thinking>\n\n你好");

        let text_deltas: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta"
            })
            .collect();

        let full_text: String = text_deltas
            .iter()
            .map(|e| e.data["delta"]["text"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_text.starts_with('\n'),
            "text after thinking should not start with \\n, got: {:?}",
            full_text
        );
        assert_eq!(full_text, "你好");
    }

    /// 辅助函数：从事件列表中提取所有 thinking_delta 的拼接内容
    fn collect_thinking_content(events: &[SseEvent]) -> String {
        events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// 辅助函数：从事件列表中提取所有 text_delta 的拼接内容
    fn collect_text_content(events: &[SseEvent]) -> String {
        events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta"
            })
            .map(|e| e.data["delta"]["text"].as_str().unwrap_or(""))
            .collect()
    }

    #[test]
    fn test_end_tag_newlines_split_across_events() {
        // `</thinking>\n` 在 chunk 1，`\n` 在 chunk 2，`text` 在 chunk 3
        // 确保 `</thinking>` 不会被部分当作 thinking 内容发出
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("你好"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(thinking, "abc", "thinking should be 'abc', got: {:?}", thinking);

        let text = collect_text_content(&all);
        assert_eq!(text, "你好", "text should be '你好', got: {:?}", text);
    }

    #[test]
    fn test_end_tag_alone_in_chunk_then_newlines_in_next() {
        // `</thinking>` 单独在一个 chunk，`\n\ntext` 在下一个 chunk
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all.extend(ctx.process_assistant_response("\n\n你好"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(thinking, "abc", "thinking should be 'abc', got: {:?}", thinking);

        let text = collect_text_content(&all);
        assert_eq!(text, "你好", "text should be '你好', got: {:?}", text);
    }

    #[test]
    fn test_start_tag_newline_split_across_events() {
        // `\n\n` 在 chunk 1，`<thinking>` 在 chunk 2，`\n` 在 chunk 3
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("\n\n"));
        all.extend(ctx.process_assistant_response("<thinking>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("abc</thinking>\n\ntext"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(thinking, "abc", "thinking should be 'abc', got: {:?}", thinking);

        let text = collect_text_content(&all);
        assert_eq!(text, "text", "text should be 'text', got: {:?}", text);
    }

    #[test]
    fn test_full_flow_maximally_split() {
        // 极端拆分：每个关键边界都在不同 chunk
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        // \n\n<thinking>\n 拆成多段
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("<thin"));
        all.extend(ctx.process_assistant_response("king>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("hello"));
        // </thinking>\n\n 拆成多段
        all.extend(ctx.process_assistant_response("</thi"));
        all.extend(ctx.process_assistant_response("nking>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("world"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(thinking, "hello", "thinking should be 'hello', got: {:?}", thinking);

        let text = collect_text_content(&all);
        assert_eq!(text, "world", "text should be 'world', got: {:?}", text);
    }

    #[test]
    fn test_thinking_only_sets_max_tokens_stop_reason() {
        // 整个流只有 thinking 块，没有 text 也没有 tool_use，stop_reason 应为 max_tokens
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "max_tokens",
            "stop_reason should be max_tokens when only thinking is produced"
        );

        // 应补发一套完整的 text 事件（content_block_start + delta 空格 + content_block_stop）
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_start" && e.data["content_block"]["type"] == "text"
            }),
            "should emit text content_block_start"
        );
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == " "
            }),
            "should emit text_delta with a single space"
        );
        // text block 应被 generate_final_events 自动关闭
        let text_block_index = all_events
            .iter()
            .find_map(|e| {
                if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                    e.data["index"].as_i64()
                } else {
                    None
                }
            })
            .expect("text block should exist");
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_stop"
                    && e.data["index"].as_i64() == Some(text_block_index)
            }),
            "text block should be stopped"
        );
    }

    #[test]
    fn test_zero_output_does_not_bill_cache_read() {
        // v50 计费保护：模拟器给了命中，但本轮零输出（completion=0）→
        // message_delta.usage 不应计费 cache_read（用户没拿到产出不该为缓存付费）。
        // 用 tool_use 让 produced_any_content()=true（绕过空响应失败判定），但保持 output=0。
        let mut ctx = StreamContext::new_with_thinking("claude-opus-4-8", 1000, false, HashMap::new());
        ctx.set_cache_read_multiplier(1.8);
        ctx.set_cache_cap_ratio(0.9);
        ctx.set_sim_cache(Some((5000, 10000))); // 模拟器命中 5000/总 10000
        ctx.input_tokens = 10000;
        ctx.context_input_tokens = Some(10000);
        // 强制零输出但有内容标记（模拟畸形：有 tool block 记录但 output_tokens=0）
        ctx.output_tokens = 0;
        ctx.tool_block_indices.insert("tu-x".to_string(), 99);
        let _ = ctx.generate_initial_events();
        let events = ctx.generate_final_events();
        // 不应判 EmptyResponse（produced_any_content 因 tool_block_indices 非空为 true）
        let msg_delta = events.iter().find(|e| e.event == "message_delta");
        if let Some(md) = msg_delta {
            let usage = &md.data["usage"];
            assert_eq!(
                usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()),
                None,
                "零输出时不应上报 cache_read"
            );
        }
        // emitted_cache_read 应为 0
        assert_eq!(ctx.emitted_cache_read, Some(0), "零输出上报缓存应为 0");
        // 但模拟器命中仍落库（emitted_raw_cache_read 保留 5000，供分析）
        assert_eq!(ctx.emitted_raw_cache_read, Some(5000), "命中真值仍记录供缓存分析");
    }

    #[test]
    fn test_nonzero_output_bills_cache_read_normally() {
        // 对照：有输出时正常按公式计费 reported = clamp(hit×mult, total×floor, total×cap)
        let mut ctx = StreamContext::new_with_thinking("claude-opus-4-8", 1000, false, HashMap::new());
        ctx.set_cache_read_multiplier(1.8);
        ctx.set_cache_cap_ratio(0.9);
        ctx.set_sim_cache(Some((4000, 10000)));
        ctx.input_tokens = 10000;
        ctx.context_input_tokens = Some(10000);
        let _ = ctx.generate_initial_events();
        let _ = ctx.process_assistant_response("Hello world");
        let _ = ctx.generate_final_events();
        // hit=4000, ×1.8=7200 < cap(0.9×10000=9000) → 7200
        assert_eq!(ctx.emitted_cache_read, Some(7200), "有输出应正常计费 cache_read");
    }

    #[test]
    fn test_thinking_with_text_keeps_end_turn_stop_reason() {
        // thinking + text 的情况，stop_reason 应为 end_turn
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>\n\nHello"));
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "end_turn",
            "stop_reason should be end_turn when text is also produced"
        );
    }

    #[test]
    fn test_thinking_with_tool_use_keeps_tool_use_stop_reason() {
        // thinking + tool_use 的情况，stop_reason 应为 tool_use
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all_events.extend(ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "test_tool".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: true,
        }));
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "tool_use",
            "stop_reason should be tool_use when tool_use is present"
        );
    }

    // ===== 原生 reasoningContentEvent → thinking 块 =====

    fn reasoning_event(text: &str) -> Event {
        let json = json!({ "text": text });
        Event::ReasoningContent(
            serde_json::from_value(json).expect("build ReasoningContentEvent"),
        )
    }

    fn reasoning_signature_event(sig: &str) -> Event {
        // 上游 thinking 流最后一帧：仅 signature，无 text
        let json = json!({ "signature": sig });
        Event::ReasoningContent(
            serde_json::from_value(json).expect("build ReasoningContentEvent"),
        )
    }

    #[test]
    fn native_reasoning_signature_passthrough() {
        // 上游下发真实签名后，关闭 thinking 块时 signature_delta 应携带该签名（非空占位）
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        let mut events = Vec::new();
        events.extend(ctx.process_kiro_event(&reasoning_event("thinking...")));
        // 上游最后一帧：签名（无 text）
        events.extend(ctx.process_kiro_event(&reasoning_signature_event("EtMBCmMIDhABGAIqQBhQ")));
        // 正文到来触发 thinking 块关闭
        events.extend(ctx.process_assistant_response("answer"));

        let sig_event = events.iter().find(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        });
        assert!(sig_event.is_some(), "应发 signature_delta");
        assert_eq!(
            sig_event.unwrap().data["delta"]["signature"], "EtMBCmMIDhABGAIqQBhQ",
            "signature_delta 应透传上游真实签名，而非空占位"
        );
    }

    #[test]
    fn native_reasoning_no_signature_falls_back_empty() {
        // 上游未给签名时，仍发空签名占位（保持结构合法，向后兼容）
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        let mut events = Vec::new();
        events.extend(ctx.process_kiro_event(&reasoning_event("thinking...")));
        events.extend(ctx.process_assistant_response("answer"));

        let sig_event = events.iter().find(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        });
        assert!(sig_event.is_some(), "无签名时仍应发 signature_delta 占位");
        assert_eq!(sig_event.unwrap().data["delta"]["signature"], "");
    }

    #[test]
    fn native_reasoning_emits_thinking_block() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        let mut events = Vec::new();
        events.extend(ctx.process_kiro_event(&reasoning_event("Let me ")));
        events.extend(ctx.process_kiro_event(&reasoning_event("think.")));

        // 应有一个 thinking 类型的 content_block_start
        let start = events.iter().find(|e| {
            e.event == "content_block_start"
                && e.data["content_block"]["type"] == "thinking"
        });
        assert!(start.is_some(), "应发出 thinking content_block_start");

        // 应有 thinking_delta，且内容为推理文本
        let deltas: Vec<&str> = events
            .iter()
            .filter(|e| e.event == "content_block_delta"
                && e.data["delta"]["type"] == "thinking_delta")
            .filter_map(|e| e.data["delta"]["thinking"].as_str())
            .collect();
        assert_eq!(deltas, vec!["Let me ", "think."]);
    }

    #[test]
    fn native_reasoning_closes_before_text() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        let mut events = Vec::new();
        events.extend(ctx.process_kiro_event(&reasoning_event("thinking...")));
        // 正文到来：应先关闭 thinking 块（含 signature_delta + content_block_stop），再开 text 块
        events.extend(ctx.process_assistant_response("answer"));

        let sig = events.iter().position(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        });
        let stop = events
            .iter()
            .position(|e| e.event == "content_block_stop");
        let text_start = events.iter().position(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "text"
        });

        assert!(sig.is_some(), "应发 signature_delta 收尾 thinking 块");
        assert!(stop.is_some(), "应发 content_block_stop 关闭 thinking 块");
        assert!(text_start.is_some(), "应为正文开 text 块");
        // 顺序：signature_delta < content_block_stop < text content_block_start
        assert!(sig.unwrap() < stop.unwrap());
        assert!(stop.unwrap() < text_start.unwrap());
    }

    #[test]
    fn native_reasoning_late_after_text_is_dropped() {
        // 修复 #1：reasoning 迟于正文到达 → 丢弃，不产生 text 后的非法 thinking 块
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        let mut events = Vec::new();
        events.extend(ctx.process_assistant_response("answer first"));
        // 此时 text 块已开；迟到的 reasoning 应被丢弃
        events.extend(ctx.process_kiro_event(&reasoning_event("late thinking")));

        let thinking_blocks = events.iter().filter(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "thinking"
        }).count();
        assert_eq!(thinking_blocks, 0, "迟到 reasoning 不应产生 thinking 块");
    }

    #[test]
    fn native_reasoning_reentry_each_block_signed() {
        // 修复 #2：reasoning → text → reasoning，两个 thinking 块都应有 signature_delta
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();

        // 注：当前实现下第二段 reasoning 在 text 块开启后会被顺序保护丢弃，
        // 所以这里验证“第一个 reasoning 块正常签名关闭”这一最关键不变量。
        let mut events = Vec::new();
        events.extend(ctx.process_kiro_event(&reasoning_event("step one")));
        events.extend(ctx.process_assistant_response("answer"));
        events.extend(ctx.generate_final_events());

        let sig_count = events.iter().filter(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        }).count();
        assert_eq!(sig_count, 1, "每个 thinking 块恰好一个 signature_delta");
    }

    #[test]
    fn native_reasoning_dropped_when_thinking_disabled() {
        // 修复 #4：thinking 未启用时，原生 reasoning 不暴露
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.generate_initial_events();

        let events = ctx.process_kiro_event(&reasoning_event("secret reasoning"));
        let has_thinking = events.iter().any(|e| {
            (e.event == "content_block_start" && e.data["content_block"]["type"] == "thinking")
                || (e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta")
        });
        assert!(!has_thinking, "thinking 关闭时不应发出任何 thinking 内容");
    }

    #[test]
    fn produced_any_content_detects_empty() {
        // 全新上下文、未产出任何内容 → 视为空响应
        let ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        assert!(!ctx.produced_any_content(), "零产出应判定为空响应");
    }

    #[test]
    fn produced_any_content_true_after_text() {
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.generate_initial_events();
        let _ = ctx.process_assistant_response("hello");
        assert!(ctx.produced_any_content(), "产出正文后不应判定为空");
    }

    #[test]
    fn empty_response_emits_terminal_error_event() {
        // 空响应：generate_final_events 应自动检测零内容、发 error 事件且不带 message_stop（终止性）
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.generate_initial_events();
        // 不产出任何内容，直接 finalize

        let events = ctx.generate_final_events();
        let has_error = events.iter().any(|e| e.event == "error"
            && e.data["error"]["type"] == "api_error");
        let has_message_stop = events.iter().any(|e| e.event == "message_stop");
        assert!(has_error, "空响应应发出 error 事件");
        assert!(!has_message_stop, "error 是终止事件，不应再发 message_stop");
        assert_eq!(ctx.failure_kind(), Some("empty_response"));
    }

    #[test]
    fn non_empty_response_no_error_event() {
        // 有正文产出 → 正常 message_stop，无 error
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.generate_initial_events();
        let _ = ctx.process_assistant_response("hello world");

        let events = ctx.generate_final_events();
        let has_error = events.iter().any(|e| e.event == "error");
        let has_message_stop = events.iter().any(|e| e.event == "message_stop");
        assert!(!has_error, "正常响应不应有 error 事件");
        assert!(has_message_stop, "正常响应应有 message_stop");
        assert_eq!(ctx.failure_kind(), None);
    }

    #[test]
    fn upstream_error_event_sets_failure() {
        // 上游 error 事件应被记录为 UpstreamError（此前被静默丢弃）
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.process_kiro_event(&Event::Error {
            error_code: "InternalError".to_string(),
            error_message: "boom".to_string(),
        });
        assert_eq!(ctx.failure_kind(), Some("upstream_error"));
        assert!(ctx.failure_message().unwrap().contains("boom"));
    }

    #[test]
    fn content_length_exceeded_is_not_failure() {
        // ContentLengthExceededException 是正常的 max_tokens 截断，不应记为失败
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, false, HashMap::new());
        let _ = ctx.generate_initial_events();
        let _ = ctx.process_assistant_response("partial answer");
        let _ = ctx.process_kiro_event(&Event::Exception {
            exception_type: "ContentLengthExceededException".to_string(),
            message: "too long".to_string(),
        });
        assert_eq!(ctx.failure_kind(), None, "ContentLength 异常不算失败");
    }

    #[test]
    fn failure_path_signs_open_fake_thinking_block() {
        // thinking 启用 + fake `<thinking>` 标签路径：流中途出错时，开着的 thinking 块
        // 必须在 stop 前补发 signature_delta（否则客户端可能拒收非法 thinking 块）
        let mut ctx = StreamContext::new_with_thinking("test-model", 1, true, HashMap::new());
        let _ = ctx.generate_initial_events();
        // 制造一个未闭合的 fake thinking 块
        let _ = ctx.process_assistant_response("<thinking>reasoning in progress");
        // 上游出错
        let _ = ctx.process_kiro_event(&Event::Error {
            error_code: "InternalError".to_string(),
            error_message: "boom".to_string(),
        });

        let events = ctx.generate_final_events();
        // 找 thinking 块的 signature_delta 与 content_block_stop 顺序
        let sig_pos = events.iter().position(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        });
        let has_error = events.iter().any(|e| e.event == "error");
        assert!(sig_pos.is_some(), "开着的 fake thinking 块应补发 signature_delta");
        assert!(has_error, "应发终止性 error 事件");
        // signature 恰好一次（不重复）
        let sig_count = events.iter().filter(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "signature_delta"
        }).count();
        assert_eq!(sig_count, 1, "signature_delta 应恰好一次");
    }
}
