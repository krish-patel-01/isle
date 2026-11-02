// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Claude Code asking from the island (Isle's `isle-daemon/src/agents`, Agents1, without D-Bus).
//!
//! Claude Code runs `isle.exe --agent-hook claude` for its hooks. That hook reads Claude's JSON, connects to
//! the running Isle over a local TCP port (its number and a random key in `%LOCALAPPDATA%\Isle\agents.json`,
//! readable by this user alone), and for a permission waits for the island's answer, which it prints for
//! Claude. With no Isle, or no answer, it prints nothing and Claude asks in its terminal as always.

pub mod file;
#[path = "../../../isle-daemon/src/agents/json_hooks.rs"]
pub mod json_hooks;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::json_hooks::Spec;

/// A permission waits this long for the island at most (Claude gives the hook ten minutes).
const ANSWER: Duration = Duration::from_secs(9 * 60 + 40);
/// Connecting and sending a report may take this long at most.
const TIMEOUT: Duration = Duration::from_secs(1);
/// The longest command or path the island asks about; longer, Claude asks in its terminal.
const MAX_SHOWN: usize = 160;
const MAX_INPUT: u64 = 1024 * 1024;
const ANSWERED_EVERY: Duration = Duration::from_secs(1);
const FIND_CALL_FOR: u32 = 6;
const TRANSCRIPT_TAIL: u64 = 1024 * 1024;
const PORT_FILE: &str = "agents.json";

/// Isle's hooks in Claude Code's settings.json (as `isle-daemon/src/agents/claude.rs`).
pub const CLAUDE: Spec = Spec {
    events: &[
        ("PermissionRequest", Some(600)),
        ("Notification", None),
        ("Stop", None),
    ],
    mark: "--agent-hook claude",
    retired: &[],
};

/// Something an AI says: it needs the user, waits, or is done.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Event {
    pub provider: String,
    pub kind: String,
    pub project: String,
    pub cwd: String,
    pub message: String,
    pub session: String,
    pub pids: Vec<u32>,
}

/// A permission the AI asks for: the tool, and what it would run or touch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub provider: String,
    pub project: String,
    pub cwd: String,
    pub session: String,
    pub tool: String,
    pub input: String,
    pub always: bool,
    pub pids: Vec<u32>,
}

/// What a hook sends over the connection: one JSON line.
#[derive(Serialize, Deserialize)]
struct Message {
    key: String,
    #[serde(default)]
    event: Option<Event>,
    #[serde(default)]
    request: Option<Request>,
}

// ── Hooks in Claude Code's settings ─────────────────────────────────────────

/// `$CLAUDE_CONFIG_DIR/settings.json`, else `~/.claude/settings.json`.
pub fn claude_settings() -> PathBuf {
    crate::usage::claude_config_dir().join("settings.json")
}

/// The command each hook runs: this program, its path with forward slashes (Claude Code runs hooks in Git
/// Bash on Windows, where a backslash escapes), quoted when it holds anything else than a plain path.
fn hook_command(ai: &str) -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    let path = exe.to_string_lossy().replace('\\', "/");
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._+-:".contains(c);
    let quoted = if path.chars().all(plain) {
        path
    } else {
        format!("'{}'", path.replace('\'', r"'\''"))
    };
    format!("{quoted} --agent-hook {ai}")
}

pub fn claude_hooked() -> bool {
    json_hooks::hooked(&claude_settings(), &CLAUDE)
}

/// Puts Isle's hooks in Claude Code's settings (beside the user's own, a backup kept), or takes them out.
pub fn set_claude_hooked(on: bool) -> anyhow::Result<()> {
    if on {
        json_hooks::install(&claude_settings(), &hook_command("claude"), &CLAUDE)
    } else {
        json_hooks::remove(&claude_settings(), &CLAUDE).map(drop)
    }
}

// ── The hook, run by Claude Code ────────────────────────────────────────────

