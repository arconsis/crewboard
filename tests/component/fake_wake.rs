//! `crewboard wake` against a fake GitHub: the calls of the gate run in process (`poll --quiet`,
//! `mine --quiet`, `say`, `tick`). The flow itself is unit-tested in `src/wake/cmd.rs`.

#![cfg(unix)]

use crate::support::board::{Agent, FakeBoard};
use crate::support::cli::{crew, run_ok};
use crate::support::headless::{Hub, Scenario};
use std::path::Path;

fn ticket(fake: &FakeBoard) -> u64 {
    fake.ticket_issue(
        "IT wake",
        "Goal: test.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    )
}

/// One `wake` run of the agent; the wake command appends its reason to `wakes`.
fn wake(fake: &FakeBoard, who: &Agent, state: &Path, extra: &[&str]) -> crate::support::board::Out {
    let cmd = format!(
        "echo \"$CREWBOARD_WAKE_REASON\" >> '{}'",
        state.join("wakes").display()
    );
    let mut args = vec![
        "wake",
        "--wake-cmd",
        &cmd,
        "--state-dir",
        state.to_str().unwrap(),
        "--retry-delays",
        "",
    ];
    args.extend_from_slice(extra);
    fake.run(&who.dir, &args)
}

fn reasons(state: &Path) -> Vec<String> {
    std::fs::read_to_string(state.join("wakes"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

#[test]
fn a_quiet_board_starts_no_wake_and_an_assignment_starts_one() {
    let (fake, c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    // the cursor starts at now: nothing is pending
    let _ = fake.poll_json(&b);
    let o = wake(&fake, &b, s, &["--work-backoff", "off"]);
    assert_eq!(
        (o.code, o.stdout.as_str(), o.stderr.as_str()),
        (0, "", ""),
        "{}",
        o.all()
    );
    assert!(reasons(s).is_empty());
    // an assignment is an event
    let t = ticket(&fake);
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    let o = wake(&fake, &b, s, &["--work-backoff", "off"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(o.stdout, "", "{}", o.all());
    assert_eq!(reasons(s), vec!["events"]);
    assert!(
        o.stderr.contains("poll-and-wake: events pending; waking: "),
        "{}",
        o.all()
    );
    assert!(
        o.stderr.contains("poll-and-wake: wake finished OK"),
        "{}",
        o.all()
    );
    assert!(!s.join("wake.lock").exists());
}

#[test]
fn owned_work_wakes_once_per_back_off() {
    let (fake, c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let t = ticket(&fake).to_string();
    run_ok(&fake, &c, &["assign", &t, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &t]);
    // the events are read: the board is quiet, but the agent owns an In Progress ticket
    let _ = fake.poll_json(&b);
    let o = wake(&fake, &b, s, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(reasons(s), vec!["work"], "{}", o.all());
    assert!(
        o.stderr.contains("owns an In Progress or Ready ticket"),
        "{}",
        o.all()
    );
    // the back-off holds: the next run is quiet and `mine` is not asked
    let before = fake.requests().len();
    let o = wake(&fake, &b, s, &[]);
    assert_eq!((o.code, o.stderr.as_str()), (0, ""), "{}", o.all());
    assert_eq!(reasons(s).len(), 1);
    let after = fake.requests().len();
    assert!(after > before, "the gate still polls");
    // twin: with the back-off off the agent is never woken for work
    std::fs::remove_file(s.join("last-wake")).unwrap();
    let o = wake(&fake, &b, s, &["--work-backoff", "off"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(reasons(s).len(), 1);
}

#[test]
fn exit_4_from_the_process_stops_the_wakes() {
    let (fake, _c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let nowhere = b.dir.join("no-such-dir");
    let a = [
        "wake",
        "--wake-cmd",
        "true",
        "--state-dir",
        s.to_str().unwrap(),
        "--retry-delays",
        "",
    ];
    let o = fake.run(&nowhere, &a);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("STOPPED: crewboard poll exit 4: "),
        "{}",
        o.all()
    );
    assert!(s.join("stopped").is_file());
    // the next run is silent, even with a good config dir
    let o = fake.run(&b.dir, &a);
    assert_eq!((o.code, o.stderr.as_str()), (0, ""), "{}", o.all());
}

#[test]
fn a_crewboard_binary_that_is_not_this_one_is_asked_instead_of_the_board() {
    let (fake, c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let t = ticket(&fake).to_string();
    run_ok(&fake, &c, &["assign", &t, "--to", &b.name]);
    // a stub that says "no events" for everything
    let stub = s.join("stub");
    std::fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = wake(
        &fake,
        &b,
        s,
        &[
            "--work-backoff",
            "off",
            "--crewboard",
            stub.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(reasons(s).is_empty(), "the stub answered, not the board");
    // the running binary itself is called in process: it sees the assignment
    let bin = env!("CARGO_BIN_EXE_crewboard");
    let o = wake(&fake, &b, s, &["--work-backoff", "off", "--crewboard", bin]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(reasons(s), vec!["events"]);
}

/// Copy a config dir (keys and all) to a new place.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dst = to.join(e.file_name());
        if e.path().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            std::fs::copy(e.path(), &dst).unwrap();
        }
    }
}

/// A dev that owns an In Progress ticket, whose Cursor field is damaged: `poll` exits 4 while
/// `mine` and `say` still work.
fn dev_with_a_damaged_cursor() -> (FakeBoard, Agent, Agent, u64) {
    let (fake, c, b) = crew();
    let t = ticket(&fake);
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    run_ok(&fake, &b, &["take", &t.to_string()]);
    fake.set_item(&b.item_id, "Cursor", "garbage");
    (fake, c, b, t)
}

fn infos_from(fake: &FakeBoard, t: u64, who: &str) -> usize {
    fake.comments_of(t)
        .iter()
        .filter(|(_, _, body)| body.contains(who) && body.contains("The wake wrapper of"))
        .count()
}

#[test]
fn exit_4_of_poll_tells_the_coordinator_through_say_in_process_and_through_a_subprocess() {
    // in process
    let (fake, _c, b, t) = dev_with_a_damaged_cursor();
    let st = tempfile::tempdir().unwrap();
    let o = wake(&fake, &b, st.path(), &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("STOPPED: crewboard poll exit 4: "),
        "{}",
        o.all()
    );
    assert!(st.path().join("stopped").is_file());
    assert!(
        std::fs::read_to_string(st.path().join("wake.log"))
            .unwrap()
            .contains(
                &format!("told the coordinator on example-org/app#{t}")
                    .replace("example-org/app", crate::support::board::REPO)
            ),
        "{}",
        o.all()
    );
    assert_eq!(infos_from(&fake, t, "stopped: crewboard exit 4"), 1);
    // the same through another binary (a copy of this one): the same message
    let (fake, _c, b, t) = dev_with_a_damaged_cursor();
    let st = tempfile::tempdir().unwrap();
    let copy = st.path().join("crewboard-copy");
    std::fs::copy(env!("CARGO_BIN_EXE_crewboard"), &copy).unwrap();
    let o = wake(
        &fake,
        &b,
        st.path(),
        &["--crewboard", copy.to_str().unwrap()],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert_eq!(
        infos_from(&fake, t, "stopped: crewboard exit 4"),
        1,
        "{}",
        o.all()
    );
}

#[test]
fn the_say_after_exit_4_is_refused_in_a_headless_dir_in_process_and_through_a_subprocess() {
    for sub in [false, true] {
        let (fake, _c, b, t) = dev_with_a_damaged_cursor();
        let headless = tempfile::tempdir().unwrap();
        copy_dir(&b.dir, headless.path());
        let toml = std::fs::read_dir(headless.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "toml"))
            .unwrap();
        let text = std::fs::read_to_string(&toml).unwrap();
        std::fs::write(&toml, format!("headless = true\n{text}")).unwrap();
        let st = tempfile::tempdir().unwrap();
        let copy = st.path().join("crewboard-copy");
        std::fs::copy(env!("CARGO_BIN_EXE_crewboard"), &copy).unwrap();
        let cmd = format!("echo x >> '{}'", st.path().join("wakes").display());
        let mut args = vec![
            "wake",
            "--wake-cmd",
            &cmd,
            "--state-dir",
            st.path().to_str().unwrap(),
            "--retry-delays",
            "",
        ];
        if sub {
            args.extend(["--crewboard", copy.to_str().unwrap()]);
        }
        let o = fake.run(headless.path(), &args);
        assert_eq!(o.code, 4, "sub={sub}: {}", o.all());
        assert!(st.path().join("stopped").is_file());
        let log = std::fs::read_to_string(st.path().join("wake.log")).unwrap();
        assert!(!log.contains("told the coordinator"), "sub={sub}: {log}");
        assert_eq!(
            infos_from(&fake, t, "stopped: crewboard exit 4"),
            0,
            "sub={sub}: nothing is posted"
        );
    }
}

#[test]
fn a_bad_env_file_is_exit_4_each_run_without_a_stop_file_and_chmod_cures_it() {
    let (fake, _c, b) = crew();
    // an env file that others can read: the loader refuses it
    let env = b.dir.join("agent.env");
    std::fs::write(&env, "GH_TOKEN=ghp_faketoken\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o644)).unwrap();
    let st = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let o = wake(&fake, &b, st.path(), &[]);
        assert_eq!(o.code, 4, "{}", o.all());
        assert!(
            o.stderr.contains("crewboard poll exit 4: ") && o.stderr.contains("chmod 600"),
            "{}",
            o.all()
        );
        assert!(!o.stderr.contains("STOPPED"), "{}", o.all());
        assert!(!st.path().join("stopped").exists(), "{}", o.all());
    }
    // fixed with chmod only (the mtime does not change): the next run works
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600)).unwrap();
    let o = wake(&fake, &b, st.path(), &["--work-backoff", "off"]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn tick_runs_in_process_in_a_headless_dir_and_a_refused_tick_is_logged() {
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: true,
    });
    let st = tempfile::tempdir().unwrap();
    let a = [
        "wake",
        "--tick",
        "--state-dir",
        st.path().to_str().unwrap(),
        "--retry-delays",
        "",
    ];
    let o = h.headless(&a);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
    let log = std::fs::read_to_string(st.path().join("wake.log")).unwrap();
    assert!(log.contains("poll-and-wake: tick: "), "{log}");
    assert!(st.path().join("last-tick").is_file());
    assert!(!st.path().join("wake.lock").exists());
    // the same output again writes no new line
    let lines = log.lines().count();
    let o = h.headless(&a);
    assert_eq!(o.code, 0, "{}", o.all());
    // the interactive coordinator's dir is no headless dir: the tick is refused, exit 2
    let st2 = tempfile::tempdir().unwrap();
    let a2 = [
        "wake",
        "--tick",
        "--state-dir",
        st2.path().to_str().unwrap(),
        "--retry-delays",
        "",
    ];
    let o = h.interactive(&a2);
    assert_eq!(o.code, 2, "{}", o.all());
    let log2 = std::fs::read_to_string(st2.path().join("wake.log")).unwrap();
    assert!(
        log2.contains("crewboard tick failed: exit 2: refused [rule=tick-headless-only]"),
        "{log2}"
    );
    let _ = lines;
}

fn events_wake(
    fake: &FakeBoard,
    who: &Agent,
    state: &Path,
    events: &Path,
    cmd: &str,
) -> crate::support::board::Out {
    fake.run(
        &who.dir,
        &[
            "wake",
            "--wake-cmd",
            cmd,
            "--state-dir",
            state.to_str().unwrap(),
            "--events-file",
            events.to_str().unwrap(),
            "--retry-delays",
            "",
            "--work-backoff",
            "off",
        ],
    )
}

#[test]
fn the_events_file_the_receipts_and_the_one_redelivery_against_the_fake_board() {
    let (fake, c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let events = s.join("events.txt");
    let log = || std::fs::read_to_string(s.join("wake.log")).unwrap_or_default();
    let _ = fake.poll_json(&b);
    let t = ticket(&fake);
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    // a wake that does not act: the file, the receipt, then the miss, delivered again once
    let o = events_wake(&fake, &b, s, &events, "true");
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(&events).unwrap();
    assert!(
        text.contains(&format!("#{t}")) && text.contains("== Your tickets (crewboard mine):"),
        "{text}"
    );
    let repo = crate::support::board::REPO;
    assert!(
        log().contains(&format!("event delivered to wake: {repo}#{t} ")),
        "{}",
        log()
    );
    assert!(
        log().contains(&format!("event not acted on: {repo}#{t} ")),
        "{}",
        log()
    );
    let kept = std::fs::read_to_string(s.join("events.txt.unhandled")).unwrap();
    assert!(
        kept.contains(crewboard::ops::acted::REDELIVERED_MARKER),
        "{kept}"
    );
    // the next wake gets it again at the top and misses again: only the log
    let o = events_wake(&fake, &b, s, &events, "true");
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(&events).unwrap();
    assert!(
        text.starts_with("== An earlier wake ended early."),
        "{text}"
    );
    assert!(
        log().contains(&format!("event not acted on twice: {repo}#{t} ")),
        "{}",
        log()
    );
    assert!(
        !s.join("events.txt.unhandled").exists(),
        "a second miss delivers nothing again: {}\n{}\n{}",
        std::fs::read_to_string(s.join("events.txt.unhandled")).unwrap_or_default(),
        text,
        log()
    );
    // nothing is kept, the board is quiet: no wake
    let before = std::fs::read_to_string(s.join("wake.log"))
        .unwrap()
        .lines()
        .count();
    let o = events_wake(
        &fake,
        &b,
        s,
        &events,
        "echo should-not-run >> /dev/null; false",
    );
    assert_eq!((o.code, o.stderr.as_str()), (0, ""), "{}", o.all());
    assert_eq!(
        std::fs::read_to_string(s.join("wake.log"))
            .unwrap()
            .lines()
            .count(),
        before
    );
}

#[test]
fn a_detached_job_that_ended_wakes_a_quiet_board_once() {
    let (fake, _c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let events = s.join("events.txt");
    let _ = fake.poll_json(&b);
    let result = s.join("results/7.result");
    // the real writer: `lock run --detach --result` (an empty process table: no CI runner)
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(&b.dir)
        .args(["lock", "run"])
        .arg(s.join("heavy.lock.d"))
        .args(["--holder", "a@x", "--no-board", "--detach", "--result"])
        .arg(&result)
        .args(["--ticket", "7", "--", "true"])
        .env("HOST_LOCK_PS_CMD", "true")
        .env("GH_TOKEN", "ghp_faketoken")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let t = std::time::Instant::now();
    while !result.exists() {
        assert!(
            t.elapsed() < std::time::Duration::from_secs(30),
            "the job did not end"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let o = events_wake(&fake, &b, s, &events, "true");
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(&events).unwrap();
    assert!(
        text.contains("detached job finished: ticket 7 rc=0 "),
        "{text}"
    );
    assert!(s.join("results/7.result.seen").is_file());
    // reported once: the result is marked, so the result itself does not wake again. A wake that
    // did not act on the finished job gets it once more from `.unhandled`, then never
    let o = events_wake(&fake, &b, s, &events, "true");
    assert_eq!(o.code, 0, "{}", o.all());
    let log = std::fs::read_to_string(s.join("wake.log")).unwrap();
    assert!(log.contains("event not acted on twice: "), "{log}");
    assert!(!s.join("events.txt.unhandled").exists());
    let o = events_wake(&fake, &b, s, &events, "false");
    assert_eq!((o.code, o.stderr.as_str()), (0, ""), "{}", o.all());
}

#[test]
fn print_cmd_prints_the_command_and_starts_nothing() {
    let (fake, _c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let wd = tempfile::tempdir().unwrap();
    let marker = st.path().join("ran");
    let fake_rt = st.path().join("fake-claude");
    std::fs::write(&fake_rt, format!("#!/bin/sh\n: > '{}'\n", marker.display())).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_rt, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let state = st.path().join("state");
    let o = fake.run(
        &b.dir,
        &[
            "wake",
            "--runtime",
            "claude",
            "--runtime-bin",
            fake_rt.to_str().unwrap(),
            "--agent",
            &b.name,
            "--initiative",
            "demo",
            "--work-dir",
            wd.path().to_str().unwrap(),
            "--state-dir",
            state.to_str().unwrap(),
            "--no-events-hook",
            "--print-cmd",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.starts_with(&format!(
            "cd {} && {} -p ",
            wd.path().display(),
            fake_rt.display()
        )),
        "{}",
        o.stdout
    );
    assert!(!marker.exists(), "the runtime was not started");
    assert!(!state.exists(), "nothing was created");
    assert!(
        fake.requests().is_empty(),
        "no call to the board: {:?}",
        fake.requests()
    );
}

#[test]
fn the_other_keys_of_the_env_file_reach_the_wake_command_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_fake, _c, b) = crew();
    let st = tempfile::tempdir().unwrap();
    let s = st.path();
    let envf = s.join("agent.env");
    std::fs::write(
        &envf,
        "GH_TOKEN=ghp_from_file\nGH_HOST=host.example\nFOO=1\nANTHROPIC_API_KEY=key-from-file\nexport BAR=2\n",
    )
    .unwrap();
    std::fs::set_permissions(&envf, std::fs::Permissions::from_mode(0o600)).unwrap();
    // the board calls go to this program, which records its environment and says "events"
    let board = s.join("board");
    std::fs::write(
        &board,
        format!(
            "#!/bin/sh\nenv >> '{}'\nexit 0\n",
            s.join("board.env").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&board, std::fs::Permissions::from_mode(0o755)).unwrap();
    let child = s.join("child.env");
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(&b.dir)
        .arg("wake")
        .args(["--state-dir", s.join("state").to_str().unwrap()])
        .args(["--retry-delays", "", "--work-backoff", "off"])
        .args(["--env-file", envf.to_str().unwrap()])
        .args(["--crewboard", board.to_str().unwrap()])
        .args(["--wake-cmd", &format!("env > '{}'", child.display())])
        .env("GH_TOKEN", "ghp_of_the_caller")
        .env("GITHUB_TOKEN", "ghp_other_of_the_caller")
        .env_remove("FOO")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    assert!(
        !stderr.contains("ignored"),
        "no line per key in wake mode: {stderr}"
    );
    let child_env = std::fs::read_to_string(&child).expect("the wake command ran");
    let has = |text: &str, line: &str| text.lines().any(|l| l == line);
    for line in [
        "FOO=1",
        "ANTHROPIC_API_KEY=key-from-file",
        "BAR=2",
        "GH_HOST=host.example",
    ] {
        assert!(has(&child_env, line), "the wake command gets {line}");
    }
    // the token of the file, not a token of the caller
    assert!(has(&child_env, "GH_TOKEN=ghp_from_file"), "{child_env}");
    assert!(!child_env.contains("ghp_of_the_caller"));
    assert!(!child_env.lines().any(|l| l.starts_with("GITHUB_TOKEN=")));
    // the board calls (crewboard's own commands) get the settings of crewboard, not the others
    let board_env = std::fs::read_to_string(s.join("board.env")).expect("the board program ran");
    assert!(has(&board_env, "GH_HOST=host.example"));
    for key in ["FOO=", "ANTHROPIC_API_KEY=", "BAR="] {
        assert!(
            !board_env.lines().any(|l| l.starts_with(key)),
            "crewboard's own commands must not get {key}"
        );
    }
}
