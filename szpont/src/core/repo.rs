use std::path::{Path, PathBuf};
use std::process::Command;

use crate::core::snapshot::SessionRow;

#[derive(Clone, Debug)]
pub struct RepoContext {
    pub root: PathBuf,
    pub name: String,
    pub worktree_roots: Vec<PathBuf>,
    pub origin_url: Option<String>,
    pub is_git: bool,
}

pub fn detect(start: &Path) -> Option<RepoContext> {
    let start = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let root = find_git_root(&start)?;
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    Some(RepoContext {
        worktree_roots: worktree_list(&root),
        origin_url: origin_url(&root).map(|u| normalize_origin(&u)),
        root,
        name,
        is_git: true,
    })
}

pub fn plain_directory_context(path: &Path) -> RepoContext {
    let root = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    RepoContext {
        name: root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        worktree_roots: Vec::new(),
        origin_url: None,
        root,
        is_git: false,
    }
}

impl RepoContext {
    pub fn matches(&self, row: &SessionRow) -> bool {
        if let Some(cwd) = row.session.cwd.as_deref() {
            if cwd.starts_with(&self.root) {
                return true;
            }
            if self.worktree_roots.iter().any(|w| cwd.starts_with(w)) {
                return true;
            }
        }
        if let (Some(mine), Some(theirs)) = (&self.origin_url, &row.session.origin_url)
            && *mine == normalize_origin(theirs)
        {
            return true;
        }
        false
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn worktree_list(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["worktree", "list", "--porcelain"])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

fn origin_url(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}

pub fn normalize_origin(url: &str) -> String {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
        .unwrap_or(url);
    let url = strip_userinfo(url);
    url.replacen(':', "/", 1).to_lowercase()
}

fn strip_userinfo(url: &str) -> &str {
    let authority_end = url.find('/').unwrap_or(url.len());
    match url[..authority_end].rfind('@') {
        Some(at) => &url[at + 1..],
        None => url,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_origin;

    #[test]
    fn origin_forms_normalize_to_same_key() {
        let a = normalize_origin("git@github.com:software-mansion/react-native-reanimated.git");
        let b = normalize_origin("https://github.com/software-mansion/react-native-reanimated");
        assert_eq!(a, b);
    }

    #[test]
    fn embedded_credentials_are_stripped() {
        assert_eq!(
            normalize_origin("https://user:ghp_secret@github.com/org/repo.git"),
            "github.com/org/repo"
        );
        assert_eq!(
            normalize_origin("https://user:ghp_secret@github.com/org/repo.git"),
            normalize_origin("https://github.com/org/repo")
        );
        assert_eq!(
            normalize_origin("ssh://user@host.xz/org/repo"),
            "host.xz/org/repo"
        );
    }

    #[test]
    fn a_directory_without_git_still_yields_a_scopeable_context() {
        let dir = std::path::PathBuf::from(".tmp/fixtures/umbrella/child");
        std::fs::create_dir_all(&dir).unwrap();
        let umbrella = dir.parent().unwrap();
        let ctx = super::plain_directory_context(umbrella);
        assert!(!ctx.is_git);
        assert_eq!(ctx.name, "umbrella");
        let row = crate::core::snapshot::SessionRow {
            project_alias: None,
            project: None,
            demo_archived: false,
            session: crate::core::SessionSummary {
                key: crate::core::SessionKey {
                    tool: crate::core::ToolId::Claude,
                    id: "s1".to_string(),
                },
                cwd: Some(dir.canonicalize().unwrap()),
                title: None,
                preview: None,
                model: None,
                updated_at_ms: 0,
                native_tokens_used: None,
                native_archived: false,
                usage_files: Vec::new(),
                created_at_ms: None,
                origin_url: None,
            },
            liveness: crate::core::Liveness::Idle,
            completed: false,
            completed_at: None,
            usage: None,
            context_tokens: None,
            context_window: None,
        };
        assert!(
            ctx.matches(&row),
            "a session in a child directory must fall inside the umbrella scope"
        );
    }
}
