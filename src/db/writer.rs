//! 后台写库 task
//!
//! 用 `spawn_blocking` 跑专用线程，独占 rusqlite::Connection，
//! 通过 std::sync::mpsc 从 handler 接收 `RequestRecord` 落库。
//!
//! - 发送端不阻塞：`std::sync::mpsc::channel()` 是 unbounded
//! - 写入端独占连接：避开 SQLite 多线程问题
//! - 单条事务：requests + errors + request_bodies 一并写入

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Result as SqlResult, params};

use super::config::RequestLogConfig;
use super::recorder::{LogRecorder, RequestRecord, RequestStatus};
use super::schema;

/// 每写入 N 条记录触发一次 ring-buffer 修剪
const TRIM_EVERY: usize = 50;

/// 启动 writer task。
///
/// 返回 `LogRecorder` 句柄（None 表示日志被配置禁用）。
///
/// 内部用 `tokio::task::spawn_blocking` 跑同步循环，
/// 直到所有 sender 释放后自然退出。
pub fn start_writer(cfg: &RequestLogConfig) -> Option<LogRecorder> {
    if !cfg.enabled {
        tracing::info!("请求日志已禁用");
        return None;
    }

    let (tx, rx) = channel::<RequestRecord>();
    let cfg = cfg.clone();
    let db_path = PathBuf::from(&cfg.db_path);

    tokio::task::spawn_blocking(move || {
        if let Err(e) = run_loop(db_path, cfg, rx) {
            tracing::error!("请求日志 writer task 异常退出: {}", e);
        }
    });

    Some(LogRecorder::new(tx))
}

