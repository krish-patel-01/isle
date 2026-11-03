// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! How much of each AI plan's limits is used, for the Usage page: Isle's `isle-daemon/src/usage.rs` (Usage1)
//! without D-Bus. Claude is asked with Claude Code's own sign-in, ChatGPT with Codex's, each every 10
//! minutes (backing off to an hour after failures); Codex's logs fill in between. Kept in step with the
//! daemon's by hand: the same requests, the same JSON, the same words.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use log::warn;
use serde::Serialize;
use serde_json::Value;

use crate::store;
use crate::timer::now_ms;

const LOCAL_EVERY: Duration = Duration::from_secs(60);
const LIVE_EVERY: Duration = Duration::from_secs(10 * 60);
const BACKOFF: Duration = Duration::from_secs(60 * 60);
/// Asked to refresh (the page opened), a service is asked again only after this long.
const AT_MOST: Duration = Duration::from_secs(2 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);
const OFFLINE_RETRIES: [Duration; 4] = [
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(5 * 60),
];
const CODEX_TAIL: u64 = 256 * 1024;
const CODEX_FILES: usize = 8;
const CODEX_USAGE: &str = "https://chatgpt.com/backend-api/wham/usage";

/// Every plan's limits, as the page draws them.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Usage {
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub plan: String,
    pub windows: Vec<Window>,
    pub updated_ms: i64,
    pub note: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub empty: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Window {
    pub label: String,
    pub percent: f64,
    pub resets_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    NeedsRefresh,
    SignIn,
    Offline,
    Busy,
}

impl Failure {
    fn note(self) -> &'static str {
        match self {
            Failure::NeedsRefresh => "Open Claude Code to refresh its sign-in",
            Failure::SignIn => "Sign in again with Claude Code",
            Failure::Offline => "Offline; showing the last numbers",
            Failure::Busy => "Could not reach Claude; showing the last numbers",
        }
    }
}

/// A wake-up for the reading loop: the page opened and wants fresh numbers.
#[derive(Default)]
pub struct Refresh {
    wanted: Mutex<bool>,
    wake: Condvar,
}

impl Refresh {
    pub fn ask(&self) {
        *self.wanted.lock().unwrap() = true;
        self.wake.notify_one();
    }

    /// Sleeps up to `wait`; returns whether a refresh was asked meanwhile.
    fn sleep(&self, wait: Duration) -> bool {
        let wanted = self.wanted.lock().unwrap();
        let (mut wanted, _) = self.wake.wait_timeout_while(wanted, wait, |w| !*w).unwrap();
        std::mem::take(&mut *wanted)
    }
}

/// What Settings ask of the Usage page (`usage-*`), as the extension sends isled's `SetConfig`.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub enabled: bool,
    pub claude: bool,
    pub codex: bool,
    pub warn: f64,
    pub critical: f64,
}

/// The marks already warned about, by "provider/window/reset": 1 the warning, 2 the critical one.
type Alerted = BTreeMap<String, u8>;
const ALERTED: &str = "usage-alerts.json";

/// Reads every plan for as long as Isle runs, calling `publish` with each change and `alert` (provider,
/// window, percent, reset) as a limit passes Settings' warning marks.
pub fn run(
    refresh: Arc<Refresh>,
    config: impl Fn() -> Config,
    publish: impl Fn(&Usage),
    alert: impl Fn(&str, &str, f64, i64),
) {
    let mut claude = Live::default();
    let mut codex = CodexReader::default();
    let mut last = Usage::default();
    let mut alerted: Alerted = store::load(ALERTED);
    let mut forced = true;
    loop {
        let config = config();
        let mut providers = Vec::new();
        if config.enabled && config.claude && claude_sign_in().is_file() {
            providers.push(claude.read(forced));
        }
        if config.enabled && config.codex
            && let Some(chatgpt) = codex.read(forced)
        {
            providers.push(chatgpt);
        }
        for provider in &mut providers {
            settle(provider, now_ms());
        }
        let warnings = marks_passed(&providers, &config, &mut alerted, now_ms());
        if !warnings.is_empty() {
            store::save(ALERTED, &alerted);
        }
        for (provider, window, percent, resets_at_ms) in warnings {
            alert(&provider, &window, percent, resets_at_ms);
        }
        let usage = Usage { providers };
        if usage != last {
            publish(&usage);
            last = usage;
        }
        forced = refresh.sleep(LOCAL_EVERY.min(claude.wait()));
    }
}

