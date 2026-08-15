use serde_json::Value;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Credentials, ToolLimits, WindowUsage};
use crate::core::{ToolId, now_ms, read_to_string_capped};
use crate::store::Store;

const LAST_CALL_KEY: &str = "kimi_oauth_last_call";
const CACHE_KEY: &str = "kimi_oauth_cache";
const DEFAULT_BASE_URL: &str = "https://api.kimi.com/coding/v1";
const KIMI_PROVIDER_SECTION: &str = "providers.\"managed:kimi-code\"";

pub fn probe(store: &Store) -> Option<ToolLimits> {
    super::cached_probe(store, LAST_CALL_KEY, CACHE_KEY, || {
        let token = access_token()?;
        let body = fetch_usages(&token)?;
        parse_response(&body, now_ms())
    })
}

fn access_token() -> Option<String> {
    let creds = credentials_json()?;
    if !matches!(credentials_state_of(&creds), Credentials::Valid) {
        return None;
    }
    creds
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn credentials_state() -> Credentials {
    credentials_json().map_or(Credentials::Missing, |creds| credentials_state_of(&creds))
}

fn credentials_json() -> Option<Value> {
    let root = ToolId::Kimi.home()?;
    let path = root.join("credentials").join("kimi-code.json");
    let text = read_to_string_capped(&path, crate::core::MAX_CREDENTIALS_BYTES)?;
    serde_json::from_str(&text).ok()
}

fn credentials_state_of(creds: &Value) -> Credentials {
    if creds
        .get("access_token")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Credentials::Missing;
    }
    match creds.get("expires_at").and_then(Value::as_i64) {
        Some(expires_at) => {
            let expires_ms = if expires_at > 100_000_000_000 {
                expires_at
            } else {
                expires_at.saturating_mul(1000)
            };
            if expires_ms <= now_ms() {
                Credentials::Expired(expires_ms)
            } else {
                Credentials::Valid
            }
        }
        None => Credentials::Valid,
    }
}

fn base_url() -> String {
    configured_base_url().unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
}

fn configured_base_url() -> Option<String> {
    let root = ToolId::Kimi.home()?;
    let text = read_to_string_capped(&root.join("config.toml"), crate::core::MAX_CONFIG_BYTES)?;
    let mut in_kimi_provider = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(section) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_kimi_provider = section.trim() == KIMI_PROVIDER_SECTION;
            continue;
        }
        if in_kimi_provider
            && let Some(value) = trimmed
                .strip_prefix("base_url")
                .and_then(|rest| rest.trim_start().strip_prefix('='))
        {
            return validated_base_url(value.trim().trim_matches('"'));
        }
    }
    None
}

fn validated_base_url(value: &str) -> Option<String> {
    if value.contains(['\\', '?', '#', '@']) || value.contains(char::is_whitespace) {
        return None;
    }
    let rest = value.strip_prefix("https://")?;
    let host = rest.split(['/', ':']).next()?;
    let allowed = host == "api.kimi.com"
        || host.ends_with(".kimi.com")
        || host == "api.moonshot.cn"
        || host.ends_with(".moonshot.cn");
    allowed.then(|| value.to_string())
}

fn fetch_usages(token: &str) -> Option<String> {
    let result = ureq::get(&format!("{}/usages", base_url()))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(super::HTTP_TIMEOUT)
        .call();
    match result {
        Ok(response) => response.into_string().ok(),
        Err(ureq::Error::Status(code, _)) => {
            crate::logging::warn(&format!("kimi usage endpoint returned HTTP {code}"));
            None
        }
        Err(_) => None,
    }
}

fn parse_response(body: &str, captured_at: i64) -> Option<ToolLimits> {
    let value: Value = serde_json::from_str(body).ok()?;
    let mut windows = Vec::new();
    if let Some(main) = value.get("usage")
        && let Some(window) = quota_window("cycle", main)
    {
        windows.push(window);
    }
    for entry in value
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let minutes = entry.get("window").and_then(window_minutes);
        let label = minutes.map_or_else(|| "window".to_string(), super::humanize_window);
        if let Some(mut window) = entry
            .get("detail")
            .and_then(|detail| quota_window_owned(label.clone(), detail))
        {
            window.horizon_secs = minutes.map(|m| m.saturating_mul(60));
            windows.push(window);
        }
    }
    if windows.is_empty() {
        return None;
    }
    let plan = value
        .pointer("/user/membership/level")
        .and_then(Value::as_str)
        .map(|level| level.strip_prefix("LEVEL_").unwrap_or(level).to_lowercase());
    Some(ToolLimits {
        tool: ToolId::Kimi,
        windows,
        plan,
        captured_at,
        source: "kimi-api".to_string(),
        note: None,
        stale_credentials: false,
    })
}

