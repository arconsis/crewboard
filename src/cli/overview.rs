//! The overview that `crewboard`, `crewboard -h`, `crewboard help` and `crewboard --help` print,
//! and the two help topics `crewboard help extensions` and `crewboard help scheduler`.
//!
//! Clap has no groups of subcommands, so `GROUPS` maps a heading to command names. The text of
//! each line is the `about` of the command in the clap tree: there is one source of truth.
//! Every command keeps its full text in `long_about`, which `crewboard <command> --help` prints.
//!
//! The overview shows the 26 commands of the core. A command that only a scheduler or a wrapper
//! script calls is hidden from it and never renamed (`SCHEDULER`); an optional command is on the
//! topic `extensions` (`EXTENSIONS`).

use clap::Command;

/// Spaces between the longest command name and its text.
const GAP: usize = 2;
/// Width of the text that the overview wraps (long help only).
const WRAP: usize = 78;

/// Heading and command names of the core, in the order of the overview. A basic crew needs only
/// these. Every visible command is in exactly one group of `GROUPS` or `EXTENSIONS`; a test
/// checks that.
pub const GROUPS: &[(&str, &[&str])] = &[
    (
        "Set up (owner)",
        &[
            "init", "agent", "approve", "doctor", "status", "skill", "update",
        ],
    ),
    (
        "Agent: life cycle",
        &["join", "start", "resume", "pause", "stop", "watch"],
    ),
    (
        "Dev agent: tickets",
        &["mine", "take", "done", "block", "ask", "say"],
    ),
    (
        "Coordinator: tickets",
        &[
            "ticket", "assign", "unblock", "reassign", "verify", "merge", "close",
        ],
    ),
];

/// Optional commands, by topic. The overview names most of them in one line (`EXTENSIONS_LINE`);
/// `crewboard help extensions` lists the groups with the `about` line of each command. No basic
/// crew needs one of them.
pub const EXTENSIONS: &[(&str, &[&str])] = &[
    ("Owner tools", &["audit", "move"]),
    ("Reports and upkeep", &["report-status", "prune"]),
    ("Heavy jobs on a shared host", &["lock", "repeat"]),
];

/// Commands that a scheduler, a wrapper script or a Monitor tool calls. They are hidden from the
/// overview and never renamed: a rename would break the wrappers of running crews. `crewboard help
/// scheduler` lists them. `monitor` is the same command as `watch --loop`.
pub const SCHEDULER: &[(&str, &[&str])] = &[
    ("Wake an agent from a scheduler", &["wake", "poll"]),
    ("Headless coordinator", &["tick"]),
    ("Liveness", &["heartbeat"]),
    ("Monitor tool", &["monitor"]),
];

/// The commands that the overview names for `help extensions` (a subset of `EXTENSIONS`).
const EXTENSIONS_LINE: &str = "audit, move, report-status, prune, lock, repeat";
/// The commands that the overview names for `help scheduler`.
const SCHEDULER_LINE: &str = "wake, poll, tick, heartbeat (called by schedulers)";

/// The options of every command, as the short overview prints them: the name and the text.
/// `--initiative-file` works but is hidden.
const OPTIONS: &[(&str, &str)] = &[
    (
        "--config-dir <DIR>",
        "Config, keys and cache dir (default ~/.config/crewboard)",
    ),
    (
        "--env-file <FILE>",
        "Env file with the agent token (mode 600)",
    ),
    ("-V, --version", "Print the version"),
];

/// All tables, for the width of the name column.
fn all_tables() -> impl Iterator<Item = &'static (&'static str, &'static [&'static str])> {
    GROUPS.iter().chain(EXTENSIONS).chain(SCHEDULER)
}

/// Render the overview from the clap tree. `long` gives the long help of the global options.
pub fn render(cmd: &Command, long: bool) -> String {
    let name = cmd.get_name();
    let mut out = String::new();
    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();
    out.push_str(&format!(
        "{name}: {about}.\n\nUsage: {name} <command> [options]\n"
    ));
    push_groups(&mut out, cmd, GROUPS, width_of(all_tables()));
    out.push_str("\nMore\n");
    let topics = [
        ("extensions", EXTENSIONS_LINE),
        ("scheduler", SCHEDULER_LINE),
    ];
    let label = |t: &str| format!("{name} help {t}");
    let pad = topics
        .iter()
        .map(|(t, _)| label(t).len())
        .max()
        .unwrap_or(0)
        + 3;
    for (t, line) in topics {
        out.push_str(&format!("  {:<pad$}{line}\n", label(t)));
    }
    out.push_str(&format!(
        "New crew (owner): `{name} agent token`, `{name} init`,\n\
         then for each agent `{name} agent add` on its host and `{name} approve`.\n\
         First try: https://github.com/arconsis/crewboard/blob/main/docs/quick-try.md\n\
         New agent: `{name} agent add`, then `{name} doctor --setup`.\n\
         Run `{name} <command> --help` for details, `{name} --help` for options.\n"
    ));
    out.push_str("\nOptions for all commands:\n");
    if long {
        push_long_options(&mut out, cmd);
    } else {
        let w = OPTIONS.iter().map(|(o, _)| o.len()).max().unwrap_or(0) + GAP;
        for (opt, text) in OPTIONS {
            out.push_str(&format!("  {opt:<w$}{text}\n"));
        }
    }
    out
}

