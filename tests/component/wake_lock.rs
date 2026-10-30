//! `crewboard wake` with real processes: the wake lock between two runs, the stale rules on a
//! real pid, and the state dir of two agents. The board is a stub binary (the hidden `--crewboard
//! BIN` seam) whose `poll` exits with the code in `$STUB_POLL_RC`; `mine` and everything else
//! exit 1. The rules themselves are unit tests in `src/wake` and `src/wake/cmd.rs`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_crewboard");

struct Rig {
    tmp: tempfile::TempDir,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn all(&self) -> String {
        format!(
            "exit {}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}

impl Rig {
    fn new() -> Rig {
        let rig = Rig {
            tmp: tempfile::tempdir().unwrap(),
        };
        let stub = rig.p("stub");
        std::fs::write(
            &stub,
            "#!/bin/sh\nfor a in \"$@\"; do\n  [ \"$a\" = poll ] && exit \"${STUB_POLL_RC:-0}\"\ndone\nexit 1\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(rig.p("cfg-none")).unwrap();
        rig
    }

    fn p(&self, n: &str) -> PathBuf {
        self.tmp.path().join(n)
    }

    fn state_root(&self) -> PathBuf {
        self.p("state/crewboard")
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.args(["wake", "--crewboard"])
            .arg(self.p("stub"))
            .args(["--retry-delays", ""])
            .args(args)
            .env("XDG_STATE_HOME", self.p("state"))
            .env("HOME", self.tmp.path())
            .env("CREWBOARD_CONFIG_DIR", self.p("cfg-none"))
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .env_remove("CREWBOARD_WAKE_CMD")
            .stdin(Stdio::null());
        c
    }

    fn run(&self, args: &[&str]) -> Out {
        self.run_env(args, &[])
    }

    /// `run` with variables of the environment of the wake.
    fn run_env(&self, args: &[&str], env: &[(&str, &str)]) -> Out {
        let mut c = self.cmd(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        }
    }

    fn spawn(&self, args: &[&str]) -> Child {
        self.cmd(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    fn wakes(&self) -> usize {
        std::fs::read_to_string(self.p("wakes"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }
}

/// Wait until a file has content, at most five seconds.
fn wait_for(p: &Path) {
    let t = Instant::now();
    while std::fs::metadata(p).map(|m| m.len() == 0).unwrap_or(true) {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "{} did not appear",
            p.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A wake that records its config dir and then holds the lock until the test creates the file
/// `release` (at most 60 s, so a failed test leaves no child for long). There is no fixed sleep
/// that a loaded host could outlast.
fn held_wake(rig: &Rig) -> String {
    format!(
        "echo \"$CREWBOARD_CONFIG_DIR\" >> '{}'; i=0; while [ ! -e '{}' ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i + 1)); done",
        rig.p("wakes").display(),
        rig.p("release").display()
    )
}

/// A wake that records its config dir at once.
fn noting_wake(rig: &Rig) -> String {
    format!(
        "echo \"$CREWBOARD_CONFIG_DIR\" >> '{}'",
        rig.p("wakes").display()
    )
}

fn release(rig: &Rig) {
    std::fs::write(rig.p("release"), "").unwrap();
}

fn quick_wake(rig: &Rig) -> String {
    format!("echo wake >> '{}'", rig.p("wakes").display())
}

fn plant(dir: &Path, pid: u32, started: i64) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("owner"), "old-owner\n").unwrap();
    std::fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
    std::fs::write(dir.join("started"), format!("{started}\n")).unwrap();
}

fn now() -> i64 {
    crewboard::clock::now_epoch()
}

fn dead_pid() -> u32 {
    let mut c = Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

fn sleeper() -> Child {
    Command::new("sleep")
        .arg("30")
        .stdout(Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn p2_a_poll_with_events_runs_one_wake_prints_nothing_and_releases_the_lock() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let o = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        lock.to_str().unwrap(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(o.stdout, "", "{}", o.all());
    assert_eq!(rig.wakes(), 1);
    assert!(!lock.exists(), "the lock is released");
    assert!(
        o.stderr.contains("poll-and-wake: events pending; waking: "),
        "{}",
        o.all()
    );
    // the other direction: a quiet board starts no wake
    let o = rig
        .cmd(&[
            "--wake-cmd",
            &quick_wake(&rig),
            "--lock-dir",
            lock.to_str().unwrap(),
            "--work-backoff",
            "off",
        ])
        .env("STUB_POLL_RC", "1")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    assert_eq!(rig.wakes(), 1);
}

#[test]
fn p4_a_second_run_during_a_wake_starts_no_second_wake() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let l = lock.to_str().unwrap();
    let mut first = rig.spawn(&["--wake-cmd", &held_wake(&rig), "--lock-dir", l]);
    // the first wake command has run: the lock exists and the wake is recorded
    wait_for(&rig.p("wakes"));
    let o = rig.run(&["--wake-cmd", &noting_wake(&rig), "--lock-dir", l]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stderr.contains("a wake already runs (pid "),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("; not starting another"), "{}", o.all());
    assert_eq!(rig.wakes(), 1, "no second wake during the first");
    release(&rig);
    assert_eq!(first.wait().unwrap().code(), Some(0));
    assert!(!lock.exists(), "the first run released the lock");
    // the other direction: with the lock free the next run wakes
    let o = rig.run(&["--wake-cmd", &quick_wake(&rig), "--lock-dir", l]);
    assert_eq!((o.code, rig.wakes()), (0, 2), "{}", o.all());
}

#[test]
fn p5_a_lock_of_a_dead_pid_is_taken_over_and_said() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    plant(&lock, dead_pid(), now());
    let o = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        lock.to_str().unwrap(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(rig.wakes(), 1);
    assert!(
        o.stderr.contains("STALE lock (pid-dead: owner pid "),
        "{}",
        o.all()
    );
    assert!(
        o.stderr.contains("owner old-owner); taking it over"),
        "{}",
        o.all()
    );
}

#[test]
fn p5_a_wake_that_was_killed_leaves_a_lock_that_the_next_run_takes_over() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let l = lock.to_str().unwrap();
    let mut first = rig.spawn(&["--wake-cmd", "exec sleep 5", "--lock-dir", l]);
    wait_for(&lock.join("started"));
    first.kill().unwrap();
    first.wait().unwrap();
    assert!(lock.is_dir(), "a SIGKILL leaves the lock behind");
    let o = rig.run(&["--wake-cmd", &quick_wake(&rig), "--lock-dir", l]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("STALE lock (pid-dead:"), "{}", o.all());
    assert_eq!(rig.wakes(), 1);
}

#[test]
fn p5b_a_live_lock_is_stale_only_over_the_budget() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let l = lock.to_str().unwrap();
    let mut live = sleeper();
    plant(&lock, live.id(), now() - 100);
    let young = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        l,
        "--stale-after",
        "3600",
    ]);
    assert_eq!(rig.wakes(), 0, "{}", young.all());
    assert!(young.stderr.contains("already runs"), "{}", young.all());
    let old = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        l,
        "--stale-after",
        "10",
    ]);
    assert_eq!(rig.wakes(), 1, "{}", old.all());
    assert!(
        old.stderr.contains("STALE lock (too-old: "),
        "{}",
        old.all()
    );
    live.kill().unwrap();
    live.wait().unwrap();
}

#[test]
fn p5c_p5d_an_empty_lock_dir_is_taken_over_only_after_the_grace() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let l = lock.to_str().unwrap();
    std::fs::create_dir_all(&lock).unwrap();
    let o = rig.run(&["--wake-cmd", &quick_wake(&rig), "--lock-dir", l]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(rig.wakes(), 0, "a fresh empty lock dir is not taken over");
    assert!(
        o.stderr.contains("already runs") && !o.stderr.contains("STALE"),
        "{}",
        o.all()
    );
    let old = std::time::SystemTime::now() - Duration::from_secs(100);
    std::fs::File::open(&lock)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let o = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        l,
        "--empty-grace",
        "5",
    ]);
    assert_eq!(rig.wakes(), 1, "{}", o.all());
    assert!(
        o.stderr.contains("STALE lock (empty-after-grace:"),
        "{}",
        o.all()
    );
}

#[test]
fn s1_two_agents_of_one_initiative_on_one_host_both_wake() {
    let rig = Rig::new();
    let (d1, d2) = (rig.p("agents/dev1"), rig.p("agents/dev2"));
    std::fs::create_dir_all(&d1).unwrap();
    std::fs::create_dir_all(&d2).unwrap();
    let held = held_wake(&rig);
    let quick = noting_wake(&rig);
    let a = |d: &Path, wake: &String| {
        vec![
            "--initiative",
            "s1",
            "--config-dir",
            d.to_str().unwrap(),
            "--wake-cmd",
            wake,
            "--work-backoff",
            "off",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>()
    };
    let args1 = a(&d1, &held);
    let mut first = rig.spawn(&args1.iter().map(String::as_str).collect::<Vec<_>>());
    wait_for(&rig.p("wakes"));
    let args2 = a(&d2, &quick);
    let o = rig.run(&args2.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(o.code, 0, "{}", o.all());
    release(&rig);
    assert_eq!(first.wait().unwrap().code(), Some(0));
    let seen = std::fs::read_to_string(rig.p("wakes")).unwrap();
    assert_eq!(rig.wakes(), 2, "{seen}");
    assert!(
        seen.lines().any(|l| l == d1.to_str().unwrap())
            && seen.lines().any(|l| l == d2.to_str().unwrap()),
        "{seen}"
    );
    for n in ["s1-dev1", "s1-dev2"] {
        let log = std::fs::read_to_string(rig.state_root().join("agents").join(n).join("wake.log"))
            .unwrap();
        assert!(
            log.contains("wake finished OK") && !log.contains("already runs"),
            "{n}: {log}"
        );
    }
}

#[test]
fn s1b_the_same_agent_twice_runs_one_wake_and_the_second_waits() {
    let rig = Rig::new();
    let d = rig.p("agents/dev1");
    std::fs::create_dir_all(&d).unwrap();
    let wake = held_wake(&rig);
    let args = [
        "--initiative",
        "s1b",
        "--config-dir",
        d.to_str().unwrap(),
        "--wake-cmd",
        &wake,
        "--work-backoff",
        "off",
    ];
    let mut first = rig.spawn(&args);
    wait_for(&rig.p("wakes"));
    let o = rig.run(&args);
    assert_eq!(o.code, 0, "{}", o.all());
    release(&rig);
    first.wait().unwrap();
    assert_eq!(rig.wakes(), 1);
    let log = std::fs::read_to_string(rig.state_root().join("agents/s1b-dev1/wake.log")).unwrap();
    assert!(log.contains("already runs"), "{log}");
}

#[test]
fn s2_print_state_dir_follows_the_agent_and_starts_nothing() {
    let rig = Rig::new();
    let root = rig.state_root();
    let dir = |args: &[&str]| {
        let o = rig.run(args);
        assert_eq!(o.code, 0, "{}", o.all());
        o.stdout.trim_end().to_string()
    };
    let r = |s: &str| format!("{}/{s}", root.display());
    assert_eq!(
        dir(&[
            "--wake-cmd",
            "x",
            "--initiative",
            "demo",
            "--config-dir",
            "/x/agents/dev1",
            "--print-state-dir"
        ]),
        r("agents/demo-dev1")
    );
    assert_eq!(
        dir(&[
            "--wake-cmd",
            "x",
            "--initiative",
            "demo",
            "--config-dir",
            "/x/agents/dev2",
            "--print-state-dir"
        ]),
        r("agents/demo-dev2")
    );
    assert_eq!(
        dir(&[
            "--wake-cmd",
            "x",
            "--initiative",
            "demo",
            "--agent",
            "dev@host-a",
            "--print-state-dir"
        ]),
        r("agents/demo-dev_host-a")
    );
    assert_eq!(
        dir(&[
            "--wake-cmd",
            "x",
            "--initiative",
            "demo",
            "--config-dir",
            "/x/agents/dev1",
            "--lock-dir",
            "/y/wake.lock",
            "--print-state-dir"
        ]),
        "/y"
    );
    assert_eq!(
        dir(&[
            "--wake-cmd",
            "x",
            "--initiative",
            "demo",
            "--print-state-dir"
        ]),
        r("demo")
    );
    // it needs no wake command and creates nothing
    assert_eq!(
        dir(&["--initiative", "demo", "--print-state-dir"]),
        r("demo")
    );
    assert!(!root.exists());
}

#[test]
fn a_run_without_a_wake_command_or_with_a_bad_flag_value_is_exit_64() {
    let rig = Rig::new();
    let o = rig.run(&["--lock-dir", rig.p("l.d").to_str().unwrap()]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr
            .contains("poll-and-wake: no wake command (--wake-cmd, CREWBOARD_WAKE_CMD or --tick)"),
        "{}",
        o.all()
    );
    assert_eq!(o.stdout, "");
    let o = rig.run(&["--wake-cmd", "true", "--work-backoff", "soon"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr
            .contains("--work-backoff needs whole seconds or off, got 'soon'"),
        "{}",
        o.all()
    );
    let o = rig.run(&["--wake-cmd", "true", "--heartbeat-every", "abc"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr
            .contains("--heartbeat-every needs whole seconds, got 'abc'"),
        "{}",
        o.all()
    );
    // the same value from the environment is a config error: exit 4
    let o = rig.run_env(
        &["--wake-cmd", "true"],
        &[("CREWBOARD_HEARTBEAT_EVERY", "abc")],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("--heartbeat-every needs whole seconds, got 'abc'"),
        "{}",
        o.all()
    );
    // an unknown option is a usage error: exit 64
    let o = rig.run(&["--wake-cmd", "true", "--no-such-flag"]);
    assert_eq!(o.code, 64, "{}", o.all());
}

#[test]
fn exit_4_of_the_board_stops_the_wakes_until_the_stop_file_goes() {
    let rig = Rig::new();
    let state = rig.p("st");
    let args = ["--wake-cmd", "true", "--state-dir", state.to_str().unwrap()];
    let o = rig.cmd(&args).env("STUB_POLL_RC", "4").output().unwrap();
    assert_eq!(o.status.code(), Some(4));
    assert!(state.join("stopped").is_file());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("STOPPED: crewboard poll exit 4: "), "{err}");
    // the next run exits 0 at once and says nothing, whatever the board says
    let o = rig.cmd(&args).env("STUB_POLL_RC", "0").output().unwrap();
    assert!(
        o.status.success() && o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // removing the stop file lifts it
    std::fs::remove_file(state.join("stopped")).unwrap();
    let o = rig.cmd(&args).env("STUB_POLL_RC", "1").output().unwrap();
    assert!(o.status.success());
}

#[test]
fn term_stops_the_wake_group_releases_the_lock_and_exits_with_143() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let pidfile = rig.p("sleeper.pid");
    // a wake with a grandchild: the shell waits for a background sleep
    let cmd = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
    let mut wake = rig.spawn(&["--wake-cmd", &cmd, "--lock-dir", lock.to_str().unwrap()]);
    wait_for(&pidfile);
    let sleeper: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    assert!(
        alive(sleeper) && lock.is_dir(),
        "the wake runs under the lock"
    );
    assert_eq!(unsafe { libc::kill(wake.id() as i32, libc::SIGTERM) }, 0);
    let t = Instant::now();
    let status = loop {
        if let Some(s) = wake.try_wait().unwrap() {
            break s;
        }
        assert!(
            t.elapsed() < Duration::from_secs(8),
            "TERM did not end the wake"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(143), "{status:?}");
    assert!(!lock.exists(), "the lock is released");
    let t = Instant::now();
    while alive(sleeper) {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the child of the wake is still alive"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // twin: the next run takes a free lock, with no takeover
    let o = rig.run(&[
        "--wake-cmd",
        &quick_wake(&rig),
        "--lock-dir",
        lock.to_str().unwrap(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stderr.contains("STALE"), "{}", o.all());
}

#[test]
fn a_wake_that_ignores_term_is_killed_after_the_grace() {
    let rig = Rig::new();
    let lock = rig.p("lock.d");
    let pidfile = rig.p("stubborn.pid");
    // the shell and its sleep both ignore TERM (an ignored signal is inherited)
    let cmd = format!(
        "trap '' TERM; sleep 30 & echo $! > '{}'; wait",
        pidfile.display()
    );
    let mut wake = rig
        .cmd(&["--wake-cmd", &cmd, "--lock-dir", lock.to_str().unwrap()])
        .env("CREWBOARD_KILL_GRACE", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(&pidfile);
    let stubborn: i32 = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    let started = Instant::now();
    assert_eq!(unsafe { libc::kill(wake.id() as i32, libc::SIGTERM) }, 0);
    // a real wait: "still alive after TERM" has no event to wait for, and the grace is 1 s
    std::thread::sleep(Duration::from_millis(400));
    assert!(alive(stubborn), "TERM alone does not stop it");
    let status = loop {
        if let Some(s) = wake.try_wait().unwrap() {
            break s;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the wake did not end"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "KILL came before the grace"
    );
    assert_eq!(status.code(), Some(143), "{status:?}");
    assert!(!lock.exists());
    let t = Instant::now();
    while alive(stubborn) {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "the child survived KILL"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
