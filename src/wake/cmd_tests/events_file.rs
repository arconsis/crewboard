//! The events file of a wake.

use super::*;

#[test]
fn ev1_the_events_file_has_the_poll_and_the_mine_text_and_the_wake_gets_its_path() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12 to dev@host-a|"]).with_mine(&[(
        0,
        "demo/app#12                        Ready            P1  order 1\n",
    )]);
    let cmd = format!(
        "echo \"$CREWBOARD_EVENTS_FILE\" > '{}'; echo \"$CREWBOARD_RESULTS_DIR\" >> '{}'",
        rig.p("env").display(),
        rig.p("env").display()
    );
    let evf = ev(&rig);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", &cmd]),
        0
    );
    let t = read(&rig.p("ev.txt"));
    assert!(t.starts_with("== Events (crewboard poll, 20"), "{t}");
    assert!(
        t.contains(
            "\nEVENT 1: assign #12 to dev@host-a\n== Your tickets (crewboard mine):\ndemo/app#12 "
        ),
        "{t}"
    );
    assert_eq!(
        st.calls,
        vec!["poll --quiet", "poll", "mine"],
        "the cursor moves with the second poll"
    );
    assert_eq!(
        read(&rig.p("env")),
        format!("{evf}\n{}/results\n", rig.state().display())
    );
    assert!(rig.has_no_temp_of("ev.txt"), "the temp file is renamed");
    // the other direction (EV3): without --events-file only `poll --quiet` runs
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(st.calls, vec!["poll --quiet"]);
    assert!(st.acted_calls.is_empty());
}

#[test]
fn ev2_a_failed_wake_keeps_its_events_for_the_next_wake() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 7: question on #4|", "0||", "1||"]);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", "exit 9"]),
        70
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(kept.contains("EVENT 7: question on #4"), "{kept}");
    let cmd = copy_events(&rig, "seen");
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", &cmd]),
        0
    );
    let seen = read(&rig.p("seen"));
    assert!(seen.starts_with(crate::wake::events::EARLIER), "{seen}");
    assert!(
        seen.contains("EVENT 7: question on #4") && seen.contains("(no new events)"),
        "{seen}"
    );
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the next wake got them and took them"
    );
}

#[test]
fn a_signal_during_the_wake_keeps_the_events_and_exits_143() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let started = rig.p("started");
    let cmd = format!("touch '{}'; sleep 30", started.display());
    let mut st = Scripted::new(&["0||", "0|EVENT 5: question on #2|", "0||"]);
    let s = rig
        .settings(&["--events-file", &evf, "--wake-cmd", &cmd], &[])
        .unwrap();
    let rc = rig.run_signals(&mut st, &s, &|| started.exists().then_some(15));
    assert_eq!(rc, 143);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(kept.contains("EVENT 5: question on #2"), "{kept}");
    assert!(!rig.lock().exists());
    assert!(
        st.acted_calls.iter().all(|c| !c.contains("--since")),
        "no check after a stopped wake: {:?}",
        st.acted_calls
    );
}

#[test]
fn an_unreadable_kept_file_stops_the_run_before_the_poll() {
    let rig = Rig::new();
    // a directory: reading it is an error that is not "not found"
    std::fs::create_dir_all(rig.p("ev.txt.unhandled")).unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert_eq!(st.calls, vec!["poll --quiet"], "the cursor did not move");
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains("cannot read the kept events "),
        "{}",
        rig.log()
    );
    assert!(rig.p("ev.txt.unhandled").is_dir());
}

#[test]
fn kept_events_that_are_not_utf8_are_read_lossily_not_dropped() {
    let rig = Rig::new();
    std::fs::write(rig.p("ev.txt.unhandled"), b"== kept\nEVENT \xff7\n").unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "1||"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 0);
    let t = read(&rig.p("ev.txt"));
    assert!(t.contains("EVENT \u{fffd}7"), "{t}");
}

