//! 账号池分组的 SQLite 持久化
//!
//! 三张表协作（见 `schema.rs`）：
//! - `groups`：分组定义（id / name / created_at）
//! - `credential_groups`：凭据 → 分组 的多对一映射（credential_id 为 token_manager 稳定 id）
//! - `api_keys.group_id`：apikey → 分组 的绑定（NULL = 未绑定，可用全部账号）
//!
//! 分组是纯运营层，与 credentials.json 解耦：增删分组、调整归属都只动 SQLite，
//! 不触碰凭据文件。写入极少（仅 Admin 操作），按需开短连接，WAL 下多连接安全。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, Result as SqlResult};

/// 一个分组
#[derive(Debug, Clone)]
pub struct GroupRow {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

/// 打开读写连接并确保 schema 存在（写入极少，按需开短连接）
fn open_rw<P: AsRef<Path>>(path: P) -> SqlResult<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", &"ON")?;
    super::schema::init(&conn)?;
    Ok(conn)
}

/// 列出全部分组（按 id 升序）
pub fn list<P: AsRef<Path>>(path: P) -> SqlResult<Vec<GroupRow>> {
    let conn = open_rw(path)?;
    let mut stmt =
        conn.prepare("SELECT id, name, created_at FROM groups ORDER BY id ASC")?;
    let rows = stmt.query_map([], |r| {
        Ok(GroupRow {
            id: r.get(0)?,
            name: r.get(1)?,
            created_at: r.get(2)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 新增分组，返回新行 id。name 重复时返回 UNIQUE 约束错误。
pub fn add<P: AsRef<Path>>(path: P, name: &str) -> SqlResult<i64> {
    let conn = open_rw(path)?;
    let now = chrono::Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO groups (name, created_at) VALUES (?1, ?2)",
        rusqlite::params![name, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// 删除分组（级联清空 credential_groups 中的归属；绑定该组的 apikey 被置回未绑定）。
/// 返回删除的分组行数（0 = id 不存在）。
pub fn delete<P: AsRef<Path>>(path: P, id: i64) -> SqlResult<usize> {
    let mut conn = open_rw(path)?;
    let tx = conn.transaction()?;
    // 解绑引用该组的 apikey（外键无 ON DELETE，手动置 NULL，避免悬空绑定）
    tx.execute(
        "UPDATE api_keys SET group_id = NULL WHERE group_id = ?1",
        [id],
    )?;
    // credential_groups 通过 FOREIGN KEY ON DELETE CASCADE 自动清理
    let n = tx.execute("DELETE FROM groups WHERE id = ?1", [id])?;
    tx.commit()?;
    Ok(n)
}

/// 重命名分组。返回受影响行数（0 = id 不存在）；name 重复返回 UNIQUE 错误。
pub fn rename<P: AsRef<Path>>(path: P, id: i64, name: &str) -> SqlResult<usize> {
    let conn = open_rw(path)?;
    conn.execute(
        "UPDATE groups SET name = ?1 WHERE id = ?2",
        rusqlite::params![name, id],
    )
}

/// 设置某凭据的分组归属。`group_id=None` 表示移出分组（删除映射）。
/// `group_id=Some(g)` 时 upsert（一个凭据只属于一个分组）。
pub fn set_credential_group<P: AsRef<Path>>(
    path: P,
    credential_id: i64,
    group_id: Option<i64>,
) -> SqlResult<()> {
    let conn = open_rw(path)?;
    match group_id {
        None => {
            conn.execute(
                "DELETE FROM credential_groups WHERE credential_id = ?1",
                [credential_id],
            )?;
        }
        Some(g) => {
            let now = chrono::Utc::now().timestamp_millis();
            conn.execute(
                "INSERT INTO credential_groups (credential_id, group_id, assigned_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(credential_id) DO UPDATE SET group_id = ?2, assigned_at = ?3",
                rusqlite::params![credential_id, g, now],
            )?;
        }
    }
    Ok(())
}

/// 返回全部 credential_id → group_id 映射（供前端展示账号归属）。
pub fn credential_group_map<P: AsRef<Path>>(path: P) -> SqlResult<HashMap<i64, i64>> {
    let conn = open_rw(path)?;
    let mut stmt = conn.prepare("SELECT credential_id, group_id FROM credential_groups")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    let mut map = HashMap::new();
    for r in rows {
        let (cid, gid) = r?;
        map.insert(cid, gid);
    }
    Ok(map)
}

/// 取某分组下的全部 credential_id（中间件鉴权时用来构造允许集合）。
pub fn credentials_in_group<P: AsRef<Path>>(path: P, group_id: i64) -> SqlResult<Vec<i64>> {
    let conn = open_rw(path)?;
    let mut stmt = conn
        .prepare("SELECT credential_id FROM credential_groups WHERE group_id = ?1 ORDER BY credential_id ASC")?;
    let rows = stmt.query_map([group_id], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

/// 按 apikey 明文查其绑定的 group_id（None = 未绑定 / key 不存在 / 已禁用）。
///
/// 中间件认证通过后调用：返回 `Some(g)` 表示该 key 严格隔离到分组 g；
/// 返回 `None` 表示该 key 未绑定分组，沿用历史行为（可用全部账号）。
pub fn group_id_for_key<P: AsRef<Path>>(path: P, key: &str) -> SqlResult<Option<i64>> {
    let conn = open_rw(path)?;
    conn.query_row(
        "SELECT group_id FROM api_keys WHERE key = ?1 AND COALESCE(disabled, 0) = 0",
        [key],
        |r| r.get::<_, Option<i64>>(0),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    })
}

/// 设置某 apikey 的分组绑定。`group_id=None` 解绑。返回受影响行数。
pub fn set_key_group<P: AsRef<Path>>(
    path: P,
    key_id: i64,
    group_id: Option<i64>,
) -> SqlResult<usize> {
    let conn = open_rw(path)?;
    conn.execute(
        "UPDATE api_keys SET group_id = ?1 WHERE id = ?2",
        rusqlite::params![group_id, key_id],
    )
}

/// 认证后一次性解析某 apikey 的允许凭据集合（单连接，避免两次开库）。
///
/// 返回：
/// - `Ok(None)`：key 未绑定分组（或 key 不存在/已禁用）→ 调用方按"全部账号"处理（历史行为）。
/// - `Ok(Some(set))`：key 绑定了分组 → **严格隔离**到该集合。集合可能为空
///   （分组无成员），此时调用方应判定为"无可用账号"并返回错误。
pub fn allowed_credential_ids_for_key<P: AsRef<Path>>(
    path: P,
    key: &str,
) -> SqlResult<Option<std::collections::HashSet<u64>>> {
    let conn = open_rw(path)?;
    let group_id: Option<i64> = conn
        .query_row(
            "SELECT group_id FROM api_keys WHERE key = ?1 AND COALESCE(disabled, 0) = 0",
            [key],
            |r| r.get::<_, Option<i64>>(0),
        )
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;

    let group_id = match group_id {
        Some(g) => g,
        None => return Ok(None),
    };

    let mut stmt =
        conn.prepare("SELECT credential_id FROM credential_groups WHERE group_id = ?1")?;
    let rows = stmt.query_map([group_id], |r| r.get::<_, i64>(0))?;
    let mut set = std::collections::HashSet::new();
    for r in rows {
        set.insert(r? as u64);
    }
    Ok(Some(set))
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.db");
        (dir, path)
    }

    #[test]
    fn add_list_rename_delete() {
        let (_d, path) = temp_db();
        let g1 = add(&path, "team-a").unwrap();
        let g2 = add(&path, "team-b").unwrap();
        let rows = list(&path).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "team-a");

        assert_eq!(rename(&path, g1, "team-a2").unwrap(), 1);
        assert_eq!(list(&path).unwrap()[0].name, "team-a2");

        assert_eq!(delete(&path, g2).unwrap(), 1);
        assert_eq!(list(&path).unwrap().len(), 1);
    }

    #[test]
    fn duplicate_group_name_errors() {
        let (_d, path) = temp_db();
        add(&path, "dup").unwrap();
        assert!(add(&path, "dup").is_err());
    }

    #[test]
    fn credential_group_assignment_is_upsert() {
        let (_d, path) = temp_db();
        let g1 = add(&path, "g1").unwrap();
        let g2 = add(&path, "g2").unwrap();
        // 凭据 5 归入 g1
        set_credential_group(&path, 5, Some(g1)).unwrap();
        assert_eq!(credentials_in_group(&path, g1).unwrap(), vec![5]);
        // 改归 g2（upsert，不重复）
        set_credential_group(&path, 5, Some(g2)).unwrap();
        assert!(credentials_in_group(&path, g1).unwrap().is_empty());
        assert_eq!(credentials_in_group(&path, g2).unwrap(), vec![5]);
        // 移出分组
        set_credential_group(&path, 5, None).unwrap();
        assert!(credentials_in_group(&path, g2).unwrap().is_empty());
    }

    #[test]
    fn delete_group_cascades_credentials_and_unbinds_keys() {
        let (_d, path) = temp_db();
        let g = add(&path, "g").unwrap();
        set_credential_group(&path, 1, Some(g)).unwrap();
        set_credential_group(&path, 2, Some(g)).unwrap();
        // 一个 apikey 绑定该组
        let kid = crate::db::api_keys::add(&path, "sk-x", None).unwrap();
        set_key_group(&path, kid, Some(g)).unwrap();
        assert_eq!(group_id_for_key(&path, "sk-x").unwrap(), Some(g));

        // 删组：credential_groups 级联清空、apikey 解绑
        assert_eq!(delete(&path, g).unwrap(), 1);
        assert!(credentials_in_group(&path, g).unwrap().is_empty());
        assert_eq!(group_id_for_key(&path, "sk-x").unwrap(), None);
    }

    #[test]
    fn group_id_for_key_respects_disabled_and_missing() {
        let (_d, path) = temp_db();
        let g = add(&path, "g").unwrap();
        let kid = crate::db::api_keys::add(&path, "sk-a", None).unwrap();
        set_key_group(&path, kid, Some(g)).unwrap();
        assert_eq!(group_id_for_key(&path, "sk-a").unwrap(), Some(g));
        // 未绑定的 key → None
        crate::db::api_keys::add(&path, "sk-b", None).unwrap();
        assert_eq!(group_id_for_key(&path, "sk-b").unwrap(), None);
        // 不存在的 key → None
        assert_eq!(group_id_for_key(&path, "nope").unwrap(), None);
        // 禁用后 → None（不参与隔离，认证层本身也会拒绝）
        crate::db::api_keys::set_disabled(&path, kid, true).unwrap();
        assert_eq!(group_id_for_key(&path, "sk-a").unwrap(), None);
    }

    #[test]
    fn credential_group_map_returns_all() {
        let (_d, path) = temp_db();
        let g1 = add(&path, "g1").unwrap();
        let g2 = add(&path, "g2").unwrap();
        set_credential_group(&path, 1, Some(g1)).unwrap();
        set_credential_group(&path, 2, Some(g1)).unwrap();
        set_credential_group(&path, 3, Some(g2)).unwrap();
        let map = credential_group_map(&path).unwrap();
        assert_eq!(map.get(&1), Some(&g1));
        assert_eq!(map.get(&2), Some(&g1));
        assert_eq!(map.get(&3), Some(&g2));
        assert_eq!(map.len(), 3);
    }
}


