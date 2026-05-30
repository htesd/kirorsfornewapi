//! Anthropic API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use parking_lot::RwLock;

use crate::common::auth;
use crate::db::LogRecorder;
use crate::kiro::provider::KiroProvider;

use super::types::ErrorResponse;

/// 运行时可变的反代访问密钥集合句柄
///
/// 客户端调 `/v1/messages` 用的 API Key，可配置多个，任意一个都能通过认证。
/// Admin 增删 key 后需要立即生效，因此用 `Arc<RwLock<Vec<String>>>` 共享：
/// 认证路径读锁（高频），增删 key 写锁（极少）。
pub type SharedApiKeys = Arc<RwLock<Vec<String>>>;

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// 反代访问密钥集合（运行时可变）
    pub api_keys: SharedApiKeys,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// 是否开启非流式响应的 thinking 块提取
    pub extract_thinking: bool,
    /// 请求日志记录器（None = 日志被禁用）
    pub log_recorder: Option<LogRecorder>,
    /// 用户感知的缓存命中比例（None = 不放大，按实际值上报）
    pub perceived_cache_hit_ratio: Option<f64>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(api_keys: SharedApiKeys, extract_thinking: bool) -> Self {
        Self {
            api_keys,
            kiro_provider: None,
            extract_thinking,
            log_recorder: None,
            perceived_cache_hit_ratio: None,
        }
    }

    /// 设置感知缓存命中放大比例
    pub fn with_perceived_cache_hit_ratio(mut self, r: Option<f64>) -> Self {
        self.perceived_cache_hit_ratio = r;
        self
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: KiroProvider) -> Self {
        self.kiro_provider = Some(Arc::new(provider));
        self
    }

    /// 设置日志记录器
    pub fn with_log_recorder(mut self, recorder: Option<LogRecorder>) -> Self {
        self.log_recorder = recorder;
        self
    }
}

/// API Key 认证中间件
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let authorized = match auth::extract_api_key(&request) {
        Some(key) => {
            let keys = state.api_keys.read();
            // 逐个常量时间比较并累积，不提前返回，避免按位置泄露时序信息
            let mut ok = false;
            for k in keys.iter() {
                ok |= auth::constant_time_eq(&key, k);
            }
            ok
        }
        None => false,
    };

    if authorized {
        next.run(request).await
    } else {
        let error = ErrorResponse::authentication_error();
        (StatusCode::UNAUTHORIZED, Json(error)).into_response()
    }
}

/// CORS 中间件层
///
/// **安全说明**：当前配置允许所有来源（Any），这是为了支持公开 API 服务。
/// 如果需要更严格的安全控制，请根据实际需求配置具体的允许来源、方法和头信息。
///
/// # 配置说明
/// - `allow_origin(Any)`: 允许任何来源的请求
/// - `allow_methods(Any)`: 允许任何 HTTP 方法
/// - `allow_headers(Any)`: 允许任何请求头
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
