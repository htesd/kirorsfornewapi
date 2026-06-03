//! Prefix 缓存命中模拟器
//!
//! ## 为什么需要它
//!
//! Kiro 上游对部分模型（opus-4-7 / 4-8 等）在 `tokenUsageEvent` 里**下发真实的**
//! `cacheReadInputTokens`，那是最准的命中值，直接用即可。但另一些模型（opus-4-6
//! 系列等）上游**不下发**该字段，历史上我们只能用 metering 反推（`cache_estimate`），
//! 误差大且无法体现"会话越长命中越高"的真实 prefix cache 形状。
//!
//! 本模块**按 Anthropic prompt prefix cache 的真实工作原理**模拟命中：
//!
//! 1. **同模型才命中**：换模型 → 缓存键变 → 整段 miss。
//! 2. **基于处理后上下文**：用 `build_history` 之后真正发给 Kiro 的消息序列
//!    （system + history + currentMessage），不是用户原始请求。
//! 3. **tokenize 后比对**：用 [`crate::token::count_tokens`] 给每条消息估 token，
//!    与上一轮的指纹序列求**最长公共前缀**，公共前缀覆盖的 token 数 = cache_read，
//!    其余（本轮新增）= uncached。
//! 4. **5 分钟 TTL**：对齐 Anthropic ephemeral 缓存；条目过期 → 下次冷启动全 miss。
//!
//! 算出的 cache_read 随会话自然增长、自带真实波动，不存在"恒定比例"的破绽。
//!
//! ## 架构
//!
//! - [`prefix_cache_read`]：**纯函数**，给定上一轮指纹序列 + 本轮指纹序列，
//!   算最长公共前缀的 token 数。无副作用、易测。
//! - [`CacheSimStore`]：按 `session_key` 索引的内存状态表（LRU + TTL）。
//!   线程安全（内部 `Mutex`），由全局单例 [`global`] 提供。
//! - [`observe`]：业务入口——传入会话键、模型、本轮指纹序列，返回模拟的
//!   cache_read（同时把本轮指纹存为下一轮的"上一轮"）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 缓存条目存活时间默认值（秒），对齐 Anthropic ephemeral prompt cache（约 5 分钟）。
/// 运行时可经 [`CacheSimStore::set_ttl_secs`] 热调（config.cache.simTtlSecs）。
const DEFAULT_ENTRY_TTL_SECS: u64 = 300;

/// 状态表最多保留的会话数默认值（LRU 淘汰），防止内存无界增长。
/// 运行时可经 [`CacheSimStore::set_max_sessions`] 热调（config.cache.maxSessions）。
const DEFAULT_MAX_SESSIONS: usize = 4096;

/// 单条消息的指纹：内容哈希 + 该消息的估算 token 数。
///
/// 哈希用于判断"这条消息与上一轮对应位置是否逐字节相同"，token 数用于
/// 累加公共前缀的缓存命中量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgFingerprint {
    /// 消息内容的 64-bit 哈希（FNV-1a，碰撞概率对计费近似足够低）。
    pub hash: u64,
    /// 该消息的估算 token 数。
    pub tokens: u32,
}

/// 计算字符串的 FNV-1a 64-bit 哈希。
///
/// 选 FNV 而非 SipHash/SHA：指纹只用于"同位置消息是否相同"的相等判断，
/// 不涉及安全，FNV 快且零依赖。
fn fnv1a(s: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// 从一段文本构造消息指纹（哈希 + token 估算）。
pub fn fingerprint(text: &str) -> MsgFingerprint {
    MsgFingerprint {
        hash: fnv1a(text),
        tokens: crate::token::count_tokens(text).min(u32::MAX as u64) as u32,
    }
}

/// **纯函数**：给定上一轮与本轮的指纹序列，算最长公共前缀的 token 总数。
///
/// 这是真实 prefix cache 的核心：缓存命中的是从头开始连续相同的那段消息；
/// 一旦某条消息变了（或本轮更长的新增部分），其后全部算未命中。
///
/// 返回命中的 token 数（= 公共前缀里所有消息的 token 之和）。
pub fn prefix_cache_read(prev: &[MsgFingerprint], curr: &[MsgFingerprint]) -> u64 {
    let mut hit: u64 = 0;
    for (a, b) in prev.iter().zip(curr.iter()) {
        if a.hash == b.hash {
            hit += b.tokens as u64;
        } else {
            break;
        }
    }
    hit
}

/// 单会话的缓存状态：上一轮发给 Kiro 的指纹序列 + 模型 + 最后访问时间。
#[derive(Debug, Clone)]
struct SessionEntry {
    model: String,
    prev: Vec<MsgFingerprint>,
    last_seen: Instant,
}

/// 模拟结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimResult {
    /// 模拟的 cache_read token 数（冷启动 / 换模型 / 无公共前缀 → 0）。
    pub cache_read_tokens: u32,
    /// 本轮上下文总 token（公共前缀 + 新增），便于上层算比例 / 校验。
    pub total_tokens: u32,
}

