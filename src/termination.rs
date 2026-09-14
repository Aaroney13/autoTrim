//! Bounded, identity-checked execution. All callers enter through actions.
use serde::{Deserialize, Serialize};
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    Unsupported,
    Failed,
    Sent,
    Gone,
    Changed,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    AlreadyGone,
    Graceful,
    Forced,
    Survived,
    IdentityChanged,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessOutcome {
    pub identity: ProcessIdentity,
    pub term: Delivery,
    pub kill: Option<Delivery>,
    pub exit: Exit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminationOutcome {
    pub processes: Vec<ProcessOutcome>,
}
impl TerminationOutcome {
    pub fn success(&self) -> bool {
        self.processes
            .iter()
            .all(|p| matches!(p.exit, Exit::AlreadyGone | Exit::Graceful | Exit::Forced))
    }
    pub fn describe(&self) -> String {
        if self.success() {
            "terminated: all target processes verified gone".into()
        } else {
            format!(
                "partial failure: unresolved target identities {}",
                self.processes
                    .iter()
                    .filter(|p| matches!(p.exit, Exit::Survived | Exit::IdentityChanged))
                    .map(|p| format!(
                        "{}:{} ({:?})",
                        p.identity.pid, p.identity.start_time, p.exit
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}
trait Processes {
    fn identity(&mut self, pid: u32) -> Option<ProcessIdentity>;
    fn signal(&mut self, target: ProcessIdentity, hard: bool) -> Delivery;
    fn pause(&mut self, duration: Duration);
}
pub(crate) struct Native {
    sys: System,
}
impl Native {
    pub fn new() -> Self {
        Self { sys: System::new() }
    }
}
impl Processes for Native {
    fn identity(&mut self, pid: u32) -> Option<ProcessIdentity> {
        let id = Pid::from_u32(pid);
        self.sys
            .refresh_processes(ProcessesToUpdate::Some(&[id]), true);
        self.sys.process(id).map(|p| ProcessIdentity {
            pid,
            start_time: p.start_time(),
        })
    }
    fn signal(&mut self, target: ProcessIdentity, hard: bool) -> Delivery {
        match self.identity(target.pid) {
            None => return Delivery::Gone,
            Some(current) if current != target => return Delivery::Changed,
            _ => {}
        }
        let Some(p) = self.sys.process(Pid::from_u32(target.pid)) else {
            return Delivery::Gone;
        };
        match if hard {
            Some(p.kill())
        } else {
            p.kill_with(Signal::Term)
        } {
            None => Delivery::Unsupported,
            Some(false) => Delivery::Failed,
            Some(true) => Delivery::Sent,
        }
    }
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
pub(crate) fn capture(sys: &System, pids: &[u32]) -> Vec<ProcessIdentity> {
    pids.iter()
        .filter_map(|pid| {
            sys.process(Pid::from_u32(*pid)).map(|p| ProcessIdentity {
                pid: *pid,
                start_time: p.start_time(),
            })
        })
        .collect()
}
pub(crate) fn terminate(targets: &[ProcessIdentity], wait: Duration) -> TerminationOutcome {
    execute(&mut Native::new(), targets, wait)
}
fn execute(
    adapter: &mut impl Processes,
    targets: &[ProcessIdentity],
    wait: Duration,
) -> TerminationOutcome {
    // Reject the whole selection before sending anything when a captured
    // identity has been replaced. Missing processes are safe to report gone.
    let changed = targets
        .iter()
        .any(|t| adapter.identity(t.pid).is_some_and(|current| current != *t));
    if changed {
        return TerminationOutcome {
            processes: targets
                .iter()
                .map(|&identity| {
                    let (term, exit) = match adapter.identity(identity.pid) {
                        None => (Delivery::Gone, Exit::AlreadyGone),
                        Some(current) if current != identity => {
                            (Delivery::Changed, Exit::IdentityChanged)
                        }
                        Some(_) => (Delivery::Failed, Exit::Survived),
                    };
                    ProcessOutcome {
                        identity,
                        term,
                        kill: None,
                        exit,
                    }
                })
                .collect(),
        };
    }
    let mut out = TerminationOutcome {
        processes: Vec::new(),
    };
    for &identity in targets {
        let term = adapter.signal(identity, false);
        let kill = (term == Delivery::Unsupported).then(|| adapter.signal(identity, true));
        out.processes.push(ProcessOutcome {
            identity,
            term,
            kill,
            exit: Exit::Survived,
        });
    }
    // Wait for the entire selected tree, including orphaned children. Use a
    // bounded number of polls so fake adapters need no wall clock or sleeps.
    let polls = wait.as_millis().div_ceil(100).min(600) as usize;
    poll(adapter, &mut out, polls);
    for p in &mut out.processes {
        if p.exit == Exit::Survived {
            p.kill = Some(adapter.signal(p.identity, true));
        }
    }
    poll(adapter, &mut out, 20);
    out
}
fn poll(adapter: &mut impl Processes, out: &mut TerminationOutcome, polls: usize) {
    for tick in 0..=polls {
        for p in &mut out.processes {
            if matches!(
                p.exit,
                Exit::AlreadyGone | Exit::Graceful | Exit::Forced | Exit::IdentityChanged
            ) {
                continue;
            }
            p.exit = match adapter.identity(p.identity.pid) {
                Some(current) if current != p.identity => Exit::IdentityChanged,
                Some(_) => Exit::Survived,
                None if p.term == Delivery::Gone => Exit::AlreadyGone,
                None if p.kill == Some(Delivery::Sent) => Exit::Forced,
                None if p.term == Delivery::Sent => Exit::Graceful,
                None => Exit::AlreadyGone,
            };
        }
        if out.processes.iter().all(|p| p.exit != Exit::Survived) || tick == polls {
            break;
        }
        adapter.pause(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    #[derive(Default)]
    struct Fake {
        live: HashMap<u32, u64>,
        ignores: HashSet<u32>,
        fails: HashSet<u32>,
        unsupported: bool,
        calls: Vec<(u32, bool)>,
    }
    impl Processes for Fake {
        fn identity(&mut self, pid: u32) -> Option<ProcessIdentity> {
            self.live
                .get(&pid)
                .map(|&start_time| ProcessIdentity { pid, start_time })
        }
        fn signal(&mut self, t: ProcessIdentity, hard: bool) -> Delivery {
            match self.identity(t.pid) {
                None => return Delivery::Gone,
                Some(p) if p != t => return Delivery::Changed,
                _ => {}
            }
            if !hard && self.unsupported {
                return Delivery::Unsupported;
            }
            self.calls.push((t.pid, hard));
            if self.fails.contains(&t.pid) {
                return Delivery::Failed;
            }
            if hard || !self.ignores.contains(&t.pid) {
                self.live.remove(&t.pid);
            }
            Delivery::Sent
        }
        fn pause(&mut self, _: Duration) {}
    }
    #[test]
    fn verifies_children_after_root_exits_and_escalates() {
        let mut f = Fake {
            live: [(1, 10), (2, 10), (3, 10)].into(),
            ignores: [3].into(),
            ..Default::default()
        };
        let targets: Vec<_> = (1..=4)
            .map(|pid| ProcessIdentity {
                pid,
                start_time: 10,
            })
            .collect();
        let o = execute(&mut f, &targets, Duration::from_secs(1));
        assert!(o.success());
        assert_eq!(
            o.processes.iter().map(|p| p.exit).collect::<Vec<_>>(),
            vec![
                Exit::Graceful,
                Exit::Graceful,
                Exit::Forced,
                Exit::AlreadyGone
            ]
        );
        assert!(f.calls.contains(&(3, true)));
    }
    #[test]
    fn failures_reuse_and_unsupported_signals() {
        let mut f = Fake {
            live: [(1, 10), (2, 20)].into(),
            fails: [1].into(),
            ..Default::default()
        };
        let t = [
            ProcessIdentity {
                pid: 1,
                start_time: 10,
            },
            ProcessIdentity {
                pid: 2,
                start_time: 10,
            },
        ];
        let failed = execute(&mut f, &t[..1], Duration::ZERO);
        assert!(!failed.success());
        assert_eq!(failed.processes[0].term, Delivery::Failed);
        assert_eq!(failed.processes[0].kill, Some(Delivery::Failed));
        assert_eq!(f.calls, vec![(1, false), (1, true)]);
        f.calls.clear();
        let o = execute(&mut f, &t, Duration::ZERO);
        assert!(!o.success());
        assert_eq!(o.processes[0].term, Delivery::Failed);
        assert_eq!(o.processes[0].exit, Exit::Survived);
        assert_eq!(o.processes[1].exit, Exit::IdentityChanged);
        assert!(f.calls.is_empty());
        f.fails.clear();
        f.unsupported = true;
        let o = execute(&mut f, &t[..1], Duration::ZERO);
        assert!(o.success());
        assert_eq!(o.processes[0].term, Delivery::Unsupported);
        assert_eq!(o.processes[0].exit, Exit::Forced);
    }
}
