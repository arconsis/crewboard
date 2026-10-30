use super::crew::{approve_all, approve_next_step, approve_to};
use super::watch::{loop_args, watch_command_args, watch_settings};
use super::*;
use crate::base::ids;
use crate::exit::Family;
use crate::ops::approve::Approval;
use clap::{CommandFactory, Parser};

/// One argv per command of spec/cli#commands, in spec order.
const ARGV: &[(&str, &[&str])] = &[
    (
        "init",
        &[
            "init",
            "--initiative",
            "demo",
            "--repo",
            "a/b",
            "--repo",
            "a/c",
        ],
    ),
    (
        "join",
        &[
            "join",
            "--agent",
            "claude@host-a",
            "--runtime",
            "claude",
            "--model",
            "sonnet",
        ],
    ),
    (
        "approve",
        &["approve", "hermes@host-b", "--role", "coordinator"],
    ),
    ("start", &["start"]),
    ("watch", &["watch", "--timeout", "10m"]),
    ("poll", &["poll", "--quiet"]),
    ("say", &["say", "12", "--to", "owner", "--kind", "report"]),
    ("take", &["take", "12"]),
    (
        "done",
        &["done", "12", "--pr", "https://github.com/a/b/pull/1"],
    ),
    ("block", &["block", "12", "--why", "waiting"]),
    ("ask", &["ask", "12", "--body", "which API?"]),
    ("unblock", &["unblock", "12"]),
    ("assign", &["assign", "12", "--to", "hermes@host-b"]),
    ("ticket", &["ticket", "adopt", "12"]),
    ("verify", &["verify", "12"]),
    ("merge", &["merge", "12"]),
    ("close", &["close", "12", "--verified"]),
    ("reassign", &["reassign", "12", "--reason", "stopping"]),
    ("report-status", &["report-status", "--health", "on-track"]),
    ("prune", &["prune"]),
    ("pause", &["pause"]),
    ("resume", &["resume"]),
    ("stop", &["stop"]),
    ("status", &["status", "--json"]),
    ("monitor", &["monitor", "--max-cycles", "1"]),
    ("mine", &["mine", "--quiet"]),
    ("heartbeat", &["heartbeat"]),
    ("tick", &["tick"]),
    (
        "acted",
        &["acted", "--events-file", "/nonexistent/events.txt"],
    ),
    ("audit", &["audit"]),
    ("move", &["move", "7", "In Progress", "--why", "x"]),
    ("doctor", &["doctor"]),
    ("lock", &["lock", "status", "/nonexistent/lock.d"]),
    ("skill", &["skill", "path"]),
    ("update", &["update", "--check"]),
    ("wake", &["wake", "--print-state-dir", "--initiative", "x"]),
    ("repeat", &["repeat", "--times", "1", "--", "true"]),
    ("hook", &["hook", "post-tool-use", "--every", "0"]),
    (
        "agent",
        &["agent", "env", "--agent-dir", "/nonexistent/agent"],
    ),
];

fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    let mut v = vec!["crewboard"];
    v.extend_from_slice(args);
    Cli::try_parse_from(v)
}

#[test]
fn join_takes_the_runtime_and_the_model_and_refuses_the_old_label_form() {
    let cli = parse(&[
        "join",
        "--agent",
        "claude@host-a",
        "--runtime",
        "claude",
        "--model",
        "sonnet",
    ])
    .unwrap();
    let Command::Crew(CrewCmd::Join { runtime, model, .. }) = &cli.command else {
        panic!("not join");
    };
    assert_eq!(*runtime, crate::ops::agent::AgentRuntime::Claude);
    assert_eq!(model.as_deref(), Some("sonnet"));
    // the model is optional
    let cli = parse(&["join", "--agent", "codex@host-a", "--runtime", "codex"]).unwrap();
    let Command::Crew(CrewCmd::Join { model, .. }) = &cli.command else {
        panic!("not join");
    };
    assert_eq!(*model, None);
    // the old form is a usage error (main maps it to exit 4) that shows the new form
    let e = parse(&[
        "join",
        "--agent",
        "claude@host-a",
        "--runtime",
        "claude-code/sonnet",
    ])
    .unwrap_err();
    assert_eq!(e.kind(), clap::error::ErrorKind::ValueValidation);
    assert!(e.use_stderr());
    assert!(
        e.to_string()
            .contains("use: --runtime claude --model sonnet"),
        "{e}"
    );
    let e = parse(&[
        "join",
        "--agent",
        "claude@host-a",
        "--runtime",
        "claude",
        "--model",
        "a b",
    ])
    .unwrap_err();
    assert!(e.to_string().contains("--model must not be empty"), "{e}");
}

