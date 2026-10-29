pub mod repo;
pub mod snapshot;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const BUCKET_RETENTION_SECS: i64 = 8 * 24 * 3600;
pub const BUCKET_FUTURE_SLACK_MS: i64 = 24 * 3600 * 1000;
pub const TAIL_SCAN_BYTES: u64 = 64 * 1024;
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
pub const MAX_CREDENTIALS_BYTES: u64 = 64 * 1024;
pub const MAX_SESSION_ID_CHARS: usize = 128;
pub const MAX_TEXT_FACT_CHARS: usize = 512;
pub const MAX_PREVIEW_CHARS: usize = 200;
pub const MAX_MODEL_CHARS: usize = 128;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn system_time_ms(t: Option<SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn valid_session_id(id: &str, required_prefix: Option<&str>) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_SESSION_ID_CHARS
        && !id.starts_with('-')
        && required_prefix.is_none_or(|prefix| id.starts_with(prefix))
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn tail_lines(path: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_SCAN_BYTES)))
        .ok()?;
    let mut buffer = Vec::new();
    file.take(TAIL_SCAN_BYTES).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

pub fn tail_find_last<T, R>(
    path: &std::path::Path,
    marker: &str,
    pick: impl Fn(T) -> Option<R>,
) -> Option<R>
where
    T: serde::de::DeserializeOwned,
{
    let buffer = tail_lines(path)?;
    for line in buffer.lines().rev() {
        if !line.contains(marker) {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<T>(line) else {
            continue;
        };
        if let Some(found) = pick(parsed) {
            return Some(found);
        }
    }
    None
}

pub fn restrict_permissions(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = std::fs::metadata(path) {
        let mut perms = metadata.permissions();
        if perms.mode() & 0o777 != mode {
            perms.set_mode(mode);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

pub fn fallback_cwd() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

pub fn format_tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1_000.0),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1_000_000.0),
        _ => format!("{:.1}B", n as f64 / 1_000_000_000.0),
    }
}

pub fn format_age(ms: i64) -> String {
    let secs = ms / 1000;
    match secs {
        i64::MIN..=59 => "now".to_string(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

pub fn fuzzy_score(query: &str, haystack: &str) -> Option<i64> {
    let hay: Vec<char> = haystack.to_lowercase().chars().collect();
    let mut total = 0i64;
    for term in query.to_lowercase().split_whitespace() {
        total += fuzzy_term_score(term, &hay)?;
    }
    Some(total)
}

fn fuzzy_term_score(term: &str, hay: &[char]) -> Option<i64> {
    let mut score = 0i64;
    let mut position = 0usize;
    let mut last_match: Option<usize> = None;
    for needle_char in term.chars() {
        let found = hay[position..]
            .iter()
            .position(|&c| c == needle_char)
            .map(|offset| position + offset)?;
        score += match last_match {
            Some(prev) if found == prev + 1 => 3,
            _ => 1,
        };
        if found == 0 || !hay[found - 1].is_alphanumeric() {
            score += 2;
        }
        last_match = Some(found);
        position = found + 1;
    }
    Some(score)
}

pub fn read_to_string_capped(path: &std::path::Path, cap: u64) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > cap {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

pub fn sanitize_for_terminal(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control()
                || matches!(
                    c,
                    '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
                )
            {
                '·'
            } else {
                c
            }
        })
        .collect()
}

pub fn clean_text_fact(s: &str, max: usize) -> String {
    sanitize_for_terminal(&truncate(s, max))
}

pub struct CappedLine {
    pub bytes: Vec<u8>,
    pub consumed: u64,
    pub complete: bool,
    pub truncated: bool,
}

pub fn read_line_capped<R: std::io::BufRead>(
    reader: &mut R,
    cap: usize,
) -> std::io::Result<CappedLine> {
    let mut bytes = Vec::new();
    let mut consumed = 0u64;
    let mut truncated = false;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(CappedLine {
                bytes,
                consumed,
                complete: false,
                truncated,
            });
        }
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let room = cap.saturating_sub(bytes.len());
            if pos > room {
                truncated = true;
            }
            bytes.extend_from_slice(&buf[..pos.min(room)]);
            consumed += (pos + 1) as u64;
            reader.consume(pos + 1);
            return Ok(CappedLine {
                bytes,
                consumed,
                complete: true,
                truncated,
            });
        }
        let len = buf.len();
        let room = cap.saturating_sub(bytes.len());
        if len > room {
            truncated = true;
        }
        bytes.extend_from_slice(&buf[..len.min(room)]);
        consumed += len as u64;
        reader.consume(len);
    }
}

