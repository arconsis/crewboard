//! The calls of `crewboard wake` to the board, behind one trait. `InProcess` calls the command
//! functions with one agent context; `Subprocess` runs another binary (`BIN [--config-dir D]
//! <command> ...`, the test seam `--crewboard BIN`) and reads its output, its stderr and its exit
//! code. Both give the same values, so the flow that uses them does not know which it has.
//!
//! This is the only file of `wake` that reaches into `ops`.

use super::policy::first_stderr_line;
use crate::ctx::{AgentCtx, Global};
use crate::ops;
use crate::protocol::names::{Kind, To};
use std::path::{Path, PathBuf};

/// The result of one call: the exit code (0 to 4, or the code of the other binary) and the first
/// line of its stderr (at most 200 characters, no control characters, `(no stderr)` if none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRc {
    pub code: u8,
    pub msg: String,
}

impl StepRc {
    pub fn ok() -> StepRc {
        StepRc {
            code: 0,
            msg: String::new(),
        }
    }

    /// A result from the whole stderr text of a call.
    pub fn from_stderr(code: u8, stderr: &str) -> StepRc {
        StepRc {
            code,
            msg: first_stderr_line(stderr),
        }
    }
}

/// What `wake` asks of the board. Standard output of a call goes into `out`.
pub trait Steps {
    /// `poll [--quiet]`: 0 events, 1 none, else an error. Without `quiet` the cursor moves.
    fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc;
    /// `mine --quiet`: 0 the agent has work to do now, 1 it has none.
    fn mine_quiet(&mut self) -> StepRc;
    /// `mine`: the text of the own tickets.
    fn mine_text(&mut self, out: &mut Vec<u8>) -> StepRc;
    /// `say <ticket> --to coordinator --kind info [--override-unread] --body <body>`.
    fn say(&mut self, ticket: &str, body: &str, anyway: bool) -> StepRc;
    /// `tick`: the lines of the headless coordinator.
    fn tick(&mut self, out: &mut Vec<u8>) -> StepRc;
    /// `acted --events-file F --list`: `<repo>#<n> <kind>` per event of the file. No board read.
    fn acted_list(&mut self, events: &Path, out: &mut Vec<u8>) -> StepRc;
    /// `acted --events-file F --since T --redeliver-to U`: the events that need an act and got
    /// none, `<repo>#<n> <kind> again|twice`; those that go to the next wake are appended to `U`.
    fn acted_check(
        &mut self,
        events: &Path,
        since: &str,
        redeliver: &Path,
        out: &mut Vec<u8>,
    ) -> StepRc;
    /// `heartbeat`: one heartbeat now.
    fn heartbeat(&mut self) -> StepRc;
    /// The head of a PR: its branch and head SHA. `None` when the PR cannot be found.
    fn pr_head(&mut self, repo: &str, number: &str) -> Result<Option<(String, String)>, StepRc>;
    /// The open PRs of a repo (up to 100): number, branch, head SHA.
    fn open_prs(&mut self, repo: &str) -> Result<Vec<(String, String, String)>, StepRc>;
}

/// The refusal of `say` in a headless config dir (the first line of what the command prints).
const HEADLESS_SAY: &str = "refused [rule=headless-scope]: `say` changes the board or a ticket; the interactive coordinator does it";

/// The calls in this process. The agent context is loaded on first use and dropped after an
/// exit 3, so the next call starts from the board again.
pub struct InProcess {
    global: Global,
    ac: Option<AgentCtx>,
}

impl InProcess {
    pub fn new(global: Global) -> InProcess {
        InProcess { global, ac: None }
    }