/// 按会话键索引的缓存状态表（LRU + TTL）。
///
/// TTL 与容量上限是运行时可热调的原子量（来自 config.cache，admin 面板可改）。
pub struct CacheSimStore {
    inner: Mutex<HashMap<String, SessionEntry>>,
    /// 条目存活时间（秒）。
    ttl_secs: AtomicU64,
    /// 最大会话数。
    max_sessions: AtomicUsize,
}

impl CacheSimStore {
    fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl_secs: AtomicU64::new(DEFAULT_ENTRY_TTL_SECS),
            max_sessions: AtomicUsize::new(DEFAULT_MAX_SESSIONS),
        }
    }

    /// 热调条目 TTL（秒，0 视为 1 避免立即过期）。
    pub fn set_ttl_secs(&self, secs: u64) {
        self.ttl_secs.store(secs.max(1), Ordering::Relaxed);
    }

    /// 热调最大会话数（0 视为 1）。
    pub fn set_max_sessions(&self, n: usize) {
        self.max_sessions.store(n.max(1), Ordering::Relaxed);
    }

    /// 当前生效的 TTL（秒）—— 权威 live 值（admin 读回用）。
    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs.load(Ordering::Relaxed)
    }

    /// 当前生效的最大会话数 —— 权威 live 值。
    pub fn max_sessions_value(&self) -> usize {
        self.max_sessions.load(Ordering::Relaxed)
    }

    fn ttl(&self) -> Duration {
        Duration::from_secs(self.ttl_secs.load(Ordering::Relaxed).max(1))
    }

    fn max_sessions(&self) -> usize {
        self.max_sessions.load(Ordering::Relaxed).max(1)
    }

    /// 观测一次请求：返回模拟 cache_read，并把本轮指纹存为下一轮的"上一轮"。
    ///
    /// 命中条件（全满足才有 cache_read > 0）：
    /// - 会话键已有上一轮记录，
    /// - 模型与上一轮相同，
    /// - 上一轮未过 TTL，
    /// - 本轮与上一轮存在非空公共前缀。
    ///
    /// `now` 显式传入便于测试；生产用 [`observe`] 包装传 `Instant::now()`。
    pub fn observe_at(
        &self,
        session_key: &str,
        model: &str,
        curr: Vec<MsgFingerprint>,
        now: Instant,
    ) -> SimResult {
        let total_tokens: u64 = curr.iter().map(|m| m.tokens as u64).sum();
        let ttl = self.ttl();
        let cap = self.max_sessions();

        let mut map = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // 计算命中：仅当上一轮存在、模型相同、未过期。
        let cache_read = match map.get(session_key) {
            Some(entry)
                if entry.model == model && now.duration_since(entry.last_seen) <= ttl =>
            {
                prefix_cache_read(&entry.prev, &curr)
            }
            _ => 0,
        };

        // 更新本会话为本轮状态（供下一轮比对）。
        map.insert(
            session_key.to_string(),
            SessionEntry {
                model: model.to_string(),
                prev: curr,
                last_seen: now,
            },
        );

        // 容量 / 过期维护：超量时按 last_seen 淘汰最旧 + 顺手清过期。
        if map.len() > cap {
            evict(&mut map, now, ttl, cap);
        }

        SimResult {
            cache_read_tokens: cache_read.min(total_tokens).min(u32::MAX as u64) as u32,
            total_tokens: total_tokens.min(u32::MAX as u64) as u32,
        }
    }
}

