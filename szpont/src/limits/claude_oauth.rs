use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Credentials, ToolLimits, WindowUsage};
use crate::core::{ToolId, now_ms};
use crate::store::Store;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const ANTHROPIC_BETA: &str = "oauth-2025-04-20";
const USER_AGENT: &str = "claude-code/2.0.0";
const LAST_CALL_KEY: &str = "claude_oauth_last_call";
const CACHE_KEY: &str = "claude_oauth_cache";
const KEYCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn probe(store: &Store) -> Option<ToolLimits> {
    super::cached_probe(store, LAST_CALL_KEY, CACHE_KEY, || {
        let token = access_token()?;
        let body = fetch_usage(&token)?;
        parse_response(&body, now_ms())
    })
}

#[derive(Deserialize)]
struct KeychainCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OauthEntry>,
}

#[derive(Deserialize)]
struct OauthEntry {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

fn access_token() -> Option<String> {
    let oauth = oauth_entry()?;
    if oauth.expires_at.is_some_and(|exp| exp <= now_ms()) {
        return None;
    }
    oauth.access_token
}

pub(crate) fn credentials_state() -> Credentials {
    let Some(oauth) = oauth_entry() else {
        return Credentials::Missing;
    };
    if oauth.access_token.is_none_or(|t| t.is_empty()) {
        return Credentials::Missing;
    }
    match oauth.expires_at {
        Some(exp) if exp <= now_ms() => Credentials::Expired(exp),
        _ => Credentials::Valid,
    }
}

fn oauth_entry() -> Option<OauthEntry> {
    let text = keychain_credentials().or_else(file_credentials)?;
    let creds: KeychainCredentials = serde_json::from_str(text.trim()).ok()?;
    creds.claude_ai_oauth
}

fn keychain_credentials() -> Option<String> {
    use std::io::Read;
    let security = crate::adapters::system_program(&["/usr/bin/security"], "security");
    let mut child = std::process::Command::new(security)
        .args([
            "find-generic-password",
            "-s",
            "Claude Code-credentials",
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + KEYCHAIN_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    crate::logging::warn("keychain query for Claude credentials timed out");
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    };
    if !status.success() {
        return None;
    }
    let mut text = String::new();
    child
        .stdout
        .take()?
        .take(crate::core::MAX_CREDENTIALS_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn file_credentials() -> Option<String> {
    let root = ToolId::Claude.home()?;
    crate::core::read_to_string_capped(
        &root.join(".credentials.json"),
        crate::core::MAX_CREDENTIALS_BYTES,
    )
}

fn fetch_usage(token: &str) -> Option<String> {
    let result = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", ANTHROPIC_BETA)
        .set("User-Agent", USER_AGENT)
        .set("Content-Type", "application/json")
        .timeout(super::HTTP_TIMEOUT)
        .call();
    match result {
        Ok(response) => response.into_string().ok(),
        Err(ureq::Error::Status(code, _)) => {
            crate::logging::warn(&format!("claude usage endpoint returned HTTP {code}"));
            None
        }
        Err(_) => None,
    }
}

#[derive(Deserialize)]
struct UsageResponse {
    five_hour: Option<UsageWindow>,
    seven_day: Option<UsageWindow>,
    seven_day_opus: Option<UsageWindow>,
    seven_day_sonnet: Option<UsageWindow>,
    extra_usage: Option<ExtraUsage>,
    limits: Option<Vec<LimitEntry>>,
}

#[derive(Deserialize)]
struct LimitEntry {
    kind: Option<String>,
    group: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    scope: Option<LimitScope>,
}

#[derive(Deserialize)]
struct LimitScope {
    model: Option<ScopeModel>,
}

#[derive(Deserialize)]
struct ScopeModel {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct UsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    utilization: Option<f64>,
}

fn parse_response(body: &str, captured_at: i64) -> Option<ToolLimits> {
    let usage: UsageResponse = serde_json::from_str(body).ok()?;
    let mut windows: Vec<WindowUsage> = usage
        .limits
        .iter()
        .flatten()
        .filter_map(window_from_limit)
        .collect();
    if windows.is_empty() {
        for (window, label, horizon_secs) in [
            (&usage.five_hour, "5h", 5 * 3600),
            (&usage.seven_day, "week", 7 * 24 * 3600),
            (&usage.seven_day_opus, "week opus", 7 * 24 * 3600),
            (&usage.seven_day_sonnet, "week sonnet", 7 * 24 * 3600),
        ] {
            let Some(w) = window else { continue };
            if w.utilization.is_none() {
                continue;
            }
            windows.push(WindowUsage {
                label: label.to_string(),
                used_percent: w.utilization,
                resets_at: parse_reset(w.resets_at.as_deref()),
                tokens: None,
                estimated: false,
                horizon_secs: Some(horizon_secs),
            });
        }
    }
    if let Some(extra) = &usage.extra_usage
        && extra.is_enabled
        && let Some(utilization) = extra.utilization
    {
        windows.push(WindowUsage {
            label: "extra".to_string(),
            used_percent: Some(utilization),
            resets_at: None,
            tokens: None,
            estimated: false,
            horizon_secs: None,
        });
    }
    if windows.is_empty() {
        return None;
    }
    Some(ToolLimits {
        tool: ToolId::Claude,
        windows,
        plan: None,
        captured_at,
        source: "oauth".to_string(),
        note: None,
        stale_credentials: false,
    })
}

fn window_from_limit(entry: &LimitEntry) -> Option<WindowUsage> {
    let percent = entry.percent?;
    let group = entry
        .group
        .as_deref()
        .or(entry.kind.as_deref())
        .unwrap_or("window");
    let (base, horizon_secs) = match group {
        "session" => ("5h".to_string(), Some(5 * 3600)),
        "weekly" => ("week".to_string(), Some(7 * 24 * 3600)),
        other => (clean_label(other), None),
    };
    let scope = entry
        .scope
        .as_ref()
        .and_then(|s| s.model.as_ref())
        .and_then(|m| m.display_name.as_deref())
        .map(clean_label)
        .filter(|name| !name.is_empty());
    let label = match scope {
        Some(model) => format!("{base} {model}"),
        None => base,
    };
    Some(WindowUsage {
        label,
        used_percent: Some(percent),
        resets_at: parse_reset(entry.resets_at.as_deref()),
        tokens: None,
        estimated: false,
        horizon_secs,
    })
}

fn clean_label(raw: &str) -> String {
    crate::core::sanitize_for_terminal(raw)
        .chars()
        .take(24)
        .collect::<String>()
        .to_lowercase()
}

fn parse_reset(ts: Option<&str>) -> Option<i64> {
    ts.and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok())
        .map(OffsetDateTime::unix_timestamp)
}

#[cfg(test)]
mod tests {
    use super::parse_response;

    #[test]
    fn full_body_produces_all_windows_and_gates_extra_on_is_enabled() {
        let body = r#"{
            "five_hour": {"utilization": 12.5, "resets_at": "2026-08-10T12:00:00Z"},
            "seven_day": {"utilization": 40.0},
            "seven_day_opus": {"utilization": 5.0},
            "seven_day_sonnet": {"utilization": 7.0},
            "extra_usage": {"is_enabled": false, "utilization": 1.0}
        }"#;
        let limits = parse_response(body, 123).unwrap();
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "week", "week opus", "week sonnet"]);
        assert_eq!(limits.windows[0].used_percent, Some(12.5));
        assert!(limits.windows[0].resets_at.is_some());
        assert_eq!(limits.windows[0].horizon_secs, Some(5 * 3600));
        assert_eq!(limits.captured_at, 123);
        assert_eq!(limits.source, "oauth");
    }

