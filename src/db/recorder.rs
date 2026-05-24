//! 请求记录的构造与发送
//!
//! - `RequestRecord`：一次请求的完整快照（不可变，发往 writer task 后入库）
//! - `RequestRecordBuilder`：handler 路径里逐步填字段，结尾 `.build()`
//! - `LogRecorder`：发送端句柄，Cloneable，通过 mpsc 发往 writer
//!
//! ## 命名约定
//!
//! 与 ALLinOne `RequestRecord` 强对齐：`prompt_tokens` / `completion_tokens` /
//! `cached_tokens` / `latency_ms` / `account_id` / `pool_id` / `status`(枚举字符串)。
//! 详见 [`super::schema`] 的映射表。

use std::sync::mpsc::{SendError, Sender};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// 请求状态（status 列的取值）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    Success,
    Error,
    Cancelled,
}

impl RequestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// 一次请求落库前的完整快照
#[derive(Debug, Clone)]
pub struct RequestRecord {
    // 主键 + 时间
    pub request_id: String,
    pub ts_ms: i64,

    // 客户端身份
    pub public_key_id: Option<String>,
    pub client_id: Option<String>,
    pub session_id: Option<String>,

    // 模型
    pub model: String,
    pub upstream_model: Option<String>,

    // 路由
    pub endpoint: String,
    pub pool_id: String,
    pub account_id: Option<String>,
    pub account_label: Option<String>,

    // 状态
    pub status: RequestStatus,
    pub http_status: Option<u16>,
    pub reason: Option<String>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub attempts: i32,
    pub is_stream: bool,

    // 时延
    pub latency_ms: i64,
    pub ttfb_ms: Option<i64>,

    // Token
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    pub cached_tokens: Option<i32>,
    pub cache_creation_tokens: Option<i32>,
    pub cost: Option<f64>,

    // Kiro 特有
    pub metering_unit: Option<String>,
    pub metering_usage: Option<f64>,
    pub context_usage_pct: Option<f64>,
    pub has_cache_control: bool,
    pub messages_count: i32,
    pub tools_count: i32,
    pub system_prompt_len: i32,

    // 透传参数
    pub params_json: Option<String>,

    // 辅表
    pub error_detail: Option<ErrorDetail>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ErrorDetail {
    pub stage: String,
    pub code: String,
    pub message: String,
}

/// 在 handler 中边处理边填字段的 builder
pub struct RequestRecordBuilder {
    request_id: String,
    ts_ms: i64,
    started_at: SystemTime,
    // 必填
    endpoint: String,
    model: String,
    is_stream: bool,
    messages_count: i32,
    tools_count: i32,
    has_cache_control: bool,
    system_prompt_len: i32,
    // 可选
    public_key_id: Option<String>,
    client_id: Option<String>,
    session_id: Option<String>,
    upstream_model: Option<String>,
    account_id: Option<String>,
    account_label: Option<String>,
    http_status: Option<u16>,
    reason: Option<String>,
    error_kind: Option<String>,
    error_message: Option<String>,
    attempts: i32,
    ttfb_ms: Option<i64>,
    prompt_tokens: Option<i32>,
    completion_tokens: Option<i32>,
    cached_tokens: Option<i32>,
    cache_creation_tokens: Option<i32>,
    cost: Option<f64>,
    metering_unit: Option<String>,
    metering_usage: Option<f64>,
    context_usage_pct: Option<f64>,
    params_json: Option<String>,
    error_detail: Option<ErrorDetail>,
    request_body: Option<String>,
    response_body: Option<String>,
}

impl RequestRecordBuilder {
    /// 创建一个新 builder。时间戳在此刻开始计。
    pub fn begin(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        is_stream: bool,
        messages_count: i32,
        tools_count: i32,
        has_cache_control: bool,
        system_prompt_len: i32,
    ) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            ts_ms: now_ms(),
            started_at: SystemTime::now(),
            endpoint: endpoint.into(),
            model: model.into(),
            is_stream,
            messages_count,
            tools_count,
            has_cache_control,
            system_prompt_len,
            public_key_id: None,
            client_id: None,
            session_id: None,
            upstream_model: None,
            account_id: None,
            account_label: None,
            http_status: None,
            reason: None,
            error_kind: None,
            error_message: None,
            attempts: 1,
            ttfb_ms: None,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            cache_creation_tokens: None,
            cost: None,
            metering_unit: None,
            metering_usage: None,
            context_usage_pct: None,
            params_json: None,
            error_detail: None,
            request_body: None,
            response_body: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.request_id
    }

