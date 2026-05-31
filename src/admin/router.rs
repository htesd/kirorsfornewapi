//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_api_key, add_credential, add_group, delete_api_key, delete_credential, delete_group,
        force_refresh_token, get_all_credentials, get_credential_balance, get_load_balancing_mode,
        get_rate_limit_cooldown, get_request_detail, get_scheduling, list_api_keys, list_groups,
        list_requests, rename_group, reset_failure_count, set_api_key_disabled, set_api_key_group,
        set_credential_concurrency, set_credential_disabled, set_credential_group,
        set_credential_priority, set_load_balancing_mode, set_rate_limit_cooldown,
        update_scheduling,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/concurrency", post(set_credential_concurrency))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/credentials/{id}/group", put(set_credential_group))
        .route("/requests", get(list_requests))
        .route("/requests/{id}", get(get_request_detail))
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route(
            "/config/rate-limit-cooldown",
            get(get_rate_limit_cooldown).put(set_rate_limit_cooldown),
        )
        .route(
            "/config/scheduling",
            get(get_scheduling).put(update_scheduling),
        )
        .route("/groups", get(list_groups).post(add_group))
        .route("/groups/{id}", put(rename_group).delete(delete_group))
        .route("/api-keys", get(list_api_keys).post(add_api_key))
        .route("/api-keys/{id}", delete(delete_api_key))
        .route("/api-keys/{id}/disabled", post(set_api_key_disabled))
        .route("/api-keys/{id}/group", put(set_api_key_group))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
