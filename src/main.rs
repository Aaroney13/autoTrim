//! Command-line entry point. All the logic lives in the library.

use anyhow::Result;
use autotrim::browser::{BrowserInfo, PageKind, TabInfo};
use autotrim::config::Config;
use autotrim::{actions, app, daemon, fmt, report, service, take_snapshot, watch};
use clap::{Args, Parser, Subcommand};
use std::time::Duration;
use sysinfo::System;

#[derive(Parser)]
#[command(
    name = "autotrim",
    version,
    about = "Notice what is holding your memory, and reclaim it safely."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot report: memory holders, agent sessions, browsers, ports, advice.
    Scan(ScanArgs),
    /// Sample continuously, keep history, notify, and log advice as it changes.
    Daemon(DaemonArgs),
    /// Show the daemon's latest snapshot without sampling anything.
    Status(StatusArgs),
    /// Live terminal view of the latest snapshot and the daemon log.
    Watch(WatchArgs),
    /// Print the daemon log.
    Log(LogArgs),
    /// Close an agent session by pid. Logs the resume command first.
    Close(TargetArgs),
    /// Stop an unmanaged local server by pid.
    Stop(TargetArgs),
    /// List open browser tabs, longest untouched first.
    Tabs(TabsArgs),
    /// Close browser tabs by id, as listed by `tabs`. Logs the URL first.
    CloseTab(CloseTabArgs),
    /// Ask an app to quit, the way ⌘Q would.
    Quit(QuitArgs),
    /// Quit an app the way ⌘Q would, wait for it to exit, and open it again fresh.
    Restart(QuitArgs),
    /// Print the action log: what was closed, when, and how to get it back.
    Actions(ActionsArgs),
    /// Show the effective settings, or write a commented config file.
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    /// Install, remove, restart, or inspect the login service (macOS launchd).
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Open the autoTrim window: start the menu bar app, or bring it forward.
    #[command(alias = "tray")]
    Open,
}

#[derive(Subcommand, Clone)]
enum ConfigCmd {
    /// Write config.toml with every setting and its default, commented.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Change settings in config.toml, keeping the rest of the file as it
    /// is. A running daemon picks the change up on its next tick.
    Set {
        /// key=value pairs, values as TOML: auto_close_sessions=true
        /// auto_grace_minutes=5 auto_hosts='["VS Code"]'
        #[arg(required = true)]
        pairs: Vec<String>,
    },
}

#[derive(Subcommand, Clone, Copy)]
enum ServiceCmd {
    /// Write the launch agent and start it now and at every login.
    Install,
    /// Stop the daemon and remove the launch agent. Data is kept.
    Uninstall,
    /// Restart the running daemon, for example after rebuilding.
    Restart,
    /// Show whether the launch agent is loaded and running.
    Status,
}

#[derive(Args, Clone, Default)]
struct ScanArgs {
    /// Emit the snapshot as JSON instead of text.
    #[arg(long)]
    json: bool,
    /// How long to sample CPU before reporting, in milliseconds.
    #[arg(long)]
    sample_ms: Option<u64>,
    /// A quiet session idle longer than this is reported as stale.
    #[arg(long)]
    stale_after_hours: Option<f64>,
}

#[derive(Args, Clone)]
struct DaemonArgs {
    /// Seconds between samples.
    #[arg(long)]
    interval: Option<u64>,
    /// Rolling window, in seconds, over which a session's CPU is averaged.
    #[arg(long)]
    window: Option<u64>,
    /// Days of history to keep on disk.
    #[arg(long)]
    retention_days: Option<u64>,
    /// A quiet session idle longer than this is reported as stale.
    #[arg(long)]
    stale_after_hours: Option<f64>,
    /// Minutes a session must be observed quiet before it can be called stale.
    #[arg(long)]
    min_quiet_minutes: Option<u64>,
    /// CPU percent below which a session counts as quiet.
    #[arg(long)]
    quiet_cpu: Option<f32>,
    /// Take a single sample, write it, and exit.
    #[arg(long)]
    once: bool,
    /// Do not send native notifications; keep advice in daemon.log.
    #[arg(long)]
    no_notify: bool,
    /// Hours before persisting advice is notified again.
    #[arg(long)]
    remind_every_hours: Option<f64>,
}

