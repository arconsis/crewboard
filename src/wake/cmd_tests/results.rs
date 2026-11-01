//! The results of detached jobs.

use super::*;

fn rsp(rig: &Rig, st: &mut Scripted) -> u8 {
    let cmd = copy_events(rig, "seen");
    let evf = ev(rig);
    rig.run(
        st,
        &[
            "--events-file",
            &evf,
            "--wake-cmd",
            &cmd,
            "--work-backoff",
            "off",
        ],
    )
}

fn put_result(rig: &Rig, name: &str, text: &str) {
    std::fs::create_dir_all(rig.state().join("results")).unwrap();
    std::fs::write(rig.state().join("results").join(name), text).unwrap();
}

#[test]
fn rs1_a_finished_result_wakes_a_quiet_poll_once_with_its_event_line() {
    let rig = Rig::new();
    let log = rig.state().join("results/7.result.log");
    put_result(
        &rig,
        "7.result",
        &format!(
            "rc=0 end=2026-10-21T10:00:00Z log={}\nticket=7\n",
            log.display()
        ),
    );
    let mut st = Scripted::new(&["1||", "1||", "1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    let want = format!(
        "detached job finished: ticket 7 rc=0 log={} result={}/results/7.result",
        log.display(),
        rig.state().display()
    );
    assert!(t.lines().any(|l| l == want), "{t}");
    assert!(
        rig.log()
            .contains("a detached job has finished (result file in ")
    );
    // the result is marked: the next poll is quiet
    std::fs::remove_file(rig.p("seen")).unwrap();
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(!rig.p("seen").exists());
    // a new run of the same job is reported again, without a ticket line
    put_result(
        &rig,
        "7.result",
        "rc=1 end=2026-10-21T11:00:00Z log=a.log\n",
    );
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 2);
    assert!(read(&rig.p("ev.txt")).contains("detached job finished: ticket unknown rc=1 "));
}

#[test]
fn rs2_an_unfinished_result_wakes_nothing_and_is_not_marked() {
    let rig = Rig::new();
    put_result(&rig, "8.result", "ticket=8\n");
    let mut st = Scripted::new(&["1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 0);
    put_result(
        &rig,
        "10.result",
        "rc=0 end=2026-10-21T10:00:00Z log=a.log\nticket=10\n",
    );
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("ticket 10 rc=0") && !t.contains("ticket 8"),
        "{t}"
    );
    assert!(!rig.state().join("results/8.result.seen").exists());
    // a script with rc= at column 0 is no result
    put_result(
        &rig,
        "239-chain.sh",
        "#!/bin/bash\nrc=${PIPESTATUS[0]}\nrc=0 end=x log=y\n",
    );
    let before = rig.wakes();
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), before);
}

#[test]
fn rs3_a_result_of_another_agents_dir_is_not_read() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.p("other/results")).unwrap();
    std::fs::write(
        rig.p("other/results/9.result"),
        "rc=0 end=x log=x.log\nticket=9\n",
    )
    .unwrap();
    let mut st = Scripted::new(&["1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 0);
    put_result(&rig, "11.result", "rc=3 end=x log=y.log\nticket=11\n");
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("ticket 11 rc=3") && !t.contains("ticket 9"),
        "{t}"
    );
    assert!(!rig.p("other/results/9.result.seen").exists());
}

#[test]
fn events_kept_by_an_earlier_wake_wake_a_quiet_board_only_when_they_have_content() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let go = |st: &mut Scripted| rig.run(st, &["--events-file", &evf, "--work-backoff", "off"]);
    let mut st = Scripted::new(&["1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 0);
    std::fs::write(rig.p("ev.txt.unhandled"), "").unwrap();
    let mut st = Scripted::new(&["1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 0, "an empty .unhandled is nothing");
    std::fs::write(rig.p("ev.txt.unhandled"), "== events\n").unwrap();
    let mut st = Scripted::new(&["1||", "1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains(&format!(
        "events kept from an earlier wake are in {evf}.unhandled"
    )));
    // without an events file the kept events are not looked at
    std::fs::write(rig.p("ev.txt.unhandled"), "== events\n").unwrap();
    let mut st = Scripted::new(&["1||"]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert_eq!(rig.wakes(), 1);
}
