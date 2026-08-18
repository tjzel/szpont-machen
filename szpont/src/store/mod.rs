pub mod checkpoints;
mod schema;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::core::{SessionKey, TitleKind, ToolId, now_ms, restrict_permissions};

const MAX_STORED_PREVIEW_CHARS: usize = crate::core::MAX_PREVIEW_CHARS;
const MAX_STORED_CWD_CHARS: usize = 4096;

pub struct Store {
    conn: Connection,
}

pub struct StoredMeta {
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub title_source: Option<String>,
    pub preview: Option<String>,
    pub model: Option<String>,
    pub context_tokens: Option<i64>,
    pub context_window: Option<i64>,
    pub completed_at: Option<i64>,
    pub last_seen_at: Option<i64>,
}

impl StoredMeta {
    pub fn custom_title(&self) -> Option<String> {
        if self.title_source.as_deref() == Some(TitleKind::Custom.as_str()) {
            self.title.clone()
        } else {
            None
        }
    }
}

impl Store {
    pub fn open(path: Option<&Path>) -> anyhow::Result<Store> {
        let is_default = path.is_none();
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => default_path()?,
        };
        if let Some(dir) = path.parent()
            && !dir.as_os_str().is_empty()
            && (is_default || !dir.exists())
        {
            create_private_dir(dir)?;
        }
        create_private_file(&path)?;
        let conn = Connection::open(&path)?;
        for suffix in ["", "-wal", "-shm"] {
            let mut target = path.clone().into_os_string();
            target.push(suffix);
            restrict_permissions(std::path::Path::new(&target), 0o600);
        }
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 2 {
            conn.execute_batch(schema::DROP_ALL)?;
        } else {
            if version == 2 {
                conn.execute_batch(schema::MIGRATE_V2_TO_V3)?;
            }
            if (2..=3).contains(&version) {
                conn.execute_batch(schema::MIGRATE_V3_TO_V4)?;
            }
        }
        conn.execute_batch(schema::SCHEMA)?;
        if version < schema::SCHEMA_VERSION {
            conn.pragma_update(None, "user_version", schema::SCHEMA_VERSION)?;
        }
        if is_default
            && let Some(home) = dirs::home_dir()
            && let Some(dir) = path.parent()
            && let Err(err) = migrate_legacy_mleko(&conn, &home.join(".mleko"), dir)
        {
            crate::logging::warn(&format!("legacy data migration failed: {err:#}"));
        }
        Ok(Store { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn mark_completed(&self, tool: ToolId, session_id: &str, by: &str) -> anyhow::Result<()> {
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO sessions_meta (tool, session_id, first_seen_at, completed_at, completed_by)
             VALUES (?1, ?2, ?3, ?3, ?4)
             ON CONFLICT (tool, session_id)
             DO UPDATE SET completed_at = excluded.completed_at, completed_by = excluded.completed_by",
            params![tool.as_str(), session_id, now, by],
        )?;
        Ok(())
    }

    pub fn reopen(&self, tool: ToolId, session_id: &str) -> anyhow::Result<bool> {
        let changed = self.conn.execute(
            "UPDATE sessions_meta SET completed_at = NULL, completed_by = NULL
             WHERE tool = ?1 AND session_id = ?2 AND completed_at IS NOT NULL",
            params![tool.as_str(), session_id],
        )?;
        Ok(changed > 0)
    }

    pub fn session_meta_map(&self) -> anyhow::Result<HashMap<SessionKey, StoredMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool, session_id, cwd, title, title_source, preview, model,
                    context_tokens, context_window, completed_at, last_seen_at
             FROM sessions_meta",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                StoredMeta {
                    cwd: row.get(2)?,
                    title: row.get(3)?,
                    title_source: row.get(4)?,
                    preview: row.get(5)?,
                    model: row.get(6)?,
                    context_tokens: row.get(7)?,
                    context_window: row.get(8)?,
                    completed_at: row.get(9)?,
                    last_seen_at: row.get(10)?,
                },
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (tool, id, meta) = row?;
            if let Ok(tool) = ToolId::parse(&tool) {
                map.insert(SessionKey { tool, id }, meta);
            }
        }
        Ok(map)
    }

    pub fn custom_titles(&self) -> anyhow::Result<HashMap<SessionKey, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool, session_id, title FROM sessions_meta
             WHERE title_source = 'custom' AND title IS NOT NULL",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (tool, id, title) = row?;
            if let Ok(tool) = ToolId::parse(&tool) {
                map.insert(SessionKey { tool, id }, title);
            }
        }
        Ok(map)
    }

    pub fn delete_session_data(&self, tool: ToolId, session_id: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM sessions_meta WHERE tool = ?1 AND session_id = ?2",
            params![tool.as_str(), session_id],
        )?;
        self.conn.execute(
            "DELETE FROM usage_cache WHERE tool = ?1 AND session_id = ?2",
            params![tool.as_str(), session_id],
        )?;
        self.conn.execute(
            "DELETE FROM mcp_events WHERE tool = ?1 AND session_id = ?2",
            params![tool.as_str(), session_id],
        )?;
        Ok(())
    }

    pub fn project_dirs(&self) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM meta WHERE key LIKE 'project_dir:%'")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut dirs = Vec::new();
        for row in rows {
            let (key, name) = row?;
            if let Some(dir) = key.strip_prefix("project_dir:")
                && !name.is_empty()
            {
                dirs.push((PathBuf::from(dir), name));
            }
        }
        dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.as_os_str().len()));
        Ok(dirs)
    }

    pub fn set_project_dir(&self, dir: &Path, name: &str) -> anyhow::Result<()> {
        self.meta_set(&format!("project_dir:{}", dir.display()), name)
    }

    pub fn clear_project_dir(&self, dir: &Path) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM meta WHERE key = ?1",
            params![format!("project_dir:{}", dir.display())],
        )?;
        Ok(())
    }

    pub fn context_window_overrides(&self) -> anyhow::Result<HashMap<String, u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM meta WHERE key LIKE 'context_window_override:%'")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (key, value) = row?;
            if let (Some(model), Ok(tokens)) = (
                key.strip_prefix("context_window_override:"),
                value.parse::<u64>(),
            ) {
                map.insert(model.to_string(), tokens);
            }
        }
        Ok(map)
    }

    pub fn set_context_window_override(&self, model: &str, tokens: u64) -> anyhow::Result<()> {
        self.meta_set(
            &format!("context_window_override:{model}"),
            &tokens.to_string(),
        )
    }

    pub fn clear_context_window_override(&self, model: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM meta WHERE key = ?1",
            params![format!("context_window_override:{model}")],
        )?;
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> anyhow::Result<Option<String>> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(value.flatten())
    }

    pub fn meta_set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn save_limits_snapshot(&self, tool: ToolId, payload_json: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO limits_snapshots (tool, captured_at, payload_json) VALUES (?1, ?2, ?3)
             ON CONFLICT (tool) DO UPDATE SET
               captured_at = excluded.captured_at,
               payload_json = excluded.payload_json",
            params![tool.as_str(), now_ms(), payload_json],
        )?;
        Ok(())
    }

    pub fn load_limits_snapshots(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload_json FROM limits_snapshots ORDER BY tool")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.filter_map(std::result::Result::ok).collect())
    }

    pub fn bucket_tokens_since(&self, tool: ToolId, since_ts: i64) -> anyhow::Result<u64> {
        let total: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(input_uncached + input_cache_read + input_cache_write + output), 0)
             FROM usage_buckets WHERE tool = ?1 AND hour_ts >= ?2",
            params![tool.as_str(), since_ts],
            |row| row.get(0),
        )?;
        Ok(total.max(0) as u64)
    }
}

fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    restrict_permissions(dir, 0o700);
    Ok(())
}

fn create_private_file(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    Ok(())
}

fn restrict_tree(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        restrict_permissions(path, 0o700);
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            restrict_tree(&entry.path());
        }
    } else if meta.is_file() {
        restrict_permissions(path, 0o600);
    }
}

pub fn default_path() -> anyhow::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot resolve home directory"))?;
    Ok(home.join(".szpont").join("szpont.db"))
}

fn migrate_legacy_mleko(conn: &Connection, old_dir: &Path, new_dir: &Path) -> anyhow::Result<()> {
    let old_db = old_dir.join("mleko.db");
    let Ok(old_db_meta) = std::fs::symlink_metadata(&old_db) else {
        return Ok(());
    };
    if !old_db_meta.is_file() {
        return Ok(());
    }
    let already: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'legacy_mleko_migrated'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if already.is_some() {
        return Ok(());
    }
    conn.execute(
        "ATTACH DATABASE ?1 AS legacy",
        params![old_db.to_string_lossy()],
    )?;
    let merge = (|| -> anyhow::Result<()> {
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO sessions_meta
               (tool, session_id, cwd, title, title_source, preview, model,
                first_seen_at, completed_at, completed_by, source, context_tokens, context_window)
             SELECT tool, session_id, substr(cwd, 1, 4096), substr(title, 1, 512), title_source,
                    substr(preview, 1, 200), substr(model, 1, 128),
                    first_seen_at, completed_at, completed_by, source, context_tokens, context_window
             FROM legacy.sessions_meta WHERE tool IN ('claude', 'codex', 'kimi')
             ON CONFLICT (tool, session_id) DO UPDATE SET
               completed_at = COALESCE(sessions_meta.completed_at, excluded.completed_at),
               completed_by = COALESCE(sessions_meta.completed_by, excluded.completed_by),
               title = COALESCE(sessions_meta.title, excluded.title),
               title_source = COALESCE(sessions_meta.title_source, excluded.title_source),
               preview = COALESCE(sessions_meta.preview, excluded.preview),
               model = COALESCE(sessions_meta.model, excluded.model),
               context_tokens = COALESCE(sessions_meta.context_tokens, excluded.context_tokens),
               context_window = COALESCE(sessions_meta.context_window, excluded.context_window),
               cwd = COALESCE(sessions_meta.cwd, excluded.cwd);
             INSERT OR IGNORE INTO meta SELECT key, value FROM legacy.meta
               WHERE key LIKE 'context\\_window\\_override:%' ESCAPE '\\'
                  OR key IN ('claude_ceiling_5h', 'claude_ceiling_7d');
             INSERT OR REPLACE INTO meta (key, value) VALUES ('legacy_mleko_migrated', '1');
             COMMIT;",
        )?;
        Ok(())
    })();
    if merge.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    conn.execute_batch("DETACH DATABASE legacy")?;
    merge?;
    restrict_tree(old_dir);
    let backup = new_dir.join("mleko-backup");
    match std::fs::rename(old_dir, &backup) {
        Ok(()) => restrict_tree(&backup),
        Err(err) => crate::logging::warn(&format!(
            "cannot move {} to {}: {err}",
            old_dir.display(),
            backup.display()
        )),
    }
    Ok(())
}

