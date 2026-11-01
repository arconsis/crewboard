//! More cases of retry, stop and signals.

use super::*;

#[test]
fn exit_4_is_never_retried() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||bad key", "4||bad key"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0 0"]), 4);
    assert_eq!(st.count("poll"), 1, "exactly one call");
    assert!(!rig.log().contains("retry in"));
}

#[test]
fn a_good_poll_followed_by_exit_3_of_mine_does_not_reset_the_back_off() {
    // the poll succeeds (no events), `mine --quiet` fails with 3 in every try: the run is a
    // failed run, so the counter must go up, and the pause must start at the second
    let rig = Rig::new();
    let mut st =
        Scripted::new(&["1||", "1||", "1||"]).with_mine(&[(3, ""), (3, ""), (3, ""), (3, "")]);
    let args = ["--retry-delays", "0"];
    assert_eq!(rig.run(&mut st, &args), 3);
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    assert_eq!(rig.run(&mut st, &args), 3);
    let b = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b.fails, b.until), (2, NOW + 300));
    // twin: a precheck that succeeds as a whole clears it
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(1, "")]);
    assert_eq!(rig.run(&mut st, &args), 0);
    assert!(rig.state_text("net-backoff").is_none());
}

#[test]
fn a_state_file_that_cannot_be_written_is_one_log_line() {
    let rig = Rig::new();
    // a directory where the net-backoff file should go: the rename fails
    std::fs::create_dir_all(rig.state().join("net-backoff")).unwrap();
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    let log = rig.log();
    assert!(
        log.contains(&format!(
            "cannot write {}",
            rig.state().join("net-backoff").display()
        )),
        "{log}"
    );
}

#[test]
fn x1c_a_newer_agent_env_lifts_the_stop_too() {
    let rig = Rig::new();
    let cfg = rig.p("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("agent.env"), "GH_TOKEN=x\n").unwrap();
    let stop = rig.state().join("stopped");
    rig.write_state("stopped", "ts poll: reason\n");
    set_mtime(&cfg.join("agent.env"), NOW - 1000);
    set_mtime(&stop, NOW);
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    let s = Settings {
        config_dir: Some(cfg.clone()),
        ..s
    };
    let mut st = Scripted::new(&["1||"]);
    rig.run_settings(&mut st, &s);
    assert!(st.calls.is_empty(), "an older agent.env does not lift it");
    set_mtime(&cfg.join("agent.env"), NOW + 100);
    rig.run_settings(&mut st, &s);
    assert_eq!(st.count("poll --quiet"), 1);
    assert!(rig.log().contains("agent.env changed after the stop"));
}

#[test]
fn a_signal_before_the_lock_ends_the_run_with_128_plus_the_signal() {
    let rig = Rig::new();
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&["0||"]);
    let mut flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| Some(15),
        heartbeat_steps: None,
    };
    assert_eq!(flow.execute(), 143);
    assert_eq!(rig.wakes(), 0);
    assert!(!rig.lock().exists());
    assert!(rig.log().contains("stopped by signal 15"));
}

/// A signal that arrives during the first call of a run.
fn signal_in_first_call(st: &mut Scripted) -> Rc<std::cell::Cell<Option<i32>>> {
    let pending = Rc::new(std::cell::Cell::new(None));
    let p = pending.clone();
    st.hook = Some(Box::new(move || p.set(Some(15))));
    pending
}

#[test]
fn a_signal_during_a_call_ends_the_run_with_143_and_no_retry() {
    let rig = Rig::new();
    let s = rig.settings(&["--retry-delays", "5 5"], &[]).unwrap();
    let mut st = Scripted::new(&["3||down", "0||"]);
    let pending = signal_in_first_call(&mut st);
    let rc = rig.run_signals(&mut st, &s, &|| pending.take());
    assert_eq!(rc, 143);
    assert_eq!(st.count("poll"), 1, "no second call");
    assert_eq!(rig.clock.now(), NOW, "no wait before a retry");
    assert!(
        rig.state_text("net-backoff").is_none(),
        "an interrupted run is no failed run"
    );
    assert!(!rig.log().contains("retry in"));
    assert_eq!(rig.wakes(), 0);
    // twin: without the signal the retry runs
    let mut st = Scripted::new(&["3||down", "0||"]);
    assert_eq!(rig.run_signals(&mut st, &s, &|| None), 0);
    assert_eq!(st.count("poll"), 2);
}

#[test]
fn a_signal_during_tick_is_143_not_0() {
    let rig = Rig::new();
    let s = rig.settings(&["--tick"], &[]).unwrap();
    let mut st = Scripted::new(&[TICKOUT]);
    let pending = signal_in_first_call(&mut st);
    assert_eq!(rig.run_signals(&mut st, &s, &|| pending.take()), 143);
    assert!(!rig.log().contains("tick: tick:"), "{}", rig.log());
    assert!(!rig.lock().exists());
    // twin: the same tick without the signal logs its lines and exits 0
    let rig = Rig::new();
    let s = rig.settings(&["--tick"], &[]).unwrap();
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run_signals(&mut st, &s, &|| None), 0);
    assert!(rig.log().contains("tick: tick:"));
}

#[test]
fn a_signal_before_the_stop_note_ends_the_run_without_the_note() {
    let rig = Rig::new();
    let s = rig.settings(&[], &[]).unwrap();
    let mut st = Scripted::new(&["4||bad"]).with_mine(&[(0, "demo/app#3 In Progress P1\n")]);
    let pending = signal_in_first_call(&mut st);
    assert_eq!(rig.run_signals(&mut st, &s, &|| pending.take()), 143);
    assert_eq!(st.count("say"), 0);
}
