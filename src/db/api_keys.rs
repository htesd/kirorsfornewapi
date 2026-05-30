//! 反代访问密钥的 SQLite 持久化（支持多个 key）
//!
//! 与请求日志同库（`api_keys` 表），写入极少（仅 Admin 增删 key 时），
//! 因此每次按需打开短连接，不与 writer 的长连接共享。WAL 模式下多连接安全。

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Result as SqlResult};

/// 一条 API Key 记录
#[derive(Debug, Clone)]
pub struct ApiKeyRow {
    pub id: i64,
    pub key: String,
    pub label: Option<String>,
    /// 创建时间（Unix 毫秒）
    pub created_at: i64,
    /// 是否禁用（true 则中间件认证时不再匹配此 key）
    pub disabled: bool,
}

/// 打开读写连接并确保 schema 存在（写入极少，按需开短连接）
fn open_rw<P: AsRef<Path>>(path: P) -> SqlResult<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    super::schema::init(&conn)?;
    Ok(conn)
}

/// 列出全部 key（按 id 升序，含禁用的）
pub fn list<P: AsRef<Path>>(path: P) -> SqlResult<Vec<ApiKeyRow>> {
    let conn = open_rw(path)?;
    let mut stmt = conn.prepare(
        "SELECT id, key, label, created_at, COALESCE(disabled, 0) FROM api_keys ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ApiKeyRow {
            id: r.get(0)?,
            key: r.get(1)?,
            label: r.get(2)?,
            created_at: r.get(3)?,
            disabled: r.get::<_, i64>(4)? != 0,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 仅取**启用**的 key 字符串（中间件认证用，禁用的不返回）
pub fn list_keys<P: AsRef<Path>>(path: P) -> SqlResult<Vec<String>> {
    Ok(list(path)?
        .into_iter()
        .filter(|r| !r.disabled)
        .map(|r| r.key)
        .collect())
}

/// 设置 key 的启用/禁用状态。返回受影响行数（0 = id 不存在）
pub fn set_disabled<P: AsRef<Path>>(path: P, id: i64, disabled: bool) -> SqlResult<usize> {
    let conn = open_rw(path)?;
    conn.execute(
        "UPDATE api_keys SET disabled = ?1 WHERE id = ?2",
        rusqlite::params![if disabled { 1 } else { 0 }, id],
    )
}

/// 新增 key，返回新行 id。key 重复时返回 UNIQUE 约束错误。
pub fn add<P: AsRef<Path>>(path: P, key: &str, label: Option<&str>) -> SqlResult<i64> {
    let conn = open_rw(path)?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO api_keys (key, label, created_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![key, label, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 按 id 删除，返回删除行数
pub fn delete<P: AsRef<Path>>(path: P, id: i64) -> SqlResult<usize> {
    let conn = open_rw(path)?;
    conn.execute("DELETE FROM api_keys WHERE id = ?1", [id])
}

/// 当前 key 数量
pub fn count<P: AsRef<Path>>(path: P) -> SqlResult<i64> {
    let conn = open_rw(path)?;
    conn.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn empty_list_when_fresh() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        assert!(list(&path).unwrap().is_empty());
        assert_eq!(count(&path).unwrap(), 0);
    }

    #[test]
    fn add_then_list() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        add(&path, "sk-a", Some("first")).unwrap();
        add(&path, "sk-b", None).unwrap();
        let rows = list(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].key, "sk-a");
        assert_eq!(rows[0].label.as_deref(), Some("first"));
        assert_eq!(rows[1].key, "sk-b");
        assert_eq!(list_keys(&path).unwrap(), vec!["sk-a", "sk-b"]);
    }

    #[test]
    fn duplicate_key_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        add(&path, "dup", None).unwrap();
        assert!(add(&path, "dup", None).is_err());
    }

    #[test]
    fn delete_removes_row() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        let id = add(&path, "sk-x", None).unwrap();
        assert_eq!(delete(&path, id).unwrap(), 1);
        assert!(list(&path).unwrap().is_empty());
    }

    #[test]
    fn set_disabled_filters_from_list_keys() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        let id_a = add(&path, "sk-a", None).unwrap();
        add(&path, "sk-b", None).unwrap();
        // Both keys active initially
        assert_eq!(list_keys(&path).unwrap(), vec!["sk-a", "sk-b"]);
        // Disable sk-a → list_keys should only return sk-b, but list() returns both
        assert_eq!(set_disabled(&path, id_a, true).unwrap(), 1);
        assert_eq!(list_keys(&path).unwrap(), vec!["sk-b"]);
        let rows = list(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().find(|r| r.id == id_a).unwrap().disabled);
        // Re-enable
        assert_eq!(set_disabled(&path, id_a, false).unwrap(), 1);
        assert_eq!(list_keys(&path).unwrap(), vec!["sk-a", "sk-b"]);
    }

    #[test]
    fn keys_survive_request_trim() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        add(&path, "keep", None).unwrap();
        let conn = Connection::open(&path).unwrap();
        super::super::schema::trim_to(&conn, 0).unwrap();
        drop(conn);
        assert_eq!(list_keys(&path).unwrap(), vec!["keep"]);
    }
}