/// `isle.exe --agent-hook claude`: passes Claude's hook event on, or asks the island about a permission.
pub fn run_hook(args: &[String]) {
    let ai = match args.first().map(String::as_str) {
        Some("claude") => "claude",
        _ => return,
    };
    let mut input = Vec::new();
    let _ = std::io::stdin().take(MAX_INPUT).read_to_end(&mut input);
    let Ok(payload) = serde_json::from_slice::<Value>(&input) else {
        debug(format_args!("{} bytes on stdin, not JSON", input.len()));
        return;
    };
    debug(format_args!("{} from {ai}", payload["hook_event_name"]));
    if payload["hook_event_name"] == "PermissionRequest" {
        let Some(request) = request(ai, &payload) else {
            debug("too long or hidden characters: Claude asks in its terminal");
            return;
        };
        // Answered in Claude's own terminal first: the hook ends, and with it the island's alert.
        let watched = payload.clone();
        thread::spawn(move || {
            answered_elsewhere(&watched);
            std::process::exit(0);
        });
        if let Some(output) = ask(&request).and_then(|answer| decision(ai, &answer, &payload)) {
            println!("{output}");
        }
        return;
    }
    if let Some(event) = from_claude(&payload) {
        let _ = send(&Message { key: String::new(), event: Some(event), request: None }, false);
    }
}

/// With `ISLE_DEBUG` set, says on stderr what the hook does (Claude Code shows a hook's stderr in its log).
fn debug(text: impl std::fmt::Display) {
    if std::env::var_os("ISLE_DEBUG").is_some() {
        eprintln!("isle hook: {text}");
    }
}

/// Where the running Isle listens, and its key.
fn port_file() -> PathBuf {
    crate::store::data_dir().join(PORT_FILE)
}

/// Sends `message` to the running Isle; with `wait`, returns the answer line it sends back.
fn send(message: &Message, wait: bool) -> Option<String> {
    let Ok(bytes) = std::fs::read(port_file()) else {
        debug(format_args!("no {} (is Isle running?)", port_file().display()));
        return None;
    };
    let info: Value = serde_json::from_slice(&bytes).ok()?;
    let port = u16::try_from(info["port"].as_u64()?).ok()?;
    let key = info["key"].as_str()?.to_owned();
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = match TcpStream::connect_timeout(&address, TIMEOUT) {
        Ok(stream) => stream,
        Err(err) => {
            debug(format_args!("could not reach Isle on port {port}: {err}"));
            return None;
        }
    };
    debug(format_args!("connected on port {port}"));
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    let mut line = serde_json::to_string(&Message { key, event: message.event.clone(), request: message.request.clone() }).ok()?;
    line.push('\n');
    stream.write_all(line.as_bytes()).ok()?;
    if !wait {
        return None;
    }
    stream.set_read_timeout(Some(ANSWER)).ok()?;
    let mut answer = String::new();
    let read = BufReader::new(stream).read_line(&mut answer);
    debug(format_args!("answer {read:?}: {}", answer.trim()));
    read.ok()?;
    let answer: Value = serde_json::from_str(answer.trim()).ok()?;
    answer["answer"].as_str().map(str::to_owned)
}

fn ask(request: &Request) -> Option<String> {
    send(&Message { key: String::new(), event: None, request: Some(request.clone()) }, true)
        .filter(|answer| !answer.is_empty())
}

/// Text with no line breaks or hidden characters, short enough to show whole.
fn shows_whole(text: &str, max: usize) -> bool {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}'
                | '\u{061C}' | '\u{FEFF}' | '\u{00AD}')
    };
    text.chars().count() <= max && !text.chars().any(hidden)
}

fn request(ai: &str, payload: &Value) -> Option<Request> {
    let text = |key: &str| payload[key].as_str().unwrap_or_default();
    let input = &payload["tool_input"];
    let line = ["command", "file_path", "path", "url", "pattern", "query"]
        .iter()
        .find_map(|key| input[key].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| readable(input));
    if !shows_whole(&line, MAX_SHOWN) {
        return None;
    }
    let cwd = if text("cwd").is_empty() {
        std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        text("cwd").to_owned()
    };
    Some(Request {
        provider: ai.into(),
        project: project_of(&cwd),
        cwd,
        pids: ancestors(),
        session: text("session_id").into(),
        tool: text("tool_name").into(),
        input: line,
        always: !rule_suggestions(payload).is_empty(),
        ..Request::default()
    })
}