#[test]
fn a_negative_order_reaches_the_order_rule_not_a_clap_error() {
    for argv in [
        &["assign", "12", "--to", "hermes@host-b", "--order", "-3"][..],
        &["assign", "12", "--order", "-3"][..],
    ] {
        parse(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    }
}

#[test]
fn clap_definition_is_consistent() {
    Cli::command().debug_assert();
}

/// Every path of nested commands, hidden ones too, without the generated `help`.
fn command_paths(cmd: &clap::Command, prefix: &[String], out: &mut Vec<Vec<String>>) {
    for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
        let mut p = prefix.to_vec();
        p.push(sub.get_name().to_string());
        out.push(p.clone());
        command_paths(sub, &p, out);
    }
}

#[test]
fn help_of_every_nested_command_works_as_help_word_and_as_flag() {
    use clap::error::ErrorKind;
    let mut paths = Vec::new();
    command_paths(&Cli::command(), &[], &mut paths);
    assert!(paths.iter().any(|p| p == &["agent", "add"]), "{paths:?}");
    for p in paths {
        for flag in [false, true] {
            let mut argv = vec!["crewboard".to_string()];
            if flag {
                argv.extend(p.clone());
                argv.push("--help".into());
            } else {
                argv.push("help".into());
                argv.extend(p.clone());
            }
            // `lock` and `gate` take free-form arguments and read `--help` themselves
            if flag && p[0] == "lock" {
                continue;
            }
            let e = Cli::try_parse_from(&argv)
                .expect_err(&format!("{argv:?} must print help, not parse"));
            assert_eq!(e.kind(), ErrorKind::DisplayHelp, "{argv:?}: {e}");
        }
    }
}

fn missing_help(cmd: &clap::Command, path: &str, out: &mut Vec<String>) {
    for a in cmd.get_arguments() {
        let id = a.get_id().as_str();
        if id == "help" || id == "version" {
            continue;
        }
        let empty = a.get_help().is_none_or(|h| h.to_string().trim().is_empty());
        if empty {
            out.push(format!("{path} --{id}"));
        }
    }
    for sub in cmd.get_subcommands() {
        let p = format!("{path} {}", sub.get_name());
        if sub.get_name() != "help"
            && sub
                .get_about()
                .is_none_or(|h| h.to_string().trim().is_empty())
        {
            out.push(format!("{p} (subcommand)"));
        }
        missing_help(sub, &p, out);
    }
}

#[test]
fn every_arg_and_subcommand_has_a_help_text() {
    let mut out = Vec::new();
    missing_help(&Cli::command(), "crewboard", &mut out);
    assert!(out.is_empty(), "no help text for: {out:#?}");
    // pass direction: the two args that adopters ask about say what to type
    let join = Cli::command();
    let join = join.find_subcommand("join").unwrap();
    let help_of = |n: &str| {
        join.get_arguments()
            .find(|a| a.get_id() == n)
            .and_then(|a| a.get_help().map(|h| h.to_string()))
            .unwrap_or_default()
    };
    assert!(
        help_of("agent").contains("claude@host-a"),
        "{}",
        help_of("agent")
    );
    assert!(
        help_of("runtime").contains("claude-code/"),
        "{}",
        help_of("runtime")
    );
}

#[test]
fn every_spec_command_exists_and_is_named_as_typed() {
    let defined: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    assert_eq!(defined.len(), ARGV.len(), "{defined:?}");
    for (name, argv) in ARGV {
        let cli = parse(argv).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(&cli.command.name(), name);
        assert!(
            defined.iter().any(|d| d == name),
            "{name} missing from the clap tree"
        );
    }
}

#[test]
fn every_command_of_the_spec_is_built_none_is_left_as_a_stub() {
    // The dispatch has no catch-all arm, so a new Command variant without a
    // handler does not compile. Here: every argv that needs an agent config reaches its
    // own handler and fails for its own reason (no config), never as `not-implemented`.
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("empty");
    let mut checked = 0;
    for (name, argv) in ARGV {
        if matches!(
            *name,
            "init"
                | "approve"
                | "audit"
                | "status"
                | "say"
                | "ask"
                | "verify"
                | "reassign"
                | "report-status"
                | "doctor"
                | "lock"
                | "skill"
                | "update"
                | "wake"
                | "repeat"
                | "hook"
                | "agent"
        ) {
            // they look for a board config or check a body first; doctor and skill need none
            continue;
        }
        let mut v = vec!["crewboard", "--config-dir", cfgdir.to_str().unwrap()];
        v.extend_from_slice(argv);
        let out = run(&Cli::try_parse_from(v).unwrap());
        let msg = out.message.clone().unwrap_or_default();
        assert!(!msg.contains("not-implemented"), "{name}: {msg}");
        assert_eq!(out.exit, Exit::Config, "{name}: {msg}");
        checked += 1;
    }
    assert!(checked >= 12, "only {checked} commands were reached");
}

