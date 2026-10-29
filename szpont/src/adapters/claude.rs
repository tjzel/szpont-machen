use std::cell::OnceCell;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::ToolAdapter;
use crate::core::{
    Enrichment, LaunchSpec, Liveness, SessionKey, SessionSummary, TitleKind, ToolId, Usage, now_ms,
    system_time_ms,
};
use crate::store::checkpoints::{CheckpointTxn, FileCheckpoint};

const DEDUP_RING_CAPACITY: usize = 64;

pub struct ClaudeAdapter {
    root: Option<PathBuf>,
    live: OnceCell<HashMap<String, Liveness>>,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        Self {
            root: ToolId::Claude.home(),
            live: OnceCell::new(),
        }
    }

    fn live_map(&self) -> &HashMap<String, Liveness> {
        self.live.get_or_init(|| {
            self.root
                .as_deref()
                .map(read_live_sessions)
                .unwrap_or_default()
        })
    }
}

impl ToolAdapter for ClaudeAdapter {
    fn id(&self) -> ToolId {
        ToolId::Claude
    }

    fn is_installed(&self) -> bool {
        self.root
            .as_ref()
            .is_some_and(|r| r.join("projects").exists())
    }

    fn store_roots(&self) -> Vec<PathBuf> {
        self.root.iter().cloned().collect()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        self.root
            .iter()
            .flat_map(|root| [root.join("projects"), root.join("sessions")])
            .collect()
    }