#[derive(Args, Clone)]
struct WatchArgs {
    /// Seconds between refreshes.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    /// Lines of daemon log to show under the report.
    #[arg(long, default_value_t = 8)]
    log_lines: usize,
}

#[derive(Args, Clone)]
struct LogArgs {
    /// Lines to print from the end of the log.
    #[arg(short = 'n', long, default_value_t = 50)]
    lines: usize,
    /// Keep printing as the daemon writes.
    #[arg(short = 'f', long)]
    follow: bool,
}

#[derive(Args, Clone)]
struct TargetArgs {
    /// Process id, as shown by scan, status, or watch.
    pid: u32,
    /// Show what would happen without doing it.
    #[arg(long)]
    dry_run: bool,
    /// Act even if the target looks active or managed.
    #[arg(long)]
    force: bool,
}

#[derive(Args, Clone)]
struct TabsArgs {
    /// Only this browser (default: every running one).
    #[arg(long)]
    browser: Option<String>,
    /// Only tabs not looked at for at least this many hours.
    #[arg(long)]
    idle_hours: Option<f64>,
    /// Emit the browsers, with their tabs and sites, as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Clone)]
struct CloseTabArgs {
    /// Tab ids, as printed by `autotrim tabs`.
    #[arg(required = true)]
    ids: Vec<i32>,
    /// The browser the tabs belong to.
    #[arg(long, default_value = "Google Chrome")]
    browser: String,
    /// Show what would happen without doing it.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Clone)]
struct QuitArgs {
    /// The app's name as shown under Top holders, e.g. "Slack".
    app: String,
    /// Show what would happen without doing it.
    #[arg(long)]
    dry_run: bool,
    /// Ask even if the app hosts agent sessions.
    #[arg(long)]
    force: bool,
}

#[derive(Args, Clone)]
struct ActionsArgs {
    /// Entries to print from the end of the log.
    #[arg(short = 'n', long, default_value_t = 20)]
    lines: usize,
}

#[derive(Args, Clone)]
struct StatusArgs {
    /// Emit the snapshot as JSON instead of text.
    #[arg(long)]
    json: bool,
}

fn scan(args: &ScanArgs, cfg: &Config) -> Result<()> {
    let mut thresholds = cfg.thresholds();
    if let Some(h) = args.stale_after_hours {
        thresholds.stale_after_secs = (h * 3600.0) as u64;
    }
    let mut sys = System::new();
    let sample = Duration::from_millis(args.sample_ms.unwrap_or(1500));
    let snap = take_snapshot(&mut sys, Some(sample), &thresholds);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print!("{}", report::render(&snap));
    }
    Ok(())
}

fn run_daemon(args: &DaemonArgs, cfg: &Config) -> Result<()> {
    daemon::run(
        cfg,
        daemon::Overrides {
            interval_secs: args.interval,
            window_secs: args.window,
            retention_days: args.retention_days,
            stale_after_hours: args.stale_after_hours,
            min_quiet_minutes: args.min_quiet_minutes,
            quiet_cpu: args.quiet_cpu,
            no_notify: args.no_notify,
            remind_every_hours: args.remind_every_hours,
            once: args.once,
        },
    )
}

fn close(args: &TargetArgs, cfg: &Config) -> Result<()> {
    let rec = actions::close_by_pid(
        args.pid,
        &cfg.thresholds(),
        args.dry_run,
        args.force,
        "manual",
    )?;
    println!("{}", actions::describe(&rec));
    Ok(())
}

fn stop(args: &TargetArgs, cfg: &Config) -> Result<()> {
    let rec = actions::stop_by_pid(
        args.pid,
        &cfg.thresholds(),
        args.dry_run,
        args.force,
        "manual",
    )?;
    println!("{}", actions::describe(&rec));
    Ok(())
}

fn tabs(args: &TabsArgs, cfg: &Config) -> Result<()> {
    let snap = take_snapshot(
        &mut System::new(),
        Some(Duration::from_millis(300)),
        &cfg.thresholds(),
    );
    let browsers: Vec<&BrowserInfo> = snap
        .browsers
        .iter()
        .filter(|b| args.browser.as_deref().is_none_or(|n| n == b.name))
        .collect();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&browsers)?);
        return Ok(());
    }
    if browsers.is_empty() {
        println!("no running browser found");
    }
    let min_idle = args.idle_hours.map(|h| (h * 3600.0) as u64);
    for b in browsers {
        println!(
            "{} · {} tabs · {} stale · {} conversation{} ({} stale) · {}",
            b.name,
            b.tabs.len(),
            b.stale_tabs,
            b.chat_tabs,
            if b.chat_tabs == 1 { "" } else { "s" },
            b.stale_chat_tabs,
            b.per_tab_estimate
                .map(|e| format!("≈{} per tab (renderers ÷ tabs)", fmt::bytes(e)))
                .unwrap_or_else(|| "no renderer memory to estimate from".to_string())
        );
        if let Some(n) = &b.tabs_note {
            println!("  {n}");
        }
        let label = |dir: &str| {
            b.open_profiles
                .iter()
                .find(|p| p.dir == dir)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| dir.to_string())
        };
        let mut tabs: Vec<&TabInfo> = b
            .tabs
            .iter()
            .filter(|t| min_idle.is_none_or(|m| t.idle_secs.is_some_and(|i| i >= m)))
            .collect();
        tabs.sort_by_key(|t| std::cmp::Reverse(t.idle_secs));
        if tabs.is_empty() {
            continue;
        }
        println!(
            "  {:>10} {:>8}  {:<20} {:<44} SITE",
            "ID", "IDLE", "PROFILE", "TITLE"
        );
        for t in tabs {
            let idle = if t.active {
                "active".to_string()
            } else {
                t.idle_secs.map(fmt::dur).unwrap_or_else(|| "-".to_string())
            };
            println!(
                "  {:>10} {:>8}  {:<20} {:<44} {}{}{}",
                t.id,
                idle,
                fmt::fit_right(&label(&t.profile), 20),
                fmt::fit_right(&t.title, 44),
                t.site,
                match t.kind {
                    PageKind::Chat => "  (conversation)",
                    PageKind::Local => "  (local app)",
                    PageKind::Page => "",
                },
                if t.pinned { "  (pinned)" } else { "" }
            );
        }
    }
    Ok(())
}

