//! The verdict of the headless scope for each command (spec/agents#headless-coordinator).
//!
//! `headless` holds what a scope is (the rule, the reasons, the config key, the Gate checks);
//! this module decides, for every command of the tree, whether a headless coordinator may run
//! it.

use super::{
    AgentAction, Command, CrewCmd, DevCmd, HostCmd, PlanCmd, ProfileAction, ReviewCmd, SkillAction,
    TicketAction, WatchCmd,
};
use crate::error::CbResult;
use crate::headless::{self, ScopeVerdict, UAct};
use crate::protocol::names::{Kind, To};

/// The verdict of a command for a headless coordinator. There is no catch-all arm: a new
/// command does not compile until someone decides its verdict.
pub(super) fn verdict(c: &Command) -> ScopeVerdict {
    use ScopeVerdict::{Allow, Deny, Unattended};
    match c {
        // read-only, or the liveness of the agent itself
        Command::Watch(WatchCmd::Watch { .. })
        | Command::Watch(WatchCmd::Monitor { .. })
        | Command::Watch(WatchCmd::Poll { .. })
        | Command::Watch(WatchCmd::Status { .. })
        | Command::Watch(WatchCmd::Mine { .. })
        | Command::Watch(WatchCmd::Acted { .. })
        | Command::Crew(CrewCmd::Audit { .. })
        | Command::Review(ReviewCmd::Retro { .. })
        | Command::Crew(CrewCmd::Heartbeat { .. })
        | Command::Crew(CrewCmd::Start { .. }) => Allow,
        // a local report of the scope; it reads no board and writes nothing
        Command::Host(HostCmd::Doctor { .. }) => Allow,
        // the host lock changes files of the host, not the board; its board check only reads
        Command::Host(HostCmd::Lock { .. }) => Allow,
        // `skill path` prints a path. `skill install` rewrites the skill and the loop scripts of
        // this host: a headless model must not repoint or rewrite its own skill
        Command::Host(HostCmd::Skill {
            action: SkillAction::Path,
        }) => Allow,
        Command::Host(HostCmd::Skill {
            action: SkillAction::Install { .. },
        }) => Deny(headless::DENY_OWNER),
        // `update` replaces the binary and the skill of this host: a headless model must not
        // rewrite its own tooling
        Command::Host(HostCmd::Update { .. }) => Deny(headless::DENY_OWNER),
        // the wake polls the board and starts the wake command of the agent's own scheduler
        Command::Host(HostCmd::Wake { action: None, .. }) => Allow,
        // `wake install` and `wake uninstall` change the scheduler of this host: a model must not
        // change its own wake; `status` only reads
        Command::Host(HostCmd::Wake {
            action: Some(crate::wake::install::WakeAction::Status(_)),
            ..
        }) => Allow,
        Command::Host(HostCmd::Wake { .. }) => Deny(headless::DENY_OWNER),
        // the events hook of a wake only reads the board (`poll`) and appends to the agent's own
        // events file; a headless config dir may run it
        Command::Host(HostCmd::Hook { .. }) => Allow,
        // `repeat` runs a command of the repo and records evidence; it writes no board field.
        // Under a scope it needs the grant of `lock run` (`repeat_cmd::scope_grant`)
        Command::Host(HostCmd::Repeat(_)) => Allow,
        // `check --authority-file` runs the command lines of a local file, which a model can
        // write: a draft of AUTHORITY.md is owner work
        Command::Host(HostCmd::Check {
            authority_file: Some(_),
            ..
        }) => Deny(headless::DENY_OWNER),
        // `check` runs the commands of AUTHORITY.md on the default branch as `tick` runs them
        // (no check whose command line names a file that a PR can change), and writes nothing
        Command::Host(HostCmd::Check { .. }) => Allow,
        // local work: `exec` and `env` set up a shell, `with-env` runs a command that its own
        // guard judges
        Command::Host(HostCmd::Agent {
            action: AgentAction::Exec { .. } | AgentAction::Env { .. } | AgentAction::WithEnv { .. },
        }) => Allow,
        // `add` and `shim` set an agent up: a model must not change its own setup
        Command::Host(HostCmd::Agent {
            action: AgentAction::Add { .. } | AgentAction::Shim { .. },
        }) => Deny(headless::DENY_OWNER),
        // `agent token` writes the env file of the agent account: a model must not change it
        Command::Host(HostCmd::Agent {
            action: AgentAction::Token { .. },
        }) => Deny(headless::DENY_OWNER),
        // `agent selftest` proves that the agent sees events
        Command::Host(HostCmd::Agent {
            action: AgentAction::Selftest { .. },
        }) => Allow,
        // `agent profile show` reads; `agent profile set` writes the profile of the registry
        Command::Host(HostCmd::Agent {
            action:
                AgentAction::Profile {
                    action: ProfileAction::Show { .. },
                },
        }) => Allow,
        Command::Host(HostCmd::Agent {
            action:
                AgentAction::Profile {
                    action: ProfileAction::Set { .. },
                },
        }) => Deny(headless::DENY_WRITE),
        // `tick` does only what `verify` and `merge` do, with their checks; the command itself
        // refuses in a config dir that is not headless (spec/agents#headless-coordinator)
        Command::Review(ReviewCmd::Tick { .. }) => Allow,
        Command::Crew(CrewCmd::Resume { agent: None, .. }) => Allow,
        Command::Crew(CrewCmd::Resume { agent: Some(_), .. }) => Deny(headless::DENY_OWNER),
        // `verify --local-run` records the coordinator's own local run (`ci_mode: local`)
        Command::Review(ReviewCmd::Verify {
            local_run: Some(_), ..
        }) => Deny(headless::DENY_LOCAL_GATE),
        // a run that is named by hand: the Gate is not CI only, and the unattended coordinator
        // names no run either
        Command::Review(ReviewCmd::Verify { runs, .. })
        | Command::Review(ReviewCmd::Merge { runs, .. })
            if !runs.is_empty() =>
        {
            Deny(headless::DENY_RUN)
        }
        // `verify` of a ticket whose signed Gate is `ci` is no judgment. Any other `verify`
        // (another Gate, `--fail`, `--run-check`) is one: the command asks the mode of the
        // unattended coordinator, after it read the signed Gate
        Command::Review(ReviewCmd::Verify { .. }) => Unattended(UAct::Verdict),
        Command::Review(ReviewCmd::Merge { .. }) => Allow,
        // a bare `close` ends a ticket whose PR the owner merged by hand. It is no
        // judgment (the owner's merge is the fact), so `tick` does it too. `close --why` and
        // `close --verified` are judgments of the coordinator, so `tick` never does them.
        Command::Review(ReviewCmd::Close { why: Some(_), .. })
        | Command::Review(ReviewCmd::Close { verified: true, .. }) => {
            Deny(headless::DENY_CLOSE_JUDGMENT)
        }
        Command::Review(ReviewCmd::Close { .. }) => Allow,
        Command::Crew(CrewCmd::Init { .. })
        | Command::Crew(CrewCmd::Join { .. })
        | Command::Crew(CrewCmd::Approve { .. })
        | Command::Crew(CrewCmd::Move { .. }) => Deny(headless::DENY_OWNER),
        // the judgments of the unattended coordinator: the command asks its mode, its budget
        // and the signed log of the ticket (spec/agents#unattended-coordinator)
        Command::Dev(DevCmd::Say {
            kind: Kind::Answer,
            to: To::Agent(_),
            ..
        }) => Unattended(UAct::Answer),
        Command::Dev(DevCmd::Ask { .. }) => Unattended(UAct::Ask),
        Command::Plan(PlanCmd::Unblock { .. }) => Unattended(UAct::Unblock),
        Command::Plan(PlanCmd::Assign {
            to: Some(_),
            order: None,
            override_capabilities: false,
            ..
        }) => Unattended(UAct::Assign),
        // a position in a queue and a capability override are decisions about the plan
        Command::Plan(PlanCmd::Assign { to: Some(_), .. }) => Deny(headless::DENY_ASSIGN_ARGS),
        Command::Dev(DevCmd::Say { .. })
        | Command::Dev(DevCmd::Take { .. })
        | Command::Dev(DevCmd::Done { .. })
        | Command::Dev(DevCmd::Block { .. })
        | Command::Plan(PlanCmd::Assign { .. })
        | Command::Plan(PlanCmd::Reassign { .. })
        | Command::Review(ReviewCmd::ReportStatus { .. })
        | Command::Crew(CrewCmd::Prune { .. }) => Deny(headless::DENY_WRITE),
        // `ticket new --template` prints a constant text: it reads no config and no board and
        // writes nothing, like `doctor`
        Command::Plan(PlanCmd::Ticket {
            action: TicketAction::New { template: true, .. },
        }) => Allow,
        Command::Plan(PlanCmd::Ticket { action }) => match action {
            TicketAction::New { .. } | TicketAction::Adopt { .. } | TicketAction::Split { .. } => {
                Deny(headless::DENY_WRITE)
            }
        },
        Command::Crew(CrewCmd::Pause { .. }) | Command::Crew(CrewCmd::Stop { .. }) => {
            Deny(headless::DENY_LIFECYCLE)
        }
    }
}