    fn discover(&self) -> anyhow::Result<Vec<SessionSummary>> {
        let Some(root) = &self.root else {
            return Ok(Vec::new());
        };
        let projects = root.join("projects");
        let mut sessions = Vec::new();
        for project in std::fs::read_dir(&projects)? {
            let Ok(project) = project else { continue };
            if !project.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            for entry in std::fs::read_dir(project.path()).into_iter().flatten() {
                let Ok(entry) = entry else { continue };
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if !is_uuid_like(stem) {
                    continue;
                }
                let Ok(meta) = entry.metadata() else { continue };
                let updated_at_ms = system_time_ms(meta.modified().ok());
                let created_at_ms = meta.created().ok().map(|t| system_time_ms(Some(t)));
                let mut usage_files = vec![path.clone()];
                let subagents = path.with_extension("").join("subagents");
                for sub in std::fs::read_dir(&subagents)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let sub_path = sub.path();
                    if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        usage_files.push(sub_path);
                    }
                }
                sessions.push(SessionSummary {
                    key: SessionKey {
                        tool: ToolId::Claude,
                        id: stem.to_string(),
                    },
                    cwd: None,
                    title: None,
                    preview: None,
                    model: None,
                    origin_url: None,
                    created_at_ms,
                    updated_at_ms,
                    native_archived: false,
                    native_tokens_used: None,
                    usage_files,
                });
            }
        }
        Ok(sessions)
    }

    fn refresh_usage(
        &self,
        session: &SessionSummary,
        ckpt: &mut CheckpointTxn,
    ) -> anyhow::Result<Enrichment> {
        let mut enrich = Enrichment::default();
        let mut total = Usage::default();
        let mut buckets: HashMap<i64, Usage> = HashMap::new();
        let bucket_cutoff_ms = now_ms() - crate::core::BUCKET_RETENTION_SECS * 1000;
        for file in &session.usage_files {
            let Ok(meta) = std::fs::metadata(file) else {
                continue;
            };
            let mtime_ms = system_time_ms(meta.modified().ok());
            let size = meta.len();
            let prior = ckpt.get(ToolId::Claude, &session.key.id, file)?;
            if let Some(p) = &prior
                && p.mtime_ms == mtime_ms
                && p.size == size
            {
                total.add(&p.usage);
                continue;
            }
            let (start_offset, mut usage, mut ring) = match prior {
                Some(p) if p.byte_offset <= size => {
                    (p.byte_offset, p.usage, load_ring(p.dedup_state.as_deref()))
                }
                _ => (0, Usage::default(), VecDeque::new()),
            };
            let track_context = session.usage_files.first() == Some(file);
            let consumed = parse_claude_file(
                file,
                start_offset,
                &mut usage,
                &mut ring,
                &mut enrich,
                &mut buckets,
                bucket_cutoff_ms,
                track_context,
            )?;
            total.add(&usage);
            ckpt.put(
                ToolId::Claude,
                &session.key.id,
                file,
                &FileCheckpoint {
                    byte_offset: consumed,
                    mtime_ms,
                    size,
                    usage,
                    dedup_state: Some(save_ring(&ring)),
                },
            )?;
        }
        for (hour_ts, usage) in &buckets {
            ckpt.add_bucket(ToolId::Claude, *hour_ts, usage)?;
        }
        enrich.usage = Some(total);
        Ok(enrich)
    }

    fn probe_context(&self, session: &SessionSummary) -> Option<(u64, Option<u64>)> {
        let main = session.usage_files.first()?;
        tail_last_context(main).map(|tokens| (tokens, None))
    }

    fn delete_session(&self, session: &SessionSummary) -> anyhow::Result<()> {
        let main = session
            .usage_files
            .first()
            .ok_or_else(|| anyhow::anyhow!("session transcript path is unknown"))?;
        remove_file_if_exists(main)?;
        let side_dir = main.with_extension("");
        if side_dir.is_dir() {
            std::fs::remove_dir_all(&side_dir)?;
        }
        Ok(())
    }

    fn liveness(&self, session: &SessionSummary) -> Liveness {
        if let Some(liveness) = self.live_map().get(&session.key.id) {
            return *liveness;
        }
        let recently_touched = session
            .usage_files
            .first()
            .and_then(|p| p.metadata().ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < super::RUNNING_WINDOW);
        if recently_touched {
            Liveness::Running
        } else {
            Liveness::Idle
        }
    }

    fn resume_command(&self, session: &SessionSummary) -> LaunchSpec {
        LaunchSpec {
            program: "claude".to_string(),
            args: vec!["--resume".to_string(), session.key.id.clone()],
            cwd: session
                .cwd
                .clone()
                .unwrap_or_else(crate::core::fallback_cwd),
        }
    }

    fn new_session_command(&self, cwd: &Path) -> LaunchSpec {
        LaunchSpec {
            program: "claude".to_string(),
            args: Vec::new(),
            cwd: cwd.to_path_buf(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeLine {
    cwd: Option<String>,
    timestamp: Option<String>,
    request_id: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    last_prompt: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

fn parse_claude_file(
    path: &Path,
    start_offset: u64,
    usage: &mut Usage,
    ring: &mut VecDeque<String>,
    enrich: &mut Enrichment,
    buckets: &mut HashMap<i64, Usage>,
    bucket_cutoff_ms: i64,
    track_context: bool,
) -> anyhow::Result<u64> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start_offset))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut consumed = start_offset;
    let bucket_ceiling_ms = now_ms() + crate::core::BUCKET_FUTURE_SLACK_MS;
    loop {
        let capped = crate::core::read_line_capped(&mut reader, crate::core::MAX_LINE_BYTES)?;
        if !capped.complete {
            break;
        }
        consumed += capped.consumed;
        if capped.truncated {
            continue;
        }
        let line = String::from_utf8_lossy(&capped.bytes);
        let interesting = line.contains("\"usage\"")
            || line.contains("\"aiTitle\"")
            || line.contains("\"customTitle\"")
            || line.contains("\"lastPrompt\"")
            || (track_context && line.contains("\"cwd\""));
        if !interesting {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<ClaudeLine>(&line) else {
            continue;
        };
        if track_context && let Some(cwd) = parsed.cwd {
            enrich.cwd = Some(PathBuf::from(cwd));
        }
        if let Some(title) = parsed.custom_title {
            enrich.title = Some((
                crate::core::clean_text_fact(&title, crate::core::MAX_TEXT_FACT_CHARS),
                TitleKind::Custom,
            ));
        } else if let Some(title) = parsed.ai_title
            && enrich.title.as_ref().map(|(_, k)| *k) != Some(TitleKind::Custom)
        {
            enrich.title = Some((
                crate::core::clean_text_fact(&title, crate::core::MAX_TEXT_FACT_CHARS),
                TitleKind::Auto,
            ));
        }
        if let Some(prompt) = parsed.last_prompt {
            enrich.preview = Some(crate::core::clean_text_fact(
                &prompt,
                crate::core::MAX_PREVIEW_CHARS,
            ));
        }
        let Some(message) = parsed.message else {
            continue;
        };
        if let Some(model) = message.model {
            enrich.model = Some(crate::core::clean_text_fact(
                &model,
                crate::core::MAX_MODEL_CHARS,
            ));
        }
        let Some(entry_usage) = message.usage else {
            continue;
        };
        if track_context {
            enrich.context_tokens = Some(
                entry_usage
                    .input_tokens
                    .saturating_add(entry_usage.cache_read_input_tokens)
                    .saturating_add(entry_usage.cache_creation_input_tokens),
            );
        }
        let Some(request_id) = parsed.request_id else {
            continue;
        };
        if ring.contains(&request_id) {
            continue;
        }
        ring.push_back(request_id);
        if ring.len() > DEDUP_RING_CAPACITY {
            ring.pop_front();
        }
        let delta = Usage {
            input_uncached: entry_usage.input_tokens,
            input_cache_read: entry_usage.cache_read_input_tokens,
            input_cache_write: entry_usage.cache_creation_input_tokens,
            output: entry_usage.output_tokens,
            reasoning: 0,
        };
        usage.add(&delta);
        if let Some(ts_ms) = parsed.timestamp.as_deref().and_then(parse_rfc3339_ms)
            && ts_ms >= bucket_cutoff_ms
            && ts_ms <= bucket_ceiling_ms
        {
            let hour_ts = ts_ms / 1000 / 3600 * 3600;
            buckets.entry(hour_ts).or_default().add(&delta);
        }
    }
    Ok(consumed)
}

const PID_START_TOLERANCE_SECS: i64 = 300;

fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn tail_last_context(path: &Path) -> Option<u64> {
    crate::core::tail_find_last(path, "\"usage\"", |parsed: ClaudeLine| {
        let usage = parsed.message.and_then(|m| m.usage)?;
        Some(
            usage
                .input_tokens
                .saturating_add(usage.cache_read_input_tokens)
                .saturating_add(usage.cache_creation_input_tokens),
        )
    })
}

#[derive(Deserialize)]
struct LiveSessionFile {
    pid: Option<i32>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "startedAt")]
    started_at: Option<i64>,
    status: Option<String>,
}