#[test]
fn watch_poll_and_mine_are_the_watch_family() {
    for (name, argv) in ARGV {
        let fam = parse(argv).unwrap().command.family();
        // `mine` shares the exit code 1 = "nothing"
        let expect = match *name {
            "watch" | "monitor" | "poll" | "mine" => Family::Watch,
            "lock" => Family::Lock,
            "wake" => Family::Wake,
            "repeat" => Family::Repeat,
            _ => Family::Other,
        };
        assert_eq!(fam, expect, "{name}");
    }
}

fn monitor_of(args: &[&str]) -> (Cli, MonitorArgs) {
    let cli = parse(args).unwrap();
    let Command::Watch(WatchCmd::Monitor { m }) = &cli.command else {
        panic!("not monitor")
    };
    let m = m.clone();
    (cli, m)
}

#[test]
fn monitor_flags_and_the_arguments_after_the_dashes_merge_for_watch() {
    // the plain flags reach watch
    let (cli, m) = monitor_of(&[
        "--config-dir",
        "/x/cfg",
        "monitor",
        "--timeout",
        "5m",
        "--interval",
        "20s",
        "--initiative",
        "demo",
        "--reset-cursor",
    ]);
    let w = watch_settings(&m).unwrap();
    let (before, after) = watch_command_args(&cli, &w);
    assert_eq!(before, ["--config-dir", "/x/cfg"]);
    assert_eq!(
        after,
        [
            "--timeout",
            "5m",
            "--interval",
            "20s",
            "--initiative",
            "demo",
            "--reset-cursor"
        ]
    );
    // the arguments after -- win and add to them
    let (cli, m) = monitor_of(&[
        "monitor",
        "--timeout",
        "5m",
        "--initiative",
        "demo",
        "--",
        "--timeout",
        "1m",
        "--initiative",
        "other",
        "--initiative-file",
        "/x/board.toml",
        "--reset-cursor",
        "--json",
    ]);
    let w = watch_settings(&m).unwrap();
    assert_eq!(w.timeout.as_deref(), Some("1m"));
    assert!(w.json && w.reset_cursor);
    let (before, after) = watch_command_args(&cli, &w);
    assert!(before.is_empty());
    assert_eq!(
        after,
        [
            "--timeout",
            "1m",
            "--initiative",
            "other",
            "--initiative-file",
            "/x/board.toml",
            "--reset-cursor",
            "--json"
        ]
    );
    // without arguments after -- the flags stay as they are
    let (_, m) = monitor_of(&["monitor", "--timeout", "5m"]);
    assert_eq!(watch_settings(&m).unwrap().timeout.as_deref(), Some("5m"));
}

#[test]
fn an_argument_that_watch_does_not_take_is_an_error() {
    for bad in [
        &["--bogus"][..],
        &["--timeout"],
        &["--config-dir", "/x"],
        &["say"],
        &["--help"],
    ] {
        let mut v = vec!["monitor", "--"];
        v.extend_from_slice(bad);
        let (_, m) = monitor_of(&v);
        assert!(watch_settings(&m).is_err(), "{bad:?}");
    }
}

#[test]
fn bad_values_are_usage_errors() {
    assert!(parse(&["say", "12", "--to", "owner", "--kind", "shout"]).is_err());
    assert!(parse(&["say", "12", "--to", "Everyone", "--kind", "info"]).is_err());
    assert!(parse(&["init", "--initiative", "x"]).is_err());
    assert!(parse(&["frobnicate"]).is_err());
}

fn say_cli(dir: &std::path::Path, extra: &[&str]) -> Cli {
    let mut v = vec![
        "crewboard",
        "--config-dir",
        dir.to_str().unwrap(),
        "say",
        "12",
        "--to",
        "owner",
        "--kind",
        "info",
    ];
    v.extend_from_slice(extra);
    Cli::try_parse_from(v).unwrap()
}