/// A window past its reset is empty again, until it is read anew.
fn settle(provider: &mut Provider, now: i64) {
    for window in &mut provider.windows {
        if window.resets_at_ms > 0 && window.resets_at_ms <= now {
            window.percent = 0.0;
        }
    }
}

/// The marks newly passed, as (provider, window, percent, reset); each is warned about once per window.
fn marks_passed(
    providers: &[Provider],
    config: &Config,
    alerted: &mut Alerted,
    now: i64,
) -> Vec<(String, String, f64, i64)> {
    // Windows that have reset are forgotten; one with no known reset (0) only once it is low again.
    alerted.retain(|key, _| {
        key.rsplit('/')
            .next()
            .and_then(|r| r.parse::<i64>().ok())
            .is_none_or(|r| r == 0 || r > now)
    });
    let mut passed = Vec::new();
    for provider in providers {
        for window in &provider.windows {
            let level = if window.percent >= config.critical {
                2
            } else if window.percent >= config.warn {
                1
            } else {
                0
            };
            // A reset read again a moment off (its fraction of a second changes) is the same window.
            let prefix = format!("{}/{}/", provider.id, window.label);
            let key = alerted
                .keys()
                .find(|key| {
                    key.strip_prefix(&prefix)
                        .and_then(|r| r.parse::<i64>().ok())
                        .is_some_and(|r| (r - window.resets_at_ms).abs() < 60_000)
                })
                .cloned()
                .unwrap_or_else(|| format!("{prefix}{}", window.resets_at_ms));
            if level == 0 && window.resets_at_ms == 0 {
                alerted.remove(&key);
            }
            if level > alerted.get(&key).copied().unwrap_or(0) {
                alerted.insert(key, level);
                passed.push((
                    provider.name.clone(),
                    window.label.clone(),
                    window.percent,
                    window.resets_at_ms,
                ));
            }
        }
    }
    passed
}

/// Which plans this computer is signed in to: (Claude, ChatGPT), for Settings to list.
pub fn found() -> (bool, bool) {
    (claude_sign_in().is_file(), codex_home().join("auth.json").is_file())
}

/// Claude's numbers: asked every 10 minutes, the last answer kept through failures.
#[derive(Default)]
struct Live {
    asked: Option<Instant>,
    last: Option<Provider>,
    failure: Option<Failure>,
    failures: usize,
}

impl Live {
    fn wait(&self) -> Duration {
        let every = match self.failure {
            None => LIVE_EVERY,
            Some(Failure::Offline) => OFFLINE_RETRIES
                .get(self.failures.saturating_sub(1))
                .copied()
                .unwrap_or(LIVE_EVERY),
            Some(_) => BACKOFF,
        };
        self.asked
            .map_or(Duration::ZERO, |at| every.saturating_sub(at.elapsed()))
            .max(Duration::from_secs(1))
    }

    fn read(&mut self, forced: bool) -> Provider {
        let due = self.asked.is_none_or(|at| {
            at.elapsed() >= if forced { AT_MOST } else { LIVE_EVERY }
        }) || (self.failure.is_some() && self.wait() <= Duration::from_secs(1));
        if due {
            self.asked = Some(Instant::now());
            match read_claude() {
                Ok(fresh) => {
                    self.last = Some(fresh);
                    self.failure = None;
                    self.failures = 0;
                }
                Err(failure) => {
                    self.failure = Some(failure);
                    self.failures += 1;
                }
            }
        }
        let mut shown = self.last.clone().unwrap_or_else(claude_shell);
        shown.note = self.failure.map(|f| f.note().to_owned()).unwrap_or_default();
        shown
    }
}

fn read_json(path: &Path) -> Value {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(
            || dirs::home_dir().unwrap_or_default().join(".claude"),
            PathBuf::from,
        )
}

fn claude_sign_in() -> PathBuf {
    claude_config_dir().join(".credentials.json")
}

/// `~/.claude.json`, where Claude Code keeps the account.
fn claude_state() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| dirs::home_dir().unwrap_or_default(), PathBuf::from)
        .join(".claude.json")
}