/// The options of the long overview: the visible global options of the tree with their full
/// text, then `--version`.
fn push_long_options(out: &mut String, cmd: &Command) {
    for a in cmd
        .get_arguments()
        .filter(|a| a.is_global_set() && !a.is_hide_set())
    {
        let long_name = a.get_long().unwrap_or_default();
        let value = a
            .get_value_names()
            .and_then(|v| v.first())
            .map(|v| format!(" <{v}>"))
            .unwrap_or_default();
        out.push_str(&format!("  --{long_name}{value}\n"));
        let help = a
            .get_long_help()
            .or(a.get_help())
            .map(|h| h.to_string())
            .unwrap_or_default();
        for line in wrap(&help, WRAP - 6) {
            out.push_str(&format!("      {line}\n"));
        }
    }
    out.push_str("  -V, --version\n      Print the version.\n");
}

/// Width of the name column: the longest command name of the groups plus `GAP`.
fn width_of<'a>(groups: impl Iterator<Item = &'a (&'a str, &'a [&'a str])>) -> usize {
    groups
        .flat_map(|(_, names)| names.iter())
        .map(|n| n.len())
        .max()
        .unwrap_or(0)
        + GAP
}

/// Add each group: a blank line, the heading, and one line per command with its `about`.
fn push_groups(out: &mut String, cmd: &Command, groups: &[(&str, &[&str])], width: usize) {
    for (heading, names) in groups {
        out.push_str(&format!("\n{heading}\n"));
        for n in *names {
            let about = cmd
                .find_subcommand(n)
                .and_then(|c| c.get_about())
                .map(|a| a.to_string())
                .unwrap_or_default();
            out.push_str(&format!("  {n:<w$}{about}\n", w = width));
        }
    }
}

/// The text of one help topic: a first line, the groups with the `about` of each command, and a
/// last line that points to the guide.
fn render_topic(cmd: &Command, first: &str, groups: &[(&str, &[&str])], guide: &str) -> String {
    let name = cmd.get_name();
    let mut out = format!("{first}\n");
    push_groups(&mut out, cmd, groups, width_of(all_tables()));
    out.push_str(&format!(
        "\nRun `{name} <command> --help` for details.\nGuide: https://github.com/arconsis/crewboard/blob/main/{guide}\n"
    ));
    out
}

/// The text of `crewboard help extensions`: the optional commands, by topic, with their `about`.
pub fn render_extensions(cmd: &Command) -> String {
    let name = cmd.get_name();
    render_topic(
        cmd,
        &format!("{name} extensions: optional commands. A basic crew needs none of them."),
        EXTENSIONS,
        "docs/guides/extensions.md",
    )
}

/// The text of `crewboard help scheduler`: the commands that a scheduler, a wrapper script or a
/// Monitor tool calls, with their `about`. A person rarely types them.
pub fn render_scheduler(cmd: &Command) -> String {
    let name = cmd.get_name();
    render_topic(
        cmd,
        &format!(
            "{name} scheduler: commands that a scheduler or a wrapper script calls.\n\
             They are hidden from the overview and keep their names for ever."
        ),
        SCHEDULER,
        "docs/guides/agents.md",
    )
}

/// Break `text` into lines of at most `width` columns, at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Does this command line ask for the overview? `args` are the arguments after the program
/// name. Returns `Some(long)` for `crewboard`, `crewboard -h`, `crewboard --help` and
/// `crewboard help` (also with global options in front), else `None`. `--help` is long.
pub fn requested(args: &[String]) -> Option<bool> {
    let mut it = args.iter();
    let mut long = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config-dir" | "--env-file" => {
                it.next();
            }
            "-h" => {}
            "--help" => long = true,
            "help" => return it.next().is_none().then_some(long),
            _ if a.starts_with("--config-dir=") || a.starts_with("--env-file=") => {}
            _ => return None,
        }
    }
    Some(long)
}