#[test]
fn say_refuses_a_secret_before_any_config_or_network_and_passes_clean_text_on() {
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("empty-config");
    let leak = dir.path().join("leak.txt");
    let token: String = "aB3dE5gH7jK9mN1pQ3sT5vW7yZ"
        .chars()
        .cycle()
        .take(36)
        .collect();
    std::fs::write(&leak, format!("my token is ghp_{token}\n")).unwrap();
    let clean = dir.path().join("clean.txt");
    std::fs::write(&clean, "CI is green. Log: https://example.org/run/1\n").unwrap();

    let leaked = run(&say_cli(&cfgdir, &["--body-file", leak.to_str().unwrap()]));
    assert_eq!(leaked.exit, Exit::Refused);
    let msg = leaked.message.unwrap();
    assert!(msg.contains("[rule=secret-in-body]"), "{msg}");
    assert!(!msg.contains(&token), "the secret must not be echoed");

    // Clean text passes the pre-checks and then fails for lack of a config: exit 4,
    // not 2. That shows the secret scan was what stopped the first one.
    let ok = run(&say_cli(&cfgdir, &["--body-file", clean.to_str().unwrap()]));
    assert_eq!(ok.exit, Exit::Config, "{:?}", ok.message);
    assert!(ok.message.unwrap().contains("no agent config"));

    let missing = run(&say_cli(&cfgdir, &["--body-file", "/nonexistent/nope.txt"]));
    assert_eq!(missing.exit, Exit::Config);
}

#[test]
fn say_refuses_a_body_over_65536_chars_and_accepts_exactly_65536() {
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("empty-config");
    let say = |n: usize| {
        let f = dir.path().join(format!("b{n}.txt"));
        std::fs::write(&f, "ä".repeat(n)).unwrap();
        run(&say_cli(&cfgdir, &["--body-file", f.to_str().unwrap()]))
    };
    let long = say(65_537);
    assert_eq!(long.exit, Exit::Refused);
    assert!(long.message.unwrap().contains("[rule=body-too-long]"));
    let edge = say(65_536);
    assert_eq!(
        edge.exit,
        Exit::Config,
        "passes the size check, then no config"
    );
}

#[test]
fn commands_that_need_a_config_say_so_with_exit_4_and_never_with_exit_1() {
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("none");
    for argv in [
        vec!["watch"],
        vec!["poll"],
        vec!["poll", "--quiet"],
        vec!["start"],
        vec!["agent", "selftest"],
        vec!["status"],
    ] {
        let mut v = vec!["crewboard", "--config-dir", cfgdir.to_str().unwrap()];
        v.extend(argv.iter().copied());
        let out = run(&Cli::try_parse_from(v).unwrap());
        assert_eq!(out.exit, Exit::Config, "{argv:?}: {:?}", out.message);
        assert_ne!(out.exit.code(), 1);
    }
}

#[test]
fn approve_role_must_be_dev_or_coordinator() {
    assert!(parse(&["approve", "a@b", "--role", "admin"]).is_err());
    assert!(parse(&["approve", "a@b"]).is_ok());
}

#[test]
fn approve_takes_one_or_more_names_and_needs_one() {
    assert!(parse(&["approve"]).is_err());
    match parse(&["approve", "a@b", "c@d", "--role", "dev"])
        .unwrap()
        .command
    {
        Command::Crew(CrewCmd::Approve { names, .. }) => assert_eq!(names, ["a@b", "c@d"]),
        _ => panic!("not approve"),
    }
}

#[test]
fn approve_ends_with_the_next_step_for_the_approved_agents() {
    let one = approve_next_step(&["a@x".to_string()]);
    assert!(
        one.starts_with("next step: tell the agent host of a@x to run"),
        "{one}"
    );
    assert!(
        one.contains("crewboard agent add") && one.contains("`start`"),
        "{one}"
    );
    let two = approve_next_step(&["a@x".to_string(), "c@x".to_string()]);
    assert!(two.contains("a@x, c@x"), "{two}");
}

#[test]
fn help_and_doctor_name_the_setup_check_for_a_new_agent() {
    let help = crate::cli::overview::render(&Cli::command(), false);
    assert!(help.contains("crewboard doctor --setup"), "{help}");
    assert!(ops::doctor::SETUP_HINT.contains("doctor --setup"));
}

