//! The wake and its lock.

use super::*;

#[test]
fn p3c_a_failing_wake_command_is_rc_70_and_the_lock_is_released() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", "exit 9"]), 70);
    assert!(!rig.lock().exists());
    assert!(rig.log().contains("wake command failed: exit 9"));
    // every wake, also a failed one, starts the back-off
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
}

#[test]
fn p4_a_lock_of_a_live_wake_starts_no_second_wake() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW - 5);
    let mut st = Scripted::new(&rep(5, "0||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains(&format!(
            "a wake already runs (pid {}, started {}); not starting another",
            std::process::id(),
            NOW - 5
        )),
        "{}",
        rig.log()
    );
    assert!(rig.lock().is_dir(), "the lock of the live wake stays");
    // twin: the same state without the lock wakes once
    std::fs::remove_dir_all(rig.lock()).unwrap();
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
}

#[test]
fn p5_a_stale_lock_of_a_dead_pid_is_taken_over_and_said() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), dead_pid(), NOW - 5);
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(
        rig.log().contains("STALE lock (pid-dead: owner pid "),
        "{}",
        rig.log()
    );
    assert!(
        rig.stderr().contains("STALE lock"),
        "the takeover is said on stderr too"
    );
}

#[test]
fn p5b_a_live_lock_over_the_budget_is_taken_a_young_one_is_not() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW - 100);
    let mut st = Scripted::new(&rep(2, "0||"));
    rig.run(&mut st, &["--stale-after", "3600"]);
    assert_eq!(rig.wakes(), 0);
    rig.run(&mut st, &["--stale-after", "10"]);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains("STALE lock (too-old: "));
}

#[test]
fn p5c_p5d_an_empty_lock_dir_waits_for_the_grace() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.lock()).unwrap();
    set_mtime(&rig.lock(), NOW - 2);
    let mut st = Scripted::new(&rep(2, "0||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 0, "a fresh empty lock dir is not taken over");
    assert!(rig.log().contains("already runs") && !rig.log().contains("STALE"));
    set_mtime(&rig.lock(), NOW - 100);
    assert_eq!(rig.run(&mut st, &["--empty-grace", "5"]), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains("STALE lock (empty-after-grace:"));
}

#[test]
fn a_lock_that_cannot_be_taken_is_rc_71() {
    let rig = Rig::new();
    // a file where the parent of the lock should be
    std::fs::create_dir_all(rig.state()).unwrap();
    std::fs::write(rig.p("blocker"), "x").unwrap();
    let lock = rig.p("blocker/wake.lock");
    let mut st = Scripted::new(&["0||"]);
    let s = rig
        .settings(
            &[
                "--lock-dir",
                lock.to_str().unwrap(),
                "--state-dir",
                rig.state().to_str().unwrap(),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(rig.run_settings(&mut st, &s), 71);
    assert!(rig.log().contains("cannot take the lock "), "{}", rig.log());
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn the_owner_text_names_the_host_and_the_agent() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    // a wake command that reads the owner file of its own lock
    let cmd = format!(
        "cat '{}/owner' > '{}'",
        rig.lock().display(),
        rig.p("owner-seen").display()
    );
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", &cmd, "--agent", "dev@host-a"]),
        0
    );
    assert_eq!(
        std::fs::read_to_string(rig.p("owner-seen")).unwrap(),
        "poll-and-wake@testhost dev@host-a\n"
    );
    // without an agent: no trailing blank
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", &cmd]), 0);
    assert_eq!(
        std::fs::read_to_string(rig.p("owner-seen")).unwrap(),
        "poll-and-wake@testhost\n"
    );
}
