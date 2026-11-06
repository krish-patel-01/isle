// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Claude Code (`claude`) in stream-json print mode, with the user's own sign-in (isled's claude.rs, for Windows):
//! one process per conversation, kept to the web tools, without the user's settings, hooks, memory or MCP servers.
//! Isle's job is its system prompt; its answers follow Isle's schema.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc::UnboundedSender;

use super::answer::{self, Answer};

/// The tools it may use: the web, nothing on this computer (Isle looks for it, see tools.rs).
const TOOLS: &str = "WebSearch,WebFetch";
const STDERR_LINES: usize = 20;

/// `%CLAUDE_CONFIG_DIR%`, else `~\.claude`: where Claude Code keeps its sign-in and chats.
pub fn config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"), PathBuf::from)
}

/// Claude Code's program: its own installer's, else on PATH.
pub fn program() -> Option<PathBuf> {
    let installed = dirs::home_dir().map(|home| home.join(r".local\bin\claude.exe"));
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).map(|dir| dir.join("claude.exe")).collect::<Vec<_>>())
        .unwrap_or_default();
    installed.into_iter().chain(on_path).find(|path| path.is_file())
}

/// Whether Claude Code is signed in: its own sign-in, or an API key.
pub fn signed_in() -> bool {
    config_dir().join(".credentials.json").exists()
        || ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
}

/// A running `claude`, the last lines of its stderr kept to explain a failure.
pub struct Claude {
    _child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    pub conversation: Option<String>,
}

impl Claude {
    /// Starts Claude Code with Isle's job (in `instructions`) in place of its coding-agent prompt.
    pub fn start(model: &str, resume: Option<&str>, workspace: &Path, instructions: &Path, schema: &str) -> anyhow::Result<Self> {
        let program = program().context("Claude Code is not installed")?;
        std::fs::create_dir_all(workspace)?;
        let mut command = Command::new(program);
        command
            .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"])
            // Its words as they come, for the island to show while it writes.
            .arg("--include-partial-messages")
            .args(["--tools", TOOLS, "--allowedTools", TOOLS])
            // None of the user's settings, hooks, CLAUDE.md or MCP servers: only its sign-in.
            .args(["--setting-sources", "", "--strict-mcp-config"])
            .arg("--system-prompt-file")
            .arg(instructions)
            .args(["--json-schema", schema]);
        if !model.is_empty() {
            command.args(["--model", model]);
        }
        if let Some(id) = resume {
            command.args(["--resume", id]);
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        command
            .current_dir(workspace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().context("could not start Claude Code")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let mut errors = BufReader::new(child.stderr.take().context("no stderr")?).lines();
        let kept = stderr.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = errors.next_line().await {
                let mut kept = kept.lock().unwrap();
                if kept.len() == STDERR_LINES {
                    kept.pop_front();
                }
                kept.push_back(line);
            }
        });
        Ok(Self { _child: child, stdin, lines, stderr, conversation: resume.map(str::to_owned) })
    }

    /// One turn; an error carries what Claude Code printed to stderr meanwhile.
    pub async fn ask(&mut self, prompt: &str, activity: &UnboundedSender<String>, draft: &UnboundedSender<String>) -> anyhow::Result<Answer> {
        self.stderr.lock().unwrap().clear();
        self.turn(prompt, activity, draft).await.map_err(|err| {
            let stderr = Vec::from(self.stderr.lock().unwrap().clone()).join(" | ");
            if stderr.is_empty() { err } else { err.context(stderr) }
        })
    }

    async fn turn(&mut self, prompt: &str, activity: &UnboundedSender<String>, draft: &UnboundedSender<String>) -> anyhow::Result<Answer> {
        let message = serde_json::json!({"type": "user", "message": {"role": "user", "content": prompt}});
        // Claude Code that quit at once (signed out) still left its reason on stdout: read it then.
        let sent = self.stdin.write_all(format!("{message}\n").as_bytes()).await;
        let _ = self.stdin.flush().await;
        // Its answer is written into the StructuredOutput tool: that JSON, as it comes, is the draft.
        let mut written = String::new();
        let mut shown = String::new();
        while let Some(line) = self.lines.next_line().await? {
            // Tool results, the bulk, are skipped unread.
            if !["\"init\"", "\"result\"", "\"tool_use\"", "\"input_json_delta\"", "content_block_start"].iter().any(|word| line.contains(word)) {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(id) = event["session_id"].as_str() {
                self.conversation = Some(id.to_owned());
            }
            match event["type"].as_str() {
                Some("result") => return parse_result(&event),
                Some("stream_event") => {
                    let delta = &event["event"]["delta"];
                    if delta["type"] == "input_json_delta" {
                        written.push_str(delta["partial_json"].as_str().unwrap_or_default());
                        if let Some(say) = answer::partial_say(&written)
                            && say != shown
                        {
                            draft.send(say.clone()).ok();
                            shown = say;
                        }
                    } else if event["event"]["type"] == "content_block_start" {
                        written.clear();
                    }
                }
                Some("assistant") => {
                    for step in steps(&event) {
                        activity.send(step).ok();
                    }
                }
                _ => {}
            }
        }
        sent.context("Claude Code stopped")?;
        bail!("Claude Code stopped")
    }
}

/// What Claude starts doing in a message, for the island: its web searches and the pages it reads.
fn steps(event: &Value) -> Vec<String> {
    let Some(content) = event["message"]["content"].as_array() else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|part| part["type"] == "tool_use")
        .filter_map(|tool| match tool["name"].as_str()? {
            "WebSearch" => Some(match tool["input"]["query"].as_str() {
                Some(query) => format!("Searching the web for “{query}”…"),
                None => "Searching the web…".into(),
            }),
            "WebFetch" => {
                let url = tool["input"]["url"].as_str().unwrap_or_default();
                let host = url.split("://").nth(1).and_then(|rest| rest.split('/').next());
                Some(format!("Reading {}…", host.unwrap_or("a page")))
            }
            _ => None,
        })
        .collect()
}

/// The answer from the `result` event: its structured output, else its plain text.
fn parse_result(event: &Value) -> anyhow::Result<Answer> {
    if event["is_error"].as_bool() == Some(true) || event["subtype"] != "success" {
        let reason = event["result"].as_str().or(event["error"].as_str()).unwrap_or("Claude Code failed").to_owned();
        bail!("{reason}");
    }
    if let Ok(answer) = Answer::deserialize(&event["structured_output"]) {
        return Ok(answer);
    }
    Ok(answer::from_text(event["result"].as_str().unwrap_or_default()))
}