fn close_tab(args: &CloseTabArgs, cfg: &Config) -> Result<()> {
    let recs = actions::close_tabs_by_id(
        &args.browser,
        &args.ids,
        &cfg.thresholds(),
        args.dry_run,
        "manual",
    )?;
    for r in recs {
        println!("{}", actions::describe(&r));
    }
    Ok(())
}

fn quit(args: &QuitArgs, cfg: &Config) -> Result<()> {
    let rec = actions::quit_app_by_name(
        &args.app,
        &cfg.thresholds(),
        args.dry_run,
        args.force,
        "manual",
    )?;
    println!("{}", actions::describe(&rec));
    Ok(())
}

fn restart(args: &QuitArgs, cfg: &Config) -> Result<()> {
    let rec = actions::restart_app_by_name(
        &args.app,
        &cfg.thresholds(),
        args.dry_run,
        args.force,
        "manual",
    )?;
    println!("{}", actions::describe(&rec));
    Ok(())
}

fn print_actions(n: usize) -> Result<()> {
    let recs = actions::read_log(n)?;
    if recs.is_empty() {
        println!("no actions yet");
    }
    for r in recs {
        println!("{}", actions::describe(&r));
    }
    Ok(())
}

fn status(args: &StatusArgs) -> Result<()> {
    match daemon::latest()? {
        None => {
            println!(
                "no daemon snapshot yet · run `autotrim daemon` (or `autotrim scan` for a one-shot)"
            );
        }
        Some((snap, age)) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&snap)?);
            } else {
                println!("latest daemon snapshot · {} ago", fmt::dur(age));
                print!("{}", report::render(&snap));
            }
        }
    }
    Ok(())
}