    // 只读访问器：供上层（logging glue）在 build 前做派生计算，
    // 例如缓存命中估计。db 层本身不含任何计费/领域逻辑。
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn prompt_tokens(&self) -> Option<i32> {
        self.prompt_tokens
    }
    pub fn completion_tokens(&self) -> Option<i32> {
        self.completion_tokens
    }
    pub fn metering_usage(&self) -> Option<f64> {
        self.metering_usage
    }
    pub fn cached_tokens(&self) -> Option<i32> {
        self.cached_tokens
    }

    pub fn set_client_identity(
        &mut self,
        public_key_id: Option<String>,
        client_id: Option<String>,
        session_id: Option<String>,
    ) {
        self.public_key_id = public_key_id;
        self.client_id = client_id;
        self.session_id = session_id;
    }

    pub fn set_upstream_model(&mut self, model: impl Into<String>) {
        self.upstream_model = Some(model.into());
    }

    pub fn set_account(
        &mut self,
        account_id: impl Into<String>,
        account_label: Option<String>,
    ) {
        self.account_id = Some(account_id.into());
        self.account_label = account_label;
    }

    pub fn set_http_status(&mut self, status: u16) {
        self.http_status = Some(status);
    }

    pub fn set_reason(&mut self, reason: impl Into<String>) {
        self.reason = Some(reason.into());
    }

    pub fn set_attempts(&mut self, n: i32) {
        self.attempts = n;
    }

    /// 标记首字节到达（流式专用）；多次调用只生效第一次
    pub fn mark_ttfb(&mut self) {
        if self.ttfb_ms.is_none() {
            self.ttfb_ms = Some(elapsed_ms(self.started_at));
        }
    }

    pub fn set_prompt_tokens(&mut self, n: i32) {
        self.prompt_tokens = Some(n);
    }
    pub fn set_completion_tokens(&mut self, n: i32) {
        self.completion_tokens = Some(n);
    }
    pub fn set_cached_tokens(&mut self, n: i32) {
        self.cached_tokens = Some(n);
    }
    pub fn set_cache_creation_tokens(&mut self, n: i32) {
        self.cache_creation_tokens = Some(n);
    }
    pub fn set_cost(&mut self, c: f64) {
        self.cost = Some(c);
    }

    pub fn set_metering(&mut self, unit: Option<String>, usage: f64) {
        self.metering_unit = unit;
        self.metering_usage = Some(usage);
    }
    pub fn set_context_usage_pct(&mut self, pct: f64) {
        self.context_usage_pct = Some(pct);
    }

    pub fn set_error(
        &mut self,
        kind: impl Into<String>,
        stage: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        let msg = message.into();
        self.error_kind = Some(kind.into());
        self.error_message = Some(msg.clone());
        self.error_detail = Some(ErrorDetail {
            stage: stage.into(),
            code: code.into(),
            message: msg,
        });
    }

    pub fn set_params_json(&mut self, json: String) {
        self.params_json = Some(json);
    }

    pub fn set_request_body(&mut self, body: String) {
        self.request_body = Some(body);
    }

    pub fn set_response_body(&mut self, body: String) {
        self.response_body = Some(body);
    }

    /// 构造最终 record。`status` 由调用方显式给。
    pub fn build(self, status: RequestStatus) -> RequestRecord {
        let latency_ms = elapsed_ms(self.started_at);
        RequestRecord {
            request_id: self.request_id,
            ts_ms: self.ts_ms,
            public_key_id: self.public_key_id,
            client_id: self.client_id,
            session_id: self.session_id,
            model: self.model,
            upstream_model: self.upstream_model,
            endpoint: self.endpoint,
            pool_id: "kiro".to_string(),
            account_id: self.account_id,
            account_label: self.account_label,
            status,
            http_status: self.http_status,
            reason: self.reason,
            error_kind: self.error_kind,
            error_message: self.error_message,
            attempts: self.attempts,
            is_stream: self.is_stream,
            latency_ms,
            ttfb_ms: self.ttfb_ms,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            cached_tokens: self.cached_tokens,
            cache_creation_tokens: self.cache_creation_tokens,
            cost: self.cost,
            metering_unit: self.metering_unit,
            metering_usage: self.metering_usage,
            context_usage_pct: self.context_usage_pct,
            has_cache_control: self.has_cache_control,
            messages_count: self.messages_count,
            tools_count: self.tools_count,
            system_prompt_len: self.system_prompt_len,
            params_json: self.params_json,
            error_detail: self.error_detail,
            request_body: self.request_body,
            response_body: self.response_body,
        }
    }
}

/// 发送端句柄
///
/// Clone 廉价（内部 Sender），可以安全地在 handler / stream 上下文里复制。
#[derive(Clone)]
pub struct LogRecorder {
    tx: Sender<RequestRecord>,
}

