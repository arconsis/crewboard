//! `crewboard repeat` (old name `gate run`): run a Gate command N times in a row and keep honest evidence.
//!
//! A Gate such as "5 clean loaded runs in a row" is not proved by a loop that appends every run
//! to ONE log and counts runs by grepping the log: a result line of an EARLIER run would count and
//! a red run could count as clean. This command does it the safe way:
//! - every run writes its OWN log file (`run-<n>.log`, removed first);
//! - a run is judged ONLY by its own run: the exit code of the command, and with `--result-re`
//!   also its own result lines (a missing line is red);
//! - the head commit is read before and after every run; a run is not counted if it differs from
//!   the commit of the first run (exit 2);
//! - the loop stops at the first red run (no "best N of M");
//! - at the end it prints the raw result lines of every run, ready to paste into a PR.
//!
//! The pure parts (the verdict, the evidence text) are in `crate::repeat`. Exit codes: 0 all runs
//! green on one commit; 1 a red run; 2 the commit changed (or `--lock` on a build without Unix
//! process control); 3 CI did not finish within `--ci-max-wait`; 4 setup error; 64 usage; 143 a
//! TERM, INT or HUP stopped the gate (the command got it too, the lock is released, the evidence
//! so far is printed).

use crate::base::ignore::{best_effort_cleanup, best_effort_output};
use crate::clock::parse_secs;
use crate::repeat::evidence::{Block, display_cmd, render};
use crate::repeat::verdict::{Patterns, Run, Verdict, judge, result_lines};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The refusal of a build without Unix process control (the lock needs it).
#[cfg(not(unix))]
const UNIX_ONLY: &str = "refused [rule=unix-only]: crewboard repeat --lock needs a Unix host (macOS or Linux); this build has no loop on Windows";

/// The options as typed. Values stay text until `run` has checked them (a usage error is 64).
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub runs: Option<String>,
    pub lock: Option<String>,
    pub owner: Option<String>,
    pub lock_wait: String,
    pub log_dir: Option<PathBuf>,
    pub repo: Option<PathBuf>,
    pub result_re: Option<String>,
    pub fail_re: Option<String>,
    pub no_ci_check: bool,
    pub ci_poll: String,
    pub ci_max_wait: String,
    pub cmd: Vec<String>,
}

/// The options after the checks.
struct Plan {
    runs: usize,
    lock: Option<String>,
    #[cfg(unix)]
    owner: String,
    #[cfg(unix)]
    lock_wait: u64,
    repo: PathBuf,
    patterns: Patterns,
    ci_check: bool,
    ci_poll: Duration,
    ci_max_wait: Duration,
    cmd: Vec<String>,
}

fn say(text: &str) {
    best_effort_output(writeln!(std::io::stderr(), "gate-loop: {text}"));
}

/// A seconds value: `10`, `0.5`, `90s`, `30m`, `2h`.
fn duration(v: &str) -> Option<Duration> {
    if let Some(s) = parse_secs(v) {
        return Some(Duration::from_secs(s));
    }
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    v.parse::<f64>()
        .ok()
        .and_then(|x| Duration::try_from_secs_f64(x).ok())
}