#[test]
fn approve_all_judges_each_name_and_a_failure_does_not_hide_the_others() {
    let mut seen = Vec::new();
    let names: Vec<String> = ["a@x", "bad@x", "c@x"].map(String::from).to_vec();
    let o = approve_all(&names, |n| {
        seen.push(n.to_string());
        if n == "bad@x" {
            Err(CbError::refused(
                ids::AGENT_REJECTED,
                format!("{n} was rejected"),
            ))
        } else {
            Ok(ap(format!("{n} approved\n"), true))
        }
    });
    assert_eq!(seen, names, "every name is judged");
    assert_eq!(o.exit, Exit::Refused);
    let msg = o.message.unwrap();
    assert!(
        msg.contains("approve bad@x") && msg.contains("agent-rejected"),
        "{msg}"
    );
    assert!(!msg.contains("a@x") && !msg.contains("c@x"), "{msg}");
    // control: all pass -> exit 0 and no message
    let all_ok = approve_all(&names[..1], |n| Ok(ap(format!("{n} approved\n"), true)));
    assert_eq!(all_ok.exit, Exit::Ok);
    let two = approve_all(&[names[0].clone(), names[2].clone()], |_| {
        Ok(ap(String::new(), true))
    });
    assert_eq!((two.exit, two.message), (Exit::Ok, None));
    // a network error first, then a refusal: the first failure gives the exit code
    let mixed = approve_all(&names, |n| match n {
        "a@x" => Err(CbError::config("bad config")),
        "bad@x" => Err(CbError::refused(ids::AGENT_REJECTED, "x")),
        _ => Ok(ap(String::new(), true)),
    });
    assert_eq!(mixed.exit, Exit::Config);
}

#[test]
fn close_takes_a_reason_or_verified_but_not_both() {
    assert!(parse(&["close", "12"]).is_ok());
    assert!(parse(&["close", "12", "--verified"]).is_ok());
    assert!(parse(&["close", "12", "--why", "x"]).is_ok());
    assert!(parse(&["close", "12", "--verified", "--why", "x"]).is_err());
    assert!(parse(&["close", "12", "--verified", "--reason", "x"]).is_err());
    for gone in ["--merge", "--merged", "--wont-fix", "--superseded-by"] {
        assert!(parse(&["close", "12", gone]).is_err(), "{gone}");
    }
}

/// The free text of take, block, close, assign and reassign is `--why`. The old names are gone.
#[test]
fn why_is_the_free_text_flag_and_the_old_names_are_gone() {
    let c = parse(&["take", "12", "--interrupt", "--why", "x"])
        .unwrap()
        .command;
    assert!(matches!(c, Command::Dev(DevCmd::Take { why: Some(w), .. }) if w == "x"));
    let c = parse(&["block", "12", "--why", "x"]).unwrap().command;
    assert!(matches!(c, Command::Dev(DevCmd::Block { why, .. }) if why == "x"));
    let c = parse(&["close", "12", "--why", "x"]).unwrap().command;
    assert!(matches!(c, Command::Review(ReviewCmd::Close { why: Some(w), .. }) if w == "x"));
    let c = parse(&[
        "assign",
        "12",
        "--to",
        "a@h",
        "--override-capabilities",
        "--why",
        "x",
    ])
    .unwrap()
    .command;
    assert!(matches!(c, Command::Plan(PlanCmd::Assign { why: Some(w), .. }) if w == "x"));
    let c = parse(&["reassign", "12", "--reason", "late", "--why", "x"])
        .unwrap()
        .command;
    assert!(matches!(
        c,
        Command::Plan(PlanCmd::Reassign { reason: Some(r), why: Some(w), .. })
            if r == "late" && w == "x"
    ));
    assert!(parse(&["take", "12", "--interrupt", "--reason", "x"]).is_err());
    assert!(parse(&["block", "12", "--reason", "x"]).is_err());
    assert!(parse(&["close", "12", "--reason", "x"]).is_err());
    assert!(parse(&["reassign", "12", "--reason", "late", "--detail", "x"]).is_err());
    // `--reason` of `reassign` is the typed code and needs a word of the list
    assert!(parse(&["reassign", "12", "--reason", "because"]).is_err());
}

