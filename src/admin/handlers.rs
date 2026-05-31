//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        AddApiKeyRequest, AddCredentialRequest, ApiKeyItem, ApiKeysResponse, GroupItem,
        GroupNameRequest, GroupsResponse, SetApiKeyGroupRequest, SetCredentialGroupRequest,
        SetDisabledRequest, SetLoadBalancingModeRequest, SetPriorityRequest,
        SetRateLimitCooldownRequest, SuccessResponse, UpdateSchedulingRequest,
    },
};

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/rate-limit-cooldown
/// 获取限流冷却时长
pub async fn get_rate_limit_cooldown(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_rate_limit_cooldown())
}

/// PUT /api/admin/config/rate-limit-cooldown
/// 设置限流冷却时长
pub async fn set_rate_limit_cooldown(
    State(state): State<AdminState>,
    Json(payload): Json<SetRateLimitCooldownRequest>,
) -> impl IntoResponse {
    match state.service.set_rate_limit_cooldown(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/scheduling
/// 获取调度策略全部参数（模式 + 冷却 + 亲和 K + 亲和 TTL）
pub async fn get_scheduling(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_scheduling())
}

/// PUT /api/admin/config/scheduling
/// 更新调度策略（各字段可选）
pub async fn update_scheduling(
    State(state): State<AdminState>,
    Json(payload): Json<UpdateSchedulingRequest>,
) -> impl IntoResponse {
    match state.service.update_scheduling(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

// ============================================================================
// 反代 API Key 配置（多 key）
// ============================================================================

/// 脱敏展示密钥：保留首 4、尾 2，中间用 `***` 代替；过短则全部隐藏
fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "***".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{}***{}", head, tail)
}

/// 从 DB 重新加载 key 列表到内存句柄，保持两者一致
async fn reload_keys_into_memory(state: &AdminState) {
    let path = state.keys_db_path.clone();
    if let Ok(Ok(keys)) =
        tokio::task::spawn_blocking(move || crate::db::api_keys::list_keys(&path)).await
    {
        *state.api_keys.write() = keys;
    }
}

/// GET /api/admin/api-keys
/// 列出全部反代访问密钥（脱敏）
pub async fn list_api_keys(State(state): State<AdminState>) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let result =
        tokio::task::spawn_blocking(move || crate::db::api_keys::list(&path)).await;

    match result {
        Ok(Ok(rows)) => {
            let keys = rows
                .into_iter()
                .map(|r| ApiKeyItem {
                    id: r.id,
                    masked: mask_api_key(&r.key),
                    label: r.label,
                    created_at: r.created_at,
                    disabled: r.disabled,
                    group_id: r.group_id,
                })
                .collect();
            Json(ApiKeysResponse { keys }).into_response()
        }
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// POST /api/admin/api-keys
/// 新增反代访问密钥：持久化到 SQLite + 立即更新内存句柄
pub async fn add_api_key(
    State(state): State<AdminState>,
    Json(payload): Json<AddApiKeyRequest>,
) -> impl IntoResponse {
    let key = payload.key.trim().to_string();
    if key.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "key 不能为空" })),
        )
            .into_response();
    }
    let label = payload
        .label
        .and_then(|l| {
            let t = l.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        });

    let path = state.keys_db_path.clone();
    let key_for_db = key.clone();
    let persist = tokio::task::spawn_blocking(move || {
        crate::db::api_keys::add(&path, &key_for_db, label.as_deref())
    })
    .await;

    match persist {
        Ok(Ok(_id)) => {
            reload_keys_into_memory(&state).await;
            Json(SuccessResponse::new("API Key 已添加")).into_response()
        }
        Ok(Err(e)) => {
            // UNIQUE 约束冲突 → 友好提示
            let msg = e.to_string();
            if msg.contains("UNIQUE") || msg.contains("constraint") {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": "该 API Key 已存在" })),
                )
                    .into_response()
            } else {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("持久化失败: {}", msg) })),
                )
                    .into_response()
            }
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/api-keys/:id
/// 删除反代访问密钥（保留至少一个，避免锁死）
pub async fn delete_api_key(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let op = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let total = crate::db::api_keys::count(&path)?;
        if total <= 1 {
            anyhow::bail!("至少保留一个 API Key");
        }
        Ok(crate::db::api_keys::delete(&path, id)?)
    })
    .await;

    match op {
        Ok(Ok(0)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("API Key #{} 不存在", id) })),
        )
            .into_response(),
        Ok(Ok(_)) => {
            reload_keys_into_memory(&state).await;
            Json(SuccessResponse::new(format!("API Key #{} 已删除", id))).into_response()
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            let code = if msg.contains("至少保留") {
                axum::http::StatusCode::BAD_REQUEST
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// POST /api/admin/api-keys/:id/disabled
/// 设置 API Key 启用/禁用状态。禁用后中间件认证立即不再匹配此 key。
/// 至少要留一个**启用**的 key，否则反代会锁死自己。
pub async fn set_api_key_disabled(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
    Json(payload): Json<crate::admin::types::SetApiKeyDisabledRequest>,
) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let want_disabled = payload.disabled;
    let op = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        // 禁用前检查：禁用后是否还有启用的 key，没有就拒绝
        if want_disabled {
            let all = crate::db::api_keys::list(&path)?;
            let still_enabled = all
                .iter()
                .filter(|r| !r.disabled && r.id != id)
                .count();
            if still_enabled == 0 {
                anyhow::bail!("至少保留一个启用的 API Key");
            }
        }
        Ok(crate::db::api_keys::set_disabled(&path, id, want_disabled)?)
    })
    .await;

    match op {
        Ok(Ok(0)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("API Key #{} 不存在", id) })),
        )
            .into_response(),
        Ok(Ok(_)) => {
            reload_keys_into_memory(&state).await;
            let action = if want_disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("API Key #{} 已{}", id, action))).into_response()
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            let code = if msg.contains("至少保留") {
                axum::http::StatusCode::BAD_REQUEST
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

// ============================================================================
// 账号池分组
// ============================================================================

/// GET /api/admin/groups —— 列出全部分组（含成员凭据 id）
pub async fn list_groups(State(state): State<AdminState>) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<GroupItem>> {
        let groups = crate::db::groups::list(&path)?;
        let map = crate::db::groups::credential_group_map(&path)?;
        Ok(groups
            .into_iter()
            .map(|g| {
                let credential_ids = map
                    .iter()
                    .filter(|(_, gid)| **gid == g.id)
                    .map(|(cid, _)| *cid as u64)
                    .collect::<Vec<_>>();
                GroupItem {
                    id: g.id,
                    name: g.name,
                    created_at: g.created_at,
                    credential_ids,
                }
            })
            .collect())
    })
    .await;

    match result {
        Ok(Ok(groups)) => Json(GroupsResponse { groups }).into_response(),
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// POST /api/admin/groups —— 新建分组
pub async fn add_group(
    State(state): State<AdminState>,
    Json(payload): Json<GroupNameRequest>,
) -> impl IntoResponse {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "分组名不能为空" })),
        )
            .into_response();
    }
    let path = state.keys_db_path.clone();
    let op = tokio::task::spawn_blocking(move || crate::db::groups::add(&path, &name)).await;
    match op {
        Ok(Ok(_id)) => Json(SuccessResponse::new("分组已创建")).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("UNIQUE") || msg.contains("constraint") {
                (
                    axum::http::StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": "同名分组已存在" })),
                )
                    .into_response()
            } else {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": msg })),
                )
                    .into_response()
            }
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// PUT /api/admin/groups/{id} —— 重命名分组
pub async fn rename_group(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
    Json(payload): Json<GroupNameRequest>,
) -> impl IntoResponse {
    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "分组名不能为空" })),
        )
            .into_response();
    }
    let path = state.keys_db_path.clone();
    let op = tokio::task::spawn_blocking(move || crate::db::groups::rename(&path, id, &name)).await;
    match op {
        Ok(Ok(0)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("分组 #{} 不存在", id) })),
        )
            .into_response(),
        Ok(Ok(_)) => Json(SuccessResponse::new("分组已重命名")).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let code = if msg.contains("UNIQUE") || msg.contains("constraint") {
                axum::http::StatusCode::CONFLICT
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/groups/{id} —— 删除分组（级联清空归属、解绑 apikey）
pub async fn delete_group(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let op = tokio::task::spawn_blocking(move || crate::db::groups::delete(&path, id)).await;
    let resp = match op {
        Ok(Ok(0)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("分组 #{} 不存在", id) })),
        )
            .into_response(),
        Ok(Ok(_)) => Json(SuccessResponse::new("分组已删除")).into_response(),
        Ok(Err(e)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    };
    // 删组可能解绑了 apikey，但内存 key 列表（仅含明文）不受 group 影响，无需 reload
    resp
}