    /// Run `f` with the context; turn its result into a [`StepRc`].
    fn with<T>(
        &mut self,
        f: impl FnOnce(&mut AgentCtx) -> Result<T, (crate::error::CbError, String)>,
    ) -> Result<T, StepRc> {
        if self.ac.is_none() {
            match AgentCtx::load(&self.global) {
                Ok(a) => self.ac = Some(a),
                Err(e) => return Err(self.failed(&e)),
            }
        }
        let Some(ac) = self.ac.as_mut() else {
            return Err(StepRc {
                code: 4,
                msg: "(no stderr)".into(),
            });
        };
        f(ac).map_err(|(e, _status)| self.failed(&e))
    }

    fn failed(&mut self, e: &crate::error::CbError) -> StepRc {
        let rc = StepRc::from_stderr(e.exit().code(), &e.to_string());
        if rc.code == 3 {
            self.ac = None;
        }
        rc
    }
}

fn plain<T>(r: crate::error::CbResult<T>) -> Result<T, (crate::error::CbError, String)> {
    r.map_err(|e| (e, String::new()))
}

impl Steps for InProcess {
    fn acted_list(&mut self, events: &Path, out: &mut Vec<u8>) -> StepRc {
        match std::fs::read_to_string(events) {
            Ok(t) => {
                out.extend_from_slice(ops::acted::list(&t).as_bytes());
                StepRc::ok()
            }
            Err(e) => StepRc::from_stderr(4, &format!("cannot read {}: {e}", events.display())),
        }
    }

    fn acted_check(
        &mut self,
        events: &Path,
        since: &str,
        redeliver: &Path,
        out: &mut Vec<u8>,
    ) -> StepRc {
        match self.with(|ac| plain(ops::acted::run(ac, events, Some(since), Some(redeliver)))) {
            Ok(text) => {
                out.extend_from_slice(text.as_bytes());
                StepRc::ok()
            }
            Err(rc) => rc,
        }
    }

    fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc {
        let opts = ops::watch::PollOpts {
            json: false,
            quiet,
            reset_cursor: false,
        };
        match self.with(|ac| ops::watch::poll(ac, opts, out)) {
            Ok(exit) => StepRc {
                code: exit.code(),
                msg: String::new(),
            },
            Err(rc) => rc,
        }
    }

    fn mine_quiet(&mut self) -> StepRc {
        match self.with(|ac| plain(ops::mine::run(ac))) {
            Ok(view) => StepRc {
                code: u8::from(!view.needs_agent()),
                msg: String::new(),
            },
            Err(rc) => rc,
        }
    }

    fn mine_text(&mut self, out: &mut Vec<u8>) -> StepRc {
        match self.with(|ac| plain(ops::mine::run(ac))) {
            Ok(view) => {
                out.extend_from_slice(view.render().as_bytes());
                StepRc {
                    code: u8::from(!view.needs_agent()),
                    msg: String::new(),
                }
            }
            Err(rc) => rc,
        }
    }

    fn say(&mut self, ticket: &str, body: &str, anyway: bool) -> StepRc {
        // `say` changes the board: a headless config dir refuses it, as the command does
        if crate::headless::in_scope(self.global.config_dir.as_deref()).unwrap_or(true) {
            return StepRc::from_stderr(2, HEADLESS_SAY);
        }
        // the secret scan and the size limit come first, as for the command
        let text = match ops::say::prepare_body(None, Some(body)) {
            Ok(t) => t,
            Err(e) => return StepRc::from_stderr(e.exit().code(), &e.to_string()),
        };
        let args = ops::say::SayArgs {
            issue: ticket,
            to: To::Coordinator,
            kind: Kind::Info,
            re: None,
            body: text,
            anyway,
        };
        match self.with(|ac| plain(ops::say::run(ac, args))) {
            Ok(_) => StepRc::ok(),
            Err(rc) => rc,
        }
    }

    fn tick(&mut self, out: &mut Vec<u8>) -> StepRc {
        // the output first, then the reported deleted notes are forgotten (`TickOut::settle`)
        match self.with(|ac| {
            let t = plain(ops::tickets::tick(ac))?;
            out.extend_from_slice(t.text.as_bytes());
            plain(t.settle(ac))
        }) {
            Ok(()) => StepRc::ok(),
            Err(rc) => rc,
        }
    }

