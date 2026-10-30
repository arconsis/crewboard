//! `crewboard lock` as the real binary, on a temp dir: the basic cycle, refusals, status, break,
//! usage errors, and the exit codes (0, 1, 2, 3, 64, 75, 76, 77, or that of CMD). The rules are
//! proved on fake time and processes in `src/lock/*`; these tests prove what only the binary
//! shows: the exit codes, the files, and real concurrency. Ids are those of the shell suite
//! (`skill/loops/tests/test-locks.sh`).
#![cfg(unix)]

use crate::support::lock::*;
use std::process::Stdio;

#[test]
fn h1_a_free_lock_is_taken_and_the_owner_file_is_complete() {
    let r = Rig::new();
    let d = r.p("l");
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "a@x",
        "--reason",
        "cargo test",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&d, "agent").as_deref(), Some("a@x"));
    assert_eq!(owner_field(&d, "host").as_deref(), Some("hostA"));
    assert_eq!(owner_field(&d, "pid"), Some(r.live().to_string()));
    assert!(owner_field(&d, "started").unwrap().starts_with("20"));
    assert!(
        owner_field(&d, "started_epoch")
            .unwrap()
            .parse::<i64>()
            .is_ok()
    );
    assert_eq!(owner_field(&d, "reason").as_deref(), Some("cargo test"));
    // the default pid is the parent: this test process
    let r2 = Rig::new();
    let o = r2.run(&["take", &r2.s("l"), "--holder", "a@x", "--no-board"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        owner_field(&r2.p("l"), "pid"),
        Some(std::process::id().to_string())
    );
}

#[test]
fn h2_a_live_lock_refuses_the_second_taker_with_75() {
    let r = Rig::new();
    r.plant("l", "a@x", HOST, r.live(), now());
    let o = r.run(&["take", &r.s("l"), "--holder", "b@x", "--no-board"]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("a@x"));
    assert!(o.stderr.contains("held by a@x"), "{}", o.stderr);
    assert!(
        o.stderr.contains(" host-lock: "),
        "the log prefix stays: {}",
        o.stderr
    );
}

#[test]
fn h3_a_dead_pid_is_taken_over_with_one_log_line() {
    let r = Rig::new();
    r.plant("l", "old@x", HOST, dead_pid(), now());
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "new@x",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        o.stderr.matches("took over stale lock").count(),
        1,
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("old owner old@x@hostA"));
    assert!(o.stderr.contains("is gone on host hostA"));
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("new@x"));
    assert!(!r.p("l.takeover").exists());
}

#[test]
fn h3b_a_live_pid_of_another_user_is_alive() {
    // pid 1 belongs to root: `kill -0` gives EPERM for any other user. As root, it is plain alive.
    let r = Rig::new();
    r.plant("l", "other@x", HOST, 1, now());
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "new@x",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("other@x"));
    let s = r.run(&["status", &r.s("l"), "--no-board"]);
    assert!(s.stdout.contains("alive=yes"), "{}", s.all());
}

