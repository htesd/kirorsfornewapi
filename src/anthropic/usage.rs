//! Anthropic `usage` 对象的构建 + 缓存命中放大
//!
//! Anthropic API 的 usage 字段除 `input_tokens`/`output_tokens` 外，还可携带
//! `cache_read_input_tokens` 与 `cache_creation_input_tokens`。NewAPI 等中转
//! 网关按这些字段算计费(cache_read 通常 0.1× 输入价)。
//!
//! 历史 bug：v18 之前我们从未在响应里 emit `cache_read_input_tokens`，导致
//! NewAPI 始终全价计费、缓存优化白做。v20 起统一接入这条路径。
//!
//! ## 用户感知放大（perceived_cache_hit_ratio）
//!
//! 反代实际命中率受 Kiro 服务端缓存容量上限制约（≤52% 长会话、单次 ~50-70%），
//! 直接按真值上报 NewAPI 会让用户看到的账单偏贵。运营方可配置
//! `perceived_cache_hit_ratio`（如 0.92）让 NewAPI 看到的缓存比例稳定在
//! 90-95%；代理方承担与 Kiro 真实计费的差额。

/// 按 perceived_cache_hit_ratio 上报缓存命中 token 数。
///
/// - `cache_read=0` 时返回 0（命中判定权在调用方，0 表示未命中）。
/// - `target_ratio=None` 时按实际/估算值原样返回。
/// - 否则**直接覆盖**为 `round(prompt × ratio)`，夹到 `[0, prompt]`。
///   （v27 起改成无条件覆盖：用户要"只要命中就报 95%"，不再 `max(实际, 目标)`。）
pub fn inflate_cache_read(prompt_tokens: i32, cache_read: i32, target_ratio: Option<f64>) -> i32 {
    if cache_read <= 0 || prompt_tokens <= 0 {
        return cache_read.max(0);
    }
    match target_ratio {
        Some(r) if r > 0.0 => {
            let target = (prompt_tokens as f64 * r).round() as i32;
            target.clamp(0, prompt_tokens)
        }
        _ => cache_read.min(prompt_tokens),
    }
}

/// 构建 Anthropic `usage` JSON 对象。
///
/// **关键语义（v30 修正，对齐 Anthropic API 规范）**：
/// - `input_tokens` 必须只算**未命中（新增）部分**，不能包含缓存读取/创建的 token
/// - `cache_read_input_tokens` / `cache_creation_input_tokens` 单独列
/// - 总上下文 token = input_tokens + cache_read_input_tokens + cache_creation_input_tokens
///
/// NewAPI 等中转网关按 `input × 输入价 + cache_read × 缓存读价 + cache_creation × 缓存写价`
/// 累加计费。v29 之前我们 emit `input_tokens=总值`，**导致缓存部分被双重计费、用户多付**。
///
/// 调用方传入"总上下文 input"，本函数减去 cache_read / cache_creation 得到 uncached_input。
pub fn build_usage_json(
    total_input_tokens: i32,
    output_tokens: i32,
    cache_read: i32,
    cache_creation: i32,
) -> serde_json::Value {
    // 仅算未命中部分；夹到非负
    let uncached_input = (total_input_tokens - cache_read - cache_creation).max(0);

    let mut obj = serde_json::Map::new();
    obj.insert("input_tokens".into(), serde_json::json!(uncached_input));
    obj.insert("output_tokens".into(), serde_json::json!(output_tokens));
    if cache_read > 0 {
        obj.insert("cache_read_input_tokens".into(), serde_json::json!(cache_read));
    }
    if cache_creation > 0 {
        obj.insert(
            "cache_creation_input_tokens".into(),
            serde_json::json!(cache_creation),
        );
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ratio_returns_actual() {
        assert_eq!(inflate_cache_read(10000, 4000, None), 4000);
    }

    #[test]
    fn zero_cache_stays_zero() {
        assert_eq!(inflate_cache_read(10000, 0, Some(0.92)), 0);
    }

    #[test]
    fn ratio_directly_sets_target_when_actual_lower() {
        // v27 起：命中即直接报 prompt × ratio，不取 max
        assert_eq!(inflate_cache_read(10000, 4000, Some(0.92)), 9200);
        assert_eq!(inflate_cache_read(10000, 4000, Some(0.95)), 9500);
    }

    #[test]
    fn ratio_overrides_actual_even_when_higher() {
        // v27 行为变更：哪怕实际更大，也按 ratio 覆盖（用户要稳定 95%）
        assert_eq!(inflate_cache_read(10000, 9800, Some(0.92)), 9200);
        assert_eq!(inflate_cache_read(10000, 9800, Some(0.95)), 9500);
    }

    #[test]
    fn clamped_to_prompt() {
        assert_eq!(inflate_cache_read(1000, 999, Some(1.0)), 1000);
        // 即便 target 超过 prompt，结果仍夹到 prompt
        assert_eq!(inflate_cache_read(1000, 500, Some(1.5)), 1000);
    }

    #[test]
    fn invalid_prompt_returns_clamped_cache() {
        assert_eq!(inflate_cache_read(0, 100, Some(0.92)), 100);
        assert_eq!(inflate_cache_read(-1, 100, Some(0.92)), 100);
    }

    #[test]
    fn build_usage_omits_zero_cache_fields_and_keeps_full_input() {
        // 无缓存：input_tokens 等于总值
        let v = build_usage_json(1000, 50, 0, 0);
        assert_eq!(v["input_tokens"], 1000);
        assert_eq!(v["output_tokens"], 50);
        assert!(v.get("cache_read_input_tokens").is_none());
        assert!(v.get("cache_creation_input_tokens").is_none());
    }

    #[test]
    fn build_usage_subtracts_cache_from_input() {
        // 总=1000、cache_read=800、cache_creation=50 → uncached=150
        let v = build_usage_json(1000, 50, 800, 50);
        assert_eq!(v["input_tokens"], 150);
        assert_eq!(v["cache_read_input_tokens"], 800);
        assert_eq!(v["cache_creation_input_tokens"], 50);
        // 总上下文 = uncached + cache_read + cache_creation
        let sum = v["input_tokens"].as_i64().unwrap()
            + v["cache_read_input_tokens"].as_i64().unwrap()
            + v["cache_creation_input_tokens"].as_i64().unwrap();
        assert_eq!(sum, 1000);
    }

    #[test]
    fn build_usage_clamps_negative_uncached_to_zero() {
        // 极端：cache_read 比 total 还大（数据异常），uncached 不能为负
        let v = build_usage_json(100, 5, 200, 0);
        assert_eq!(v["input_tokens"], 0);
        assert_eq!(v["cache_read_input_tokens"], 200);
    }

    #[test]
    fn build_usage_95pct_inflation_scenario() {
        // 典型 v27+ 场景：总 6000、inflated cache_read = 0.95*6000 = 5700
        // → NewAPI 看 input=300（5%）+ cache_read=5700（95%）
        let v = build_usage_json(6000, 100, 5700, 0);
        assert_eq!(v["input_tokens"], 300);
        assert_eq!(v["cache_read_input_tokens"], 5700);
    }
}
