use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use super::{OpenSessionIndex, ToolAdapter};
use crate::core::{Enrichment, LaunchSpec, Liveness, SessionKey, SessionSummary, ToolId, Usage};
use crate::store::checkpoints::CheckpointTxn;

pub struct CodexAdapter {
    root: Option<PathBuf>,
    open_index: OpenSessionIndex,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            root: ToolId::Codex.home(),
            open_index: OpenSessionIndex::new("codex"),
        }
    }
}

impl ToolAdapter for CodexAdapter {
    fn id(&self) -> ToolId {
        ToolId::Codex
    }

    fn is_installed(&self) -> bool {
        self.root
            .as_ref()
            .is_some_and(|r| r.join("sessions").exists() || sessions_db(r).is_some())
    }

    fn store_roots(&self) -> Vec<PathBuf> {
        self.root.iter().cloned().collect()
    }

    fn discover(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        let Some(db_path) = sessions_db(root) else {
            if root.join("sessions").exists() {
                anyhow::bail!(
                    "no state_<N>.sqlite thread store in {} (codex too old for szpont?)",
                    root.display()
                );
            }
            return Ok(Vec::new());
        };
        let conn = open_read_only(&db_path)?;
        let sessions = query_threads(&conn)?;
        self.open_index.reindex(&sessions);
        Ok(sessions)
    }

    fn refresh_usage(
        &self,
        session: &SessionSummary,
        _ckpt: &mut CheckpointTxn,
    ) -> anyhow::Result<Enrichment> {
        let mut enrich = Enrichment::default();
        let Some(rollout) = session.usage_files.first() else {
            return Ok(enrich);
        };
        let Some(info) = tail_token_count(rollout) else {
            return Ok(enrich);
        };
        if let Some(total) = &info.total_token_usage {
            let input = total.input_tokens.unwrap_or(0);
            let cached = total.cached_input_tokens.unwrap_or(0).min(input);
            enrich.usage = Some(Usage {
                input_uncached: input - cached,
                input_cache_read: cached,
                input_cache_write: total.cache_write_input_tokens.unwrap_or(0),
                output: total.output_tokens.unwrap_or(0),
                reasoning: total.reasoning_output_tokens.unwrap_or(0),
            });
        }
        enrich.context_tokens = info.last_token_usage.as_ref().and_then(|last| {
            last.total_tokens.or_else(|| {
                Some(
                    last.input_tokens
                        .unwrap_or(0)
                        .saturating_add(last.output_tokens.unwrap_or(0)),
                )
            })
        });
        enrich.context_window = info.model_context_window;
        Ok(enrich)
    }

    fn delete_session(&self, session: &SessionSummary) -> anyhow::Result<()> {
        run_codex(&delete_args(&session.key.id)?)
    }

    fn unarchive_session(&self, session: &SessionSummary) -> anyhow::Result<()> {
        run_codex(&unarchive_args(&session.key.id)?)
    }

    fn resume_needs_unarchive(&self) -> bool {
        true
    }

    fn liveness(&self, session: &SessionSummary) -> Liveness {
        let recently_touched = session
            .usage_files
            .first()
            .and_then(|p| p.metadata().ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < super::RUNNING_WINDOW);
        if recently_touched {
            return Liveness::Running;
        }
        self.open_index.liveness(session)
    }

    fn resume_command(&self, session: &SessionSummary) -> LaunchSpec {
        LaunchSpec {
            program: "codex".to_string(),
            args: vec!["resume".to_string(), session.key.id.clone()],
            cwd: session
                .cwd
                .clone()
                .unwrap_or_else(crate::core::fallback_cwd),
        }
    }

    fn new_session_command(&self, cwd: &Path) -> LaunchSpec {
        LaunchSpec {
            program: "codex".to_string(),
            args: Vec::new(),
            cwd: cwd.to_path_buf(),
        }
    }
}

fn run_codex(args: &[String]) -> anyhow::Result<()> {
    let subcommand = args.first().map_or("", String::as_str);
    let output = std::process::Command::new("codex").args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("codex {subcommand} failed: {}", stderr.trim());
    }
    Ok(())
}

fn delete_args(session_id: &str) -> anyhow::Result<Vec<String>> {
    codex_args("delete", &["--force"], session_id)
}

fn unarchive_args(session_id: &str) -> anyhow::Result<Vec<String>> {
    codex_args("unarchive", &[], session_id)
}

fn codex_args(subcommand: &str, flags: &[&str], session_id: &str) -> anyhow::Result<Vec<String>> {
    if !crate::core::valid_session_id(session_id, None) {
        anyhow::bail!("unexpected codex session id {session_id:?}");
    }
    let mut args = vec![subcommand.to_string()];
    args.extend(flags.iter().map(|flag| (*flag).to_string()));
    args.push(session_id.to_string());
    Ok(args)
}

fn sessions_db(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .filter_map(std::result::Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n: u32 = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

fn open_read_only(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(2))?;
    Ok(conn)
}

const THREAD_COLUMNS: [&str; 13] = [
    "id",
    "rollout_path",
    "cwd",
    "title",
    "preview",
    "first_user_message",
    "tokens_used",
    "model",
    "created_at_ms",
    "updated_at_ms",
    "recency_at_ms",
    "archived",
    "git_origin_url",
];

fn available_thread_columns(
    conn: &Connection,
) -> anyhow::Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?;
    Ok(names.filter_map(std::result::Result::ok).collect())
}

