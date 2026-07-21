//! Claude usage, fetched directly from the OAuth usage API
//! (`https://api.anthropic.com/api/oauth/usage`). This replaces the
//! `claude-code-stats` CLI/crate so we can read the newer `limits` array, which
//! carries per-model weekly windows (e.g. Fable) the legacy fields omit.

use std::env;
use std::fs;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::Deserialize;

const USAGE_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";

const KEYCHAIN_SERVICES: &[&str] = &[
    "Claude Code-credentials",
    "Claude Code-local-oauth-credentials",
    "Claude Code",
    "Claude Code-local-oauth",
];

/// A user-facing reason the stats couldn't be fetched, rendered as a card.
pub struct StatsError {
    pub title: String,
    pub message: String,
    /// True when the failure is authentication (expired/missing token, 401) and
    /// the fix is re-logging into Claude Code — renders a step-by-step card.
    pub needs_auth: bool,
}

impl StatsError {
    fn new(title: &str, message: impl Into<String>) -> Self {
        StatsError {
            title: title.to_string(),
            message: message.into(),
            needs_auth: false,
        }
    }

    fn auth() -> Self {
        StatsError {
            title: "Sign in to Claude".to_string(),
            message: "Your Claude login expired".to_string(),
            needs_auth: true,
        }
    }
}

/// Parsed, render-ready usage data.
pub struct ActiveData {
    pub five_hour: Option<UsageWindow>,
    pub seven_day: Option<UsageWindow>,
    /// Per-model weekly windows from the `limits` array (e.g. Fable).
    pub scoped: Vec<ScopedWindow>,
    pub updated_at: Option<String>,
}

#[derive(Clone)]
pub struct UsageWindow {
    pub utilization: f64,
    pub resets_in_minutes: Option<f64>,
    pub usage_level: String,
    pub pace: Option<PaceInfo>,
}

#[derive(Clone)]
pub struct ScopedWindow {
    /// Model display name, e.g. "Fable".
    pub model: String,
    pub utilization: f64,
    pub resets_in_minutes: Option<f64>,
    pub usage_level: String,
    pub is_active: bool,
}

#[derive(Clone)]
pub struct PaceInfo {
    pub delta_percent: f64,
    pub expected_percent: f64,
    pub will_last_to_reset: bool,
    pub eta_minutes: Option<f64>,
}

// ── Raw API shapes ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApiUsage {
    five_hour: Option<ApiWindow>,
    seven_day: Option<ApiWindow>,
    #[serde(default)]
    limits: Vec<ApiLimit>,
}

#[derive(Deserialize)]
struct ApiWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiLimit {
    kind: Option<String>,
    percent: Option<f64>,
    resets_at: Option<String>,
    #[serde(default)]
    is_active: bool,
    scope: Option<ApiScope>,
}

#[derive(Deserialize)]
struct ApiScope {
    model: Option<ApiModel>,
}