/// `--override-unread` is the one flag of say, done, block, ask and unblock.
#[test]
fn override_unread_is_one_flag_and_the_old_names_are_gone() {
    for old in ["--anyway", "--ignore-events"] {
        assert!(parse(&["done", "12", "--pr", "https://github.com/a/b/pull/1", old]).is_err());
        assert!(parse(&["block", "12", "--why", "x", old]).is_err());
    }
    {
        let flag = "--override-unread";
        let c = parse(&["done", "12", "--pr", "https://github.com/a/b/pull/1", flag])
            .unwrap()
            .command;
        assert!(
            matches!(
                c,
                Command::Dev(DevCmd::Done {
                    override_unread: true,
                    ..
                })
            ),
            "{flag}"
        );
    }
    {
        let flag = "--override-unread";
        let c = parse(&["block", "12", "--why", "x", flag]).unwrap().command;
        assert!(
            matches!(
                c,
                Command::Dev(DevCmd::Block {
                    override_unread: true,
                    ..
                })
            ),
            "{flag}"
        );
        let c = parse(&["ask", "12", "--body", "q", flag]).unwrap().command;
        assert!(
            matches!(
                c,
                Command::Dev(DevCmd::Ask {
                    override_unread: true,
                    ..
                })
            ),
            "{flag}"
        );
        let c = parse(&["unblock", "12", flag]).unwrap().command;
        assert!(
            matches!(
                c,
                Command::Plan(PlanCmd::Unblock {
                    override_unread: true,
                    ..
                })
            ),
            "{flag}"
        );
        let c = parse(&["say", "12", "--to", "owner", "--kind", "info", flag])
            .unwrap()
            .command;
        assert!(
            matches!(
                c,
                Command::Dev(DevCmd::Say {
                    override_unread: true,
                    ..
                })
            ),
            "{flag}"
        );
    }
    // the flag is off by default, and `--ignore-events` is not a flag of `block`
    let c = parse(&["block", "12", "--why", "x"]).unwrap().command;
    assert!(matches!(
        c,
        Command::Dev(DevCmd::Block {
            override_unread: false,
            ..
        })
    ));
    // clap lists the new name and hides the old ones
    let help = Cli::command()
        .find_subcommand("done")
        .unwrap()
        .clone()
        .render_long_help()
        .to_string();
    assert!(help.contains("--override-unread"), "{help}");
    assert!(
        !help.contains("--anyway") && !help.contains("--ignore-events"),
        "{help}"
    );
}

#[test]
fn reassign_needs_a_reason_or_derived_and_not_both() {
    assert!(parse(&["reassign", "12"]).is_err());
    assert!(parse(&["reassign", "12", "--reason", "dead"]).is_ok());
    assert!(parse(&["reassign", "12", "--derived"]).is_ok());
    assert!(parse(&["reassign", "12", "--derived", "--reason", "dead"]).is_err());
}

#[test]
fn the_lifecycle_commands_take_agent_only_for_the_owner_path() {
    for v in ["pause", "resume", "stop"] {
        assert!(parse(&[v]).is_ok());
        assert!(parse(&[v, "--agent", "claude@host-a"]).is_ok());
        assert!(parse(&[v, "--agent", "x"]).is_ok()); // name syntax is checked by the command
    }
}

#[test]
fn the_heartbeat_interval_flag_is_gone() {
    assert!(parse(&["--heartbeat-interval", "20s", "poll", "--quiet"]).is_err());
    assert!(parse(&["poll", "--heartbeat-interval", "20s"]).is_err());
}

#[test]
fn report_status_refuses_an_unknown_state_before_any_config() {
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("none");
    let out = run(&Cli::try_parse_from([
        "crewboard",
        "--config-dir",
        cfgdir.to_str().unwrap(),
        "report-status",
        "--health",
        "green",
    ])
    .unwrap());
    assert_eq!(out.exit, Exit::Config);
    assert!(out.message.unwrap().contains("not a status"));
    // control: a known state gets as far as the config
    let ok = run(&Cli::try_parse_from([
        "crewboard",
        "--config-dir",
        cfgdir.to_str().unwrap(),
        "report-status",
        "--health",
        "on-track",
    ])
    .unwrap());
    assert!(ok.message.unwrap().contains("no agent config"));
}

fn ap(text: String, changed: bool) -> Approval {
    Approval { text, changed }
}

#[test]
fn approve_of_an_agent_with_nothing_to_do_has_no_next_step() {
    let idle = |n: &str| {
        ap(
            format!("{n} is already approved and active; nothing to do\n"),
            false,
        )
    };
    let fresh = |n: &str| {
        ap(
            format!("{n} approved as dev; key K is frozen; State=approved\n"),
            true,
        )
    };
    // one idle agent: the one line and nothing else
    let mut out = String::new();
    let o = approve_to(&mut out, &["a@x".to_string()], |n| Ok(idle(n)));
    assert_eq!(o.exit, Exit::Ok);
    assert_eq!(out, "a@x is already approved and active; nothing to do\n");
    // one new agent: its text, then the next step
    let mut out = String::new();
    approve_to(&mut out, &["c@x".to_string()], |n| Ok(fresh(n)));
    assert!(out.starts_with("c@x approved as dev;"), "{out}");
    assert_eq!(
        out.lines().last().unwrap(),
        approve_next_step(&["c@x".to_string()])
    );
    // one idle and one new agent: the next step names only the new one
    let mut out = String::new();
    let o = approve_to(&mut out, &["a@x".to_string(), "c@x".to_string()], |n| {
        Ok(if n == "a@x" { idle(n) } else { fresh(n) })
    });
    assert_eq!(o.exit, Exit::Ok);
    assert!(
        out.contains("approve a@x:\na@x is already approved and active; nothing to do\n"),
        "{out}"
    );
    assert!(out.contains("approved 2 of 2\n"), "{out}");
    assert_eq!(
        out.lines().last().unwrap(),
        approve_next_step(&["c@x".to_string()])
    );
    assert_eq!(out.matches("next step").count(), 1, "{out}");
    // all idle: no next step line
    let mut out = String::new();
    approve_to(&mut out, &["a@x".to_string(), "b@x".to_string()], |n| {
        Ok(idle(n))
    });
    assert!(!out.contains("next step"), "{out}");
}