#[test]
fn h6_release_by_a_non_owner_other_host_and_owner() {
    let r = Rig::new();
    let l = r.s("l");
    assert_eq!(
        r.run(&["take", &l, "--holder", "a@x", "--no-board"]).code,
        0
    );
    let o = r.run(&["release", &l, "--holder", "b@x"]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(r.p("l/owner").exists() && o.stderr.contains("refused"));
    // the right agent from another host
    let mut c = r.cmd(&["release", &l, "--holder", "a@x"]);
    c.env("HOST_LOCK_HOST", "hostC");
    assert_eq!(out_of(c.output().unwrap()).code, 3);
    assert!(r.p("l/owner").exists());
    // the owner
    let o = r.run(&["release", &l, "--holder", "a@x"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!r.p("l").exists());
}

#[test]
fn h7_eight_concurrent_takers_on_a_free_lock_exactly_one_wins() {
    let r = Rig::new();
    let kids: Vec<_> = (1..=8)
        .map(|i| {
            r.cmd(&[
                "take",
                &r.s("l"),
                "--holder",
                &format!("t{i}@x"),
                "--no-board",
                "--pid",
                &r.live().to_string(),
            ])
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
        })
        .collect();
    let codes: Vec<i32> = kids
        .into_iter()
        .map(|mut k| k.wait().unwrap().code().unwrap())
        .collect();
    assert_eq!(codes.iter().filter(|c| **c == 0).count(), 1, "{codes:?}");
    assert_eq!(codes.iter().filter(|c| **c == 75).count(), 7, "{codes:?}");
}

#[test]
fn h7b_eight_takers_on_a_stale_lock_one_holder_one_takeover_line() {
    let r = Rig::new();
    r.plant("l", "dead@x", HOST, dead_pid(), now());
    let kids: Vec<_> = (1..=8)
        .map(|i| {
            r.cmd(&[
                "take",
                &r.s("l"),
                "--holder",
                &format!("t{i}@x"),
                "--no-board",
                "--pid",
                &r.live().to_string(),
            ])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
        })
        .collect();
    let outs: Vec<_> = kids
        .into_iter()
        .map(|k| k.wait_with_output().unwrap())
        .collect();
    let wins = outs.iter().filter(|o| o.status.code() == Some(0)).count();
    let lines: usize = outs
        .iter()
        .map(|o| {
            String::from_utf8_lossy(&o.stderr)
                .matches("took over stale lock")
                .count()
        })
        .sum();
    assert_eq!((wins, lines), (1, 1));
    assert!(owner_field(&r.p("l"), "agent").is_some());
    assert!(!r.p("l.takeover").exists());
}

#[test]
fn h9_run_holds_the_lock_while_the_command_runs_releases_and_returns_its_code() {
    let r = Rig::new();
    let st = r.s("during.txt");
    let o = r.run(&[
        "run",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--",
        "sh",
        "-c",
        &format!("{BIN} lock status {} >{st}; exit 7", r.s("l")),
    ]);
    assert_eq!(o.code, 7, "{}", o.all());
    assert!(
        std::fs::read_to_string(&st)
            .unwrap()
            .starts_with("held: a@x@hostA")
    );
    assert!(!r.p("l").exists());
}

#[test]
fn h9b_run_on_a_held_live_lock_does_not_start_the_command_75() {
    let r = Rig::new();
    r.plant("l", "live@x", HOST, r.live(), now());
    let o = r.run(&[
        "run",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--",
        "touch",
        &r.s("ran"),
    ]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert!(!r.p("ran").exists());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("live@x"));
}

#[test]
fn h9c_a_command_that_does_not_exist_is_127_and_the_lock_is_released() {
    let r = Rig::new();
    let o = r.run(&[
        "run",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--",
        "/nonexistent/cmd",
    ]);
    assert_eq!(o.code, 127, "{}", o.all());
    assert!(!r.p("l").exists());
}

/// Start `run` with a job that records its pid, in a process group of its own (the run is the
/// leader), and wait until the job is up.
fn run_with_job(r: &Rig) -> (std::process::Child, u32) {
    let pidfile = r.s("job.pid");
    let mut c = r.cmd(&[
        "run",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--",
        "sh",
        "-c",
        &format!("echo $$ >{pidfile}; exec sleep 300"),
    ]);
    c.stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut c, 0);
    let k = c.spawn().unwrap();
    assert!(wait_for(10, || std::fs::read_to_string(&pidfile)
        .is_ok_and(|t| !t.trim().is_empty())));
    let job = std::fs::read_to_string(&pidfile)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    (k, job)
}

#[test]
fn run_releases_the_lock_and_exits_143_when_the_group_gets_term() {
    let r = Rig::new();
    let (mut k, job) = run_with_job(&r);
    // a TERM of the process group (the budget stop of a wake): the job gets it from the group
    unsafe { libc::kill(-(k.id() as i32), libc::SIGTERM) };
    let st = k.wait().unwrap();
    assert_eq!(st.code(), Some(143), "{st:?}");
    assert!(wait_for(5, || !pid_alive(job)), "the job ended");
    assert!(!r.p("l").exists(), "the lock is released");
}

#[test]
fn run_does_not_forward_a_signal_to_the_job_the_group_does_that() {
    let r = Rig::new();
    let (mut k, job) = run_with_job(&r);
    // TERM to the run only: a second delivery to the job (a second INT is a force quit for many
    // tools) must not happen, so the job lives on and the run waits for it
    unsafe { libc::kill(k.id() as i32, libc::SIGTERM) };
    // a real wait: "the job got no signal" is a negative with no event to wait for
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(pid_alive(job), "the job got no signal from the run");
    assert!(
        r.p("l/owner").exists(),
        "the lock is held while the job runs"
    );
    assert!(k.try_wait().unwrap().is_none(), "the run waits for the job");
    // the job ends: the run releases and exits 143 because it got the TERM
    unsafe { libc::kill(job as i32, libc::SIGKILL) };
    let st = k.wait().unwrap();
    assert_eq!(st.code(), Some(143), "{st:?}");
    assert!(!r.p("l").exists());
}

#[test]
fn a_closed_stderr_or_stdout_does_not_panic_the_command() {
    use std::os::fd::{FromRawFd, OwnedFd};
    let r = Rig::new();
    r.plant("l", "a@x", HOST, r.live(), now());
    // a pipe whose read end is closed, as in `... 2>&1 | grep -q held`
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    unsafe { libc::close(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let o = r
        .cmd(&["take", &r.s("l"), "--holder", "b@x", "--no-board"])
        .stderr(Stdio::from(w))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(75),
        "the exit code of the lock, not a panic (101)"
    );
    // the help text to a closed stdout
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    unsafe { libc::close(fds[0]) };
    let w = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    let o = r.cmd(&["--help"]).stdout(Stdio::from(w)).output().unwrap();
    assert_eq!(o.status.code(), Some(0));
}

#[test]
fn a_max_age_above_i64_max_never_takes_a_live_lock_over() {
    let r = Rig::new();
    r.plant("l", "a@x", HOST, r.live(), now() - 100_000);
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "b@x",
        "--no-board",
        "--max-age",
        "18446744073709551615",
    ]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("a@x"));
    // a duration that does not fit in 64 bits is a usage error
    let o = r.run(&[
        "take",
        &r.s("m"),
        "--holder",
        "b@x",
        "--wait",
        "99999999999999999999",
    ]);
    assert_eq!(o.code, 64, "{}", o.all());
}