    #[test]
    fn structured_limits_array_wins_and_carries_model_scoped_windows() {
        let body = r#"{
            "five_hour": {"utilization": 23.0, "resets_at": "2026-08-12T17:30:00.076485+00:00"},
            "limits": [
                {"kind": "session", "group": "session", "percent": 23,
                 "resets_at": "2026-08-12T17:30:00.076485+00:00", "scope": null},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 86,
                 "resets_at": "2026-08-14T10:00:00.077500+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
                {"kind": "weekly", "group": "weekly", "percent": null}
            ]
        }"#;
        let limits = parse_response(body, 5).unwrap();
        let labels: Vec<&str> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, vec!["5h", "week fable"]);
        assert_eq!(limits.windows[1].used_percent, Some(86.0));
        assert_eq!(limits.windows[1].horizon_secs, Some(7 * 24 * 3600));
        assert!(limits.windows[1].resets_at.is_some());
    }

    #[test]
    fn null_utilization_windows_are_dropped_and_empty_body_is_none() {
        let body = r#"{
            "five_hour": {"utilization": null},
            "extra_usage": {"is_enabled": true, "utilization": 33.0}
        }"#;
        let limits = parse_response(body, 0).unwrap();
        assert_eq!(limits.windows.len(), 1);
        assert_eq!(limits.windows[0].label, "extra");
        assert!(parse_response("{}", 0).is_none());
        assert!(parse_response("not json", 0).is_none());
    }
}
