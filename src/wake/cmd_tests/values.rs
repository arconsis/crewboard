//! Values and flags.

use super::*;

#[test]
fn bad_values_are_refused_with_the_texts_of_the_shell() {
    let rig = Rig::new();
    let text = |extra: &[&str]| rig.settings(extra, &[]).unwrap_err();
    let e = text(&["--work-backoff", "soon"]);
    assert_eq!(
        e.text,
        "--work-backoff needs whole seconds or off, got 'soon'"
    );
    assert_eq!(
        (e.code, e.paths.is_some()),
        (64, true),
        "a bad flag value is a usage error; the line goes to the log too"
    );
    assert_eq!(
        text(&["--retry-delays", "10 x"]).text,
        "--retry-delays needs whole seconds, got '10 x'"
    );
    // a negative number of seconds is a usage error of its own code
    let e = text(&["--stale-after", "-5"]);
    assert_eq!(e.code, 64, "{}", e.text);
    assert!(
        e.text
            .starts_with("--stale-after needs a number of seconds of 0 or more")
    );
    assert_eq!(text(&["--empty-grace", "-1"]).code, 64);
    // control: zero and the good forms pass
    let s = rig
        .settings(
            &[
                "--stale-after",
                "0",
                "--empty-grace",
                "0",
                "--retry-delays",
                "1 2",
            ],
            &[],
        )
        .unwrap();
    assert_eq!(
        (s.stale_after, s.empty_grace, s.retry_delays.clone()),
        (0, 0, vec![1, 2])
    );
    // the environment gives the same default as the flag, and the same refusal
    assert_eq!(
        rig.settings(&[], &[("CREWBOARD_WORK_BACKOFF", "off")])
            .unwrap()
            .work_backoff,
        WorkBackoff::Off
    );
    assert_eq!(
        rig.settings(&[], &[("CREWBOARD_EMPTY_GRACE", "-3")])
            .unwrap_err()
            .code,
        64
    );
    // a set value that is no number is a logged config error, like a bad --work-backoff
    let e = rig
        .settings(&[], &[("CREWBOARD_EMPTY_GRACE", "soon")])
        .unwrap_err();
    assert_eq!(
        e.text,
        "CREWBOARD_EMPTY_GRACE needs a number of seconds, got 'soon'"
    );
    assert_eq!((e.code, e.paths.is_some()), (4, true));
    // the flag wins over a bad variable
    assert!(
        rig.settings(
            &["--empty-grace", "5"],
            &[("CREWBOARD_EMPTY_GRACE", "soon")]
        )
        .is_ok()
    );
}

#[test]
fn flags_that_nothing_reads_are_not_accepted() {
    // nothing in this command reads them, so they must not look as if they worked
    let argv = ["wake", "--wake-cmd", "true", "--role", "x"];
    assert!(P::try_parse_from(argv).is_err(), "--role is accepted");
    assert!(P::try_parse_from(["wake", "--wake-cmd", "true", "--tick"]).is_ok());
}

#[test]
fn the_defaults_and_the_environment_fill_the_options() {
    let rig = Rig::new();
    let s = rig.settings(&["--retry-delays", "10 30"], &[]).unwrap();
    assert_eq!(s.retry_delays, vec![10, 30]);
    assert_eq!((s.stale_after, s.empty_grace), (14_400, 30));
    assert_eq!(s.work_backoff, WorkBackoff::Secs(900));
    // environment: set but empty means no retry
    let a = parse(&["--state-dir", "/s", "--wake-cmd", "x"]);
    let env = |k: &str| match k {
        "CREWBOARD_RETRY_DELAYS" => Some(String::new()),
        "CREWBOARD_EMPTY_GRACE" => Some("7".into()),
        _ => None,
    };
    let Plan::Run(s) = resolve(&a, &GlobalFlags::default(), &env, None).unwrap() else {
        panic!()
    };
    assert_eq!((s.retry_delays.len(), s.empty_grace), (0, 7));
}

#[test]
fn the_agent_dir_names_the_config_dir_the_agent_and_the_state_dir() {
    let agent = crate::config::AgentConfig {
        name: "dev@host".into(),
        runtime: "claude-code".into(),
        host: "h".into(),
        item_id: "I".into(),
        board_sha: None,
        board: crate::config::BoardConfig::parse(
            "initiative = \"demo\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"ow\"\nregistry_issue = 1\n",
        )
        .unwrap(),
        insecure_one_account: false,
        tools: None,
        loop_cfg: None,
        wake: None,
    };
    let a = parse(&["--agent-dir", "/cfg/dev1", "--wake-cmd", "x"]);
    let env = |k: &str| (k == "XDG_STATE_HOME").then(|| "/st".to_string());
    let Plan::Run(s) = resolve(&a, &GlobalFlags::default(), &env, Some(&agent)).unwrap() else {
        panic!()
    };
    assert_eq!(s.config_dir, Some(PathBuf::from("/cfg/dev1")));
    assert_eq!(
        (s.agent.as_deref(), s.initiative.as_deref()),
        (Some("dev@host"), Some("demo"))
    );
    assert_eq!(s.paths.dir, PathBuf::from("/st/crewboard/agents/demo-dev1"));
    // an explicit --config-dir wins over the agent dir; a flag wins over the config
    let g = GlobalFlags {
        config_dir: Some("/cfg/other".into()),
        ..GlobalFlags::default()
    };
    let b = parse(&[
        "--agent-dir",
        "/cfg/dev1",
        "--wake-cmd",
        "x",
        "--agent",
        "x@y",
    ]);
    let Plan::Run(s) = resolve(&b, &g, &env, Some(&agent)).unwrap() else {
        panic!()
    };
    assert_eq!(
        (s.config_dir, s.agent.as_deref()),
        (Some(PathBuf::from("/cfg/other")), Some("x@y"))
    );
}