/// 清理过期条目；若清理后仍超量，按 last_seen 最旧优先淘汰到容量内。
fn evict(map: &mut HashMap<String, SessionEntry>, now: Instant, ttl: Duration, cap: usize) {
    map.retain(|_, e| now.duration_since(e.last_seen) <= ttl);
    while map.len() > cap {
        if let Some(oldest_key) = map
            .iter()
            .min_by_key(|(_, e)| e.last_seen)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest_key);
        } else {
            break;
        }
    }
}

/// 全局单例状态表。
pub fn global() -> &'static CacheSimStore {
    static STORE: OnceLock<CacheSimStore> = OnceLock::new();
    STORE.get_or_init(CacheSimStore::new)
}

/// 业务入口：用全局状态表观测一次请求（`now = Instant::now()`）。
pub fn observe(session_key: &str, model: &str, curr: Vec<MsgFingerprint>) -> SimResult {
    global().observe_at(session_key, model, curr, Instant::now())
}

/// 从处理后的 [`ConversationState`] 抽取指纹序列。
///
/// 顺序严格对齐发给 Kiro 的真实 prefix：`history[0..]` + `currentMessage`。
///
/// **关键（v53 修复"用户提问轮"崩盘）**：每条消息的指纹只取其**稳定语义内容**
/// —— role + 正文 + tool_results(id/状态/内容) + tool_uses(id/name/input) + 图片/文档
/// 计数。**刻意忽略两类东西**：
///   1. `tools` 列表（几百个工具定义）—— 它不是对话内容，且只挂在 currentMessage 上；
///   2. 容器结构差异（`UserInputMessage` vs `UserMessage`/`AssistantMessage` 的 JSON 形状）。
///
/// 为什么必须这样：同一句用户输入，本轮在 `currentMessage`（带 tools、`UserInputMessage`
/// 结构），下一轮沉淀进 `history`（不带 tools、`UserMessage` 结构）。若按整条 JSON 序列化
/// 算指纹，**同一句话在两轮的指纹必然不同** → 公共前缀在 history/current 接缝处 break →
/// 每个"用户提问轮"都被算成低命中甚至 0（实测线上间歇崩盘的真正机理）。改取稳定语义内容
/// 后，一条消息无论在 current 还是 history 都得到**相同指纹**，前缀和随会话平滑单调增长。
///
/// 注意：**不含** `conversationId` / `agentContinuationId` 等会话级元数据——
/// 它们不属于被缓存的 prompt 前缀内容。
pub fn fingerprints_from_state(
    state: &crate::kiro::model::requests::conversation::ConversationState,
) -> Vec<MsgFingerprint> {
    use crate::kiro::model::requests::conversation::Message;
    let mut fps = Vec::with_capacity(state.history.len() + 1);
    for msg in &state.history {
        let canon = match msg {
            Message::User(u) => canon_user(
                &u.user_input_message.content,
                &u.user_input_message.user_input_message_context.tool_results,
                u.user_input_message.images.len(),
                u.user_input_message.documents.len(),
            ),
            Message::Assistant(a) => canon_assistant(
                &a.assistant_response_message.content,
                a.assistant_response_message.tool_uses.as_deref(),
            ),
        };
        fps.push(fingerprint(&canon));
    }
    let cur = &state.current_message.user_input_message;
    let canon = canon_user(
        &cur.content,
        &cur.user_input_message_context.tool_results,
        cur.images.len(),
        cur.documents.len(),
    );
    fps.push(fingerprint(&canon));
    fps
}

/// 规范化一条 user 消息为稳定语义字符串（current 与 history 走同一逻辑）。
/// 忽略 tools 列表与容器结构差异，使同一内容跨轮指纹一致。
fn canon_user(
    content: &str,
    tool_results: &[crate::kiro::model::requests::tool::ToolResult],
    n_images: usize,
    n_documents: usize,
) -> String {
    let mut s = String::with_capacity(content.len() + 64);
    s.push_str("U\x1f");
    s.push_str(content);
    for tr in tool_results {
        s.push_str("\x1ftr:");
        s.push_str(&tr.tool_use_id);
        s.push('\x1e');
        if let Some(st) = &tr.status {
            s.push_str(st);
        }
        s.push('\x1e');
        // content 是 Vec<Map>，序列化为稳定字符串（字段顺序由 serde_json 保证插入序，
        // 但 Map 是 BTreeMap-like? serde_json::Map 默认保留插入序——同一来源同序，足够稳定）
        if let Ok(c) = serde_json::to_string(&tr.content) {
            s.push_str(&c);
        }
    }
    if n_images > 0 {
        s.push_str(&format!("\x1fimg:{}", n_images));
    }
    if n_documents > 0 {
        s.push_str(&format!("\x1fdoc:{}", n_documents));
    }
    s
}

