mod claude_oauth;
mod kimi_oauth;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::{ToolId, now_ms};
use crate::store::Store;

const STALE_AFTER_MS: i64 = 3600 * 1000;
const FIVE_HOURS_SECS: i64 = 5 * 3600;
const WEEK_SECS: i64 = 7 * 24 * 3600;
pub(crate) const MIN_CALL_INTERVAL_MS: i64 = 180_000;
const CACHE_WRITER: &str = env!("CARGO_PKG_VERSION");
pub(crate) const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowUsage {
    pub label: String,
    pub used_percent: Option<f64>,
    pub resets_at: Option<i64>,
    pub tokens: Option<u64>,
    pub estimated: bool,
    #[serde(default)]
    pub horizon_secs: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolLimits {
    pub tool: ToolId,
    pub windows: Vec<WindowUsage>,
    pub plan: Option<String>,
    pub captured_at: i64,
    pub source: String,
    pub note: Option<String>,
    #[serde(default)]
    pub stale_credentials: bool,
}

pub(crate) enum Credentials {
    Valid,
    Expired(i64),
    Missing,
}

impl Credentials {
    pub(crate) fn note(&self, tool: ToolId) -> Option<String> {
        let command = tool.as_str();
        match self {
            Credentials::Valid => None,
            Credentials::Expired(at) => Some(format!(
                "{} sign-in expired {} ago — run {command} to refresh it",
                tool.display_name(),
                crate::core::format_age(now_ms().saturating_sub(*at))
            )),
            Credentials::Missing => Some(format!(
                "no {} credentials found — run {command} and sign in",
                tool.display_name()
            )),
        }
    }
}

pub fn collect(store: &Store) -> Vec<ToolLimits> {
    let installed: std::collections::HashSet<ToolId> = crate::adapters::installed()
        .iter()
        .map(|a| a.id())
        .collect();
    let mut all = Vec::new();
    if installed.contains(&ToolId::Claude)
        && let Some(limits) = claude_limits(store)
    {
        all.push(limits);
    }
    if installed.contains(&ToolId::Codex)
        && let Some(limits) = codex_limits()
    {
        all.push(limits);
    }
    if installed.contains(&ToolId::Kimi) {
        all.push(kimi_limits(store));
    }
    for limits in &all {
        if let Ok(json) = serde_json::to_string(limits) {
            let _ = store.save_limits_snapshot(limits.tool, &json);
        }
    }
    all
}

pub fn load_cached(store: &Store) -> Vec<ToolLimits> {
    store
        .load_limits_snapshots()
        .unwrap_or_default()
        .iter()
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect()
}

fn claude_limits(store: &Store) -> Option<ToolLimits> {
    if let Some(mut limits) = claude_oauth::probe(store) {
        let now_secs = now_ms() / 1000;
        for window in &mut limits.windows {
            let horizon_secs = window.horizon_secs.unwrap_or(match window.label.as_str() {
                "5h" => FIVE_HOURS_SECS,
                _ => WEEK_SECS,
            });
            window.tokens = store
                .bucket_tokens_since(ToolId::Claude, now_secs - horizon_secs)
                .ok();
        }
        return Some(limits);
    }
    let now = now_ms() / 1000;
    let tokens_5h = store
        .bucket_tokens_since(ToolId::Claude, now - FIVE_HOURS_SECS)
        .ok()?;
    let tokens_7d = store
        .bucket_tokens_since(ToolId::Claude, now - WEEK_SECS)
        .ok()?;
    let ceiling_5h = calibrate_ceiling(store, "claude_ceiling_5h", tokens_5h);
    let ceiling_7d = calibrate_ceiling(store, "claude_ceiling_7d", tokens_7d);
    let credentials = claude_oauth::credentials_state();
    let stale_note = credentials.note(ToolId::Claude);
    Some(ToolLimits {
        tool: ToolId::Claude,
        windows: vec![
            WindowUsage {
                label: "5h".to_string(),
                used_percent: percent_of(tokens_5h, ceiling_5h),
                resets_at: None,
                tokens: Some(tokens_5h),
                estimated: true,
                horizon_secs: Some(FIVE_HOURS_SECS),
            },
            WindowUsage {
                label: "week".to_string(),
                used_percent: percent_of(tokens_7d, ceiling_7d),
                resets_at: None,
                tokens: Some(tokens_7d),
                estimated: true,
                horizon_secs: Some(WEEK_SECS),
            },
        ],
        plan: None,
        captured_at: now_ms(),
        source: "local-estimate".to_string(),
        note: Some(stale_note.map_or_else(
            || {
                "estimated from local logs; % is relative to the largest window ever observed"
                    .to_string()
            },
            |note| format!("{note}; showing local estimates until then"),
        )),
        stale_credentials: !matches!(credentials, Credentials::Valid),
    })
}

pub(crate) fn cached_probe(
    store: &Store,
    last_call_key: &str,
    cache_key: &str,
    fetch: impl FnOnce() -> Option<ToolLimits>,
) -> Option<ToolLimits> {
    let now = now_ms();
    let last_call = store
        .meta_get(last_call_key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    if now - last_call < MIN_CALL_INTERVAL_MS
        && let Some(cached) = store.meta_get(cache_key).ok().flatten()
        && store
            .meta_get(&format!("{cache_key}_writer"))
            .ok()
            .flatten()
            .is_some_and(|writer| writer == CACHE_WRITER)
        && let Ok(limits) = serde_json::from_str::<ToolLimits>(&cached)
    {
        return Some(limits);
    }
    let _ = store.meta_set(last_call_key, &now.to_string());
    let limits = fetch()?;
    if let Ok(json) = serde_json::to_string(&limits) {
        let _ = store.meta_set(cache_key, &json);
        let _ = store.meta_set(&format!("{cache_key}_writer"), CACHE_WRITER);
    }
    Some(limits)
}

fn kimi_limits(store: &Store) -> ToolLimits {
    if let Some(limits) = kimi_oauth::probe(store) {
        return limits;
    }
    let credentials = kimi_oauth::credentials_state();
    ToolLimits {
        tool: ToolId::Kimi,
        windows: Vec::new(),
        plan: None,
        captured_at: now_ms(),
        source: "none".to_string(),
        note: Some(credentials.note(ToolId::Kimi).unwrap_or_else(|| {
            "Kimi keeps no limit data on disk; its usage endpoint did not answer".to_string()
        })),
        stale_credentials: !matches!(credentials, Credentials::Valid),
    }
}

fn calibrate_ceiling(store: &Store, key: &str, observed: u64) -> u64 {
    let stored = store
        .meta_get(key)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    if observed > stored {
        let _ = store.meta_set(key, &observed.to_string());
        observed
    } else {
        stored
    }
}

fn percent_of(tokens: u64, ceiling: u64) -> Option<f64> {
    if ceiling == 0 {
        return None;
    }
    Some((tokens as f64 / ceiling as f64 * 100.0).min(100.0))
}

#[derive(Deserialize)]
struct CodexRolloutLine {
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    rate_limits: Option<CodexRateLimits>,
}

#[derive(Deserialize)]
struct CodexRateLimits {
    primary: Option<CodexRateWindow>,
    secondary: Option<CodexRateWindow>,
    plan_type: Option<String>,
    rate_limit_reached_type: Option<String>,
}

#[derive(Deserialize)]
struct CodexRateWindow {
    used_percent: Option<f64>,
    window_minutes: Option<i64>,
    resets_at: Option<i64>,
}

fn codex_limits() -> Option<ToolLimits> {
    let root = ToolId::Codex.home()?.join("sessions");
    let rollout = newest_file(&root)?;
    let mtime_ms = std::fs::metadata(&rollout)
        .ok()
        .map_or(0, |m| crate::core::system_time_ms(m.modified().ok()));
    let rate = last_rate_limits(&rollout)?;
    let mut windows = Vec::new();
    for (window, fallback) in [(&rate.primary, "primary"), (&rate.secondary, "secondary")] {
        let Some(w) = window else { continue };
        windows.push(WindowUsage {
            label: w
                .window_minutes
                .map_or_else(|| fallback.to_string(), humanize_window),
            used_percent: w.used_percent,
            resets_at: w.resets_at,
            tokens: None,
            estimated: false,
            horizon_secs: w.window_minutes.map(|m| m.saturating_mul(60)),
        });
    }
    let mut note = rate
        .rate_limit_reached_type
        .filter(|t| !t.is_empty())
        .map(|t| format!("limit reached: {t}"));
    if now_ms() - mtime_ms > STALE_AFTER_MS {
        let staleness = crate::core::format_age(now_ms() - mtime_ms);
        let stale_note = format!("data is {staleness} old (from the last codex activity)");
        note = Some(match note {
            Some(existing) => format!("{existing}; {stale_note}"),
            None => stale_note,
        });
    }
    Some(ToolLimits {
        tool: ToolId::Codex,
        windows,
        plan: rate.plan_type,
        captured_at: mtime_ms,
        source: "codex-rollout".to_string(),
        note,
        stale_credentials: false,
    })
}

fn last_rate_limits(rollout: &Path) -> Option<CodexRateLimits> {
    crate::core::tail_find_last(rollout, "\"rate_limits\"", |parsed: CodexRolloutLine| {
        let payload = parsed.payload?;
        if payload.kind.as_deref() != Some("token_count") {
            return None;
        }
        payload.rate_limits
    })
}

fn newest_file(root: &Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) else {
                    continue;
                };
                if newest.as_ref().is_none_or(|(t, _)| mtime > *t) {
                    newest = Some((mtime, path));
                }
            }
        }
    }
    newest.map(|(_, p)| p)
}

