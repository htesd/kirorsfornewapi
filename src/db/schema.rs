//! SQLite 表结构定义
//!
//! ## 设计目标
//!
//! 列名与 ALLinOne `allinone/admin/usage.py` 的 `RequestRecord` **强对齐**，
//! 方便以后把 kiro 逻辑用 Python 重写进 ALLinOne 时直接 `INSERT...SELECT`
//! 把历史数据迁过去。
//!
//! ## 与 ALLinOne 字段对照
//!
//! | ALLinOne (Python)           | kiro.rs (本表)        | 备注 |
//! |-----------------------------|----------------------|------|
//! | request_id (TEXT PK)        | request_id           | UUID v4 |
//! | ts (REAL)                   | ts_ms (INTEGER)      | kiro 用 ms，移植时 ts_ms / 1000.0 |
//! | public_key_id               | public_key_id        | 客户端用的 API key 摘要 |
//! | client_id                   | client_id            | X-AllInOne-Client-ID（可空） |
//! | session_id                  | session_id           | X-AllInOne-Session-ID（可空） |
//! | model                       | model                | 客户端请求的模型 |
//! | upstream_model              | upstream_model       | 实际打到 Kiro 的模型 |
//! | pool_id                     | pool_id              | 固定 "kiro" |
//! | account_id                  | account_id           | 凭据邮箱或 "kiro-{credential_id}" |
//! | status                      | status               | success / error / cancelled |
//! | reason                      | reason               | end_turn / tool_use / ... |
//! | latency_ms                  | latency_ms           | 总耗时（ms） |
//! | ttfb_ms                     | ttfb_ms              | 流式首字节耗时 |
//! | prompt_tokens               | prompt_tokens        | |
//! | completion_tokens           | completion_tokens    | |
//! | cached_tokens               | cached_tokens        | 暂全 0，后续估算 |
//! | cache_creation_tokens       | cache_creation_tokens| 暂全 0，后续估算 |
//! | cost                        | cost                 | 暂空，等定义 credit→USD 汇率 |
//! | error                       | error_message        | 错误消息（详情见 errors 表） |
//! | params                      | params_json          | 脱敏后的客户端参数 JSON |
//!
//! ## Kiro 独有字段（同表内 inline，避免 JOIN）
//!
//! - `endpoint`            (TEXT)    `/v1/messages` 还是 `/cc/v1/messages`
//! - `is_stream`           (INTEGER) 0/1
//! - `attempts`            (INTEGER) 调度层 retry 次数
//! - `http_status`         (INTEGER) 上游返回的真实 HTTP 状态码
//! - `error_kind`          (TEXT)    quota_exhausted / token_invalid / convert_failed / ...
//! - `metering_unit`       (TEXT)    "credit"
//! - `metering_usage`      (REAL)    Kiro meteringEvent.usage（关键信号）
//! - `context_usage_pct`   (REAL)    contextUsageEvent.context_usage_percentage
//! - `has_cache_control`   (INTEGER) 客户端是否传了 cache_control
//! - `messages_count`      (INTEGER) 请求里的 messages 数
//! - `tools_count`         (INTEGER) tools 数
//! - `system_prompt_len`   (INTEGER) system 字符长度
//!
//! ## 辅表
//!
//! - `errors`：失败请求的详细错误（与 requests 一对一）
//! - `request_bodies`：可选的请求/响应原文存档（按配置开关）

use rusqlite::{Connection, Result as SqlResult};

