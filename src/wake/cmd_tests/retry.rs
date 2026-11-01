//! Retry and stop.

use super::*;

#[test]
fn r1_exit_3_is_retried_and_the_retry_finds_events() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||HTTP 502", "0||"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0"]), 0);
    assert_eq!((rig.wakes(), st.count("poll")), (1, 2));
    assert!(
        rig.log()
            .contains("crewboard poll: exit 3 (try 1): HTTP 502; retry in 0 s"),
        "{}",
        rig.log()
    );
    assert_eq!(
        rig.stderr().matches("retry in").count(),
        0,
        "a retry notice goes to the log only"
    );
}

#[test]
fn r1b_every_delay_is_waited_through_the_clock() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down", "3||down"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "10 30"]), 3);
    assert_eq!(
        st.count("poll"),
        3,
        "the first call and one retry per delay"
    );
    assert_eq!(rig.clock.now(), NOW + 40, "10 s and 30 s, no real sleep");
    let log = rig.log();
    assert!(
        log.contains("exit 3 (try 1): down; retry in 10 s")
            && log.contains("exit 3 (try 2): down; retry in 30 s")
    );
    // twin: a retry that succeeds stops waiting
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "1||"]);
    rig.run(
        &mut st,
        &["--retry-delays", "10 30", "--work-backoff", "off"],
    );
    assert_eq!((st.count("poll"), rig.clock.now()), (2, NOW + 10));
    // and exit 2 is never retried
    let rig = Rig::new();
    let mut st = Scripted::new(&["2||refused"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "10 30"]), 2);
    assert_eq!((st.count("poll"), rig.clock.now()), (1, NOW));
}

#[test]
fn r2_exit_3_in_every_try_pauses_the_runs_from_the_second_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down", "3||down", "3||down", "1||"]);
    let args = ["--retry-delays", "0"];
    assert_eq!(rig.run(&mut st, &args), 3);
    let b1 = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b1.fails, b1.until <= NOW), (1, true));
    assert_eq!(rig.run(&mut st, &args), 3);
    let b2 = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b2.fails, b2.until), (2, NOW + 300));
    let before = st.calls.len();
    assert_eq!(rig.run(&mut st, &args), 0);
    assert_eq!(st.calls.len(), before, "inside the pause nothing is called");
    assert_eq!(rig.wakes(), 0);
    // the paused run counts as a quiet poll
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        1
    );
}

#[test]
fn r2_the_owner_hears_of_it_at_the_third_failed_run_only() {
    let rig = Rig::new();
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    let said = rig.stderr();
    assert!(said.contains("GitHub or network error in 3 runs in a row; the owner should check the network and the token. The next runs pause up to 1 h"), "{said}");
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()),
        NetBackoff {
            fails: 3,
            until: NOW + 600
        }
    );
    // the fourth run does not say it again
    rig.write_state("net-backoff", &format!("3 {}\n", NOW - 1));
    let before = rig.stderr().matches("GitHub or network error").count();
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    assert_eq!(
        rig.stderr().matches("GitHub or network error").count(),
        before
    );
}

