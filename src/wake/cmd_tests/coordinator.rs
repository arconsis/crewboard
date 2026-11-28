//! `--tick --runtime`: the model wake of the unattended coordinator. The tick is scripted; the
//! runtime is a shell script that counts its starts.

use super::*;
use crate::unattended::{CoordMode, Need};
use crate::wake::policy::CoordWakes;

/// The day of `NOW` (UTC), and the next day.
const TODAY: &str = "2023-11-14";
const NEXT_DAY: &str = "2023-11-15";

/// The tick lines of a list with one ticket.
const LIST6: &str = "0|needs judgment: demo/app#6, prose Gate\\nunattended coordinator: mode verify; verdicts 0/10 today|";

/// A runtime that records the reason of each start in `wakes` and its words in `args`, then
/// exits with `rc`.
fn runtime(rig: &Rig, rc: u8) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = rig.p(&format!("rt{rc}"));
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\necho \"$CREWBOARD_WAKE_REASON\" >> '{}'\nprintf '%s\\n' \"$@\" > '{}'\npwd > '{}'\nexit {rc}\n",
            rig.p("wakes").display(),
            rig.p("args").display(),
            rig.p("cwd").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.to_string_lossy().into_owned()
}

/// The flags of a coordinator unit with the runtime script `rt`.
fn unit(rig: &Rig, rt: &str) -> Vec<String> {
    std::fs::create_dir_all(rig.p("wd")).unwrap();
    [
        "--tick",
        "--initiative",
        "demo",
        "--runtime",
        "claude",
        "--runtime-bin",
        rt,
        "--work-dir",
        rig.p("wd").to_str().unwrap(),
    ]
    .map(str::to_string)
    .to_vec()
}

fn run(rig: &Rig, st: &mut Scripted, flags: &[String]) -> u8 {
    let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
    rig.run(st, &refs)
}

fn listed(tickets: &[u64], used: u32) -> Option<Listed> {
    Some(Listed {
        mode: CoordMode::Verify,
        needs: tickets
            .iter()
            .map(|n| (format!("demo/app#{n}"), Need::ProseGate))
            .collect(),
        used,
        budget: 10,
    })
}

/// Steps whose ticks all give `tick` and the values `judgment`.
fn steps(tick: &str, n: usize, judgment: Option<Listed>) -> Scripted {
    let mut st = Scripted::new(&rep(n, tick));
    st.judgment = judgment;
    st
}

fn wakes_file(rig: &Rig) -> Option<CoordWakes> {
    state::read_coord_wakes(&rig.state().join("coordinator-wakes"))
}

fn plant_wakes(rig: &Rig, day: &str, count: u32, last: i64, hash: &str) {
    rig.write_state(
        "coordinator-wakes",
        &format!("{day} {count} {last} {hash} verify claude\n"),
    );
}

fn state_files(rig: &Rig) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(rig.state())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn uc1_with_the_mode_off_the_unit_does_what_a_tick_unit_does_and_starts_no_process() {
    let plain = Rig::new();
    let model = Rig::new();
    let rt = runtime(&model, 0);
    let flags = unit(&model, &rt);
    let plan = [TICKOUT, TICKOUT, "0|tick: no ticket in Review|", TICKOUT];
    // the mode is off: the tick gives no values
    let mut a = Scripted::new(&plan);
    let mut b = Scripted::new(&plan);
    for _ in 0..plan.len() {
        assert_eq!(plain.run(&mut a, &["--tick"]), 0);
        assert_eq!(run(&model, &mut b, &flags), 0);
    }
    assert_eq!(a.calls, b.calls);
    assert_eq!(b.calls, rep(plan.len(), "tick"));
    assert_eq!(plain.log(), model.log());
    assert!(!plain.log().is_empty());
    assert_eq!(
        (plain.stderr(), model.stderr()),
        (String::new(), String::new())
    );
    assert_eq!(state_files(&plain), state_files(&model));
    assert_eq!(model.wakes(), 0, "no process started");
    assert!(!model.p("args").exists());
    // the tick of both units gets no checkout
    assert!(a.tick_dirs.iter().chain(&b.tick_dirs).all(Option::is_none));
}

