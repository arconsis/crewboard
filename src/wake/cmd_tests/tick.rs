//! The headless coordinator: `--tick`.

use super::*;

#[test]
fn co1_tick_runs_its_lines_go_to_the_log_and_no_model_starts_and_no_poll_runs() {
    let rig = Rig::new();
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(st.calls, vec!["tick"]);
    assert_eq!(rig.wakes(), 0);
    assert_eq!(rig.stderr(), "");
    let log = rig.log();
    assert!(
        log.contains("poll-and-wake: tick: tick: verified and merged demo/app#5\n"),
        "{log}"
    );
    assert!(log.contains("poll-and-wake: tick: left for the interactive coordinator: demo/app#6, the Gate is not CI only\n"));
    assert!(!rig.lock().exists(), "the lock is released");
    assert!(
        rig.state_text("last-tick")
            .unwrap()
            .starts_with("tick: verified")
    );
}

#[test]
fn co2_the_same_output_again_and_an_idle_tick_write_no_new_line() {
    let rig = Rig::new();
    let mut st = Scripted::new(&[TICKOUT, TICKOUT, "0|tick: no ticket in Review|"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    let before = rig.log().lines().count();
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(rig.log().lines().count(), before);
    assert_eq!(st.count("tick"), 3);
    // the idle tick replaced last-tick, so the busy output is news again
    let mut st = Scripted::new(&[TICKOUT]);
    rig.run(&mut st, &["--tick"]);
    assert!(rig.log().lines().count() > before);
}

#[test]
fn co3_a_refused_tick_is_logged_with_its_reason_and_exits_2() {
    let rig = Rig::new();
    let mut st =
        Scripted::new(&["2||refused [rule=tick-headless-only] tick is for a headless config dir"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 2);
    assert!(
        rig.log()
            .contains("crewboard tick failed: exit 2: refused [rule=tick-headless-only]")
    );
    assert!(!rig.lock().exists());
}

#[test]
fn a_tick_with_exit_3_retries_and_counts_a_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down"]);
    assert_eq!(rig.run(&mut st, &["--tick", "--retry-delays", "0"]), 3);
    assert!(
        rig.log()
            .contains("crewboard tick: exit 3 (try 1): down; retry in 0 s")
    );
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    // a good tick clears it
    let mut st = Scripted::new(&["0|tick: no ticket in Review|"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert!(rig.state_text("net-backoff").is_none());
}

#[test]
fn a_tick_under_a_live_lock_starts_no_second_tick() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW);
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert!(st.calls.is_empty());
    assert!(rig.log().contains("already runs"));
}
