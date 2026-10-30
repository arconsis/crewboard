//! The overview that the real binary prints for `crewboard`, `-h`, `help` and `--help`.
//! The unit tests of `src/cli/overview.rs` cover the table and the text; this file covers the
//! wiring in `main`: the stream and the exit code of each form.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .args(args)
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env("NO_COLOR", "1")
        .output()
        .expect("run crewboard")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).expect("utf-8")
}

#[test]
fn no_arguments_print_the_overview_to_stdout_and_exit_0_like_help() {
    let o = run(&[]);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stderr.is_empty(), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert!(
        out.contains("\nSet up (owner)\n") && out.contains("doctor --setup"),
        "{out}"
    );
}

#[test]
fn an_option_without_a_command_and_a_missing_subcommand_are_still_usage_errors() {
    for args in [&["--config-dir", "/nonexistent"][..], &["agent"][..]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(64), "{args:?}");
        assert!(o.stdout.is_empty(), "{args:?}");
        assert!(!text(&o.stderr).contains("\nSet up (owner)\n"), "{args:?}");
    }
}

#[test]
fn h_help_word_and_long_help_print_the_overview_to_stdout_and_exit_0() {
    for args in [&["-h"][..], &["help"], &["--help"]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        let out = text(&o.stdout);
        assert!(out.starts_with("crewboard: "), "{args:?}: {out}");
        assert!(
            out.lines().all(|l| l.chars().count() <= 80),
            "{args:?}: {out}"
        );
        // the hidden global option works, and no overview lists it
        assert!(!out.contains("--initiative-file"), "{args:?}: {out}");
        assert_eq!(
            out.contains("--config-dir <CONFIG_DIR>"),
            args == ["--help"],
            "{args:?}: only the long help has the full text of the options"
        );
        // `--initiative` is a per-command option, not a global one: no overview lists it
        assert!(!out.contains("--initiative <NAME>"), "{args:?}: {out}");
    }
}

#[test]
fn help_of_one_command_is_not_the_overview() {
    let o = run(&["help", "update"]);
    assert_eq!(o.status.code(), Some(0));
    let out = text(&o.stdout);
    assert!(out.starts_with("Update or downgrade this binary"), "{out}");
    assert!(out.contains("Exit codes: 0"), "{out}");
    let o = run(&["update", "-h"]);
    assert!(text(&o.stdout).starts_with("Update or downgrade this binary"));
}

#[test]
fn help_extensions_prints_the_optional_commands_to_stdout_and_exits_0() {
    let o = run(&["help", "extensions"]);
    assert_eq!(o.status.code(), Some(0));
    let out = text(&o.stdout);
    assert!(out.starts_with("crewboard extensions: "), "{out}");
    for want in [
        "\nHeavy jobs on a shared host\n",
        "  lock ",
        "  audit ",
        "  repeat ",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    // the old names of the queue moves are hidden: `assign` does them
    assert!(
        !out.contains("  reorder") && !out.contains("  rebalance"),
        "{out}"
    );
    assert!(out.lines().all(|l| l.chars().count() <= 80), "{out}");
    assert!(!out.contains("\nSet up (owner)\n"), "{out}");
}

#[test]
fn help_scheduler_prints_the_machine_called_commands_to_stdout_and_exits_0() {
    let o = run(&["help", "scheduler"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stderr.is_empty());
    let out = text(&o.stdout);
    assert!(out.starts_with("crewboard scheduler: "), "{out}");
    for want in [
        "  wake ",
        "  poll ",
        "  tick ",
        "  heartbeat ",
        "  monitor ",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    assert!(out.lines().all(|l| l.chars().count() <= 80), "{out}");
    assert!(!out.contains("\nSet up (owner)\n"), "{out}");
    // global options in front do not matter
    let o = run(&["--config-dir", "x", "help", "scheduler"]);
    assert!(text(&o.stdout).starts_with("crewboard scheduler: "));
}

#[test]
fn the_hidden_machine_commands_still_have_a_help_of_their_own() {
    for name in [
        "wake",
        "poll",
        "tick",
        "heartbeat",
        "monitor",
        "acted",
        "hook",
    ] {
        let o = run(&[name, "--help"]);
        assert_eq!(o.status.code(), Some(0), "{name}");
        assert!(!o.stdout.is_empty(), "{name}");
    }
    // a tuning option of `wake` is hidden from the list and still works
    let o = run(&["wake", "--help"]);
    let out = text(&o.stdout);
    assert!(!out.contains("\n      --work-backoff"), "{out}");
    assert!(out.contains("--work-backoff"), "named in the text: {out}");
}

#[test]
fn the_overview_names_the_extensions_on_a_few_lines() {
    let o = run(&["help"]);
    let out = text(&o.stdout);
    assert!(
        out.contains(
            "\nMore\n  crewboard help extensions   audit, move, report-status, prune, lock, repeat\n  crewboard help scheduler    wake, poll, tick, heartbeat (called by schedulers)\n"
        ),
        "{out}"
    );
    // 26 commands in 4 groups, none of the extensions or scheduler commands
    for gone in [
        "  monitor ",
        "  poll ",
        "  tick ",
        "  wake ",
        "  heartbeat ",
        "  lock ",
    ] {
        assert!(!out.contains(gone), "{gone}: {out}");
    }
}

#[test]
fn the_overview_follows_the_newcomer_path_and_has_an_owner_line() {
    let out = text(&run(&["help"]).stdout);
    let at = |s: &str| out.find(s).unwrap_or_else(|| panic!("{s}: {out}"));
    assert!(
        out.contains(
            "New crew (owner): `crewboard agent token`, `crewboard init`,\n\
             then for each agent `crewboard agent add` on its host and `crewboard approve`.\n"
        ),
        "{out}"
    );
    assert!(out.contains("docs/quick-try.md"), "{out}");
    assert!(at("\n  init ") < at("\n  agent ") && at("\n  agent ") < at("\n  approve "));
    assert!(at("\n  approve ") < at("\n  join ") && at("\n  join ") < at("\n  mine "));
    assert!(at("\n  status ") < at("\n  update ") && at("\n  update ") < at("\n  join "));
    assert!(at("\n  skill ") < at("\n  update "));
}

#[test]
fn help_extensions_ends_with_a_url_and_no_repo_path() {
    let out = text(&run(&["help", "extensions"]).stdout);
    assert!(
        out.contains("https://github.com/arconsis/crewboard/blob/main/docs/guides/extensions.md"),
        "{out}"
    );
    assert!(!out.contains("in the repository"), "{out}");
}

#[test]
fn help_word_works_for_a_nested_command() {
    for args in [
        &["help", "agent", "add"][..],
        &["help", "agent"],
        &["help", "wake", "install"],
        &["help", "agent", "token"],
    ] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", text(&o.stderr));
        assert!(!text(&o.stdout).starts_with("crewboard: "), "{args:?}");
    }
}