#[test]
fn uc2_an_empty_list_a_used_budget_and_a_tick_that_fails_start_no_model() {
    let rig = Rig::new();
    let rt = runtime(&rig, 0);
    let flags = unit(&rig, &rt);
    // the mode is on, and no ticket needs a judgment
    let mut st = steps(
        "0|unattended coordinator: mode verify; verdicts 0/10 today|",
        1,
        listed(&[], 0),
    );
    assert_eq!(run(&rig, &mut st, &flags), 0);
    // a ticket needs a judgment, and the verdicts of the day are used
    let mut st = steps(LIST6, 1, listed(&[6], 10));
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert!(
        rig.log().contains(
            "1 ticket(s) need a judgment; no model starts: the verdicts of today are used (10/10)"
        ),
        "{}",
        rig.log()
    );
    // a tick that fails gives its exit code, whatever values an earlier tick gave
    let mut st = steps("2||refused [rule=tick-headless-only] x", 1, listed(&[6], 0));
    assert_eq!(run(&rig, &mut st, &flags), 2);
    assert_eq!(rig.wakes(), 0);
    assert!(wakes_file(&rig).is_none() && !rig.state().join("judgment.txt").exists());
    assert_eq!(
        rig.stderr().lines().count(),
        1,
        "only the failed tick: {}",
        rig.stderr()
    );
}

#[test]
fn uc3_a_list_starts_the_model_once_then_after_a_change_or_after_the_back_off() {
    let rig = Rig::new();
    let rt = runtime(&rig, 0);
    let flags = unit(&rig, &rt);
    let mut st = steps(LIST6, 8, listed(&[6], 3));
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(read(&rig.p("wakes")), "judgment\n");
    // the model starts in the work dir, with the coordinator prompt as its last word
    assert_eq!(
        std::fs::canonicalize(read(&rig.p("cwd")).trim()).unwrap(),
        std::fs::canonicalize(rig.p("wd")).unwrap()
    );
    let args = read(&rig.p("args"));
    assert!(
        args.starts_with("-p\n--permission-mode\ndontAsk\n--allowedTools\n"),
        "{args}"
    );
    assert!(
        args.contains("You are the unattended coordinator of demo"),
        "{args}"
    );
    assert!(!args.contains("--settings"), "no events hook: {args}");
    // the two files are written before the start
    let hash = crate::wake::policy::list_hash(&["demo/app#6, prose Gate".to_string()]);
    assert_eq!(
        rig.state_text("coordinator-wakes").unwrap(),
        format!("{TODAY} 1 {NOW} {hash} verify claude\n")
    );
    assert_eq!(
        rig.state_text("judgment.txt").unwrap(),
        "judgment list, written by crewboard wake at 2023-11-14T22:13:20Z\nmode: verify\nverdicts today: 3/10\nneeds judgment: demo/app#6, prose Gate\n"
    );
    let log = rig.log();
    assert!(
        log.contains("1 ticket(s) need a judgment; waking the model (wake 1 of 6 today): cd "),
        "{log}"
    );
    assert!(
        log.ends_with("poll-and-wake: model wake finished OK\n"),
        "{log}"
    );
    // the wake asked the board for the tick only: no poll, no heartbeat, no message
    assert_eq!(st.calls, ["tick"]);
    assert!(
        st.tick_dirs.iter().all(Option::is_none),
        "the work dir is for the model"
    );
    assert!(!rig.state().join("last-wake").exists() && !rig.lock().exists());

    // the same list: no second model, and no line for it (the tick is quiet)
    let lines = rig.log().lines().count();
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.log().lines().count(), lines);
    rig.clock.advance(policy_backoff() - 1);
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(!rig.log().contains("no model starts"), "{}", rig.log());

    // the back-off is over: one more model; the new UTC day starts its count at 1
    rig.clock.advance(1);
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 2);
    let w = wakes_file(&rig).unwrap();
    assert_eq!(
        (w.day.as_str(), w.count, w.last),
        (NEXT_DAY, 1, NOW + policy_backoff())
    );

    // a list that changed starts a model at once
    st.judgment = listed(&[6, 7], 3);
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 3);
    assert_eq!(wakes_file(&rig).unwrap().count, 2);
    assert!(rig.state_text("judgment.txt").unwrap().ends_with(
        "needs judgment: demo/app#6, prose Gate\nneeds judgment: demo/app#7, prose Gate\n"
    ));
    // a list that got shorter is a change too
    st.judgment = listed(&[7], 3);
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 4);
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 4);
}

fn policy_backoff() -> i64 {
    crate::wake::policy::COORD_BACKOFF_SECS
}