fn query_threads(conn: &Connection) -> anyhow::Result<Vec<SessionSummary>> {
    let available = available_thread_columns(conn)?;
    let selected: Vec<String> = THREAD_COLUMNS
        .iter()
        .map(|c| {
            if available.contains(*c) {
                (*c).to_string()
            } else {
                format!("NULL AS {c}")
            }
        })
        .collect();
    let column_list = selected.join(", ");
    let sql_user_threads =
        format!("select {column_list} from threads where thread_source = 'user'");
    let sql_all = format!("select {column_list} from threads");
    let mut stmt = match conn.prepare(&sql_user_threads) {
        Ok(stmt) => stmt,
        Err(_) => conn.prepare(&sql_all)?,
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get("id")?;
        let rollout_path: Option<String> = row.get("rollout_path")?;
        let cwd: Option<String> = row.get("cwd")?;
        let title: Option<String> = row.get("title")?;
        let preview: Option<String> = row.get("preview")?;
        let first_user_message: Option<String> = row.get("first_user_message")?;
        let tokens_used: Option<i64> = row.get("tokens_used")?;
        let model: Option<String> = row.get("model")?;
        let created_at_ms: Option<i64> = row.get("created_at_ms")?;
        let updated_at_ms: Option<i64> = row.get("updated_at_ms")?;
        let recency_at_ms: Option<i64> = row.get("recency_at_ms")?;
        let archived: Option<i64> = row.get("archived")?;
        let git_origin_url: Option<String> = row.get("git_origin_url")?;
        let preview = preview
            .or(first_user_message)
            .map(|p| crate::core::clean_text_fact(&p, crate::core::MAX_PREVIEW_CHARS));
        Ok(SessionSummary {
            key: SessionKey {
                tool: ToolId::Codex,
                id,
            },
            cwd: cwd.map(PathBuf::from),
            title: title
                .filter(|t| !t.is_empty())
                .map(|t| crate::core::clean_text_fact(&t, crate::core::MAX_TEXT_FACT_CHARS))
                .or_else(|| preview.clone()),
            preview,
            model: model.map(|m| crate::core::clean_text_fact(&m, crate::core::MAX_MODEL_CHARS)),
            origin_url: git_origin_url,
            created_at_ms,
            updated_at_ms: recency_at_ms
                .or(updated_at_ms)
                .or(created_at_ms)
                .unwrap_or(0),
            native_archived: archived.unwrap_or(0) != 0,
            native_tokens_used: tokens_used.map(|t| t.max(0) as u64),
            usage_files: rollout_path.map(PathBuf::from).into_iter().collect(),
        })
    })?;
    Ok(rows
        .filter_map(std::result::Result::ok)
        .filter(|s| crate::core::valid_session_id(&s.key.id, None))
        .collect())
}

#[derive(Deserialize)]
struct RolloutLine {
    payload: Option<RolloutPayload>,
}

#[derive(Deserialize)]
struct RolloutPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    info: Option<TokenCountInfo>,
}

#[derive(Deserialize)]
struct TokenCountInfo {
    total_token_usage: Option<TokenTotals>,
    last_token_usage: Option<TokenTotals>,
    model_context_window: Option<u64>,
}

#[derive(Deserialize)]
struct TokenTotals {
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_write_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    reasoning_output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

fn tail_token_count(rollout: &Path) -> Option<TokenCountInfo> {
    crate::core::tail_find_last(rollout, "\"token_count\"", |parsed: RolloutLine| {
        let payload = parsed.payload?;
        if payload.kind.as_deref() != Some("token_count") {
            return None;
        }
        payload.info
    })
}

#[cfg(test)]
mod tests {
    use crate::core::valid_session_id;

    #[test]
    fn codex_subcommands_refuse_an_unsafe_session_id() {
        let err = super::unarchive_args("-rf").unwrap_err();
        assert!(
            err.to_string().contains("unexpected codex session id"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn delete_is_forced_because_codex_cannot_prompt_without_a_terminal() {
        let id = "019fa33b-1ff5-7110-b9a4-82c5a1a86484";
        assert_eq!(
            super::delete_args(id).unwrap(),
            vec!["delete".to_string(), "--force".to_string(), id.to_string()]
        );
    }

    #[test]
    fn unarchive_passes_only_the_session_id() {
        let id = "019fa33b-1ff5-7110-b9a4-82c5a1a86484";
        assert_eq!(
            super::unarchive_args(id).unwrap(),
            vec!["unarchive".to_string(), id.to_string()]
        );
    }

    #[test]
    fn session_id_validation_rejects_unsafe_ids() {
        assert!(valid_session_id(
            "0198c5c2-abcd-7de0-89ab-0123456789ab",
            None
        ));
        assert!(valid_session_id("thread_123", None));
        assert!(!valid_session_id("", None));
        assert!(!valid_session_id("-rf", None));
        assert!(!valid_session_id("a b", None));
        assert!(!valid_session_id("a;rm", None));
        assert!(!valid_session_id("a/../b", None));
    }
}
