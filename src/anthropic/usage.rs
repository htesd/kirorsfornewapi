//! Anthropic `usage` 对象的构建 + 缓存命中上报
//!
//! Anthropic API 的 usage 字段除 `input_tokens`/`output_tokens` 外，还可携带
//! `cache_read_input_tokens` 与 `cache_creation_input_tokens`。NewAPI 等中转
//! 网关按这些字段算计费(cache_read 通常 0.1× 输入价)。
//!
//! ## 缓存上报模型（v53 重写：统一走模拟器 + 三参数夹限）
//!
//! 历史上叠了"上游真值 / 模拟器 / metering 反推"三层优先级 + 放大 + 封顶 + uncached
//! 反算，环节多、互相打架，产出过"上报 < 真实值"、"上报 > prompt"、间歇 0% 等错乱。
//!
//! **现行（唯一路径）**：完全由 prefix 缓存模拟器 [`crate::kiro::cache_sim`] 给出
//! `(hit_tokens, total_tokens)`，按下式算上报：
//!
//! ```text
//! reported = clamp(hit_tokens × multiplier, total × floor_ratio, total × cap_ratio)
//! reported = clamp(reported, 0, total)          // 恒不超过总上下文
//! uncached_input = total - reported             // cap_ratio<1 保证恒为正
//! ```
//!
//! 三个运营可调参数（admin 面板热调，见 config.cache）：
//! - `multiplier`（缩放倍率，默认 1.8）：hit 乘此倍率换取更大折扣；
//! - `cap_ratio`（命中上限比率，默认 0.9）：上报封顶 = total × cap_ratio，杜绝假到全命中；
//! - `floor_ratio`（最低比率，默认 0.0）：上报下限 = total × floor_ratio。
//!   默认 0 → 冷启动/无命中如实报 0、不造假；调高可消灭吓人的 0% 全价行。

/// 缩放倍率默认值。运行时实际值来自 config.cache.readMultiplier（admin 可热调）。
pub const DEFAULT_CACHE_READ_MULTIPLIER: f64 = 1.8;

/// 命中上限比率默认值（上报封顶 = total × 此值）。
pub const DEFAULT_CACHE_CAP_RATIO: f64 = 0.9;

/// 最低比率默认值（上报下限 = total × 此值）。0 = 冷启动如实报 0、不造假。
pub const DEFAULT_CACHE_FLOOR_RATIO: f64 = 0.0;

