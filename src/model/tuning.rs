//! 可调运营参数配置分组
//!
//! 把此前散落在各模块的硬编码 `const` 收口到 config.json 的分组对象里：
//! - [`CacheConfig`] —— 缓存模拟/计费相关（**运行时可热调**，走 token_manager 运行时 cell）
//! - [`RetryConfig`] —— 上游重试/退避/超时（启动时读，改动需重启）
//! - [`CredentialConfig`] —— 凭据并发/失败/token 刷新（启动时读）
//!
//! 设计要点：
//! - 每个字段 `#[serde(default = ...)]`，默认值**等于原先的 const 值**，保证旧 config.json
//!   不含这些键时行为与改造前**完全一致**（向后兼容硬约束）。
//! - 镜像既有 [`crate::db::RequestLogConfig`] 的嵌套子结构范式。

use serde::{Deserialize, Serialize};

/// 缓存模拟 / 计费相关参数（运行时可热调）。
///
/// 对应 config.json 的 `cache` 对象。这些项调整后应立即生效（计费/命中直接相关，
/// 运营会频繁调），故在 token_manager 里持有运行时 cell，并经 admin 接口热更新 + 持久化。
///
/// 注：缓存上报封顶比例沿用既有顶层字段 `perceivedCacheHitRatio`（未并入此组以保持
/// 向后兼容），逻辑上属于本组。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheConfig {
    /// prefix 缓存模拟器条目存活时间（秒）。超时后下一轮视为冷启动 miss。
    /// 原 `cache_sim::ENTRY_TTL`（300）。
    #[serde(default = "default_sim_ttl_secs")]
    pub sim_ttl_secs: u64,

    /// 缓存模拟器最多保留的会话数（LRU 淘汰）。原 `cache_sim::MAX_SESSIONS`（4096）。
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,

    /// cache_read 上报锚定放大倍率：`reported = clamp(real × multiplier, real, prompt×cap)`。
    /// 原 `usage::CACHE_READ_MULTIPLIER`（1.3）。
    #[serde(default = "default_read_multiplier")]
    pub read_multiplier: f64,

    /// metering 反推命中判定阈值：`ratio < threshold` 视为命中。
    /// 原 `cache_estimate::HIT_THRESHOLD`（0.8）。仅影响末级估算兜底。
    #[serde(default = "default_hit_threshold")]
    pub hit_threshold: f64,
}

fn default_sim_ttl_secs() -> u64 {
    300
}
fn default_max_sessions() -> usize {
    4096
}
fn default_read_multiplier() -> f64 {
    1.3
}
fn default_hit_threshold() -> f64 {
    0.8
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            sim_ttl_secs: default_sim_ttl_secs(),
            max_sessions: default_max_sessions(),
            read_multiplier: default_read_multiplier(),
            hit_threshold: default_hit_threshold(),
        }
    }
}

/// 上游重试 / 退避 / 超时参数（启动时读，改动需重启）。
///
/// 对应 config.json 的 `retry` 对象。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    /// 单凭据最多重试次数。原 `provider::MAX_RETRIES_PER_CREDENTIAL`（3）。
    #[serde(default = "default_max_retries_per_credential")]
    pub max_retries_per_credential: u32,

    /// 跨凭据故障转移的总重试上限。原 `provider::MAX_TOTAL_RETRIES`（9）。
    #[serde(default = "default_max_total_retries")]
    pub max_total_retries: u32,

    /// 指数退避基础延迟（毫秒）。原 `provider` BASE_MS（200）。
    #[serde(default = "default_backoff_base_ms")]
    pub backoff_base_ms: u64,

    /// 指数退避最大延迟（毫秒）。原 `provider` MAX_MS（2000）。
    #[serde(default = "default_backoff_max_ms")]
    pub backoff_max_ms: u64,

    /// 上游 API 主请求 HTTP 超时（秒）。原 `provider` build_client 硬编码 720。
    #[serde(default = "default_api_timeout_secs")]
    pub api_timeout_secs: u64,
}

