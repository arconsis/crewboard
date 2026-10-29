//! Toolchain check of `start` (spec/agents#toolchain-check-of-start): can this agent run the build
//! tools of the repo?
//!
//! `.crewboard.toml` may list `required_tools` (commands with a version flag, each for every
//! agent or only for the agents with one of its tags) and one `smoke_cmd`. `start` runs them from the agent's current work folder with the agent's own
//! PATH, as the agent's harness would. A harness sandbox may hide the tools in the user's
//! home from the agent; this check finds that before the first ticket.
//!
//! The commands run through a [`Runner`], so the tests need no real toolchain.

use crate::config::RequiredTool;
use std::collections::BTreeSet;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One required tool may run this long.
pub const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
/// The smoke command may run this long (a build can be slow).
pub const SMOKE_TIMEOUT: Duration = Duration::from_secs(300);

/// What a command did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// It exited with a failure, or it could not start. The text is the reason.
    Failed(String),
    TimedOut,
}

/// Runs one shell command from the current work folder.
pub trait Runner {
    fn run(&self, cmd: &str, limit: Duration) -> Outcome;
}

/// The real runner: the platform shell runs `<cmd>` (`sh -c` on Unix, `cmd /C` on Windows),
/// stdin closed, the process PATH.
pub struct ShellRunner;

impl Runner for ShellRunner {
    #[allow(
        clippy::disallowed_methods,
        reason = "real elapsed time of a child process"
    )]
    fn run(&self, cmd: &str, limit: Duration) -> Outcome {
        #[cfg(windows)]
        let (shell, flag) = ("cmd", "/C");
        #[cfg(not(windows))]
        let (shell, flag) = ("sh", "-c");
        let mut sh = Command::new(shell);
        sh.arg(flag)
            .arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::agent_env::scrub_children(&mut sh);
        let spawned = crate::spawn::spawn_retrying(&mut sh);
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => return Outcome::Failed(format!("cannot start {shell}: {e}")),
        };
        let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
            return Outcome::Failed("no output pipe".into());
        };
        let t_out = std::thread::spawn(move || {
            let mut s = String::new();
            out.read_to_string(&mut s).ok(); // a read error keeps what was read so far
            s
        });
        let t_err = std::thread::spawn(move || {
            let mut s = String::new();
            err.read_to_string(&mut s).ok(); // a read error keeps what was read so far
            s
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if start.elapsed() < limit => {
                    crate::clock::pause(Duration::from_millis(10));
                }
                _ => {
                    child.kill().ok(); // the child may have ended already
                    child.wait().ok(); // the child may have ended already
                    break None;
                }
            }
        };
        // A killed shell can leave a grandchild that holds the pipe open; do not wait for it.
        let Some(status) = status else {
            return Outcome::TimedOut;
        };
        let stdout = t_out.join().unwrap_or_default();
        let stderr = t_err.join().unwrap_or_default();
        if status.success() {
            return Outcome::Ok;
        }
        let tail = |s: &str| {
            s.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .chars()
                .take(200)
                .collect::<String>()
        };
        let msg = if stderr.trim().is_empty() {
            tail(&stdout)
        } else {
            tail(&stderr)
        };
        Outcome::Failed(format!(
            "exit {}{}",
            status.code().map_or("?".to_string(), |c| c.to_string()),
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ))
    }
}

/// One failed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub cmd: String,
    pub reason: String,
}

/// The result of the check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    /// Commands that ran and passed.
    pub passed: Vec<String>,
    pub failed: Vec<Failure>,
    /// Tools that were not run, because the agent has none of their tags.
    pub skipped: Vec<Skipped>,
}