#[test]
fn r2b_when_the_pause_is_over_a_good_poll_clears_the_back_off() {
    let rig = Rig::new();
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    assert!(rig.state_text("net-backoff").is_none());
    // control: inside the pause the file stays
    rig.write_state("net-backoff", &format!("2 {}\n", NOW + 100));
    let mut st = Scripted::new(&["1||"]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert!(rig.state_text("net-backoff").is_some());
    assert_eq!(st.calls.len(), 0);
}

#[test]
fn x1_exit_4_stops_says_once_tells_the_coordinator_and_silences_the_next_run() {
    let rig = Rig::new();
    let env_file = rig.p("x.env");
    std::fs::write(&env_file, "GH_TOKEN=x\n").unwrap();
    let mut st =
        Scripted::new(&["4||configuration error: key file missing", "0||"]).with_mine(&[(
            0,
            "demo/app#3                         In Progress      P1  order 1\n",
        )]);
    let g = GlobalFlags {
        env_file: Some(env_file.clone()),
        ..GlobalFlags::default()
    };
    let run = |st: &mut Scripted| {
        let s = rig.settings(&["--agent", "dev@host-a"], &[]).unwrap();
        let s = Settings {
            env_file: g.env_file.clone(),
            ..s
        };
        rig.run_settings(st, &s)
    };
    assert_eq!(run(&mut st), 4);
    let calls = st.calls.len();
    assert_eq!(run(&mut st), 0, "the next run exits 0 at once");
    assert_eq!(st.calls.len(), calls, "and calls nothing");
    assert_eq!(st.count("mine"), 1);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .ends_with(" poll: configuration error: key file missing\n")
    );
    let log = rig.log();
    assert_eq!(
        log.matches("STOPPED: crewboard poll exit 4: configuration error: key file missing")
            .count(),
        1
    );
    assert!(rig.stderr().contains("STOPPED"));
    assert!(log.contains("told the coordinator on demo/app#3"));
    let said = st.calls.iter().find(|c| c.starts_with("say ")).unwrap();
    assert!(said.starts_with("say demo/app#3 anyway=true The wake wrapper of dev@host-a stopped: crewboard exit 4 (configuration error: key file missing)."), "{said}");
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn x1b_a_newer_env_file_lifts_the_stop_and_the_run_polls_again() {
    let rig = Rig::new();
    let env_file = rig.p("x.env");
    std::fs::write(&env_file, "GH_TOKEN=x\n").unwrap();
    rig.write_state("stopped", "ts poll: reason\n");
    let stop = rig.state().join("stopped");
    let mut st = Scripted::new(&["1||"]);
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    let s = Settings {
        env_file: Some(env_file.clone()),
        ..s
    };
    // the env file is older than the stop: still stopped, and silent
    set_mtime(&env_file, NOW - 1000);
    set_mtime(&stop, NOW);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(
        (st.calls.len(), rig.log(), rig.stderr()),
        (0, String::new(), String::new())
    );
    assert!(stop.exists());
    // the env file is newer: the stop is lifted and said, the poll runs
    set_mtime(&stop, NOW - 5000);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert!(!stop.exists());
    assert_eq!(st.count("poll --quiet"), 1);
    assert!(rig.log().contains(&format!(
        "stop lifted: {} changed after the stop; polling again",
        env_file.display()
    )));
}

#[test]
fn x1c_a_changed_agent_config_or_key_lifts_the_stop_too() {
    let rig = Rig::new();
    let cfg = rig.p("cfg");
    std::fs::create_dir_all(cfg.join("keys")).unwrap();
    std::fs::write(cfg.join("demo.toml"), "x").unwrap();
    std::fs::write(cfg.join("keys/dev.key"), "k").unwrap();
    std::fs::write(cfg.join("notes.txt"), "n").unwrap();
    let stop = rig.state().join("stopped");
    let cfg_s = cfg.to_str().unwrap();
    let go = |newer: &str| {
        rig.write_state("stopped", "ts poll: reason\n");
        for f in ["demo.toml", "keys/dev.key", "notes.txt"] {
            set_mtime(&cfg.join(f), NOW - 1000);
        }
        set_mtime(&stop, NOW);
        set_mtime(&cfg.join(newer), NOW + 100);
        let mut st = Scripted::new(&["1||"]);
        let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
        let s = Settings {
            config_dir: Some(PathBuf::from(cfg_s)),
            ..s
        };
        rig.run_settings(&mut st, &s);
        st.calls.len()
    };
    assert_eq!(go("demo.toml"), 1);
    assert_eq!(go("keys/dev.key"), 1);
    // a file that is no config does not lift it
    assert_eq!(go("notes.txt"), 0);
}

#[test]
fn x2_exit_2_is_no_stop_and_the_next_run_polls_again() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["2||refused [rule=x]", "1||"]);
    let a = ["--work-backoff", "off"];
    assert_eq!(rig.run(&mut st, &a), 2);
    assert_eq!(rig.run(&mut st, &a), 0);
    assert_eq!(st.count("poll --quiet"), 2);
    assert!(rig.state_text("stopped").is_none());
    assert!(
        rig.log()
            .contains("crewboard poll failed: exit 2: refused [rule=x]")
    );
}

#[test]
fn x1_exit_4_of_mine_in_the_precheck_stops_too() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(4, "")]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" mine --quiet: ")
    );
    assert!(
        rig.log()
            .contains("STOPPED: crewboard mine --quiet exit 4:")
    );
}

#[test]
fn exit_3_of_mine_in_the_precheck_counts_as_a_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(3, ""), (3, "")]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0"]), 3);
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    assert!(rig.log().contains("crewboard mine --quiet failed: exit 3"));
    assert_eq!(rig.wakes(), 0);
}
