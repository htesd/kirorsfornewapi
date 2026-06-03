//! Admin API 类型定义

use serde::{Deserialize, Serialize};

// ============ 凭据状态 ============

/// 所有凭据状态响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatusResponse {
    /// 凭据总数
    pub total: usize,
    /// 可用凭据数量（未禁用）
    pub available: usize,
    /// 当前活跃凭据 ID
    pub current_id: u64,
    /// 各凭据状态列表
    pub credentials: Vec<CredentialStatusItem>,
}

/// 单个凭据的状态信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatusItem {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级（数字越小优先级越高）
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// 是否为当前活跃凭据
    pub is_current: bool,
    /// Token 过期时间（RFC3339 格式）
    pub expires_at: Option<String>,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// refreshToken 的 SHA-256 哈希（仅 OAuth 凭据，用于前端去重）
    pub refresh_token_hash: Option<String>,
    /// kiroApiKey 的 SHA-256 哈希（仅 API Key 凭据，用于前端去重）
    pub api_key_hash: Option<String>,
    /// kiroApiKey 的脱敏展示（仅 API Key 凭据，用于前端显示）
    pub masked_api_key: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 是否配置了凭据级代理
    pub has_proxy: bool,
    /// 代理 URL（用于前端展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 禁用原因
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// 端点名称（决定该凭据走哪套 Kiro API，已回退到默认端点）
    pub endpoint: String,
    /// 该凭据的并发上限
    pub max_concurrency: u32,
    /// 当前正在该凭据上执行的请求数（已被 Semaphore 占用的 permit 数）
    pub in_flight: u32,
}

// ============ 操作请求 ============

/// 启用/禁用凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDisabledRequest {
    /// 是否禁用
    pub disabled: bool,
}

/// 修改优先级请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPriorityRequest {
    /// 新优先级值
    pub priority: u32,
}

/// 添加凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialRequest {
    /// 刷新令牌（OAuth 凭据必填，API Key 凭据不需要）
    pub refresh_token: Option<String>,

    /// 认证方式（可选，默认 social）
    #[serde(default = "default_auth_method")]
    pub auth_method: String,

    /// OIDC Client ID（IdC 认证需要）
    pub client_id: Option<String>,

    /// OIDC Client Secret（IdC 认证需要）
    pub client_secret: Option<String>,

    /// 优先级（可选，默认 0）
    #[serde(default)]
    pub priority: u32,

    /// 凭据级 Region 配置（用于 OIDC token 刷新）
    /// 未配置时回退到 config.json 的全局 region
    pub region: Option<String>,

    /// 凭据级 Auth Region（用于 Token 刷新）
    pub auth_region: Option<String>,

    /// 凭据级 API Region（用于 API 请求）
    pub api_region: Option<String>,

    /// 凭据级 Machine ID（可选，64 位字符串）
    /// 未配置时回退到 config.json 的 machineId
    pub machine_id: Option<String>,

    /// 用户邮箱（可选，用于前端显示）
    pub email: Option<String>,

    /// 凭据级代理 URL（可选，特殊值 "direct" 表示不使用代理）
    pub proxy_url: Option<String>,

    /// 凭据级代理认证用户名（可选）
    pub proxy_username: Option<String>,

    /// 凭据级代理认证密码（可选）
    pub proxy_password: Option<String>,

    /// Kiro API Key（API Key 凭据必填，格式: ksk_xxxxxxxx）
    /// 设置后直接作为 Bearer Token 使用，无需 refreshToken
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kiro_api_key: Option<String>,

    /// 端点名称（可选，未配置时使用 config.defaultEndpoint）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

fn default_auth_method() -> String {
    "social".to_string()
}

/// 添加凭据成功响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialResponse {
    pub success: bool,
    pub message: String,
    /// 新添加的凭据 ID
    pub credential_id: u64,
    /// 用户邮箱（如果获取成功）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

// ============ 余额查询 ============

/// 余额查询响应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// 凭据 ID
    pub id: u64,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 当前使用量
    pub current_usage: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 剩余额度
    pub remaining: f64,
    /// 使用百分比
    pub usage_percentage: f64,
    /// 下次重置时间（Unix 时间戳）
    pub next_reset_at: Option<f64>,
}

// ============ 负载均衡配置 ============

/// 负载均衡模式响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancingModeResponse {
    /// 当前模式（"priority" 或 "balanced"）
    pub mode: String,
}

/// 设置负载均衡模式请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLoadBalancingModeRequest {
    /// 模式（"priority" 或 "balanced"）
    pub mode: String,
}