    fn heartbeat(&mut self) -> StepRc {
        match self.with(|ac| plain(ops::lifecycle::heartbeat(ac))) {
            Ok(_) => StepRc::ok(),
            Err(rc) => rc,
        }
    }

    fn pr_head(&mut self, repo: &str, number: &str) -> Result<Option<(String, String)>, StepRc> {
        self.with(|ac| plain(ops::mine::pr_head(ac, repo, number)))
    }

    fn open_prs(&mut self, repo: &str) -> Result<Vec<(String, String, String)>, StepRc> {
        self.with(|ac| plain(ops::mine::open_prs(ac, repo)))
    }
}

/// The calls through another binary.
pub struct Subprocess {
    pub bin: String,
    pub config_dir: Option<PathBuf>,
    pub initiative: Option<String>,
}

/// The longest a heartbeat call through another binary may take. A heartbeat that hangs must not
/// keep the wake from ending.
const HEARTBEAT_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

impl Subprocess {
    /// `BIN [--config-dir D] <args> [--initiative I]`; `tail` goes after the initiative.
    fn run(&self, args: &[&str], tail: &[&str], out: &mut Vec<u8>) -> StepRc {
        self.run_within(args, tail, out, None)
    }

    /// [`Self::run`] with a limit: a call that takes longer is killed and is exit 124.
    fn run_within(
        &self,
        args: &[&str],
        tail: &[&str],
        out: &mut Vec<u8>,
        limit: Option<std::time::Duration>,
    ) -> StepRc {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(&self.bin);
        if let Some(d) = &self.config_dir {
            cmd.arg("--config-dir").arg(d);
            cmd.env("CREWBOARD_CONFIG_DIR", d);
        }
        cmd.args(args);
        if let Some(i) = &self.initiative {
            cmd.arg("--initiative").arg(i);
        }
        cmd.args(tail).stdin(Stdio::null());
        // the other binary gets the token and the settings of the env file
        if let Some(env) = crate::agent_env::current() {
            env.apply(&mut cmd);
        }
        let result = match limit {
            Some(l) => crate::spawn::output_within(&mut cmd, l),
            None => crate::spawn::output_retrying(&mut cmd),
        };
        match result {
            Ok(o) => {
                out.clear();
                out.extend_from_slice(&o.stdout);
                StepRc::from_stderr(code_of(o.status), &String::from_utf8_lossy(&o.stderr))
            }
            Err(e) => {
                let code = match e.kind() {
                    std::io::ErrorKind::NotFound => 127,
                    std::io::ErrorKind::TimedOut => 124,
                    _ => 126,
                };
                StepRc::from_stderr(code, &format!("cannot run {}: {e}", self.bin))
            }
        }
    }
}

/// The exit code of a child as a byte (see `spawn::code_of`); a code that is no byte gives 1.
pub fn code_of(st: std::process::ExitStatus) -> u8 {
    u8::try_from(crate::spawn::code_of(st)).unwrap_or(1)
}

impl Steps for Subprocess {
    fn acted_list(&mut self, events: &Path, out: &mut Vec<u8>) -> StepRc {
        let e = events.to_string_lossy();
        self.run(&["acted", "--events-file", &e, "--list"], &[], out)
    }

    fn acted_check(
        &mut self,
        events: &Path,
        since: &str,
        redeliver: &Path,
        out: &mut Vec<u8>,
    ) -> StepRc {
        let (e, r) = (events.to_string_lossy(), redeliver.to_string_lossy());
        self.run(
            &[
                "acted",
                "--events-file",
                &e,
                "--since",
                since,
                "--redeliver-to",
                &r,
            ],
            &[],
            out,
        )
    }

    fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc {
        if quiet {
            self.run(&["poll", "--quiet"], &[], out)
        } else {
            self.run(&["poll"], &[], out)
        }
    }