fn check(o: &Options) -> Result<Plan, String> {
    let runs_text = o.runs.as_deref().unwrap_or("");
    let runs = match runs_text.parse::<usize>() {
        Ok(n) if n >= 1 && runs_text.bytes().all(|b| b.is_ascii_digit()) => n,
        _ => {
            return Err(format!(
                "--times needs a whole number >= 1, got '{runs_text}'"
            ));
        }
    };
    if o.cmd.is_empty() {
        return Err("a command is required after -- (for example: -- cargo test)".to_string());
    }
    let bad =
        |opt: &str, v: &str| format!("{opt} needs a duration (90, 90s, 30m or 2h), got '{v}'");
    // parsed on every host so a bad value gives the same error text; only `--lock` uses it
    let lock_wait = parse_secs(&o.lock_wait).ok_or_else(|| bad("--lock-wait", &o.lock_wait))?;
    #[cfg(not(unix))]
    let _ = lock_wait;
    // at least 100 ms: a poll of 0 would spin
    let ci_poll = duration(&o.ci_poll)
        .ok_or_else(|| bad("--ci-poll", &o.ci_poll))?
        .max(Duration::from_millis(100));
    let ci_max_wait =
        duration(&o.ci_max_wait).ok_or_else(|| bad("--ci-max-wait", &o.ci_max_wait))?;
    let patterns = Patterns::compile(o.result_re.as_deref(), o.fail_re.as_deref())?;
    #[cfg(unix)]
    let owner = o
        .owner
        .clone()
        .or_else(|| {
            std::env::var("CREWBOARD_AGENT")
                .ok()
                .filter(|v| !v.is_empty())
        })
        .unwrap_or_else(|| "gate-loop".to_string());
    Ok(Plan {
        runs,
        lock: o.lock.clone(),
        #[cfg(unix)]
        owner,
        #[cfg(unix)]
        lock_wait,
        repo: o.repo.clone().unwrap_or_else(|| PathBuf::from(".")),
        patterns,
        ci_check: !o.no_ci_check,
        ci_poll,
        ci_max_wait,
        cmd: o.cmd.clone(),
    })
}

/// The head commit of `repo`, or `None` when it is not a git checkout with a commit.
fn head_sha(repo: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// The directory for the logs: the one asked for, or a new one in the temp dir.
fn make_log_dir(given: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(d) = given {
        return std::fs::create_dir_all(d)
            .map(|()| d.to_path_buf())
            .map_err(|_| format!("cannot create {}", d.display()));
    }
    let base = std::env::temp_dir();
    let nanos = crate::clock::now_nanos();
    for attempt in 0..100u32 {
        let d = base.join(format!(
            "gate-loop.{}.{}",
            std::process::id(),
            nanos.wrapping_add(attempt)
        ));
        match private_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => break,
        }
    }
    Err("cannot create a log directory".to_string())
}

fn private_dir(d: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut b = std::fs::DirBuilder::new();
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(d)
    }
    #[cfg(not(unix))]
    std::fs::DirBuilder::new().create(d)
}

