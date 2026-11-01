//! The state of an older version.

use super::*;

/// A run with the legacy dir of an older version set.
fn run_legacy(rig: &Rig, legacy: &Path, config_dir: &str, steps: &mut Scripted) -> u8 {
    let mut s = rig.settings(&["--work-backoff", "900"], &[]).unwrap();
    s.legacy_dir = Some(legacy.to_path_buf());
    s.config_dir = Some(PathBuf::from(config_dir));
    rig.run_settings(steps, &s)
}

#[test]
fn the_old_last_wake_is_read_once_and_the_back_off_holds() {
    let rig = Rig::new();
    let legacy = rig.p("old");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("last-wake"), format!("{}\n", NOW - 60)).unwrap();
    // quiet poll, the last wake 60 s ago, back-off 900 s: `mine` is not asked
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st), 0);
    assert_eq!(st.count("mine"), 0, "{:?}", st.calls);
    assert_eq!(rig.state_text("last-wake"), Some(format!("{}\n", NOW - 60)));
    // twin: the file is copied only when the new state has none
    std::fs::write(legacy.join("last-wake"), "1\n").unwrap();
    let mut st = Scripted::new(&["1||"]);
    run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st);
    assert_eq!(rig.state_text("last-wake"), Some(format!("{}\n", NOW - 60)));
}

#[test]
fn a_live_old_lock_of_this_agent_starts_no_wake_another_agents_does_not() {
    let rig = Rig::new();
    let legacy = rig.p("old");
    std::fs::create_dir_all(legacy.join("wake.lock")).unwrap();
    let hold = |cfg: &str| {
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 30; :", "holder", "--config-dir", cfg])
            .spawn()
            .unwrap();
        // until the fork has become `sh`, `ps` shows the command line of this test binary
        let seen = || {
            SystemProcs
                .command(child.id())
                .is_some_and(|c| c.contains("holder"))
        };
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !seen() && std::time::Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(seen(), "the holder runs");
        child
    };
    let lock_pid =
        |pid: u32| std::fs::write(legacy.join("wake.lock/pid"), pid.to_string()).unwrap();
    // the holder names this agent's config dir (a final slash does not matter): no wake
    let mut same = hold("/x/agents/dev1/");
    lock_pid(same.id());
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st), 0);
    assert!(
        st.calls.is_empty(),
        "no call while an older wake runs: {:?}",
        st.calls
    );
    assert!(
        rig.log()
            .contains("an older per-initiative wake runs (pid "),
        "{}",
        rig.log()
    );
    assert_eq!(rig.wakes(), 0);
    same.kill().unwrap();
    same.wait().unwrap();
    // a dead holder is ignored
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    // another agent's holder: this agent wakes
    let mut other = hold("/x/agents/other");
    lock_pid(other.id());
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st), 0);
    assert_eq!(rig.wakes(), 2);
    other.kill().unwrap();
    other.wait().unwrap();
    // a holder with no --config-dir at all counts as this agent's
    let mut none = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    lock_pid(none.id());
    let before = rig.wakes();
    let mut st = Scripted::new(&["0||"]);
    run_legacy(&rig, &legacy, "/x/agents/dev1", &mut st);
    assert_eq!(rig.wakes(), before);
    none.kill().unwrap();
    none.wait().unwrap();
}

#[test]
fn the_holder_config_dir_is_the_word_after_the_flag() {
    assert_eq!(
        config_dir_of_command("bash poll-and-wake.sh --config-dir /a/b --x y").as_deref(),
        Some("/a/b")
    );
    assert_eq!(
        config_dir_of_command("bash poll-and-wake.sh --initiative i"),
        None
    );
}