/// The plan as Claude Code names it ("Max"), from its sign-in and account.
fn claude_plan() -> String {
    let sign_in = read_json(&claude_sign_in());
    let account = &read_json(&claude_state())["oauthAccount"];
    let named = ["plan_display_name", "planDisplayName"].iter().find_map(|key| {
        account[key]
            .as_str()
            .or(account["organization"][key].as_str())
    });
    if let Some(name) = named.filter(|name| !name.is_empty()) {
        return name.to_owned();
    }
    let text = |value: &Value| value.as_str().unwrap_or_default().to_lowercase();
    let mut kind = text(&sign_in["claudeAiOauth"]["subscriptionType"]);
    if kind.is_empty() {
        kind = text(&account["organizationType"])
            .trim_start_matches("claude_")
            .to_owned();
    }
    match kind.as_str() {
        "" => String::new(),
        "pro" => "Pro".into(),
        "max" => "Max".into(),
        "team" => "Team".into(),
        "enterprise" => "Enterprise".into(),
        other => title_case(other),
    }
}

fn claude_shell() -> Provider {
    Provider {
        id: "claude".into(),
        name: "Claude".into(),
        plan: claude_plan(),
        ..Provider::default()
    }
}

/// Claude's limits now, with Claude Code's sign-in (read, never refreshed: that is Claude Code's to do).
fn read_claude() -> Result<Provider, Failure> {
    let text = std::fs::read_to_string(claude_sign_in()).map_err(|_| Failure::SignIn)?;
    let sign_in: Value = serde_json::from_str(&text).map_err(|_| Failure::SignIn)?;
    let oauth = &sign_in["claudeAiOauth"];
    let token = oauth["accessToken"].as_str().ok_or(Failure::SignIn)?;
    if oauth["expiresAt"]
        .as_i64()
        .is_some_and(|expires| expires <= now_ms())
    {
        return Err(Failure::NeedsRefresh);
    }
    let response: Value = crate::http::get("https://api.anthropic.com/api/oauth/usage", TIMEOUT)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        // Said plainly who asks, never another app.
        .header("User-Agent", concat!("Isle/", env!("CARGO_PKG_VERSION")))
        .call()
        .and_then(|mut reply| reply.body_mut().read_json())
        .map_err(|err| {
            warn!("could not read Claude's usage: {err}");
            match err {
                ureq::Error::StatusCode(401 | 403) => Failure::SignIn,
                ureq::Error::StatusCode(_) | ureq::Error::Json(_) => Failure::Busy,
                _ => Failure::Offline,
            }
        })?;
    Ok(Provider {
        windows: claude_windows(&response),
        updated_ms: now_ms(),
        ..claude_shell()
    })
}

/// The limits Claude reports for this plan, in its order: "5 hours", "Week", "Week · Opus"…
fn claude_windows(response: &Value) -> Vec<Window> {
    if let Some(limits) = response["limits"].as_array() {
        return limits
            .iter()
            .filter_map(|limit| {
                let label = match limit["group"].as_str().or(limit["kind"].as_str())? {
                    "session" => "5 hours".to_owned(),
                    "weekly" | "weekly_all" | "weekly_scoped" => "Week".to_owned(),
                    other => title_case(other),
                };
                let model = limit["scope"]["model"]["display_name"]
                    .as_str()
                    .filter(|m| !m.is_empty());
                Some(Window {
                    label: model.map_or(label.clone(), |model| format!("{label} · {model}")),
                    percent: limit["percent"].as_f64()?,
                    resets_at_ms: limit["resets_at"].as_str().map_or(0, rfc3339_ms),
                })
            })
            .collect();
    }
    [
        ("five_hour", "5 hours"),
        ("seven_day", "Week"),
        ("seven_day_opus", "Week · Opus"),
    ]
    .iter()
    .filter_map(|(key, label)| {
        let window = response.get(*key).filter(|w| !w.is_null())?;
        Some(Window {
            label: (*label).into(),
            percent: window["utilization"].as_f64()?,
            resets_at_ms: window["resets_at"].as_str().map_or(0, rfc3339_ms),
        })
    })
    .collect()
}

/// `$CODEX_HOME`, else `~/.codex`.
fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(
            || dirs::home_dir().unwrap_or_default().join(".codex"),
            PathBuf::from,
        )
}

