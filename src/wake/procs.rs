//! The processes of a wake: the signal that the process caught, and the stop of a wake command with its process group (`stop_group`): TERM, a grace,
//! then KILL for what is still the same process.

#[cfg(unix)]
use std::time::Duration;

/// The signal (TERM, INT or HUP) that this process caught since the last call.
#[cfg(unix)]
pub fn caught_signal() -> Option<i32> {
    crate::lock::sys::caught()
}

/// One row of a process snapshot (`ps -axo pid=,ppid=,pgid=,lstart=`). A pid names the same
/// process only while its start time is the same: a pid can be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub pgid: u32,
    pub started: String,
}

/// The rows of a snapshot. A row with no group or start time (`<pid> <ppid>`) has `pgid` 0 and
/// no start time.
pub fn procs_of(snapshot: &str) -> Vec<Proc> {
    snapshot
        .lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            let pid = w.next()?.parse().ok()?;
            let ppid = w.next()?.parse().ok()?;
            let pgid = w.next().and_then(|g| g.parse().ok()).unwrap_or(0);
            Some(Proc {
                pid,
                ppid,
                pgid,
                started: w.collect::<Vec<_>>().join(" "),
            })
        })
        .collect()
}

/// The descendants of `root` in a snapshot of `ps -axo pid=,ppid=` (lines `<pid> <ppid>`), the
/// children first as the snapshot lists them. `root` itself is not in the result.
#[cfg(test)]
pub fn descendants_of(snapshot: &str, root: u32) -> Vec<u32> {
    descendant_procs(&procs_of(snapshot), root)
        .into_iter()
        .map(|p| p.pid)
        .collect()
}

pub(super) fn descendant_procs(rows: &[Proc], root: u32) -> Vec<Proc> {
    let mut out: Vec<Proc> = Vec::new();
    let mut todo = vec![root];
    while let Some(p) = todo.pop() {
        for r in rows {
            if r.ppid == p && r.pid != root && !out.iter().any(|o| o.pid == r.pid) {
                out.push(r.clone());
                todo.push(r.pid);
            }
        }
    }
    out
}

/// The pids of `before` that are still the same process in `now`: same pid and same start time.
/// A pid that is gone, or that a new process has taken, is not a target any more.
pub fn same_processes(before: &[Proc], now: &[Proc]) -> Vec<u32> {
    before
        .iter()
        .filter(|b| {
            now.iter()
                .any(|n| n.pid == b.pid && n.started == b.started && !b.started.is_empty())
        })
        .map(|b| b.pid)
        .collect()
}

/// What `stop_group` asks of the operating system; a test replaces it.
#[cfg(unix)]
pub(super) trait Os {
    /// The process table now; a failure is a note and an empty table.
    fn snapshot(&self, note: &dyn Fn(&str)) -> Vec<Proc>;
    fn signal_group(&self, pgid: u32, sig: i32);
    fn signal_pid(&self, pid: u32, sig: i32);
    fn group_alive(&self, pgid: u32) -> bool;
}

#[cfg(unix)]
struct RealOs;

#[cfg(unix)]
impl Os for RealOs {
    fn snapshot(&self, note: &dyn Fn(&str)) -> Vec<Proc> {
        match crate::lock::procs::ps(&["-axo", "pid=,ppid=,pgid=,lstart="]) {
            Ok(text) => procs_of(&text),
            Err(e) => {
                note(&format!("cannot list the processes of the wake: {e}"));
                Vec::new()
            }
        }
    }
    fn signal_group(&self, pgid: u32, sig: i32) {
        crate::lock::sys::signal_group(pgid, sig);
    }
    fn signal_pid(&self, pid: u32, sig: i32) {
        crate::lock::sys::signal_pid(pid, sig);
    }
    fn group_alive(&self, pgid: u32) -> bool {
        crate::lock::sys::group_alive(pgid)
    }
}

/// Stop the wake: TERM to its process group and to every descendant outside that group, a grace
/// (`$CREWBOARD_KILL_GRACE`, 10 s) while any process of the group lives, so that a
/// `crewboard lock run` in it can release its lock; then KILL what is left; the child is reaped.
/// KILL goes only to a process that is still the one that got the TERM (same pid, same start
/// time), and to the group only while it still has members: a pid that was freed and reused by an
/// unrelated process is left alone. A job that `lock run --detach` started has its own session
/// and no parent: the stop does not reach it.
#[cfg(unix)]
pub(super) fn stop_group(child: &mut std::process::Child, note: &dyn Fn(&str)) {
    let grace: u64 = std::env::var("CREWBOARD_KILL_GRACE")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(10)
        .min(3600);
    stop_group_with(child, note, &RealOs, Duration::from_secs(grace));
}

#[cfg(unix)]
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
pub(super) fn stop_group_with(
    child: &mut std::process::Child,
    note: &dyn Fn(&str),
    os: &dyn Os,
    grace: Duration,
) {
    let pgid = child.id();
    // the tree of the leader and the members of its group, as they are when the TERM goes out
    let table = os.snapshot(note);
    let mut first = descendant_procs(&table, pgid);
    for p in table.iter().filter(|p| p.pgid == pgid && p.pid != pgid) {
        if !first.iter().any(|f| f.pid == p.pid) {
            first.push(p.clone());
        }
    }
    os.signal_group(pgid, libc::SIGTERM);
    // the members of the group have the TERM from the group signal
    for p in first.iter().filter(|p| p.pgid != pgid) {
        os.signal_pid(p.pid, libc::SIGTERM);
    }
    let end = std::time::Instant::now() + grace;
    let mut reaped = false;
    loop {
        reaped = reaped || matches!(child.try_wait(), Ok(Some(_)));
        if reaped && !os.group_alive(pgid) {
            break;
        }
        if std::time::Instant::now() >= end {
            break;
        }
        crate::clock::pause(Duration::from_millis(50));
    }
    // Once the leader is reaped its pid is free: from then on only a process that is still the one
    // that got the TERM (same pid, same start time) is a target. The tree of the leader is read
    // again only while the leader is still ours.
    let now = os.snapshot(note);
    let mut targets = same_processes(&first, &now);
    if !reaped {
        for p in descendant_procs(&now, pgid) {
            if !targets.contains(&p.pid) {
                targets.push(p.pid);
            }
        }
    }
    let in_group = |pid: u32| now.iter().any(|p| p.pid == pid && p.pgid == pgid);
    if !reaped || targets.iter().any(|t| in_group(*t)) {
        os.signal_group(pgid, libc::SIGKILL);
    }
    for pid in targets {
        if !in_group(pid) {
            os.signal_pid(pid, libc::SIGKILL);
        }
    }
    if let Err(e) = child.wait() {
        note(&format!("cannot reap the wake command: {e}"));
    }
}

#[cfg(not(unix))]
pub(super) fn stop_group(child: &mut std::process::Child, _note: &dyn Fn(&str)) {
    child.kill().ok(); // the child may have ended already
    child.wait().ok(); // the child may have ended already
}