    fn mine_quiet(&mut self) -> StepRc {
        self.run(&["mine", "--quiet"], &[], &mut Vec::new())
    }

    fn mine_text(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.run(&["mine"], &[], out)
    }

    fn say(&mut self, ticket: &str, body: &str, anyway: bool) -> StepRc {
        let mut head = vec!["say", ticket, "--to", "coordinator", "--kind", "info"];
        if anyway {
            head.push("--override-unread");
        }
        self.run(&head, &["--body", body], &mut Vec::new())
    }

    fn tick(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.run(&["tick"], &[], out)
    }

    fn heartbeat(&mut self) -> StepRc {
        self.run_within(&["heartbeat"], &[], &mut Vec::new(), Some(HEARTBEAT_LIMIT))
    }

    // The test seam asks `gh`, as the shell did: the other binary has no PR calls.
    fn pr_head(&mut self, repo: &str, number: &str) -> Result<Option<(String, String)>, StepRc> {
        let mut out = Vec::new();
        let rc = gh(
            &[
                "pr",
                "view",
                number,
                "--repo",
                repo,
                "--json",
                "headRefName,headRefOid",
                "--jq",
                ".headRefName + \" \" + .headRefOid",
            ],
            &mut out,
        );
        if rc.code != 0 {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out);
        Ok(text
            .lines()
            .next()
            .and_then(|l| l.split_once(' '))
            .map(|(b, h)| (b.to_string(), h.to_string())))
    }

    fn open_prs(&mut self, repo: &str) -> Result<Vec<(String, String, String)>, StepRc> {
        let mut out = Vec::new();
        let rc = gh(
            &[
                "pr",
                "list",
                "--repo",
                repo,
                "--state",
                "open",
                "--limit",
                "100",
                "--json",
                "number,headRefName,headRefOid",
                "--jq",
                ".[] | \"\\(.number) \\(.headRefName) \\(.headRefOid)\"",
            ],
            &mut out,
        );
        if rc.code != 0 {
            return Err(rc);
        }
        Ok(String::from_utf8_lossy(&out)
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                Some((w.next()?.into(), w.next()?.into(), w.next()?.into()))
            })
            .collect())
    }
}

/// `gh <args>` for the test seam; its answer in `out`.
fn gh(args: &[&str], out: &mut Vec<u8>) -> StepRc {
    use std::process::{Command, Stdio};
    match crate::spawn::output_retrying(Command::new("gh").args(args).stdin(Stdio::null())) {
        Ok(o) => {
            out.extend_from_slice(&o.stdout);
            StepRc::from_stderr(code_of(o.status), &String::from_utf8_lossy(&o.stderr))
        }
        Err(e) => StepRc::from_stderr(127, &format!("cannot run gh: {e}")),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_call_that_hangs_is_killed_at_its_limit_and_is_exit_124() {
        let sub = Subprocess {
            bin: "sleep".into(),
            config_dir: None,
            initiative: None,
        };
        let t0 = Instant::now();
        let rc = sub.run_within(
            &["30"],
            &[],
            &mut Vec::new(),
            Some(Duration::from_millis(300)),
        );
        assert_eq!(rc.code, 124, "{rc:?}");
        assert!(rc.msg.contains("cannot run sleep"), "{rc:?}");
        assert!(t0.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_call_inside_its_limit_gives_its_output_and_code() {
        let sub = Subprocess {
            bin: "sh".into(),
            config_dir: None,
            initiative: None,
        };
        let mut out = Vec::new();
        let rc = sub.run_within(
            &["-c", "echo out; echo err >&2; exit 3", "x"],
            &[],
            &mut out,
            Some(Duration::from_secs(20)),
        );
        assert_eq!(rc.code, 3);
        assert_eq!(rc.msg, "err");
        assert_eq!(String::from_utf8_lossy(&out), "out\n");
    }
}