/// The check that `dispatch` runs first, before any config load or request.
///
/// `doctor` and an allowed command pass. Any other command is refused when an agent config of
/// the config dir has `headless = true`.
pub(super) fn guard(c: &Command, config_dir: Option<&std::path::Path>) -> CbResult<()> {
    if matches!(c, Command::Host(HostCmd::Doctor { .. })) {
        return Ok(());
    }
    headless::enforce(c.name(), verdict(c), config_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn cmd(args: &[&str]) -> Command {
        let mut v = vec!["crewboard"];
        v.extend_from_slice(args);
        Cli::try_parse_from(v).unwrap().command
    }

    /// The verdict of a command in the table, in its three states.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum V {
        /// Allowed in a headless dir.
        Allow,
        /// Refused in a headless dir with `headless-scope`, before any request.
        Deny,
        /// The command asks the mode of the unattended coordinator: `unattended-off` with the
        /// mode `off`, and the checks of the act with any other mode.
        Mode(UAct),
    }
    use V::{Allow as A, Deny as D};

    fn state(v: &ScopeVerdict) -> V {
        match v {
            ScopeVerdict::Allow => V::Allow,
            ScopeVerdict::Deny(_) => V::Deny,
            ScopeVerdict::Unattended(a) => V::Mode(*a),
        }
    }

    /// Every command with its verdict.
    const TABLE: &[(&[&str], V)] = &[
        (&["init", "--initiative", "d", "--repo", "a/b"], D),
        (
            &["join", "--agent", "claude@host-a", "--runtime", "claude"],
            D,
        ),
        (&["approve", "hermes@host-b"], D),
        (&["start"], A),
        (&["watch"], A),
        (&["monitor"], A),
        (&["poll", "--quiet"], A),
        (&["say", "1", "--to", "owner", "--kind", "info"], D),
        // an answer to an agent is a judgment of the unattended coordinator; no other `say` is
        (
            &["say", "1", "--to", "hermes@host-b", "--kind", "answer"],
            V::Mode(UAct::Answer),
        ),
        (&["say", "1", "--to", "owner", "--kind", "answer"], D),
        (&["say", "1", "--to", "coordinator", "--kind", "answer"], D),
        (&["say", "1", "--to", "hermes@host-b", "--kind", "info"], D),
        (
            &["say", "1", "--to", "hermes@host-b", "--kind", "request"],
            D,
        ),
        (&["take", "1"], D),
        (&["done", "1", "--pr", "https://github.com/a/b/pull/1"], D),
        (&["block", "1", "--why", "x"], D),
        (&["ask", "1", "--body", "x"], V::Mode(UAct::Ask)),
        (&["unblock", "1"], V::Mode(UAct::Unblock)),
        (&["unblock", "1", "--body", "x"], V::Mode(UAct::Unblock)),
        (
            &["assign", "1", "--to", "hermes@host-b"],
            V::Mode(UAct::Assign),
        ),
        // a queue position and a capability override are for the interactive coordinator
        (&["assign", "1", "--to", "hermes@host-b", "--order", "3"], D),
        (&["assign", "1", "--order", "3"], D),
        (
            &[
                "assign",
                "1",
                "--to",
                "hermes@host-b",
                "--override-capabilities",
                "--why",
                "x",
            ],
            D,
        ),
        (&["ticket", "new", "--title", "t", "--body-file", "f"], D),
        (&["ticket", "adopt", "1"], D),
        (
            &["ticket", "split", "1", "--title", "t", "--body-file", "f"],
            D,
        ),
        // `verify` reads the signed Gate first: a `ci` Gate needs no mode, any other does
        (&["verify", "1"], V::Mode(UAct::Verdict)),
        (&["verify", "1", "--run", "9"], D),
        (
            &["verify", "1", "--fail", "--body", "x"],
            V::Mode(UAct::Verdict),
        ),
        (&["verify", "1", "--fail", "--body", "x", "--run", "9"], D),
        (&["verify", "1", "--body", "x"], V::Mode(UAct::Verdict)),
        (&["verify", "1", "--run-check", "a"], V::Mode(UAct::Verdict)),
        (
            &["verify", "1", "--run-check", "a", "--must-fail-at", "abc"],
            V::Mode(UAct::Verdict),
        ),
        (&["verify", "1", "--local-run", "host-a: make test rc=0"], D),
        (&["merge", "1"], A),
        (&["merge", "1", "--run", "9"], D),
        (&["close", "1"], A),
        (&["close", "1", "--why", "x"], D),
        (&["close", "1", "--verified"], D),
        (&["reassign", "1", "--reason", "dead"], D),
        (&["report-status", "--health", "on-track"], D),
        (&["prune"], D),
        (&["pause"], D),
        (&["pause", "--agent", "hermes@host-b"], D),
        (&["resume"], A),
        (&["resume", "--agent", "hermes@host-b"], D),
        (&["stop"], D),
        (&["status"], A),
        (&["mine", "--quiet"], A),
        (&["acted", "--events-file", "/x/events.txt"], A),
        (&["heartbeat"], A),
        (&["tick"], A),
        (&["audit"], A),
        (&["retro", "--since", "24h"], A),
        // the owner moves a ticket by hand; a model must not write the owner's word
        (&["move", "7", "Ready"], D),
        (&["doctor"], A),
        // the host lock changes files of the host, not the board
        (&["lock", "status", "/x/lock.d"], A),
        (
            &["lock", "run", "/x/lock.d", "--holder", "a@x", "--", "true"],
            A,
        ),
        (&["skill", "install"], D),
        (&["skill", "path"], A),
        (&["update", "--check"], D),
        // the wake polls the board and runs the wake command of the agent's scheduler
        (&["wake", "--wake-cmd", "true"], A),
        (&["wake", "--tick"], A),
        // the events hook of a wake reads the board and appends to the events file of the wake
        (&["hook", "post-tool-use", "--every", "0"], A),
        // the gate runs a command of the repo and writes no board field
        (&["repeat", "--times", "1", "--", "true"], A),
        // it runs commands of AUTHORITY.md, as `tick` does, and writes nothing
        (&["check", "fmt"], A),
        (&["check", "fmt", "--must-fail-at", "abc"], A),
        // the command lines of a local file: owner work
        (&["check", "--authority-file", "draft.md"], D),
        // local work: a shell setup, or a command that its own guard judges
        (&["agent", "env"], A),
        // it writes the env file of the agent account: a model must not change it
        (&["agent", "token", "--initiative", "x"], D),
        // the moved commands keep the verdict of their old names
        (&["agent", "selftest"], A),
        (&["agent", "profile", "show"], A),
        (&["agent", "profile", "set", "--tag", "x"], D),
        (&["status", "--agents", "--can", "x86_64"], A),
        (&["monitor", "--max-cycles", "1"], A),
        (&["agent", "exec", "--", "true"], A),
        (
            &["agent", "with-env", "--env-file", "/x/a.env", "--", "true"],
            A,
        ),
    ];

    #[test]
    fn the_verdict_of_every_command_is_the_table() {
        for (argv, allowed) in TABLE {
            let v = verdict(&cmd(argv));
            assert_eq!(state(&v), *allowed, "{argv:?}: {v:?}");
        }
        // every subcommand of the command tree is in the table
        use clap::CommandFactory;
        for c in Cli::command().get_subcommands() {
            assert!(
                TABLE.iter().any(|(a, _)| a[0] == c.get_name()),
                "{} has no verdict in the table",
                c.get_name()
            );
        }
    }

    #[test]
    fn the_refusal_names_the_rule_the_command_and_what_to_do() {
        let c = cmd(&["reassign", "1", "--reason", "dead"]);
        let ScopeVerdict::Deny(why) = verdict(&c) else {
            panic!("reassign must be refused")
        };
        let r = headless::refusal(c.name(), why);
        assert_eq!(r.rule, "headless-scope");
        assert!(r.message.contains("`reassign`") && r.message.contains("interactive"));
        assert_eq!(
            r.to_string().split(']').next().unwrap(),
            "refused [rule=headless-scope"
        );
    }

    fn dir_with_agent_config(first_line: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "{first_line}\nname = \"claude@host-a\"\nruntime = \"x\"\nhost = \"h\"\nitem_id = \"PVTI_x\"\n\n[board]\ninitiative = \"demo\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n"
        );
        std::fs::write(dir.path().join("demo.toml"), text).unwrap();
        dir
    }

    #[test]
    fn the_guard_refuses_in_a_headless_dir_and_lets_the_interactive_dir_through() {
        let headless = dir_with_agent_config("headless = true");
        let interactive = dir_with_agent_config("# interactive");
        let denied = [
            vec!["assign", "1", "--to", "hermes@host-b", "--order", "3"],
            vec!["assign", "1", "--order", "3"],
            vec!["reassign", "1", "--reason", "dead"],
            vec!["ticket", "adopt", "1"],
            vec!["say", "1", "--to", "owner", "--kind", "info"],
            vec!["verify", "1", "--run", "9"],
        ];
        for argv in &denied {
            let c = cmd(argv);
            let e = guard(&c, Some(headless.path())).unwrap_err();
            assert!(
                e.to_string().contains("[rule=headless-scope]"),
                "{argv:?}: {e}"
            );
            assert_eq!(e.exit(), crate::exit::Exit::Refused);
            // control: the same command is not refused in the interactive dir
            assert!(guard(&c, Some(interactive.path())).is_ok(), "{argv:?}");
        }
        // the allowed commands pass in the headless dir, too
        for argv in [vec!["merge", "1"], vec!["status"]] {
            assert!(
                guard(&cmd(&argv), Some(headless.path())).is_ok(),
                "{argv:?}"
            );
        }
        // the guard lets a judgment of the unattended coordinator pass: the command decides
        // after it loaded its context (the mode, the budget, the signed log of the ticket)
        for argv in [
            vec!["verify", "1"],
            vec!["verify", "1", "--fail", "--body", "x"],
            vec!["say", "1", "--to", "hermes@host-b", "--kind", "answer"],
            vec!["unblock", "1"],
            vec!["ask", "1", "--body", "x"],
            vec!["assign", "1", "--to", "hermes@host-b"],
        ] {
            let c = cmd(&argv);
            assert!(
                matches!(verdict(&c), ScopeVerdict::Unattended(_)),
                "{argv:?}"
            );
            assert!(guard(&c, Some(headless.path())).is_ok(), "{argv:?}");
        }
        // a config dir without any agent config is not headless
        let empty = tempfile::tempdir().unwrap();
        assert!(
            guard(
                &cmd(&["reassign", "1", "--reason", "dead"]),
                Some(empty.path())
            )
            .is_ok()
        );
    }

    #[test]
    fn a_headless_value_that_is_not_a_boolean_refuses_with_a_config_error_not_silence() {
        let bad = dir_with_agent_config("headless = \"yes\"");
        let e = guard(
            &cmd(&["reassign", "1", "--reason", "dead"]),
            Some(bad.path()),
        )
        .unwrap_err();
        assert_eq!(e.exit(), crate::exit::Exit::Config, "{e}");
    }
}