#[derive(Deserialize)]
struct ApiModel {
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct KeychainPayload {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<OAuthData>,
}

#[derive(Deserialize)]
struct OAuthData {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<i64>,
}

// ── Token retrieval (keychain, then credentials file) ───────────────────────

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// A token is usable if it has no expiry, or expires more than 30s from now.
fn token_valid(expires_at: Option<i64>) -> bool {
    match expires_at {
        Some(exp) => exp > now_ms() + 30_000,
        None => true,
    }
}

/// (access_token, expiresAt) from a keychain item.
fn keychain_creds(service: &str, user: &str) -> Option<(String, Option<i64>)> {
    let out = Command::new("security")
        .args(["find-generic-password", "-a", user, "-s", service, "-w"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let oauth = serde_json::from_str::<KeychainPayload>(&s).ok()?.claude_ai_oauth?;
    Some((oauth.access_token?, oauth.expires_at))
}

fn credentials_file_creds() -> Option<(String, Option<i64>)> {
    let home = env::var("HOME").ok()?;
    let path = std::path::PathBuf::from(home).join(".claude").join(".credentials.json");
    let contents = fs::read_to_string(path).ok()?;
    let oauth = serde_json::from_str::<KeychainPayload>(&contents).ok()?.claude_ai_oauth?;
    Some((oauth.access_token?, oauth.expires_at))
}

fn creds_from_stores() -> Option<(String, Option<i64>)> {
    let user = env::var("USER").unwrap_or_default();
    for svc in KEYCHAIN_SERVICES {
        if let Some(c) = keychain_creds(svc, &user) {
            return Some(c);
        }
    }
    credentials_file_creds()
}

/// Locate the `claude` CLI. It lives in a version-specific nvm dir that isn't on
/// the daemon's launchd PATH, so search explicitly.
fn claude_bin() -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    if let Ok(path) = env::var("PATH") {
        for dir in path.split(':') {
            let p = Path::new(dir).join("claude");
            if p.exists() {
                return Some(p);
            }
        }
    }
    let home = env::var("HOME").ok()?;
    if let Ok(entries) = fs::read_dir(Path::new(&home).join(".nvm/versions/node")) {
        for e in entries.flatten() {
            let c = e.path().join("bin/claude");
            if c.exists() {
                return Some(c);
            }
        }
    }
    for cand in [
        format!("{home}/.claude/local/claude"),
        "/opt/homebrew/bin/claude".to_string(),
        "/usr/local/bin/claude".to_string(),
    ] {
        if Path::new(&cand).exists() {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

/// Ask Claude Code to refresh its OAuth token (it does so on any invocation),
/// which rewrites the keychain item. No-op if the CLI can't be found.
fn refresh_token() {
    if let Some(bin) = claude_bin() {
        let _ = Command::new(bin).arg("--version").output();
    }
}

fn get_token() -> Option<String> {
    if let Some((tok, exp)) = creds_from_stores() {
        if token_valid(exp) {
            return Some(tok);
        }
    }
    // Expired or missing — trigger a refresh via the Claude Code CLI, then re-read.
    refresh_token();
    creds_from_stores().and_then(|(tok, exp)| token_valid(exp).then_some(tok))
}

// ── Fetch + transform ───────────────────────────────────────────────────────

/// Fetch the raw usage JSON body from the API (returned as a string so it can be
/// cached verbatim).
fn fetch_body(token: &str) -> Result<String, StatsError> {
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(USAGE_API_URL)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json, text/plain, */*")
        .header("Content-Type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header(
            "anthropic-beta",
            "oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14",
        )
        .header("anthropic-dangerous-direct-browser-access", "true")
        .timeout(Duration::from_secs(30))
        .send()
        .map_err(|e| StatsError::new("Not connected", format!("Usage request failed: {e}")))?;

    let status = resp.status();
    let body = resp.text().unwrap_or_default();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(StatsError::auth());
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(StatsError::new("Rate limited", "API 429 — using cached data"));
    }
    if !status.is_success() {
        return Err(StatsError::new("Not connected", format!("API error {status}")));
    }
    Ok(body)
}

// ── On-disk response cache ──────────────────────────────────────────────────
//
// The usage endpoint rate-limits, and the daemon polls it every cycle. Caching
// the raw body (keyed by fetch time) lets us skip the network when fresh and,
// crucially, fall back to the last good response when a fetch fails (e.g. 429)
// instead of blanking the screen. Windows are always recomputed against the
// current time, so a cached body still counts down correctly.

const CACHE_TTL_SECS: u64 = 60;
/// Never fall back to cache older than this on a fetch error — beyond it, show
/// the error card rather than silently displaying stale usage.
const CACHE_STALE_MAX_SECS: u64 = 30 * 60;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cache_path() -> Option<std::path::PathBuf> {
    let home = env::var("HOME").ok()?;
    Some(
        std::path::PathBuf::from(home)
            .join(".cache")
            .join("geekmagic-stats")
            .join("usage.json"),
    )
}

/// (body, age_in_secs) if a cache file exists and parses.
fn read_cache() -> Option<(String, u64)> {
    let raw = fs::read_to_string(cache_path()?).ok()?;
    let (first, body) = raw.split_once('\n')?;
    let at: u64 = first.trim().parse().ok()?;
    Some((body.to_string(), now_secs().saturating_sub(at)))
}

fn write_cache(body: &str) {
    if let Some(path) = cache_path() {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&path, format!("{}\n{}", now_secs(), body));
    }
}

fn usage_level(percent: f64) -> String {
    if percent >= 100.0 {
        "over"
    } else if percent >= 80.0 {
        "danger"
    } else if percent >= 60.0 {
        "warn"
    } else {
        "normal"
    }
    .to_string()
}

fn minutes_until(resets_at: Option<&str>, now: DateTime<Utc>) -> Option<f64> {
    let reset = DateTime::parse_from_rfc3339(resets_at?).ok()?;
    Some(((reset.with_timezone(&Utc) - now).num_seconds() as f64 / 60.0).max(0.0))
}

/// Mirrors claude-code-stats' pace math: how usage tracks against elapsed time.
fn compute_pace(utilization: f64, resets_in_minutes: f64, window_minutes: f64) -> Option<PaceInfo> {
    if window_minutes <= 0.0 || resets_in_minutes <= 0.0 || resets_in_minutes > window_minutes {
        return None;
    }
    let elapsed = (window_minutes - resets_in_minutes) * 60.0;
    let duration = window_minutes * 60.0;
    let time_left = resets_in_minutes * 60.0;

    let actual = utilization.clamp(0.0, 100.0);
    let expected = ((elapsed / duration) * 100.0).clamp(0.0, 100.0);
    if (elapsed == 0.0 && actual > 0.0) || expected < 3.0 {
        return None;
    }

    let delta = actual - expected;
    let (will_last_to_reset, eta_minutes) = if elapsed > 0.0 && actual > 0.0 {
        let rate = actual / elapsed;
        if rate > 0.0 {
            let remaining = (100.0 - actual).max(0.0);
            let candidate = remaining / rate;
            if candidate >= time_left {
                (true, None)
            } else {
                (false, Some(candidate / 60.0))
            }
        } else {
            (true, None)
        }
    } else if elapsed > 0.0 {
        (true, None)
    } else {
        return None;
    };

    Some(PaceInfo {
        delta_percent: delta,
        expected_percent: expected,
        will_last_to_reset,
        eta_minutes,
    })
}

fn build_window(api: &ApiWindow, now: DateTime<Utc>, window_minutes: f64) -> UsageWindow {
    let utilization = api.utilization.unwrap_or(0.0);
    let resets_in_minutes = minutes_until(api.resets_at.as_deref(), now);
    let pace = resets_in_minutes.and_then(|rm| compute_pace(utilization, rm, window_minutes));
    UsageWindow {
        utilization,
        resets_in_minutes,
        usage_level: usage_level(utilization),
        pace,
    }
}

/// Transform a raw usage body into render-ready data (relative to `now`).
fn build_active(body: &str, now: DateTime<Utc>) -> Result<ActiveData, StatsError> {
    let usage: ApiUsage = serde_json::from_str(body)
        .map_err(|e| StatsError::new("Not connected", format!("Bad usage response: {e}")))?;

    let five_hour = usage.five_hour.as_ref().map(|w| build_window(w, now, 300.0));
    let seven_day = usage.seven_day.as_ref().map(|w| build_window(w, now, 10080.0));

    // Per-model weekly windows live in the `limits` array as "weekly_scoped".
    let scoped: Vec<ScopedWindow> = usage
        .limits
        .iter()
        .filter(|l| l.kind.as_deref() == Some("weekly_scoped"))
        .filter_map(|l| {
            let model = l
                .scope
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.clone())?;
            let utilization = l.percent.unwrap_or(0.0);
            Some(ScopedWindow {
                model,
                utilization,
                resets_in_minutes: minutes_until(l.resets_at.as_deref(), now),
                usage_level: usage_level(utilization),
                is_active: l.is_active,
            })
        })
        .collect();

    if five_hour.is_none() && seven_day.is_none() && scoped.is_empty() {
        return Err(StatsError::new("Not connected", "No usage data returned"));
    }

    Ok(ActiveData {
        five_hour,
        seven_day,
        scoped,
        updated_at: Some(now.to_rfc3339()),
    })
}

pub fn fetch_stats() -> Result<ActiveData, StatsError> {
    let now = Utc::now();

    // 1. Serve a fresh cache without touching the network.
    if let Some((body, age)) = read_cache() {
        if age < CACHE_TTL_SECS {
            if let Ok(data) = build_active(&body, now) {
                return Ok(data);
            }
        }
    }

    // 2. Fetch live.
    let live = get_token()
        .ok_or_else(StatsError::auth)
        .and_then(|token| fetch_body(&token));

    match live {
        Ok(body) => {
            let data = build_active(&body, now)?;
            write_cache(&body);
            Ok(data)
        }
        // 3. On a failure (429, network, expired token), fall back to recent
        //    cache to ride out transient errors — but only if it's fresh enough.
        //    Beyond CACHE_STALE_MAX_SECS, surface the error so the screen shows
        //    "Not connected" instead of silently displaying days-old usage.
        Err(e) => read_cache()
            .filter(|(_, age)| *age < CACHE_STALE_MAX_SECS)
            .and_then(|(body, _)| build_active(&body, now).ok())
            .ok_or(e),
    }
}
