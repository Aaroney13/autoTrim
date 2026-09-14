//! Roll processes up into the thing a person would actually quit: an app
//! bundle, an agent kind, or a bare executable.
//!
//! What counts as "an app" is the one platform-specific idea here. Each
//! platform's rule is a pure function of the executable path, compiled and
//! tested everywhere; `cfg` only picks which one `app_name` uses.

use crate::agents::{AgentKind, AgentSession, Detection};
use crate::procs::{Proc, ProcTable};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    App,
    Agent,
    Other,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppGroup {
    pub name: String,
    pub kind: GroupKind,
    pub rss: u64,
    /// CPU percent summed over the group's processes.
    #[serde(default)]
    pub cpu: f32,
    /// Process count. For an agent group, every process in every session
    /// tree, plus the folded-in app's.
    pub procs: usize,
    /// Root pids: one per process for an app, one per session for an
    /// agent group, plus the folded-in app's processes.
    pub pids: Vec<u32>,
    /// For an agent group, the agent's own desktop client folded into it:
    /// the Claude app under "Claude Code sessions", ChatGPT under "Codex
    /// sessions". Its memory and processes count here, and quitting it is
    /// offered from this group instead of a second one.
    #[serde(default)]
    pub app: Option<String>,
}

/// Terminal emulators do not absorb their children: a dev server started
/// from a shell is its own thing, not "Terminal". Spelled the way
/// `app_name` reports them on each platform.
const TERMINALS: &[&str] = &[
    "Terminal",
    "iTerm2",
    "iTerm",
    "Warp",
    "Ghostty",
    "Alacritty",
    "kitty",
    "WezTerm",
    "Hyper",
    "wezterm-gui",
    "warp-terminal",
    "gnome-terminal-server",
    "konsole",
    "tilix",
    "terminator",
    "xfce4-terminal",
    "tabby",
    "Windows Terminal",
    "WindowsTerminal",
    "ConEmu64",
    "ConEmu",
];

/// The application a process belongs to, by the platform's own notion of an
/// installed app: the outermost `.app` bundle on macOS, an install root on
/// Linux (`/opt`, `/snap`, Flatpak, AppImage, or a same-named directory
/// under `/usr/lib` or `/usr/share`), Program Files or a per-user Programs
/// directory on Windows. Known browsers and editors are reported under their
/// macOS bundle names on every platform so the rest of the crate matches one
/// spelling.
pub fn app_name(p: &Proc) -> Option<String> {
    let exe = p.exe.as_ref()?;
    #[cfg(target_os = "macos")]
    return macos::app_name(exe);
    #[cfg(target_os = "linux")]
    return linux::app_name(exe);
    #[cfg(target_os = "windows")]
    return windows::app_name(exe);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = exe;
        None
    }
}

/// The bundle an executable runs from, as macOS sees it: the outermost
/// `.app` in the path. Pure, so `actions` can name what to open again and
/// the rule is tested on every host.
pub fn bundle_path(exe: &Path) -> Option<PathBuf> {
    macos::bundle_path(exe)
}

