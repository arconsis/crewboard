//! The precheck and the quiet polls.

use super::*;

#[test]
fn p1_twenty_quiet_polls_start_no_wake_and_say_nothing() {
    let rig = Rig::new();
    let mut st = Scripted::new(&rep(20, "1||"));
    for _ in 0..20 {
        assert_eq!(rig.run(&mut st, &[]), 0);
    }
    assert_eq!(rig.wakes(), 0);
    assert_eq!(st.count("poll --quiet"), 20);
    assert_eq!(rig.stderr(), "");
    assert_eq!(rig.log(), "");
}

#[test]
fn p2_one_exit_0_among_twenty_polls_gives_exactly_one_wake_and_releases_the_lock() {
    let rig = Rig::new();
    let mut plan = rep(7, "1||");
    plan.push("0||");
    plan.extend(rep(12, "1||"));
    let mut st = Scripted::new(&plan);
    for _ in 0..20 {
        assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    }
    assert_eq!(rig.wakes(), 1);
    assert!(!rig.lock().exists(), "the lock is released");
}

#[test]
fn p3_exit_3_is_rc_3_with_no_wake_and_is_logged() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||network down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log()
            .contains("crewboard poll failed: exit 3: network down"),
        "{}",
        rig.log()
    );
}

#[test]
fn p3b_exit_4_passes_through_and_a_missing_wake_command_is_refused() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||no key"]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert_eq!(rig.wakes(), 0);
    // no wake command and no --tick
    let a = parse(&["--state-dir", "/s"]);
    let e = resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap_err();
    assert_eq!(
        e.text,
        "no wake command (--wake-cmd, CREWBOARD_WAKE_CMD or --tick)"
    );
    assert_eq!(e.code, 64, "a missing command is a usage error");
    assert!(e.paths.is_none(), "the line comes before the log is known");
    // the other direction: the environment gives the wake command, and so does --tick
    let env = |k: &str| (k == "CREWBOARD_WAKE_CMD").then(|| "true".to_string());
    assert!(resolve(&a, &GlobalFlags::default(), &env, None).is_ok());
    let t = parse(&["--state-dir", "/s", "--tick"]);
    assert!(resolve(&t, &GlobalFlags::default(), &|_| None, None).is_ok());
}

#[test]
fn p3d_a_failing_poll_logs_its_first_stderr_line_cut_and_cleaned() {
    let rig = Rig::new();
    let long = "Z".repeat(300);
    let mut st = Scripted::new(&[&format!(
        "4||configuration error: no board file for initiative demo\x1b[31m {long}\nhint: run crewboard join first"
    )]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    let log = rig.log();
    let line = log
        .lines()
        .find(|l| l.contains("STOPPED: crewboard poll exit 4"))
        .unwrap();
    assert!(line.contains("exit 4: configuration error: no board file for initiative demo"));
    assert!(!line.contains("hint:") && !line.contains('\x1b'));
    let reason = line
        .split("exit 4: ")
        .nth(1)
        .unwrap()
        .split(". No more wakes")
        .next()
        .unwrap();
    assert!(reason.chars().count() <= 200, "{}", reason.chars().count());
    assert!(rig.stderr().contains("configuration error"));
}

#[test]
fn p3e_a_quiet_poll_logs_nothing_at_all() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||configuration error: never shown"]);
    assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    assert_eq!(rig.log(), "");
    assert_eq!(rig.stderr(), "");
}

#[test]
fn q1_quiet_polls_inside_one_hour_write_no_line_and_are_counted() {
    let rig = Rig::new();
    let mut st = Scripted::new(&rep(5, "1||"));
    for _ in 0..5 {
        assert_eq!(rig.run(&mut st, &[]), 0);
    }
    assert_eq!((rig.log(), rig.stderr()), (String::new(), String::new()));
    let q = Quiet::parse(&rig.state_text("wake.log.quiet").unwrap()).unwrap();
    assert_eq!(q.n, 5);
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn q2_the_first_quiet_poll_after_the_hour_writes_one_summary_line() {
    let rig = Rig::new();
    rig.write_state(
        "wake.log.quiet",
        &format!("{} 2026-01-01T00:00:00Z 5\n", NOW - 3601),
    );
    let mut st = Scripted::new(&rep(2, "1||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.stderr(), "");
    let log = rig.log();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.ends_with("poll-and-wake: quiet: 6 polls since 2026-01-01T00:00:00Z\n"),
        "{log}"
    );
    let after = Quiet::parse(&rig.state_text("wake.log.quiet").unwrap()).unwrap();
    assert_eq!(after.n, 0);
    // the next quiet poll writes no line and starts counting
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.log().lines().count(), 1);
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        1
    );
}

#[test]
fn q3_a_wake_logs_as_before_and_a_broken_count_file_restarts() {
    let rig = Rig::new();
    rig.write_state("wake.log.quiet", "junk\n");
    let mut st = Scripted::new(&["1||", "0||", "1||"]);
    for _ in 0..3 {
        rig.run(&mut st, &[]);
    }
    assert_eq!(rig.wakes(), 1);
    let log = rig.log();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(log.contains("poll-and-wake: events pending; waking: "));
    assert!(log.ends_with("poll-and-wake: wake finished OK\n"));
    assert!(!log.contains("quiet:"));
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        2
    );
}