/// Wait for the command. A TERM, INT or HUP that reaches the gate is passed on to the command (it
/// runs in its own process group, so nothing else would reach it); the gate then stops.
#[cfg(unix)]
fn wait_child(child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
    let mut forwarded = false;
    loop {
        if let Some(st) = child.try_wait()? {
            return Ok(st);
        }
        if !forwarded && let Some(sig) = crate::lock::sys::stop_requested() {
            crate::lock::sys::signal_group(child.id(), sig);
            forwarded = true;
        }
        crate::clock::pause(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn wait_child(child: &mut std::process::Child) -> std::io::Result<std::process::ExitStatus> {
    child.wait()
}

/// Run the command of one run without the lock: stdout and stderr go to `log`. A command that
/// cannot start gives 127 (not found) or 126, and its error text is the log.
fn run_plain(cmd: &[String], repo: &Path, log: &Path) -> i32 {
    let started = std::fs::File::create(log)
        .and_then(|out| Ok((out.try_clone()?, out)))
        .and_then(|(err, out)| {
            let mut c = Command::new(&cmd[0]);
            c.args(&cmd[1..]).current_dir(repo).stdout(out).stderr(err);
            #[cfg(unix)]
            std::os::unix::process::CommandExt::process_group(&mut c, 0);
            crate::spawn::spawn_retrying(&mut c)
        })
        .and_then(|mut child| wait_child(&mut child));
    match started {
        Ok(st) => crate::spawn::code_of(st),
        Err(e) => {
            let (rc, why) = match e.kind() {
                std::io::ErrorKind::NotFound => (127, "command not found".to_string()),
                std::io::ErrorKind::PermissionDenied => (126, "Permission denied".to_string()),
                _ => (126, e.to_string()),
            };
            best_effort_output(std::fs::write(log, format!("{}: {why}\n", cmd[0])));
            rc
        }
    }
}

/// Run the command of one run under the lock of `dir` (in this process; the child gets the grant).
#[cfg(unix)]
fn run_locked(config_dir: Option<&Path>, p: &Plan, dir: &str, n: usize, log: &Path) -> i32 {
    let mut args: Vec<String> = [
        "run",
        dir,
        "--holder",
        &p.owner,
        "--reason",
        &format!("gate-loop run {n} of {}", p.runs),
        "--wait",
        &p.lock_wait.to_string(),
        "--poll",
        "0.2",
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(p.cmd.iter().cloned());
    super::lock_cmd::run_in(config_dir, &args, &p.repo, log)
}

#[cfg(not(unix))]
fn run_locked(_: Option<&Path>, _: &Plan, _: &str, _: usize, _: &Path) -> i32 {
    2
}

/// What the loop knows while it runs.
struct Loop<'a> {
    plan: &'a Plan,
    config_dir: Option<&'a Path>,
    log_dir: PathBuf,
    first_sha: String,
    block: Block,
}

impl Loop<'_> {
    /// The report, and the exit code.
    fn finish(&self, code: i32, verdict: &str) -> i32 {
        let text = render(
            &display_cmd(&self.plan.cmd),
            &self.block,
            verdict,
            &self.log_dir.display().to_string(),
        );
        let mut out = std::io::stdout().lock();
        best_effort_output(out.write_all(text.as_bytes()));
        best_effort_output(out.flush());
        code
    }

    /// A TERM, INT or HUP reached the gate: end with the evidence so far and exit 143. `where_`
    /// says when ("before run 2 of 3", "during run 2 of 3").
    fn stopped(&self, where_: &str) -> Option<i32> {
        #[cfg(unix)]
        if let Some(sig) = crate::lock::sys::stop_requested() {
            return Some(self.finish(143, &format!("STOPPED by signal {sig} {where_}")));
        }
        let _ = where_;
        None
    }

    /// Sleep `d`, in short steps that end at a signal.
    #[allow(
        clippy::disallowed_methods,
        reason = "real elapsed time of a wait that ends at a signal; no Clock is passed here"
    )]
    fn pause(d: Duration) {
        let end = Instant::now() + d;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            #[cfg(unix)]
            if crate::lock::sys::stop_requested().is_some() {
                return;
            }
            crate::clock::pause(left.min(Duration::from_millis(100)));
        }
    }

    /// Before run `n`: wait while CI runs or waits on this host. `Some(code)` ends the gate.
    #[allow(
        clippy::disallowed_methods,
        reason = "real elapsed time of a wait that ends at a signal; no Clock is passed here"
    )]
    fn wait_for_ci(&mut self, n: usize) -> Option<i32> {
        let p = self.plan;
        let dir = p
            .lock
            .as_ref()
            .map_or_else(|| self.log_dir.join("no-lock"), PathBuf::from);
        let mut paused_from: Option<Instant> = None;
        while let Some(line) = super::lock_cmd::ci_present(&dir) {
            let since = *paused_from.get_or_insert_with(|| {
                say(&format!("run {n} paused: {line}"));
                Instant::now()
            });
            let waited = since.elapsed();
            if waited >= p.ci_max_wait {
                self.block.ci_timeout(n, waited.as_secs(), &line);
                return Some(self.finish(
                    3,
                    &format!(
                        "CI did not finish within {} s before run {n} of {}",
                        p.ci_max_wait.as_secs(),
                        p.runs
                    ),
                ));
            }
            Self::pause(p.ci_poll);
            if let Some(code) = self.stopped(&format!("before run {n} of {}", p.runs)) {
                return Some(code);
            }
        }
        if let Some(since) = paused_from {
            self.block.ci_pause(n, since.elapsed().as_secs());
            say(&format!("run {n} continues: CI is gone"));
        }
        None
    }

    /// Run number `n`. `Some(code)` ends the gate.
    fn one_run(&mut self, n: usize) -> Option<i32> {
        let p = self.plan;
        if let Some(code) = self.stopped(&format!("before run {n} of {}", p.runs)) {
            return Some(code);
        }
        if p.ci_check
            && let Some(code) = self.wait_for_ci(n)
        {
            return Some(code);
        }
        let log = self.log_dir.join(format!("run-{n}.log"));
        best_effort_cleanup(std::fs::remove_file(&log));
        let before = head_sha(&p.repo).unwrap_or_default();
        if before != self.first_sha {
            self.block.head_changed(n, &before, &self.first_sha);
            return Some(self.finish(
                2,
                &format!(
                    "SHA CHANGED before run {n} of {}: the runs do not prove one commit",
                    p.runs
                ),
            ));
        }
        // a signal that came while the commit was read: do not start the command
        if let Some(code) = self.stopped(&format!("before run {n} of {}", p.runs)) {
            return Some(code);
        }
        let rc = match &p.lock {
            Some(dir) => run_locked(self.config_dir, p, dir, n, &log),
            None => run_plain(&p.cmd, &p.repo, &log),
        };
        let after = head_sha(&p.repo).unwrap_or_default();
        if !log.is_file() {
            best_effort_output(std::fs::write(&log, ""));
        }
        let results = match &p.patterns.result {
            Some(re) => result_lines(
                &String::from_utf8_lossy(&std::fs::read(&log).unwrap_or_default()),
                re,
            ),
            None => Vec::new(),
        };
        self.block.ran(n, &after, rc, &log.display().to_string());
        if p.patterns.result.is_none() {
            self.block.judged_by_exit_code();
        } else {
            self.block.results(&results);
        }
        if let Some(code) = self.stopped(&format!(
            "during run {n} of {}: run {n} is not counted",
            p.runs
        )) {
            return Some(code);
        }
        if after != self.first_sha {
            return Some(self.finish(
                2,
                &format!(
                    "SHA CHANGED during run {n} of {} ({} -> {after}): run {n} is not counted",
                    p.runs, self.first_sha
                ),
            ));
        }
        let run = Run {
            n,
            runs: p.runs,
            rc,
            log: &log.display().to_string(),
            results: &results,
        };
        match judge(&p.patterns, &run) {
            Verdict::Green => None,
            Verdict::Red(line) => Some(self.finish(1, &line)),
        }
    }
}