fn readable(input: &Value) -> String {
    match input {
        Value::Null => String::new(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| match value {
                Value::String(text) => format!("{key}: {text}"),
                other => format!("{key}: {other}"),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn rule_suggestions(payload: &Value) -> Vec<Value> {
    payload["permission_suggestions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|rule| matches!(rule["type"].as_str(), Some("addRules" | "addDirectories")))
        .cloned()
        .collect()
}

/// The answer as Claude's hook output; "always" keeps the rule Claude itself suggested.
fn decision(_ai: &str, answer: &str, payload: &Value) -> Option<String> {
    let decision = match answer {
        "allow" => json!({"behavior": "allow"}),
        "always" => json!({"behavior": "allow", "updatedPermissions": rule_suggestions(payload)}),
        "deny" => json!({"behavior": "deny", "message": "The user denied this from Isle."}),
        _ => return None,
    };
    Some(json!({"hookSpecificOutput": {"hookEventName": "PermissionRequest", "decision": decision}}).to_string())
}

/// Returns once the permission was answered in Claude Code's own terminal (the call's result is in its
/// transcript); never without a transcript or the call in it.
fn answered_elsewhere(payload: &Value) {
    let path = payload["transcript_path"].as_str().map(PathBuf::from);
    let mut found = None;
    for _ in 0..FIND_CALL_FOR {
        found = path
            .as_deref()
            .and_then(|path| pending_call(path, &payload["tool_name"], &payload["tool_input"]));
        if found.is_some() {
            break;
        }
        thread::sleep(ANSWERED_EVERY);
    }
    let (Some(path), Some((id, mut read))) = (path, found) else {
        loop {
            thread::park();
        }
    };
    loop {
        if read_on(&path, &mut read).is_some_and(|text| has_result(&text, &id)) {
            return;
        }
        thread::sleep(ANSWERED_EVERY);
    }
}

fn pending_call(path: &Path, tool: &Value, input: &Value) -> Option<(String, u64)> {
    let (id, read) = find_call(path, tool, input)?;
    let mut probe = read;
    let answered = read_on(path, &mut probe).is_some_and(|text| has_result(&text, &id));
    (!answered).then_some((id, read))
}

fn find_call(path: &Path, tool: &Value, input: &Value) -> Option<(String, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL).read_to_end(&mut bytes).ok()?;
    let mut line_end = bytes.iter().rposition(|&b| b == b'\n')? + 1;
    for line in bytes[..line_end].rsplit(|&b| b == b'\n').skip(1) {
        let next = start + line_end as u64;
        line_end = line_end.saturating_sub(line.len() + 1);
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        if !line.contains("tool_use") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let calls = entry["message"]["content"].as_array().into_iter().flatten();
        for call in calls.filter(|call| call["type"] == "tool_use" && &call["name"] == tool) {
            let same = &call["input"] == input
                || (!input["command"].is_null() && call["input"]["command"] == input["command"]);
            if same && let Some(id) = call["id"].as_str() {
                return Some((id.to_owned(), next));
            }
        }
    }
    None
}

fn read_on(path: &Path, read: &mut u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= *read {
        return None;
    }
    file.seek(SeekFrom::Start(*read)).ok()?;
    let mut bytes = Vec::new();
    file.take(TRANSCRIPT_TAIL).read_to_end(&mut bytes).ok()?;
    let whole = bytes.iter().rposition(|&b| b == b'\n')? + 1;
    bytes.truncate(whole);
    *read += whole as u64;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn has_result(lines: &str, id: &str) -> bool {
    lines
        .lines()
        .any(|line| line.contains(id) && line.contains("tool_result"))
}

/// Claude Code's `Notification` (it waits for the user) and `Stop` (done).
fn from_claude(payload: &Value) -> Option<Event> {
    let text = |key: &str| payload[key].as_str().unwrap_or_default();
    let (kind, message) = match text("hook_event_name") {
        "Notification" => {
            let kind = notification_kind(text("notification_type"), text("message"))?;
            (kind, text("message").to_owned())
        }
        "Stop" => ("done", said(text("last_assistant_message"))),
        _ => return None,
    };
    let cwd = if text("cwd").is_empty() {
        std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        text("cwd").to_owned()
    };
    Some(Event {
        provider: "claude".into(),
        kind: kind.into(),
        project: project_of(&cwd),
        cwd,
        message,
        session: text("session_id").into(),
        pids: ancestors(),
    })
}

fn notification_kind(kind: &str, message: &str) -> Option<&'static str> {
    match kind {
        "permission_prompt" => Some("permission"),
        "idle_prompt" | "elicitation_dialog" => Some("waiting"),
        "auth_success" => None,
        _ if message.to_lowercase().contains("permission") => Some("permission"),
        _ => Some("waiting"),
    }
}

fn said(text: &str) -> String {
    let trimmed = text.trim();
    let fenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.trim_end().strip_suffix("```"))
        .map(str::trim);
    if let Ok(value) = serde_json::from_str::<Value>(fenced.unwrap_or(trimmed))
        && let Some(say) = value.get("say").and_then(Value::as_str)
        && !say.trim().is_empty()
    {
        return first_line(say).to_owned();
    }
    first_line(trimmed).to_owned()
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

fn project_of(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ── Processes: whose window is in front ─────────────────────────────────────

/// This process's parents, nearest first: the AI, its shell, and the terminal or editor it runs in.
fn ancestors() -> Vec<u32> {
    let parents = parent_map();
    let mut pids = Vec::new();
    let mut pid = std::process::id();
    while let Some(&parent) = parents.get(&pid) {
        if parent == 0 || pids.contains(&parent) || pids.len() >= 32 {
            break;
        }
        pids.push(parent);
        pid = parent;
    }
    pids
}

/// Every process's parent.
#[cfg(windows)]
fn parent_map() -> HashMap<u32, u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut map = HashMap::new();
    // SAFETY: a snapshot of the process list, walked and closed here.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snapshot, &mut entry).is_ok();
        while ok {
            map.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            ok = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    map
}

#[cfg(not(windows))]
fn parent_map() -> HashMap<u32, u32> {
    HashMap::new()
}

/// Whether the window in front belongs to one of `pids` (the AI's terminal or editor): then the user is
/// looking at it, and the island stays out of the way.
#[cfg(windows)]
pub fn in_front(pids: &[u32]) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut front = 0u32;
    // SAFETY: reads the process id of the window in front.
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut front));
    }
    front != 0 && front != std::process::id() && pids.contains(&front)
}

#[cfg(not(windows))]
pub fn in_front(_pids: &[u32]) -> bool {
    false
}

// ── The island's side: listening for hooks ──────────────────────────────────

/// Permissions waiting for the island, by id.
#[derive(Default)]
pub struct Pending {
    answers: Mutex<HashMap<String, Sender<String>>>,
}

impl Pending {
    /// Agents1.Resolve: the user's answer ("allow", "always", "deny") to request `id`.
    pub fn resolve(&self, id: &str, answer: &str) {
        if let Some(sender) = self.answers.lock().unwrap().remove(id) {
            let _ = sender.send(answer.to_owned());
        }
    }
}

/// A key no other program can guess: two hashes seeded by the operating system's randomness.
fn new_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        hasher.finish()
    };
    format!("{:016x}{:016x}", half(), half())
}

