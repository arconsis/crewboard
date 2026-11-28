//! `run --detach --result FILE` as the real binary: the caller returns at
//! once, the job holds the lock in a session of its own, writes the result file and releases.
#![cfg(unix)]

use crate::support::lock::*;
use std::process::Stdio;
use std::time::Instant;

fn detach(r: &Rig, name: &str, res: &str, extra: &[&str], cmd: &[&str]) -> Out {
    let l = r.s(name);
    let resf = r.s(res);
    let mut a = vec![
        "run",
        l.as_str(),
        "--holder",
        "a@hostA",
        "--no-board",
        "--detach",
        "--result",
        resf.as_str(),
    ];
    a.extend_from_slice(extra);
    a.push("--");
    a.extend_from_slice(cmd);
    r.run(&a)
}

fn job_pid(out: &str) -> u32 {
    out.strip_prefix("detached: pid=")
        .and_then(|r| r.split(' ').next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no pid in: {out}"))
}

/// The lock dir is gone: the job wrote its result and released.
fn lock_gone(r: &Rig, name: &str) -> bool {
    wait_for(20, || !r.p(name).exists())
}

#[test]
fn dt1_detach_returns_at_once_the_job_holds_the_lock_with_its_pid_and_detached_1() {
    let r = Rig::new();
    let t0 = Instant::now();
    let o = detach(
        &r,
        "l1",
        "r1",
        &["--reason", "job"],
        &["sh", "-c", "sleep 4; exit 5"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        t0.elapsed().as_secs() < 3,
        "returns at once: {:?}",
        t0.elapsed()
    );
    let jp = job_pid(o.stdout.trim());
    assert!(pid_alive(jp));
    assert!(!r.p("r1").exists(), "no result yet");
    assert_eq!(owner_field(&r.p("l1"), "pid"), Some(jp.to_string()));
    assert_eq!(owner_field(&r.p("l1"), "detached").as_deref(), Some("1"));
    assert!(
        o.stdout.contains("r1.log result=") && o.stdout.trim().ends_with("r1"),
        "{}",
        o.stdout
    );
    // another taker gets 75; status says it is a detached job
    let t = r.run(&[
        "run",
        &r.s("l1"),
        "--holder",
        "b@hostA",
        "--no-board",
        "--",
        "true",
    ]);
    let st = r.run(&["status", &r.s("l1"), "--no-board"]);
    assert_eq!(t.code, 75, "{}", t.all());
    assert_eq!(st.code, 0, "{}", st.all());
    assert!(st.stdout.contains("[detached job]"), "{}", st.stdout);
    // the result file appears with rc, end and log; then the lock is free
    assert!(wait_for(20, || r.p("r1").exists()));
    assert!(lock_gone(&r, "l1"));
    let got = std::fs::read_to_string(r.p("r1")).unwrap();
    let line = got.lines().next().unwrap();
    assert!(
        line.starts_with("rc=5 end=20") && line.contains(" log=") && line.ends_with("r1.log"),
        "{got}"
    );
}

#[test]
fn dt2b_ticket_and_sha_add_their_lines_the_first_line_stays() {
    let r = Rig::new();
    let o = detach(
        &r,
        "l2b",
        "r2b",
        &["--ticket", "42", "--sha", "abc123"],
        &["sh", "-c", "exit 0"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(wait_for(20, || r.p("r2b").exists()));
    assert!(lock_gone(&r, "l2b"));
    let got = std::fs::read_to_string(r.p("r2b")).unwrap();
    let lines: Vec<&str> = got.lines().collect();
    assert!(lines[0].starts_with("rc=0 end="), "{got}");
    assert!(
        lines.contains(&"ticket=42") && lines.contains(&"sha=abc123"),
        "{got}"
    );
}

#[test]
fn dt2c_ticket_needs_digits_and_detach() {
    let r = Rig::new();
    let o = detach(&r, "l2c", "r2c", &["--ticket", "x1"], &["true"]);
    assert_ne!(o.code, 0);
    let o2 = r.run(&[
        "run",
        &r.s("l2c"),
        "--holder",
        "a@hostA",
        "--no-board",
        "--ticket",
        "5",
        "--",
        "true",
    ]);
    assert_eq!(o2.code, 64, "{}", o2.all());
    assert!(!r.p("r2c").exists() && !r.p("l2c").exists());
}

#[test]
fn dt3_a_kill_of_the_caller_group_leaves_the_job_and_its_lock_the_job_ends_later() {
    // the budget stop of a wake: TERM, then KILL, to the process group of the caller
    let r = Rig::new();
    let (l, res, out) = (r.s("l3"), r.s("r3"), r.s("w3.out"));
    let mut wake = std::process::Command::new("sh");
    wake.args([
        "-c",
        &format!(
            "{BIN} lock run {l} --holder a@hostA --no-board --detach --result {res} -- sh -c 'sleep 4; echo done3' >{out} 2>&1; sleep 60"
        ),
    ])
    .env_remove("CREWBOARD_WAKE")
    .env("HOST_LOCK_HOST", HOST)
    .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
    .stdin(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut wake, 0);
    let mut w = wake.spawn().unwrap();
    let pgid = w.id() as i32;
    assert!(wait_for(10, || std::fs::read_to_string(&out)
        .is_ok_and(|t| t.contains("detached:"))));
    unsafe {
        libc::kill(-pgid, libc::SIGTERM);
    }
    // a real wait: the group gets TERM, then KILL after a pause, as the budget stop of a wake does
    std::thread::sleep(std::time::Duration::from_millis(500));
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
    w.wait().unwrap();
    let jp = owner_field(&r.p("l3"), "pid").unwrap_or_default();
    assert!(
        jp.parse::<u32>().is_ok_and(pid_alive),
        "the job is alive: {jp:?}"
    );
    assert_eq!(owner_field(&r.p("l3"), "detached").as_deref(), Some("1"));
    assert!(!r.p("r3").exists());
    // the job finishes after the wake: result rc=0, lock released
    assert!(wait_for(20, || r.p("r3").exists()));
    assert!(lock_gone(&r, "l3"));
    assert!(
        std::fs::read_to_string(r.p("r3"))
            .unwrap()
            .starts_with("rc=0 ")
    );
    assert!(
        std::fs::read_to_string(r.p("r3.log"))
            .unwrap()
            .contains("done3")
    );
}

#[test]
fn dt5_a_detached_release_leaves_no_moment_with_an_empty_lock_dir() {
    let r = Rig::new();
    let lock = r.p("l5");
    let o = detach(&r, "l5", "r5", &[], &["sh", "-c", "sleep 1"]);
    assert_eq!(o.code, 0, "{}", o.all());
    // the job holds the lock now. From here to its release, the lock dir is complete or gone:
    // the taker's own mkdir-then-owner moment is over.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (l2, s2, e2) = (lock.clone(), stop.clone(), seen.clone());
    let watcher = std::thread::spawn(move || {
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(rd) = std::fs::read_dir(&l2)
                && rd.count() == 0
                && l2.is_dir()
            {
                e2.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    });
    assert!(wait_for(20, || r.p("r5").exists()));
    assert!(lock_gone(&r, "l5"));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    watcher.join().unwrap();
    assert!(
        !seen.load(std::sync::atomic::Ordering::SeqCst),
        "the watcher saw an empty lock dir"
    );
}

#[test]
fn dt7_the_job_supervisor_is_a_session_leader_of_its_own() {
    let r = Rig::new();
    let o = detach(&r, "l7", "r7", &[], &["sh", "-c", "sleep 3"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let sup = job_pid(o.stdout.trim());
    // SAFETY: getsid only reads the session id.
    let (mine, theirs) = unsafe { (libc::getsid(0), libc::getsid(sup as i32)) };
    assert_eq!(theirs, sup as i32, "the supervisor leads its own session");
    assert_ne!(theirs, mine, "and it is not the session of the caller");
    assert!(wait_for(20, || r.p("r7").exists()));
}

#[test]
fn term_of_supervisor_reaches_the_group_of_the_job() {
    let r = Rig::new();
    let kid = r.s("kid.pid");
    let o = detach(
        &r,
        "lt",
        "rt",
        &[],
        // `exec`: the leader of the group is then a `sleep`, which a TERM always ends with the
        // signal. A shell that waits in `wait` can exit 0 instead when its child dies first
        // (bash as `sh` did so in 9 of 300 runs).
        &[
            "sh",
            "-c",
            &format!("sleep 300 & echo $! >{kid}; exec sleep 300"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let sup = job_pid(o.stdout.trim());
    assert!(wait_for(10, || std::fs::read_to_string(&kid)
        .is_ok_and(|t| !t.trim().is_empty())));
    let sleeper: u32 = std::fs::read_to_string(&kid)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pid_alive(sleeper));
    // TERM to the supervisor only: it goes to the whole group of the job (the shell AND its sleep)
    unsafe {
        libc::kill(sup as i32, libc::SIGTERM);
    }
    assert!(wait_for(10, || r.p("rt").exists()));
    let got = std::fs::read_to_string(r.p("rt")).unwrap();
    assert!(
        got.starts_with("rc=143 "),
        "a job ended by TERM has 128 + 15: {got}"
    );
    assert!(
        wait_for(10, || !pid_alive(sleeper)),
        "the grandchild got the TERM too"
    );
    assert!(lock_gone(&r, "lt"));
}

#[test]
fn supervisor_failed_release_is_logged() {
    let r = Rig::new();
    let l = r.s("lf");
    // the job puts another agent in the owner file: the supervisor's release is refused (3)
    let script = format!(
        "printf 'agent=thief@x\\nhost=hostA\\npid=1\\nstarted=x\\nstarted_epoch=1\\n' >{l}/owner"
    );
    let o = detach(&r, "lf", "rf", &[], &["sh", "-c", &script]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(wait_for(20, || r.p("rf").exists()));
    assert!(wait_for(10, || {
        std::fs::read_to_string(r.p("rf.log")).is_ok_and(|t| t.contains("was not released"))
    }));
    let log = std::fs::read_to_string(r.p("rf.log")).unwrap();
    assert!(log.contains("refused:"), "{log}");
    assert!(r.p("lf").exists(), "a lock that is not ours stays");
}

#[test]
fn a_detached_run_inside_a_re_entered_lock_is_64() {
    let r = Rig::new();
    let (l, res) = (r.s("lr"), r.s("rr"));
    let o = r.run(&[
        "run", &l, "--holder", "outer@x", "--no-board", "--", "sh", "-c",
        &format!(
            "{BIN} lock run {l} --holder inner@x --no-board --detach --result {res} -- true 2>{l}.err; echo $? >{l}.rc"
        ),
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(std::fs::read_to_string(r.p("lr.rc")).unwrap().trim(), "64");
    assert!(
        std::fs::read_to_string(r.p("lr.err"))
            .unwrap()
            .contains("cannot detach a job inside a lock that your parent process holds")
    );
}

/// The same start again, with a wait that would be long.
fn start_again(r: &Rig, name: &str, res: &str, extra: &[&str]) -> (Out, std::time::Duration) {
    let t0 = Instant::now();
    let mut e = vec!["--wait", "60"];
    e.extend_from_slice(extra);
    let o = detach(r, name, res, &e, &["sh", "-c", "exit 3"]);
    (o, t0.elapsed())
}

#[test]
fn dt8_a_second_start_is_refused_while_the_job_runs_and_while_its_result_is_unread() {
    let r = Rig::new();
    let o = detach(&r, "l8", "r8", &[], &["sh", "-c", "sleep 4; exit 0"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let jp = job_pid(o.stdout.trim());
    // the owner file names the result file of the job
    let named = owner_field(&r.p("l8"), "result").unwrap_or_default();
    assert!(named.ends_with("/r8"), "{named}");
    // 1. the job still runs: exit 1 at once (no wait for the lock), and the job goes on
    let (o2, took) = start_again(&r, "l8", "r8", &[]);
    assert_eq!(o2.code, 1, "{}", o2.all());
    assert!(took.as_secs() < 3, "no wait for the lock: {took:?}");
    assert!(
        o2.stderr.contains(&format!(
            "host-lock: the detached job pid={jp} still runs and writes {named}; not starting it again. End the wake: the next wake gets the event `detached job finished`"
        )),
        "{}",
        o2.stderr
    );
    assert!(o2.stdout.is_empty(), "{}", o2.stdout);
    assert_eq!(owner_field(&r.p("l8"), "pid"), Some(jp.to_string()));
    // 2. the job has ended and nobody has reported its result: exit 1, the result stays
    assert!(wait_for(20, || r.p("r8").exists()));
    assert!(lock_gone(&r, "l8"));
    let first = std::fs::read_to_string(r.p("r8")).unwrap();
    let rc_line = first.lines().next().unwrap().to_string();
    assert!(rc_line.starts_with("rc=0 end="), "{first}");
    let (o3, _) = start_again(&r, "l8", "r8", &[]);
    assert_eq!(o3.code, 1, "{}", o3.all());
    assert!(
        o3.stderr.contains(&format!(
            "host-lock: the detached job has finished: {named} holds `{rc_line}` and no wake has reported it; not starting the job again. Handle that result first: the next wake reports it as the event `detached job finished` (rc=0: run done; another rc: fix the cause). To start the job again on purpose, add --again"
        )),
        "{}",
        o3.stderr
    );
    assert_eq!(std::fs::read_to_string(r.p("r8")).unwrap(), first);
    assert!(!r.p("l8").exists(), "the lock is not taken");
    // 3. the wake reported it (`.seen` holds the rc line): the old result goes and the job starts
    std::fs::write(r.p("r8.seen"), format!("{rc_line}\n")).unwrap();
    let (o4, _) = start_again(&r, "l8", "r8", &[]);
    assert_eq!(o4.code, 0, "{}", o4.all());
    assert!(o4.stdout.starts_with("detached: pid="), "{}", o4.stdout);
    assert!(wait_for(20, || {
        std::fs::read_to_string(r.p("r8")).is_ok_and(|t| t.starts_with("rc=3 "))
    }));
    assert!(lock_gone(&r, "l8"));
}

#[test]
fn dt9_again_starts_over_an_unread_result_and_another_result_file_waits_for_the_lock() {
    let r = Rig::new();
    std::fs::write(r.p("r9"), "rc=1 end=2026-10-21T10:00:00Z log=l\nticket=9\n").unwrap();
    // control: without --again the unread result stops the start
    let (no, _) = start_again(&r, "l9", "r9", &[]);
    assert_eq!(no.code, 1, "{}", no.all());
    // --again: the unread result goes, the job starts
    let o = detach(
        &r,
        "l9",
        "r9",
        &["--again"],
        &["sh", "-c", "sleep 4; exit 0"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        !r.p("r9").exists(),
        "the old result is removed at the start"
    );
    // --again does not start a job that still runs
    let (twice, _) = start_again(&r, "l9", "r9", &["--again"]);
    assert_eq!(twice.code, 1, "{}", twice.all());
    assert!(twice.stderr.contains("still runs"), "{}", twice.stderr);
    // a job under another result file is another job: the lock rules apply as before (no wait
    // given: 75, held by a live owner)
    let other = detach(&r, "l9", "r9b", &[], &["true"]);
    assert_eq!(other.code, 75, "{}", other.all());
    assert!(other.stderr.contains("held by"), "{}", other.stderr);
    assert!(!other.stderr.contains("still runs"), "{}", other.stderr);
    assert!(wait_for(20, || {
        std::fs::read_to_string(r.p("r9")).is_ok_and(|t| t.starts_with("rc=0 "))
    }));
    assert!(lock_gone(&r, "l9"));
}

#[test]
fn dt10_a_result_that_appears_during_the_wait_for_the_lock_is_not_removed() {
    // the owner file of an older release has no `result=` line: the second start waits for the
    // lock. The first job writes its result before it releases; the second start must keep it.
    let r = Rig::new();
    let (l, res) = (r.s("l10"), r.s("r10"));
    let mut hold = r
        .cmd(&[
            "run",
            &l,
            "--holder",
            "old@hostA",
            "--no-board",
            "--",
            "sh",
            "-c",
            &format!("sleep 2; echo 'rc=0 end=2026-10-21T10:00:00Z log=l' >{res}"),
        ])
        .spawn()
        .unwrap();
    assert!(wait_for(10, || r.p("l10").join("owner").exists()));
    let (o, took) = start_again(&r, "l10", "r10", &["--poll", "0.2"]);
    hold.wait().unwrap();
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(took.as_millis() > 500, "it waited for the lock: {took:?}");
    assert!(
        o.stderr.contains("the detached job has finished: "),
        "{}",
        o.stderr
    );
    assert_eq!(
        std::fs::read_to_string(r.p("r10")).unwrap(),
        "rc=0 end=2026-10-21T10:00:00Z log=l\n"
    );
    assert!(
        lock_gone(&r, "l10"),
        "the lock taken for the start is released"
    );
}
