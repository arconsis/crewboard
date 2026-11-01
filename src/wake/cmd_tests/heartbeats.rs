//! The heartbeats during a wake, and their flags.

use super::*;

#[test]
fn tb3_a_bad_budget_or_heartbeat_is_exit_64_as_a_flag_4_from_the_env_and_logged() {
    let rig = Rig::new();
    for (flag, bad_value, text) in [
        (
            "--max-wake",
            "soon",
            "--max-wake needs a duration (45m, 90s, 2h) or off, got 'soon'",
        ),
        ("--max-wake", "1m30s", "--max-wake needs a duration"),
        (
            "--heartbeat-every",
            "x",
            "--heartbeat-every needs whole seconds, got 'x'",
        ),
        (
            "--heartbeat-every",
            "-1",
            "--heartbeat-every needs whole seconds",
        ),
        (
            "--heartbeat-every",
            "+5",
            "--heartbeat-every needs whole seconds",
        ),
    ] {
        let e = rig.settings(&[flag, bad_value], &[]).unwrap_err();
        assert!(e.text.contains(text), "{flag} {bad_value}: {}", e.text);
        assert_eq!(e.code, 64, "a bad value typed as a flag is a usage error");
        assert!(e.paths.is_some(), "the line goes to the log");
    }
    // the same bad value from the environment is a config error: a person fixes the setup (the
    // rig passes --retry-delays as a flag, so its variable is tested in the component suite)
    for (var, bad_value, text) in [
        (
            "CREWBOARD_HEARTBEAT_EVERY",
            "abc",
            "--heartbeat-every needs whole seconds, got 'abc'",
        ),
        ("CREWBOARD_MAX_WAKE", "soon", "--max-wake needs a duration"),
        (
            "CREWBOARD_WORK_BACKOFF",
            "soon",
            "--work-backoff needs whole seconds or off",
        ),
    ] {
        let e = rig.settings(&[], &[(var, bad_value)]).unwrap_err();
        assert!(e.text.contains(text), "{var}={bad_value}: {}", e.text);
        assert_eq!(e.code, 4, "{var}");
        assert!(e.paths.is_some(), "the line goes to the log");
    }
    let s = rig.settings(&["--max-wake", "off"], &[]).unwrap();
    assert_eq!((s.max_wake.secs, s.max_wake.text.as_str()), (0, "off"));
    let s = rig.settings(&["--max-wake", "2h"], &[]).unwrap();
    assert_eq!(s.max_wake.secs, 7200);
    // the default is 45m and 300 s; the environment comes before it and the flag before that
    let s = rig.settings(&[], &[]).unwrap();
    assert_eq!((s.max_wake.secs, s.heartbeat_every), (2700, 300));
    let s = rig
        .settings(
            &["--heartbeat-every", "7"],
            &[
                ("CREWBOARD_MAX_WAKE", "90s"),
                ("CREWBOARD_HEARTBEAT_EVERY", "9"),
            ],
        )
        .unwrap();
    assert_eq!((s.max_wake.secs, s.heartbeat_every), (90, 7));
    let s = rig
        .settings(&[], &[("CREWBOARD_HEARTBEAT_EVERY", "9")])
        .unwrap();
    assert_eq!(s.heartbeat_every, 9);
}

/// Counts the heartbeats of the thread of a wake; the other calls do not happen.
struct Beats(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Steps for Beats {
    fn heartbeat(&mut self) -> StepRc {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        StepRc::ok()
    }
    fn poll(&mut self, _: bool, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn mine_quiet(&mut self) -> StepRc {
        unreachable!()
    }
    fn mine_text(&mut self, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn say(&mut self, _: &str, _: &str, _: bool) -> StepRc {
        unreachable!()
    }
    fn tick(&mut self, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn acted_list(&mut self, _: &Path, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn pr_head(&mut self, _: &str, _: u64) -> Result<Option<PrHead>, StepRc> {
        unreachable!()
    }
    fn open_prs(&mut self, _: &str) -> Result<Vec<OpenPrRef>, StepRc> {
        unreachable!()
    }
}

#[test]
fn p12_a_long_wake_sends_heartbeats_from_its_own_thread_and_none_after_it() {
    let beats = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = || beats.load(std::sync::atomic::Ordering::SeqCst);
    let run = |every: &str, cmd: &str| {
        let rig = Rig::new();
        let s = rig
            .settings(&["--wake-cmd", cmd, "--heartbeat-every", every], &[])
            .unwrap();
        std::fs::create_dir_all(&s.paths.dir).unwrap();
        let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
        let mut st = Scripted::new(&["0||"]);
        let b = beats.clone();
        let make = move || -> Box<dyn Steps> { Box::new(Beats(b.clone())) };
        let rc = Flow {
            s: &s,
            steps: &mut st,
            clock: &rig.clock,
            pids: &SystemProcs,
            log: &log,
            host: "testhost",
            sig: Cell::new(None),
            signals: &|| None,
            heartbeat_steps: Some(&make),
        }
        .execute();
        assert_eq!(rc, 0);
    };
    run("1", "sleep 2.5");
    let during = count();
    assert!(
        (1..=3).contains(&during),
        "heartbeats during a 2.5 s wake: {during}"
    );
    // a real wait: "no beat after the wake" has no event to wait for; a stray thread would beat
    // within this 1.5 s (interval 1 s)
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(count(), during, "none after the wake");
    // a fast wake sends none; `--heartbeat-every 0` sends none at all
    run("5", "exit 0");
    run("0", "sleep 1.5");
    assert_eq!(count(), during);
}