/// Listens for hooks for as long as Isle runs: `on_event` for news, `on_request` for a permission (whose
/// answer comes through `pending`), `on_gone` when an asking hook ends unanswered.
pub fn serve(
    pending: Arc<Pending>,
    on_event: impl Fn(Event) + Send + Sync + 'static,
    on_request: impl Fn(Request) + Send + Sync + 'static,
    on_gone: impl Fn(String) + Send + Sync + 'static,
) {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(err) => {
            warn!("could not listen for Claude Code's hooks: {err}");
            return;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let key = new_key();
    crate::store::save(PORT_FILE, &json!({"port": port, "key": key}));
    info!("listening for Claude Code's hooks on port {port}");
    let (on_event, on_request, on_gone) = (Arc::new(on_event), Arc::new(on_request), Arc::new(on_gone));
    let mut count = 0u64;
    for stream in listener.incoming().flatten() {
        count += 1;
        let id = format!("{:x}-{count}", crate::timer::now_ms());
        let (key, pending) = (key.clone(), pending.clone());
        let (on_event, on_request, on_gone) = (on_event.clone(), on_request.clone(), on_gone.clone());
        thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(TIMEOUT));
            let mut reader = BufReader::new(match stream.try_clone() {
                Ok(stream) => stream,
                Err(_) => return,
            });
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let Ok(message) = serde_json::from_str::<Message>(line.trim()) else {
                return;
            };
            if message.key != key {
                warn!("a hook came with the wrong key; ignored");
                return;
            }
            if let Some(event) = message.event {
                on_event(event);
                return;
            }
            let Some(mut request) = message.request else {
                return;
            };
            request.id = id.clone();
            let (answer_tx, answer_rx) = mpsc::channel();
            pending.answers.lock().unwrap().insert(id.clone(), answer_tx.clone());
            // The hook going away (answered in the terminal, Claude stopped) ends the wait too. Only a closed
            // connection counts: a read that merely times out (the clone keeps its own timeout) waits on.
            let _ = stream.set_read_timeout(None);
            let _ = reader.get_ref().set_read_timeout(None);
            thread::spawn(move || {
                let mut byte = [0u8; 1];
                loop {
                    match reader.read(&mut byte) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(err) if matches!(err.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(_) => break,
                    }
                }
                let _ = answer_tx.send(String::new());
            });
            // While the user is in the AI's own window it asks there; the island shows it once they leave it.
            let started = std::time::Instant::now();
            let mut early = None;
            while in_front(&request.pids) && started.elapsed() < ANSWER {
                if let Ok(answer) = answer_rx.recv_timeout(Duration::from_millis(500)) {
                    early = Some(answer);
                    break;
                }
            }
            let answer = match early {
                Some(answer) => answer,
                None => {
                    on_request(request);
                    answer_rx
                        .recv_timeout(ANSWER.saturating_sub(started.elapsed()))
                        .unwrap_or_default()
                }
            };
            pending.answers.lock().unwrap().remove(&id);
            if answer.is_empty() {
                on_gone(id);
                return;
            }
            let mut stream = stream;
            let _ = stream.write_all(format!("{}\n", json!({"answer": answer})).as_bytes());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_become_claudes_hook_output() {
        let payload = json!({"permission_suggestions": [{"type": "addRules", "rules": []}, {"type": "setMode"}]});
        let always: Value = serde_json::from_str(&decision("claude", "always", &payload).unwrap()).unwrap();
        assert_eq!(always["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert_eq!(always["hookSpecificOutput"]["decision"]["updatedPermissions"].as_array().unwrap().len(), 1);
        let deny: Value = serde_json::from_str(&decision("claude", "deny", &payload).unwrap()).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(decision("claude", "", &payload).is_none());
    }

    #[test]
    fn a_request_shows_the_command_whole_or_not_at_all() {
        let short = json!({"tool_name": "Bash", "tool_input": {"command": "npm run build"}, "cwd": "C:\\work\\site"});
        let asked = request("claude", &short).unwrap();
        assert_eq!(asked.input, "npm run build");
        assert_eq!(asked.project, "site");
        let long = json!({"tool_name": "Bash", "tool_input": {"command": "echo a\necho b"}});
        assert!(request("claude", &long).is_none());
    }

    #[test]
    fn hooks_go_in_and_come_out() {
        let dir = file::tests::scratch("hooks");
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\n  \"model\": \"opus\"\n}\n").unwrap();
        json_hooks::install(&path, &hook_command("claude"), &CLAUDE).unwrap();
        assert!(json_hooks::hooked(&path, &CLAUDE));
        let settings: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(settings["model"], "opus");
        assert!(json_hooks::remove(&path, &CLAUDE).unwrap());
        assert!(!json_hooks::hooked(&path, &CLAUDE));
    }

    #[test]
    fn a_said_answer_is_its_first_line() {
        assert_eq!(said("```json\n{\"say\": \"Built it.\"}\n```"), "Built it.");
        assert_eq!(said("\n\nFixed the bug.\nMore."), "Fixed the bug.");
    }
}