/// PUT /api/admin/credentials/{id}/group —— 设置某凭据的分组归属
pub async fn set_credential_group(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
    Json(payload): Json<SetCredentialGroupRequest>,
) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let group_id = payload.group_id;
    let op = tokio::task::spawn_blocking(move || {
        crate::db::groups::set_credential_group(&path, id as i64, group_id)
    })
    .await;
    match op {
        Ok(Ok(())) => Json(SuccessResponse::new("凭据分组已更新")).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            // group_id 不存在会触发外键错误
            let code = if msg.contains("FOREIGN KEY") || msg.contains("constraint") {
                axum::http::StatusCode::BAD_REQUEST
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

/// PUT /api/admin/api-keys/{id}/group —— 设置某 apikey 的分组绑定
pub async fn set_api_key_group(
    State(state): State<AdminState>,
    Path(id): Path<i64>,
    Json(payload): Json<SetApiKeyGroupRequest>,
) -> impl IntoResponse {
    let path = state.keys_db_path.clone();
    let group_id = payload.group_id;
    let op =
        tokio::task::spawn_blocking(move || crate::db::groups::set_key_group(&path, id, group_id))
            .await;
    match op {
        Ok(Ok(0)) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("API Key #{} 不存在", id) })),
        )
            .into_response(),
        Ok(Ok(_)) => Json(SuccessResponse::new("API Key 分组绑定已更新")).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let code = if msg.contains("FOREIGN KEY") || msg.contains("constraint") {
                axum::http::StatusCode::BAD_REQUEST
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("task panic: {}", e) })),
        )
            .into_response(),
    }
}