fn codex_plan(plan: &str) -> String {
    match plan.to_lowercase().as_str() {
        "free" => "Free".into(),
        "go" => "Go".into(),
        "plus" => "Plus".into(),
        "prolite" | "pro_lite" => "Pro".into(),
        "pro" => "Pro (More)".into(),
        "promax" | "pro_max" => "Pro (Max)".into(),
        "team" => "Team".into(),
        "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        "edu" => "Edu".into(),
        other => title_case(other),
    }
}

/// ChatGPT's plan and limits: asked as Codex's /status does, and in between as Codex last logged them.
#[derive(Default)]
struct CodexReader {
    seen: Option<(PathBuf, std::time::SystemTime)>,
    last: Option<Provider>,
    live: Option<Provider>,
    asked: Option<Instant>,
}

impl CodexReader {
    fn read(&mut self, forced: bool) -> Option<Provider> {
        if !codex_home().join("auth.json").is_file() {
            return None;
        }
        let due = |every| self.asked.is_none_or(|at: Instant| at.elapsed() >= every);
        if (forced && due(AT_MOST)) || due(LIVE_EVERY) {
            self.asked = Some(Instant::now());
            if let Some(fresh) = read_codex_live() {
                self.live = Some(fresh);
            }
        }
        let logged = self.read_logs();
        match (self.live.clone(), logged) {
            (Some(live), Some(logged)) if logged.updated_ms > live.updated_ms => Some(Provider {
                plan: live.plan,
                ..logged
            }),
            (Some(live), _) => Some(live),
            (None, logged) => logged,
        }
    }

    fn read_logs(&mut self) -> Option<Provider> {
        let mut files = Vec::new();
        newest_logs(&codex_home().join("sessions"), &mut files);
        let modified = |file: &PathBuf| std::fs::metadata(file).and_then(|m| m.modified()).ok();
        files.sort_by_key(|file| std::cmp::Reverse(modified(file)));
        let newest = files
            .first()
            .and_then(|file| Some((file.clone(), modified(file)?)));
        if newest.is_some() && newest == self.seen {
            return self.last.clone();
        }
        self.seen = newest;
        self.last = files
            .iter()
            .take(CODEX_FILES)
            .find_map(|file| codex_limits(file));
        self.last.clone()
    }
}

fn read_codex_live() -> Option<Provider> {
    let text = std::fs::read_to_string(codex_home().join("auth.json")).ok()?;
    let sign_in: Value = serde_json::from_str(&text).ok()?;
    let token = sign_in["tokens"]["access_token"].as_str()?;
    let mut request = crate::http::get(CODEX_USAGE, TIMEOUT)
        .header("Authorization", &format!("Bearer {token}"))
        .header("User-Agent", concat!("Isle/", env!("CARGO_PKG_VERSION")));
    if let Some(account) = sign_in["tokens"]["account_id"].as_str() {
        request = request.header("ChatGPT-Account-Id", account);
    }
    let response: Value = request
        .call()
        .and_then(|mut reply| reply.body_mut().read_json())
        .map_err(|err| warn!("could not read ChatGPT's usage: {err}"))
        .ok()?;
    Some(codex_live(&response, now_ms()))
}

fn codex_live(response: &Value, now: i64) -> Provider {
    let limits = &response["rate_limit"];
    let windows = ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(*key).filter(|w| !w.is_null())?;
            Some(Window {
                label: window_label(window["limit_window_seconds"].as_i64().unwrap_or(0) / 60),
                percent: window["used_percent"].as_f64()?,
                resets_at_ms: window["reset_at"].as_i64().map_or(0, |s| s * 1000),
            })
        })
        .collect();
    Provider {
        id: "codex".into(),
        name: "ChatGPT".into(),
        plan: codex_plan(response["plan_type"].as_str().unwrap_or_default()),
        windows,
        updated_ms: now,
        ..Provider::default()
    }
}

fn codex_limits(file: &Path) -> Option<Provider> {
    use std::io::{Read, Seek, SeekFrom};
    let mut log = std::fs::File::open(file).ok()?;
    let length = log.metadata().ok()?.len();
    log.seek(SeekFrom::Start(length.saturating_sub(CODEX_TAIL)))
        .ok()?;
    let mut tail = Vec::new();
    log.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    let (limits, at) = text.lines().rev().find_map(|line| {
        if !line.contains("rate_limits") {
            return None;
        }
        let event: Value = serde_json::from_str(line).ok()?;
        let limits = find_key(&event, "rate_limits")?.clone();
        Some((limits, event["timestamp"].as_str().map_or(0, rfc3339_ms)))
    })?;
    let modified = std::fs::metadata(file)
        .and_then(|m| m.modified())
        .map(|t| chrono::DateTime::<chrono::Local>::from(t).timestamp_millis())
        .unwrap_or(0);
    Some(Provider {
        id: "codex".into(),
        name: "ChatGPT".into(),
        plan: codex_plan(limits["plan_type"].as_str().unwrap_or_default()),
        windows: codex_windows(&limits),
        updated_ms: if at > 0 { at } else { modified },
        note: "As of Codex’s last use".into(),
        ..Provider::default()
    })
}

