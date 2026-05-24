//! 请求日志查询 API
//!
//! 给 Admin 层提供只读查询能力。读连接独立打开，避开 writer 的写连接。

use rusqlite::{Connection, OptionalExtension, Result as SqlResult};
use serde::Serialize;
use std::path::Path;

/// 请求日志列表项（精简版，列表展示用）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogSummary {
    pub request_id: String,
    pub ts_ms: i64,
    pub model: String,
    pub upstream_model: Option<String>,
    pub endpoint: String,
    pub account_id: Option<String>,
    pub account_label: Option<String>,
    pub status: String,
    pub http_status: Option<i64>,
    pub error_kind: Option<String>,
    pub reason: Option<String>,
    pub attempts: i32,
    pub is_stream: bool,
    pub latency_ms: i64,
    pub ttfb_ms: Option<i64>,
    pub prompt_tokens: Option<i32>,
    pub completion_tokens: Option<i32>,
    /// 估计的缓存读 token（NULL=无法判断，0=未命中，>0=命中）
    pub cached_tokens: Option<i32>,
    pub metering_unit: Option<String>,
    pub metering_usage: Option<f64>,
    pub context_usage_pct: Option<f64>,
}

/// 请求详情（含原文）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    #[serde(flatten)]
    pub summary: RequestLogSummary,
    pub messages_count: Option<i32>,
    pub tools_count: Option<i32>,
    pub system_prompt_len: Option<i32>,
    pub has_cache_control: Option<bool>,
    pub error_message: Option<String>,
    pub error_stage: Option<String>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

/// 列表查询参数
#[derive(Debug, Clone, Default)]
pub struct ListQuery {
    pub limit: usize,
    pub offset: usize,
    pub status: Option<String>,
    pub account_id: Option<String>,
}

pub fn open_readonly<P: AsRef<Path>>(path: P) -> SqlResult<Connection> {
    use rusqlite::OpenFlags;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.pragma_update(None, "journal_mode", &"WAL").ok();
    Ok(conn)
}

pub fn list(conn: &Connection, q: &ListQuery) -> SqlResult<Vec<RequestLogSummary>> {
    let mut sql = String::from(
        "SELECT request_id, ts_ms, model, upstream_model, endpoint,
                account_id, account_label, status, http_status, error_kind, reason,
                attempts, is_stream, latency_ms, ttfb_ms,
                prompt_tokens, completion_tokens,
                metering_unit, metering_usage, context_usage_pct,
                cached_tokens
         FROM requests",
    );
    let mut wheres = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = &q.status {
        wheres.push("status = ?".to_string());
        params.push(Box::new(s.clone()));
    }
    if let Some(a) = &q.account_id {
        wheres.push("account_id = ?".to_string());
        params.push(Box::new(a.clone()));
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    sql.push_str(" ORDER BY ts_ms DESC LIMIT ? OFFSET ?");
    params.push(Box::new(q.limit as i64));
    params.push(Box::new(q.offset as i64));

    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_refs.as_slice(), |row| {
        Ok(RequestLogSummary {
            request_id: row.get(0)?,
            ts_ms: row.get(1)?,
            model: row.get(2)?,
            upstream_model: row.get(3)?,
            endpoint: row.get(4)?,
            account_id: row.get(5)?,
            account_label: row.get(6)?,
            status: row.get(7)?,
            http_status: row.get(8)?,
            error_kind: row.get(9)?,
            reason: row.get(10)?,
            attempts: row.get(11)?,
            is_stream: row.get::<_, i64>(12)? != 0,
            latency_ms: row.get(13)?,
            ttfb_ms: row.get(14)?,
            prompt_tokens: row.get(15)?,
            completion_tokens: row.get(16)?,
            metering_unit: row.get(17)?,
            metering_usage: row.get(18)?,
            context_usage_pct: row.get(19)?,
            cached_tokens: row.get(20)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

pub fn count(conn: &Connection, q: &ListQuery) -> SqlResult<i64> {
    let mut sql = String::from("SELECT COUNT(*) FROM requests");
    let mut wheres = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(s) = &q.status {
        wheres.push("status = ?".to_string());
        params.push(Box::new(s.clone()));
    }
    if let Some(a) = &q.account_id {
        wheres.push("account_id = ?".to_string());
        params.push(Box::new(a.clone()));
    }
    if !wheres.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&wheres.join(" AND "));
    }
    let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
    conn.query_row(&sql, params_refs.as_slice(), |r| r.get(0))
}

pub fn get(conn: &Connection, request_id: &str) -> SqlResult<Option<RequestLogDetail>> {
    let detail = conn.query_row(
        "SELECT request_id, ts_ms, model, upstream_model, endpoint,
                account_id, account_label, status, http_status, error_kind, reason,
                attempts, is_stream, latency_ms, ttfb_ms,
                prompt_tokens, completion_tokens,
                metering_unit, metering_usage, context_usage_pct,
                messages_count, tools_count, system_prompt_len, has_cache_control,
                error_message, cached_tokens
         FROM requests WHERE request_id = ?",
        [request_id],
        |row| {
            Ok(RequestLogDetail {
                summary: RequestLogSummary {
                    request_id: row.get(0)?,
                    ts_ms: row.get(1)?,
                    model: row.get(2)?,
                    upstream_model: row.get(3)?,
                    endpoint: row.get(4)?,
                    account_id: row.get(5)?,
                    account_label: row.get(6)?,
                    status: row.get(7)?,
                    http_status: row.get(8)?,
                    error_kind: row.get(9)?,
                    reason: row.get(10)?,
                    attempts: row.get(11)?,
                    is_stream: row.get::<_, i64>(12)? != 0,
                    latency_ms: row.get(13)?,
                    ttfb_ms: row.get(14)?,
                    prompt_tokens: row.get(15)?,
                    completion_tokens: row.get(16)?,
                    metering_unit: row.get(17)?,
                    metering_usage: row.get(18)?,
                    context_usage_pct: row.get(19)?,
                    cached_tokens: row.get(25)?,
                },
                messages_count: row.get(20)?,
                tools_count: row.get(21)?,
                system_prompt_len: row.get(22)?,
                has_cache_control: row
                    .get::<_, Option<i64>>(23)?
                    .map(|v| v != 0),
                error_message: row.get(24)?,
                error_stage: None,
                request_body: None,
                response_body: None,
            })
        },
    )
    .optional()?;

    let Some(mut detail) = detail else {
        return Ok(None);
    };

    // 错误详细
    if let Some(stage) = conn
        .query_row(
            "SELECT stage FROM errors WHERE request_id = ?",
            [request_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
    {
        detail.error_stage = Some(stage);
    }

    // 请求/响应原文
    if let Some((req, resp)) = conn
        .query_row(
            "SELECT request_body, response_body FROM request_bodies WHERE request_id = ?",
            [request_id],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .optional()?
    {
        detail.request_body = req;
        detail.response_body = resp;
    }

    Ok(Some(detail))
}