fn quota_window(label: &str, detail: &Value) -> Option<WindowUsage> {
    quota_window_owned(label.to_string(), detail)
}

fn quota_window_owned(label: String, detail: &Value) -> Option<WindowUsage> {
    let limit = number(detail.get("limit")?)?;
    if limit <= 0.0 {
        return None;
    }
    let used = detail.get("used").and_then(number).unwrap_or(0.0);
    let resets_at = detail
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok())
        .map(time::OffsetDateTime::unix_timestamp);
    Some(WindowUsage {
        label,
        used_percent: Some((used / limit * 100.0).clamp(0.0, 100.0)),
        resets_at,
        tokens: None,
        estimated: false,
        horizon_secs: None,
    })
}

fn window_minutes(window: &Value) -> Option<i64> {
    let duration = window.get("duration").and_then(number)? as i64;
    let unit = window.get("timeUnit").and_then(Value::as_str).unwrap_or("");
    match unit {
        "TIME_UNIT_MINUTE" => Some(duration),
        "TIME_UNIT_HOUR" => Some(duration.saturating_mul(60)),
        "TIME_UNIT_DAY" => Some(duration.saturating_mul(1440)),
        "TIME_UNIT_SECOND" => Some(duration / 60),
        _ => None,
    }
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .filter(|v: &f64| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::{parse_response, validated_base_url, window_minutes};
    use serde_json::json;

    #[test]
    fn base_url_override_requires_https_and_a_known_host() {
        assert_eq!(
            validated_base_url("https://api.kimi.com/coding/v1"),
            Some("https://api.kimi.com/coding/v1".to_string())
        );
        assert!(validated_base_url("http://api.kimi.com/coding/v1").is_none());
        assert!(validated_base_url("https://attacker.example/coding/v1").is_none());
        assert!(validated_base_url("https://api.kimi.com.evil.example/x").is_none());
        assert!(validated_base_url("https://api.moonshot.cn/v1").is_some());
    }

    #[test]
    fn base_url_override_rejects_authority_delimiter_smuggling() {
        assert!(validated_base_url("https://evil.example\\.kimi.com").is_none());
        assert!(validated_base_url("https://evil.example?.kimi.com").is_none());
        assert!(validated_base_url("https://evil.example#.kimi.com").is_none());
        assert!(validated_base_url("https://user@evil.example/.kimi.com").is_none());
        assert!(validated_base_url("https://evil.example .kimi.com").is_none());
    }

    #[test]
    fn usage_and_limit_entries_produce_windows() {
        let body = json!({
            "usage": { "limit": 100, "used": 25 },
            "limits": [{
                "window": { "duration": 5, "timeUnit": "TIME_UNIT_HOUR" },
                "detail": { "limit": "200", "used": "300", "resetTime": "2026-08-10T12:00:00Z" }
            }],
            "user": { "membership": { "level": "LEVEL_PRO" } }
        })
        .to_string();
        let limits = parse_response(&body, 7).unwrap();
        assert_eq!(limits.plan.as_deref(), Some("pro"));
        assert_eq!(limits.windows.len(), 2);
        assert_eq!(limits.windows[0].label, "cycle");
        assert_eq!(limits.windows[0].used_percent, Some(25.0));
        assert_eq!(limits.windows[1].label, "5h");
        assert_eq!(limits.windows[1].used_percent, Some(100.0));
        assert_eq!(limits.windows[1].horizon_secs, Some(5 * 3600));
        assert!(limits.windows[1].resets_at.is_some());
    }

    #[test]
    fn zero_limit_windows_are_dropped_and_empty_body_is_none() {
        let body = json!({ "usage": { "limit": 0, "used": 5 } }).to_string();
        assert!(parse_response(&body, 0).is_none());
        assert!(parse_response("{}", 0).is_none());
        assert!(parse_response("not json", 0).is_none());
    }

    #[test]
    fn window_minutes_understands_all_time_units() {
        assert_eq!(
            window_minutes(&json!({"duration": 5, "timeUnit": "TIME_UNIT_MINUTE"})),
            Some(5)
        );
        assert_eq!(
            window_minutes(&json!({"duration": 2, "timeUnit": "TIME_UNIT_HOUR"})),
            Some(120)
        );
        assert_eq!(
            window_minutes(&json!({"duration": 1, "timeUnit": "TIME_UNIT_DAY"})),
            Some(1440)
        );
        assert_eq!(
            window_minutes(&json!({"duration": 120, "timeUnit": "TIME_UNIT_SECOND"})),
            Some(2)
        );
        assert_eq!(
            window_minutes(&json!({"duration": 1, "timeUnit": "bogus"})),
            None
        );
    }
}
