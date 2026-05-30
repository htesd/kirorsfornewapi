//! Kiro prompt cache 命中估计（纯函数，无 I/O）
//!
//! ## 背景
//!
//! Kiro 后端对长会话做 prefix cache，命中后 metering(credit) 明显低于全价。
//! 但回给 NewAPI 的 usage 不含缓存拆分，导致按全价计费、用户被多收。
//! 本模块用"无缓存成本基线"反推每次请求是否命中，供日志/计费使用。
//!
//! ## 模型
//!
//! ```text
//! metering ≈ a·input_tokens + b·output_tokens + c        （a,b,c 为每模型无缓存基线）
//! ratio    = 实际metering / 无缓存预测
//! 命中      当 ratio < HIT_THRESHOLD
//! ```
//!
//! **关键**：基线 (a,b,c) 必须用"未命中样本"拟合：
//! - Kiro 报 `cacheReadInputTokens` 的模型（opus-4-7 系列、sonnet-4-5 等）：
//!   用 `cached_tokens=0` 行作 ground truth。
//! - Kiro **不**报的模型（opus-4-6 系列、sonnet-4-6-thinking、haiku-4-5）：
//!   用短 prompt（<25k token）样本作"几乎未命中"代理。
//!
//! 若用含缓存数据拟合，缓存越多基线越被拉低、命中越测不出来（实测踩过这个坑）。
//!
//! 命中时反推 cache_read：折扣来自 cache_read 按 0.1× 计费 ——
//! ```text
//! metering - b·out - c = a·(input - cache_read) + 0.1·a·cache_read
//!                      = a·input - 0.9·a·cache_read
//! => cache_read = (a·input - (metering - b·out - c)) / (0.9·a)
//! ```
//!
//! ## thinking 变体单独拟合
//!
//! 数据显示 `claude-*-thinking` 的 b（输出单价）比非 thinking 高 30-60%，
//! 原因是 thinking 输出含推理 token、单位成本更高。共享 baseline 会在多输出
//! 场景下显著偏差。从 v19 起 thinking / 非 thinking 各持独立基线。
//!
//! ## 系数来源（v19 重拟合，n 见下表，c 强制 0）
//!
//! | 模型 | 数据源 | n | a | b | R² |
//! |---|---|---|---|---|---|
//! | opus-4-7 | cached=0（精确） | 381 | 8.37e-6 | 2.96e-4 | 0.58 |
//! | opus-4-7-thinking | cached=0 | 53 | 7.22e-6 | 4.06e-4 | 0.93 |
//! | opus-4-6 | cached=0（含估算回环） | 139 | 5.53e-6 | 2.45e-4 | 0.92 |
//! | opus-4-6-thinking | cached=0 | 52 | 6.90e-6 | 2.10e-4 | 0.71 |
//! | sonnet-4-5 | prompt<25k | 22 | 2.84e-6 | 1.37e-4 | 0.92 |
//! | sonnet-4-6-thinking | prompt<25k | 16 | 2.53e-6 | 3.93e-4 | 0.89 |
//! | haiku-4-5 | prompt<25k | 26 | 1.42e-6 | 3.90e-5 | 0.92 |

/// 命中判定阈值：ratio 低于此值视为命中。
/// v27 起放宽 0.8 → 0.9（更激进，多识别为命中，配合 95% 感知放大让用户账单更便宜）。
/// 代价：边界 miss（ratio 0.85-0.95 区间）会被误判为 hit、按 95% 缓存上报。
const HIT_THRESHOLD: f64 = 0.9;

/// cache_read 相对全价的"省下比例"——按 0.1× 计费即省 0.9。
const CACHE_READ_SAVING: f64 = 0.9;

/// 每模型无缓存成本基线：metering = a·input + b·output + c
struct Baseline {
    /// 每输入 token 的 credit（无缓存）
    a: f64,
    /// 每输出 token 的 credit
    b: f64,
    /// 固定开销（v19 起统一 0；非零会把短请求误判全命中）
    c: f64,
}