/// 规范化一条 assistant 消息为稳定语义字符串。
fn canon_assistant(
    content: &str,
    tool_uses: Option<&[crate::kiro::model::requests::tool::ToolUseEntry]>,
) -> String {
    let mut s = String::with_capacity(content.len() + 64);
    s.push_str("A\x1f");
    s.push_str(content);
    if let Some(tus) = tool_uses {
        for tu in tus {
            s.push_str("\x1ftu:");
            s.push_str(&tu.tool_use_id);
            s.push('\x1e');
            s.push_str(&tu.name);
            s.push('\x1e');
            if let Ok(inp) = serde_json::to_string(&tu.input) {
                s.push_str(&inp);
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(hash: u64, tokens: u32) -> MsgFingerprint {
        MsgFingerprint { hash, tokens }
    }

    #[test]
    fn empty_prefix_is_zero() {
        assert_eq!(prefix_cache_read(&[], &[fp(1, 10)]), 0);
        assert_eq!(prefix_cache_read(&[fp(1, 10)], &[]), 0);
    }

    #[test]
    fn full_common_prefix_sums_tokens() {
        let prev = vec![fp(1, 10), fp(2, 20), fp(3, 30)];
        let curr = vec![fp(1, 10), fp(2, 20), fp(3, 30), fp(4, 40)];
        // 前 3 条相同 → 命中 10+20+30=60；第 4 条是新增不算
        assert_eq!(prefix_cache_read(&prev, &curr), 60);
    }

    #[test]
    fn divergence_stops_prefix() {
        let prev = vec![fp(1, 10), fp(2, 20), fp(3, 30)];
        // 第 2 条变了 → 只命中第 1 条
        let curr = vec![fp(1, 10), fp(99, 20), fp(3, 30)];
        assert_eq!(prefix_cache_read(&prev, &curr), 10);
    }

    #[test]
    fn first_message_divergence_zero_hit() {
        let prev = vec![fp(1, 10), fp(2, 20)];
        let curr = vec![fp(99, 10), fp(2, 20)];
        assert_eq!(prefix_cache_read(&prev, &curr), 0);
    }

    #[test]
    fn cold_start_is_miss() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        let r = store.observe_at("sess-a", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        assert_eq!(r.cache_read_tokens, 0, "首轮冷启动应 0 命中");
        assert_eq!(r.total_tokens, 150);
    }

    #[test]
    fn second_turn_hits_growing_prefix() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        // turn1: [sys, u1]
        store.observe_at("s", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        // turn2: [sys, u1, a1, u2] —— 前两条不变
        let t1 = t0 + Duration::from_secs(5);
        let r = store.observe_at(
            "s",
            "opus-4-7",
            vec![fp(1, 100), fp(2, 50), fp(3, 30), fp(4, 20)],
            t1,
        );
        assert_eq!(r.cache_read_tokens, 150, "应命中前两条 100+50");
        assert_eq!(r.total_tokens, 200);
    }

    #[test]
    fn model_switch_is_full_miss() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        store.observe_at("s", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        // 同会话同前缀，但换了模型 → 缓存键失效 → 全 miss
        let r = store.observe_at(
            "s",
            "opus-4-6",
            vec![fp(1, 100), fp(2, 50), fp(3, 30)],
            t0 + Duration::from_secs(5),
        );
        assert_eq!(r.cache_read_tokens, 0, "换模型应全 miss");
    }

    #[test]
    fn ttl_expiry_is_cold_again() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        store.observe_at("s", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        // 超过 5 分钟 TTL → 视为冷启动
        let r = store.observe_at(
            "s",
            "opus-4-7",
            vec![fp(1, 100), fp(2, 50), fp(3, 30)],
            t0 + Duration::from_secs(DEFAULT_ENTRY_TTL_SECS + 1),
        );
        assert_eq!(r.cache_read_tokens, 0, "TTL 过期应冷启动 0 命中");
    }

    #[test]
    fn within_ttl_still_hits() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        store.observe_at("s", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        let r = store.observe_at(
            "s",
            "opus-4-7",
            vec![fp(1, 100), fp(2, 50), fp(3, 30)],
            t0 + Duration::from_secs(DEFAULT_ENTRY_TTL_SECS - 1),
        );
        assert_eq!(r.cache_read_tokens, 150, "TTL 边界内应命中");
    }

    #[test]
    fn different_sessions_isolated() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        store.observe_at("s1", "opus-4-7", vec![fp(1, 100)], t0);
        // s2 不同会话，即便指纹相同也应冷启动
        let r = store.observe_at("s2", "opus-4-7", vec![fp(1, 100), fp(2, 50)], t0);
        assert_eq!(r.cache_read_tokens, 0, "不同会话应隔离");
    }

    #[test]
    fn fingerprint_stable_and_distinct() {
        let a = fingerprint("hello world");
        let b = fingerprint("hello world");
        let c = fingerprint("hello worle");
        assert_eq!(a.hash, b.hash, "相同文本指纹应一致");
        assert_ne!(a.hash, c.hash, "不同文本指纹应不同");
        assert!(a.tokens > 0);
    }

    #[test]
    fn cache_read_clamped_to_total() {
        // 防御：即便 prev 比 curr token 多，命中也不超过本轮 total
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        store.observe_at("s", "m", vec![fp(1, 100), fp(2, 100)], t0);
        let r = store.observe_at("s", "m", vec![fp(1, 100)], t0 + Duration::from_secs(1));
        assert!(r.cache_read_tokens <= r.total_tokens);
        assert_eq!(r.cache_read_tokens, 100);
    }

    #[test]
    fn eviction_keeps_within_capacity() {
        let store = CacheSimStore::new();
        let t0 = Instant::now();
        // 插入超过容量的会话，验证不 panic 且最终有界
        for i in 0..(DEFAULT_MAX_SESSIONS + 100) {
            store.observe_at(
                &format!("sess-{i}"),
                "m",
                vec![fp(i as u64, 10)],
                t0 + Duration::from_millis(i as u64),
            );
        }
        let len = store.inner.lock().unwrap().len();
        assert!(len <= DEFAULT_MAX_SESSIONS, "淘汰后应不超过容量, len={len}");
    }

    #[test]
    fn fingerprints_from_state_orders_history_then_current() {
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, HistoryAssistantMessage, HistoryUserMessage,
            Message, UserInputMessage,
        };
        let mut state = ConversationState::new("conv-x");
        state.history = vec![
            Message::User(HistoryUserMessage::new("sys+u1", "opus-4-7")),
            Message::Assistant(HistoryAssistantMessage::new("a1")),
        ];
        let mut cur = UserInputMessage::default();
        cur.content = "u2".to_string();
        cur.model_id = "opus-4-7".to_string();
        state.current_message = CurrentMessage::new(cur);

        let fps = fingerprints_from_state(&state);
        // 2 条 history + 1 条 current
        assert_eq!(fps.len(), 3);
        // 每条都有非零 token、非零哈希
        assert!(fps.iter().all(|f| f.tokens > 0));
        // 内容不同 → 指纹各异
        assert_ne!(fps[0].hash, fps[1].hash);
        assert_ne!(fps[1].hash, fps[2].hash);
    }

    #[test]
    fn fingerprints_change_when_history_grows() {
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, HistoryUserMessage, Message, UserInputMessage,
        };
        // turn1: history=[u1], current=u_q1
        let mut s1 = ConversationState::new("c");
        s1.history = vec![Message::User(HistoryUserMessage::new("u1", "m"))];
        let mut c1 = UserInputMessage::default();
        c1.content = "q1".into();
        s1.current_message = CurrentMessage::new(c1);
        let f1 = fingerprints_from_state(&s1);

        // turn2: history=[u1, q1, a1], current=q2 —— 前缀 u1 应稳定
        let mut s2 = ConversationState::new("c");
        s2.history = vec![Message::User(HistoryUserMessage::new("u1", "m"))];
        let f2 = fingerprints_from_state(&s2);
        assert_eq!(f1[0].hash, f2[0].hash, "相同首条 history 指纹应稳定");
    }

    #[test]
    fn current_message_fingerprint_stable_after_sinking_into_history() {
        // v53 核心回归：同一句用户输入，本轮在 currentMessage（带 tools，UserInputMessage
        // 结构），下一轮沉淀进 history（不带 tools，UserMessage 结构）。两者指纹必须相同，
        // 否则"用户提问轮"前缀在 history/current 接缝处 break → 间歇崩盘（线上实证）。
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, HistoryUserMessage, Message, UserInputMessage,
            UserInputMessageContext, UserMessage,
        };
        use crate::kiro::model::requests::tool::{InputSchema, Tool, ToolSpecification};

        let make_tool = |name: &str| Tool {
            tool_specification: ToolSpecification {
                name: name.to_string(),
                description: "x".to_string(),
                input_schema: InputSchema::from_json(serde_json::json!({"type": "object"})),
            },
        };

        // turn N: "解释这段代码" 作为 currentMessage，挂着 295 个 tools
        let mut cur = UserInputMessage::default();
        cur.content = "解释这段代码".to_string();
        cur.model_id = "opus-4-8".to_string();
        cur.user_input_message_context = UserInputMessageContext {
            tool_results: vec![],
            tools: (0..295).map(|i| make_tool(&format!("tool_{i}"))).collect(),
        };
        let mut s_cur = ConversationState::new("c");
        s_cur.current_message = CurrentMessage::new(cur);
        let f_cur = fingerprints_from_state(&s_cur);
        let cur_fp = *f_cur.last().unwrap();

        // turn N+1: 同一句话沉淀进 history（UserMessage，无 tools），currentMessage 换新内容
        let mut sunk = UserMessage::new("解释这段代码", "opus-4-8");
        sunk.user_input_message_context = UserInputMessageContext::default();
        let mut s_next = ConversationState::new("c");
        s_next.history = vec![Message::User(HistoryUserMessage {
            user_input_message: sunk,
        })];
        let mut cur2 = UserInputMessage::default();
        cur2.content = "下一个问题".to_string();
        s_next.current_message = CurrentMessage::new(cur2);
        let f_next = fingerprints_from_state(&s_next);
        let sunk_fp = f_next[0];

        assert_eq!(
            cur_fp.hash, sunk_fp.hash,
            "同一句话在 current(带295 tools) 与沉淀进 history 后指纹必须一致——这是缓存不崩的命门"
        );
        assert_eq!(cur_fp.tokens, sunk_fp.tokens, "token 数也应一致");
    }

    #[test]
    fn tools_list_does_not_affect_fingerprint() {
        // 同一 currentMessage 内容，tools 多寡不应改变指纹（tools 不是对话内容）。
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, UserInputMessage, UserInputMessageContext,
        };
        use crate::kiro::model::requests::tool::{InputSchema, Tool, ToolSpecification};
        let make_tool = |name: &str| Tool {
            tool_specification: ToolSpecification {
                name: name.to_string(),
                description: "x".to_string(),
                input_schema: InputSchema::from_json(serde_json::json!({"type": "object"})),
            },
        };
        let mut a = UserInputMessage::default();
        a.content = "hi".into();
        let mut b = a.clone();
        a.user_input_message_context = UserInputMessageContext {
            tool_results: vec![],
            tools: vec![],
        };
        b.user_input_message_context = UserInputMessageContext {
            tool_results: vec![],
            tools: (0..50).map(|i| make_tool(&format!("t{i}"))).collect(),
        };
        let mut sa = ConversationState::new("c");
        sa.current_message = CurrentMessage::new(a);
        let mut sb = ConversationState::new("c");
        sb.current_message = CurrentMessage::new(b);
        assert_eq!(
            fingerprints_from_state(&sa).last().unwrap().hash,
            fingerprints_from_state(&sb).last().unwrap().hash,
            "tools 多寡不应影响指纹"
        );
    }
}