/// A tool that the agent does not need, because it has none of the tags of the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub cmd: String,
    pub tags: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }

    /// The lines that name what ran and what the tags skipped. Empty when nothing is skipped
    /// and nothing ran.
    pub fn detail(&self) -> String {
        let mut s = String::new();
        if !self.passed.is_empty() {
            let cmds: Vec<String> = self.passed.iter().map(|c| format!("`{c}`")).collect();
            s.push_str(&format!("  checked: {}\n", cmds.join(", ")));
        }
        if !self.skipped.is_empty() {
            let cmds: Vec<String> = self
                .skipped
                .iter()
                .map(|k| format!("`{}` (needs tag {})", k.cmd, k.tags.join(" or ")))
                .collect();
            s.push_str(&format!("  skipped by tag: {}\n", cmds.join(", ")));
        }
        s
    }

    /// One text for the refusal and the note: every failed command with its reason.
    pub fn failure_text(&self) -> String {
        self.failed
            .iter()
            .map(|f| format!("`{}` ({})", f.cmd, f.reason))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// Run every required tool that applies to the agent (an entry without tags applies to every
/// agent; a tagged entry applies if the agent has one of its tags), then the smoke command. No
/// keys: no check, an empty passing report. A tool that does not apply is listed in `skipped`. A failing required tool does not stop the others, so the refusal names them all;
/// the smoke command runs only if every tool passed (it would fail for the same cause).
pub fn check(
    required_tools: &[RequiredTool],
    agent_tags: &BTreeSet<String>,
    smoke_cmd: Option<&str>,
    runner: &dyn Runner,
    tool_limit: Duration,
    smoke_limit: Duration,
) -> Report {
    let mut rep = Report::default();
    let record = |rep: &mut Report, cmd: &str, o: Outcome, limit: Duration| match o {
        Outcome::Ok => rep.passed.push(cmd.to_string()),
        Outcome::Failed(r) => rep.failed.push(Failure {
            cmd: cmd.to_string(),
            reason: r,
        }),
        Outcome::TimedOut => rep.failed.push(Failure {
            cmd: cmd.to_string(),
            reason: format!("timed out after {} s", limit.as_secs()),
        }),
    };
    for t in required_tools {
        if !t.applies_to(agent_tags) {
            rep.skipped.push(Skipped {
                cmd: t.cmd.clone(),
                tags: t.tags.clone(),
            });
            continue;
        }
        let o = runner.run(&t.cmd, tool_limit);
        record(&mut rep, &t.cmd, o, tool_limit);
    }
    if rep.ok()
        && let Some(s) = smoke_cmd.filter(|s| !s.trim().is_empty())
    {
        let o = runner.run(s, smoke_limit);
        record(&mut rep, s, o, smoke_limit);
    }
    rep
}

/// The `required_tools` and `smoke_cmd` in force: the file `.crewboard.toml` on the default
/// branch of the main repo wins over the copy in the agent config (an edit takes effect
/// without `join`) when it sets at least one of the two keys. A file without both keys, or
/// one that cannot be read, leaves the copy in force.
pub fn settings_in_force(
    fresh: Option<&crate::config::BoardConfig>,
    copy: &crate::config::BoardConfig,
) -> (Vec<RequiredTool>, Option<String>) {
    let c = match fresh {
        Some(f) if !f.required_tools.is_empty() || f.smoke_cmd.is_some() => f,
        _ => copy,
    };
    (c.required_tools.clone(), c.smoke_cmd.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct Fake {
        outcomes: HashMap<String, Outcome>,
        ran: RefCell<Vec<String>>,
    }
    impl Runner for Fake {
        fn run(&self, cmd: &str, _l: Duration) -> Outcome {
            self.ran.borrow_mut().push(cmd.to_string());
            self.outcomes
                .get(cmd)
                .cloned()
                .unwrap_or(Outcome::Failed("exit 127: not found".into()))
        }
    }
    fn fake(ok: &[&str]) -> Fake {
        Fake {
            outcomes: ok.iter().map(|c| (c.to_string(), Outcome::Ok)).collect(),
            ran: RefCell::new(vec![]),
        }
    }
    fn all(v: &[&str]) -> Vec<RequiredTool> {
        v.iter().map(|x| RequiredTool::all(*x)).collect()
    }
    fn no_tags() -> BTreeSet<String> {
        BTreeSet::new()
    }
    fn tags(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    const L: Duration = Duration::from_secs(1);

    #[test]
    fn a_missing_tool_fails_and_a_present_tool_passes() {
        let f = fake(&["cargo --version"]);
        let r = check(
            &all(&["cargo --version", "node --version"]),
            &no_tags(),
            None,
            &f,
            L,
            L,
        );
        assert!(!r.ok());
        assert_eq!(r.passed, ["cargo --version"]);
        assert_eq!(r.failed.len(), 1);
        assert!(r.failure_text().contains("node --version"));
        // control: all present passes
        let r = check(&all(&["cargo --version"]), &no_tags(), None, &f, L, L);
        assert!(r.ok(), "{r:?}");
    }

    #[test]
    fn a_failing_smoke_command_fails_and_a_passing_one_passes() {
        let f = fake(&["cargo --version", "cargo check"]);
        let r = check(
            &all(&["cargo --version"]),
            &no_tags(),
            Some("cargo check"),
            &f,
            L,
            L,
        );
        assert!(r.ok());
        assert_eq!(r.passed.len(), 2);
        let r = check(
            &all(&["cargo --version"]),
            &no_tags(),
            Some("cargo build"),
            &f,
            L,
            L,
        );
        assert!(!r.ok());
        assert!(r.failure_text().contains("cargo build"));
    }

    #[test]
    fn the_smoke_command_does_not_run_after_a_failed_tool() {
        let f = fake(&[]);
        let r = check(
            &all(&["cargo --version"]),
            &no_tags(),
            Some("cargo check"),
            &f,
            L,
            L,
        );
        assert!(!r.ok());
        assert_eq!(*f.ran.borrow(), ["cargo --version"]);
    }

    #[test]
    fn no_keys_means_no_check() {
        let f = fake(&[]);
        let r = check(&[], &no_tags(), None, &f, L, L);
        assert!(r.ok() && r.passed.is_empty());
        assert!(f.ran.borrow().is_empty(), "nothing ran");
        // a blank smoke command counts as none
        assert!(check(&[], &no_tags(), Some("  "), &f, L, L).ok());
    }

    #[test]
    fn the_real_runner_sees_a_missing_tool_a_present_tool_and_a_hang() {
        let r = ShellRunner;
        // `exit N` works in sh and in cmd
        assert_eq!(r.run("exit 0", Duration::from_secs(5)), Outcome::Ok);
        assert!(matches!(
            r.run("exit 3", Duration::from_secs(5)),
            Outcome::Failed(m) if m.starts_with("exit 3")
        ));
        // sh answers 127, cmd answers 1: only the failure counts
        assert!(matches!(
            r.run("crewboard-no-such-tool-xyz --version", Duration::from_secs(5)),
            Outcome::Failed(m) if m.starts_with("exit ")
        ));
        #[cfg(unix)]
        let hang = "exec sleep 5";
        #[cfg(windows)]
        let hang = "ping -n 6 127.0.0.1 > NUL";
        assert_eq!(r.run(hang, Duration::from_millis(200)), Outcome::TimedOut);
    }

    #[cfg(unix)]
    #[test]
    fn the_real_runner_uses_the_process_path() {
        let r = ShellRunner;
        // the PATH of the process counts: an empty PATH hides `ls`
        assert!(matches!(
            r.run("PATH=/nonexistent; ls", Duration::from_secs(5)),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn the_file_on_the_repo_wins_over_the_copy() {
        let mut copy = crate::registry::tests::cfg();
        copy.required_tools = all(&["old --version"]);
        let mut fresh = copy.clone();
        fresh.required_tools = all(&["new --version"]);
        fresh.smoke_cmd = Some("make".into());
        assert_eq!(
            settings_in_force(Some(&fresh), &copy),
            (all(&["new --version"]), Some("make".into()))
        );
        assert_eq!(
            settings_in_force(None, &copy),
            (all(&["old --version"]), None)
        );
        // a file without the keys leaves the copy in force
        let mut empty = copy.clone();
        empty.required_tools.clear();
        assert_eq!(
            settings_in_force(Some(&empty), &copy),
            (all(&["old --version"]), None)
        );
    }

    fn mixed() -> Vec<RequiredTool> {
        vec![
            RequiredTool::all("git --version"),
            RequiredTool::tagged("cargo --version", &["rust"]),
            RequiredTool::tagged("pytest --version", &["python", "docs"]),
        ]
    }

    #[test]
    fn a_tagged_tool_is_skipped_for_an_agent_without_the_tag() {
        let f = fake(&["git --version"]);
        let r = check(&mixed(), &no_tags(), None, &f, L, L);
        assert!(r.ok(), "{r:?}");
        assert_eq!(*f.ran.borrow(), ["git --version"]);
        assert_eq!(r.passed, ["git --version"]);
        assert_eq!(r.skipped.len(), 2);
        // another tag does not unlock it
        let f = fake(&["git --version"]);
        let r = check(&mixed(), &tags(&["web"]), None, &f, L, L);
        assert!(r.ok(), "{r:?}");
        assert_eq!(r.skipped.len(), 2);
        let d = r.detail();
        assert!(d.contains("checked: `git --version`"), "{d}");
        assert!(
            d.contains("`cargo --version` (needs tag rust)")
                && d.contains("`pytest --version` (needs tag python or docs)"),
            "{d}"
        );
    }

    #[test]
    fn a_tagged_tool_is_checked_for_an_agent_with_the_tag_and_can_fail() {
        // control of the filter above: the same tool fails when the agent has the tag
        let f = fake(&["git --version"]);
        let r = check(&mixed(), &tags(&["rust"]), None, &f, L, L);
        assert!(!r.ok());
        assert_eq!(r.failed.len(), 1);
        assert!(r.failure_text().contains("cargo --version"));
        assert_eq!(r.skipped.len(), 1, "pytest still skipped");
        // present: passes; any one of several tags is enough
        let f = fake(&["git --version", "cargo --version", "pytest --version"]);
        let r = check(&mixed(), &tags(&["rust", "docs"]), None, &f, L, L);
        assert!(r.ok(), "{r:?}");
        assert_eq!(r.passed.len(), 3);
        assert!(r.skipped.is_empty());
    }

    #[test]
    fn the_smoke_command_runs_for_every_agent() {
        let f = fake(&["git --version", "smoke"]);
        let r = check(&mixed(), &no_tags(), Some("smoke"), &f, L, L);
        assert_eq!(r.passed, ["git --version", "smoke"]);
    }
}