impl LogRecorder {
    pub fn new(tx: Sender<RequestRecord>) -> Self {
        Self { tx }
    }

    /// 非阻塞发送，失败仅记日志（不影响主流程）
    pub fn record(&self, record: RequestRecord) {
        if let Err(SendError(rec)) = self.tx.send(record) {
            tracing::warn!(
                request_id = %rec.request_id,
                "请求日志发送失败：writer task 已退出"
            );
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn elapsed_ms(start: SystemTime) -> i64 {
    start.elapsed().map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_default_minimal_fields() {
        let b = RequestRecordBuilder::begin("/v1/messages", "claude-sonnet-4-6", false, 1, 0, false, 0);
        let rec = b.build(RequestStatus::Success);
        assert_eq!(rec.endpoint, "/v1/messages");
        assert_eq!(rec.model, "claude-sonnet-4-6");
        assert_eq!(rec.status, RequestStatus::Success);
        assert_eq!(rec.pool_id, "kiro");
        assert_eq!(rec.attempts, 1);
        assert!(rec.metering_usage.is_none());
        assert!(Uuid::parse_str(&rec.request_id).is_ok());
        assert!(rec.latency_ms >= 0);
    }

    #[test]
    fn builder_full_path() {
        let mut b = RequestRecordBuilder::begin("/cc/v1/messages", "claude-opus-4-7", true, 5, 2, true, 42);
        b.set_client_identity(Some("pk-abc".into()), Some("client-x".into()), Some("sess-1".into()));
        b.set_account("a@b.com", Some("KIRO PRO+".into()));
        b.set_upstream_model("claude-opus-4.7");
        b.set_prompt_tokens(1100);
        b.set_completion_tokens(800);
        b.set_cached_tokens(900);
        b.set_cache_creation_tokens(50);
        b.set_context_usage_pct(13.4);
        b.set_metering(Some("credit".into()), 0.025);
        b.set_http_status(200);
        b.set_reason("end_turn");
        b.mark_ttfb();
        let rec = b.build(RequestStatus::Success);
        assert_eq!(rec.account_id.as_deref(), Some("a@b.com"));
        assert_eq!(rec.account_label.as_deref(), Some("KIRO PRO+"));
        assert_eq!(rec.upstream_model.as_deref(), Some("claude-opus-4.7"));
        assert_eq!(rec.prompt_tokens, Some(1100));
        assert_eq!(rec.cached_tokens, Some(900));
        assert_eq!(rec.metering_usage, Some(0.025));
        assert_eq!(rec.http_status, Some(200));
        assert_eq!(rec.reason.as_deref(), Some("end_turn"));
        assert!(rec.ttfb_ms.is_some());
    }

    #[test]
    fn builder_error_path() {
        let mut b = RequestRecordBuilder::begin("/v1/messages", "claude-sonnet-4-6", false, 1, 0, false, 0);
        b.set_http_status(402);
        b.set_error("quota_exhausted", "upstream", "402", "monthly limit reached");
        b.set_response_body("{\"reason\":\"MONTHLY_REQUEST_COUNT\"}".into());
        let rec = b.build(RequestStatus::Error);
        assert_eq!(rec.status, RequestStatus::Error);
        assert_eq!(rec.error_kind.as_deref(), Some("quota_exhausted"));
        assert_eq!(rec.error_message.as_deref(), Some("monthly limit reached"));
        let detail = rec.error_detail.unwrap();
        assert_eq!(detail.code, "402");
    }

    #[test]
    fn ttfb_only_records_first_call() {
        let mut b = RequestRecordBuilder::begin("/v1/messages", "m", true, 1, 0, false, 0);
        b.mark_ttfb();
        let first = b.ttfb_ms;
        std::thread::sleep(std::time::Duration::from_millis(2));
        b.mark_ttfb();
        assert_eq!(b.ttfb_ms, first, "重复调用 mark_ttfb 不应改写");
    }

    #[test]
    fn recorder_send_after_writer_drop_does_not_panic() {
        let (tx, rx) = std::sync::mpsc::channel();
        let recorder = LogRecorder::new(tx);
        drop(rx);
        let rec = RequestRecordBuilder::begin("/v1/messages", "m", false, 0, 0, false, 0)
            .build(RequestStatus::Success);
        recorder.record(rec);
    }

    #[test]
    fn status_as_str() {
        assert_eq!(RequestStatus::Success.as_str(), "success");
        assert_eq!(RequestStatus::Error.as_str(), "error");
        assert_eq!(RequestStatus::Cancelled.as_str(), "cancelled");
    }
}
