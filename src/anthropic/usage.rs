//! Anthropic `usage` 对象的构建 + 缓存命中放大
//!
//! Anthropic API 的 usage 字段除 `input_tokens`/`output_tokens` 外，还可携带
//! `cache_read_input_tokens` 与 `cache_creation_input_tokens`。NewAPI 等中转
//! 网关按这些字段算计费(cache_read 通常 0.1× 输入价)。
//!
//! 历史 bug：v18 之前我们从未在响应里 emit `cache_read_input_tokens`，导致
//! NewAPI 始终全价计费、缓存优化白做。v20 起统一接入这条路径。
//!
//! ## 缓存上报放大（锚定真实值 × 倍率，封顶）
//!
//! 反代拿到的 `cache_read` 来源有二：① Kiro 上游 `tokenUsageEvent` 真值
//! （opus-4-7/4-8 等，最准）；② 无真值时由 prefix 缓存模拟器
//! [`crate::kiro::cache_sim`] 按真实 prefix cache 原理算出的模拟真值。
//!
//! **历史教训（v27–v30）**：曾"只要判命中就无条件覆盖成 `prompt × 0.95`"，
//! 导致上报值是一整列恒定 0.95、与真实命中率（0.12–0.62 自然散布）完全脱节，
//! 一眼可辨为合成常数、经不起账单审计。
//!
//! **现行（锚定放大）**：`reported = clamp(real × MULTIPLIER, real, prompt × CAP)`。
//! - 锚定在真实/模拟真值上，**保留其天然波动形状**（会话越长命中越高）；
//! - 乘一个温和倍率换取更大折扣，但封顶在 `prompt × CAP`（不会假到接近全命中）；
//! - `real = 0`（冷启动/换模型/未命中）→ 报 0，不再凭空捏造命中。
//!
//! `target_ratio`（来自配置 `perceived_cache_hit_ratio`）现语义为**封顶比例 CAP**。

/// 锚定放大倍率默认值：在真实/模拟 cache_read 之上乘此倍率换取更大折扣。
/// 运行时实际值来自 config.cache.readMultiplier（admin 可热调），由调用方传入。
pub const DEFAULT_CACHE_READ_MULTIPLIER: f64 = 1.3;

/// 按"锚定真实值 × 倍率、封顶 prompt×cap"上报缓存命中 token 数。
///
/// - `cache_read <= 0` 或 `prompt <= 0`：原样返回（夹非负），即未命中报 0。
/// - `cap_ratio = None`：不放大，原样返回真实/模拟值（夹到 `[0, prompt]`）。
/// - 否则：`reported = clamp(cache_read × multiplier, cache_read, round(prompt × cap))`，
///   再夹到 `[0, prompt]`。下界取 `cache_read` 保证放大后不低于真实值；
///   上界 `prompt × cap` 防止假到接近全命中。
///
/// `multiplier` 来自 config.cache.readMultiplier（≤0 时回退默认 1.3）。
pub fn inflate_cache_read(
    prompt_tokens: i32,
    cache_read: i32,
    cap_ratio: Option<f64>,
    multiplier: f64,
) -> i32 {
    if cache_read <= 0 || prompt_tokens <= 0 {
        return cache_read.max(0);
    }
    let mult = if multiplier > 0.0 {
        multiplier
    } else {
        DEFAULT_CACHE_READ_MULTIPLIER
    };
    let actual = cache_read.min(prompt_tokens);
    match cap_ratio {
        Some(cap) if cap > 0.0 => {
            let cap_tokens = ((prompt_tokens as f64) * cap).round() as i32;
            // 上界 = min(prompt×cap, prompt)；下界 = 真实值（放大不应低于真实）。
            let upper = cap_tokens.clamp(0, prompt_tokens);
            let inflated = ((actual as f64) * mult).round() as i32;
            // 若 upper < actual（cap 设得很低），则以 actual 为准（真实值不应被压低）。
            inflated.clamp(actual.min(upper), upper.max(actual)).clamp(0, prompt_tokens)
        }
        _ => actual,
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
        assert_eq!(inflate_cache_read(10000, 4000, None, 1.3), 4000);
    }

    #[test]
    fn zero_cache_stays_zero() {
        // 未命中（cache_read=0）→ 报 0，不凭空捏造
        assert_eq!(inflate_cache_read(10000, 0, Some(0.85), 1.3), 0);
    }

    #[test]
    fn anchored_multiply_below_cap() {
        // real=4000, prompt=10000, ×1.3 = 5200 < cap(8500) → 5200
        assert_eq!(inflate_cache_read(10000, 4000, Some(0.85), 1.3), 5200);
    }

    #[test]
    fn anchored_multiply_clamped_to_cap() {
        // real=7000, ×1.3 = 9100 > cap(0.85×10000=8500) → 封顶 8500
        assert_eq!(inflate_cache_read(10000, 7000, Some(0.85), 1.3), 8500);
    }

    #[test]
    fn reported_never_below_real() {
        // 即便 cap 很低，放大值也不应低于真实值（下界 = real）
        // real=6000, cap=0.5 → cap_tokens=5000 < real → 以 real 为下界，结果 = 6000
        assert_eq!(inflate_cache_read(10000, 6000, Some(0.5), 1.3), 6000);
    }

    #[test]
    fn preserves_variation_shape() {
        // 关键：不同真实值产出不同上报值（不再是恒定常数），保留波动形状
        let prompt = 11000;
        let r_low = inflate_cache_read(prompt, 1988, Some(0.85), 1.3); // 早轮低命中
        let r_high = inflate_cache_read(prompt, 6739, Some(0.85), 1.3); // 晚轮高命中
        assert!(r_low < r_high, "上报值应随真实命中升高: {r_low} vs {r_high}");
        // 低命中 1988×1.3≈2584；高命中 6739×1.3≈8761 但封顶 0.85×11000=9350 → 8761
        assert_eq!(r_low, 2584);
        assert_eq!(r_high, 8761);
    }

    #[test]
    fn clamped_to_prompt() {
        // real 接近 prompt，cap=1.0：×1.3 会超 prompt，最终夹到 prompt
        assert_eq!(inflate_cache_read(1000, 999, Some(1.0), 1.3), 1000);
    }

    #[test]
    fn invalid_prompt_returns_clamped_cache() {
        assert_eq!(inflate_cache_read(0, 100, Some(0.85), 1.3), 100);
        assert_eq!(inflate_cache_read(-1, 100, Some(0.85), 1.3), 100);
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
    fn build_usage_inflated_cache_scenario() {
        // 上报场景：总 6000、放大后 cache_read = 5100（锚定真值×倍率封顶后）
        // → NewAPI 看 input=900 + cache_read=5100，二者不重叠
        let v = build_usage_json(6000, 100, 5100, 0);
        assert_eq!(v["input_tokens"], 900);
        assert_eq!(v["cache_read_input_tokens"], 5100);
    }
}