/// 按"模拟器命中 × 倍率、再用上下限比率夹住"算上报 cache_read。
///
/// 按"模拟器命中比例 × 倍率、再用上下限比率夹住"算上报 cache_read。
///
/// **同口径修复（审查 CRITICAL）**：`hit` 与 `sim_total` 都来自模拟器自己的 tokenizer
/// （canon 串估算），二者比值 `frac = hit / sim_total` 才有稳定物理意义。而上报给中转
/// 网关的基准是 `report_total`（Kiro contextUsageEvent 的权威 token 数）。于是先在同口径
/// 内算命中比例，再把比例映射到权威基准并放大、夹限：
///
/// ```text
/// frac     = hit / sim_total                       // 同口径比例
/// reported = clamp(frac × report_total × mult, report_total × floor, report_total × cap)
/// reported = clamp(reported, 0, report_total)
/// ```
///
/// - `report_total <= 0`：返回 0（无上下文）。
/// - `sim_total <= 0`：命中比例无意义 → 返回 0（除非 floor>0 抬到 floor）。
///
/// 参数 clamp：`multiplier <= 0` 回退默认；`cap` 夹到 `[0,1]`；`floor` 夹到 `[0, cap]`。
pub fn reported_cache_read(
    report_total: i32,
    hit_tokens: i32,
    sim_total: i32,
    multiplier: f64,
    cap_ratio: f64,
    floor_ratio: f64,
) -> i32 {
    if report_total <= 0 {
        return 0;
    }
    let total = report_total as f64;
    let mult = if multiplier > 0.0 {
        multiplier
    } else {
        DEFAULT_CACHE_READ_MULTIPLIER
    };
    let cap = cap_ratio.clamp(0.0, 1.0);
    let floor = floor_ratio.clamp(0.0, cap);

    // 同口径命中比例（sim_total<=0 时无从算比例，frac=0，仅由 floor 决定下限）。
    let frac = if sim_total > 0 {
        (hit_tokens.max(0) as f64) / (sim_total as f64)
    } else {
        0.0
    };

    let scaled = frac * total * mult;
    let upper = total * cap;
    let lower = total * floor;
    let reported = scaled.clamp(lower, upper);
    (reported.round() as i32).clamp(0, report_total)
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
    fn zero_total_returns_zero() {
        assert_eq!(reported_cache_read(0, 100, 1000, 1.8, 0.9, 0.0), 0);
        assert_eq!(reported_cache_read(-1, 100, 1000, 1.8, 0.9, 0.0), 0);
    }

    #[test]
    fn zero_sim_total_returns_floor() {
        // sim_total<=0 → frac=0 → 仅由 floor 决定（floor=0 报 0）
        assert_eq!(reported_cache_read(10000, 100, 0, 1.8, 0.9, 0.0), 0);
        assert_eq!(reported_cache_read(10000, 100, 0, 1.8, 0.9, 0.3), 3000);
    }

    #[test]
    fn zero_hit_with_floor_zero_reports_zero() {
        // 冷启动/无命中 + floor=0 → 报 0，不造假
        assert_eq!(reported_cache_read(10000, 0, 10000, 1.8, 0.9, 0.0), 0);
    }

    #[test]
    fn zero_hit_with_floor_lifts_to_floor() {
        // floor=0.3 → 即便 hit=0 也抬到 total×0.3=3000（运营用它消灭 0% 行）
        assert_eq!(reported_cache_read(10000, 0, 10000, 1.8, 0.9, 0.3), 3000);
    }

    #[test]
    fn frac_scaled_below_cap() {
        // hit=2000/sim_total=10000 → frac=0.2；report_total=10000，×1.8 → 3600 < cap(9000)
        assert_eq!(reported_cache_read(10000, 2000, 10000, 1.8, 0.9, 0.0), 3600);
    }

    #[test]
    fn frac_scaled_clamped_to_cap() {
        // frac=0.6, ×1.8=1.08 → 1.08×10000=10800 > cap(9000) → 封顶 9000
        assert_eq!(reported_cache_read(10000, 6000, 10000, 1.8, 0.9, 0.0), 9000);
    }

    #[test]
    fn cross_tokenizer_uses_fraction_not_absolute() {
        // 关键(审查 CRITICAL 修复)：sim 口径与 report 口径不同也按比例映射，不混用绝对值。
        // sim: hit=3000/sim_total=6000 → frac=0.5；report_total=20000，×1.8 → 0.5×20000×1.8=18000
        //  > cap(0.9×20000=18000) → 恰好 18000
        assert_eq!(reported_cache_read(20000, 3000, 6000, 1.8, 0.9, 0.0), 18000);
        // 若错误地把 sim 的 hit=3000 当绝对值 ×1.8=5400，会严重低估——证明用的是比例
        assert_ne!(reported_cache_read(20000, 3000, 6000, 1.8, 0.9, 0.0), 5400);
    }

    #[test]
    fn reported_never_exceeds_total() {
        // frac=0.8, cap=1.0, ×1.8 → 1.44×1000=1440 > total → 夹到 total
        assert_eq!(reported_cache_read(1000, 800, 1000, 1.8, 1.0, 0.0), 1000);
    }

    #[test]
    fn preserves_variation_shape() {
        // 不同命中比例产出不同上报值，保留波动形状（会话越长命中越高）
        let total = 11000;
        let r_low = reported_cache_read(total, 1500, total, 1.8, 0.9, 0.0); // 早轮低命中
        let r_high = reported_cache_read(total, 5000, total, 1.8, 0.9, 0.0); // 晚轮高命中
        assert!(r_low < r_high, "上报值应随命中升高: {r_low} vs {r_high}");
        assert_eq!(r_low, 2700); // frac=1500/11000, ×11000×1.8 = 1500×1.8 = 2700
        assert_eq!(r_high, 9000); // 5000×1.8=9000 < cap(9900) → 9000
    }

    #[test]
    fn multiplier_nonpositive_falls_back_to_default() {
        // multiplier<=0 → 用默认 1.8
        assert_eq!(
            reported_cache_read(10000, 2000, 10000, 0.0, 0.9, 0.0),
            reported_cache_read(10000, 2000, 10000, DEFAULT_CACHE_READ_MULTIPLIER, 0.9, 0.0)
        );
    }

    #[test]
    fn floor_clamped_below_cap() {
        // floor 设得比 cap 高 → floor 被夹到 cap，结果不超过 cap×total
        // hit=0, cap=0.5, floor=0.9 → floor 夹到 0.5 → 报 total×0.5=5000
        assert_eq!(reported_cache_read(10000, 0, 10000, 1.8, 0.5, 0.9), 5000);
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
