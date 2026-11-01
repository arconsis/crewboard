//! The receipts of the events: `acted`.

use super::*;

fn acw(rig: &Rig, st: &mut Scripted, cmd: &str) -> u8 {
    let evf = ev(rig);
    rig.run(
        st,
        &[
            "--events-file",
            &evf,
            "--wake-cmd",
            cmd,
            "--work-backoff",
            "off",
        ],
    )
}

#[test]
fn ac1_receipts_are_logged_and_a_wake_that_acted_gets_no_redelivery() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12|"]);
    st.acted_list = "demo/app#12 assignment\ndemo/app#7 info\nowner-message\n".into();
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let lg = rig.log();
    for r in ["demo/app#12 assignment", "demo/app#7 info", "owner-message"] {
        assert!(
            lg.contains(&format!("poll-and-wake: event delivered to wake: {r}\n")),
            "{lg}"
        );
    }
    assert!(!lg.contains("not acted on"));
    assert!(!rig.p("ev.txt.unhandled").exists());
    assert_eq!(st.acted_calls.len(), 2);
    let evf = ev(&rig);
    assert!(
        st.acted_calls[1].starts_with(&format!("--events-file {evf} --since 20")),
        "{:?}",
        st.acted_calls
    );
    assert!(st.acted_calls[1].ends_with(&format!(" --redeliver-to {evf}.unhandled")));
}

#[test]
fn ac2_one_miss_is_delivered_again_once_and_a_second_miss_only_logs() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|", "1||", "1||"]);
    st.acted_list = "demo/app#12 question\n".into();
    st.acted_check = vec!["demo/app#12 question".into()];
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert_eq!(
        kept.matches("REDELIVERED demo/app#12 question").count(),
        1,
        "{kept}"
    );
    assert!(
        rig.log()
            .contains("poll-and-wake: event not acted on: demo/app#12 question; delivered again\n")
    );
    // the next wake gets it at the top of its file; a second miss only logs
    let cmd = copy_events(&rig, "seen");
    st.plan.push_back((0, String::new(), String::new()));
    assert_eq!(acw(&rig, &mut st, &cmd), 0);
    let seen = read(&rig.p("seen"));
    assert!(seen.starts_with(crate::wake::events::EARLIER), "{seen}");
    assert!(
        seen.lines()
            .any(|l| l == "REDELIVERED demo/app#12 question"),
        "{seen}"
    );
    let lg = rig.log();
    assert!(
        lg.contains(
            "poll-and-wake: event not acted on twice: demo/app#12 question; not delivered again\n"
        ),
        "{lg}"
    );
    assert_eq!(
        lg.matches("event not acted on: ").count(),
        1,
        "the first miss is logged once"
    );
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the second miss delivers nothing again"
    );
}

#[test]
fn ac3_a_failing_wake_keeps_the_whole_file_and_runs_no_check() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|"]);
    st.acted_list = "demo/app#12 question\n".into();
    st.acted_check = vec!["demo/app#12 question".into()];
    assert_eq!(acw(&rig, &mut st, "exit 9"), 70);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("EVENT 1: ask on #12") && !kept.contains("REDELIVERED"),
        "{kept}"
    );
    assert_eq!(st.acted_calls.len(), 1, "only the receipts, no check");
    assert!(!rig.log().contains("not acted on"));
}

#[test]
fn ac5_a_check_that_fails_is_logged_and_changes_nothing() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|"]);
    st.acted_rc = 2;
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let lg = rig.log();
    assert!(lg.contains("event receipts not written: crewboard acted exit 2: error: unrecognized subcommand 'acted'"), "{lg}");
    assert!(
        lg.contains("event check failed: crewboard acted exit 2: "),
        "{lg}"
    );
    assert!(lg.contains("wake finished OK"));
    assert!(!rig.p("ev.txt.unhandled").exists());
}

#[test]
fn ac5b_exit_4_of_the_check_stops_the_wakes() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let s = rig
        .settings(
            &[
                "--events-file",
                &evf,
                "--wake-cmd",
                "exit 0",
                "--work-backoff",
                "off",
            ],
            &[],
        )
        .unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    // the receipts pass, the check says 4
    let mut steps = FourOnCheck {
        inner: Scripted::new(&["0||", "0|EVENT 1|"]),
    };
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let rc = Flow {
        s: &s,
        steps: &mut steps,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    }
    .execute();
    assert_eq!(rc, 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" acted: key gone")
    );
    assert!(!rig.log().contains("wake finished OK"));
}

/// Steps whose second `acted` call (the check) exits with 4.
struct FourOnCheck {
    inner: Scripted,
}

impl Steps for FourOnCheck {
    fn poll(&mut self, q: bool, o: &mut Vec<u8>) -> StepRc {
        self.inner.poll(q, o)
    }
    fn mine_quiet(&mut self) -> StepRc {
        self.inner.mine_quiet()
    }
    fn mine_text(&mut self, o: &mut Vec<u8>) -> StepRc {
        self.inner.mine_text(o)
    }
    fn say(&mut self, t: &str, b: &str, a: bool) -> StepRc {
        self.inner.say(t, b, a)
    }
    fn tick(&mut self, o: &mut Vec<u8>) -> StepRc {
        self.inner.tick(o)
    }
    fn acted_list(&mut self, e: &Path, o: &mut Vec<u8>) -> StepRc {
        self.inner.acted_list(e, o)
    }
    fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
        StepRc::from_stderr(4, "key gone")
    }
    fn heartbeat(&mut self) -> StepRc {
        self.inner.heartbeat()
    }
    fn pr_head(&mut self, r: &str, n: u64) -> Result<Option<PrHead>, StepRc> {
        self.inner.pr_head(r, n)
    }
    fn open_prs(&mut self, r: &str) -> Result<Vec<OpenPrRef>, StepRc> {
        self.inner.open_prs(r)
    }
}

#[test]
fn ac6_without_an_events_file_there_is_no_receipt_and_no_check() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", "exit 0"]), 0);
    assert!(st.acted_calls.is_empty());
}