#[test]
fn a_lock_dir_that_cannot_be_made_is_exit_1_with_one_line_not_a_false_held() {
    // root ignores the mode of a directory
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let r = Rig::new();
    let ro = r.p("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
    let o = r.run(&[
        "take",
        &format!("{}/l", ro.display()),
        "--holder",
        "a@x",
        "--no-board",
    ]);
    std::fs::set_permissions(&ro, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(o.stderr.contains("cannot make"), "{}", o.stderr);
    assert!(!ro.join("l").exists());
}

#[test]
fn a_wake_allows_lock_run_only_for_the_dirs_it_names_and_the_rest_of_lock_stays_open() {
    let r = Rig::new();
    let (granted, other) = (r.s("granted.lock.d"), r.s("other.lock.d"));
    let wake = |args: &[&str], dirs: &str| {
        let mut c = r.cmd(args);
        c.env("CREWBOARD_WAKE", "1")
            .env("CREWBOARD_LOCK_RUN_DIRS", dirs);
        out_of(c.output().unwrap())
    };
    let touch = |n: &str| {
        [
            "run",
            &r.s(n),
            "--holder",
            "a@x",
            "--no-board",
            "--",
            "touch",
            &r.s("ran"),
        ]
        .map(String::from)
    };
    // a wake without the dir: refused, the command does not run
    let a = touch("other.lock.d");
    let o = wake(&a.iter().map(String::as_str).collect::<Vec<_>>(), &granted);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=lock-run-not-granted]"),
        "{}",
        o.stderr
    );
    assert!(!r.p("ran").exists() && !r.p("other.lock.d").exists());
    let o = wake(&a.iter().map(String::as_str).collect::<Vec<_>>(), "");
    assert_eq!(o.code, 2, "{}", o.all());
    // the granted dir runs
    let a = touch("granted.lock.d");
    let o = wake(
        &a.iter().map(String::as_str).collect::<Vec<_>>(),
        &format!("{other}:{granted}"),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(r.p("ran").exists());
    // take, status and release stay open under a wake, for any dir
    let o = wake(
        &[
            "take",
            &other,
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            "0",
            "--expect",
            "5m",
        ],
        "",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(wake(&["status", &other, "--no-board"], "").code, 0);
    assert_eq!(wake(&["release", &other, "--holder", "a@x"], "").code, 0);
    // and without a wake or scope the same `run` is not restricted
    let a = touch("free.lock.d");
    let o = r.run(&a.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn h10_status_free_is_1_held_is_0_stale_is_2() {
    let r = Rig::new();
    let l = r.s("l");
    let o = r.run(&["status", &l]);
    assert_eq!((o.code, o.stdout.trim()), (1, "free"), "{}", o.all());
    assert_eq!(
        r.run(&[
            "take",
            &l,
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            &r.live().to_string()
        ])
        .code,
        0
    );
    let o = r.run(&["status", &l, "--no-board"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.starts_with("held:"), "{}", o.stdout);
    std::fs::remove_dir_all(r.p("l")).unwrap();
    r.plant("l", "old@x", HOST, dead_pid(), now());
    let o = r.run(&["status", &l, "--no-board"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stdout.starts_with("stale:"), "{}", o.stdout);
}

#[test]
fn h10b_status_on_a_live_lock_says_alive_and_age_a_dead_pid_is_stale_pid_dead() {
    let r = Rig::new();
    r.plant("l", "agent-a@host-b", HOST, r.live(), now());
    let o = r.run(&["status", &r.s("l"), "--no-board"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line = o.stdout.trim();
    assert!(
        line.starts_with("held: agent-a@host-b@hostA pid="),
        "{line}"
    );
    assert!(line.contains(" alive=yes age="), "{line}");
    std::fs::remove_dir_all(r.p("l")).unwrap();
    r.plant("l", "agent-a@host-b", HOST, dead_pid(), now());
    let o = r.run(&["status", &r.s("l"), "--no-board"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stdout.starts_with("stale: pid-dead:"), "{}", o.stdout);
}

#[test]
fn h10d_break_if_dead_on_a_live_lock_is_refused_3_and_the_owner_file_stays() {
    let r = Rig::new();
    r.plant("l", "agent-a@host-b", HOST, r.live(), now());
    let before = std::fs::read(r.p("l/owner")).unwrap();
    let o = r.run(&["break", &r.s("l"), "--if-dead", "--no-board"]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(
        o.stdout.starts_with("refused: held: agent-a@host-b@hostA"),
        "{}",
        o.stdout
    );
    assert_eq!(std::fs::read(r.p("l/owner")).unwrap(), before);
}

#[test]
fn h10e_break_if_dead_on_a_dead_pid_removes_it_with_one_log_line_in_the_log_file() {
    let r = Rig::new();
    r.plant("l", "agent-a@host-b", HOST, dead_pid(), now());
    let o = r.run(&[
        "break",
        &r.s("l"),
        "--if-dead",
        "--no-board",
        "--log",
        &r.s("h10e.log"),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!r.p("l").exists() && !r.p("l.takeover").exists());
    assert!(o.stderr.contains("took over stale lock"));
    let log = std::fs::read_to_string(r.p("h10e.log")).unwrap();
    assert_eq!(log.matches("stale lock").count(), 1, "{log}");
}

#[test]
fn h10f_break_if_dead_on_a_free_dir_prints_free_0_h10g_without_if_dead_is_64() {
    let r = Rig::new();
    let o = r.run(&["break", &r.s("none"), "--if-dead", "--no-board"]);
    assert_eq!((o.code, o.stdout.trim()), (0, "free"), "{}", o.all());
    r.plant("l", "agent-a@host-b", HOST, dead_pid(), now());
    let o = r.run(&["break", &r.s("l"), "--no-board"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(r.p("l/owner").exists(), "the lock stays");
    assert!(o.stderr.contains("break needs --if-dead"), "{}", o.stderr);
}

#[test]
fn h17_pid_runner_records_the_worker_alive_blocks_killed_frees() {
    // a fake Runner.Worker: a shell under that name is the parent of the lock call (the walk is real)
    let r = Rig::new();
    std::os::unix::fs::symlink("/bin/sh", r.p("Runner.Worker")).unwrap();
    let rc = r.s("h17.rc");
    let mut w = std::process::Command::new(r.p("Runner.Worker"));
    w.arg("-c")
        .arg(format!(
            "{BIN} lock take {} --holder ci-r --ci --no-board --pid runner; echo $? >{rc}; sleep 60",
            r.s("l")
        ))
        .env("HOST_LOCK_HOST", HOST)
        .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    let mut worker = w.spawn().unwrap();
    assert!(wait_for(15, || std::fs::read_to_string(&rc)
        .is_ok_and(|t| !t.trim().is_empty())));
    assert_eq!(std::fs::read_to_string(&rc).unwrap().trim(), "0");
    // the pid recorded is the worker
    assert_eq!(owner_field(&r.p("l"), "pid"), Some(worker.id().to_string()));
    // alive, an agent does not take the lock
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 75, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "agent").as_deref(), Some("ci-r"));
    // killed, the agent takes over (pid-dead)
    worker.kill().unwrap();
    worker.wait().unwrap();
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("reason=pid-dead"), "{}", o.stderr);
}

fn write_table(r: &Rig, name: &str, text: &str) -> String {
    let p = r.p(name);
    std::fs::write(&p, text).unwrap();
    std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    p.to_string_lossy().to_string()
}

#[test]
fn h17d_the_walk_finds_runner_worker_behind_a_full_path_comm_with_the_hook() {
    let r = Rig::new();
    let pt = write_table(
        &r,
        "pt.sh",
        "#!/bin/sh\ncase \"$1\" in\n  500) echo \"400 /bin/bash\" ;;\n  400) echo \"300 /Users/x/actions-runner/bin/Runner.Worker\" ;;\n  300) echo \"1 /sbin/launchd\" ;;\n  *)   echo \"500 /bin/bash\" ;;\nesac\n",
    );
    let mut c = r.cmd(&[
        "take",
        &r.s("l"),
        "--holder",
        "ci-h",
        "--no-board",
        "--pid",
        "runner",
    ]);
    c.env("HOST_LOCK_PARENT_CMD", format!("{pt} $1"));
    let o = out_of(c.output().unwrap());
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "pid").as_deref(), Some("400"));
}

#[test]
fn h18_pid_runner_without_a_runner_ancestor_is_64_and_writes_nothing_pid_0_works() {
    let r = Rig::new();
    let pt = write_table(
        &r,
        "pt2.sh",
        "#!/bin/sh\ncase \"$1\" in\n  500) echo \"300 /bin/bash\" ;;\n  300) echo \"1 /sbin/launchd\" ;;\n  *)   echo \"500 /bin/bash\" ;;\nesac\n",
    );
    let hook = format!("{pt} $1");
    let mut c = r.cmd(&[
        "take",
        &r.s("l"),
        "--holder",
        "ci-n",
        "--no-board",
        "--pid",
        "runner",
    ]);
    c.env("HOST_LOCK_PARENT_CMD", &hook);
    let o1 = out_of(c.output().unwrap());
    let mut c = r.cmd(&["ci-wait", &r.s("l"), "--holder", "ci-n", "--pid", "runner"]);
    c.env("HOST_LOCK_PARENT_CMD", &hook);
    let o2 = out_of(c.output().unwrap());
    assert_eq!((o1.code, o2.code), (64, 64), "{}\n{}", o1.all(), o2.all());
    assert!(!r.p("l").exists() && !r.p("l.waiting").exists());
    assert!(o1.stderr.contains("no Runner.Worker"), "{}", o1.stderr);
    // `--pid 0` still works
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "ci-n",
        "--no-board",
        "--pid",
        "0",
        "--expect",
        "10m",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "pid").as_deref(), Some("0"));
}

#[test]
fn e1_pid_0_needs_expect_names_run_and_writes_no_lock_recorded_with_it_not_needed_with_a_pid() {
    let r = Rig::new();
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        "0",
    ]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(!r.p("l").exists());
    assert!(
        o.stderr.contains("--expect") && o.stderr.contains("'run'"),
        "{}",
        o.stderr
    );
    // E1b
    let o = r.run(&[
        "take",
        &r.s("l"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        "0",
        "--expect",
        "1",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("l"), "pid").as_deref(), Some("0"));
    assert_eq!(owner_field(&r.p("l"), "expect").as_deref(), Some("1"));
    // a pid needs no --expect and writes no expect= line
    let o = r.run(&[
        "take",
        &r.s("m"),
        "--holder",
        "a@x",
        "--no-board",
        "--pid",
        &r.live().to_string(),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(owner_field(&r.p("m"), "expect"), None);
}

#[test]
fn e5b_bad_durations_are_64_and_leave_nothing_behind() {
    let r = Rig::new();
    for args in [
        vec![
            "take",
            "X",
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            "0",
            "--expect",
            "2d",
        ],
        vec![
            "take",
            "X",
            "--holder",
            "a@x",
            "--no-board",
            "--wait",
            "soon",
        ],
        vec![
            "take",
            "X",
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            "0",
            "--expect",
            "m",
        ],
    ] {
        let d = r.s("e5");
        let a: Vec<&str> = args
            .iter()
            .map(|s| if *s == "X" { d.as_str() } else { *s })
            .collect();
        let o = r.run(&a);
        assert_eq!(o.code, 64, "{args:?}: {}", o.all());
        assert!(!r.p("e5").exists());
    }
    // the other direction: the forms of the table are accepted
    for (v, want) in [("90", "90"), ("90s", "90"), ("30m", "1800"), ("2h", "7200")] {
        let d = format!("e5-{v}");
        let o = r.run(&[
            "take",
            &r.s(&d),
            "--holder",
            "a@x",
            "--no-board",
            "--pid",
            "0",
            "--expect",
            v,
            "--wait",
            v,
            "--max-age",
            v,
        ]);
        assert_eq!(o.code, 0, "{v}: {}", o.all());
        assert_eq!(owner_field(&r.p(&d), "expect").as_deref(), Some(want));
    }
}

#[test]
fn f6b_release_of_a_foreign_dir_is_refused_3_and_its_file_stays() {
    let r = Rig::new();
    std::fs::create_dir(r.p("l")).unwrap();
    std::fs::write(r.p("l/info"), "i\n").unwrap();
    let o = r.run(&["release", &r.s("l"), "--holder", "a@x"]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(r.p("l/info").exists());
}

#[test]
fn o4_release_of_an_invalid_owner_file_is_refused_3_once_valid_it_is_released() {
    let r = Rig::new();
    std::fs::create_dir(r.p("l")).unwrap();
    std::fs::write(
        r.p("l/owner"),
        format!("agent=a@x\nhost=hostA\npid={}\nstarted=x\n", r.live()),
    )
    .unwrap();
    let o = r.run(&["release", &r.s("l"), "--holder", "a@x"]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(r.p("l/owner").exists());
    assert!(
        o.stderr
            .contains("held by foreign holder: owner (no numeric started_epoch=)"),
        "{}",
        o.stderr
    );
    let mut t = std::fs::OpenOptions::new()
        .append(true)
        .open(r.p("l/owner"))
        .unwrap();
    std::io::Write::write_all(&mut t, format!("started_epoch={}\n", now()).as_bytes()).unwrap();
    let o = r.run(&["release", &r.s("l"), "--holder", "a@x"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!r.p("l").exists());
}

#[test]
fn op1_to_op3_an_option_that_does_not_apply_is_64_the_lock_stays() {
    let r = Rig::new();
    r.plant("l", "b", HOST, r.live(), now());
    let o = r.run(&["release", &r.s("l"), "--holder", "b@hostA", "--wait", "60"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(r.p("l").exists());
    assert!(
        o.stderr.contains("--wait does not apply to release"),
        "{}",
        o.stderr
    );
    let o = r.run(&["status", &r.s("l"), "--no-board", "--pid", "1"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr.contains("--pid does not apply to status"),
        "{}",
        o.stderr
    );
    let o = r.run(&["break", &r.s("l"), "--if-dead", "--reason", "x"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(r.p("l").exists());
    assert!(
        o.stderr.contains("--reason does not apply to break"),
        "{}",
        o.stderr
    );
}

#[test]
fn dt4_detach_and_result_go_together_64() {
    let r = Rig::new();
    let l = r.s("l4");
    for args in [
        vec!["run", &l, "--holder", "a", "--detach", "--", "true"],
        vec!["run", &l, "--holder", "a", "--result", "R", "--", "true"],
        vec!["take", &l, "--holder", "a", "--detach", "--result", "R"],
    ] {
        let o = r.run(&args);
        assert_eq!(o.code, 64, "{args:?}: {}", o.all());
    }
    assert!(!r.p("l4").exists());
}

#[test]
fn dt6_release_by_a_stranger_is_3_and_the_lock_is_complete() {
    let r = Rig::new();
    r.plant("l6", "a@hostA", HOST, r.live(), now());
    let o = r.run(&["release", &r.s("l6"), "--holder", "z@hostA"]);
    assert_eq!(o.code, 3, "{}", o.all());
    assert!(std::fs::metadata(r.p("l6/owner")).unwrap().len() > 0);
}

#[test]
fn usage_without_a_subcommand_help_and_an_unknown_word() {
    let r = Rig::new();
    // no subcommand: usage, 64
    let o = out_of(
        std::process::Command::new(BIN)
            .arg("lock")
            .output()
            .unwrap(),
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr
            .contains("usage: crewboard lock take|release|status|run <dir>"),
        "{}",
        o.stderr
    );
    // help: the text, exit 0 (also for `take --help`)
    for args in [vec!["--help"], vec!["-h"], vec!["take", "--help"]] {
        let o = r.run(&args);
        assert_eq!(o.code, 0, "{args:?}: {}", o.all());
        assert!(
            o.stdout.contains("crewboard lock take") && o.stdout.contains("Exit codes:"),
            "{}",
            o.stdout
        );
    }
    let o = r.run(&["frobnicate", "x"]);
    assert_eq!(o.code, 64, "{}", o.all());
    let o = r.run(&["take", &r.s("l"), "--holder", "a", "--bogus"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(o.stderr.contains("unknown option --bogus"));
}

#[test]
fn the_global_config_dir_may_stand_before_lock_and_a_clap_error_outside_lock_is_64_too() {
    // a clap error of another command (here: a global flag without its value) is 64 too
    let o = out_of(
        std::process::Command::new(BIN)
            .args(["--config-dir"])
            .output()
            .unwrap(),
    );
    assert_eq!(o.code, 64, "{}", o.all());
    let r = Rig::new();
    let o = out_of(
        std::process::Command::new(BIN)
            .args([
                "--config-dir",
                &r.s("cfg"),
                "lock",
                "status",
                &r.s("l"),
                "--no-board",
            ])
            .env_remove("CREWBOARD_HOST_LOCK")
            // an empty process table: a CI job of the real host must not change the answer
            .env("HOST_LOCK_HOST", HOST)
            .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
            .output()
            .unwrap(),
    );
    assert_eq!((o.code, o.stdout.trim()), (1, "free"), "{}", o.all());
}
