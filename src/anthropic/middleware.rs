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

/// 认证后注入到请求扩展的"允许凭据集合"。
///
/// - `None`：该 apikey 未绑定分组 → 不限制（可用全部账号，历史行为）。
/// - `Some(set)`：绑定了分组 → **严格隔离**到该集合；空集合表示分组无成员（无可用账号）。
#[derive(Clone, Debug)]
pub struct AllowedCredentials(pub Option<std::collections::HashSet<u64>>);

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
    /// api_keys / groups 所在的 SQLite 路径（认证时按 apikey 解析分组隔离集合）
    pub keys_db_path: Option<std::path::PathBuf>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(api_keys: SharedApiKeys, extract_thinking: bool) -> Self {
        Self {
            api_keys,
            kiro_provider: None,
            extract_thinking,
            log_recorder: None,
            keys_db_path: None,
        }
    }

    /// 设置 api_keys / groups 数据库路径（认证时按 apikey 解析分组隔离集合）
    pub fn with_keys_db_path(mut self, path: Option<std::path::PathBuf>) -> Self {
        self.keys_db_path = path;
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
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // 提取并验证 apikey；记下匹配到的那个 key 用于后续分组解析
    let matched_key = match auth::extract_api_key(&request) {
        Some(key) => {
            let keys = state.api_keys.read();
            // 逐个常量时间比较并累积，不提前返回，避免按位置泄露时序信息
            let mut ok = false;
            for k in keys.iter() {
                ok |= auth::constant_time_eq(&key, k);
            }
            if ok { Some(key) } else { None }
        }
        None => None,
    };

    let key = match matched_key {
        Some(k) => k,
        None => {
            let error = ErrorResponse::authentication_error();
            return (StatusCode::UNAUTHORIZED, Json(error)).into_response();
        }
    };

    // 解析该 apikey 的分组隔离集合，注入请求扩展供 handler 使用。
    // 失败策略：fail-open —— DB 不可用时降级为"不限制"（None），放行而非阻断。
    // 分组定位是成本/缓存局部性优化（把某些 key 固定到某些账号），不是安全租户隔离，
    // 因此可用性优先：偶发 DB 抖动时宁可这次请求多花点成本，也不让它直接失败。
    // 若未来要把分组当作硬安全边界，需改为 fail-closed（DB 失败则拒绝请求）。
    let allowed = if let Some(path) = state.keys_db_path.clone() {
        match tokio::task::spawn_blocking(move || {
            crate::db::groups::allowed_credential_ids_for_key(&path, &key)
        })
        .await
        {
            Ok(Ok(opt)) => opt,
            Ok(Err(e)) => {
                tracing::warn!("解析 apikey 分组失败（降级为不限制）: {}", e);
                None
            }
            Err(e) => {
                tracing::warn!("分组解析任务 join 失败（降级为不限制）: {}", e);
                None
            }
        }
    } else {
        None
    };
    request.extensions_mut().insert(AllowedCredentials(allowed));

    next.run(request).await
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
