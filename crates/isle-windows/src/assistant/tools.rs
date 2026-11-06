// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The tools the assistant looks with, run by Isle and their results sent back to the AI (isled's check.rs,
//! files.rs and apps.rs, for Windows): a PowerShell command that only looks, finding files and apps, a folder's
//! contents, the start of a text file, what uses the processor. And `run`, for a command that changes the
//! computer, once the user pressed Run.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

/// A check gets this long.
const CHECK_TIMEOUT: Duration = Duration::from_secs(25);
/// The end of a long output, as much as is sent back.
const MAX_OUTPUT: usize = 6000;
/// A command the user ran gets this long.
pub const MAX_RUN: Duration = Duration::from_secs(30 * 60);
/// Finding files stops after this, with what it found.
const FIND_TIME: Duration = Duration::from_secs(6);
const MAX_FOUND: usize = 40;
/// The start of a text file read.
const MAX_READ: usize = 12_000;

/// PowerShell, without the user's profile, never asking anything.
fn powershell() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    command
}

/// The end of `text`, as much as is sent back.
fn tail(text: &str) -> String {
    let text = text.trim();
    if text.len() <= MAX_OUTPUT {
        return text.to_owned();
    }
    let mut start = text.len() - MAX_OUTPUT;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

/// Cmdlet verbs that only look.
const LOOKING_VERBS: &[&str] = &[
    "get", "test", "select", "where", "sort", "format", "measure", "group", "resolve", "compare", "convertto", "split",
    "join",
];
/// PowerShell's approved verbs: a word `verb-noun` with one of these is a cmdlet (Wi-Fi is not).
const VERBS: &[&str] = &[
    "add", "clear", "close", "copy", "enter", "exit", "find", "format", "get", "hide", "join", "lock", "move", "new",
    "open", "optimize", "pop", "push", "redo", "remove", "rename", "reset", "resize", "search", "select", "set", "show",
    "skip", "split", "step", "switch", "undo", "unlock", "watch", "connect", "disconnect", "read", "receive", "send",
    "write", "backup", "checkpoint", "compare", "compress", "convert", "convertfrom", "convertto", "dismount", "edit",
    "expand", "export", "group", "import", "initialize", "limit", "merge", "mount", "out", "publish", "restore", "save",
    "sync", "unpublish", "update", "approve", "assert", "build", "complete", "confirm", "deny", "deploy", "disable",
    "enable", "install", "invoke", "register", "request", "restart", "resume", "start", "stop", "submit", "suspend",
    "uninstall", "unregister", "wait", "debug", "measure", "ping", "repair", "resolve", "test", "trace", "block",
    "grant", "protect", "revoke", "unblock", "unprotect", "use", "where", "sort", "tee", "foreach",
];
/// Cmdlets with a looking verb that still change something, or show what they should not.
const REFUSED_CMDLETS: &[&str] = &["format-volume", "format-hex", "get-credential", "convertto-securestring"];
/// Programs that only look, with the arguments that would change something.
const LOOKING_PROGRAMS: &[(&str, &[&str])] = &[
    ("ipconfig", &["/release", "/renew", "/flushdns", "/registerdns", "/setclassid"]),
    ("systeminfo", &[]),
    ("tasklist", &[]),
    ("whoami", &[]),
    ("hostname", &[]),
    ("where", &[]),
    ("where.exe", &[]),
    ("driverquery", &[]),
    ("nvidia-smi", &["-r", "--gpu-reset", "-pm", "-pl", "-ac", "-rac", "-e", "-c"]),
    ("winget", &["install", "uninstall", "upgrade", "remove", "import", "configure", "settings", "source", "pin", "repair", "update"]),
    ("powercfg", &["/change", "-change", "/setactive", "-setactive", "/s", "-s", "/hibernate", "-h", "/batteryreport", "/energy", "/sleepstudy", "/srumutil", "/systempowerreport", "/delete", "-d", "/import", "/x", "-x", "/setacvalueindex", "/setdcvalueindex", "/duplicatescheme", "/changename"]),
    ("ping", &[]),
    ("tracert", &[]),
    ("nslookup", &[]),
    ("netstat", &[]),
    ("route", &["add", "delete", "change", "-f"]),
    ("arp", &["-d", "-s"]),
    ("pnputil", &["/add-driver", "/delete-driver", "/disable-device", "/enable-device", "/remove-device", "/restart-device", "/scan-devices", "/export-driver", "-a", "-d", "-i"]),
    ("dism", &["/online /cleanup-image /restorehealth", "/add", "/remove", "/enable", "/disable", "/cleanup", "/restorehealth", "/startcomponentcleanup", "/apply", "/set"]),
];
/// netsh looks only with `show`; sc only with `query` and `qc`.
fn netsh_looks(words: &[String]) -> bool {
    words.iter().any(|word| word == "show") && !words.iter().any(|word| ["set", "add", "delete", "reset", "connect", "disconnect", "dump", "exec"].contains(&word.as_str()))
}

/// Why `command` may not run as a check, or None when it only looks: each part of a pipeline is a cmdlet with a
/// looking verb or a looking program; no redirects, script blocks that do things, sub-expressions, calls or
/// several statements.
pub fn refused(command: &str) -> Option<String> {
    let lower = command.to_lowercase();
    for (bad, why) in [
        (">", "redirects"),
        (";", "several commands"),
        ("&&", "several commands"),
        ("||", "several commands"),
        ("$(", "sub-expressions"),
        ("::", ".NET calls"),
        ("`", "escapes"),
        ("invoke-", "Invoke-"),
        ("foreach", "ForEach"),
        ("start-process", "starting programs"),
        (" -command", "nested commands"),
        ("-encodedcommand", "nested commands"),
        ("iex", "Invoke-Expression"),
        ("&", "the call operator"),
        ("%", "ForEach"),
    ] {
        if lower.contains(bad) {
            return Some(format!("no {why} in a check"));
        }
    }
    // .Delete(), .Kill(), .SetValue(): no methods called.
    if regex_lite_method(&lower) {
        return Some("no methods called in a check".into());
    }
    // Every cmdlet named anywhere (also inside { } of Where-Object) must only look.
    for word in lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if let Some((verb, noun)) = word.split_once('-')
            && !verb.is_empty()
            && noun.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && !word.starts_with('-')
            && verb.chars().all(|c| c.is_ascii_alphabetic())
            && VERBS.contains(&verb)
        {
            if REFUSED_CMDLETS.contains(&word) {
                return Some(format!("{word} is not a check"));
            }
            if !LOOKING_VERBS.contains(&verb) && !word.starts_with("out-string") && !is_program_name(word) {
                return Some(format!("{word} changes things; offer it as a run instead"));
            }
        }
    }
    for part in lower.split('|') {
        let words: Vec<String> = part.split_whitespace().map(|word| word.trim_matches(['"', '\'']).to_owned()).collect();
        let Some(first) = words.first() else {
            return Some("an empty part of the pipeline".into());
        };
        let program = first.trim_end_matches(".exe");
        if first.contains('-') && LOOKING_VERBS.contains(&first.split('-').next().unwrap_or_default()) || first == "out-string" {
            continue;
        }
        // Aliases that only look.
        if ["select", "where", "sort", "measure", "group", "?", "gci", "ls", "dir", "gc", "cat", "type", "gps", "ps", "gsv", "gwmi", "gcim", "fl", "ft", "gi", "gp", "gm", "echo"].contains(&first.as_str()) {
            continue;
        }
        if program == "netsh" {
            if netsh_looks(&words) {
                continue;
            }
            return Some("netsh only with show".into());
        }
        if program == "sc" || program == "sc.exe" {
            if words.get(1).is_some_and(|verb| ["query", "qc", "queryex", "qdescription"].contains(&verb.as_str())) {
                continue;
            }
            return Some("sc only with query".into());
        }
        if let Some((_, bad)) = LOOKING_PROGRAMS.iter().find(|(name, _)| *name == program || *name == first) {
            let rest = words[1..].join(" ");
            if let Some(bad) = bad.iter().find(|bad| words[1..].iter().any(|word| word == *bad) || (bad.contains(' ') && rest.contains(*bad))) {
                return Some(format!("{program} {bad} changes things; offer it as a run instead"));
            }
            continue;
        }
        // Any program asked its version.
        if words.len() == 2 && ["--version", "-version", "-v", "/version", "version"].contains(&words[1].as_str()) {
            continue;
        }
        return Some(format!("{first} is not known to only look"));
    }
    None
}