/// writer 主循环。建库 → 阻塞 recv → 落库 → 周期 trim。
fn run_loop(
    db_path: PathBuf,
    cfg: RequestLogConfig,
    rx: Receiver<RequestRecord>,
) -> anyhow::Result<()> {
    // 确保父目录存在
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).ok();
        }
    }

    let mut conn = Connection::open(&db_path)?;
    schema::init(&conn)?;
    tracing::info!(
        path = %db_path.display(),
        mode = ?cfg.mode,
        max_records = cfg.max_records,
        "请求日志 writer 启动"
    );

    let mut written_since_trim: usize = 0;

    loop {
        // 阻塞等待下一条记录；所有 sender 释放后 recv 返回 Err 触发退出
        let record = match rx.recv() {
            Ok(r) => r,
            Err(_) => {
                tracing::info!("请求日志 writer：所有 sender 已释放，正常退出");
                break;
            }
        };

        let success = record.status == RequestStatus::Success;
        if !cfg.should_record(success) {
            continue;
        }

        let should_save_body = cfg.save_request_body
            || (cfg.save_response_body_on_error && !success);

        match insert_record(&mut conn, &record, should_save_body, &cfg) {
            Ok(()) => {
                written_since_trim += 1;
                if written_since_trim >= TRIM_EVERY {
                    match schema::trim_to(&conn, cfg.max_records) {
                        Ok(n) if n > 0 => {
                            tracing::debug!(deleted = n, "请求日志触发 ring-buffer 修剪");
                        }
                        Err(e) => tracing::warn!("请求日志修剪失败: {}", e),
                        _ => {}
                    }
                    written_since_trim = 0;
                }
            }
            Err(e) => {
                tracing::warn!(
                    request_id = %record.request_id,
                    error = %e,
                    "请求日志写入失败"
                );
                // 失败但不退出——下一条还可以试
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    Ok(())
}

/// 单条记录入库（事务包裹 3 张表）
fn insert_record(
    conn: &mut Connection,
    rec: &RequestRecord,
    save_body: bool,
    cfg: &RequestLogConfig,
) -> SqlResult<()> {
    let tx = conn.transaction()?;

    tx.execute(
        r#"
        INSERT OR REPLACE INTO requests (
            request_id, ts_ms,
            public_key_id, client_id, session_id,
            model, upstream_model,
            endpoint, pool_id, account_id, account_label,
            status, http_status, reason, error_kind, error_message,
            attempts, is_stream,
            latency_ms, ttfb_ms,
            prompt_tokens, completion_tokens, cached_tokens, cache_creation_tokens, cost,
            metering_unit, metering_usage, context_usage_pct,
            has_cache_control, messages_count, tools_count, system_prompt_len,
            params_json,
            cache_read_reported
        ) VALUES (
            ?1, ?2,
            ?3, ?4, ?5,
            ?6, ?7,
            ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16,
            ?17, ?18,
            ?19, ?20,
            ?21, ?22, ?23, ?24, ?25,
            ?26, ?27, ?28,
            ?29, ?30, ?31, ?32,
            ?33,
            ?34
        )
        "#,
        params![
            rec.request_id,
            rec.ts_ms,
            rec.public_key_id,
            rec.client_id,
            rec.session_id,
            rec.model,
            rec.upstream_model,
            rec.endpoint,
            rec.pool_id,
            rec.account_id,
            rec.account_label,
            rec.status.as_str(),
            rec.http_status.map(|s| s as i64),
            rec.reason,
            rec.error_kind,
            rec.error_message,
            rec.attempts,
            rec.is_stream as i32,
            rec.latency_ms,
            rec.ttfb_ms,
            rec.prompt_tokens,
            rec.completion_tokens,
            rec.cached_tokens,
            rec.cache_creation_tokens,
            rec.cost,
            rec.metering_unit,
            rec.metering_usage,
            rec.context_usage_pct,
            rec.has_cache_control as i32,
            rec.messages_count,
            rec.tools_count,
            rec.system_prompt_len,
            rec.params_json,
            rec.cache_read_reported,
        ],
    )?;

    if let Some(err) = &rec.error_detail {
        tx.execute(
            "INSERT OR REPLACE INTO errors (request_id, stage, code, message)
             VALUES (?1, ?2, ?3, ?4)",
            params![rec.request_id, err.stage, err.code, err.message],
        )?;
    }

    if save_body {
        // request_body 按 save_request_body 决定；response_body 仅在失败时
        let request_body = if cfg.save_request_body {
            rec.request_body.as_deref()
        } else {
            None
        };
        let response_body = if cfg.save_response_body_on_error
            && rec.status != RequestStatus::Success
        {
            rec.response_body.as_deref()
        } else {
            None
        };
        if request_body.is_some() || response_body.is_some() {
            tx.execute(
                "INSERT OR REPLACE INTO request_bodies (request_id, request_body, response_body)
                 VALUES (?1, ?2, ?3)",
                params![rec.request_id, request_body, response_body],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::config::LogMode;
    use crate::db::recorder::RequestRecordBuilder;
    use tempfile::tempdir;

    /// 启动一个真实的 writer，发送几条记录，等 task 退出后查 DB
    async fn run_with_records(cfg: RequestLogConfig, records: Vec<RequestRecord>) -> PathBuf {
        let path = PathBuf::from(&cfg.db_path);
        let recorder = start_writer(&cfg).expect("writer 应启动");
        for r in records {
            recorder.record(r);
        }
        drop(recorder); // 触发 sender 释放，writer 退出
        // 等 task 完全退出（spawn_blocking 没有 await 句柄，简单 sleep 几十 ms）
        tokio::time::sleep(Duration::from_millis(200)).await;
        path
    }

    fn mk_record(id_suffix: &str, status: RequestStatus) -> RequestRecord {
        let mut b = RequestRecordBuilder::begin(
            "/v1/messages",
            "claude-sonnet-4-6",
            false,
            1,
            0,
            false,
            0,
        );
        b.set_request_body(format!("{{\"id\":\"{}\"}}", id_suffix));
        if status != RequestStatus::Success {
            b.set_error("test", "test", "TEST", "test error");
            b.set_response_body(format!("{{\"err\":\"{}\"}}", id_suffix));
        }
        b.build(status)
    }

    #[tokio::test]
    async fn writes_success_record_in_all_mode() {
        let dir = tempdir().unwrap();
        let cfg = RequestLogConfig {
            db_path: dir.path().join("test.db").to_str().unwrap().to_string(),
            mode: LogMode::All,
            ..Default::default()
        };
        let path = run_with_records(cfg, vec![mk_record("ok", RequestStatus::Success)]).await;
        let conn = Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn skips_success_record_in_errors_only_mode() {
        let dir = tempdir().unwrap();
        let cfg = RequestLogConfig {
            db_path: dir.path().join("test.db").to_str().unwrap().to_string(),
            mode: LogMode::ErrorsOnly,
            ..Default::default()
        };
        let path = run_with_records(
            cfg,
            vec![
                mk_record("ok", RequestStatus::Success),
                mk_record("bad", RequestStatus::Error),
            ],
        )
        .await;
        let conn = Connection::open(&path).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "ErrorsOnly 模式只应记录失败");
        let status: String = conn
            .query_row("SELECT status FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "error");
    }

    #[tokio::test]
    async fn saves_error_body_when_configured() {
        let dir = tempdir().unwrap();
        let cfg = RequestLogConfig {
            db_path: dir.path().join("test.db").to_str().unwrap().to_string(),
            mode: LogMode::All,
            save_request_body: true,
            save_response_body_on_error: true,
            ..Default::default()
        };
        let path = run_with_records(cfg, vec![mk_record("bad", RequestStatus::Error)]).await;
        let conn = Connection::open(&path).unwrap();
        let row: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT request_body, response_body FROM request_bodies",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap();
        let (req, resp) = row.expect("应有 body");
        assert!(req.unwrap().contains("bad"));
        assert!(resp.unwrap().contains("bad"));
    }

    #[tokio::test]
    async fn writer_disabled_returns_none() {
        let cfg = RequestLogConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(start_writer(&cfg).is_none());
    }
}