pub fn parse_token_count(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (number, multiplier) = match s.chars().last() {
        Some('k' | 'K') => (&s[..s.len() - 1], 1_000.0),
        Some('m' | 'M') => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    let value: f64 = number.trim().parse().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let tokens = (value * multiplier).round();
    if tokens < 1.0 {
        return None;
    }
    Some(tokens as u64)
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.char_indices().nth(max).is_none() {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolId {
    Claude,
    Codex,
    Kimi,
}

impl ToolId {
    pub fn parse(s: &str) -> anyhow::Result<ToolId> {
        match s {
            "claude" => Ok(ToolId::Claude),
            "codex" => Ok(ToolId::Codex),
            "kimi" => Ok(ToolId::Kimi),
            other => anyhow::bail!("unknown tool {other:?}, expected claude, codex or kimi"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ToolId::Claude => "claude",
            ToolId::Codex => "codex",
            ToolId::Kimi => "kimi",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ToolId::Claude => "Claude Code",
            ToolId::Codex => "Codex",
            ToolId::Kimi => "Kimi Code",
        }
    }

    pub fn env_key(self) -> &'static str {
        match self {
            ToolId::Claude => "CLAUDE_CONFIG_DIR",
            ToolId::Codex => "CODEX_HOME",
            ToolId::Kimi => "KIMI_CODE_HOME",
        }
    }

    pub fn default_dir(self) -> &'static str {
        match self {
            ToolId::Claude => ".claude",
            ToolId::Codex => ".codex",
            ToolId::Kimi => ".kimi-code",
        }
    }

    pub fn home(self) -> Option<PathBuf> {
        std::env::var_os(self.env_key())
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(self.default_dir())))
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)]
pub struct SessionKey {
    pub tool: ToolId,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct SessionSummary {
    pub key: SessionKey,
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub model: Option<String>,
    pub origin_url: Option<String>,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: i64,
    pub native_archived: bool,
    pub native_tokens_used: Option<u64>,
    pub usage_files: Vec<PathBuf>,
}

#[derive(Clone, Copy, Default, Debug, Serialize)]
pub struct Usage {
    pub input_uncached: u64,
    pub input_cache_read: u64,
    pub input_cache_write: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input_uncached
            .saturating_add(self.input_cache_read)
            .saturating_add(self.input_cache_write)
            .saturating_add(self.output)
    }

    pub fn add(&mut self, other: &Usage) {
        self.input_uncached = self.input_uncached.saturating_add(other.input_uncached);
        self.input_cache_read = self.input_cache_read.saturating_add(other.input_cache_read);
        self.input_cache_write = self
            .input_cache_write
            .saturating_add(other.input_cache_write);
        self.output = self.output.saturating_add(other.output);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Running,
    WaitingForInput,
    Open,
    Idle,
}

#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TitleKind {
    Custom,
    Auto,
}

impl TitleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TitleKind::Custom => "custom",
            TitleKind::Auto => "auto",
        }
    }
}

#[derive(Default, Debug)]
pub struct Enrichment {
    pub usage: Option<Usage>,
    pub cwd: Option<PathBuf>,
    pub title: Option<(String, TitleKind)>,
    pub preview: Option<String>,
    pub model: Option<String>,
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_line_capped_reports_truncation_and_full_consumption() {
        let line = format!("{}\n", "x".repeat(20));
        let mut reader = std::io::BufReader::new(line.as_bytes());
        let capped = read_line_capped(&mut reader, 8).unwrap();
        assert!(capped.truncated);
        assert!(capped.complete);
        assert_eq!(capped.bytes.len(), 8);
        assert_eq!(capped.consumed, line.len() as u64);
    }

    #[test]
    fn read_line_capped_line_at_cap_is_not_truncated() {
        let mut reader = std::io::BufReader::new(&b"abcd\n"[..]);
        let capped = read_line_capped(&mut reader, 4).unwrap();
        assert!(!capped.truncated);
        assert!(capped.complete);
        assert_eq!(capped.bytes, b"abcd");
        assert_eq!(capped.consumed, 5);
    }

    #[test]
    fn read_line_capped_eof_without_newline_is_incomplete() {
        let mut reader = std::io::BufReader::new(&b"partial"[..]);
        let capped = read_line_capped(&mut reader, 64).unwrap();
        assert!(!capped.complete);
        assert_eq!(capped.bytes, b"partial");
        let mut empty = std::io::BufReader::new(&b""[..]);
        let capped = read_line_capped(&mut empty, 64).unwrap();
        assert!(!capped.complete);
        assert_eq!(capped.bytes, Vec::<u8>::new());
    }

    #[test]
    fn sanitize_replaces_control_and_bidi_characters() {
        assert_eq!(sanitize_for_terminal("a\x1b[31mb"), "a·[31mb");
        assert_eq!(sanitize_for_terminal("a\u{202E}b\u{2066}c"), "a·b·c");
        assert_eq!(sanitize_for_terminal("plain ünïcode"), "plain ünïcode");
    }

    #[test]
    fn truncate_keeps_short_strings_and_caps_long_ones() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abcd", 3), "ab…");
        assert_eq!(truncate("", 3), "");
        assert_eq!(truncate("ééééé", 4), "ééé…");
    }

    #[test]
    fn session_id_validation_supports_optional_prefix_and_length_cap() {
        assert!(valid_session_id("abc-123", None));
        assert!(valid_session_id("session_abc", Some("session_")));
        assert!(!valid_session_id("abc", Some("session_")));
        assert!(!valid_session_id("", None));
        assert!(!valid_session_id("-rf", None));
        assert!(!valid_session_id("a b", None));
        assert!(!valid_session_id(
            &"x".repeat(MAX_SESSION_ID_CHARS + 1),
            None
        ));
    }
}