/// A word with a hyphen that is a program (nvidia-smi), not a cmdlet.
fn is_program_name(word: &str) -> bool {
    LOOKING_PROGRAMS.iter().any(|(name, _)| *name == word)
}

/// Whether `.Something(` is in the text: a .NET method called.
fn regex_lite_method(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'.' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && bytes[j] == b'(' && bytes[i + 1].is_ascii_alphabetic() {
                return true;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    false
}

/// A check: runs `command` if it only looks, its output back.
pub async fn check(command: &str) -> Value {
    if let Some(why) = refused(command) {
        return json!({"ok": false, "error": format!("Refused: {why}. Checks are PowerShell cmdlets with Get, Test, \
            Select, Where, Sort, Format, Measure, Group or ConvertTo verbs (piped together is fine), or ipconfig, \
            systeminfo, tasklist, winget list/show/search, powercfg /list /query /a, netsh … show, sc query, \
            driverquery, nvidia-smi, ping, nslookup, or a program's --version.")});
    }
    let mut child = match powershell().arg(format!("{command} | Out-String -Width 200")).spawn() {
        Ok(child) => child,
        Err(err) => return json!({"ok": false, "error": format!("PowerShell would not start: {err}")}),
    };
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let read = async {
        let (mut stdout, mut stderr) = (String::new(), String::new());
        let _ = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        let status = child.wait().await;
        (stdout, stderr, status)
    };
    match tokio::time::timeout(CHECK_TIMEOUT, read).await {
        Ok((stdout, stderr, status)) => {
            let ok = status.is_ok_and(|status| status.success()) && stderr.trim().is_empty();
            let mut output = stdout;
            if !stderr.trim().is_empty() {
                output.push_str("\n[errors]\n");
                output.push_str(&stderr);
            }
            json!({"ok": ok, "output": tail(&output)})
        }
        Err(_) => json!({"ok": false, "error": "it took too long (25 s) and was stopped"}),
    }
}

/// `~`, `~\Downloads`, `%USERPROFILE%\x` and plain paths, as a path.
pub fn expand(path: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    let path = path.trim().trim_matches('"');
    if path.is_empty() || path == "~" {
        return home;
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home.join(rest);
    }
    let mut expanded = path.to_owned();
    for (name, value) in std::env::vars() {
        expanded = expanded.replace(&format!("%{name}%"), &value);
    }
    let named = |name: &str| match name.to_lowercase().as_str() {
        "downloads" => dirs::download_dir(),
        "documents" => dirs::document_dir(),
        "desktop" => dirs::desktop_dir(),
        "pictures" => dirs::picture_dir(),
        "music" => dirs::audio_dir(),
        "videos" => dirs::video_dir(),
        _ => None,
    };
    named(&expanded).unwrap_or_else(|| PathBuf::from(expanded))
}

const KINDS: &[(&str, &[&str])] = &[
    ("image", &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic", "svg", "tif", "tiff"]),
    ("document", &["pdf", "doc", "docx", "odt", "txt", "md", "rtf", "xls", "xlsx", "csv", "ppt", "pptx", "odp", "ods"]),
    ("video", &["mp4", "mkv", "mov", "avi", "webm", "wmv"]),
    ("audio", &["mp3", "wav", "flac", "m4a", "ogg", "aac", "opus"]),
];
/// Folders never looked in: big and never what is meant.
const SKIPPED: &[&str] = &["appdata", "node_modules", ".git", "$recycle.bin", ".cache", ".cargo", ".rustup", "target", ".venv", "venv", "__pycache__", ".npm", ".gradle", "windows", "program files", "program files (x86)", "programdata"];

fn modified(path: &Path) -> SystemTime {
    path.metadata().and_then(|meta| meta.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Files and folders whose names hold any of `terms`, newest first.
pub fn find_files(terms: &[String], within: &str, kind: &str) -> Value {
    let terms: Vec<String> = terms.iter().map(|term| term.to_lowercase()).filter(|term| !term.trim().is_empty()).collect();
    if terms.is_empty() {
        return json!({"ok": false, "error": "no terms to look for"});
    }
    let root = expand(within);
    let extensions = KINDS.iter().find(|(name, _)| *name == kind).map(|(_, extensions)| *extensions);
    let started = Instant::now();
    let mut found: Vec<PathBuf> = Vec::new();
    let mut stack = vec![(root.clone(), 0usize)];
    let mut complete = true;
    while let Some((dir, depth)) = stack.pop() {
        if started.elapsed() > FIND_TIME {
            complete = false;
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let matches = terms.iter().any(|term| name.contains(term.as_str()));
            let fits = match (kind, extensions) {
                ("folder", _) => is_dir,
                (_, Some(extensions)) => !is_dir && extensions.iter().any(|extension| name.ends_with(&format!(".{extension}"))),
                _ => true,
            };
            if matches && fits {
                found.push(entry.path());
            }
            if is_dir && depth < 8 && !name.starts_with('.') && !SKIPPED.contains(&name.as_str()) {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    found.sort_by_key(|path| std::cmp::Reverse(modified(path)));
    found.truncate(MAX_FOUND);
    json!({"ok": true, "in": root, "found": found, "complete": complete})
}

/// A folder's contents, newest first.
pub fn list_folder(path: &str) -> Value {
    let dir = expand(path);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return json!({"ok": false, "error": format!("no folder {}", dir.display())});
    };
    let mut items: Vec<(SystemTime, String)> = entries
        .flatten()
        .map(|entry| {
            let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let name = entry.file_name().to_string_lossy().into_owned();
            (modified(&entry.path()), if folder { format!("{name}\\") } else { name })
        })
        .collect();
    items.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    let total = items.len();
    let names: Vec<String> = items.into_iter().take(80).map(|(_, name)| name).collect();
    json!({"ok": true, "folder": dir, "items": names, "total": total})
}

/// The files the user opened lately (Windows' Recent folder), newest first.
pub fn recent_files() -> Value {
    let recent = dirs::data_dir().unwrap_or_default().join(r"Microsoft\Windows\Recent");
    let Ok(entries) = std::fs::read_dir(&recent) else {
        return json!({"ok": false, "error": "no recent files"});
    };
    let mut items: Vec<(SystemTime, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            Some((modified(&entry.path()), name.strip_suffix(".lnk")?.to_owned()))
        })
        .collect();
    items.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    json!({"ok": true, "items": items.into_iter().take(30).map(|(_, name)| name).collect::<Vec<_>>()})
}

/// The start of a small text file.
pub fn read_text(path: &str) -> Value {
    let path = expand(path);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_READ)]).into_owned();
            if text.chars().filter(|c| *c == '\u{FFFD}' || (c.is_control() && !c.is_whitespace())).count() > text.len() / 20 {
                return json!({"ok": false, "error": "not a plain text file"});
            }
            json!({"ok": true, "text": text, "whole": bytes.len() <= MAX_READ})
        }
        Err(err) => json!({"ok": false, "error": format!("could not read {}: {err}", path.display())}),
    }
}

