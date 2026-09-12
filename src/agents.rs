//! Detect AI agent sessions and judge whether each one is doing anything.
//!
//! A session is the root process of a recognized agent plus everything it
//! spawned. Memory and CPU are summed over that tree.

use crate::groups::app_name;
use crate::procs::{Proc, ProcTable};
use crate::system::home;
use crate::transcripts;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    CursorAgent,
    GeminiCli,
    Aider,
    OpenCode,
    Copilot,
    OpenClaw,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "Claude Code",
            AgentKind::Codex => "Codex",
            AgentKind::CursorAgent => "Cursor Agent",
            AgentKind::GeminiCli => "Gemini CLI",
            AgentKind::Aider => "Aider",
            AgentKind::OpenCode => "OpenCode",
            AgentKind::Copilot => "Copilot CLI",
            AgentKind::OpenClaw => "OpenClaw",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Burning CPU right now.
    Active,
    /// Quiet, but younger than the stale threshold.
    Idle,
    /// Quiet and older than the stale threshold.
    Stale,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentSession {
    pub pid: u32,
    pub kind: AgentKind,
    /// What launched it: a desktop app, an editor, or a terminal.
    pub host: String,
    /// Raw bundle name of the hosting app, if any. Used to keep the rules
    /// from suggesting you quit the app that owns your sessions.
    pub host_app: Option<String>,
    pub cwd: Option<String>,
    /// `cwd` with the home directory shortened to `~`.
    pub project: Option<String>,
    pub age_secs: u64,
    /// Epoch seconds when the root process started. With the pid, this
    /// identifies a session across daemon ticks even if the pid is reused.
    pub start_time: u64,
    /// CPU percent summed over the session's process tree.
    pub cpu: f32,
    /// Resident bytes summed over the session's process tree.
    pub rss: u64,
    pub procs: usize,
    pub state: SessionState,
    /// True when this scan is itself running inside the session.
    pub is_self: bool,
    /// Mean CPU over the daemon's rolling window. None for a one-shot scan.
    pub cpu_window_mean: Option<f32>,
    /// How long the session has been continuously quiet, when the daemon
    /// has been watching it. None when it is busy or not yet windowed.
    pub quiet_for_secs: Option<u64>,
    /// The agent's own session id, when it publishes one.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The best human name there is for the session: a title the user
    /// gave it, else the agent's own name when that is more than a label
    /// derived from the directory, else the first thing the user asked.
    #[serde(default)]
    pub session_name: Option<String>,
    /// The user's own title for the session, when the agent recorded one.
    #[serde(default)]
    pub title: Option<String>,
    /// The first thing the user asked, shortened to one line.
    #[serde(default)]
    pub first_prompt: Option<String>,
    /// Where the transcript lives, when known. What a close action would log
    /// next to the resume command.
    #[serde(default)]
    pub transcript: Option<String>,
    /// Epoch seconds of the last real message in the session's transcript.
    #[serde(default)]
    pub last_activity: Option<u64>,
    /// Seconds since `last_activity`. The strongest idle signal we have.
    #[serde(default)]
    pub idle_secs: Option<u64>,
    /// Every pid in the session's process tree, root first.
    #[serde(default)]
    pub pids: Vec<u32>,
    /// Ports something in the session tree is listening on.
    #[serde(default)]
    pub ports: Vec<u16>,
}

pub struct Detection {
    pub sessions: Vec<AgentSession>,
    /// Every pid that belongs to some session tree.
    pub claimed: HashSet<u32>,
}

fn is_runtime(exe: &str) -> bool {
    let e = exe.to_ascii_lowercase();
    e == "node" || e == "bun" || e == "deno" || e.starts_with("python")
}

/// Whether one of the first script arguments is a file called `name`, on
/// either kind of path separator.
fn script_named(p: &Proc, name: &str) -> bool {
    p.cmd.iter().skip(1).take(2).any(|a| {
        a.strip_suffix(name)
            .map(|dir| dir.ends_with(['/', '\\']))
            .unwrap_or(false)
    })
}

/// Which agent, if any, this single process is. Case-sensitive on the
/// executable name on purpose: `claude` is the CLI, `Claude` is the desktop app.
pub fn classify(p: &Proc) -> Option<AgentKind> {
    let full = p.exe_name();
    let exe = full.strip_suffix(".exe").unwrap_or(&full);
    // Claude Desktop on Windows is also claude.exe; it is the host, not a session.
    if p.exe
        .as_ref()
        .is_some_and(|e| e.to_string_lossy().contains("AnthropicClaude"))
    {
        return None;
    }
    match exe {
        "claude" => return Some(AgentKind::ClaudeCode),
        "codex" => return Some(AgentKind::Codex),
        "cursor-agent" => return Some(AgentKind::CursorAgent),
        "gemini" => return Some(AgentKind::GeminiCli),
        "aider" => return Some(AgentKind::Aider),
        "opencode" => return Some(AgentKind::OpenCode),
        "copilot" => return Some(AgentKind::Copilot),
        "openclaw" => return Some(AgentKind::OpenClaw),
        _ => {}
    }
    if !is_runtime(exe) {
        return None;
    }
    if script_named(p, "claude") || p.cmd_has("@anthropic-ai/claude-code") {
        Some(AgentKind::ClaudeCode)
    } else if p.cmd_has("@openai/codex") {
        Some(AgentKind::Codex)
    } else if p.cmd_has("@google/gemini-cli") || script_named(p, "gemini") {
        Some(AgentKind::GeminiCli)
    } else if exe.to_ascii_lowercase().starts_with("python") && p.cmd_has("aider") {
        Some(AgentKind::Aider)
    } else if p.cmd_has("openclaw") {
        Some(AgentKind::OpenClaw)
    } else if p.cmd_has("@github/copilot") {
        Some(AgentKind::Copilot)
    } else {
        None
    }
}

