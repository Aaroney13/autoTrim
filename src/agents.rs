//! Detect AI agent sessions and judge whether each one is doing anything.
//!
//! A session is the root process of a recognized agent plus everything it
//! spawned. Memory and CPU are summed over that tree. An app's agent engine
//! (Codex's app server under the ChatGPT app or VS Code, Copilot in server
//! mode under VS Code) is one session serving however many threads it holds
//! open, and nothing at all when it holds none.

use crate::groups::app_name;
use crate::openfiles;
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

    /// The agent's own desktop client, by the name `groups::app_name`
    /// reports it. To the person deciding what to quit, the Claude app and
    /// the Claude Code sessions inside it are one thing, so the app is
    /// folded into the sessions group. Editors that happen to host an agent
    /// (VS Code, Cursor) are not this: they are quit for their own reasons.
    pub fn home_app(self) -> Option<&'static str> {
        match self {
            AgentKind::ClaudeCode => Some("Claude"),
            AgentKind::Codex => Some("ChatGPT"),
            _ => None,
        }
    }

    /// What the agent calls one conversation, for "3 open threads".
    pub fn thread_noun(self) -> &'static str {
        match self {
            AgentKind::Codex => "thread",
            AgentKind::CursorAgent => "chat",
            _ => "session",
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
    /// Memory summed over the session's process tree: `phys_footprint` on
    /// macOS, so what closing it returns; resident size elsewhere.
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
    /// True for an app's agent engine (Codex's app server, Copilot in
    /// server mode): it serves the threads its host app shows, and the app
    /// starts it again if it is closed, so auto mode leaves it alone and a
    /// manual close needs `force`.
    #[serde(default)]
    pub engine: bool,
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

/// Whether the process was handed a JavaScript file to run. An Electron
/// app's helper running as node (how VS Code starts its bundled Copilot)
/// looks like this without being called `node`.
fn runs_script(p: &Proc) -> bool {
    p.cmd.iter().skip(1).take(2).any(|a| {
        let a = a.to_ascii_lowercase();
        a.ends_with(".js") || a.ends_with(".mjs") || a.ends_with(".cjs")
    })
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

fn exe_path_has(p: &Proc, needle: &str) -> bool {
    p.exe
        .as_ref()
        .is_some_and(|e| e.to_string_lossy().contains(needle))
}

/// Copilot CLI's npm package, and the per-platform packages that hold the
/// program itself (VS Code bundles one of those). Not
/// `@github/copilot-language-server`, the inline-completion helper.
const COPILOT_PACKAGES: &[&str] = &[
    "@github/copilot/",
    "@github/copilot\\",
    "@github/copilot-darwin",
    "@github/copilot-linux",
    "@github/copilot-win32",
];

/// Which agent, if any, this single process is. Case-sensitive on the
/// executable name on purpose: `claude` is the CLI, `Claude` is the desktop app.
pub fn classify(p: &Proc) -> Option<AgentKind> {
    let full = p.exe_name();
    let exe = full.strip_suffix(".exe").unwrap_or(&full);
    // Claude Desktop on Windows is also claude.exe; it is the host, not a session.
    if exe_path_has(p, "AnthropicClaude") {
        return None;
    }
    match exe {
        "claude" => return Some(AgentKind::ClaudeCode),
        "codex" => return Some(AgentKind::Codex),
        "cursor-agent" | "cursor-agent-sea" => return Some(AgentKind::CursorAgent),
        "gemini" => return Some(AgentKind::GeminiCli),
        "aider" => return Some(AgentKind::Aider),
        "opencode" => return Some(AgentKind::OpenCode),
        "copilot" => return Some(AgentKind::Copilot),
        "openclaw" => return Some(AgentKind::OpenClaw),
        _ => {}
    }
    if !is_runtime(exe) && !runs_script(p) {
        return None;
    }
    if script_named(p, "claude") || p.cmd_has("@anthropic-ai/claude-code") {
        Some(AgentKind::ClaudeCode)
    } else if p.cmd_has("@openai/codex") {
        Some(AgentKind::Codex)
    } else if p.cmd_has("cursor-agent/versions/")
        || p.cmd_has("cursor-agent\\versions\\")
        || (is_runtime(exe) && exe_path_has(p, "cursor-agent"))
    {
        // Cursor's CLI is a launcher that execs its own bundled node on
        // `<install>/cursor-agent/versions/<v>/index.js`, under the name
        // `agent`, which is too plain to trust on its own.
        Some(AgentKind::CursorAgent)
    } else if p.cmd_has("@google/gemini-cli") || script_named(p, "gemini") {
        Some(AgentKind::GeminiCli)
    } else if exe.to_ascii_lowercase().starts_with("python") && p.cmd_has("aider") {
        Some(AgentKind::Aider)
    } else if p.cmd_has("openclaw") {
        Some(AgentKind::OpenClaw)
    } else if COPILOT_PACKAGES.iter().any(|m| p.cmd_has(m)) || script_named(p, "copilot") {
        Some(AgentKind::Copilot)
    } else {
        None
    }
}

/// Whether this process is an app's agent engine rather than a session a
/// person typed into: Codex's app or MCP server, Copilot in server, ACP,
/// or stdio mode.
pub fn is_engine(kind: AgentKind, p: &Proc) -> bool {
    let args = p.cmd.iter().skip(1);
    match kind {
        AgentKind::Codex => args
            .map(|a| a.as_str())
            .any(|a| matches!(a, "app-server" | "mcp-server" | "exec-server")),
        AgentKind::Copilot => args
            .map(|a| a.as_str())
            .any(|a| matches!(a, "--server" | "--acp" | "--stdio")),
        _ => false,
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
        let pids: Vec<u32> = std::iter::once(p.pid)
            .chain(desc.iter().map(|d| d.pid))
            .collect();
        let proc_cwd = p.cwd.as_ref().map(|c| c.to_string_lossy().into_owned());
        let engine = is_engine(kind, p);
        let threads = match kind {
            AgentKind::Codex => transcripts::codex_threads(&pids),
            AgentKind::Copilot => transcripts::copilot_threads(&pids),
            AgentKind::CursorAgent => transcripts::cursor_threads(&pids, proc_cwd.as_deref()),
            _ => None,
        };
        // An engine with nothing open is the host app's helper, not a
        // session; it stays in the app's group. Only when open files can
        // be read, or every engine would vanish.
        if engine && threads.is_none() && openfiles::supported() {
            continue;
        }
        let rss = p.rss + desc.iter().map(|d| d.rss).sum::<u64>();
        let cpu = p.cpu + desc.iter().map(|d| d.cpu).sum::<f32>();
        claimed.extend(pids.iter().copied());

        let claude = if kind == AgentKind::ClaudeCode {
            transcripts::claude_session(p.pid)
        } else {
            None
        };
        let t = threads.as_ref();
        let last_activity = claude
            .as_ref()
            .and_then(|c| c.last_activity)
            .or_else(|| t.and_then(|t| t.last_activity));
        let mut cwd = proc_cwd;
        // An engine's own cwd says nothing about the work (Codex's app
        // server runs in `/`); the thread it is serving is the project a
        // person recognises.
        if let Some(c) = t.and_then(|t| t.cwd.clone())
            && (engine || cwd.as_deref().is_none_or(|d| d == "/"))
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
            .or_else(|| t.and_then(|t| t.first_prompt.clone()));
        let session_name = match (&claude, t) {
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
            (None, Some(t)) => t
                .name
                .clone()
                .or_else(|| t.first_prompt.clone())
                .or_else(|| {
                    Some(format!(
                        "{} open {}{}",
                        t.count,
                        kind.thread_noun(),
                        if t.count == 1 { "" } else { "s" }
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
            session_id: claude
                .as_ref()
                .map(|c| c.session_id.clone())
                .or_else(|| t.and_then(|t| t.session_id.clone())),
            session_name,
            title,
            first_prompt,
            transcript: claude
                .as_ref()
                .and_then(|c| c.transcript.as_ref())
                .or_else(|| t.and_then(|t| t.transcript.as_ref()))
                .map(|p| p.to_string_lossy().into_owned()),
            last_activity,
            idle_secs: last_activity.map(|t| now.saturating_sub(t)),
            pids,
            ports: Vec::new(),
            engine,
        });
    }

    sessions.sort_by_key(|s| std::cmp::Reverse(s.age_secs));
    Detection { sessions, claimed }
}

/// Every agent has a way of showing up that is not its own executable
/// name; these are the shapes seen in the wild, on each platform.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn proc(exe: &str, cmd: &[&str]) -> Proc {
        Proc {
            pid: 1,
            ppid: None,
            name: Path::new(exe)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default(),
            exe: Some(PathBuf::from(exe)),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            rss: 0,
            cpu: 0.0,
            start_time: 0,
            run_time: 0,
        }
    }

    #[test]
    fn codex_binary_and_app_server() {
        let cli = proc("/opt/homebrew/bin/codex", &["codex"]);
        assert_eq!(classify(&cli), Some(AgentKind::Codex));
        assert!(!is_engine(AgentKind::Codex, &cli));
        let engine = proc(
            "/Applications/ChatGPT.app/Contents/Resources/codex",
            &["codex", "-c", "features.code_mode_host=true", "app-server"],
        );
        assert_eq!(classify(&engine), Some(AgentKind::Codex));
        assert!(is_engine(AgentKind::Codex, &engine));
        let npm = proc(
            "/usr/local/bin/node",
            &[
                "node",
                "/usr/local/lib/node_modules/@openai/codex/bin/codex.js",
            ],
        );
        assert_eq!(classify(&npm), Some(AgentKind::Codex));
    }

    #[test]
    fn cursor_cli_is_its_own_node_under_a_plain_name() {
        let cli = proc(
            "/Users/a/.local/share/cursor-agent/versions/2026.09.10-fd3934a/node",
            &[
                "/Users/a/.local/bin/agent",
                "--use-system-ca",
                "/Users/a/.local/share/cursor-agent/versions/2026.09.10-fd3934a/index.js",
                "-p",
                "fix it",
            ],
        );
        assert_eq!(classify(&cli), Some(AgentKind::CursorAgent));
        let sea = proc(
            // Spelled with forward slashes so the test splits the path on
            // every host; Windows accepts both.
            "C:/Users/a/AppData/Local/cursor-agent/cursor-agent-sea.exe",
            &["cursor-agent-sea.exe"],
        );
        assert_eq!(classify(&sea), Some(AgentKind::CursorAgent));
        // `agent` alone is anyone's; so is a node running something else.
        assert_eq!(classify(&proc("/usr/local/bin/agent", &["agent"])), None);
        assert_eq!(classify(&proc("/usr/bin/ssh-agent", &["ssh-agent"])), None);
        assert_eq!(
            classify(&proc("/usr/local/bin/node", &["node", "/srv/app/index.js"])),
            None
        );
    }

    #[test]
    fn copilot_from_npm_and_inside_vs_code() {
        let loader = proc(
            "/opt/homebrew/Cellar/node/25.7.0/bin/node",
            &["node", "/opt/homebrew/bin/copilot"],
        );
        assert_eq!(classify(&loader), Some(AgentKind::Copilot));
        assert!(!is_engine(AgentKind::Copilot, &loader));
        let platform = proc(
            "/opt/homebrew/Cellar/node/25.7.0/bin/node",
            &[
                "node",
                "/opt/homebrew/lib/node_modules/@github/copilot/node_modules/@github/copilot-darwin-arm64/index.js",
            ],
        );
        assert_eq!(classify(&platform), Some(AgentKind::Copilot));
        let vscode = proc(
            "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
            &[
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
                "/Applications/Visual Studio Code.app/Contents/Resources/app/node_modules.asar.unpacked/@github/copilot-darwin-arm64/index.js",
                "--server",
                "--stdio",
            ],
        );
        assert_eq!(classify(&vscode), Some(AgentKind::Copilot));
        assert!(is_engine(AgentKind::Copilot, &vscode));
        let native = proc(
            "C:/Users/a/AppData/Local/Programs/copilot/copilot.exe",
            &["copilot.exe"],
        );
        assert_eq!(classify(&native), Some(AgentKind::Copilot));
    }

    #[test]
    fn copilot_lookalikes_are_not_sessions() {
        let completions = proc(
            "/usr/local/bin/node",
            &[
                "node",
                "/Users/a/.vscode/extensions/github.copilot-1.0/node_modules/@github/copilot-language-server/dist/language-server.js",
                "--stdio",
            ],
        );
        assert_eq!(classify(&completions), None);
        let worker = proc(
            "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
            &[
                "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)",
                "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions/copilot/dist/copilotCLITodoWorker.js",
            ],
        );
        assert_eq!(classify(&worker), None);
        let gh = proc(
            "/Users/a/.local/share/gh/extensions/gh-copilot/gh-copilot",
            &["gh-copilot", "suggest"],
        );
        assert_eq!(classify(&gh), None);
    }

    #[test]
    fn claude_code_is_the_cli_not_the_app() {
        assert_eq!(
            classify(&proc("/Users/a/.local/bin/claude", &["claude"])),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            classify(&proc(
                "/usr/local/bin/node",
                &["node", "/usr/local/bin/claude"]
            )),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(
            classify(&proc(
                r"C:\Users\a\AppData\Local\AnthropicClaude\app-0.9.1\claude.exe",
                &["claude.exe"]
            )),
            None
        );
    }
}