/// 按模型名取无缓存基线。未知模型返回 None（不分类，日志记 NULL）。
///
/// 匹配规则：模型名（lowercase）含相应关键字。"thinking" 后缀单独识别，
/// 与非 thinking 同模型走不同基线（输出单价差异显著）。
fn baseline_for(model: &str) -> Option<Baseline> {
    let m = model.to_ascii_lowercase();
    let has = |s: &str| m.contains(s);
    let v = |x: &str, y: &str| has(x) || has(y);
    let thinking = has("thinking");

    if has("opus") && v("4-8", "4.8") {
        // v27：Kiro 上游刚发布 opus-4.8（2026-05-29），样本不足以独立拟合，
        // 暂复用 4.7 baseline。Anthropic 通常迭代版本单价差异 <10%，等积累
        // 100+ 样本后用 sqlite + python OLS 重拟合。
        if thinking {
            Some(Baseline { a: 7.22e-6, b: 406.0e-6, c: 0.0 })
        } else {
            Some(Baseline { a: 8.37e-6, b: 296.0e-6, c: 0.0 })
        }
    } else if has("opus") && v("4-7", "4.7") {
        if thinking {
            Some(Baseline { a: 7.22e-6, b: 406.0e-6, c: 0.0 })
        } else {
            Some(Baseline { a: 8.37e-6, b: 296.0e-6, c: 0.0 })
        }
    } else if has("opus") && v("4-6", "4.6") {
        if thinking {
            Some(Baseline { a: 6.90e-6, b: 210.0e-6, c: 0.0 })
        } else {
            Some(Baseline { a: 5.53e-6, b: 245.0e-6, c: 0.0 })
        }
    } else if has("sonnet") && v("4-6", "4.6") && thinking {
        // 非 thinking sonnet-4-6 暂无生产样本，返回 None 让 NewAPI 全价兜底（保守）
        Some(Baseline { a: 2.53e-6, b: 393.0e-6, c: 0.0 })
    } else if has("sonnet") && v("4-5", "4.5") {
        // 旧 v16 基线 (6.5e-6, 130e-6) 高 2× 导致 100% 假命中；v19 用 n=22 重拟合修正
        Some(Baseline { a: 2.84e-6, b: 137.0e-6, c: 0.0 })
    } else if has("haiku") && v("4-5", "4.5") {
        Some(Baseline { a: 1.42e-6, b: 39.0e-6, c: 0.0 })
    } else {
        None
    }
}

/// 缓存命中估计结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CacheEstimate {
    /// 是否命中
    pub hit: bool,
    /// 实际/无缓存预测 比值（越小命中越深）
    pub ratio: f64,
    /// 反推的缓存读 token 数（miss 时为 0）
    pub cache_read_tokens: i32,
}