fn read_live_sessions(root: &Path) -> HashMap<String, Liveness> {
    let mut map = HashMap::new();
    let sessions_dir = root.join("sessions");
    let elapsed = process_elapsed_secs();
    let now = now_ms();
    for entry in std::fs::read_dir(&sessions_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(text) = crate::core::read_to_string_capped(&path, crate::core::MAX_CONFIG_BYTES)
        else {
            continue;
        };
        let Ok(live) = serde_json::from_str::<LiveSessionFile>(&text) else {
            continue;
        };
        let (Some(pid), Some(session_id)) = (live.pid, live.session_id) else {
            continue;
        };
        if pid <= 0 {
            continue;
        }
        if unsafe { libc::kill(pid, 0) } != 0 {
            continue;
        }
        let Some(started_at) = live.started_at else {
            continue;
        };
        if !elapsed.is_empty() {
            let Some(proc_elapsed) = elapsed.get(&pid) else {
                continue;
            };
            let recorded_elapsed = (now - started_at) / 1000;
            if (recorded_elapsed - proc_elapsed).abs() > PID_START_TOLERANCE_SECS {
                continue;
            }
        }
        let liveness = match live.status.as_deref() {
            Some("waiting") => Liveness::WaitingForInput,
            Some("idle") => Liveness::Open,
            _ => Liveness::Running,
        };
        map.insert(session_id, liveness);
    }
    map
}

fn process_elapsed_secs() -> HashMap<i32, i64> {
    let ps = super::system_program(&["/bin/ps", "/usr/bin/ps"], "ps");
    let output = std::process::Command::new(ps)
        .args(["-axo", "pid=,etime="])
        .output();
    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let trimmed = line.trim_start();
        let Some((pid, rest)) = trimmed.split_once(' ') else {
            continue;
        };
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        let Some(secs) = parse_etime_secs(rest.trim()) else {
            continue;
        };
        map.insert(pid, secs);
    }
    map
}

fn parse_etime_secs(etime: &str) -> Option<i64> {
    let (days, clock) = match etime.split_once('-') {
        Some((d, rest)) => (d.parse::<i64>().ok()?, rest),
        None => (0, etime),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [h, m, s] => (
            h.parse::<i64>().ok()?,
            m.parse::<i64>().ok()?,
            s.parse::<i64>().ok()?,
        ),
        [m, s] => (0, m.parse::<i64>().ok()?, s.parse::<i64>().ok()?),
        _ => return None,
    };
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds)
}

fn load_ring(state: Option<&str>) -> VecDeque<String> {
    state
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .map(VecDeque::from)
        .unwrap_or_default()
}