/// What uses the processor and memory most.
pub async fn top_processes() -> Value {
    check("Get-Process | Sort-Object CPU -Descending | Select-Object -First 15 Name, Id, @{n='CPU s';e={[int]$_.CPU}}, @{n='Memory MB';e={[int]($_.WorkingSet64/1MB)}} | Format-Table -AutoSize").await
}

/// What a command the user let run did.
pub struct Ran {
    pub ok: bool,
    pub code: Option<i32>,
    pub output: String,
}

/// Runs `command` in PowerShell (as administrator when `root`, Windows asking first), up to [`MAX_RUN`].
pub async fn run(command: &str, root: bool, log: &Path) -> Ran {
    let _ = std::fs::create_dir_all(log.parent().unwrap_or(Path::new(".")));
    let _ = std::fs::remove_file(log);
    let script = if root {
        // Elevated, it can't hand back its output: it writes it to the log, read once it ends.
        let inner = format!("& {{ {command} }} *>&1 | Out-File -Encoding utf8 -FilePath '{}'; exit $LASTEXITCODE", log.display());
        let encoded: String = {
            let bytes: Vec<u8> = inner.encode_utf16().flat_map(u16::to_le_bytes).collect();
            base64(&bytes)
        };
        format!(
            "$p = Start-Process powershell.exe -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList \
             '-NoProfile','-ExecutionPolicy','Bypass','-EncodedCommand','{encoded}'; exit $p.ExitCode"
        )
    } else {
        command.to_owned()
    };
    let mut child = match powershell().arg(script).spawn() {
        Ok(child) => child,
        Err(err) => return Ran { ok: false, code: None, output: format!("PowerShell would not start: {err}") },
    };
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let read = async {
        let (mut stdout, mut stderr) = (String::new(), String::new());
        let _ = tokio::join!(out.read_to_string(&mut stdout), err.read_to_string(&mut stderr));
        let status = child.wait().await;
        (stdout, stderr, status)
    };
    match tokio::time::timeout(MAX_RUN, read).await {
        Ok((stdout, stderr, status)) => {
            let mut output = if root { std::fs::read_to_string(log).unwrap_or_default() } else { stdout };
            if !stderr.trim().is_empty() {
                output.push_str("\n[errors]\n");
                output.push_str(&stderr);
            }
            let code = status.ok().and_then(|status| status.code());
            if !root {
                let _ = std::fs::write(log, &output);
            }
            Ran { ok: code == Some(0), code, output: tail(&output) }
        }
        Err(_) => Ran { ok: false, code: None, output: "it ran 30 minutes and was stopped".into() },
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_looking_commands_are_checks() {
        for fine in [
            "Get-Process | Sort-Object CPU -Descending | Select-Object -First 5",
            "Get-CimInstance Win32_Battery",
            "Get-NetAdapter | Where-Object { $_.Status -eq 'Up' }",
            "ipconfig /all",
            "winget list --name vlc",
            "powercfg /getactivescheme",
            "netsh wlan show interfaces",
            "sc query wuauserv",
            "node --version",
            "Get-ItemProperty HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\* | Select-Object DisplayName",
            "nvidia-smi",
            "Get-NetAdapter -Name Wi-Fi",
        ] {
            assert_eq!(refused(fine), None, "{fine}");
        }
        for bad in [
            "Remove-Item C:\\x",
            "Get-Process | Stop-Process",
            "Get-ChildItem | ForEach-Object { $_.Delete() }",
            "Get-Process > out.txt",
            "ipconfig /release",
            "winget install vlc",
            "Get-Item x; Remove-Item x",
            "Get-NetAdapter | Where-Object { Disable-NetAdapter $_ }",
            "[System.IO.File]::Delete('x')",
            "netsh wlan set hostednetwork",
            "Invoke-WebRequest x",
            "cmd /c del x",
            "(Get-Item x).Delete()",
        ] {
            assert!(refused(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn base64_as_powershell_reads_it() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