pub fn upsert_session_facts(
    conn: &Connection,
    tool: ToolId,
    session_id: &str,
    cwd: Option<&str>,
    title: Option<(&str, crate::core::TitleKind)>,
    preview: Option<&str>,
    model: Option<&str>,
    context_tokens: Option<i64>,
    context_window: Option<i64>,
    source: &str,
) -> anyhow::Result<()> {
    let (title_text, title_source) = match title {
        Some((text, kind)) => (
            Some(crate::core::truncate(
                text,
                crate::core::MAX_TEXT_FACT_CHARS,
            )),
            Some(kind.as_str()),
        ),
        None => (None, None),
    };
    let preview = preview.map(|p| crate::core::truncate(p, MAX_STORED_PREVIEW_CHARS));
    let model = model.map(|m| crate::core::truncate(m, crate::core::MAX_MODEL_CHARS));
    let cwd = cwd.filter(|c| c.chars().count() <= MAX_STORED_CWD_CHARS);
    conn.execute(
        "INSERT INTO sessions_meta
           (tool, session_id, cwd, title, title_source, preview, model,
            context_tokens, context_window, first_seen_at, source)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
         ON CONFLICT (tool, session_id)
         DO UPDATE SET
           cwd = COALESCE(excluded.cwd, sessions_meta.cwd),
           preview = COALESCE(excluded.preview, sessions_meta.preview),
           model = COALESCE(excluded.model, sessions_meta.model),
           context_tokens = COALESCE(excluded.context_tokens, sessions_meta.context_tokens),
           context_window = COALESCE(excluded.context_window, sessions_meta.context_window),
           title = CASE
             WHEN excluded.title IS NULL THEN sessions_meta.title
             WHEN sessions_meta.title_source = 'custom' AND excluded.title_source != 'custom'
               THEN sessions_meta.title
             ELSE excluded.title END,
           title_source = CASE
             WHEN excluded.title IS NULL THEN sessions_meta.title_source
             WHEN sessions_meta.title_source = 'custom' AND excluded.title_source != 'custom'
               THEN sessions_meta.title_source
             ELSE excluded.title_source END",
        params![
            tool.as_str(),
            session_id,
            cwd,
            title_text,
            title_source,
            preview,
            model,
            context_tokens,
            context_window,
            now_ms(),
            source,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store(name: &str) -> Store {
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("store-{name}.db"));
        let _ = std::fs::remove_file(&db);
        Store::open(Some(&db)).unwrap()
    }

    fn key(id: &str) -> SessionKey {
        SessionKey {
            tool: ToolId::Claude,
            id: id.to_string(),
        }
    }

    #[test]
    fn custom_title_survives_later_auto_upsert() {
        let store = test_store("sticky-title");
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "s1",
            None,
            Some(("Mine", TitleKind::Custom)),
            None,
            None,
            None,
            None,
            "rename",
        )
        .unwrap();
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "s1",
            None,
            Some(("Auto guess", TitleKind::Auto)),
            None,
            None,
            None,
            None,
            "scan",
        )
        .unwrap();
        let meta = store.session_meta_map().unwrap();
        let stored = meta.get(&key("s1")).unwrap();
        assert_eq!(stored.title.as_deref(), Some("Mine"));
        assert_eq!(stored.title_source.as_deref(), Some("custom"));
        assert_eq!(stored.custom_title().as_deref(), Some("Mine"));
    }

    #[test]
    fn auto_title_is_replaced_by_later_auto_title() {
        let store = test_store("auto-title");
        for title in ["First", "Second"] {
            upsert_session_facts(
                store.conn(),
                ToolId::Claude,
                "s1",
                None,
                Some((title, TitleKind::Auto)),
                None,
                None,
                None,
                None,
                "scan",
            )
            .unwrap();
        }
        let meta = store.session_meta_map().unwrap();
        assert_eq!(
            meta.get(&key("s1")).unwrap().title.as_deref(),
            Some("Second")
        );
    }

    #[test]
    fn null_title_upsert_keeps_existing_title() {
        let store = test_store("null-title");
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "s1",
            None,
            Some(("Mine", TitleKind::Custom)),
            None,
            None,
            None,
            None,
            "rename",
        )
        .unwrap();
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "s1",
            Some("/tmp/repo"),
            None,
            Some("a prompt"),
            None,
            None,
            None,
            "scan",
        )
        .unwrap();
        let meta = store.session_meta_map().unwrap();
        let stored = meta.get(&key("s1")).unwrap();
        assert_eq!(stored.title.as_deref(), Some("Mine"));
        assert_eq!(stored.title_source.as_deref(), Some("custom"));
        assert_eq!(stored.preview.as_deref(), Some("a prompt"));
    }

    #[test]
    fn preview_is_truncated_before_storage() {
        let store = test_store("preview-cap");
        let long = "p".repeat(5000);
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "s1",
            None,
            None,
            Some(&long),
            None,
            None,
            None,
            "scan",
        )
        .unwrap();
        let meta = store.session_meta_map().unwrap();
        let stored = meta.get(&key("s1")).unwrap().preview.clone().unwrap();
        assert!(stored.chars().count() <= MAX_STORED_PREVIEW_CHARS);
    }

    #[test]
    fn delete_session_data_clears_all_three_tables() {
        let store = test_store("delete-audit");
        for id in ["s1", "s2"] {
            upsert_session_facts(
                store.conn(),
                ToolId::Claude,
                id,
                Some("/tmp/x"),
                None,
                None,
                None,
                None,
                None,
                "scan",
            )
            .unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO usage_cache (tool, session_id, file_path, byte_offset, mtime_ms, size)
                     VALUES ('claude', ?1, '/tmp/f', 0, 0, 0)",
                    params![id],
                )
                .unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO mcp_events (at, tool, session_id, kind, payload_json)
                     VALUES (1, 'claude', ?1, 'report_session_start', '{}')",
                    params![id],
                )
                .unwrap();
        }
        store.delete_session_data(ToolId::Claude, "s1").unwrap();
        for table in ["sessions_meta", "usage_cache", "mcp_events"] {
            let gone: i64 = store
                .conn()
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = 's1'"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let kept: i64 = store
                .conn()
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE session_id = 's2'"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(gone, 0, "{table}");
            assert_eq!(kept, 1, "{table}");
        }
    }

    #[test]
    fn store_files_and_created_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let base = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.join("store-private-dir");
        let _ = std::fs::remove_dir_all(&dir);
        let db = dir.join("private.db");
        let _store = Store::open(Some(&db)).unwrap();
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        for suffix in ["", "-wal", "-shm"] {
            let mut target = db.clone().into_os_string();
            target.push(suffix);
            let target = PathBuf::from(target);
            if target.exists() {
                let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "{}", target.display());
            }
        }
    }

    #[test]
    fn v2_database_is_migrated_without_losing_rows() {
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("store-migrate-v2.db");
        let _ = std::fs::remove_file(&db);
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions_meta (
                   tool TEXT NOT NULL,
                   session_id TEXT NOT NULL,
                   cwd TEXT,
                   title TEXT,
                   title_source TEXT,
                   preview TEXT,
                   model TEXT,
                   first_seen_at INTEGER NOT NULL,
                   completed_at INTEGER,
                   completed_by TEXT,
                   source TEXT NOT NULL DEFAULT 'scan',
                   PRIMARY KEY (tool, session_id)
                 );
                 INSERT INTO sessions_meta (tool, session_id, title, title_source, first_seen_at, completed_at)
                 VALUES ('claude', 'old-1', 'Kept title', 'custom', 5, 7);
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let store = Store::open(Some(&db)).unwrap();
        let meta = store.session_meta_map().unwrap();
        let stored = meta.get(&key("old-1")).unwrap();
        assert_eq!(stored.title.as_deref(), Some("Kept title"));
        assert_eq!(stored.completed_at, Some(7));
        assert!(stored.context_tokens.is_none());
        assert!(stored.last_seen_at.is_some());
        let version: i64 = store
            .conn()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, schema::SCHEMA_VERSION);
    }

    #[test]
    fn open_is_idempotent_on_a_current_database() {
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("store-reopen.db");
        let _ = std::fs::remove_file(&db);
        {
            let store = Store::open(Some(&db)).unwrap();
            store.mark_completed(ToolId::Claude, "s1", "tui").unwrap();
        }
        let store = Store::open(Some(&db)).unwrap();
        let meta = store.session_meta_map().unwrap();
        assert!(meta.get(&key("s1")).unwrap().completed_at.is_some());
    }

    fn legacy_db(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let conn = Connection::open(dir.join("mleko.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions_meta (
               tool TEXT NOT NULL,
               session_id TEXT NOT NULL,
               cwd TEXT,
               title TEXT,
               title_source TEXT,
               preview TEXT,
               model TEXT,
               context_tokens INTEGER,
               context_window INTEGER,
               first_seen_at INTEGER NOT NULL,
               completed_at INTEGER,
               completed_by TEXT,
               source TEXT NOT NULL DEFAULT 'scan',
               PRIMARY KEY (tool, session_id)
             );
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO sessions_meta (tool, session_id, title, title_source, preview, first_seen_at, completed_at)
             VALUES ('claude', 'legacy-1', 'Legacy title', 'custom', 'legacy preview', 1, 42);
             INSERT INTO meta VALUES ('context_window_override:m1', '100000');
             INSERT INTO meta VALUES ('contextXwindowXoverride:evil', '1');
             INSERT INTO meta VALUES ('claude_oauth_cache', 'secret');",
        )
        .unwrap();
    }

    #[test]
    fn legacy_merge_fills_gaps_and_keeps_local_values() {
        let base = PathBuf::from(".tmp/fixtures");
        let old_dir = base.join("legacy-mleko-src");
        let new_dir = base.join("legacy-mleko-dst");
        let _ = std::fs::remove_dir_all(&old_dir);
        let _ = std::fs::remove_dir_all(&new_dir);
        legacy_db(&old_dir);
        std::fs::create_dir_all(&new_dir).unwrap();
        let store = Store::open(Some(&new_dir.join("szpont.db"))).unwrap();
        upsert_session_facts(
            store.conn(),
            ToolId::Claude,
            "legacy-1",
            None,
            Some(("Local custom", TitleKind::Custom)),
            None,
            None,
            None,
            None,
            "rename",
        )
        .unwrap();
        migrate_legacy_mleko(store.conn(), &old_dir, &new_dir).unwrap();
        let meta = store.session_meta_map().unwrap();
        let stored = meta.get(&key("legacy-1")).unwrap();
        assert_eq!(stored.title.as_deref(), Some("Local custom"));
        assert_eq!(stored.completed_at, Some(42));
        assert_eq!(stored.preview.as_deref(), Some("legacy preview"));
        assert_eq!(
            store.meta_get("context_window_override:m1").unwrap(),
            Some("100000".to_string())
        );
        assert_eq!(
            store.meta_get("contextXwindowXoverride:evil").unwrap(),
            None
        );
        assert_eq!(store.meta_get("claude_oauth_cache").unwrap(), None);
        assert!(!old_dir.exists());
        assert!(new_dir.join("mleko-backup").join("mleko.db").exists());
        migrate_legacy_mleko(store.conn(), &old_dir, &new_dir).unwrap();
    }

    #[test]
    fn legacy_merge_runs_once() {
        let base = PathBuf::from(".tmp/fixtures");
        let old_dir = base.join("legacy-once-src");
        let new_dir = base.join("legacy-once-dst");
        let _ = std::fs::remove_dir_all(&old_dir);
        let _ = std::fs::remove_dir_all(&new_dir);
        legacy_db(&old_dir);
        std::fs::create_dir_all(&new_dir).unwrap();
        let store = Store::open(Some(&new_dir.join("szpont.db"))).unwrap();
        store.meta_set("legacy_mleko_migrated", "1").unwrap();
        migrate_legacy_mleko(store.conn(), &old_dir, &new_dir).unwrap();
        let meta = store.session_meta_map().unwrap();
        assert!(meta.is_empty());
        assert!(old_dir.exists());
    }
}