fn save_ring(ring: &VecDeque<String>) -> String {
    serde_json::to_string(&ring.iter().collect::<Vec<_>>()).unwrap_or_else(|_| "[]".to_string())
}

fn parse_rfc3339_ms(ts: &str) -> Option<i64> {
    OffsetDateTime::parse(ts, &Rfc3339)
        .ok()
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
}

fn is_uuid_like(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rfc3339(ts_ms: i64) -> String {
        let t = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ts_ms) * 1_000_000).unwrap();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            t.year(),
            t.month() as u8,
            t.day(),
            t.hour(),
            t.minute(),
            t.second(),
            t.millisecond()
        )
    }

    fn assistant_line_at(request_id: &str, input: u64, output: u64, ts_ms: i64) -> String {
        let timestamp = rfc3339(ts_ms);
        format!(
            "{{\"type\":\"assistant\",\"requestId\":\"{request_id}\",\"timestamp\":\"{timestamp}\",\"cwd\":\"/tmp/repo\",\"message\":{{\"model\":\"claude-fable-5\",\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"cache_creation_input_tokens\":5,\"cache_read_input_tokens\":100}}}}}}\n"
        )
    }

    fn assistant_line(request_id: &str, input: u64, output: u64) -> String {
        assistant_line_at(request_id, input, output, now_ms() - 3_600_000)
    }

    fn fixture_path(name: &str) -> PathBuf {
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn parse_all(
        path: &Path,
        offset: u64,
        usage: &mut Usage,
        ring: &mut VecDeque<String>,
    ) -> (u64, Enrichment, HashMap<i64, Usage>) {
        let mut enrich = Enrichment::default();
        let mut buckets = HashMap::new();
        let consumed = parse_claude_file(
            path,
            offset,
            usage,
            ring,
            &mut enrich,
            &mut buckets,
            0,
            true,
        )
        .unwrap();
        (consumed, enrich, buckets)
    }

    #[test]
    fn duplicated_request_id_counted_once() {
        let path = fixture_path("dedup.jsonl");
        let line = assistant_line("req_1", 10, 20);
        std::fs::write(&path, format!("{line}{line}{line}")).unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (_, enrich, buckets) = parse_all(&path, 0, &mut usage, &mut ring);
        assert_eq!(usage.input_uncached, 10);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.input_cache_write, 5);
        assert_eq!(usage.input_cache_read, 100);
        assert_eq!(enrich.cwd.as_deref(), Some(Path::new("/tmp/repo")));
        assert_eq!(enrich.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(buckets.len(), 1);
    }

    #[test]
    fn incremental_parse_with_ring_skips_old_request_ids() {
        let path = fixture_path("incremental.jsonl");
        std::fs::write(&path, assistant_line("req_1", 10, 20)).unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (consumed, _, _) = parse_all(&path, 0, &mut usage, &mut ring);
        let mut appended = std::fs::read(&path).unwrap();
        appended.extend_from_slice(assistant_line("req_1", 10, 20).as_bytes());
        appended.extend_from_slice(assistant_line("req_2", 1, 2).as_bytes());
        std::fs::write(&path, appended).unwrap();
        let (_, _, _) = parse_all(&path, consumed, &mut usage, &mut ring);
        assert_eq!(usage.input_uncached, 11);
        assert_eq!(usage.output, 22);
    }

    fn cwd_line(cwd: &str) -> String {
        format!("{{\"type\":\"user\",\"cwd\":\"{cwd}\",\"sessionId\":\"x\"}}\n")
    }

    #[test]
    fn last_cwd_in_transcript_wins() {
        let path = fixture_path("cwd-moved.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}{}{}",
                cwd_line("/tmp/old"),
                cwd_line("/tmp"),
                cwd_line("/tmp/new")
            ),
        )
        .unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (_, enrich, _) = parse_all(&path, 0, &mut usage, &mut ring);
        assert_eq!(enrich.cwd.as_deref(), Some(Path::new("/tmp/new")));
    }

    #[test]
    fn incremental_parse_updates_cwd_and_keeps_it_when_absent() {
        let path = fixture_path("cwd-incremental.jsonl");
        std::fs::write(&path, cwd_line("/tmp/a")).unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (consumed, enrich, _) = parse_all(&path, 0, &mut usage, &mut ring);
        assert_eq!(enrich.cwd.as_deref(), Some(Path::new("/tmp/a")));
        let mut appended = std::fs::read(&path).unwrap();
        appended.extend_from_slice(cwd_line("/tmp/b").as_bytes());
        std::fs::write(&path, &appended).unwrap();
        let (consumed, enrich, _) = parse_all(&path, consumed, &mut usage, &mut ring);
        assert_eq!(enrich.cwd.as_deref(), Some(Path::new("/tmp/b")));
        appended
            .extend_from_slice(b"{\"type\":\"ai-title\",\"aiTitle\":\"T\",\"sessionId\":\"x\"}\n");
        std::fs::write(&path, &appended).unwrap();
        let (_, enrich, _) = parse_all(&path, consumed, &mut usage, &mut ring);
        assert_eq!(enrich.cwd, None);
    }

    #[test]
    fn custom_title_wins_over_ai_title() {
        let path = fixture_path("titles.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"ai-title\",\"aiTitle\":\"First\",\"sessionId\":\"x\"}\n\
             {\"type\":\"custom-title\",\"customTitle\":\"Mine\",\"sessionId\":\"x\"}\n\
             {\"type\":\"ai-title\",\"aiTitle\":\"Second\",\"sessionId\":\"x\"}\n\
             {\"type\":\"last-prompt\",\"lastPrompt\":\"do things\",\"sessionId\":\"x\"}\n",
        )
        .unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (_, enrich, _) = parse_all(&path, 0, &mut usage, &mut ring);
        assert_eq!(enrich.title, Some(("Mine".to_string(), TitleKind::Custom)));
        assert_eq!(enrich.preview.as_deref(), Some("do things"));
    }

    #[test]
    fn incomplete_tail_line_is_not_consumed() {
        let path = fixture_path("partial.jsonl");
        let full = assistant_line("req_1", 10, 20);
        let partial = "{\"type\":\"assistant\",\"requestId\":\"req_3\"";
        std::fs::write(&path, format!("{full}{partial}")).unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let (consumed, _, _) = parse_all(&path, 0, &mut usage, &mut ring);
        assert_eq!(consumed, full.len() as u64);
        assert_eq!(usage.input_uncached, 10);
    }

    #[test]
    fn tail_context_survives_multibyte_split_at_tail_boundary() {
        let path = fixture_path("tail-multibyte.jsonl");
        let usage_line = assistant_line("req_tail", 42, 7);
        let mut filler = 0usize;
        let content = loop {
            let mut c = String::from("{\"pad\":\"");
            c.push_str(&"x".repeat(filler));
            c.push_str(&"€".repeat(30_000));
            c.push_str("\"}\n");
            c.push_str(&usage_line);
            if !c.is_char_boundary(c.len() - crate::core::TAIL_SCAN_BYTES as usize) {
                break c;
            }
            filler += 1;
        };
        std::fs::write(&path, &content).unwrap();
        assert_eq!(tail_last_context(&path), Some(42 + 100 + 5));
    }

    #[test]
    fn far_future_and_pre_cutoff_timestamps_are_counted_but_not_bucketed() {
        let path = fixture_path("bucket-bounds.jsonl");
        let now = now_ms();
        let content = format!(
            "{}{}{}",
            assistant_line_at("req_future", 10, 20, now + 48 * 3600 * 1000),
            assistant_line_at("req_ancient", 10, 20, now - 30 * 24 * 3600 * 1000),
            assistant_line_at("req_current", 10, 20, now - 3_600_000),
        );
        std::fs::write(&path, content).unwrap();
        let mut usage = Usage::default();
        let mut ring = VecDeque::new();
        let mut enrich = Enrichment::default();
        let mut buckets = HashMap::new();
        parse_claude_file(
            &path,
            0,
            &mut usage,
            &mut ring,
            &mut enrich,
            &mut buckets,
            now - crate::core::BUCKET_RETENTION_SECS * 1000,
            true,
        )
        .unwrap();
        assert_eq!(usage.input_uncached, 30);
        assert_eq!(buckets.len(), 1);
    }

    #[test]
    fn etime_parsing() {
        assert_eq!(parse_etime_secs("05:33"), Some(333));
        assert_eq!(parse_etime_secs("01:02:03"), Some(3723));
        assert_eq!(parse_etime_secs("2-01:02:03"), Some(176_523));
        assert_eq!(parse_etime_secs("bogus"), None);
    }

    #[test]
    fn uuid_like_check() {
        assert!(is_uuid_like("827e8acd-3061-4994-a5ec-9a9f33c06fa4"));
        assert!(!is_uuid_like("agent-abc123"));
        assert!(!is_uuid_like("827e8acd-3061-4994-a5ec"));
    }
}