#[test]
fn the_moved_commands_parse_in_their_new_homes_and_the_old_names_still_parse() {
    for argv in [
        &[
            "status", "--agents", "--can", "x86_64", "--can", "tool:git", "--json",
        ][..],
        &["agent", "profile", "show", "--json"],
        &["agent", "profile", "set", "--tag", "x", "--agent", "a@h"],
        &[
            "agent",
            "token",
            "--initiative",
            "demo",
            "--crew-login",
            "bot",
        ],
        &["agent", "selftest"],
        &[
            "watch",
            "--loop",
            "--pause",
            "1",
            "--max-cycles",
            "2",
            "--log",
            "/x/l",
            "--json",
        ],
        &["monitor", "--max-cycles", "1"],
    ] {
        parse(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    }
}

#[test]
fn the_options_of_status_agents_and_watch_loop_need_their_switch() {
    for bad in [
        &["status", "--can", "x86_64"][..],
        &["status", "--agents", "--queue"],
        &["status", "--agents", "--verbose"],
        &["watch", "--pause", "1"],
        &["watch", "--max-cycles", "1"],
        &["watch", "--log", "/x/l"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?} must not parse");
    }
}

#[test]
fn the_old_names_are_gone_and_the_new_homes_are_listed() {
    let c = Cli::command();
    assert!(c.find_subcommand("monitor").unwrap().is_hide_set());
    for old in [
        "agents",
        "profile",
        "selftest",
        "crew-token",
        "reorder",
        "rebalance",
        "gate",
        "migrate",
    ] {
        assert!(c.find_subcommand(old).is_none(), "{old}");
    }
    let agent = c.find_subcommand("agent").unwrap();
    for new in ["profile", "token", "selftest"] {
        let s = agent.find_subcommand(new).unwrap();
        assert!(!s.is_hide_set(), "agent {new}");
    }
    let watch = c.find_subcommand("watch").unwrap();
    assert!(watch.get_arguments().any(|a| a.get_long() == Some("loop")));
    let status = c.find_subcommand("status").unwrap();
    for flag in ["agents", "can"] {
        assert!(status.get_arguments().any(|a| a.get_long() == Some(flag)));
    }
}

#[test]
fn watch_loop_makes_the_same_loop_as_monitor() {
    let cli = parse(&[
        "watch",
        "--loop",
        "--timeout",
        "5m",
        "--interval",
        "20s",
        "--pause",
        "3",
        "--max-cycles",
        "7",
        "--log",
        "/x/l.log",
        "--reset-cursor",
        "--initiative",
        "demo",
    ])
    .unwrap();
    let Command::Watch(WatchCmd::Watch {
        timeout,
        interval,
        json,
        reset_cursor,
        watch_loop,
        pause,
        max_cycles,
        log,
        sel,
    }) = &cli.command
    else {
        panic!("not watch")
    };
    assert!(*watch_loop);
    let m = loop_args(
        timeout,
        interval,
        *json,
        *reset_cursor,
        (pause, *max_cycles, log),
        sel,
    );
    assert_eq!(m.timeout.as_deref(), Some("5m"));
    assert_eq!(m.interval.as_deref(), Some("20s"));
    assert_eq!(m.pause.as_deref(), Some("3"));
    assert_eq!(m.max_cycles, 7);
    assert_eq!(m.log.as_deref(), Some(std::path::Path::new("/x/l.log")));
    assert!(m.reset_cursor && !m.json && m.watch_args.is_empty());
    assert_eq!(m.sel.initiative.as_deref(), Some("demo"));
    // the same settings reach `watch` as for `monitor` with those flags
    let w = watch_settings(&m).unwrap();
    assert_eq!(w.timeout.as_deref(), Some("5m"));
    assert!(w.reset_cursor && !w.json);
    // without `--max-cycles` the loop never ends by itself, as `monitor` does
    let cli = parse(&["watch", "--loop"]).unwrap();
    let Command::Watch(WatchCmd::Watch {
        timeout,
        interval,
        json,
        reset_cursor,
        pause,
        max_cycles,
        log,
        sel,
        ..
    }) = &cli.command
    else {
        panic!("not watch")
    };
    let m = loop_args(
        timeout,
        interval,
        *json,
        *reset_cursor,
        (pause, *max_cycles, log),
        sel,
    );
    assert_eq!(m.max_cycles, 0);
    // `--json` is the form of the last line and also of the events
    let cli = parse(&["watch", "--loop", "--json"]).unwrap();
    let Command::Watch(WatchCmd::Watch {
        json,
        pause,
        max_cycles,
        log,
        sel,
        timeout,
        interval,
        reset_cursor,
        ..
    }) = &cli.command
    else {
        panic!("not watch")
    };
    let m = loop_args(
        timeout,
        interval,
        *json,
        *reset_cursor,
        (pause, *max_cycles, log),
        sel,
    );
    assert!(m.json);
    assert!(watch_settings(&m).unwrap().json);
}

#[test]
fn a_new_home_reaches_the_handler_of_its_old_name() {
    // In an empty config dir both stop at the config load, with the same exit and message.
    let dir = tempfile::tempdir().unwrap();
    let cfgdir = dir.path().join("empty");
    let run_in = |argv: &[&str]| {
        let mut v = vec!["crewboard", "--config-dir", cfgdir.to_str().unwrap()];
        v.extend_from_slice(argv);
        run(&Cli::try_parse_from(v).unwrap())
    };
    {
        let old = &["monitor", "--max-cycles", "1"][..];
        let new = &["watch", "--loop", "--max-cycles", "1"][..];
        let (a, b) = (run_in(old), run_in(new));
        assert_eq!(a.exit, Exit::Config, "{old:?}: {a:?}");
        assert_eq!(a, b, "{old:?} and {new:?} differ");
    }
}

#[test]
fn the_hidden_tuning_flags_of_wake_and_the_hidden_globals_still_parse() {
    for argv in [
        &[
            "wake",
            "--tick",
            "--work-backoff",
            "off",
            "--retry-delays",
            "10 30",
            "--stale-after",
            "60",
            "--empty-grace",
            "5",
            "--state-dir",
            "/x/s",
            "--lock-dir",
            "/x/l",
            "--log",
            "/x/log",
            "--events-file",
            "/x/e",
            "--max-wake",
            "45m",
            "--heartbeat-every",
            "300",
            "--print-state-dir",
        ][..],
        &[
            "wake",
            "--runtime",
            "claude",
            "--allow",
            "Bash",
            "--runtime-flags",
            "--x",
            "--runtime-bin",
            "/x/claude",
            "--prompt-extra",
            "hi",
            "--host-lock",
            "/x/h",
            "--no-events-hook",
            "--events-every",
            "60",
            "--shim-dir",
            "/x/bin",
            "--print-cmd",
        ],
        &[
            "wake",
            "install",
            "--agent-dir",
            "/x/a",
            "--max-wake",
            "1h",
            "--dry-run",
        ],
        &["status", "--initiative-file", "/x/board.toml"],
    ] {
        parse(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"));
    }
}

#[test]
fn the_hidden_flags_are_hidden_and_the_listed_ones_are_not() {
    let c = Cli::command();
    let wake = c.find_subcommand("wake").unwrap();
    let hidden = |name: &str| {
        wake.get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("no --{name}"))
            .is_hide_set()
    };
    for name in [
        "work-backoff",
        "retry-delays",
        "stale-after",
        "empty-grace",
        "state-dir",
        "lock-dir",
        "log",
        "events-file",
        "max-wake",
        "heartbeat-every",
        "print-state-dir",
        "shim-dir",
        "allow",
        "runtime-flags",
        "runtime-bin",
        "prompt-extra",
        "host-lock",
        "no-events-hook",
        "events-every",
        "print-cmd",
    ] {
        assert!(hidden(name), "--{name} is shown");
    }
    for name in ["agent-dir", "runtime", "work-dir", "model", "tick"] {
        assert!(!hidden(name), "--{name} is hidden");
    }
    let install = wake.find_subcommand("install").unwrap();
    for name in ["scheduler", "every", "dry-run", "replace"] {
        let a = install
            .get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("no --{name}"));
        assert!(!a.is_hide_set(), "--{name} is hidden");
    }
    let global = |name: &str| {
        c.get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("no --{name}"))
            .is_hide_set()
    };
    assert!(!global("config-dir") && !global("env-file"));
    let status = c.find_subcommand("status").unwrap();
    let hidden_of = |name: &str| {
        status
            .get_arguments()
            .find(|a| a.get_long() == Some(name))
            .unwrap_or_else(|| panic!("no --{name}"))
            .is_hide_set()
    };
    assert!(hidden_of("initiative-file") && !hidden_of("initiative"));
}
