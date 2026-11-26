//! Wake commands that fail in a row, and the hourly try of a stop. Both come from one
//! host: a login of the runtime had ended, the wake command failed 2571 times (one start every
//! five minutes for days, nobody was told), and one 401 of GitHub then stopped the wakes for good.

use super::*;

/// A wake command that counts its starts and exits with `rc`.
fn counting(rig: &Rig, rc: u8) -> String {
    format!("echo wake >> '{}'; exit {rc}", rig.p("wakes").display())
}

fn fails(rig: &Rig) -> Option<NetBackoff> {
    rig.state_text("wake-fail").map(|t| NetBackoff::parse(&t))
}

#[test]
fn f1_failed_wakes_in_a_row_pause_the_next_wakes_and_tell_the_coordinator_once() {
    let rig = Rig::new();
    let cmd = counting(&rig, 9);
    let args = ["--agent", "dev@host-a", "--wake-cmd", cmd.as_str()];
    let mut st = Scripted::new(&rep(12, "0||")).with_mine(&[(0, MINE12)]);

    // 1: counted, no pause yet
    assert_eq!(rig.run(&mut st, &args), 70);
    assert_eq!(
        fails(&rig),
        Some(NetBackoff {
            fails: 1,
            until: NOW
        })
    );
    assert!(
        rig.log().ends_with("wake command failed: exit 9\n"),
        "{}",
        rig.log()
    );

    // 2: the next run starts the command again; now the wakes pause for 300 s
    assert_eq!(rig.run(&mut st, &args), 70);
    assert_eq!(
        fails(&rig),
        Some(NetBackoff {
            fails: 2,
            until: NOW + 300
        })
    );
    assert!(
        rig.log()
            .contains("wake command failed: exit 9 (2 in a row; the next wake not before "),
        "{}",
        rig.log()
    );
    assert_eq!(st.count("say "), 0, "nobody is told before the third");

    // in the pause: the run polls (the heartbeat goes on) and starts no command
    let polls = st.count("poll --quiet");
    assert_eq!(rig.run(&mut st, &args), 0);
    assert_eq!(st.count("poll --quiet"), polls + 1);
    assert_eq!(rig.wakes(), 2);
    assert!(!rig.lock().exists());

    // 3: after the pause the command starts again; the coordinator is told
    rig.clock.advance(300);
    assert_eq!(rig.run(&mut st, &args), 70);
    assert_eq!(rig.wakes(), 3);
    assert_eq!(fails(&rig).map(|b| b.until - (NOW + 300)), Some(600));
    let said = st.calls.iter().find(|c| c.starts_with("say ")).unwrap();
    assert!(
        said.starts_with(
            "say demo/app#12 anyway=true The wake command of dev@host-a failed 3 times in a row (last exit 9)."
        ),
        "{said}"
    );
    assert!(
        !said.contains(&*rig.state().to_string_lossy()),
        "no host path: {said}"
    );
    assert!(rig.log().contains("told the coordinator on demo/app#12"));

    // 4: told once only
    rig.clock.advance(600);
    assert_eq!(rig.run(&mut st, &args), 70);
    assert_eq!(st.count("say "), 1);
    assert_eq!(fails(&rig).map(|b| b.fails), Some(4));
}

#[test]
fn f2_a_wake_that_works_ends_the_count() {
    let rig = Rig::new();
    let bad = counting(&rig, 9);
    let good = counting(&rig, 0);
    let mut st = Scripted::new(&rep(6, "0||"));
    assert_eq!(rig.run(&mut st, &["--wake-cmd", bad.as_str()]), 70);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", bad.as_str()]), 70);
    rig.clock.advance(300);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", good.as_str()]), 0);
    assert_eq!(rig.state_text("wake-fail"), None);
    // the next failure is the first again: no pause
    assert_eq!(rig.run(&mut st, &["--wake-cmd", bad.as_str()]), 70);
    assert_eq!(fails(&rig).map(|b| b.fails), Some(1));
}

#[test]
fn f3_without_a_ticket_the_count_goes_on_and_nobody_is_told() {
    let rig = Rig::new();
    let cmd = counting(&rig, 1);
    let mut st = Scripted::new(&rep(6, "0||"));
    for _ in 0..3 {
        assert_eq!(rig.run(&mut st, &["--wake-cmd", cmd.as_str()]), 70);
        rig.clock.advance(3600);
    }
    assert_eq!(fails(&rig).map(|b| b.fails), Some(3));
    assert_eq!(st.count("say "), 0);
}

#[test]
fn f4_a_stop_is_tried_again_after_one_hour_and_ends_when_the_board_answers() {
    let rig = Rig::new();
    let stop = rig.state().join("stopped");
    rig.write_state("stopped", "ts poll: HTTP 401\n");
    let mut st = Scripted::new(&["1||"]);
    let args = ["--work-backoff", "off"];

    set_mtime(&stop, NOW - 3599);
    assert_eq!(rig.run(&mut st, &args), 0);
    assert!(
        st.calls.is_empty(),
        "younger than one hour: nothing is called"
    );

    set_mtime(&stop, NOW - 3600);
    assert_eq!(rig.run(&mut st, &args), 0);
    assert_eq!(st.count("poll --quiet"), 1);
    assert!(!stop.exists(), "the poll worked: the stop is over");
    let log = rig.log();
    assert!(
        log.contains("stopped for 60 min: trying the board once"),
        "{log}"
    );
    assert!(
        log.contains("stop lifted: the board calls work again"),
        "{log}"
    );
}

#[test]
fn f5_a_stop_that_holds_is_written_again_and_told_to_nobody_twice() {
    let rig = Rig::new();
    let stop = rig.state().join("stopped");
    rig.write_state("stopped", "ts poll: old reason\n");
    set_mtime(&stop, NOW - 7200);
    let mut st =
        Scripted::new(&["4||configuration error: key file missing"]).with_mine(&[(0, MINE12)]);
    assert_eq!(rig.run(&mut st, &["--agent", "dev@host-a"]), 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .ends_with(" poll: configuration error: key file missing\n")
    );
    let log = rig.log();
    assert!(
        log.contains("still stopped: crewboard poll exit 4: configuration error: key file missing"),
        "{log}"
    );
    assert!(!log.contains("STOPPED"), "{log}");
    assert_eq!(st.count("say "), 0);
    assert_eq!(st.count("mine"), 0);
    // the file is new: the next try is one hour later
    st.push("1||");
    let calls = st.calls.len();
    assert_eq!(rig.run(&mut st, &["--agent", "dev@host-a"]), 0);
    assert_eq!(st.calls.len(), calls);
}
