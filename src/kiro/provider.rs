//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试
//! 支持按凭据级 endpoint 切换不同 Kiro API 端点

use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::endpoint::{KiroEndpoint, RequestContext};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::MultiTokenManager;
use crate::model::config::TlsBackend;
use parking_lot::Mutex;

// 重试 / 退避 / 超时参数已迁出为 config.json 的 `retry` 组（见 model::tuning::RetryConfig）。
// 这些值改造前是 provider 内的硬编码 const，现统一从 token_manager.config().retry 读取
// （冷读：启动时确定，运行时改需重启）。

/// API 调用成功后的输出
///
/// 调度层把"用了哪张账号、retry 几次、HTTP 状态"等信息一并交还给 handler 层，
/// 这样 handler 可以在 RequestRecord 里填上 `account_id` / `attempts` / `http_status`。
pub struct CallOutcome {
    pub response: reqwest::Response,
    pub credential_id: u64,
    /// 业务侧账号标识：优先用邮箱，回退到 "kiro-{id}"
    pub account_id: String,
    /// 订阅等级（KIRO PRO+ / KIRO FREE / ...）
    pub account_label: Option<String>,
    /// 第几次 attempt 成功（从 1 开始）
    pub attempts: u32,
    /// HTTP 状态码
    pub http_status: u16,
}

/// 上游 API 调用失败时的结构化错误
///
/// 携带"最后一次实际打的账号"信息，让 handler 在落库 error 日志时能填上
/// `account_id` / `account_label` / `http_status`，而不是显示 "-"。
/// 通过 `anyhow::Error::new(UpstreamCallError { .. })` 包装，下游用
/// `err.downcast_ref::<UpstreamCallError>()` 提取。
#[derive(Debug)]
pub struct UpstreamCallError {
    pub credential_id: Option<u64>,
    pub account_id: Option<String>,
    pub account_label: Option<String>,
    pub http_status: Option<u16>,
    pub attempts: u32,
    pub message: String,
}

impl std::fmt::Display for UpstreamCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UpstreamCallError {}

/// 构造 `UpstreamCallError` 包装的 anyhow::Error，附带最后尝试的账号 / HTTP 状态
fn upstream_err(
    ctx: &crate::kiro::token_manager::CallContext,
    http_status: Option<u16>,
    attempts: u32,
    message: String,
) -> anyhow::Error {
    let account_id = ctx
        .credentials
        .email
        .clone()
        .unwrap_or_else(|| format!("kiro-{}", ctx.id));
    let account_label = ctx.credentials.subscription_title.clone();
    anyhow::Error::new(UpstreamCallError {
        credential_id: Some(ctx.id),
        account_id: Some(account_id),
        account_label,
        http_status,
        attempts,
        message,
    })
}

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
/// 按凭据 `endpoint` 字段选择 [`KiroEndpoint`] 实现
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 全局代理配置（用于凭据无自定义代理时的回退）
    global_proxy: Option<ProxyConfig>,
    /// Client 缓存：key = effective proxy config, value = reqwest::Client
    /// 不同代理配置的凭据使用不同的 Client，共享相同代理的凭据复用 Client
    client_cache: Mutex<HashMap<Option<ProxyConfig>, Client>>,
    /// TLS 后端配置
    tls_backend: TlsBackend,
    /// 端点实现注册表（key: endpoint 名称）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称（凭据未指定 endpoint 时使用）
    default_endpoint: String,
}