fn default_max_retries_per_credential() -> u32 {
    3
}
fn default_max_total_retries() -> u32 {
    9
}
fn default_backoff_base_ms() -> u64 {
    200
}
fn default_backoff_max_ms() -> u64 {
    2000
}
fn default_api_timeout_secs() -> u64 {
    720
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries_per_credential: default_max_retries_per_credential(),
            max_total_retries: default_max_total_retries(),
            backoff_base_ms: default_backoff_base_ms(),
            backoff_max_ms: default_backoff_max_ms(),
            api_timeout_secs: default_api_timeout_secs(),
        }
    }
}

/// 凭据并发 / 失败 / token 刷新参数（启动时读，改动需重启）。
///
/// 对应 config.json 的 `credential` 对象。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialConfig {
    /// 单凭据连续 API 失败多少次后自动禁用。原 `token_manager::MAX_FAILURES_PER_CREDENTIAL`（3）。
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// 单凭据默认最大并发数。原 `token_manager::MAX_CONCURRENCY_PER_CREDENTIAL`（2）。
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: usize,

    /// token 提前判过期的余量（秒）。原 `token_manager` is_token_expired 硬编码 300。
    #[serde(default = "default_token_expiry_margin_secs")]
    pub token_expiry_margin_secs: u64,

    /// token "即将过期"判定窗口（秒）。原 `token_manager` is_token_expiring_soon 硬编码 600。
    #[serde(default = "default_token_expiring_soon_secs")]
    pub token_expiring_soon_secs: u64,

    /// token 刷新 / 额度查询 HTTP 超时（秒）。原 `token_manager` build_client 硬编码 60。
    #[serde(default = "default_refresh_timeout_secs")]
    pub refresh_timeout_secs: u64,
}

fn default_max_failures() -> u32 {
    3
}
fn default_max_concurrency() -> usize {
    2
}
fn default_token_expiry_margin_secs() -> u64 {
    300
}
fn default_token_expiring_soon_secs() -> u64 {
    600
}
fn default_refresh_timeout_secs() -> u64 {
    60
}

impl Default for CredentialConfig {
    fn default() -> Self {
        Self {
            max_failures: default_max_failures(),
            max_concurrency: default_max_concurrency(),
            token_expiry_margin_secs: default_token_expiry_margin_secs(),
            token_expiring_soon_secs: default_token_expiring_soon_secs(),
            refresh_timeout_secs: default_refresh_timeout_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_defaults_match_original_consts() {
        let c = CacheConfig::default();
        assert_eq!(c.sim_ttl_secs, 300);
        assert_eq!(c.max_sessions, 4096);
        assert_eq!(c.read_multiplier, 1.3);
        assert_eq!(c.hit_threshold, 0.8);
    }

    #[test]
    fn retry_defaults_match_original_consts() {
        let r = RetryConfig::default();
        assert_eq!(r.max_retries_per_credential, 3);
        assert_eq!(r.max_total_retries, 9);
        assert_eq!(r.backoff_base_ms, 200);
        assert_eq!(r.backoff_max_ms, 2000);
        assert_eq!(r.api_timeout_secs, 720);
    }

    #[test]
    fn credential_defaults_match_original_consts() {
        let c = CredentialConfig::default();
        assert_eq!(c.max_failures, 3);
        assert_eq!(c.max_concurrency, 2);
        assert_eq!(c.token_expiry_margin_secs, 300);
        assert_eq!(c.token_expiring_soon_secs, 600);
        assert_eq!(c.refresh_timeout_secs, 60);
    }

    #[test]
    fn empty_json_yields_all_defaults() {
        // 向后兼容硬约束：空对象 {} 反序列化后 = 全默认（= 旧 const 值）
        let c: CacheConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(c.read_multiplier, 1.3);
        let r: RetryConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(r.max_total_retries, 9);
        let cr: CredentialConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cr.max_concurrency, 2);
    }

    #[test]
    fn partial_json_overrides_only_named_field() {
        // 只给一个字段，其余仍默认
        let c: CacheConfig = serde_json::from_str(r#"{"readMultiplier": 1.5}"#).unwrap();
        assert_eq!(c.read_multiplier, 1.5);
        assert_eq!(c.sim_ttl_secs, 300); // 未给 → 默认
    }
}
