//! A TERM signal ends `crewboard monitor` at once, also while a `watch` waits for events. The
//! loop installs no handler: the default action of TERM ends the process.
#![cfg(unix)]

use crate::support::board::FakeBoard;
use crate::support::cli::crew;
use crate::support::lock::wait_for;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn term_ends_the_loop_at_once() {
    let (fake, _c, b) = crew();
    // a watch that would wait 30 minutes
    let before = fake.requests().len();
    let mut child = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(&b.dir)
        .args(["monitor", "--timeout", "30m", "--interval", "60s"])
        .env("CREWBOARD_API_BASE", fake.base())
        .env("GH_TOKEN", "ghp_faketoken")
        .env_remove("GITHUB_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // it waits: the twin of the test, the process does not end by itself
    wait_for_poll(&fake, before);
    assert!(
        child.try_wait().unwrap().is_none(),
        "the monitor ended without a signal"
    );
    let started = Instant::now();
    let rc = unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    assert_eq!(rc, 0);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "TERM did not end the monitor"
        );
        // a poll of the exit of the child, which has no event to wait on
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    let out = child.wait_with_output().unwrap();
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Wait until the board has had a request since `before` requests: the monitor has polled once,
/// and from then on it waits for the next interval.
fn wait_for_poll(fake: &FakeBoard, before: usize) {
    assert!(
        wait_for(20, || fake.requests().len() > before),
        "the monitor did not poll the board"
    );
}

/// How many child processes the monitor has after a moment.
fn children_of(pid: u32) -> usize {
    let o = Command::new("pgrep")
        .args(["-P", &pid.to_string()])
        .output()
        .expect("pgrep");
    String::from_utf8_lossy(&o.stdout).lines().count()
}

/// `monitor --crewboard crewboard` with `crewboard` found on the PATH.
fn spawn_with_path_dir(
    fake: &crate::support::board::FakeBoard,
    cfg: &std::path::Path,
    dir: &std::path::Path,
) -> std::process::Child {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(cfg)
        .args([
            "monitor",
            "--crewboard",
            "crewboard",
            "--timeout",
            "30m",
            "--interval",
            "60s",
        ])
        .env("PATH", path)
        .env("CREWBOARD_API_BASE", fake.base())
        .env("GH_TOKEN", "ghp_faketoken")
        .env_remove("GITHUB_TOKEN")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn a_bare_crewboard_on_the_path_that_is_this_binary_runs_in_process() {
    let (fake, _c, b) = crew();
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_crewboard"));
    // `crewboard` on the PATH is a link to this binary: no child process runs `watch`
    let same = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(bin, same.path().join("crewboard")).unwrap();
    let before = fake.requests().len();
    let mut child = spawn_with_path_dir(&fake, &b.dir, same.path());
    wait_for_poll(&fake, before);
    assert!(child.try_wait().unwrap().is_none(), "the monitor ended");
    assert_eq!(children_of(child.id()), 0, "watch ran in a child process");
    child.kill().unwrap();
    child.wait().unwrap();
    // the twin: another binary of that name on the PATH is the seam, and `watch` runs in a child
    let other = tempfile::tempdir().unwrap();
    std::fs::copy(bin, other.path().join("crewboard")).unwrap();
    let before = fake.requests().len();
    let mut child = spawn_with_path_dir(&fake, &b.dir, other.path());
    // the child that runs `watch` makes the first call after this point
    wait_for_poll(&fake, before);
    assert_eq!(children_of(child.id()), 1, "no child process runs watch");
    let _ = Command::new("pkill")
        .args(["-P", &child.id().to_string()])
        .status();
    child.kill().unwrap();
    child.wait().unwrap();
}