#[test]
fn uc4_the_day_has_six_model_wakes_and_a_lost_count_file_allows_six_more() {
    let rig = Rig::new();
    let rt = runtime(&rig, 0);
    let flags = unit(&rig, &rt);
    let mut st = steps(LIST6, 6, listed(&[6], 0));
    plant_wakes(&rig, TODAY, 6, NOW - 9000, "other");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains(
            "1 ticket(s) need a judgment; no model starts: 6 model wakes today (UTC), the limit is 6"
        ),
        "{}",
        rig.log()
    );
    // the fifth wake of the day leaves room for one
    plant_wakes(&rig, TODAY, 5, NOW - 9000, "other");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(wakes_file(&rig).unwrap().count, 6);
    // the count of an earlier day does not count
    plant_wakes(&rig, "2023-11-13", 6, NOW - 9000, "other");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 2);
    assert_eq!(wakes_file(&rig).unwrap().count, 1);
    // a lost file, and a broken one: the count starts at 0 again, so at most six more
    std::fs::remove_file(rig.state().join("coordinator-wakes")).unwrap();
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!((rig.wakes(), wakes_file(&rig).unwrap().count), (3, 1));
    rig.write_state("coordinator-wakes", "not a count\n");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!((rig.wakes(), wakes_file(&rig).unwrap().count), (4, 1));
}

#[test]
fn uc5_a_stored_day_after_today_starts_no_model() {
    let rig = Rig::new();
    let rt = runtime(&rig, 0);
    let flags = unit(&rig, &rt);
    let mut st = steps(LIST6, 2, listed(&[6], 0));
    plant_wakes(&rig, NEXT_DAY, 1, NOW - 9000, "other");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains(
            "no model starts: the count of model wakes is of 2023-11-15, a later day than today (UTC)"
        ),
        "{}",
        rig.log()
    );
    // the file is as it was: the day of the state is reached, or the owner removes the file
    assert_eq!(wakes_file(&rig).unwrap().day, NEXT_DAY);
}

#[test]
fn uc6_no_model_starts_when_a_file_cannot_be_written_before_the_start() {
    for blocked in ["coordinator-wakes", "judgment.txt"] {
        let rig = Rig::new();
        let rt = runtime(&rig, 0);
        let flags = unit(&rig, &rt);
        // a dir that is not empty is in the place of the file: the rename fails
        std::fs::create_dir_all(rig.state().join(blocked).join("x")).unwrap();
        let mut st = steps(LIST6, 1, listed(&[6], 0));
        assert_eq!(run(&rig, &mut st, &flags), 0, "{blocked}");
        assert_eq!(rig.wakes(), 0, "{blocked}");
        let err = rig.stderr();
        assert!(
            err.contains("cannot write ")
                && err.contains(blocked)
                && err.contains("no model starts"),
            "{blocked}: {err}"
        );
        if blocked == "judgment.txt" {
            assert!(
                !rig.state().join("coordinator-wakes").exists(),
                "no wake is counted without the judgment file"
            );
        }
    }
}

#[test]
fn uc7_failed_model_wakes_pause_the_model_and_the_tick_goes_on() {
    let rig = Rig::new();
    let bad = runtime(&rig, 9);
    let flags = unit(&rig, &bad);
    let fails = |rig: &Rig| rig.state_text("wake-fail").map(|t| NetBackoff::parse(&t));
    let mut st = steps(LIST6, 12, listed(&[6], 0));

    // 1 and 2: the model fails; from the second failure the model wakes pause
    assert_eq!(run(&rig, &mut st, &flags), 70);
    assert_eq!(fails(&rig).map(|b| b.fails), Some(1));
    assert!(
        rig.log().ends_with("wake command failed: exit 9\n"),
        "{}",
        rig.log()
    );
    rig.clock.advance(policy_backoff());
    assert_eq!(run(&rig, &mut st, &flags), 70);
    let paused_until = rig.clock.now() + 300;
    assert_eq!(
        fails(&rig),
        Some(NetBackoff {
            fails: 2,
            until: paused_until
        })
    );
    assert_eq!(rig.wakes(), 2);

    // in the pause: the tick runs, and a list that changed starts no model
    st.judgment = listed(&[6, 7], 0);
    st.plan.clear();
    for _ in 0..4 {
        st.push(
            "0|needs judgment: demo/app#6, prose Gate\\nneeds judgment: demo/app#7, prose Gate|",
        );
    }
    let ticks = st.count("tick");
    assert_eq!(run(&rig, &mut st, &flags), 0);
    assert_eq!((st.count("tick"), rig.wakes()), (ticks + 1, 2));
    assert!(
        rig.log().contains("2 ticket(s) need a judgment; no model starts: the model wakes pause after wakes that failed in a row"),
        "{}",
        rig.log()
    );

    // 3: after the pause the model starts again; one line tells the owner on standard error
    rig.clock.advance(300);
    assert_eq!(run(&rig, &mut st, &flags), 70);
    assert_eq!((rig.wakes(), fails(&rig).map(|b| b.fails)), (3, Some(3)));
    let err = rig.stderr();
    assert!(
        err.contains(
            "the model wake of the unattended coordinator failed 3 times in a row (last exit 9)"
        ),
        "{err}"
    );
    // nobody is told on the board: the headless scope refuses a `say`, so none is tried
    assert_eq!((st.count("say "), st.count("mine")), (0, 0));

    // a model that works ends the count
    rig.clock.advance(policy_backoff());
    let good = runtime(&rig, 0);
    assert_eq!(run(&rig, &mut st, &unit(&rig, &good)), 0);
    assert_eq!(rig.state_text("wake-fail"), None);
}