impl KiroProvider {
    /// 创建带代理配置和端点注册表的 KiroProvider 实例
    ///
    /// # Arguments
    /// * `token_manager` - 多凭据 Token 管理器
    /// * `proxy` - 全局代理配置
    /// * `endpoints` - 端点名 → 实现的注册表（至少包含 `default_endpoint` 对应条目）
    /// * `default_endpoint` - 凭据未显式指定 endpoint 时使用的名称
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );
        let tls_backend = token_manager.config().tls_backend;
        let api_timeout = token_manager.config().retry.api_timeout_secs;
        // 预热：构建全局代理对应的 Client
        let initial_client = build_client(proxy.as_ref(), api_timeout, tls_backend)
            .expect("创建 HTTP 客户端失败");
        let mut cache = HashMap::new();
        cache.insert(proxy.clone(), initial_client);

        Self {
            token_manager,
            global_proxy: proxy,
            client_cache: Mutex::new(cache),
            tls_backend,
            endpoints,
            default_endpoint,
        }
    }

    /// 当前生效的缓存上报缩放倍率（运行时可调，读 token_manager live 值）
    ///
    /// handler 在每次请求时调用此方法，而非使用启动时的 AppState 快照，
    /// 这样 Admin 面板热调后立即对后续请求生效。
    pub fn cache_read_multiplier(&self) -> f64 {
        self.token_manager.get_cache_read_multiplier()
    }

    /// 当前生效的命中上限比率（运行时可调）
    pub fn cache_cap_ratio(&self) -> f64 {
        self.token_manager.get_cache_cap_ratio()
    }

    /// 当前生效的最低比率（运行时可调）
    pub fn cache_floor_ratio(&self) -> f64 {
        self.token_manager.get_cache_floor_ratio()
    }

    /// 根据凭据的代理配置获取（或创建并缓存）对应的 reqwest::Client
    fn client_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Client> {
        let effective = credentials.effective_proxy(self.global_proxy.as_ref());
        let mut cache = self.client_cache.lock();
        if let Some(client) = cache.get(&effective) {
            return Ok(client.clone());
        }
        let api_timeout = self.token_manager.config().retry.api_timeout_secs;
        let client = build_client(effective.as_ref(), api_timeout, self.tls_backend)?;
        cache.insert(effective, client.clone());
        Ok(client)
    }

    /// 根据凭据选择 endpoint 实现
    fn endpoint_for(
        &self,
        credentials: &KiroCredentials,
    ) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let name = credentials
            .endpoint
            .as_deref()
            .unwrap_or(&self.default_endpoint);
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移（见 [`Self::call_api_with_retry`]）。
    /// 返回值包含 `account_id` / `attempts` 等元信息，供日志层使用。
    pub async fn call_api(&self, request_body: &str) -> anyhow::Result<CallOutcome> {
        self.call_api_with_retry(request_body, false, None).await
    }

    /// 发送流式 API 请求
    pub async fn call_api_stream(&self, request_body: &str) -> anyhow::Result<CallOutcome> {
        self.call_api_with_retry(request_body, true, None).await
    }

    /// 发送 API 请求（带分组隔离）。`allowed_group` 见
    /// [`MultiTokenManager::acquire_context_with_session_and_group`]。
    pub async fn call_api_in_group(
        &self,
        request_body: &str,
        allowed_group: Option<std::collections::HashSet<u64>>,
    ) -> anyhow::Result<CallOutcome> {
        self.call_api_with_retry(request_body, false, allowed_group)
            .await
    }

    /// 发送流式 API 请求（带分组隔离）。
    pub async fn call_api_stream_in_group(
        &self,
        request_body: &str,
        allowed_group: Option<std::collections::HashSet<u64>>,
    ) -> anyhow::Result<CallOutcome> {
        self.call_api_with_retry(request_body, true, allowed_group)
            .await
    }

    /// 发送 MCP API 请求（WebSearch 等工具调用）
    pub async fn call_mcp(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        self.call_mcp_with_retry(request_body).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(&self, request_body: &str) -> anyhow::Result<reqwest::Response> {
        let total_credentials = self.token_manager.total_count();
        let retry_cfg = self.token_manager.config().retry;
        let max_retries = (total_credentials * retry_cfg.max_retries_per_credential as usize)
            .min(retry_cfg.max_total_retries as usize);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();

        for attempt in 0..max_retries {
            // MCP 调用（WebSearch 等工具）不涉及模型选择，无需按模型过滤凭据
            let ctx = match self.token_manager.acquire_context(None).await {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    // endpoint 解析失败：记为失败，换下一张凭据
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.mcp_url(&rctx);
            let body = endpoint.transform_mcp_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_mcp(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(self.retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                return Ok(response);
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 额度用尽
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 400 Bad Request
            if status.as_u16() == 400 {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self.token_manager.force_refresh_token_for(ctx.id).await.is_ok() {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                if !has_available {
                    anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                }
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(self.retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(self.retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 每个凭据最多重试 MAX_RETRIES_PER_CREDENTIAL 次
    /// - 总重试次数 = min(凭据数量 × 每凭据重试次数, MAX_TOTAL_RETRIES)
    /// - 硬上限 9 次，避免无限重试
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
        allowed_group: Option<std::collections::HashSet<u64>>,
    ) -> anyhow::Result<CallOutcome> {
        let total_credentials = self.token_manager.total_count();
        let retry_cfg = self.token_manager.config().retry;
        let max_retries = (total_credentials * retry_cfg.max_retries_per_credential as usize)
            .min(retry_cfg.max_total_retries as usize);
        let mut last_error: Option<anyhow::Error> = None;
        let mut force_refreshed: HashSet<u64> = HashSet::new();
        let api_type = if is_stream { "流式" } else { "非流式" };

        // 尝试从请求体中提取模型信息
        let model = Self::extract_model_from_request(request_body);
        // 提取 conversationId 作 session 亲和键：同会话稳定锁同账号，命中 Kiro prefix cache
        let session_key = Self::extract_conversation_id_from_request(request_body);

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token）
            let ctx = match self
                .token_manager
                .acquire_context_with_session_and_group(
                    model.as_deref(),
                    session_key.as_deref(),
                    allowed_group.clone(),
                )
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, config);

            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(e) => e,
                Err(e) => {
                    last_error = Some(e);
                    self.token_manager.report_failure(ctx.id);
                    continue;
                }
            };

            let rctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config,
            };

            let url = endpoint.api_url(&rctx);
            let body = endpoint.transform_api_body(request_body, &rctx);

            let base = self
                .client_for(&ctx.credentials)?
                .post(&url)
                .body(body)
                .header("content-type", "application/json")
                .header("Connection", "close");
            let request = endpoint.decorate_api(base, &rctx);

            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    // 网络错误通常是上游/链路瞬态问题，不应导致"禁用凭据"或"切换凭据"
                    // （否则一段时间网络抖动会把所有凭据都误禁用，需要重启才能恢复）
                    last_error = Some(upstream_err(
                        &ctx,
                        None,
                        (attempt + 1) as u32,
                        format!("API 请求发送失败: {}", e),
                    ));
                    if attempt + 1 < max_retries {
                        sleep(self.retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                let account_id = ctx
                    .credentials
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("kiro-{}", ctx.id));
                let account_label = ctx.credentials.subscription_title.clone();
                return Ok(CallOutcome {
                    response,
                    credential_id: ctx.id,
                    account_id,
                    account_label,
                    attempts: (attempt + 1) as u32,
                    http_status: status.as_u16(),
                });
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            // 402 Payment Required 且额度用尽：禁用凭据并故障转移
            if status.as_u16() == 402 && endpoint.is_monthly_request_limit(&body) {
                tracing::warn!(
                    "API 请求失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                let status_code = status.as_u16();
                if !has_available {
                    return Err(upstream_err(
                        &ctx,
                        Some(status_code),
                        (attempt + 1) as u32,
                        format!(
                            "{} API 请求失败（所有凭据已用尽）: {} {}",
                            api_type, status, body
                        ),
                    ));
                }

                last_error = Some(upstream_err(
                    &ctx,
                    Some(status_code),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
                continue;
            }

            // 400 Bad Request - 请求问题，重试/切换凭据无意义
            if status.as_u16() == 400 {
                return Err(upstream_err(
                    &ctx,
                    Some(status.as_u16()),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
            }

            // 401/403 - 更可能是凭据/权限问题：计入失败并允许故障转移
            if matches!(status.as_u16(), 401 | 403) {
                tracing::warn!(
                    "API 请求失败（可能为凭据错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );

                // token 被上游失效：先尝试 force-refresh，每凭据仅一次机会
                if endpoint.is_bearer_token_invalid(&body) && !force_refreshed.contains(&ctx.id) {
                    force_refreshed.insert(ctx.id);
                    tracing::info!("凭据 #{} token 疑似被上游失效，尝试强制刷新", ctx.id);
                    if self.token_manager.force_refresh_token_for(ctx.id).await.is_ok() {
                        tracing::info!("凭据 #{} token 强制刷新成功，重试请求", ctx.id);
                        continue;
                    }
                    tracing::warn!("凭据 #{} token 强制刷新失败，计入失败", ctx.id);
                }

                let has_available = self.token_manager.report_failure(ctx.id);
                let status_code = status.as_u16();
                if !has_available {
                    return Err(upstream_err(
                        &ctx,
                        Some(status_code),
                        (attempt + 1) as u32,
                        format!(
                            "{} API 请求失败（所有凭据已用尽）: {} {}",
                            api_type, status, body
                        ),
                    ));
                }

                last_error = Some(upstream_err(
                    &ctx,
                    Some(status_code),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
                continue;
            }

            // 429 - 限流：立即临时停用当前凭据（按配置冷却，到点自愈）并切换到其他号
            // （旧行为是重试同一个号，但 PRO+ 持续限流时反复打同号无意义）
            if status.as_u16() == 429 {
                tracing::warn!(
                    "API 请求命中 429 限流（尝试 {}/{}），停用当前凭据并切换: {}",
                    attempt + 1,
                    max_retries,
                    body
                );
                let has_available = self.token_manager.report_rate_limited(ctx.id);
                let status_code = status.as_u16();
                last_error = Some(upstream_err(
                    &ctx,
                    Some(status_code),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
                if !has_available {
                    return Err(upstream_err(
                        &ctx,
                        Some(status_code),
                        (attempt + 1) as u32,
                        format!(
                            "{} API 请求失败（所有凭据已限流/禁用）: {} {}",
                            api_type, status, body
                        ),
                    ));
                }
                continue;
            }

            // 408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            // （避免 502 high load 等瞬态错误把所有凭据锁死）
            if status.as_u16() == 408 || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                last_error = Some(upstream_err(
                    &ctx,
                    Some(status.as_u16()),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
                if attempt + 1 < max_retries {
                    sleep(self.retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                return Err(upstream_err(
                    &ctx,
                    Some(status.as_u16()),
                    (attempt + 1) as u32,
                    format!("{} API 请求失败: {} {}", api_type, status, body),
                ));
            }

            // 兜底：当作可重试的瞬态错误处理（不切换凭据）
            tracing::warn!(
                "API 请求失败（未知错误，尝试 {}/{}）: {} {}",
                attempt + 1,
                max_retries,
                status,
                body
            );
            last_error = Some(upstream_err(
                &ctx,
                Some(status.as_u16()),
                (attempt + 1) as u32,
                format!("{} API 请求失败: {} {}", api_type, status, body),
            ));
            if attempt + 1 < max_retries {
                sleep(self.retry_delay(attempt)).await;
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大重试次数（{}次）",
                api_type,
                max_retries
            )
        }))
    }

    /// 从请求体中提取模型信息
    ///
    /// 尝试解析 JSON 请求体，提取 conversationState.currentMessage.userInputMessage.modelId
    fn extract_model_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;

        json.get("conversationState")?
            .get("currentMessage")?
            .get("userInputMessage")?
            .get("modelId")?
            .as_str()
            .map(|s| s.to_string())
    }

    /// 从 Kiro 请求体里提取 conversationId（用作会话亲和的 session key）。
    /// 该 id 由 converter::derive_conversation_id_from_messages 基于前 2 条 user 消息
    /// 哈希得到，**同一会话连续 turn 之间稳定**，正好可做会话粘性路由的键。
    fn extract_conversation_id_from_request(request_body: &str) -> Option<String> {
        use serde_json::Value;

        let json: Value = serde_json::from_str(request_body).ok()?;
        json.get("conversationState")?
            .get("conversationId")?
            .as_str()
            .map(|s| s.to_string())
    }

    fn retry_delay(&self, attempt: usize) -> Duration {
        // 指数退避 + 少量抖动，避免上游抖动时放大故障。基础/上限来自 config.retry。
        let retry_cfg = self.token_manager.config().retry;
        let base_ms = retry_cfg.backoff_base_ms.max(1);
        let max_ms = retry_cfg.backoff_max_ms.max(base_ms);
        let exp = base_ms.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(max_ms);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }
}
