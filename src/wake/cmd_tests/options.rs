//! The options of a run, and `--print-cmd`.

use super::*;

#[test]
fn p6_the_config_dir_reaches_the_wake_command() {
    let rig = Rig::new();
    let cmd = format!(
        "printf '%s' \"$CREWBOARD_CONFIG_DIR\" > '{}'",
        rig.p("cfgseen").display()
    );
    let mut st = Scripted::new(&["0||"]);
    let a = parse(&[
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--wake-cmd",
        &cmd,
        "--retry-delays",
        "",
    ]);
    let g = GlobalFlags {
        config_dir: Some("/x/agentcfg".into()),
        ..GlobalFlags::default()
    };
    let Plan::Run(s) = resolve(&a, &g, &|_| None, None).unwrap() else {
        panic!()
    };
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(
        std::fs::read_to_string(rig.p("cfgseen")).unwrap(),
        "/x/agentcfg"
    );
}

#[test]
fn p7_owned_work_without_events_gives_one_work_wake() {
    let rig = Rig::new();
    let cmd = format!(
        "echo wake >> '{}'; printf '%s' \"$CREWBOARD_WAKE_REASON\" > '{}'",
        rig.p("wakes").display(),
        rig.p("reason").display()
    );
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", &cmd]), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(std::fs::read_to_string(rig.p("reason")).unwrap(), "work");
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
    assert!(rig.log().contains("owns an In Progress or Ready ticket"));
    // the other direction of the reason: an event wake says events
    let mut st = Scripted::new(&["0||"]);
    rig.run(&mut st, &["--wake-cmd", &cmd]);
    assert_eq!(std::fs::read_to_string(rig.p("reason")).unwrap(), "events");
}

#[test]
fn p8_no_events_and_no_owned_ticket_is_a_silent_quiet_poll() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(1, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!((rig.wakes(), st.count("mine --quiet")), (0, 1));
    assert_eq!((rig.log(), rig.stderr()), (String::new(), String::new()));
}

#[test]
fn p9_p10_the_back_off_decides_whether_mine_is_even_asked() {
    let rig = Rig::new();
    rig.write_state("last-wake", &format!("{}\n", NOW - 60));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(
        (rig.wakes(), st.count("mine")),
        (0, 0),
        "inside the back-off mine is not asked"
    );
    // 1000 s ago is over the 900 s back-off: a work wake, and the back-off restarts
    rig.write_state("last-wake", &format!("{}\n", NOW - 1000));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
}

#[test]
fn p10b_the_back_off_is_configurable_off_never_wakes_and_junk_is_refused() {
    let rig = Rig::new();
    rig.write_state("last-wake", &format!("{}\n", NOW - 10));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &["--work-backoff", "5"]);
    assert_eq!(rig.wakes(), 1);
    std::fs::remove_file(rig.state().join("last-wake")).unwrap();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert_eq!(
        (rig.wakes(), st.count("mine")),
        (1, 0),
        "off never wakes for work"
    );
    let e = rig.settings(&["--work-backoff", "soon"], &[]).unwrap_err();
    assert_eq!(
        e.text,
        "--work-backoff needs whole seconds or off, got 'soon'"
    );
    assert!(e.paths.is_some(), "this line goes to the log too");
    // the environment gives the same default as the flag
    let s = rig
        .settings(&[], &[("CREWBOARD_WORK_BACKOFF", "off")])
        .unwrap();
    assert_eq!(s.work_backoff, WorkBackoff::Off);
}

#[test]
fn p11_an_event_wake_starts_the_back_off_too() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &[]);
    rig.run(&mut st, &[]);
    assert_eq!(
        rig.wakes(),
        1,
        "a quiet tick right after a wake does not wake for work"
    );
    assert_eq!(st.count("mine"), 0);
}

#[test]
fn s2_print_state_dir_follows_the_flags() {
    let env = |k: &str| (k == "XDG_STATE_HOME").then(|| "/st".to_string());
    let dir =
        |argv: &[&str]| match resolve(&parse(argv), &GlobalFlags::default(), &env, None).unwrap() {
            Plan::PrintStateDir(d) => d,
            Plan::Run(_) | Plan::PrintCmd(_) => panic!("run"),
        };
    let p = PathBuf::from;
    assert_eq!(
        dir(&[
            "--print-state-dir",
            "--initiative",
            "demo",
            "--agent",
            "dev@host-a"
        ]),
        p("/st/crewboard/agents/demo-dev_host-a")
    );
    assert_eq!(
        dir(&[
            "--print-state-dir",
            "--initiative",
            "demo",
            "--lock-dir",
            "/y/wake.lock"
        ]),
        p("/y")
    );
    assert_eq!(
        dir(&["--print-state-dir", "--initiative", "demo"]),
        p("/st/crewboard/demo")
    );
    // it needs no wake command
    assert_eq!(dir(&["--print-state-dir", "--state-dir", "/s"]), p("/s"));
    // no state root at all
    let none = |_: &str| None;
    assert!(
        resolve(
            &parse(&["--print-state-dir"]),
            &GlobalFlags::default(),
            &none,
            None
        )
        .is_err()
    );
}

#[test]
fn print_cmd_is_a_plan_not_a_run() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let a = parse(&[
        "--runtime",
        "hermes",
        "--work-dir",
        wd.to_str().unwrap(),
        "--agent",
        "dev@host-a",
        "--initiative",
        "demo",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    match resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap() {
        Plan::PrintCmd(t) => {
            assert!(
                t.starts_with(&format!(
                    "cd {} && hermes chat -Q -s crewboard -q 'Use the crewboard skill",
                    wd.display()
                )),
                "{t}"
            );
        }
        _ => panic!("not a print"),
    }
    // the tick line of a headless coordinator
    let a = parse(&[
        "--tick",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    match resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap() {
        Plan::PrintCmd(t) => assert!(
            t.starts_with("crewboard tick (no model; the lines go to "),
            "{t}"
        ),
        _ => panic!("not a print"),
    }
    // --print-cmd without a command to print is a wrong flag
    let a = parse(&[
        "--wake-cmd",
        "true",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    assert!(
        resolve(&a, &GlobalFlags::default(), &|_| None, None)
            .unwrap_err()
            .text
            .contains("--print-cmd needs")
    );
}
