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
//! **关键**：基线 (a,b,c) 必须用"未命中样本"(短会话/首轮)拟合。若用含缓存的数据
//! 拟合，缓存越多基线越被拉低，命中越测不出来（实测踩过这个坑）。
//!
//! 命中时反推 cache_read：折扣来自 cache_read 按 0.1× 计费 ——
//! ```text
//! metering - b·out - c = a·(input - cache_read) + 0.1·a·cache_read
//!                      = a·input - 0.9·a·cache_read
//! => cache_read = (a·input - (metering - b·out - c)) / (0.9·a)
//! ```
//!
//! 系数来自生产请求日志拟合（opus-4-7 R²≈0.79，可信；sonnet-4-5 短会话样本少，
//! 暂为经验值）。后续应定期用 DB 重新拟合；偶有误判可接受（计费近似，非审计）。

/// 命中判定阈值：ratio 低于此值视为命中。
/// miss 簇实测约 0.95，hit 簇约 0.57，0.8 落在天然空隙里，鲁棒。
const HIT_THRESHOLD: f64 = 0.8;

/// cache_read 相对全价的"省下比例"——按 0.1× 计费即省 0.9。
const CACHE_READ_SAVING: f64 = 0.9;

/// 每模型无缓存成本基线：metering = a·input + b·output + c
struct Baseline {
    /// 每输入 token 的 credit（无缓存）
    a: f64,
    /// 每输出 token 的 credit
    b: f64,
    /// 固定开销
    c: f64,
}

/// 按模型名取无缓存基线。未知模型返回 None（不分类，日志记 NULL）。
///
/// 匹配客户端原始模型名（如 "claude-opus-4-7"），与 DB 里存的 model 一致。
fn baseline_for(model: &str) -> Option<Baseline> {
    let m = model.to_ascii_lowercase();
    let has = |s: &str| m.contains(s);
    let v = |x: &str, y: &str| has(x) || has(y);

    if has("opus") && v("4-7", "4.7") {
        // 生产数据拟合，n=155 短会话，R²≈0.79，可信
        Some(Baseline { a: 7.12e-6, b: 187.0e-6, c: 0.011 })
    } else if has("opus") && v("4-6", "4.6") {
        // opencode 走 opus-4-6(thinking)。实测 Kiro 对该模型**不报** cacheReadInputTokens
        // (tokenUsageEvent 缺该字段)，导致命中也按全价计费 → 必须回退估算。
        // n=218 迭代拟合未命中样本，强制 c=0（thinking output 未计入 completion，
        // 其成本摊入 a；带 c 拟合会得 c≈0.16 把小请求误判成全命中，故弃用）。
        // 命中率约 62%；大会话(成本重点)判定准：命中反推 cache_read ~48%、未命中判 0。
        Some(Baseline { a: 6.48e-6, b: 112.0e-6, c: 0.0 })
    } else if has("sonnet") && v("4-5", "4.5") {
        // 短会话样本少(~10)，经验值，待更多数据重新拟合
        Some(Baseline { a: 6.5e-6, b: 130.0e-6, c: 0.0 })
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

    #[test]
    fn unknown_model_returns_none() {
        assert!(estimate("gpt-4", 1000, 10, 0.05).is_none());
        assert!(estimate("claude-haiku-4-5", 1000, 10, 0.05).is_none());
    }

    #[test]
    fn missing_signals_return_none() {
        assert!(estimate("claude-opus-4-7", 0, 10, 0.05).is_none());
        assert!(estimate("claude-opus-4-7", 1000, 10, 0.0).is_none());
    }

    #[test]
    fn opus_long_cached_request_is_hit() {
        // 实测样本：247k 输入、73 输出、0.956 credit —— 无缓存该花 ~1.76，明显命中
        let e = estimate("claude-opus-4-7", 247149, 73, 0.956).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
        assert!(e.ratio < 0.6, "ratio={}", e.ratio);
        // 反推缓存读应在合理范围（约一半以上输入被缓存）
        assert!(e.cache_read_tokens > 100_000 && e.cache_read_tokens <= 247149);
    }

    #[test]
    fn opus_short_fresh_request_is_miss() {
        // 短会话首轮：~20k 输入，无缓存预测≈0.191 credit，实测 ratio 中位≈0.95 → miss
        let e = estimate("claude-opus-4-7", 20000, 200, 0.181).unwrap();
        assert!(!e.hit, "ratio={}", e.ratio);
        assert_eq!(e.cache_read_tokens, 0);
    }

    #[test]
    fn opus46_long_cached_request_is_hit() {
        // opencode 大会话命中样本：160k 输入、618 输出、0.654 credit。
        // 无缓存预测≈1.06，ratio≈0.61 → 命中，反推 cache_read 约一半输入。
        let e = estimate("claude-opus-4-6-thinking", 160000, 618, 0.654).unwrap();
        assert!(e.hit, "ratio={}", e.ratio);
        assert!(e.cache_read_tokens > 60_000 && e.cache_read_tokens <= 160_000,
            "cache_read={}", e.cache_read_tokens);
    }

    #[test]
    fn opus46_long_uncached_request_is_miss() {
        // 同规模未命中样本：160k 输入、618 输出、1.335 credit，ratio>1 → miss。
        let e = estimate("claude-opus-4-6-thinking", 160000, 618, 1.335).unwrap();
        assert!(!e.hit, "ratio={}", e.ratio);
        assert_eq!(e.cache_read_tokens, 0);
    }

    #[test]
    fn cache_read_never_exceeds_prompt() {
        // 极低 metering 的极端情况，cache_read 仍夹在 [0, prompt]
        let e = estimate("claude-opus-4-7", 100000, 10, 0.001).unwrap();
        assert!(e.hit);
        assert!(e.cache_read_tokens <= 100000);
        assert!(e.cache_read_tokens >= 0);
    }
}
