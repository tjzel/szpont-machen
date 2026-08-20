mod claude;
mod codex;
mod kimi;

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::core::{Enrichment, LaunchSpec, Liveness, SessionSummary, ToolId};
use crate::store::checkpoints::CheckpointTxn;

pub(crate) const RUNNING_WINDOW: Duration = Duration::from_mins(1);

pub(crate) fn recently_modified(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < RUNNING_WINDOW)
}

pub trait ToolAdapter {
    fn id(&self) -> ToolId;
    fn is_installed(&self) -> bool;
    fn store_roots(&self) -> Vec<PathBuf>;
    fn watch_paths(&self) -> Vec<PathBuf> {
        self.store_roots()
            .into_iter()
            .map(|root| root.join("sessions"))
            .collect()
    }
    fn discover(&self) -> anyhow::Result<Vec<SessionSummary>>;
    fn refresh_usage(
        &self,
        _session: &SessionSummary,
        _ckpt: &mut CheckpointTxn,
    ) -> anyhow::Result<Enrichment> {
        Ok(Enrichment::default())
    }
    fn probe_context(&self, _session: &SessionSummary) -> Option<(u64, Option<u64>)> {
        None
    }
    fn delete_session(&self, session: &SessionSummary) -> anyhow::Result<()> {
        anyhow::bail!(
            "deleting {} sessions is not supported",
            session.key.tool.display_name()
        )
    }
    fn unarchive_session(&self, session: &SessionSummary) -> anyhow::Result<()> {
        anyhow::bail!(
            "this session is archived inside {} itself — unarchive it there",
            session.key.tool.display_name()
        )
    }
    fn resume_needs_unarchive(&self) -> bool {
        false
    }
    fn liveness(&self, session: &SessionSummary) -> Liveness;
    fn resume_command(&self, session: &SessionSummary) -> LaunchSpec;
    fn new_session_command(&self, cwd: &Path) -> LaunchSpec;
}

pub(crate) fn system_program(candidates: &[&'static str], fallback: &'static str) -> &'static str {
    candidates
        .iter()
        .copied()
        .find(|p| Path::new(p).is_file())
        .unwrap_or(fallback)
}

pub(crate) fn program_on_path(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        std::fs::metadata(dir.join(program))
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

pub(crate) struct OpenSessionIndex {
    process: &'static str,
    open_cwds: OnceCell<Vec<PathBuf>>,
    newest_by_cwd: RefCell<HashMap<PathBuf, (i64, String)>>,
}

impl OpenSessionIndex {
    pub(crate) fn new(process: &'static str) -> OpenSessionIndex {
        OpenSessionIndex {
            process,
            open_cwds: OnceCell::new(),
            newest_by_cwd: RefCell::new(HashMap::new()),
        }
    }

    pub(crate) fn reindex(&self, sessions: &[SessionSummary]) {
        let mut newest = self.newest_by_cwd.borrow_mut();
        newest.clear();
        for session in sessions {
            if let Some(cwd) = &session.cwd {
                let entry = newest
                    .entry(cwd.clone())
                    .or_insert((session.updated_at_ms, session.key.id.clone()));
                if session.updated_at_ms > entry.0 {
                    *entry = (session.updated_at_ms, session.key.id.clone());
                }
            }
        }
    }

    pub(crate) fn liveness(&self, session: &SessionSummary) -> Liveness {
        let Some(cwd) = session.cwd.as_deref() else {
            return Liveness::Idle;
        };
        let open_cwds = self.open_cwds.get_or_init(|| process_cwds(self.process));
        if open_cwds.iter().any(|open| open == cwd) {
            let newest = self.newest_by_cwd.borrow();
            if newest.get(cwd).is_some_and(|(_, id)| *id == session.key.id) {
                return Liveness::Open;
            }
        }
        Liveness::Idle
    }
}

pub(crate) fn process_cwds(command: &str) -> Vec<PathBuf> {
    static LSOF_WARNED: std::sync::Once = std::sync::Once::new();
    let lsof = system_program(&["/usr/sbin/lsof", "/usr/bin/lsof"], "lsof");
    let output = std::process::Command::new(lsof)
        .args(["-a", "-c", command, "-d", "cwd", "-Fn"])
        .output();
    let Ok(output) = output else {
        LSOF_WARNED.call_once(|| {
            crate::logging::warn(
                "lsof is unavailable; open/idle detection for codex and kimi is degraded",
            );
        });
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .collect()
}

pub fn all() -> Vec<Box<dyn ToolAdapter>> {
    vec![
        Box::new(claude::ClaudeAdapter::new()),
        Box::new(codex::CodexAdapter::new()),
        Box::new(kimi::KimiAdapter::new()),
    ]
}

pub fn by_id(tool: ToolId) -> Option<Box<dyn ToolAdapter>> {
    all().into_iter().find(|a| a.id() == tool)
}

pub fn installed() -> Vec<Box<dyn ToolAdapter>> {
    all().into_iter().filter(|a| a.is_installed()).collect()
}
