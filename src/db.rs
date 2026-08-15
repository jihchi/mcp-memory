use crate::okf::{self, OkfOptions, ParsedOkf};
use rusqlite::{Connection, OptionalExtension, params};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone)]
pub struct Paths {
    pub db: PathBuf,
    pub memories: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StoredMemory {
    pub key: String,
    pub namespace: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub okf_payload: String,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RetrievedMemory {
    pub parsed: ParsedOkf,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub key: String,
    pub namespace: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub okf_payload: String,
}

pub fn project_root() -> PathBuf {
    if let Some(path) = env::var_os("MCP_MEMORY_PROJECT_ROOT") {
        let path = absolute(Path::new(&path));
        if path != Path::new("/") && writable(&path) {
            return path;
        }
    }
    if let Ok(path) = env::current_dir() {
        let path = absolute(&path);
        if path != Path::new("/") && writable(&path) {
            return path;
        }
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let fallback = home.join(".mcp_memory");
    let _ = fs::create_dir_all(&fallback);
    fallback
}

fn writable(path: &Path) -> bool {
    path.is_dir()
        && !fs::metadata(path)
            .map(|m| m.permissions().readonly())
            .unwrap_or(true)
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().unwrap_or_default().join(path)
    }
}

pub fn paths(project_override: Option<&str>) -> Paths {
    let root = project_override
        .map(PathBuf::from)
        .map(|p| absolute(&p))
        .unwrap_or_else(project_root);
    let db_raw =
        env::var("MCP_MEMORY_DB_PATH").unwrap_or_else(|_| ".mcp_memory/memories.db".into());
    let mem_raw = env::var("MCP_MEMORY_DIR").unwrap_or_else(|_| "memory".into());
    let resolve = |raw: String| {
        let p = PathBuf::from(raw);
        if p.is_absolute() { p } else { root.join(p) }
    };
    Paths {
        db: resolve(db_raw),
        memories: resolve(mem_raw),
    }
}

fn connection(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Connection::open(path).map_err(|e| e.to_string())
}

pub fn init_db(path: &Path) -> Result<()> {
    let conn = connection(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS memories (
            key TEXT NOT NULL,
            namespace TEXT NOT NULL DEFAULT 'default',
            okf_payload TEXT NOT NULL,
            tags TEXT NOT NULL DEFAULT '[]',
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (key, namespace)
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
            key UNINDEXED, namespace UNINDEXED, tags, okf_payload
         );
         CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
            INSERT INTO memories_fts(rowid, key, namespace, tags, okf_payload)
            VALUES (new.rowid, new.key, new.namespace, new.tags, new.okf_payload);
         END;
         CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
            DELETE FROM memories_fts WHERE rowid = old.rowid;
         END;
         CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
            DELETE FROM memories_fts WHERE rowid = old.rowid;
            INSERT INTO memories_fts(rowid, key, namespace, tags, okf_payload)
            VALUES (new.rowid, new.key, new.namespace, new.tags, new.okf_payload);
         END;",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn store_memory(mut options: OkfOptions, project_root: Option<&str>) -> Result<StoredMemory> {
    let paths = paths(project_root);
    init_db(&paths.db)?;
    let now = okf::utc_now_iso();
    let conn = connection(&paths.db)?;
    let created_at: Option<String> = conn
        .query_row(
            "SELECT created_at FROM memories WHERE key = ?1 AND namespace = ?2",
            params![options.key, options.namespace],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    let created_at = created_at.unwrap_or_else(|| now.clone());
    options.created_at = Some(created_at.clone());
    options.updated_at = Some(now.clone());
    let payload = okf::serialize_okf(&options)?;
    let tags_json = serde_json::to_string(&options.tags).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO memories (key, namespace, okf_payload, tags, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(key, namespace) DO UPDATE SET
            okf_payload = excluded.okf_payload,
            tags = excluded.tags,
            updated_at = excluded.updated_at",
        params![
            options.key,
            options.namespace,
            payload,
            tags_json,
            created_at,
            now
        ],
    )
    .map_err(|e| e.to_string())?;
    let file_path =
        okf::sync_memory_to_disk(&options.key, &options.namespace, &payload, &paths.memories)?;
    Ok(StoredMemory {
        key: options.key,
        namespace: options.namespace,
        tags: options.tags,
        created_at,
        updated_at: now,
        okf_payload: payload,
        file_path,
    })
}

pub fn retrieve_memory(
    key: &str,
    namespace: &str,
    project_root: Option<&str>,
) -> Result<Option<RetrievedMemory>> {
    let paths = paths(project_root);
    init_db(&paths.db)?;
    let conn = connection(&paths.db)?;
    let row: Option<(String, String, String)> = conn.query_row(
        "SELECT okf_payload, created_at, updated_at FROM memories WHERE key = ?1 AND namespace = ?2",
        params![key, namespace], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    ).optional().map_err(|e| e.to_string())?;
    row.map(|(payload, created_at, updated_at)| {
        Ok(RetrievedMemory {
            parsed: okf::parse_okf(&payload)?,
            created_at,
            updated_at,
        })
    })
    .transpose()
}

pub fn search_memories(
    query: Option<&str>,
    tags: Option<&[String]>,
    namespace: Option<&str>,
    limit: i64,
    project_root: Option<&str>,
) -> Result<Vec<SearchResult>> {
    let paths = paths(project_root);
    init_db(&paths.db)?;
    let conn = connection(&paths.db)?;
    let query = query.filter(|q| !q.trim().is_empty());
    let candidate_limit = if tags.is_some_and(|v| !v.is_empty()) {
        -1
    } else {
        limit.saturating_mul(2)
    };
    let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut sql;
    if let Some(query) = query {
        let escaped = query
            .split_whitespace()
            .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        sql = "SELECT m.key, m.namespace, m.okf_payload, m.created_at, m.updated_at
               FROM memories m JOIN memories_fts fts ON m.rowid = fts.rowid
               WHERE memories_fts MATCH ?"
            .to_owned();
        values.push(Box::new(escaped));
    } else {
        sql = "SELECT m.key, m.namespace, m.okf_payload, m.created_at, m.updated_at FROM memories m WHERE 1=1".into();
    }
    if let Some(namespace) = namespace {
        sql.push_str(" AND m.namespace = ?");
        values.push(Box::new(namespace.to_owned()));
    }
    sql.push_str(if query.is_some() {
        " ORDER BY rank LIMIT ?"
    } else {
        " ORDER BY m.updated_at DESC LIMIT ?"
    });
    values.push(Box::new(candidate_limit));
    let refs = values.iter().map(|v| &**v).collect::<Vec<_>>();

    type Row = (String, String, String, String, String);
    let read_rows = |sql: &str, refs: &[&dyn rusqlite::ToSql]| -> Result<Vec<Row>> {
        let mut statement = conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = statement
            .query_map(refs, |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    };
    let rows = match read_rows(&sql, &refs) {
        Ok(rows) => rows,
        Err(_) if query.is_some() => {
            let mut fallback = "SELECT m.key, m.namespace, m.okf_payload, m.created_at, m.updated_at FROM memories m WHERE (m.key LIKE ? OR m.okf_payload LIKE ?)".to_owned();
            let pattern = format!("%{}%", query.unwrap());
            let mut fallback_values: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(pattern.clone()), Box::new(pattern)];
            if let Some(namespace) = namespace {
                fallback.push_str(" AND m.namespace = ?");
                fallback_values.push(Box::new(namespace.to_owned()));
            }
            fallback.push_str(" ORDER BY m.updated_at DESC LIMIT ?");
            fallback_values.push(Box::new(candidate_limit));
            let fallback_refs = fallback_values.iter().map(|v| &**v).collect::<Vec<_>>();
            read_rows(&fallback, &fallback_refs)?
        }
        Err(error) => return Err(error),
    };

    let mut results = Vec::new();
    for (key, namespace, payload, created_at, updated_at) in rows {
        let parsed = okf::parse_okf(&payload)?;
        if let Some(filter) = tags
            && !filter.is_empty()
            && !filter.iter().any(|tag| parsed.tags.contains(tag))
        {
            continue;
        }
        results.push(SearchResult {
            key,
            namespace,
            tags: parsed.tags,
            created_at,
            updated_at,
            okf_payload: payload,
        });
        if results.len() >= limit.max(0) as usize {
            break;
        }
    }
    Ok(results)
}

pub fn delete_memory(key: &str, namespace: &str, project_root: Option<&str>) -> Result<bool> {
    let paths = paths(project_root);
    init_db(&paths.db)?;
    let conn = connection(&paths.db)?;
    let count = conn
        .execute(
            "DELETE FROM memories WHERE key = ?1 AND namespace = ?2",
            params![key, namespace],
        )
        .map_err(|e| e.to_string())?;
    if count > 0 {
        okf::remove_memory_from_disk(key, namespace, &paths.memories)?;
    }
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn options(key: &str, content: Value, tags: &[&str]) -> OkfOptions {
        OkfOptions {
            key: key.into(),
            namespace: "default".into(),
            content,
            tags: tags.iter().map(|v| (*v).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn store_retrieve_search_delete() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_str().unwrap();
        store_memory(
            options(
                "key/alpha",
                json!("Python programming language"),
                &["code", "python"],
            ),
            Some(root),
        )
        .unwrap();
        assert_eq!(
            retrieve_memory("key/alpha", "default", Some(root))
                .unwrap()
                .unwrap()
                .parsed
                .body,
            "Python programming language"
        );
        assert_eq!(
            search_memories(
                Some("Python"),
                Some(&["python".into()]),
                None,
                10,
                Some(root)
            )
            .unwrap()
            .len(),
            1
        );
        assert!(delete_memory("key/alpha", "default", Some(root)).unwrap());
        assert!(
            retrieve_memory("key/alpha", "default", Some(root))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn update_preserves_creation_time() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_str().unwrap();
        let first = store_memory(options("key/update", json!("first"), &[]), Some(root)).unwrap();
        let second = store_memory(options("key/update", json!("second"), &[]), Some(root)).unwrap();
        assert_eq!(first.created_at, second.created_at);
        assert_eq!(
            retrieve_memory("key/update", "default", Some(root))
                .unwrap()
                .unwrap()
                .parsed
                .body,
            "second"
        );
    }

    #[test]
    fn tag_filter_does_not_lose_older_matches() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().to_str().unwrap();
        store_memory(
            options("key/wanted", json!("target"), &["wanted"]),
            Some(root),
        )
        .unwrap();
        for index in 0..25 {
            store_memory(
                options(&format!("key/other-{index}"), json!("target"), &["other"]),
                Some(root),
            )
            .unwrap();
        }
        let results = search_memories(
            Some("target"),
            Some(&["wanted".into()]),
            None,
            10,
            Some(root),
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].key, "key/wanted");
    }
}