/// 限流冷却时长响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitCooldownResponse {
    /// 命中 429 后凭据自动停用的冷却时长（秒）
    pub cooldown_secs: u64,
}

/// 设置限流冷却时长请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRateLimitCooldownRequest {
    /// 冷却时长（秒）
    pub cooldown_secs: u64,
}

/// 调度策略聚合响应（供调度面板一次拉取全部参数）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulingResponse {
    /// 负载均衡模式（"priority" / "balanced" / "affinity"）
    pub mode: String,
    /// 命中 429 后凭据自动停用的冷却时长（秒）
    pub cooldown_secs: u64,
    /// 会话亲和：次选连续命中多少次后转正
    pub affinity_promote_threshold: u32,
    /// 会话亲和映射 TTL（秒）
    pub affinity_map_ttl_secs: u64,
    /// 缓存上报缩放倍率（config.cache.readMultiplier）
    pub cache_read_multiplier: f64,
    /// 命中上限比率（config.cache.capRatio）：上报封顶 = total × 此值
    pub cache_cap_ratio: f64,
    /// 最低比率（config.cache.floorRatio）：上报下限 = total × 此值
    pub cache_floor_ratio: f64,
    /// 缓存模拟器条目 TTL（秒，config.cache.simTtlSecs）
    pub cache_sim_ttl_secs: u64,
    /// 缓存模拟器最大会话数（config.cache.maxSessions）
    pub cache_max_sessions: usize,
}

/// 更新调度策略请求（各字段可选，仅更新提供的项）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSchedulingRequest {
    pub mode: Option<String>,
    pub cooldown_secs: Option<u64>,
    pub affinity_promote_threshold: Option<u32>,
    pub affinity_map_ttl_secs: Option<u64>,
    /// 缓存上报缩放倍率。缺省=不改
    pub cache_read_multiplier: Option<f64>,
    /// 命中上限比率（0-1，会被 clamp）。缺省=不改
    pub cache_cap_ratio: Option<f64>,
    /// 最低比率（0-1，会被 clamp）。缺省=不改
    pub cache_floor_ratio: Option<f64>,
    /// 缓存模拟器 TTL（秒）。缺省=不改
    pub cache_sim_ttl_secs: Option<u64>,
    /// 缓存模拟器最大会话数。缺省=不改
    pub cache_max_sessions: Option<usize>,
}

// ============ 反代 API Key 配置（多 key） ============

/// 单个反代访问密钥（脱敏，不回传完整密钥）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyItem {
    /// 行 ID（删除时用）
    pub id: i64,
    /// 脱敏后的密钥展示（如 `sk-4***b!`）
    pub masked: String,
    /// 备注标签
    pub label: Option<String>,
    /// 创建时间（Unix 毫秒）
    pub created_at: i64,
    /// 是否禁用（true = 已停用，认证时不匹配）
    pub disabled: bool,
    /// 绑定的分组 id（None = 未绑定，可用全部账号）
    pub group_id: Option<i64>,
}

/// 设置 API Key 禁用状态请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyDisabledRequest {
    pub disabled: bool,
}

/// 设置 API Key 分组绑定请求（groupId=null 表示解绑）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetApiKeyGroupRequest {
    pub group_id: Option<i64>,
}

/// 反代访问密钥列表响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeysResponse {
    pub keys: Vec<ApiKeyItem>,
}

/// 新增反代访问密钥请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddApiKeyRequest {
    /// 新的反代访问密钥
    pub key: String,
    /// 备注标签（可选）
    pub label: Option<String>,
}

// ============ 账号池分组 ============

/// 单个分组（含成员凭据 id 列表）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupItem {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    /// 该分组下的凭据 id 列表
    pub credential_ids: Vec<u64>,
}

/// 分组列表响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupsResponse {
    pub groups: Vec<GroupItem>,
}

/// 新增 / 重命名分组请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupNameRequest {
    pub name: String,
}

/// 设置某凭据的分组归属请求（groupId=null 表示移出分组）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetCredentialGroupRequest {
    pub group_id: Option<i64>,
}

// ============ 通用响应 ============

/// 操作成功响应
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
    pub message: String,
}

impl SuccessResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

/// 错误响应
#[derive(Debug, Serialize)]
pub struct AdminErrorResponse {
    pub error: AdminError,
}

#[derive(Debug, Serialize)]
pub struct AdminError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl AdminErrorResponse {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: AdminError {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid or missing admin API key")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new("api_error", message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new("internal_error", message)
    }
}