/// Executable or directory names that stand for a known app, and the name
/// the rest of the crate uses for it (its macOS bundle name). Matched
/// case-insensitively, without `.exe`.
const KNOWN: &[(&str, &str)] = &[
    ("chrome", "Google Chrome"),
    ("google-chrome", "Google Chrome"),
    ("google-chrome-stable", "Google Chrome"),
    ("chromium", "Chromium"),
    ("chromium-browser", "Chromium"),
    ("brave", "Brave Browser"),
    ("brave-browser", "Brave Browser"),
    ("msedge", "Microsoft Edge"),
    ("microsoft-edge", "Microsoft Edge"),
    ("vivaldi", "Vivaldi"),
    ("vivaldi-bin", "Vivaldi"),
    ("arc", "Arc"),
    ("firefox", "Firefox"),
    ("firefox-bin", "Firefox"),
    ("code", "Code"),
    ("code-insiders", "Visual Studio Code - Insiders"),
    ("code - insiders", "Visual Studio Code - Insiders"),
    ("cursor", "Cursor"),
    ("chatgpt", "ChatGPT"),
    ("codex", "Codex"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("spotify", "Spotify"),
    ("obsidian", "Obsidian"),
    ("notion", "Notion"),
    ("zoom", "zoom.us"),
    ("windowsterminal", "Windows Terminal"),
];

fn known(name: &str) -> Option<String> {
    KNOWN
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.to_string())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod macos {
    use std::path::{Path, PathBuf};

    /// Name of the outermost `.app` bundle in the executable path, if any.
    pub fn app_name(exe: &Path) -> Option<String> {
        for comp in exe.components() {
            let s = comp.as_os_str().to_string_lossy();
            if let Some(stem) = s.strip_suffix(".app") {
                return Some(stem.to_string());
            }
        }
        None
    }

    /// The path up to and including the outermost `.app` component. None
    /// when the executable is not inside a bundle, or is the bundle
    /// directory itself. Joined as text so the answer is the same on every
    /// host, which is what lets the test run everywhere.
    pub fn bundle_path(exe: &Path) -> Option<PathBuf> {
        let text = exe.to_string_lossy();
        let parts: Vec<&str> = text.split('/').collect();
        let at = parts
            .iter()
            .position(|s| s.strip_suffix(".app").is_some_and(|stem| !stem.is_empty()))?;
        if at + 1 >= parts.len() {
            return None;
        }
        Some(PathBuf::from(parts[..=at].join("/")))
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux {
    use super::known;
    use std::path::Path;

    /// Directory names that never name an app on their own.
    const GENERIC: &[&str] = &[
        "bin",
        "sbin",
        "lib",
        "lib64",
        "libexec",
        "resources",
        "app",
        "usr",
        "share",
        "current",
        "opt",
    ];

    /// Vendor directories under `/usr/lib` and `/usr/share` that hold system
    /// services, not something a person would quit.
    const SYSTEM: &[&str] = &[
        "systemd",
        "snapd",
        "udisks2",
        "upower",
        "packagekit",
        "dbus",
        "dbus-1",
        "polkit-1",
        "xorg",
        "gvfs",
    ];

    /// The most specific directory in `path` that could name an app.
    fn app_dir(path: &str) -> Option<&str> {
        let mut parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop(); // the executable itself
        parts
            .into_iter()
            .rev()
            .find(|d| !GENERIC.contains(&d.to_ascii_lowercase().as_str()))
    }

    pub fn app_name(exe: &Path) -> Option<String> {
        let stem = exe.file_name()?.to_string_lossy();
        if let Some(n) = known(&stem) {
            return Some(n);
        }
        let s = exe.to_string_lossy();
        // Flatpak sandboxes mount the app at /app; AppImages mount under
        // /tmp/.mount_<name><random>. The executable name is the best label.
        if s.starts_with("/app/") || s.starts_with("/tmp/.mount_") {
            return Some(stem.into_owned());
        }
        if let Some(rest) = s.strip_prefix("/snap/") {
            let name = rest.split('/').next()?;
            return Some(known(name).unwrap_or_else(|| name.to_string()));
        }
        if let Some(rest) = s.strip_prefix("/opt/") {
            let dir = app_dir(rest)?;
            return Some(known(dir).unwrap_or_else(|| dir.to_string()));
        }
        // /usr/lib/<x>/<x> and /usr/share/<x>/<x>: an app installed by a
        // package, as long as the directory names the executable.
        for root in [
            "/usr/lib/",
            "/usr/lib64/",
            "/usr/share/",
            "/usr/local/lib/",
            "/usr/local/share/",
        ] {
            let Some(rest) = s.strip_prefix(root) else {
                continue;
            };
            let dir = rest.split('/').next()?;
            if SYSTEM.contains(&dir.to_ascii_lowercase().as_str()) {
                return None;
            }
            if let Some(n) = known(dir) {
                return Some(n);
            }
            if dir.eq_ignore_ascii_case(&stem) {
                return Some(dir.to_string());
            }
            return None;
        }
        None
    }
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows {
    use super::known;
    use std::path::Path;

    /// Parsed by hand so the same code runs in tests on every host: `Path`
    /// only understands backslashes on Windows.
    pub fn app_name(exe: &Path) -> Option<String> {
        let raw = exe.to_string_lossy().replace('/', "\\");
        let file = raw.rsplit('\\').next()?;
        let stem = file
            .strip_suffix(".exe")
            .or_else(|| file.strip_suffix(".EXE"))
            .unwrap_or(file);
        if stem.is_empty() {
            return None;
        }
        let lower = raw.to_ascii_lowercase();
        // Claude Desktop and Claude Code are both claude.exe; only the app
        // lives under AnthropicClaude.
        if lower.contains("\\anthropicclaude\\") {
            return Some("Claude".to_string());
        }
        if let Some(n) = known(stem) {
            return Some(n);
        }
        let installed = lower.contains("\\program files\\")
            || lower.contains("\\program files (x86)\\")
            || lower.contains("\\appdata\\local\\programs\\")
            || lower.contains("\\windowsapps\\")
            // Chrome-style per-user installs: <Vendor>\<App>\Application\x.exe
            || (lower.contains("\\appdata\\local\\") && lower.contains("\\application\\"))
            // Squirrel installs: <App>\app-<version>\x.exe
            || (lower.contains("\\appdata\\local\\") && lower.contains("\\app-"));
        installed.then(|| stem.to_string())
    }
}

fn exe_stem(p: &Proc) -> String {
    let n = p.exe_name();
    if n.is_empty() { p.name.clone() } else { n }
}

fn owner_of(table: &ProcTable, p: &Proc) -> (String, GroupKind) {
    if let Some(b) = app_name(p) {
        return (b, GroupKind::App);
    }
    for a in table.ancestors(p.pid) {
        if let Some(b) = app_name(a) {
            if TERMINALS.contains(&b.as_str()) {
                break;
            }
            return (b, GroupKind::App);
        }
    }
    (exe_stem(p), GroupKind::Other)
}

pub fn group(table: &ProcTable, det: &Detection) -> Vec<AppGroup> {
    let mut map: HashMap<String, AppGroup> = HashMap::new();
    for p in &table.procs {
        if det.claimed.contains(&p.pid) {
            continue;
        }
        let (name, kind) = owner_of(table, p);
        let g = map.entry(name.clone()).or_insert_with(|| AppGroup {
            name,
            kind,
            rss: 0,
            cpu: 0.0,
            procs: 0,
            pids: Vec::new(),
            app: None,
        });
        g.rss += p.rss;
        g.cpu += p.cpu;
        g.procs += 1;
        g.pids.push(p.pid);
    }

    let mut by_kind: HashMap<AgentKind, AppGroup> = HashMap::new();
    for s in &det.sessions {
        let g = by_kind.entry(s.kind).or_insert_with(|| AppGroup {
            name: format!("{} sessions", s.kind.label()),
            kind: GroupKind::Agent,
            rss: 0,
            cpu: 0.0,
            procs: 0,
            pids: Vec::new(),
            app: None,
        });
        g.rss += s.rss;
        g.cpu += s.cpu;
        g.procs += s.procs;
        g.pids.push(s.pid);
    }
    fold_home_apps(&mut map, &mut by_kind);

    let mut out: Vec<AppGroup> = map.into_values().chain(by_kind.into_values()).collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.rss));
    out
}

/// "Claude" next to "Claude Code sessions" is one thing listed twice: the
/// app exists to run the sessions, and quitting it takes them along. Fold
/// each agent's own client into its sessions group. The sessions group
/// keeps the name, because sessions also run in terminals the app knows
/// nothing about. An app with no sessions of its agent stays a plain app.
fn fold_home_apps(
    apps: &mut HashMap<String, AppGroup>,
    by_kind: &mut HashMap<AgentKind, AppGroup>,
) {
    for (kind, g) in by_kind.iter_mut() {
        let Some(home) = kind.home_app() else {
            continue;
        };
        let Some(app) = apps.remove(home) else {
            continue;
        };
        g.rss += app.rss;
        g.cpu += app.cpu;
        g.procs += app.procs;
        g.pids.extend(app.pids);
        g.app = Some(app.name);
    }
}

/// The app's own share of the agent group it was folded into: what
/// `fold_home_apps` added, taken back out. The app verbs act on this, so
/// "quit Claude" still means the app and only the app.
pub fn unfold_app(g: &AppGroup, sessions: &[AgentSession]) -> AppGroup {
    let mine: Vec<&AgentSession> = sessions
        .iter()
        .filter(|s| g.pids.contains(&s.pid))
        .collect();
    let roots: HashSet<u32> = mine.iter().map(|s| s.pid).collect();
    AppGroup {
        name: g.app.clone().unwrap_or_else(|| g.name.clone()),
        kind: GroupKind::App,
        rss: g
            .rss
            .saturating_sub(mine.iter().map(|s| s.rss).sum::<u64>()),
        cpu: (g.cpu - mine.iter().map(|s| s.cpu).sum::<f32>()).max(0.0),
        procs: g
            .procs
            .saturating_sub(mine.iter().map(|s| s.procs).sum::<usize>()),
        pids: g
            .pids
            .iter()
            .copied()
            .filter(|p| !roots.contains(p))
            .collect(),
        app: None,
    }
}

/// Keep the platform rules honest on every host; the Windows one parses
/// strings by hand for exactly this reason.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn mac(p: &str) -> Option<String> {
        macos::app_name(Path::new(p))
    }
    fn lin(p: &str) -> Option<String> {
        linux::app_name(Path::new(p))
    }
    fn win(p: &str) -> Option<String> {
        windows::app_name(Path::new(p))
    }

    fn app(name: &str, rss: u64, pids: &[u32]) -> AppGroup {
        AppGroup {
            name: name.to_string(),
            kind: GroupKind::App,
            rss,
            cpu: 1.0,
            procs: pids.len(),
            pids: pids.to_vec(),
            app: None,
        }
    }

    fn sessions(kind: AgentKind, rss: u64, pids: &[u32]) -> AppGroup {
        AppGroup {
            name: format!("{} sessions", kind.label()),
            kind: GroupKind::Agent,
            rss,
            cpu: 2.0,
            procs: pids.len() * 3,
            pids: pids.to_vec(),
            app: None,
        }
    }

    #[test]
    fn home_app_folds_into_its_sessions() {
        let mut apps = HashMap::new();
        apps.insert("Claude".to_string(), app("Claude", 1_000, &[10, 11]));
        apps.insert("ChatGPT".to_string(), app("ChatGPT", 500, &[20]));
        apps.insert("Code".to_string(), app("Code", 700, &[30]));
        let mut by_kind = HashMap::new();
        by_kind.insert(
            AgentKind::ClaudeCode,
            sessions(AgentKind::ClaudeCode, 300, &[40, 41]),
        );
        by_kind.insert(AgentKind::Codex, sessions(AgentKind::Codex, 100, &[50]));
        by_kind.insert(AgentKind::Copilot, sessions(AgentKind::Copilot, 50, &[60]));
        fold_home_apps(&mut apps, &mut by_kind);

        let cc = &by_kind[&AgentKind::ClaudeCode];
        assert_eq!(cc.name, "Claude Code sessions");
        assert_eq!(cc.app.as_deref(), Some("Claude"));
        assert_eq!(cc.rss, 1_300);
        assert_eq!(cc.cpu, 3.0);
        assert_eq!(cc.procs, 6 + 2);
        assert_eq!(cc.pids, vec![40, 41, 10, 11]);
        assert!(!apps.contains_key("Claude"));

        let codex = &by_kind[&AgentKind::Codex];
        assert_eq!(codex.app.as_deref(), Some("ChatGPT"));
        assert_eq!(codex.rss, 600);
        assert!(!apps.contains_key("ChatGPT"));

        // An editor hosting an agent is not that agent's client.
        assert!(apps.contains_key("Code"));
        assert_eq!(by_kind[&AgentKind::Copilot].app, None);
    }

    fn session(pid: u32, kind: AgentKind, rss: u64, procs: usize) -> AgentSession {
        AgentSession {
            threads: Vec::new(),
            pid,
            kind,
            host: "app".to_string(),
            host_app: kind.home_app().map(str::to_string),
            cwd: None,
            project: None,
            age_secs: 0,
            start_time: 0,
            cpu: 2.0,
            rss,
            procs,
            state: crate::agents::SessionState::Idle,
            is_self: false,
            cpu_window_mean: None,
            quiet_for_secs: None,
            session_id: None,
            session_name: None,
            title: None,
            first_prompt: None,
            transcript: None,
            last_activity: None,
            idle_secs: None,
            pids: vec![pid],
            ports: Vec::new(),
            engine: false,
        }
    }

    #[test]
    fn unfold_returns_the_app_alone() {
        let mut apps = HashMap::new();
        apps.insert("Claude".to_string(), app("Claude", 1_000, &[10, 11]));
        let mut by_kind = HashMap::new();
        let mut g = sessions(AgentKind::ClaudeCode, 0, &[]);
        g.cpu = 0.0;
        let sess = [
            session(40, AgentKind::ClaudeCode, 200, 3),
            session(41, AgentKind::ClaudeCode, 100, 3),
        ];
        for s in &sess {
            g.rss += s.rss;
            g.cpu += s.cpu;
            g.procs += s.procs;
            g.pids.push(s.pid);
        }
        by_kind.insert(AgentKind::ClaudeCode, g);
        fold_home_apps(&mut apps, &mut by_kind);

        let back = unfold_app(&by_kind[&AgentKind::ClaudeCode], &sess);
        assert_eq!(back.name, "Claude");
        assert_eq!(back.kind, GroupKind::App);
        assert_eq!(back.rss, 1_000);
        assert_eq!(back.cpu, 1.0);
        assert_eq!(back.procs, 2);
        assert_eq!(back.pids, vec![10, 11]);
        assert_eq!(back.app, None);
    }

    #[test]
    fn app_without_sessions_stays_an_app() {
        let mut apps = HashMap::new();
        apps.insert("Claude".to_string(), app("Claude", 1_000, &[10]));
        let mut by_kind = HashMap::new();
        by_kind.insert(AgentKind::Codex, sessions(AgentKind::Codex, 100, &[50]));
        fold_home_apps(&mut apps, &mut by_kind);
        assert_eq!(apps["Claude"].rss, 1_000);
        assert_eq!(by_kind[&AgentKind::Codex].app, None);
        assert_eq!(by_kind[&AgentKind::Codex].rss, 100);
    }

    #[test]
    fn macos_outermost_bundle() {
        assert_eq!(
            mac("/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)").as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(mac("/usr/bin/ssh"), None);
    }

    #[test]
    fn macos_bundle_path() {
        let b = |p: &str| bundle_path(Path::new(p));
        let chrome = Some(PathBuf::from("/Applications/Google Chrome.app"));
        assert_eq!(
            b("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            chrome
        );
        // A helper nested in a framework resolves to the outer bundle.
        assert_eq!(
            b(
                "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper (Renderer).app/Contents/MacOS/Google Chrome Helper (Renderer)"
            ),
            chrome
        );
        assert_eq!(
            b("/Users/a/Applications/Slack.app/Contents/MacOS/Slack"),
            Some(PathBuf::from("/Users/a/Applications/Slack.app"))
        );
        assert_eq!(
            b("/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal"),
            Some(PathBuf::from("/System/Applications/Utilities/Terminal.app"))
        );
        for bare in [
            "/usr/bin/ssh",
            "/Users/a/.cargo/bin/autotrim",
            "/opt/homebrew/bin/node",
            "/Applications/Foo.app",
            "/Users/a/my.app.bak/bin/x",
            "/Users/a/notes.apple/x",
        ] {
            assert_eq!(b(bare), None, "{bare}");
        }
    }

    #[test]
    fn linux_known_and_roots() {
        assert_eq!(
            lin("/opt/google/chrome/chrome").as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(
            lin("/opt/google/chrome/chrome_crashpad_handler").as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(
            lin("/opt/brave.com/brave/brave").as_deref(),
            Some("Brave Browser")
        );
        assert_eq!(lin("/opt/vivaldi/vivaldi-bin").as_deref(), Some("Vivaldi"));
        assert_eq!(lin("/opt/Obsidian/obsidian").as_deref(), Some("Obsidian"));
        assert_eq!(lin("/opt/foo/bin/foo").as_deref(), Some("foo"));
        assert_eq!(lin("/usr/share/code/code").as_deref(), Some("Code"));
        assert_eq!(lin("/usr/lib/firefox/firefox").as_deref(), Some("Firefox"));
        assert_eq!(lin("/usr/lib/slack/slack").as_deref(), Some("Slack"));
        assert_eq!(
            lin("/snap/spotify/80/usr/share/spotify/spotify").as_deref(),
            Some("Spotify")
        );
        assert_eq!(lin("/app/extra/chrome").as_deref(), Some("Google Chrome"));
        assert_eq!(
            lin("/tmp/.mount_CursorAb12Cd/usr/bin/cursor").as_deref(),
            Some("Cursor")
        );
    }

    #[test]
    fn linux_system_is_not_an_app() {
        assert_eq!(lin("/usr/lib/systemd/systemd"), None);
        assert_eq!(lin("/usr/lib/systemd/systemd-journald"), None);
        assert_eq!(lin("/usr/lib/polkit-1/polkitd"), None);
        assert_eq!(lin("/usr/bin/node"), None);
        assert_eq!(lin("/home/a/.local/bin/claude"), None);
    }

    #[test]
    fn windows_known_and_roots() {
        assert_eq!(
            win(r"C:\Program Files\Google\Chrome\Application\chrome.exe").as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(
            win(r"C:\Users\a\AppData\Local\Microsoft\Edge\Application\msedge.exe").as_deref(),
            Some("Microsoft Edge")
        );
        assert_eq!(
            win(r"C:\Users\a\AppData\Local\Programs\Microsoft VS Code\Code.exe").as_deref(),
            Some("Code")
        );
        assert_eq!(
            win(r"C:\Users\a\AppData\Local\AnthropicClaude\app-0.9.1\claude.exe").as_deref(),
            Some("Claude")
        );
        assert_eq!(
            win(r"C:\Users\a\AppData\Local\Discord\app-1.0.9\Discord.exe").as_deref(),
            Some("Discord")
        );
        assert_eq!(
            win(r"C:\Program Files\Mozilla Firefox\firefox.exe").as_deref(),
            Some("Firefox")
        );
        assert_eq!(
            win(r"C:\Program Files\Obsidian\Obsidian.exe").as_deref(),
            Some("Obsidian")
        );
        assert_eq!(
            win(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe").as_deref(),
            Some("Windows Terminal")
        );
    }

    #[test]
    fn windows_cli_and_system_are_not_apps() {
        assert_eq!(win(r"C:\Users\a\.local\bin\claude.exe"), None);
        assert_eq!(win(r"C:\Windows\System32\svchost.exe"), None);
        assert_eq!(win(r"C:\Users\a\AppData\Roaming\npm\node.exe"), None);
    }
}