fn newest_logs(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort_by(|a, b| b.cmp(a));
    for path in entries {
        if files.len() >= CODEX_FILES {
            return;
        }
        if path.is_dir() {
            newest_logs(&path, files);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            files.push(path);
        }
    }
}

fn codex_windows(limits: &Value) -> Vec<Window> {
    ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let window = limits.get(*key).filter(|w| !w.is_null())?;
            Some(Window {
                label: window_label(window["window_minutes"].as_i64().unwrap_or(0)),
                percent: window["used_percent"].as_f64()?,
                resets_at_ms: window["resets_at"].as_i64().map_or(0, |s| s * 1000),
            })
        })
        .collect()
}

fn window_label(minutes: i64) -> String {
    match minutes {
        300 => "5 hours".to_owned(),
        10080 => "Week".to_owned(),
        m if m > 0 && m % 1440 == 0 => format!("{} days", m / 1440),
        m if m > 0 => format!("{} hours", (m + 30) / 60),
        _ => "Limit".to_owned(),
    }
}

fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .filter(|v| !v.is_null())
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

fn rfc3339_ms(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text).map_or(0, |t| t.timestamp_millis())
}

fn title_case(text: &str) -> String {
    text.split(['_', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or(String::new(), |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn claude_limits_read_in_both_shapes() {
        let new = json!({"limits": [
            {"group": "session", "percent": 12.0, "resets_at": "2026-11-03T05:00:00Z"},
            {"kind": "weekly_scoped", "percent": 40.0, "scope": {"model": {"display_name": "Opus"}}},
        ]});
        let windows = claude_windows(&new);
        assert_eq!(windows[0].label, "5 hours");
        assert!(windows[0].resets_at_ms > 0);
        assert_eq!(windows[1].label, "Week · Opus");
        let old = json!({"five_hour": {"utilization": 3.0}, "seven_day": null});
        assert_eq!(claude_windows(&old).len(), 1);
    }

    #[test]
    fn codex_windows_are_named_as_people_say_them() {
        assert_eq!(window_label(300), "5 hours");
        assert_eq!(window_label(10080), "Week");
        assert_eq!(window_label(43200), "30 days");
        let live = codex_live(
            &json!({"plan_type": "plus", "rate_limit": {"primary_window": {"used_percent": 5.0, "limit_window_seconds": 18000, "reset_at": 10}}}),
            7,
        );
        assert_eq!(live.plan, "Plus");
        assert_eq!(live.windows[0].label, "5 hours");
        assert_eq!(live.windows[0].resets_at_ms, 10_000);
    }

    #[test]
    fn warns_once_per_mark_and_window() {
        let config = Config { enabled: true, claude: true, codex: true, warn: 80.0, critical: 95.0 };
        let mut alerted = Alerted::new();
        let at = |percent: f64| {
            vec![Provider {
                id: "claude".into(),
                name: "Claude".into(),
                windows: vec![Window { label: "5 hours".into(), percent, resets_at_ms: 2_000 }],
                ..Provider::default()
            }]
        };
        assert!(marks_passed(&at(50.0), &config, &mut alerted, 1_000).is_empty());
        assert_eq!(marks_passed(&at(82.0), &config, &mut alerted, 1_000).len(), 1);
        assert!(marks_passed(&at(85.0), &config, &mut alerted, 1_000).is_empty());
        assert_eq!(marks_passed(&at(96.0), &config, &mut alerted, 1_000).len(), 1);
        assert!(marks_passed(&at(99.0), &config, &mut alerted, 1_000).is_empty());
        // Once the window has reset, it may warn again.
        assert!(marks_passed(&[], &config, &mut alerted, 3_000).is_empty() && alerted.is_empty());
    }
}