pub(crate) fn humanize_window(minutes: i64) -> String {
    match minutes {
        i64::MIN..=0 => "?".to_string(),
        1..=119 => format!("{minutes}m"),
        120..=2879 => format!("{}h", minutes / 60),
        2880..=20159 => format!("{}d", minutes / 1440),
        _ => format!("{}w", minutes / 10080),
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolLimits, humanize_window, last_rate_limits, percent_of};
    #[test]
    fn credential_states_explain_themselves_per_tool() {
        use crate::core::ToolId;
        assert!(super::Credentials::Valid.note(ToolId::Kimi).is_none());

        let expired = super::Credentials::Expired(crate::core::now_ms() - 4 * 24 * 3600 * 1000)
            .note(ToolId::Kimi)
            .unwrap();
        assert!(expired.contains("expired"), "{expired}");
        assert!(expired.contains("4d"), "{expired}");
        assert!(expired.contains("run kimi"), "{expired}");

        let missing = super::Credentials::Missing.note(ToolId::Claude).unwrap();
        assert!(missing.contains("no Claude Code credentials"), "{missing}");
        assert!(missing.contains("run claude"), "{missing}");
    }

    #[test]
    fn a_cache_written_by_another_version_is_refetched_not_reused() {
        let dir = std::path::PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("limits-cache-writer.db");
        let _ = std::fs::remove_file(&db);
        let store = crate::store::Store::open(Some(&db)).unwrap();
        let stale = ToolLimits {
            tool: crate::core::ToolId::Claude,
            windows: Vec::new(),
            plan: None,
            captured_at: 1,
            source: "stale".to_string(),
            note: None,
            stale_credentials: false,
        };
        store
            .meta_set("k_cache", &serde_json::to_string(&stale).unwrap())
            .unwrap();
        store.meta_set("k_cache_writer", "0.0.1-old").unwrap();
        store
            .meta_set("k_last", &crate::core::now_ms().to_string())
            .unwrap();

        let fresh = |source: &str| ToolLimits {
            tool: crate::core::ToolId::Claude,
            windows: Vec::new(),
            plan: None,
            captured_at: 2,
            source: source.to_string(),
            note: None,
            stale_credentials: false,
        };
        let got = super::cached_probe(&store, "k_last", "k_cache", || Some(fresh("fresh")));
        assert_eq!(got.unwrap().source, "fresh");

        let got = super::cached_probe(&store, "k_last", "k_cache", || {
            panic!("must reuse the cache written by this version")
        });
        assert_eq!(got.unwrap().source, "fresh");
    }

    #[test]
    fn last_rate_limits_takes_the_newest_token_count_line() {
        let dir = std::path::PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("limits-rollout.jsonl");
        std::fs::write(
            &path,
            "{\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":10.0,\"window_minutes\":300},\"plan_type\":\"old\"}}}\n\
             {\"payload\":{\"type\":\"other\",\"rate_limits\":{\"plan_type\":\"decoy\"}}}\n\
             {\"payload\":{\"type\":\"token_count\",\"rate_limits\":{\"primary\":{\"used_percent\":42.0,\"window_minutes\":300},\"plan_type\":\"new\"}}}\n",
        )
        .unwrap();
        let rate = last_rate_limits(&path).unwrap();
        assert_eq!(rate.plan_type.as_deref(), Some("new"));
        let primary = rate.primary.unwrap();
        assert!((primary.used_percent.unwrap() - 42.0).abs() < f64::EPSILON);
        assert_eq!(primary.window_minutes, Some(300));
    }

    #[test]
    fn humanize_window_boundaries() {
        assert_eq!(humanize_window(0), "?");
        assert_eq!(humanize_window(1), "1m");
        assert_eq!(humanize_window(119), "119m");
        assert_eq!(humanize_window(120), "2h");
        assert_eq!(humanize_window(2879), "47h");
        assert_eq!(humanize_window(2880), "2d");
        assert_eq!(humanize_window(20159), "13d");
        assert_eq!(humanize_window(20160), "2w");
    }

    #[test]
    fn percent_of_clamps_and_handles_zero_ceiling() {
        assert_eq!(percent_of(5, 0), None);
        assert!((percent_of(50, 100).unwrap() - 50.0).abs() < f64::EPSILON);
        assert!((percent_of(200, 100).unwrap() - 100.0).abs() < f64::EPSILON);
    }
}