/// Run `crewboard repeat`. `config_dir` is the global `--config-dir` (the board check of the
/// lock reads it). The result is the exit code.
pub fn run(config_dir: Option<&Path>, o: &Options) -> i32 {
    #[cfg(not(unix))]
    if o.lock.is_some() {
        eprintln!("{UNIX_ONLY}");
        return 2;
    }
    let plan = match check(o) {
        Ok(p) => p,
        Err(msg) => {
            say(&msg);
            return 64;
        }
    };
    if !plan.repo.is_dir() {
        say("--work-dir is not a directory");
        return 4;
    }
    let Some(first_sha) = head_sha(&plan.repo) else {
        say(&format!(
            "{} is not a git checkout with a commit: cannot record the SHA",
            plan.repo.display()
        ));
        return 4;
    };
    let log_dir = match make_log_dir(o.log_dir.as_deref()) {
        Ok(d) => d,
        Err(msg) => {
            say(&msg);
            return 4;
        }
    };
    #[cfg(unix)]
    crate::lock::sys::catch_term_int_hup();
    let mut lp = Loop {
        plan: &plan,
        config_dir,
        log_dir,
        first_sha,
        block: Block::new(),
    };
    for n in 1..=plan.runs {
        if let Some(code) = lp.one_run(n) {
            return code;
        }
    }
    let verdict = format!(
        "PASS: {} of {} runs green on {}",
        plan.runs, plan.runs, lp.first_sha
    );
    lp.finish(0, &verdict)
}
