//! Admin API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use super::service::AdminService;
use super::types::AdminErrorResponse;
use crate::anthropic::SharedApiKeys;
use crate::common::auth;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥
    pub admin_api_key: String,
    /// Admin 服务
    pub service: Arc<AdminService>,
    /// 请求日志 SQLite 路径（None = 日志被禁用）
    pub log_db_path: Option<std::path::PathBuf>,
    /// 反代访问密钥集合句柄（运行时可改，与 Anthropic 路由共享同一个 RwLock）
    pub api_keys: SharedApiKeys,
    /// api_keys 表持久化用的 SQLite 路径（独立于日志开关）
    pub keys_db_path: std::path::PathBuf,
}

impl AdminState {
    pub fn new(
        admin_api_key: impl Into<String>,
        service: AdminService,
        api_keys: SharedApiKeys,
        keys_db_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            admin_api_key: admin_api_key.into(),
            service: Arc::new(service),
            log_db_path: None,
            api_keys,
            keys_db_path: keys_db_path.into(),
        }
    }

    pub fn with_log_db<P: Into<std::path::PathBuf>>(mut self, path: Option<P>) -> Self {
        self.log_db_path = path.map(|p| p.into());
        self
    }
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let api_key = auth::extract_api_key(&request);

    match api_key {
        Some(key) if auth::constant_time_eq(&key, &state.admin_api_key) => next.run(request).await,
        _ => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