fn host_label(app: &str) -> String {
    match app {
        "Claude" => "Claude app".to_string(),
        "Code" | "Visual Studio Code" | "Visual Studio Code - Insiders" => "VS Code".to_string(),
        "ChatGPT" => "ChatGPT app".to_string(),
        "Codex" => "Codex app".to_string(),
        other => other.to_string(),
    }
}

fn host_of(table: &ProcTable, p: &Proc) -> (String, Option<String>) {
    for a in table.ancestors(p.pid) {
        if let Some(b) = app_name(a) {
            return (host_label(&b), Some(b));
        }
    }
    ("terminal".to_string(), None)
}

fn shorten_home(path: &str) -> String {
    if let Some(h) = home() {
        let h = h.to_string_lossy();
        if let Some(rest) = path.strip_prefix(h.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// Find every agent session. States are provisional here (instantaneous
/// CPU only); `rules::session_state` makes the real call once transcript and
/// window information are folded in.
pub fn detect(table: &ProcTable, stale_after_secs: u64) -> Detection {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let self_pid = std::process::id();
    let self_chain: HashSet<u32> = table.ancestors(self_pid).iter().map(|p| p.pid).collect();

    let mut sessions = Vec::new();
    let mut claimed = HashSet::new();

    for p in &table.procs {
        let Some(kind) = classify(p) else { continue };
        // Nested agents belong to the outermost session, not their own.
        if table.ancestors(p.pid).iter().any(|a| classify(a).is_some()) {
            continue;
        }
        let desc = table.descendants(p.pid);
        let rss = p.rss + desc.iter().map(|d| d.rss).sum::<u64>();
        let cpu = p.cpu + desc.iter().map(|d| d.cpu).sum::<f32>();
        claimed.insert(p.pid);
        claimed.extend(desc.iter().map(|d| d.pid));

        let claude = if kind == AgentKind::ClaudeCode {
            transcripts::claude_session(p.pid)
        } else {
            None
        };
        let codex = if kind == AgentKind::Codex {
            transcripts::codex_session(p.pid)
        } else {
            None
        };
        let last_activity = claude
            .as_ref()
            .and_then(|c| c.last_activity)
            .or_else(|| codex.as_ref().and_then(|c| c.last_activity));
        let mut cwd = p.cwd.as_ref().map(|c| c.to_string_lossy().into_owned());
        // A Codex server's own cwd is usually `/`; the thread it is working
        // in is the project a person recognises.
        if let Some(c) = codex.as_ref().and_then(|c| c.cwd.clone())
            && cwd.as_deref().is_none_or(|d| d == "/")
        {
            cwd = Some(c);
        }
        let project = cwd.as_deref().map(shorten_home);
        let is_self = p.pid == self_pid || self_chain.contains(&p.pid);
        let state = if cpu >= 2.0 {
            SessionState::Active
        } else if p.run_time >= stale_after_secs {
            SessionState::Stale
        } else {
            SessionState::Idle
        };

        let (host, host_app) = host_of(table, p);
        let title = claude.as_ref().and_then(|c| c.title.clone());
        let first_prompt = claude
            .as_ref()
            .and_then(|c| c.first_prompt.clone())
            .or_else(|| codex.as_ref().and_then(|c| c.first_prompt.clone()));
        let session_name = match (&claude, &codex) {
            (Some(c), _) => c
                .title
                .clone()
                .or_else(|| {
                    c.name
                        .clone()
                        .filter(|_| c.name_source.as_deref() != Some("derived"))
                })
                .or_else(|| c.first_prompt.clone())
                .or_else(|| c.name.clone()),
            (None, Some(c)) => c.first_prompt.clone().or_else(|| {
                Some(format!(
                    "{} open thread{}",
                    c.threads,
                    if c.threads == 1 { "" } else { "s" }
                ))
            }),
            (None, None) => None,
        };
        sessions.push(AgentSession {
            pid: p.pid,
            kind,
            host,
            host_app,
            cwd,
            project,
            age_secs: p.run_time,
            start_time: p.start_time,
            cpu,
            rss,
            procs: 1 + desc.len(),
            state,
            is_self,
            cpu_window_mean: None,
            quiet_for_secs: None,
            session_id: claude.as_ref().map(|c| c.session_id.clone()),
            session_name,
            title,
            first_prompt,
            transcript: claude
                .as_ref()
                .and_then(|c| c.transcript.as_ref())
                .or_else(|| codex.as_ref().and_then(|c| c.transcript.as_ref()))
                .map(|p| p.to_string_lossy().into_owned()),
            last_activity,
            idle_secs: last_activity.map(|t| now.saturating_sub(t)),
            pids: std::iter::once(p.pid)
                .chain(desc.iter().map(|d| d.pid))
                .collect(),
            ports: Vec::new(),
        });
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.age_secs));
    Detection { sessions, claimed }
}