#[test]
fn a1_a_dev_agent_wake_runs_the_runtime_in_the_work_dir_with_its_environment() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let out = rig.p("rt.out");
    let bin = rig.p("fake-claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\n{{ pwd -P; echo \"wake=$CREWBOARD_WAKE events=$CREWBOARD_EVENTS_FILE\"; for a in \"$@\"; do echo \"arg=$a\"; done; }} > '{}'\n",
            out.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (wds, bins) = (wd.to_str().unwrap(), bin.to_str().unwrap());
    let extra = [
        "--runtime",
        "claude",
        "--runtime-bin",
        bins,
        "--work-dir",
        wds,
        "--agent",
        "dev@host-a",
        "--initiative",
        "demo",
        "--no-events-hook",
    ];
    let mut st = Scripted::new(&["0||", "0|EVENT 1|", "0||"]);
    assert_eq!(rig.run(&mut st, &extra), 0);
    let t = read(&out);
    assert!(
        t.starts_with(&format!("{}\n", wd.canonicalize().unwrap().display())),
        "{t}"
    );
    let events = rig.state().join("events.txt");
    assert!(
        t.contains(&format!("wake=1 events={}", events.display())),
        "{t}"
    );
    assert!(
        t.contains("arg=-p\narg=--permission-mode\narg=dontAsk\n"),
        "{t}"
    );
    assert!(t.contains("arg=--\narg=Use the crewboard skill"), "{t}");
    assert!(
        events.is_file(),
        "the default events file is in the state dir"
    );
    assert!(
        rig.log().contains("events pending; waking: cd "),
        "{}",
        rig.log()
    );
    // a runtime that cannot start is a failed wake with its reason in the log
    std::fs::remove_file(&bin).unwrap();
    let s = rig.settings(&extra, &[]);
    assert!(s.unwrap_err().text.contains("is gone (moved or removed)"));
}

#[test]
fn the_kept_events_stay_when_the_new_file_cannot_be_written() {
    let rig = Rig::new();
    // a directory where the events file should go: the rename fails
    std::fs::create_dir_all(rig.p("ev.txt")).unwrap();
    std::fs::write(rig.p("ev.txt.unhandled"), "== kept\nEVENT 7\n").unwrap();
    std::fs::create_dir_all(rig.state().join("results")).unwrap();
    std::fs::write(
        rig.state().join("results/7.result"),
        "rc=0 end=2026-10-21T10:00:00Z log=l\nticket=7\n",
    )
    .unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert_eq!(rig.wakes(), 0, "no wake without its events");
    assert!(
        rig.log().contains("cannot write the events file "),
        "{}",
        rig.log()
    );
    assert_eq!(
        read(&rig.p("ev.txt.unhandled")),
        "== kept\nEVENT 7\n",
        "nothing is lost"
    );
    assert!(
        !rig.state().join("results/7.result.seen").exists(),
        "the result is not marked"
    );
    assert!(rig.has_no_temp_of("ev.txt"));
    assert!(!rig.lock().exists());
    // twin: a file that can be written removes `.unhandled` and marks the result
    std::fs::remove_dir_all(rig.p("ev.txt")).unwrap();
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 0);
    assert!(!rig.p("ev.txt.unhandled").exists());
    assert!(rig.state().join("results/7.result.seen").exists());
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("EVENT 7") && t.contains("detached job finished: ticket 7 rc=0 "),
        "{t}"
    );
}

#[test]
fn a_poll_that_fails_for_the_events_file_is_told_to_the_model_not_fatal() {
    let rig = Rig::new();
    let evf = ev(&rig);
    // poll --quiet says 0; the second poll fails with 2; mine fails with 2
    let mut st = Scripted::new(&["0||", "2||refused something"]).with_mine(&[(2, "")]);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", "true"]),
        0
    );
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains(
            "(crewboard poll failed with exit 2: run crewboard watch --timeout 1m yourself)"
        ),
        "{t}"
    );
    assert!(t.contains("(crewboard mine failed with exit 2)"), "{t}");
    assert!(
        rig.log()
            .contains("crewboard poll for the events file failed: exit 2: refused something")
    );
}

#[test]
fn exit_4_while_the_events_file_is_built_stops_the_wakes() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "4||key gone"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" poll: key gone")
    );
    assert!(!rig.p("ev.txt").exists() && rig.has_no_temp_of("ev.txt"));
    assert_eq!(rig.wakes(), 0);
}