#[test]
fn uc8_no_heartbeat_thread_runs_beside_the_model() {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new();
    let slow = rig.p("slow");
    std::fs::write(&slow, "#!/bin/sh\nsleep 2.5\n").unwrap();
    std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut flags = unit(&rig, slow.to_str().unwrap());
    flags.extend(["--heartbeat-every", "1"].map(str::to_string));
    let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
    let s = rig.settings(&refs, &[]).unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = steps(LIST6, 1, listed(&[6], 0));
    let beats = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let b = beats.clone();
    let make = move || -> Box<dyn Steps> { Box::new(heartbeats::Beats(b.clone())) };
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
    assert!(
        rig.log().contains("model wake finished OK"),
        "{}",
        rig.log()
    );
    // the same wait gives 1 to 3 heartbeats in a wake of a dev agent (`p12`)
    assert_eq!(beats.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(st.calls, ["tick"]);
}

#[test]
fn uc9_the_settings_of_a_coordinator_unit_and_print_cmd_shows_both_parts() {
    let rig = Rig::new();
    let rt = runtime(&rig, 0);
    let flags = unit(&rig, &rt);
    let refs: Vec<&str> = flags.iter().map(String::as_str).collect();
    let s = rig.settings(&refs, &[]).unwrap();
    assert!(s.tick && s.direct.is_some());
    assert_eq!(s.events_file, None, "no events file");
    assert_eq!(s.direct.as_ref().map(|d| d.runtime), Some("claude"));
    // `--print-cmd`: the tick, then the model command
    let state = rig.state();
    let mut argv = vec!["--state-dir", state.to_str().unwrap(), "--print-cmd"];
    argv.extend(&refs);
    let Plan::PrintCmd(text) =
        resolve(&parse(&argv), &GlobalFlags::default(), &|_| None, None).unwrap()
    else {
        panic!("print")
    };
    let (first, second) = text.split_once('\n').unwrap();
    assert_eq!(
        first,
        format!(
            "crewboard tick (the lines go to {})",
            rig.log_path().display()
        )
    );
    assert!(
        second.starts_with(&format!(
            "then, only when a ticket needs a judgment and `unattended_coordinator` of AUTHORITY.md is not `off`: cd {} && {rt} -p ",
            rig.p("wd").display()
        )),
        "{second}"
    );
    // a plain tick unit prints its one line
    let argv = [
        "--state-dir",
        state.to_str().unwrap(),
        "--print-cmd",
        "--tick",
    ];
    let Plan::PrintCmd(text) =
        resolve(&parse(&argv), &GlobalFlags::default(), &|_| None, None).unwrap()
    else {
        panic!("print")
    };
    assert!(
        text.starts_with("crewboard tick (no model; the lines go to "),
        "{text}"
    );
    // options that cannot work with the model wake of a coordinator
    for (extra, text) in [
        (
            vec!["--events-every", "60"],
            "--events-every is for a dev agent",
        ),
        (
            vec!["--no-events-hook"],
            "--no-events-hook is for a dev agent",
        ),
        (
            vec!["--wake-cmd", "true"],
            "--runtime and --wake-cmd exclude each other",
        ),
    ] {
        let mut a = refs.clone();
        a.extend(extra);
        let e = rig.settings(&a, &[]).unwrap_err();
        assert!(e.text.contains(text), "{}", e.text);
    }
    // a tick unit with a checkout runs the checks there; with a runtime the checkout is the
    // start dir of the model, and the tick gets none
    let mut st = Scripted::new(&[TICKOUT]);
    let wd = rig.p("wd");
    assert_eq!(
        rig.run(&mut st, &["--tick", "--work-dir", wd.to_str().unwrap()]),
        0
    );
    assert_eq!(st.tick_dirs, [Some(wd)]);
}