/// The topic of `crewboard help <topic>` (also with global options in front): the one word after
/// `help`, when the command line has nothing else. There is no command of the names `extensions`
/// and `scheduler`, so a topic cannot clash with `help <command>`.
fn help_topic(args: &[String]) -> Option<&str> {
    let mut it = args.iter();
    let mut words: Vec<&str> = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config-dir" | "--env-file" => {
                it.next();
            }
            _ if a.starts_with("--config-dir=") || a.starts_with("--env-file=") => {}
            w => words.push(w),
        }
    }
    match words[..] {
        ["help", topic] => Some(topic),
        _ => None,
    }
}

/// Does this command line ask for `crewboard help extensions`?
pub fn extensions_requested(args: &[String]) -> bool {
    help_topic(args) == Some("extensions")
}

/// Does this command line ask for `crewboard help scheduler`?
pub fn scheduler_requested(args: &[String]) -> bool {
    help_topic(args) == Some("scheduler")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn cmd() -> Command {
        crate::cli::Cli::command()
    }

    fn visible(cmd: &Command) -> Vec<String> {
        cmd.get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
            .map(|c| c.get_name().to_string())
            .collect()
    }

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// The overview of the proposal, as the short form prints it. One screen, 80 columns.
    const WANT: &str = "\
crewboard: AI agents work as a crew through a GitHub Project board.

Usage: crewboard <command> [options]

Set up (owner)
  init           Create the board, labels and authority file
  agent          Set up and run one agent on this host
  approve        Approve an agent that joined
  doctor         Check this host, an agent, the board or a repo move
  status         Show the board, the agents and tickets that need care
  skill          Install the crewboard skill
  update         Update or downgrade this binary

Agent: life cycle
  join           Ask to join the crew
  start          Check approval and setup, set the agent active
  resume         Continue after a restart or a pause
  pause          Finish the current step and take nothing new
  stop           Drain open tickets, then leave the crew
  watch          Wait for events and print them (--loop: keep waiting)

Dev agent: tickets
  mine           List your tickets and what each one needs
  take           Take a Ready ticket and start work
  done           Report a finished ticket and move it to Review
  block          Move your ticket to Blocked
  ask            Ask the owner a question
  say            Post a signed message

Coordinator: tickets
  ticket         Create, adopt or split a ticket
  assign         Give a ticket to an agent, or move a queued ticket
  unblock        Answer a block so the agent can resume
  reassign       Take a ticket from its agent
  verify         Check evidence and CI of a ticket in Review
  merge          Merge the PR of a verified ticket
  close          Close or drop a ticket (Done and Dropped are final)

More
  crewboard help extensions   audit, move, report-status, prune, lock, repeat
  crewboard help scheduler    wake, poll, tick, heartbeat (called by schedulers)
New crew (owner): `crewboard agent token`, `crewboard init`,
then for each agent `crewboard agent add` on its host and `crewboard approve`.
First try: https://github.com/arconsis/crewboard/blob/main/docs/quick-try.md
New agent: `crewboard agent add`, then `crewboard doctor --setup`.
Run `crewboard <command> --help` for details, `crewboard --help` for options.

Options for all commands:
  --config-dir <DIR>  Config, keys and cache dir (default ~/.config/crewboard)
  --env-file <FILE>   Env file with the agent token (mode 600)
  -V, --version       Print the version
";

    #[test]
    fn the_overview_is_the_text_of_the_proposal() {
        assert_eq!(render(&cmd(), false), WANT);
    }

    #[test]
    fn the_overview_shows_26_commands_in_4_groups() {
        assert_eq!(GROUPS.len(), 4);
        assert_eq!(GROUPS.iter().map(|(_, n)| n.len()).sum::<usize>(), 26);
    }

    #[test]
    fn every_visible_command_is_in_exactly_one_group_and_no_unknown_name() {
        let mut listed: Vec<&str> = GROUPS
            .iter()
            .chain(EXTENSIONS)
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        listed.sort_unstable();
        let mut dup = listed.clone();
        dup.dedup();
        assert_eq!(dup, listed, "a command is in two groups");
        let mut want = visible(&cmd());
        want.sort();
        assert_eq!(listed, want, "the group table and the clap tree differ");
    }

    #[test]
    fn the_scheduler_commands_are_hidden_from_the_overview_and_listed_once_on_their_topic() {
        let c = cmd();
        let mut names: Vec<&str> = SCHEDULER
            .iter()
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        names.sort_unstable();
        let mut dup = names.clone();
        dup.dedup();
        assert_eq!(dup, names, "a command is twice on the scheduler topic");
        for n in &names {
            assert!(c.find_subcommand(n).unwrap().is_hide_set(), "{n} is shown");
            assert!(
                !GROUPS.iter().chain(EXTENSIONS).any(|(_, g)| g.contains(n)),
                "{n} is in the overview or the extensions"
            );
        }
        // every machine-called name is there, and none is renamed
        for want in ["wake", "poll", "tick", "heartbeat", "monitor"] {
            assert!(names.contains(&want), "{want}");
        }
        // the one line of the overview names the same commands
        for n in ["wake", "poll", "tick", "heartbeat"] {
            assert!(SCHEDULER_LINE.contains(n), "{n}");
        }
    }

    #[test]
    fn the_extensions_line_names_every_extension() {
        let on_line: Vec<&str> = EXTENSIONS_LINE.split(", ").collect();
        let mut on_page: Vec<&str> = EXTENSIONS
            .iter()
            .flat_map(|(_, n)| n.iter().copied())
            .collect();
        for n in &on_line {
            assert!(on_page.contains(n), "{n} is on the line, not on the page");
        }
        on_page.retain(|n| !on_line.contains(n));
        assert!(
            on_page.is_empty(),
            "{on_page:?} are on the page but not on the line"
        );
    }

    fn all_subcommands(c: &Command, path: &str, out: &mut Vec<(String, String)>) {
        for s in c.get_subcommands().filter(|s| !s.is_hide_set()) {
            if s.get_name() == "help" {
                continue;
            }
            let p = format!("{path} {}", s.get_name()).trim().to_string();
            out.push((
                p.clone(),
                s.get_about().map(|a| a.to_string()).unwrap_or_default(),
            ));
            all_subcommands(s, &p, out);
        }
    }

    #[test]
    fn every_about_is_one_short_line() {
        let mut all = Vec::new();
        all_subcommands(&cmd(), "", &mut all);
        assert!(all.len() > 40, "{}", all.len());
        for (path, about) in all {
            let limit = if path.contains(' ') { 80 } else { 60 };
            assert!(!about.is_empty(), "{path} has no about");
            assert!(!about.contains('\n'), "{path}: {about}");
            assert!(
                about.chars().count() <= limit,
                "{path} ({}): {about}",
                about.len()
            );
        }
    }

    #[test]
    fn overview_fits_one_screen() {
        for long in [false, true] {
            let text = render(&cmd(), long);
            for line in text.lines() {
                assert!(line.chars().count() <= 80, "{} columns: {line}", line.len());
            }
            if !long {
                assert!(
                    text.lines().count() <= 53,
                    "{} lines\n{text}",
                    text.lines().count()
                );
            }
        }
        let text = render(&cmd(), false);
        assert!(text.contains("crewboard doctor --setup"), "{text}");
        assert!(
            text.contains("  update         Update or downgrade"),
            "{text}"
        );
    }

    #[test]
    fn every_command_line_has_at_least_two_spaces_before_its_text() {
        let text = render(&cmd(), false);
        let names: Vec<&str> = GROUPS.iter().flat_map(|(_, n)| n.iter().copied()).collect();
        let mut seen = 0;
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("  ") else {
                continue;
            };
            let name = rest.split(' ').next().unwrap();
            if names.contains(&name) {
                assert!(rest[name.len()..].starts_with("  "), "{line}");
                seen += 1;
            }
        }
        assert_eq!(seen, names.len());
    }

    #[test]
    fn overview_lists_the_core_with_text_and_names_the_two_topics_in_one_line_each() {
        let text = render(&cmd(), false);
        for (_, names) in GROUPS {
            for n in *names {
                assert!(text.contains(&format!("\n  {n} ")), "core {n} missing");
            }
        }
        // an extension or a scheduler command has no line of its own in the overview
        for (_, names) in EXTENSIONS.iter().chain(SCHEDULER) {
            for n in *names {
                assert!(
                    !text.contains(&format!("\n  {n} ")),
                    "{n} is in the core part"
                );
            }
        }
        let more = text.split_once("\nMore\n").unwrap().1;
        let more: Vec<&str> = more.lines().take(2).collect();
        assert!(more[0].ends_with(EXTENSIONS_LINE), "{more:?}");
        assert!(more[1].ends_with(SCHEDULER_LINE), "{more:?}");
    }

    #[test]
    fn help_extensions_lists_every_extension_with_its_about_in_80_columns() {
        let text = render_extensions(&cmd());
        let c = cmd();
        for (heading, names) in EXTENSIONS {
            assert!(text.contains(&format!("\n{heading}\n")), "{heading}");
            for n in *names {
                let about = c
                    .find_subcommand(n)
                    .unwrap()
                    .get_about()
                    .unwrap()
                    .to_string();
                assert!(
                    text.contains(&format!("  {n}")) && text.contains(&about),
                    "{n}"
                );
            }
        }
        assert!(text.contains("docs/guides/extensions.md"));
        assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
    }

    #[test]
    fn help_scheduler_lists_every_scheduler_command_with_its_about_in_80_columns() {
        let text = render_scheduler(&cmd());
        let c = cmd();
        for (heading, names) in SCHEDULER {
            assert!(text.contains(&format!("\n{heading}\n")), "{heading}");
            for n in *names {
                let about = c
                    .find_subcommand(n)
                    .unwrap()
                    .get_about()
                    .unwrap()
                    .to_string();
                assert!(
                    text.contains(&format!("  {n}")) && text.contains(&about),
                    "{n}"
                );
            }
        }
        assert!(text.starts_with("crewboard scheduler: "), "{text}");
        assert!(text.contains("docs/guides/agents.md"));
        assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
    }

    #[test]
    fn help_scheduler_is_asked_for_only_by_those_two_words() {
        assert!(scheduler_requested(&args(&["help", "scheduler"])));
        assert!(scheduler_requested(&args(&[
            "--env-file",
            "x",
            "help",
            "scheduler"
        ])));
        assert!(!scheduler_requested(&args(&["help"])));
        assert!(!scheduler_requested(&args(&["help", "extensions"])));
        assert!(!scheduler_requested(&args(&["scheduler"])));
        assert!(!scheduler_requested(&args(&["help", "scheduler", "x"])));
        assert!(!extensions_requested(&args(&["help", "scheduler"])));
    }

    #[test]
    fn help_extensions_is_asked_for_only_by_those_two_words() {
        assert!(extensions_requested(&args(&["help", "extensions"])));
        assert!(extensions_requested(&args(&[
            "--config-dir",
            "x",
            "help",
            "extensions"
        ])));
        assert!(!extensions_requested(&args(&["help"])));
        assert!(!extensions_requested(&args(&["help", "update"])));
        assert!(!extensions_requested(&args(&["extensions"])));
        assert!(!extensions_requested(&args(&["help", "extensions", "x"])));
    }

    #[test]
    fn long_overview_has_the_full_text_of_the_global_options() {
        let text = render(&cmd(), true);
        assert!(text.contains("--config-dir <CONFIG_DIR>"), "{text}");
        assert!(text.contains("--env-file <ENV_FILE>"), "{text}");
        assert!(text.contains("agent.env"), "{text}");
        // the hidden global option works, and the overview does not list it
        assert!(!text.contains("--initiative-file"), "{text}");
    }

    #[test]
    fn overview_is_asked_for_by_no_arguments_h_help_and_the_help_word() {
        assert_eq!(requested(&args(&[])), Some(false));
        assert_eq!(requested(&args(&["-h"])), Some(false));
        assert_eq!(requested(&args(&["help"])), Some(false));
        assert_eq!(requested(&args(&["--help"])), Some(true));
        assert_eq!(requested(&args(&["--config-dir", "x", "-h"])), Some(false));
        assert_eq!(requested(&args(&["help", "init"])), None);
        assert_eq!(requested(&args(&["init", "--help"])), None);
        assert_eq!(requested(&args(&["lock", "-h"])), None);
    }

    fn long_help(name: &str) -> String {
        let mut c = cmd();
        c.build();
        c.find_subcommand(name)
            .unwrap()
            .clone()
            .render_long_help()
            .to_string()
    }

    #[test]
    fn long_help_of_a_command_keeps_the_moved_details() {
        let update = long_help("update");
        for want in [
            "Exit codes: 0",
            "aarch64-apple-darwin",
            "GH_TOKEN=$(gh auth token) crewboard update",
        ] {
            assert!(update.contains(want), "update lost {want}");
        }
        let lock = long_help("lock");
        for want in [
            "64 (usage)",
            "75 (held)",
            "77 (held by your own parent process)",
        ] {
            assert!(lock.contains(want), "lock lost {want}");
        }
        let mine = long_help("mine");
        for want in [
            "Exit 0: a ticket needs you now",
            "Exit 1: nothing to do now",
        ] {
            assert!(mine.contains(want), "mine lost {want}");
        }
        assert!(long_help("monitor").contains("crewboard-watch: exit <code>"));
    }
}