// ============================================================================
// 请求日志查询
// ============================================================================
use axum::extract::Query;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRequestsQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

/// GET /api/admin/requests?limit=50&offset=0&status=error&accountId=xxx
pub async fn list_requests(
    State(state): State<AdminState>,
    Query(params): Query<ListRequestsQuery>,
) -> impl IntoResponse {
    use crate::db::query::{self, ListQuery};
    let Some(path) = state.log_db_path.clone() else {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "error": "请求日志未启用"
        }))).into_response();
    };

    let q = ListQuery {
        limit: params.limit.unwrap_or(50).min(500),
        offset: params.offset.unwrap_or(0),
        status: params.status,
        account_id: params.account_id,
    };

    // SQLite 阻塞操作，包到 spawn_blocking
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let conn = query::open_readonly(&path)?;
        let total = query::count(&conn, &q)?;
        let items = query::list(&conn, &q)?;
        Ok((total, items))
    })
    .await;

    match result {
        Ok(Ok((total, items))) => Json(serde_json::json!({
            "total": total,
            "items": items,
        })).into_response(),
        Ok(Err(e)) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": e.to_string()
        }))).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": format!("task panic: {}", e)
        }))).into_response(),
    }
}

/// GET /api/admin/requests/:id
pub async fn get_request_detail(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    use crate::db::query;
    let Some(path) = state.log_db_path.clone() else {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "error": "请求日志未启用"
        }))).into_response();
    };

    let id_clone = id.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let conn = query::open_readonly(&path)?;
        Ok(query::get(&conn, &id_clone)?)
    })
    .await;

    match result {
        Ok(Ok(Some(detail))) => Json(detail).into_response(),
        Ok(Ok(None)) => (axum::http::StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("请求 {} 不存在", id)
        }))).into_response(),
        Ok(Err(e)) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": e.to_string()
        }))).into_response(),
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({
            "error": format!("task panic: {}", e)
        }))).into_response(),
    }
}

// ============================================================================
// 凭据并发数管理
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetConcurrencyRequest {
    pub max_concurrency: u32,
}

/// POST /api/admin/credentials/:id/concurrency
pub async fn set_credential_concurrency(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetConcurrencyRequest>,
) -> impl IntoResponse {
    if payload.max_concurrency == 0 || payload.max_concurrency > 100 {
        return (axum::http::StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "max_concurrency 必须在 1..=100 之间"
        }))).into_response();
    }
    match state.service.set_max_concurrency(id, payload.max_concurrency) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 并发上限已设为 {}", id, payload.max_concurrency
        ))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}