fn show_config(cfg: &Config, from: Option<&std::path::Path>) -> Result<()> {
    match from {
        Some(p) => println!("# from {}", p.display()),
        None => println!(
            "# defaults (no file at {}; `autotrim config init` writes one)",
            Config::path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "?".to_string())
        ),
    }
    print!("{}", toml::to_string_pretty(cfg)?);
    Ok(())
}

fn init_config(force: bool) -> Result<()> {
    let path =
        Config::path().ok_or_else(|| anyhow::anyhow!("no data directory on this platform"))?;
    if path.exists() && !force {
        println!(
            "{} already exists; pass --force to overwrite",
            path.display()
        );
        return Ok(());
    }
    autotrim::storage::write_atomic(&path, Config::template().as_bytes())?;
    println!("wrote {}", path.display());
    Ok(())
}

fn set_config(pairs: &[String]) -> Result<()> {
    let mut kv = Vec::new();
    for p in pairs {
        let Some((k, v)) = p.split_once('=') else {
            anyhow::bail!("expected key=value, got {p}");
        };
        let k = k.trim();
        if !Config::has_key(k) {
            anyhow::bail!("{k} is not a setting; `autotrim config` lists them");
        }
        kv.push((k, v.trim().to_string()));
    }
    let path = Config::set_values(&kv)?;
    println!("wrote {}", path.display());
    for (k, v) in &kv {
        println!("  {k} = {v}");
    }
    println!("A running daemon picks this up on its next tick.");
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Cmd::Daemon(args)) = &cli.cmd {
        autotrim::diagnostics::set_notifications(!args.no_notify);
        autotrim::diagnostics::install_panic_hook();
        let result = Config::load().and_then(|(cfg, _)| run_daemon(args, &cfg));
        if let Err(error) = &result {
            autotrim::diagnostics::fatal(error);
        }
        return result;
    }
    let (cfg, cfg_path) = Config::load()?;
    match cli.cmd.unwrap_or(Cmd::Scan(ScanArgs::default())) {
        Cmd::Scan(args) => scan(&args, &cfg),
        Cmd::Daemon(args) => run_daemon(&args, &cfg),
        Cmd::Status(args) => status(&args),
        Cmd::Watch(args) => watch::run(
            Duration::from_secs(args.interval.max(1)),
            args.log_lines,
            cfg.thresholds(),
        ),
        Cmd::Log(args) => watch::print_log(args.lines, args.follow),
        Cmd::Close(args) => close(&args, &cfg),
        Cmd::Stop(args) => stop(&args, &cfg),
        Cmd::Tabs(args) => tabs(&args, &cfg),
        Cmd::CloseTab(args) => close_tab(&args, &cfg),
        Cmd::Quit(args) => quit(&args, &cfg),
        Cmd::Restart(args) => restart(&args, &cfg),
        Cmd::Actions(args) => print_actions(args.lines),
        Cmd::Config { action } => match action {
            None => show_config(&cfg, cfg_path.as_deref()),
            Some(ConfigCmd::Init { force }) => init_config(force),
            Some(ConfigCmd::Set { pairs }) => set_config(&pairs),
        },
        Cmd::Service { action } => service::run(match action {
            ServiceCmd::Install => service::Action::Install,
            ServiceCmd::Uninstall => service::Action::Uninstall,
            ServiceCmd::Restart => service::Action::Restart,
            ServiceCmd::Status => service::Action::Status,
        }),
        Cmd::Open => app::open(),
    }
}
