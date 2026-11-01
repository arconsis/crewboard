//! The host locks of the agent.

use super::*;

fn lock_dir(rig: &Rig, name: &str, owner: &str, pid: u32, extra: &str) -> PathBuf {
    let d = rig.p(name);
    std::fs::create_dir_all(&d).unwrap();
    let host = crate::lock::sys::short_hostname().unwrap_or_else(|| "host".to_string());
    std::fs::write(
        d.join("owner"),
        format!("agent={owner}\nhost={host}\npid={pid}\nstarted=x\nstarted_epoch={NOW}\nreason=r\n{extra}"),
    )
    .unwrap();
    d
}

fn release_run(rig: &Rig, locks: Vec<PathBuf>, agent: Option<&str>) {
    let mut s = rig.settings(&[], &[]).unwrap();
    s.host_locks = locks;
    s.agent = agent.map(str::to_string);
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&[]);
    let flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    flow.release_host_locks();
}

#[test]
fn the_host_locks_of_the_agent_are_released_except_a_running_or_detached_job_and_a_stranger() {
    let rig = Rig::new();
    let gone = lock_dir(&rig, "gone.lock.d", "dev@host-a", dead_pid(), "");
    let running = lock_dir(&rig, "running.lock.d", "dev@host-a", std::process::id(), "");
    let detached = lock_dir(
        &rig,
        "detached.lock.d",
        "dev@host-a",
        dead_pid(),
        "detached=1\n",
    );
    let other = lock_dir(&rig, "other.lock.d", "other@host-b", std::process::id(), "");
    release_run(
        &rig,
        vec![
            gone.clone(),
            running.clone(),
            detached.clone(),
            other.clone(),
        ],
        Some("dev@host-a"),
    );
    assert!(!gone.exists(), "released: {}", rig.log());
    assert!(running.exists() && detached.exists(), "kept: {}", rig.log());
    assert!(other.exists(), "not ours: {}", rig.log());
    let log = rig.log();
    assert!(log.contains("running.lock.d: kept for dev@host-a"), "{log}");
    assert!(
        log.contains("detached.lock.d: kept for dev@host-a"),
        "{log}"
    );
    assert!(
        log.contains("gone.lock.d: released for dev@host-a"),
        "{log}"
    );
    assert!(log.contains("other.lock.d: not released"), "{log}");
    // without an agent nothing is released
    let rig = Rig::new();
    let gone = lock_dir(&rig, "gone.lock.d", "dev@host-a", dead_pid(), "");
    release_run(&rig, vec![gone.clone()], None);
    assert!(gone.exists());
    assert!(
        rig.log()
            .contains("host locks not released: --agent is not set")
    );
}

#[test]
fn a_signal_during_the_wake_stops_it_and_releases_the_host_locks() {
    let rig = Rig::new();
    let marker = rig.p("started");
    let wake = format!("touch '{}'; sleep 30", marker.display());
    let mut s = rig.settings(&["--wake-cmd", &wake], &[]).unwrap();
    let lock = lock_dir(&rig, "held.lock.d", "dev@host-a", dead_pid(), "");
    s.host_locks = vec![lock.clone()];
    s.agent = Some("dev@host-a".into());
    let t0 = std::time::Instant::now();
    let rc = rig.run_signals(&mut Scripted::new(&["0||"]), &s, &|| {
        marker.exists().then_some(libc::SIGTERM)
    });
    assert_eq!(rc, 128 + u8::try_from(libc::SIGTERM).unwrap());
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "the sleep was stopped"
    );
    assert!(!lock.exists(), "released on the signal path: {}", rig.log());
    assert!(!rig.lock().exists());
}

#[test]
fn no_in_progress_ticket_is_one_log_line_and_no_message() {
    let rig = Rig::new();
    // `mine` exits 1 when no owned ticket needs the agent now, and still prints the rows
    let mut st = Scripted::new(&["0||"]).with_mine(&[(1, MINE12)]);
    let rc = rig.run(&mut st, &["--wake-cmd", "sleep 30", "--max-wake", "1s"]);
    assert_eq!(rc, 0);
    assert_eq!(st.count("say"), 0);
    assert!(
        rig.log()
            .contains("no In Progress ticket: no message to the coordinator"),
        "{}",
        rig.log()
    );
    // a say that fails is logged with its exit code
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "2||refused [rule=x]: no"]).with_mine(&[(0, MINE12)]);
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", "sleep 30", "--max-wake", "1s"]),
        0
    );
    assert!(
        rig.log()
            .contains("say on demo/app#12 failed: exit 2: refused [rule=x]: no"),
        "{}",
        rig.log()
    );
}