/// 初始化数据库（建表 + 建索引 + WAL）
///
/// 幂等：表已存在时不报错。
pub fn init(conn: &Connection) -> SqlResult<()> {
    conn.pragma_update(None, "journal_mode", &"WAL")?;
    conn.pragma_update(None, "synchronous", &"NORMAL")?;
    conn.pragma_update(None, "foreign_keys", &"ON")?;

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS requests (
            -- 主键 + 时间
            request_id              TEXT PRIMARY KEY,
            ts_ms                   INTEGER NOT NULL,

            -- 客户端身份（对齐 ALLinOne）
            public_key_id           TEXT,
            client_id               TEXT,
            session_id              TEXT,

            -- 模型映射
            model                   TEXT NOT NULL,
            upstream_model          TEXT,

            -- 路由与账号
            endpoint                TEXT NOT NULL,
            pool_id                 TEXT NOT NULL DEFAULT 'kiro',
            account_id              TEXT,
            account_label           TEXT,

            -- 状态
            status                  TEXT NOT NULL,
            http_status             INTEGER,
            reason                  TEXT,
            error_kind              TEXT,
            error_message           TEXT,
            attempts                INTEGER NOT NULL DEFAULT 1,
            is_stream               INTEGER NOT NULL,

            -- 时延
            latency_ms              INTEGER NOT NULL,
            ttfb_ms                 INTEGER,

            -- Token 计量
            prompt_tokens           INTEGER,
            completion_tokens       INTEGER,
            cached_tokens           INTEGER,
            cache_creation_tokens   INTEGER,
            cost                    REAL,

            -- Kiro 独有信号
            metering_unit           TEXT,
            metering_usage          REAL,
            context_usage_pct       REAL,
            has_cache_control       INTEGER,
            messages_count          INTEGER,
            tools_count             INTEGER,
            system_prompt_len       INTEGER,

            -- 透传：客户端原始参数（脱敏后的 JSON）
            params_json             TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_requests_ts       ON requests(ts_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_requests_account  ON requests(account_id, ts_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_requests_status   ON requests(status, ts_ms DESC);
        CREATE INDEX IF NOT EXISTS idx_requests_model    ON requests(model, ts_ms DESC);

        CREATE TABLE IF NOT EXISTS errors (
            request_id  TEXT PRIMARY KEY REFERENCES requests(request_id) ON DELETE CASCADE,
            stage       TEXT,
            code        TEXT,
            message     TEXT
        );

        CREATE TABLE IF NOT EXISTS request_bodies (
            request_id     TEXT PRIMARY KEY REFERENCES requests(request_id) ON DELETE CASCADE,
            request_body   TEXT,
            response_body  TEXT
        );

        -- 反代访问密钥（可配置多个，任意一个都能通过认证）。
        -- 独立于 requests 环形缓冲，不会被 trim_to 清理。
        CREATE TABLE IF NOT EXISTS api_keys (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            key         TEXT NOT NULL UNIQUE,
            label       TEXT,
            created_at  INTEGER NOT NULL
        );
        "#,
    )?;

    // v28 迁移：cache_read_reported = 实际发给 NewAPI 的 cache_read（被 perceived 比例放大后的值）。
    // 注：`cached_tokens` 列继续记真实估算值（保留给未来重拟合 baseline 用），互不覆盖。
    // ALTER TABLE ADD COLUMN 在 SQLite 不是幂等的，先 pragma 查一下再加。
    let has_col: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('requests') WHERE name = 'cache_read_reported'",
        [],
        |r| r.get(0),
    )?;
    if has_col == 0 {
        conn.execute(
            "ALTER TABLE requests ADD COLUMN cache_read_reported INTEGER",
            [],
        )?;
    }

    // v32 迁移：api_keys.disabled 列，支持禁用而不删除（保留 label/历史）
    let has_disabled: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('api_keys') WHERE name = 'disabled'",
        [],
        |r| r.get(0),
    )?;
    if has_disabled == 0 {
        conn.execute(
            "ALTER TABLE api_keys ADD COLUMN disabled INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    Ok(())
}

/// 删除超过 `max_records` 条的旧记录（按 ts_ms 升序，最旧的先删）
///
/// `errors` 和 `request_bodies` 通过 FOREIGN KEY CASCADE 自动清理。
pub fn trim_to(conn: &Connection, max_records: usize) -> SqlResult<usize> {
    conn.pragma_update(None, "foreign_keys", &"ON")?;

    let current: i64 = conn.query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))?;
    let excess = current.saturating_sub(max_records as i64);
    if excess <= 0 {
        return Ok(0);
    }

    let deleted = conn.execute(
        "DELETE FROM requests WHERE request_id IN (
            SELECT request_id FROM requests ORDER BY ts_ms ASC LIMIT ?1
        )",
        [excess],
    )?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init(&conn).unwrap();
        conn
    }

    fn insert_dummy(conn: &Connection, id: &str, ts: i64) {
        conn.execute(
            "INSERT INTO requests (request_id, ts_ms, model, endpoint, status, is_stream,
             latency_ms) VALUES (?1, ?2, 'm', '/v1/messages', 'success', 0, 0)",
            rusqlite::params![id, ts],
        )
        .unwrap();
    }

    #[test]
    fn init_is_idempotent() {
        let conn = open_memory();
        init(&conn).unwrap();
    }

    #[test]
    fn trim_removes_oldest_records() {
        let conn = open_memory();
        for i in 0..5 {
            insert_dummy(&conn, &format!("u{}", i), i * 1000);
        }
        let deleted = trim_to(&conn, 3).unwrap();
        assert_eq!(deleted, 2);
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 3);

        // 确认删的是最旧的两条
        let ids: Vec<String> = conn
            .prepare("SELECT request_id FROM requests ORDER BY ts_ms ASC")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids, vec!["u2", "u3", "u4"]);
    }

    #[test]
    fn trim_cascades_to_errors_and_bodies() {
        let conn = open_memory();
        insert_dummy(&conn, "x", 0);

        conn.execute(
            "INSERT INTO errors (request_id, code, message) VALUES ('x', 'TestErr', 'msg')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_bodies (request_id, request_body) VALUES ('x', '{}')",
            [],
        )
        .unwrap();

        trim_to(&conn, 0).unwrap();

        let errs: i64 = conn
            .query_row("SELECT COUNT(*) FROM errors", [], |r| r.get(0))
            .unwrap();
        let bodies: i64 = conn
            .query_row("SELECT COUNT(*) FROM request_bodies", [], |r| r.get(0))
            .unwrap();
        assert_eq!(errs, 0, "errors 应级联删除");
        assert_eq!(bodies, 0, "request_bodies 应级联删除");
    }

    #[test]
    fn trim_noop_when_under_limit() {
        let conn = open_memory();
        insert_dummy(&conn, "only", 0);
        assert_eq!(trim_to(&conn, 100).unwrap(), 0);
    }

    #[test]
    fn schema_has_all_allinone_aligned_columns() {
        let conn = open_memory();
        let cols: Vec<String> = conn
            .prepare("PRAGMA table_info(requests)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // ALLinOne 关键字段必须存在
        for must in &[
            "request_id", "ts_ms", "public_key_id", "client_id", "session_id",
            "model", "upstream_model", "pool_id", "account_id", "status",
            "reason", "latency_ms", "ttfb_ms", "prompt_tokens", "completion_tokens",
            "cached_tokens", "cache_creation_tokens", "cost", "error_message",
            "params_json",
        ] {
            assert!(cols.contains(&must.to_string()), "缺少列: {}", must);
        }
    }
}
