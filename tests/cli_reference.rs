//! `docs/reference/cli.md` is generated from `--help` of the real binary: one section for the
//! top level and one for every command and subcommand that `--help` lists (hidden commands are
//! not listed, so they are not in the page). The test fails when the page is out of date.
//!
//! To regenerate the page: `CREWBOARD_UPDATE_DOCS=1 cargo test --test cli_reference`.
//!
//! The page documents the Unix binary: some commands (`lock`, `skill`, `agent`) are Unix only and
//! have no help on Windows, so the whole file runs on Unix only.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_crewboard");
const PAGE: &str = "docs/reference/cli.md";

fn help(path: &[String]) -> String {
    let out = Command::new(BIN)
        .args(path)
        .arg("--help")
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env_remove("CREWBOARD_HEARTBEAT_INTERVAL")
        .env("NO_COLOR", "1")
        .output()
        .expect("run crewboard --help");
    assert!(out.status.success(), "`--help` failed for {path:?}");
    String::from_utf8(out.stdout).expect("utf-8 help")
}

/// The text of a help topic, `crewboard help <topic>`.
fn topic(name: &str) -> String {
    let out = Command::new(BIN)
        .args(["help", name])
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env("NO_COLOR", "1")
        .output()
        .expect("run crewboard help <topic>");
    assert!(out.status.success(), "`help {name}` failed");
    String::from_utf8(out.stdout).expect("utf-8 help")
}

/// The names in the `Commands:` section, without `help`. The top level has no such section: its
/// overview, and the two topics, list `  name  text` lines under group headings, up to the
/// options. The `More` lines of the overview (`  crewboard help ...`) name no command.
fn subcommands(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    if text.starts_with("crewboard: ")
        || text.starts_with("crewboard extensions: ")
        || text.starts_with("crewboard scheduler: ")
    {
        for line in text.lines() {
            if line.starts_with("Options") {
                break;
            }
            if line.starts_with("  ")
                && !line.starts_with("   ")
                && let Some(name) = line.split_whitespace().next()
                && name.starts_with(|c: char| c.is_ascii_lowercase())
                && name != "crewboard"
            {
                names.push(name.to_string());
            }
        }
        return names;
    }
    let mut in_section = false;
    for line in text.lines() {
        if line == "Commands:" {
            in_section = true;
        } else if in_section {
            if !line.starts_with("  ") {
                break;
            }
            if let Some(name) = line.split_whitespace().next()
                && name != "help"
                && !line.starts_with("   ")
            {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Option blocks of the `Options:` section: a line with 6 spaces and `-` or 2 spaces and `-`,
/// followed by indented description lines.
fn option_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        if line == "Options:" {
            in_section = true;
        } else if in_section {
            if line.is_empty() {
                // long help puts a blank line between two options
                continue;
            }
            if !line.starts_with(' ') {
                break;
            }
            if line.starts_with("  ") && !line.starts_with("   ") || line.starts_with("      -") {
                blocks.push(line.to_string());
            } else if let Some(last) = blocks.last_mut() {
                last.push('\n');
                last.push_str(line);
            }
        }
    }
    blocks
}

/// Remove the global options from the help of a command: the top level lists them once.
fn without_globals(text: &str, globals: &[String]) -> String {
    let mut out = text.to_string();
    for g in globals {
        out = out.replace(&format!("{g}\n\n"), "");
        out = out.replace(&format!("{g}\n"), "");
    }
    out
}

fn walk(path: &mut Vec<String>, globals: &[String], page: &mut String) {
    let text = help(path);
    let shown = if path.is_empty() {
        text.clone()
    } else {
        without_globals(&text, globals)
    };
    let title = if path.is_empty() {
        "crewboard".to_string()
    } else {
        format!("crewboard {}", path.join(" "))
    };
    let level = "#".repeat(path.len().min(2) + 2);
    page.push_str(&format!(
        "{level} `{title}`\n\n```text\n{}\n```\n\n",
        shown.trim_end()
    ));
    let mut names = subcommands(&text);
    if path.is_empty() {
        // the extensions and the scheduler commands are not in the overview, but they are
        // documented here, after their topic
        for t in ["extensions", "scheduler"] {
            let text = topic(t);
            page.push_str(&format!(
                "## `crewboard help {t}`\n\n```text\n{}\n```\n\n",
                text.trim_end()
            ));
            names.extend(subcommands(&text));
        }
    }
    for name in names {
        path.push(name);
        walk(path, globals, page);
        path.pop();
    }
}

fn generate() -> String {
    // the top level prints an overview; the option blocks are the same on every command
    let top = help(&["status".to_string()]);
    let globals: Vec<String> = option_blocks(&top)
        .into_iter()
        .filter(|b| b.starts_with("      --config-dir") || b.starts_with("      --env-file"))
        .collect();
    assert_eq!(globals.len(), 2, "the visible global options changed");
    let mut page = String::from(
        "# CLI reference\n\n\
         This page is generated from `crewboard --help` and the help of every command. \
         Do not edit it by hand. To regenerate it, run:\n\n\
         ```sh\nCREWBOARD_UPDATE_DOCS=1 cargo test --test cli_reference\n```\n\n\
         Two options exist on every command and are shown once, under `crewboard`: \
         `--config-dir` and `--env-file`. One more works on every command and is hidden: \
         `--initiative-file`. \
         The commands that a scheduler calls (`wake`, `poll`, `tick`, `heartbeat`, `monitor`) are \
         hidden from the overview; they are listed by `crewboard help scheduler` and documented \
         below. The optional commands are listed by `crewboard help extensions`. \
         Internal commands (`acted`, `hook`) are hidden and are not listed. \
         The exit codes of every command are in the [exit codes reference](exit-codes.md).\n\n",
    );
    walk(&mut Vec::new(), &globals, &mut page);
    page
}

#[test]
fn cli_reference_is_up_to_date() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let file = root.join(PAGE);
    let want = generate();
    if std::env::var_os("CREWBOARD_UPDATE_DOCS").is_some() {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(
        have == want,
        "{PAGE} is out of date. Regenerate it with:\n    CREWBOARD_UPDATE_DOCS=1 cargo test --test cli_reference"
    );
}

#[test]
fn every_listed_command_is_in_the_page() {
    // Guards the generator itself: a parser that finds no command would pass the test above.
    let core = subcommands(&help(&[]));
    assert_eq!(core.len(), 26, "the overview shows 26 commands: {core:?}");
    assert!(core.contains(&"update".to_string()), "{core:?}");
    assert!(
        !core.contains(&"wake".to_string()) && !core.contains(&"crewboard".to_string()),
        "wake is not in the overview, and the More lines are no command: {core:?}"
    );
    let ext = subcommands(&topic("extensions"));
    assert!(
        ext.contains(&"lock".to_string()) && ext.contains(&"audit".to_string()),
        "{ext:?}"
    );
    let sched = subcommands(&topic("scheduler"));
    assert!(
        sched.contains(&"wake".to_string())
            && sched.contains(&"monitor".to_string())
            && !sched.contains(&"acted".to_string()),
        "wake is listed, the hidden acted is not: {sched:?}"
    );
    let agent = subcommands(&help(&["agent".to_string()]));
    for want in ["add", "profile", "token", "selftest"] {
        assert!(agent.contains(&want.to_string()), "{agent:?}");
    }
    // the page has a section for every command of the three lists
    let page = generate();
    for name in core.iter().chain(&ext).chain(&sched) {
        assert!(
            page.contains(&format!("\n## `crewboard {name}`\n"))
                || page.contains(&format!("\n### `crewboard {name}`\n")),
            "{name} has no section"
        );
    }
}
