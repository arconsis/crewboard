//! The time budget and the heartbeat of `crewboard wake`, with a real child process and a fake
//! board: the stop reaches the grandchildren, the coordinator hears of it, and a wake that is
//! killed leaves no heartbeat behind. The flow itself is unit-tested in `src/wake/cmd.rs`.

#![cfg(unix)]

use crate::support::board::{Agent, FakeBoard};
use crate::support::cli::{crew, run_ok};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn alive(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Wait until `f` holds, at most `secs`.
fn until(secs: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

/// A dev agent that owns an In Progress ticket, with the events read.
fn working(fake: &FakeBoard, c: &Agent, b: &Agent) -> u64 {
    let t = fake.ticket_issue(
        "IT budget",
        "Goal: test.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    );
    run_ok(fake, c, &["assign", &t.to_string(), "--to", &b.name]);
    run_ok(fake, b, &["take", &t.to_string()]);
    let _ = fake.poll_json(b);
    t
}

fn wake_cmd(fake: &FakeBoard, who: &Agent, state: &Path, extra: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_crewboard"));
    c.arg("--config-dir")
        .arg(&who.dir)
        .arg("wake")
        .args(["--state-dir", state.to_str().unwrap()])
        .args(["--retry-delays", "", "--work-backoff", "0"])
        .args(extra)
        .env("CREWBOARD_API_BASE", fake.base())
        .env("GH_TOKEN", "ghp_faketoken")
        .env_remove("GITHUB_TOKEN")
        .env("CREWBOARD_KILL_GRACE", "1");
    c
}

#[test]
fn a_wake_over_its_budget_is_stopped_with_its_grandchildren_and_the_coordinator_hears_of_it() {
    let (fake, c, b) = crew();
    let t = working(&fake, &c, &b);
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let (child, grand, apart) = (
        s.join("child.pid"),
        s.join("grand.pid"),
        s.join("apart.pid"),
    );
    // a child, a grandchild in a subshell, and a grandchild in a session of its own (the group of
    // the wake does not reach it; the tree of the wake does)
    let cmd = format!(
        "sleep 300 & echo $! > '{}'; (sleep 300 & echo $! > '{}'; wait) & perl -MPOSIX -e 'POSIX::setsid(); exec \"sleep\", \"300\"' & echo $! > '{}'; wait",
        child.display(),
        grand.display(),
        apart.display()
    );
    let t0 = Instant::now();
    let o = wake_cmd(
        &fake,
        &b,
        s,
        &[
            "--wake-cmd",
            &cmd,
            "--max-wake",
            "2s",
            "--heartbeat-every",
            "0",
        ],
    )
    .output()
    .unwrap();
    let code = o.status.code();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(
        t0.elapsed() < Duration::from_secs(30),
        "stopped at the budget, not at the end"
    );
    for f in [&child, &grand, &apart] {
        let pid = std::fs::read_to_string(f).unwrap().trim().to_string();
        assert!(until(5, || !alive(&pid)), "process {pid} is still alive");
    }
    assert!(
        stderr.contains("wake stopped at the time budget of 2s"),
        "{stderr}"
    );
    let said = fake.requests().iter().any(|r| {
        r.method == "POST"
            && r.body
                .contains("stopped at the time budget (2s) of the wake wrapper")
    });
    assert!(said, "the coordinator got the message on #{t}");
    assert!(!s.join("wake.lock").exists());
    assert!(s.join("last-wake").exists());
    // twin: inside its budget the wake ends by itself and says nothing
    let before = fake.requests().len();
    let o = wake_cmd(
        &fake,
        &b,
        s,
        &[
            "--wake-cmd",
            "true",
            "--max-wake",
            "20s",
            "--heartbeat-every",
            "0",
        ],
    )
    .output()
    .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let new: Vec<_> = fake.requests().into_iter().skip(before).collect();
    assert!(
        !new.iter()
            .any(|r| r.method == "POST" && r.body.contains("time budget")),
        "{new:?}"
    );
}

#[test]
fn a_killed_wake_leaves_no_heartbeat_behind() {
    let (fake, c, b) = crew();
    let _t = working(&fake, &c, &b);
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let pidf = s.join("child.pid");
    let cmd = format!("sleep 300 & echo $! > '{}'; wait", pidf.display());
    let mut child = wake_cmd(
        &fake,
        &b,
        s,
        &[
            "--wake-cmd",
            &cmd,
            "--max-wake",
            "off",
            "--heartbeat-every",
            "1",
        ],
    )
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .spawn()
    .unwrap();
    // wait for the wake to start, then for a call of the heartbeat thread (the gate polled once)
    let seen = until(15, || pidf.exists());
    assert!(seen, "the wake started");
    let base = fake.requests().len();
    assert!(
        until(10, || fake.requests().len() > base + 1),
        "the heartbeat thread sends something"
    );
    // SIGKILL: no TERM handler runs, so the thread must die with the process
    child.kill().unwrap();
    child.wait().unwrap();
    let pid = std::fs::read_to_string(&pidf).unwrap().trim().to_string();
    // the orphaned sleep is not the subject of this test: remove it
    let _ = Command::new("kill").arg(&pid).status();
    // a request that was on its way when the process died may still be counted: wait until the
    // count is the same twice, 100 ms apart
    let counted = std::cell::Cell::new(fake.requests().len());
    until(5, || {
        std::thread::sleep(Duration::from_millis(100));
        let now = fake.requests().len();
        now == counted.replace(now)
    });
    let after_kill = counted.get();
    // a real wait: "nothing is sent" has no event to wait for, only two intervals of the
    // heartbeat that must pass without a call
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        fake.requests().len(),
        after_kill,
        "no call to the board after the kill (two intervals)"
    );
}

#[test]
fn a_wake_that_ignores_term_is_killed_after_the_grace_with_its_session_grandchild() {
    let (fake, c, b) = crew();
    let _t = working(&fake, &c, &b);
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let (child, apart) = (s.join("child.pid"), s.join("apart.pid"));
    // the wake itself, a child and a grandchild in a session of its own ignore TERM: only KILL
    // ends them, and only after the grace
    let cmd = format!(
        "trap '' TERM; echo $$ > '{}'; perl -MPOSIX -e 'POSIX::setsid(); $SIG{{TERM}}=\"IGNORE\"; exec \"sleep\", \"300\"' & echo $! > '{}'; sleep 300",
        child.display(),
        apart.display()
    );
    let t0 = Instant::now();
    let o = wake_cmd(
        &fake,
        &b,
        s,
        &[
            "--wake-cmd",
            &cmd,
            "--max-wake",
            "2s",
            "--heartbeat-every",
            "0",
        ],
    )
    .output()
    .unwrap();
    let took = t0.elapsed();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    // budget 2 s, then a grace of 1 s (CREWBOARD_KILL_GRACE=1) before the KILL
    assert!(
        took >= Duration::from_millis(2900),
        "no KILL before the grace: {took:?}"
    );
    assert!(took < Duration::from_secs(30), "bounded: {took:?}");
    for f in [&child, &apart] {
        let pid = std::fs::read_to_string(f).unwrap().trim().to_string();
        assert!(until(5, || !alive(&pid)), "process {pid} is still alive");
    }
    assert!(!s.join("wake.lock").exists(), "the lock is released");
    let said = fake
        .requests()
        .iter()
        .any(|r| r.method == "POST" && r.body.contains("stopped at the time budget (2s)"));
    assert!(said, "the coordinator heard of it");
}
