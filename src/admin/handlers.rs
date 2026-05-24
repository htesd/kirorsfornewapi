//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, SetDisabledRequest, SetLoadBalancingModeRequest,
        SetPriorityRequest, SetRateLimitCooldownRequest, SuccessResponse,
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
