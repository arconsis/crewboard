//! Stopping the wake: the process table and the signals.

use super::*;

/// A process table that a test scripts, and the signals that were sent to it.
struct FakeOs {
    tables: RefCell<std::collections::VecDeque<Vec<Proc>>>,
    group_members: Cell<bool>,
    sent: RefCell<Vec<String>>,
}

impl FakeOs {
    fn new(tables: Vec<Vec<Proc>>, group_members: bool) -> FakeOs {
        FakeOs {
            tables: RefCell::new(tables.into()),
            group_members: Cell::new(group_members),
            sent: RefCell::new(Vec::new()),
        }
    }
    fn sent(&self) -> Vec<String> {
        self.sent.borrow().clone()
    }
}

impl Os for FakeOs {
    fn snapshot(&self, _: &dyn Fn(&str)) -> Vec<Proc> {
        let mut t = self.tables.borrow_mut();
        if t.len() > 1 {
            t.pop_front().unwrap()
        } else {
            t.front().cloned().unwrap_or_default()
        }
    }
    fn signal_group(&self, _: u32, sig: i32) {
        self.sent.borrow_mut().push(format!("group {sig}"));
    }
    fn signal_pid(&self, pid: u32, sig: i32) {
        self.sent.borrow_mut().push(format!("pid {pid} {sig}"));
    }
    fn group_alive(&self, _: u32) -> bool {
        self.group_members.get()
    }
}

fn proc_row(pid: u32, ppid: u32, pgid: u32, started: &str) -> Proc {
    Proc {
        pid,
        ppid,
        pgid,
        started: started.into(),
    }
}

/// A child that has already ended, in a group of its own.
fn ended_child() -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new("sh");
    c.args(["-c", "exit 0"]);
    c.process_group(0);
    c.spawn().unwrap()
}

/// The table of a wake whose leader is `leader`: one grandchild in a session of its own.
fn tree(leader: u32, started: &str) -> Vec<Proc> {
    vec![proc_row(4242, leader, 4242, started)]
}

#[test]
fn when_everything_ends_on_term_no_kill_is_sent() {
    let mut child = ended_child();
    let leader = child.id();
    // a member of the wake's own group (5000) has the TERM from the group signal only
    let mut table = tree(leader, "Sat Oct 24 10:00:00 2026");
    table.push(proc_row(5000, leader, leader, "Sat Oct 24 10:00:01 2026"));
    let os = FakeOs::new(vec![table, Vec::new()], false);
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_secs(5));
    let sent = os.sent();
    assert!(
        sent.iter()
            .all(|s| s.ends_with(&format!(" {}", libc::SIGTERM))),
        "{sent:?}"
    );
    assert!(
        sent.contains(&format!("pid 4242 {}", libc::SIGTERM)),
        "{sent:?}"
    );
    assert!(!sent.iter().any(|s| s.starts_with("pid 5000")), "{sent:?}");
}

#[test]
fn a_pid_that_another_process_took_is_not_killed_one_that_is_the_same_is() {
    // the grandchild ignores TERM: the same process (same start time) gets the KILL
    let mut child = ended_child();
    let leader = child.id();
    let t = tree(leader, "Sat Oct 24 10:00:00 2026");
    let os = FakeOs::new(vec![t.clone(), t.clone()], false);
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    assert!(
        os.sent().contains(&format!("pid 4242 {}", libc::SIGKILL)),
        "{:?}",
        os.sent()
    );
    // a stranger has the pid now (other start time): no KILL to it, none to the group
    let mut child = ended_child();
    let leader = child.id();
    let stranger = tree(leader, "Sat Oct 24 10:05:00 2026");
    let os = FakeOs::new(
        vec![tree(leader, "Sat Oct 24 10:00:00 2026"), stranger],
        false,
    );
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    let sent = os.sent();
    assert!(
        !sent
            .iter()
            .any(|s| s.ends_with(&format!(" {}", libc::SIGKILL))),
        "{sent:?}"
    );
    // a pgid number that a stranger group took after the leader was reaped: it has no process
    // of the first scan, so the group gets no KILL either
    let mut child = ended_child();
    let leader = child.id();
    let os = FakeOs::new(
        vec![tree(leader, "A"), vec![proc_row(9999, 1, leader, "B")]],
        true,
    );
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    let sent = os.sent();
    assert!(
        !sent
            .iter()
            .any(|s| s.ends_with(&format!(" {}", libc::SIGKILL))),
        "{sent:?}"
    );
}

#[test]
fn the_snapshot_keeps_the_start_time_and_a_short_row_has_none() {
    let t = procs_of("  10     1    10 Sat Oct 24 10:00:00 2026\n11 10\nx y\n");
    assert_eq!(t.len(), 2);
    assert_eq!((t[0].pid, t[0].ppid, t[0].pgid), (10, 1, 10));
    assert_eq!(t[0].started, "Sat Oct 24 10:00:00 2026");
    assert_eq!((t[1].pgid, t[1].started.as_str()), (0, ""));
    // a row without a start time is never "the same process"
    assert!(same_processes(&t[1..], &t[1..]).is_empty());
    assert_eq!(same_processes(&t[..1], &t[..1]), [10]);
}

/// The descendants of `root` in a snapshot of `ps -axo pid=,ppid=` (lines `<pid> <ppid>`), the
/// children first as the snapshot lists them. `root` itself is not in the result.
fn descendants(snapshot: &str, root: u32) -> Vec<u32> {
    descendant_procs(&procs_of(snapshot), root)
        .into_iter()
        .map(|p| p.pid)
        .collect()
}

#[test]
fn descendants_come_from_a_ps_snapshot() {
    let ps = "  1     0\n 10     1\n 11    10\n 12    11\n 13    10\n 20     1\n 21    20\n junk\n";
    let mut d = descendants(ps, 10);
    d.sort_unstable();
    assert_eq!(d, [11, 12, 13]);
    assert!(descendants(ps, 12).is_empty());
    assert!(descendants("", 10).is_empty());
    // a loop in the table ends
    assert_eq!(descendants("5 6\n6 5\n", 5), [6]);
}