/// 估计一次请求是否命中 Kiro prompt cache。
///
/// 返回 None 表示无法判断（未知模型，或缺 metering/输入）。
pub fn estimate(
    model: &str,
    prompt_tokens: i32,
    output_tokens: i32,
    metering: f64,
) -> Option<CacheEstimate> {
    if prompt_tokens <= 0 || metering <= 0.0 {
        return None;
    }
    let base = baseline_for(model)?;

    let input = prompt_tokens as f64;
    let output = output_tokens.max(0) as f64;
    let expected_nocache = base.a * input + base.b * output + base.c;
    if expected_nocache <= 0.0 {
        return None;
    }

    let ratio = metering / expected_nocache;
    let hit = ratio < HIT_THRESHOLD;

    let cache_read_tokens = if hit {
        let input_cost = metering - base.b * output - base.c;
        let cr = (base.a * input - input_cost) / (CACHE_READ_SAVING * base.a);
        cr.clamp(0.0, input) as i32
    } else {
        0
    };

    Some(CacheEstimate {
        hit,
        ratio,
        cache_read_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ====== 兜底：未知模型与缺信号 ======

    #[test]
    fn unknown_model_returns_none() {
        assert!(estimate("gpt-4", 1000, 10, 0.05).is_none());
        // claude-3-5-haiku 样本太少未配 baseline；fall through 返回 None
        assert!(estimate("claude-3-5-haiku-20241022", 5000, 100, 0.01).is_none());
        // 非 thinking sonnet-4-6 无数据，安全返回 None
        assert!(estimate("claude-sonnet-4-6", 50000, 100, 0.2).is_none());
    }

    #[test]
    fn missing_signals_return_none() {
        assert!(estimate("claude-opus-4-7", 0, 10, 0.05).is_none());
        assert!(estimate("claude-opus-4-7", 1000, 10, 0.0).is_none());
    }

    // ====== opus-4-7（非 thinking）======

    #[test]
    fn opus47_long_cached_request_is_hit() {
        // 真实 cached=0 之前的样本：339k 输入、72 输出、1.276 credit
        // expected ≈ 8.37e-6·339116 + 296e-6·72 = 2.86  ratio ≈ 0.45 → HIT
        let e = estimate("claude-opus-4-7", 339116, 72, 1.2757).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
        assert!(e.ratio < 0.6, "ratio={}", e.ratio);
        assert!(e.cache_read_tokens > 150_000);
    }

    #[test]
    fn opus47_uncached_request_is_miss() {
        // 真实未命中样本：168k 输入、125 输出、6.33 credit → ratio ≈ 4.4，绝对 miss
        let e = estimate("claude-opus-4-7", 168656, 125, 6.3257).unwrap();
        assert!(!e.hit, "ratio={}", e.ratio);
        assert_eq!(e.cache_read_tokens, 0);
    }

    // ====== thinking / 非 thinking 必须走不同 baseline ======

    #[test]
    fn opus47_thinking_uses_separate_baseline() {
        // v27 阈值 0.9：找一组样本使 plain ratio > 0.9（miss）、thinking ratio < 0.9（hit）。
        // prompt=5000, compl=1500, met=0.55：
        //   plain expected = 8.37e-6·5000 + 296e-6·1500 = 0.486 → ratio ≈ 1.13 → MISS
        //   thinking expected = 7.22e-6·5000 + 406e-6·1500 = 0.645 → ratio ≈ 0.85 → HIT
        let plain = estimate("claude-opus-4-7", 5000, 1500, 0.55).unwrap();
        let think = estimate("claude-opus-4-7-thinking", 5000, 1500, 0.55).unwrap();
        assert!(!plain.hit, "plain ratio={}", plain.ratio);
        assert!(think.hit, "thinking ratio={}", think.ratio);
    }

    #[test]
    fn opus47_thinking_real_hit() {
        // 真实样本：151k 输入、6 输出、0.5625 credit → ratio ≈ 0.51 → HIT
        let e = estimate("claude-opus-4-7-thinking", 151480, 6, 0.5625).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
    }

    // ====== opus-4-8（v27 新增，复用 4-7 系数）======

    #[test]
    fn opus48_uses_same_baseline_as_47() {
        // 同样本在 4-7 和 4-8 上应得到完全一致的 ratio 和判定（baseline 复用）
        let e7 = estimate("claude-opus-4-7", 100000, 100, 0.5).unwrap();
        let e8 = estimate("claude-opus-4-8", 100000, 100, 0.5).unwrap();
        assert_eq!(e7.hit, e8.hit);
        assert!((e7.ratio - e8.ratio).abs() < 1e-9);
        assert_eq!(e7.cache_read_tokens, e8.cache_read_tokens);
    }

    #[test]
    fn opus48_thinking_uses_thinking_baseline() {
        // thinking 变体走独立 baseline（b 更高）
        let plain = estimate("claude-opus-4-8", 5000, 1500, 0.55).unwrap();
        let think = estimate("claude-opus-4-8-thinking", 5000, 1500, 0.55).unwrap();
        assert!(!plain.hit, "plain 4-8 ratio={}", plain.ratio);
        assert!(think.hit, "thinking 4-8 ratio={}", think.ratio);
    }

    // ====== opus-4-6（非 thinking）======

    #[test]
    fn opus46_long_cached_request_is_hit() {
        // 真实样本：101k 输入、19 输出、0.379 credit → ratio ≈ 0.67 → HIT
        let e = estimate("claude-opus-4-6", 101288, 19, 0.3785).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
        assert!(e.cache_read_tokens > 30_000);
    }

    #[test]
    fn opus46_short_request_is_miss() {
        // 真实样本：26k 输入、80 输出、0.223 credit → ratio ≈ 1.37 → MISS
        let e = estimate("claude-opus-4-6", 25820, 80, 0.2231).unwrap();
        assert!(!e.hit, "ratio={}", e.ratio);
        assert_eq!(e.cache_read_tokens, 0);
    }

    #[test]
    fn opus46_thinking_uses_separate_baseline() {
        // opus-4-6 thinking 的 a 比非 thinking 高（6.90 vs 5.53）
        // 真实 thinking 命中样本：127k 输入、0 输出、0.469 credit → ratio ≈ 0.54
        let e = estimate("claude-opus-4-6-thinking", 127082, 0, 0.4692).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
    }

    // ====== sonnet-4-5（旧基线高 2×，v19 大修正）======

    #[test]
    fn sonnet45_hit_with_corrected_baseline() {
        // 真实样本：22k 输入、266 输出、0.0756 credit
        // 新 baseline (2.84e-6, 137e-6, 0)：expected ≈ 0.099、ratio ≈ 0.76 → HIT
        let e = estimate("claude-sonnet-4-5", 22194, 266, 0.0756).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
    }

    #[test]
    fn sonnet45_large_request_is_miss() {
        // 真实样本：130k 输入、1008 输出、0.679 credit → ratio ≈ 1.34 → MISS
        let e = estimate("claude-sonnet-4-5", 130229, 1008, 0.6791).unwrap();
        assert!(!e.hit, "ratio={}", e.ratio);
    }

    // ====== 新增模型 ======

    #[test]
    fn haiku45_hit_with_new_baseline() {
        // 真实样本：43k 输入、49 输出、0.0365 credit → ratio ≈ 0.58 → HIT
        let e = estimate("claude-haiku-4-5-20251001", 43194, 49, 0.0365).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
        assert!(e.cache_read_tokens > 10_000);
    }

    #[test]
    fn sonnet46_thinking_hit() {
        // 真实样本：28k 输入、1263 输出、0.312 credit → ratio ≈ 0.55 → HIT
        let e = estimate("claude-sonnet-4-6-thinking", 28073, 1263, 0.3119).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
    }

    // ====== 边界 ======

    #[test]
    fn cache_read_never_exceeds_prompt() {
        // 极低 metering 的极端情况，cache_read 仍夹在 [0, prompt]
        let e = estimate("claude-opus-4-7", 100000, 10, 0.001).unwrap();
        assert!(e.hit);
        assert!(e.cache_read_tokens <= 100000);
        assert!(e.cache_read_tokens >= 0);
    }
}
