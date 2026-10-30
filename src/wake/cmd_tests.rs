use super::*;
use crate::clock::FakeClock;
use crate::lock::procs::Procs;
use crate::wake::describe_head;
use crate::wake::flow::{Flow, Reason};
use crate::wake::lockdir::set_mtime;
use crate::wake::policy::{NetBackoff, Quiet, WorkBackoff};
use crate::wake::preflight::config_dir_of_command;
use crate::wake::procs::{Os, Proc, descendant_procs, procs_of, same_processes, stop_group_with};
use crate::wake::state;
use crate::wake::steps::StepRc;
use crate::wake::wake_events::with_budget_header;
use clap::Parser;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const NOW: i64 = 1_700_000_000;

#[derive(Parser)]
struct P {
    #[command(flatten)]
    w: WakeArgs,
}

fn parse(argv: &[&str]) -> WakeArgs {
    let mut v = vec!["wake"];
    v.extend_from_slice(argv);
    P::try_parse_from(v).unwrap().w
}

/// The calls of a run, scripted: one result per call, in order. `mine` has a plan of its own.
/// An empty plan gives exit 1.
#[derive(Default)]
struct Scripted {
    plan: VecDeque<(u8, String, String)>,
    mine: VecDeque<(u8, String)>,
    calls: Vec<String>,
    /// Runs at the start of each call that pops the plan (a test sets a signal here).
    hook: Option<Box<dyn Fn()>>,
    /// The answer of `acted --list`, and the receipts that `acted --since` gives (one per
    /// line); `acted_rc` is the exit code of both.
    acted_list: String,
    acted_check: Vec<String>,
    acted_rc: u8,
    acted_calls: Vec<String>,
    /// The heads of PRs by `<repo>#<n>`, and the open PRs of every repo: number, branch, SHA.
    heads: std::collections::HashMap<String, (String, String)>,
    open: Vec<(String, String, String)>,
}

impl Scripted {
    fn new(plan: &[&str]) -> Scripted {
        let mut s = Scripted::default();
        for l in plan {
            s.push(l);
        }
        s
    }

    /// `<code>|<stdout>|<stderr>`
    fn push(&mut self, l: &str) {
        let mut it = l.splitn(3, '|');
        let code = it.next().unwrap().parse().unwrap();
        let mut out = it.next().unwrap_or("").replace("\\n", "\n");
        if !out.is_empty() {
            out.push('\n');
        }
        let err = it.next().unwrap_or("").to_string();
        self.plan.push_back((code, out, err));
    }

    fn with_mine(mut self, plan: &[(u8, &str)]) -> Scripted {
        self.mine = plan.iter().map(|(c, o)| (*c, o.to_string())).collect();
        self
    }

    fn next(&mut self, out: &mut Vec<u8>) -> StepRc {
        if let Some(h) = &self.hook {
            h();
        }
        let (code, o, e) = self
            .plan
            .pop_front()
            .unwrap_or((1, String::new(), String::new()));
        out.extend_from_slice(o.as_bytes());
        StepRc::from_stderr(code, &e)
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls.iter().filter(|c| c.starts_with(prefix)).count()
    }
}

impl Steps for Scripted {
    fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc {
        self.calls.push(if quiet {
            "poll --quiet".into()
        } else {
            "poll".into()
        });
        self.next(out)
    }
    fn mine_quiet(&mut self) -> StepRc {
        self.calls.push("mine --quiet".into());
        let (c, _) = self.mine.pop_front().unwrap_or((1, String::new()));
        StepRc {
            code: c,
            msg: String::new(),
        }
    }
    fn mine_text(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.calls.push("mine".into());
        let (c, o) = self.mine.pop_front().unwrap_or((1, String::new()));
        out.extend_from_slice(o.as_bytes());
        StepRc {
            code: c,
            msg: String::new(),
        }
    }
    fn say(&mut self, ticket: &str, body: &str, anyway: bool) -> StepRc {
        self.calls
            .push(format!("say {ticket} anyway={anyway} {body}"));
        self.next(&mut Vec::new())
    }
    fn tick(&mut self, out: &mut Vec<u8>) -> StepRc {
        self.calls.push("tick".into());
        self.next(out)
    }
    fn acted_list(&mut self, events: &Path, out: &mut Vec<u8>) -> StepRc {
        self.acted_calls
            .push(format!("--events-file {} --list", events.display()));
        if self.acted_rc != 0 {
            return StepRc::from_stderr(self.acted_rc, "error: unrecognized subcommand 'acted'");
        }
        out.extend_from_slice(self.acted_list.as_bytes());
        StepRc::ok()
    }
    fn acted_check(
        &mut self,
        events: &Path,
        since: &str,
        redeliver: &Path,
        out: &mut Vec<u8>,
    ) -> StepRc {
        self.acted_calls.push(format!(
            "--events-file {} --since {since} --redeliver-to {}",
            events.display(),
            redeliver.display()
        ));
        if self.acted_rc != 0 {
            return StepRc::from_stderr(self.acted_rc, "error: unrecognized subcommand 'acted'");
        }
        let text = std::fs::read_to_string(events).unwrap_or_default();
        for r in self.acted_check.clone() {
            if text.lines().any(|l| l == format!("REDELIVERED {r}")) {
                out.extend_from_slice(format!("{r} twice\n").as_bytes());
            } else {
                out.extend_from_slice(format!("{r} again\n").as_bytes());
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(redeliver)
                    .unwrap();
                write!(f, "REDELIVERED {r}\nblock of {r}\n\n").unwrap();
            }
        }
        StepRc::ok()
    }

    fn heartbeat(&mut self) -> StepRc {
        self.calls.push("heartbeat".into());
        StepRc::ok()
    }
    fn pr_head(&mut self, repo: &str, number: &str) -> Result<Option<(String, String)>, StepRc> {
        self.calls.push(format!("pr_head {repo}#{number}"));
        Ok(self.heads.get(&format!("{repo}#{number}")).cloned())
    }
    fn open_prs(&mut self, repo: &str) -> Result<Vec<(String, String, String)>, StepRc> {
        self.calls.push(format!("open_prs {repo}"));
        Ok(self.open.clone())
    }
}

#[derive(Clone, Default)]
struct Buf(Rc<RefCell<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A temp dir, a clock and a standard error that a test can read.
struct Rig {
    tmp: tempfile::TempDir,
    clock: FakeClock,
    err: Buf,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            tmp: tempfile::tempdir().unwrap(),
            clock: FakeClock::at(NOW),
            err: Buf::default(),
        }
    }

    fn p(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    /// True when no temp file `<name>.tmp*` of an atomic write is left in the rig dir.
    fn has_no_temp_of(&self, name: &str) -> bool {
        let prefix = format!("{name}.tmp");
        let mut n = 0;
        for e in std::fs::read_dir(self.tmp.path()).unwrap().flatten() {
            n += 1;
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                return false;
            }
        }
        n > 0 // the dir is read: a listing that is empty proves nothing
    }

    fn state(&self) -> PathBuf {
        self.p("state")
    }

    fn lock(&self) -> PathBuf {
        self.state().join("wake.lock")
    }

    fn log_path(&self) -> PathBuf {
        self.state().join("wake.log")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    fn stderr(&self) -> String {
        String::from_utf8(self.err.0.borrow().clone()).unwrap()
    }

    /// The wake command that records each wake as a line in `wakes`.
    fn wake_cmd(&self) -> String {
        format!("echo wake >> '{}'", self.p("wakes").display())
    }

    fn wakes(&self) -> usize {
        std::fs::read_to_string(self.p("wakes"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    fn write_state(&self, name: &str, text: &str) {
        std::fs::create_dir_all(self.state()).unwrap();
        std::fs::write(self.state().join(name), text).unwrap();
    }

    fn state_text(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.state().join(name)).ok()
    }

    /// The settings for the flags: the defaults of a test, then `extra` (a flag in `extra`
    /// replaces the default of the same name).
    fn settings(&self, extra: &[&str], env: &[(&str, &str)]) -> Result<Settings, BadFlag> {
        let wake = self.wake_cmd();
        let state = self.state().to_string_lossy().to_string();
        let mut argv: Vec<String> = Vec::new();
        for (flag, val) in [
            ("--state-dir", state.as_str()),
            ("--wake-cmd", wake.as_str()),
            ("--retry-delays", ""),
        ] {
            let own_job = flag == "--wake-cmd" && extra.contains(&"--runtime");
            if !extra.contains(&flag) && !own_job {
                argv.push(flag.into());
                argv.push(val.into());
            }
        }
        argv.extend(extra.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let a = parse(&refs);
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let get = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        match resolve(&a, &GlobalFlags::default(), &get, None)? {
            Plan::Run(s) => Ok(*s),
            Plan::PrintStateDir(_) | Plan::PrintCmd(_) => panic!("print"),
        }
    }

    /// One run with the flags; the exit code.
    fn run(&self, steps: &mut Scripted, extra: &[&str]) -> u8 {
        self.run_env(steps, extra, &[])
    }

    fn run_env(&self, steps: &mut Scripted, extra: &[&str], env: &[(&str, &str)]) -> u8 {
        let s = self.settings(extra, env).unwrap();
        self.run_settings(steps, &s)
    }

    fn run_settings(&self, steps: &mut Scripted, s: &Settings) -> u8 {
        self.run_signals(steps, s, &|| None)
    }

    fn run_signals(
        &self,
        steps: &mut Scripted,
        s: &Settings,
        signals: &dyn Fn() -> Option<i32>,
    ) -> u8 {
        std::fs::create_dir_all(&s.paths.dir).unwrap();
        let log = Log::with_err(s.paths.log.clone(), &self.clock, Box::new(self.err.clone()));
        Flow {
            s,
            steps,
            clock: &self.clock,
            pids: &SystemProcs,
            log: &log,
            host: "testhost",
            sig: Cell::new(None),
            signals,
            heartbeat_steps: None,
        }
        .execute()
    }
}

/// A pid of a process that is gone.
fn dead_pid() -> u32 {
    let mut c = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

fn plant_lock(dir: &Path, pid: u32, started: i64) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("owner"), "old-owner\n").unwrap();
    std::fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
    std::fs::write(dir.join("started"), format!("{started}\n")).unwrap();
}

fn rep(n: usize, l: &str) -> Vec<&str> {
    vec![l; n]
}

// --- gate and quiet ---

#[test]
fn p1_twenty_quiet_polls_start_no_wake_and_say_nothing() {
    let rig = Rig::new();
    let mut st = Scripted::new(&rep(20, "1||"));
    for _ in 0..20 {
        assert_eq!(rig.run(&mut st, &[]), 0);
    }
    assert_eq!(rig.wakes(), 0);
    assert_eq!(st.count("poll --quiet"), 20);
    assert_eq!(rig.stderr(), "");
    assert_eq!(rig.log(), "");
}

#[test]
fn p2_one_exit_0_among_twenty_polls_gives_exactly_one_wake_and_releases_the_lock() {
    let rig = Rig::new();
    let mut plan = rep(7, "1||");
    plan.push("0||");
    plan.extend(rep(12, "1||"));
    let mut st = Scripted::new(&plan);
    for _ in 0..20 {
        assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    }
    assert_eq!(rig.wakes(), 1);
    assert!(!rig.lock().exists(), "the lock is released");
}

#[test]
fn p3_exit_3_is_rc_3_with_no_wake_and_is_logged() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||network down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log()
            .contains("crewboard poll failed: exit 3: network down"),
        "{}",
        rig.log()
    );
}

#[test]
fn p3b_exit_4_passes_through_and_a_missing_wake_command_is_refused() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||no key"]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert_eq!(rig.wakes(), 0);
    // no wake command and no --tick
    let a = parse(&["--state-dir", "/s"]);
    let e = resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap_err();
    assert_eq!(
        e.text,
        "no wake command (--wake-cmd, CREWBOARD_WAKE_CMD or --tick)"
    );
    assert!(e.paths.is_none(), "the line comes before the log is known");
    // the other direction: the environment gives the wake command, and so does --tick
    let env = |k: &str| (k == "CREWBOARD_WAKE_CMD").then(|| "true".to_string());
    assert!(resolve(&a, &GlobalFlags::default(), &env, None).is_ok());
    let t = parse(&["--state-dir", "/s", "--tick"]);
    assert!(resolve(&t, &GlobalFlags::default(), &|_| None, None).is_ok());
}

#[test]
fn p3d_a_failing_poll_logs_its_first_stderr_line_cut_and_cleaned() {
    let rig = Rig::new();
    let long = "Z".repeat(300);
    let mut st = Scripted::new(&[&format!(
        "4||configuration error: no board file for initiative demo\x1b[31m {long}\nhint: run crewboard join first"
    )]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    let log = rig.log();
    let line = log
        .lines()
        .find(|l| l.contains("STOPPED: crewboard poll exit 4"))
        .unwrap();
    assert!(line.contains("exit 4: configuration error: no board file for initiative demo"));
    assert!(!line.contains("hint:") && !line.contains('\x1b'));
    let reason = line
        .split("exit 4: ")
        .nth(1)
        .unwrap()
        .split(". No more wakes")
        .next()
        .unwrap();
    assert!(reason.chars().count() <= 200, "{}", reason.chars().count());
    assert!(rig.stderr().contains("configuration error"));
}

#[test]
fn p3e_a_quiet_poll_logs_nothing_at_all() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||configuration error: never shown"]);
    assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    assert_eq!(rig.log(), "");
    assert_eq!(rig.stderr(), "");
}

#[test]
fn q1_quiet_polls_inside_one_hour_write_no_line_and_are_counted() {
    let rig = Rig::new();
    let mut st = Scripted::new(&rep(5, "1||"));
    for _ in 0..5 {
        assert_eq!(rig.run(&mut st, &[]), 0);
    }
    assert_eq!((rig.log(), rig.stderr()), (String::new(), String::new()));
    let q = Quiet::parse(&rig.state_text("wake.log.quiet").unwrap()).unwrap();
    assert_eq!(q.n, 5);
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn q2_the_first_quiet_poll_after_the_hour_writes_one_summary_line() {
    let rig = Rig::new();
    rig.write_state(
        "wake.log.quiet",
        &format!("{} 2026-01-01T00:00:00Z 5\n", NOW - 3601),
    );
    let mut st = Scripted::new(&rep(2, "1||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.stderr(), "");
    let log = rig.log();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(
        log.ends_with("poll-and-wake: quiet: 6 polls since 2026-01-01T00:00:00Z\n"),
        "{log}"
    );
    let after = Quiet::parse(&rig.state_text("wake.log.quiet").unwrap()).unwrap();
    assert_eq!(after.n, 0);
    // the next quiet poll writes no line and starts counting
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.log().lines().count(), 1);
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        1
    );
}

#[test]
fn q3_a_wake_logs_as_before_and_a_broken_count_file_restarts() {
    let rig = Rig::new();
    rig.write_state("wake.log.quiet", "junk\n");
    let mut st = Scripted::new(&["1||", "0||", "1||"]);
    for _ in 0..3 {
        rig.run(&mut st, &[]);
    }
    assert_eq!(rig.wakes(), 1);
    let log = rig.log();
    assert_eq!(log.lines().count(), 2, "{log}");
    assert!(log.contains("poll-and-wake: events pending; waking: "));
    assert!(log.ends_with("poll-and-wake: wake finished OK\n"));
    assert!(!log.contains("quiet:"));
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        2
    );
}

// --- wake and lock ---

#[test]
fn p3c_a_failing_wake_command_is_rc_70_and_the_lock_is_released() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", "exit 9"]), 70);
    assert!(!rig.lock().exists());
    assert!(rig.log().contains("wake command failed: exit 9"));
    // every wake, also a failed one, starts the back-off
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
}

#[test]
fn p4_a_lock_of_a_live_wake_starts_no_second_wake() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW - 5);
    let mut st = Scripted::new(&rep(5, "0||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains(&format!(
            "a wake already runs (pid {}, started {}); not starting another",
            std::process::id(),
            NOW - 5
        )),
        "{}",
        rig.log()
    );
    assert!(rig.lock().is_dir(), "the lock of the live wake stays");
    // twin: the same state without the lock wakes once
    std::fs::remove_dir_all(rig.lock()).unwrap();
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
}

#[test]
fn p5_a_stale_lock_of_a_dead_pid_is_taken_over_and_said() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), dead_pid(), NOW - 5);
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(
        rig.log().contains("STALE lock (pid-dead: owner pid "),
        "{}",
        rig.log()
    );
    assert!(
        rig.stderr().contains("STALE lock"),
        "the takeover is said on stderr too"
    );
}

#[test]
fn p5b_a_live_lock_over_the_budget_is_taken_a_young_one_is_not() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW - 100);
    let mut st = Scripted::new(&rep(2, "0||"));
    rig.run(&mut st, &["--stale-after", "3600"]);
    assert_eq!(rig.wakes(), 0);
    rig.run(&mut st, &["--stale-after", "10"]);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains("STALE lock (too-old: "));
}

#[test]
fn p5c_p5d_an_empty_lock_dir_waits_for_the_grace() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.lock()).unwrap();
    set_mtime(&rig.lock(), NOW - 2);
    let mut st = Scripted::new(&rep(2, "0||"));
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 0, "a fresh empty lock dir is not taken over");
    assert!(rig.log().contains("already runs") && !rig.log().contains("STALE"));
    set_mtime(&rig.lock(), NOW - 100);
    assert_eq!(rig.run(&mut st, &["--empty-grace", "5"]), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains("STALE lock (empty-after-grace:"));
}

#[test]
fn a_lock_that_cannot_be_taken_is_rc_71() {
    let rig = Rig::new();
    // a file where the parent of the lock should be
    std::fs::create_dir_all(rig.state()).unwrap();
    std::fs::write(rig.p("blocker"), "x").unwrap();
    let lock = rig.p("blocker/wake.lock");
    let mut st = Scripted::new(&["0||"]);
    let s = rig
        .settings(
            &[
                "--lock-dir",
                lock.to_str().unwrap(),
                "--state-dir",
                rig.state().to_str().unwrap(),
            ],
            &[],
        )
        .unwrap();
    assert_eq!(rig.run_settings(&mut st, &s), 71);
    assert!(rig.log().contains("cannot take the lock "), "{}", rig.log());
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn the_owner_text_names_the_host_and_the_agent() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    // a wake command that reads the owner file of its own lock
    let cmd = format!(
        "cat '{}/owner' > '{}'",
        rig.lock().display(),
        rig.p("owner-seen").display()
    );
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", &cmd, "--agent", "dev@host-a"]),
        0
    );
    assert_eq!(
        std::fs::read_to_string(rig.p("owner-seen")).unwrap(),
        "poll-and-wake@testhost dev@host-a\n"
    );
    // without an agent: no trailing blank
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", &cmd]), 0);
    assert_eq!(
        std::fs::read_to_string(rig.p("owner-seen")).unwrap(),
        "poll-and-wake@testhost\n"
    );
}

// --- options ---

#[test]
fn p6_the_config_dir_reaches_the_wake_command() {
    let rig = Rig::new();
    let cmd = format!(
        "printf '%s' \"$CREWBOARD_CONFIG_DIR\" > '{}'",
        rig.p("cfgseen").display()
    );
    let mut st = Scripted::new(&["0||"]);
    let a = parse(&[
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--wake-cmd",
        &cmd,
        "--retry-delays",
        "",
    ]);
    let g = GlobalFlags {
        config_dir: Some("/x/agentcfg".into()),
        ..GlobalFlags::default()
    };
    let Plan::Run(s) = resolve(&a, &g, &|_| None, None).unwrap() else {
        panic!()
    };
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(
        std::fs::read_to_string(rig.p("cfgseen")).unwrap(),
        "/x/agentcfg"
    );
}

#[test]
fn p7_owned_work_without_events_gives_one_work_wake() {
    let rig = Rig::new();
    let cmd = format!(
        "echo wake >> '{}'; printf '%s' \"$CREWBOARD_WAKE_REASON\" > '{}'",
        rig.p("wakes").display(),
        rig.p("reason").display()
    );
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", &cmd]), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(std::fs::read_to_string(rig.p("reason")).unwrap(), "work");
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
    assert!(rig.log().contains("owns an In Progress or Ready ticket"));
    // the other direction of the reason: an event wake says events
    let mut st = Scripted::new(&["0||"]);
    rig.run(&mut st, &["--wake-cmd", &cmd]);
    assert_eq!(std::fs::read_to_string(rig.p("reason")).unwrap(), "events");
}

#[test]
fn p8_no_events_and_no_owned_ticket_is_a_silent_quiet_poll() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(1, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!((rig.wakes(), st.count("mine --quiet")), (0, 1));
    assert_eq!((rig.log(), rig.stderr()), (String::new(), String::new()));
}

#[test]
fn p9_p10_the_back_off_decides_whether_mine_is_even_asked() {
    let rig = Rig::new();
    rig.write_state("last-wake", &format!("{}\n", NOW - 60));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(
        (rig.wakes(), st.count("mine")),
        (0, 0),
        "inside the back-off mine is not asked"
    );
    // 1000 s ago is over the 900 s back-off: a work wake, and the back-off restarts
    rig.write_state("last-wake", &format!("{}\n", NOW - 1000));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(rig.wakes(), 1);
    assert_eq!(
        state::read_last_wake(&rig.state().join("last-wake")),
        Some(NOW)
    );
}

#[test]
fn p10b_the_back_off_is_configurable_off_never_wakes_and_junk_is_refused() {
    let rig = Rig::new();
    rig.write_state("last-wake", &format!("{}\n", NOW - 10));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &["--work-backoff", "5"]);
    assert_eq!(rig.wakes(), 1);
    std::fs::remove_file(rig.state().join("last-wake")).unwrap();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert_eq!(
        (rig.wakes(), st.count("mine")),
        (1, 0),
        "off never wakes for work"
    );
    let e = rig.settings(&["--work-backoff", "soon"], &[]).unwrap_err();
    assert_eq!(
        e.text,
        "--work-backoff needs whole seconds or off, got 'soon'"
    );
    assert!(e.paths.is_some(), "this line goes to the log too");
    // the environment gives the same default as the flag
    let s = rig
        .settings(&[], &[("CREWBOARD_WORK_BACKOFF", "off")])
        .unwrap();
    assert_eq!(s.work_backoff, WorkBackoff::Off);
}

#[test]
fn p11_an_event_wake_starts_the_back_off_too() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "1||"]).with_mine(&[(0, "")]);
    rig.run(&mut st, &[]);
    rig.run(&mut st, &[]);
    assert_eq!(
        rig.wakes(),
        1,
        "a quiet tick right after a wake does not wake for work"
    );
    assert_eq!(st.count("mine"), 0);
}

#[test]
fn s2_print_state_dir_follows_the_flags() {
    let env = |k: &str| (k == "XDG_STATE_HOME").then(|| "/st".to_string());
    let dir =
        |argv: &[&str]| match resolve(&parse(argv), &GlobalFlags::default(), &env, None).unwrap() {
            Plan::PrintStateDir(d) => d,
            Plan::Run(_) | Plan::PrintCmd(_) => panic!("run"),
        };
    let p = PathBuf::from;
    assert_eq!(
        dir(&[
            "--print-state-dir",
            "--initiative",
            "demo",
            "--agent",
            "dev@host-a"
        ]),
        p("/st/crewboard/agents/demo-dev_host-a")
    );
    assert_eq!(
        dir(&[
            "--print-state-dir",
            "--initiative",
            "demo",
            "--lock-dir",
            "/y/wake.lock"
        ]),
        p("/y")
    );
    assert_eq!(
        dir(&["--print-state-dir", "--initiative", "demo"]),
        p("/st/crewboard/demo")
    );
    // it needs no wake command
    assert_eq!(dir(&["--print-state-dir", "--state-dir", "/s"]), p("/s"));
    // no state root at all
    let none = |_: &str| None;
    assert!(
        resolve(
            &parse(&["--print-state-dir"]),
            &GlobalFlags::default(),
            &none,
            None
        )
        .is_err()
    );
}

// --- retry and stop ---

#[test]
fn r1_exit_3_is_retried_and_the_retry_finds_events() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||HTTP 502", "0||"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0"]), 0);
    assert_eq!((rig.wakes(), st.count("poll")), (1, 2));
    assert!(
        rig.log()
            .contains("crewboard poll: exit 3 (try 1): HTTP 502; retry in 0 s"),
        "{}",
        rig.log()
    );
    assert_eq!(
        rig.stderr().matches("retry in").count(),
        0,
        "a retry notice goes to the log only"
    );
}

#[test]
fn r1b_every_delay_is_waited_through_the_clock() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down", "3||down"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "10 30"]), 3);
    assert_eq!(
        st.count("poll"),
        3,
        "the first call and one retry per delay"
    );
    assert_eq!(rig.clock.now(), NOW + 40, "10 s and 30 s, no real sleep");
    let log = rig.log();
    assert!(
        log.contains("exit 3 (try 1): down; retry in 10 s")
            && log.contains("exit 3 (try 2): down; retry in 30 s")
    );
    // twin: a retry that succeeds stops waiting
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "1||"]);
    rig.run(
        &mut st,
        &["--retry-delays", "10 30", "--work-backoff", "off"],
    );
    assert_eq!((st.count("poll"), rig.clock.now()), (2, NOW + 10));
    // and exit 2 is never retried
    let rig = Rig::new();
    let mut st = Scripted::new(&["2||refused"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "10 30"]), 2);
    assert_eq!((st.count("poll"), rig.clock.now()), (1, NOW));
}

#[test]
fn r2_exit_3_in_every_try_pauses_the_runs_from_the_second_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down", "3||down", "3||down", "1||"]);
    let args = ["--retry-delays", "0"];
    assert_eq!(rig.run(&mut st, &args), 3);
    let b1 = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b1.fails, b1.until <= NOW), (1, true));
    assert_eq!(rig.run(&mut st, &args), 3);
    let b2 = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b2.fails, b2.until), (2, NOW + 300));
    let before = st.calls.len();
    assert_eq!(rig.run(&mut st, &args), 0);
    assert_eq!(st.calls.len(), before, "inside the pause nothing is called");
    assert_eq!(rig.wakes(), 0);
    // the paused run counts as a quiet poll
    assert_eq!(
        Quiet::parse(&rig.state_text("wake.log.quiet").unwrap())
            .unwrap()
            .n,
        1
    );
}

#[test]
fn r2_the_owner_hears_of_it_at_the_third_failed_run_only() {
    let rig = Rig::new();
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    let said = rig.stderr();
    assert!(said.contains("GitHub or network error in 3 runs in a row; the owner should check the network and the token. The next runs pause up to 1 h"), "{said}");
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()),
        NetBackoff {
            fails: 3,
            until: NOW + 600
        }
    );
    // the fourth run does not say it again
    rig.write_state("net-backoff", &format!("3 {}\n", NOW - 1));
    let before = rig.stderr().matches("GitHub or network error").count();
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    assert_eq!(
        rig.stderr().matches("GitHub or network error").count(),
        before
    );
}

#[test]
fn r2b_when_the_pause_is_over_a_good_poll_clears_the_back_off() {
    let rig = Rig::new();
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rig.run(&mut st, &["--work-backoff", "off"]), 0);
    assert!(rig.state_text("net-backoff").is_none());
    // control: inside the pause the file stays
    rig.write_state("net-backoff", &format!("2 {}\n", NOW + 100));
    let mut st = Scripted::new(&["1||"]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert!(rig.state_text("net-backoff").is_some());
    assert_eq!(st.calls.len(), 0);
}

#[test]
fn x1_exit_4_stops_says_once_tells_the_coordinator_and_silences_the_next_run() {
    let rig = Rig::new();
    let env_file = rig.p("x.env");
    std::fs::write(&env_file, "GH_TOKEN=x\n").unwrap();
    let mut st =
        Scripted::new(&["4||configuration error: key file missing", "0||"]).with_mine(&[(
            0,
            "demo/app#3                         In Progress      P1  order 1\n",
        )]);
    let g = GlobalFlags {
        env_file: Some(env_file.clone()),
        ..GlobalFlags::default()
    };
    let run = |st: &mut Scripted| {
        let s = rig.settings(&["--agent", "dev@host-a"], &[]).unwrap();
        let s = Settings {
            env_file: g.env_file.clone(),
            ..s
        };
        rig.run_settings(st, &s)
    };
    assert_eq!(run(&mut st), 4);
    let calls = st.calls.len();
    assert_eq!(run(&mut st), 0, "the next run exits 0 at once");
    assert_eq!(st.calls.len(), calls, "and calls nothing");
    assert_eq!(st.count("mine"), 1);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .ends_with(" poll: configuration error: key file missing\n")
    );
    let log = rig.log();
    assert_eq!(
        log.matches("STOPPED: crewboard poll exit 4: configuration error: key file missing")
            .count(),
        1
    );
    assert!(rig.stderr().contains("STOPPED"));
    assert!(log.contains("told the coordinator on demo/app#3"));
    let said = st.calls.iter().find(|c| c.starts_with("say ")).unwrap();
    assert!(said.starts_with("say demo/app#3 anyway=true The wake wrapper of dev@host-a stopped: crewboard exit 4 (configuration error: key file missing)."), "{said}");
    assert_eq!(rig.wakes(), 0);
}

#[test]
fn x1b_a_newer_env_file_lifts_the_stop_and_the_run_polls_again() {
    let rig = Rig::new();
    let env_file = rig.p("x.env");
    std::fs::write(&env_file, "GH_TOKEN=x\n").unwrap();
    rig.write_state("stopped", "ts poll: reason\n");
    let stop = rig.state().join("stopped");
    let mut st = Scripted::new(&["1||"]);
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    let s = Settings {
        env_file: Some(env_file.clone()),
        ..s
    };
    // the env file is older than the stop: still stopped, and silent
    set_mtime(&env_file, NOW - 1000);
    set_mtime(&stop, NOW);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(
        (st.calls.len(), rig.log(), rig.stderr()),
        (0, String::new(), String::new())
    );
    assert!(stop.exists());
    // the env file is newer: the stop is lifted and said, the poll runs
    set_mtime(&stop, NOW - 5000);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert!(!stop.exists());
    assert_eq!(st.count("poll --quiet"), 1);
    assert!(rig.log().contains(&format!(
        "stop lifted: {} changed after the stop; polling again",
        env_file.display()
    )));
}

#[test]
fn x1c_a_changed_agent_config_or_key_lifts_the_stop_too() {
    let rig = Rig::new();
    let cfg = rig.p("cfg");
    std::fs::create_dir_all(cfg.join("keys")).unwrap();
    std::fs::write(cfg.join("demo.toml"), "x").unwrap();
    std::fs::write(cfg.join("keys/dev.key"), "k").unwrap();
    std::fs::write(cfg.join("notes.txt"), "n").unwrap();
    let stop = rig.state().join("stopped");
    let cfg_s = cfg.to_str().unwrap();
    let go = |newer: &str| {
        rig.write_state("stopped", "ts poll: reason\n");
        for f in ["demo.toml", "keys/dev.key", "notes.txt"] {
            set_mtime(&cfg.join(f), NOW - 1000);
        }
        set_mtime(&stop, NOW);
        set_mtime(&cfg.join(newer), NOW + 100);
        let mut st = Scripted::new(&["1||"]);
        let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
        let s = Settings {
            config_dir: Some(PathBuf::from(cfg_s)),
            ..s
        };
        rig.run_settings(&mut st, &s);
        st.calls.len()
    };
    assert_eq!(go("demo.toml"), 1);
    assert_eq!(go("keys/dev.key"), 1);
    // a file that is no config does not lift it
    assert_eq!(go("notes.txt"), 0);
}

#[test]
fn x2_exit_2_is_no_stop_and_the_next_run_polls_again() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["2||refused [rule=x]", "1||"]);
    let a = ["--work-backoff", "off"];
    assert_eq!(rig.run(&mut st, &a), 2);
    assert_eq!(rig.run(&mut st, &a), 0);
    assert_eq!(st.count("poll --quiet"), 2);
    assert!(rig.state_text("stopped").is_none());
    assert!(
        rig.log()
            .contains("crewboard poll failed: exit 2: refused [rule=x]")
    );
}

#[test]
fn x1_exit_4_of_mine_in_the_gate_stops_too() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(4, "")]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" mine --quiet: ")
    );
    assert!(
        rig.log()
            .contains("STOPPED: crewboard mine --quiet exit 4:")
    );
}

#[test]
fn exit_3_of_mine_in_the_gate_counts_as_a_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["1||"]).with_mine(&[(3, ""), (3, "")]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0"]), 3);
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    assert!(rig.log().contains("crewboard mine --quiet failed: exit 3"));
    assert_eq!(rig.wakes(), 0);
}

// --- gate details ---

// --- tick ---

const TICKOUT: &str = "0|tick: verified and merged demo/app#5\\nleft for the interactive coordinator: demo/app#6, the Gate is not CI only|";

#[test]
fn co1_tick_runs_its_lines_go_to_the_log_and_no_model_starts_and_no_poll_runs() {
    let rig = Rig::new();
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(st.calls, vec!["tick"]);
    assert_eq!(rig.wakes(), 0);
    assert_eq!(rig.stderr(), "");
    let log = rig.log();
    assert!(
        log.contains("poll-and-wake: tick: tick: verified and merged demo/app#5\n"),
        "{log}"
    );
    assert!(log.contains("poll-and-wake: tick: left for the interactive coordinator: demo/app#6, the Gate is not CI only\n"));
    assert!(!rig.lock().exists(), "the lock is released");
    assert!(
        rig.state_text("last-tick")
            .unwrap()
            .starts_with("tick: verified")
    );
}

#[test]
fn co2_the_same_output_again_and_an_idle_tick_write_no_new_line() {
    let rig = Rig::new();
    let mut st = Scripted::new(&[TICKOUT, TICKOUT, "0|tick: no ticket in Review|"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    let before = rig.log().lines().count();
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert_eq!(rig.log().lines().count(), before);
    assert_eq!(st.count("tick"), 3);
    // the idle tick replaced last-tick, so the busy output is news again
    let mut st = Scripted::new(&[TICKOUT]);
    rig.run(&mut st, &["--tick"]);
    assert!(rig.log().lines().count() > before);
}

#[test]
fn co3_a_refused_tick_is_logged_with_its_reason_and_exits_2() {
    let rig = Rig::new();
    let mut st =
        Scripted::new(&["2||refused [rule=tick-headless-only] tick is for a headless config dir"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 2);
    assert!(
        rig.log()
            .contains("crewboard tick failed: exit 2: refused [rule=tick-headless-only]")
    );
    assert!(!rig.lock().exists());
}

#[test]
fn a_tick_with_exit_3_retries_and_counts_a_failed_run() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["3||down", "3||down"]);
    assert_eq!(rig.run(&mut st, &["--tick", "--retry-delays", "0"]), 3);
    assert!(
        rig.log()
            .contains("crewboard tick: exit 3 (try 1): down; retry in 0 s")
    );
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    // a good tick clears it
    let mut st = Scripted::new(&["0|tick: no ticket in Review|"]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert!(rig.state_text("net-backoff").is_none());
}

#[test]
fn a_tick_under_a_live_lock_starts_no_second_tick() {
    let rig = Rig::new();
    plant_lock(&rig.lock(), std::process::id(), NOW);
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run(&mut st, &["--tick"]), 0);
    assert!(st.calls.is_empty());
    assert!(rig.log().contains("already runs"));
}

// --- values and flags ---

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
        (4, true),
        "the line goes to the log too"
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

// --- more cases ---

#[test]
fn exit_4_is_never_retried() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||bad key", "4||bad key"]);
    assert_eq!(rig.run(&mut st, &["--retry-delays", "0 0"]), 4);
    assert_eq!(st.count("poll"), 1, "exactly one call");
    assert!(!rig.log().contains("retry in"));
}

#[test]
fn a_good_poll_followed_by_exit_3_of_mine_does_not_reset_the_back_off() {
    // the poll succeeds (no events), `mine --quiet` fails with 3 in every try: the run is a
    // failed run, so the counter must go up, and the pause must start at the second
    let rig = Rig::new();
    let mut st =
        Scripted::new(&["1||", "1||", "1||"]).with_mine(&[(3, ""), (3, ""), (3, ""), (3, "")]);
    let args = ["--retry-delays", "0"];
    assert_eq!(rig.run(&mut st, &args), 3);
    assert_eq!(
        NetBackoff::parse(&rig.state_text("net-backoff").unwrap()).fails,
        1
    );
    assert_eq!(rig.run(&mut st, &args), 3);
    let b = NetBackoff::parse(&rig.state_text("net-backoff").unwrap());
    assert_eq!((b.fails, b.until), (2, NOW + 300));
    // twin: a gate that succeeds as a whole clears it
    rig.write_state("net-backoff", &format!("2 {}\n", NOW - 1));
    let mut st = Scripted::new(&["1||"]).with_mine(&[(1, "")]);
    assert_eq!(rig.run(&mut st, &args), 0);
    assert!(rig.state_text("net-backoff").is_none());
}

#[test]
fn a_state_file_that_cannot_be_written_is_one_log_line() {
    let rig = Rig::new();
    // a directory where the net-backoff file should go: the rename fails
    std::fs::create_dir_all(rig.state().join("net-backoff")).unwrap();
    let mut st = Scripted::new(&["3||down"]);
    assert_eq!(rig.run(&mut st, &[]), 3);
    let log = rig.log();
    assert!(
        log.contains(&format!(
            "cannot write {}",
            rig.state().join("net-backoff").display()
        )),
        "{log}"
    );
}

#[test]
fn x1c_a_newer_agent_env_lifts_the_stop_too() {
    let rig = Rig::new();
    let cfg = rig.p("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("agent.env"), "GH_TOKEN=x\n").unwrap();
    let stop = rig.state().join("stopped");
    rig.write_state("stopped", "ts poll: reason\n");
    set_mtime(&cfg.join("agent.env"), NOW - 1000);
    set_mtime(&stop, NOW);
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    let s = Settings {
        config_dir: Some(cfg.clone()),
        ..s
    };
    let mut st = Scripted::new(&["1||"]);
    rig.run_settings(&mut st, &s);
    assert!(st.calls.is_empty(), "an older agent.env does not lift it");
    set_mtime(&cfg.join("agent.env"), NOW + 100);
    rig.run_settings(&mut st, &s);
    assert_eq!(st.count("poll --quiet"), 1);
    assert!(rig.log().contains("agent.env changed after the stop"));
}

#[test]
fn a_signal_before_the_lock_ends_the_run_with_128_plus_the_signal() {
    let rig = Rig::new();
    let s = rig.settings(&["--work-backoff", "off"], &[]).unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&["0||"]);
    let mut flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| Some(15),
        heartbeat_steps: None,
    };
    assert_eq!(flow.execute(), 143);
    assert_eq!(rig.wakes(), 0);
    assert!(!rig.lock().exists());
    assert!(rig.log().contains("stopped by signal 15"));
}

/// A signal that arrives during the first call of a run.
fn signal_in_first_call(st: &mut Scripted) -> Rc<std::cell::Cell<Option<i32>>> {
    let pending = Rc::new(std::cell::Cell::new(None));
    let p = pending.clone();
    st.hook = Some(Box::new(move || p.set(Some(15))));
    pending
}

#[test]
fn a_signal_during_a_call_ends_the_run_with_143_and_no_retry() {
    let rig = Rig::new();
    let s = rig.settings(&["--retry-delays", "5 5"], &[]).unwrap();
    let mut st = Scripted::new(&["3||down", "0||"]);
    let pending = signal_in_first_call(&mut st);
    let rc = rig.run_signals(&mut st, &s, &|| pending.take());
    assert_eq!(rc, 143);
    assert_eq!(st.count("poll"), 1, "no second call");
    assert_eq!(rig.clock.now(), NOW, "no wait before a retry");
    assert!(
        rig.state_text("net-backoff").is_none(),
        "an interrupted run is no failed run"
    );
    assert!(!rig.log().contains("retry in"));
    assert_eq!(rig.wakes(), 0);
    // twin: without the signal the retry runs
    let mut st = Scripted::new(&["3||down", "0||"]);
    assert_eq!(rig.run_signals(&mut st, &s, &|| None), 0);
    assert_eq!(st.count("poll"), 2);
}

#[test]
fn a_signal_during_tick_is_143_not_0() {
    let rig = Rig::new();
    let s = rig.settings(&["--tick"], &[]).unwrap();
    let mut st = Scripted::new(&[TICKOUT]);
    let pending = signal_in_first_call(&mut st);
    assert_eq!(rig.run_signals(&mut st, &s, &|| pending.take()), 143);
    assert!(!rig.log().contains("tick: tick:"), "{}", rig.log());
    assert!(!rig.lock().exists());
    // twin: the same tick without the signal logs its lines and exits 0
    let rig = Rig::new();
    let s = rig.settings(&["--tick"], &[]).unwrap();
    let mut st = Scripted::new(&[TICKOUT]);
    assert_eq!(rig.run_signals(&mut st, &s, &|| None), 0);
    assert!(rig.log().contains("tick: tick:"));
}

#[test]
fn a_signal_before_the_stop_note_ends_the_run_without_the_note() {
    let rig = Rig::new();
    let s = rig.settings(&[], &[]).unwrap();
    let mut st = Scripted::new(&["4||bad"]).with_mine(&[(0, "demo/app#3 In Progress P1\n")]);
    let pending = signal_in_first_call(&mut st);
    assert_eq!(rig.run_signals(&mut st, &s, &|| pending.take()), 143);
    assert_eq!(st.count("say"), 0);
}

// --- the events file, the receipts and the results ---

fn ev(rig: &Rig) -> String {
    rig.p("ev.txt").to_string_lossy().to_string()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// A wake command that copies the events file it was given.
fn copy_events(rig: &Rig, to: &str) -> String {
    format!(
        "cp \"$CREWBOARD_EVENTS_FILE\" '{}'; echo wake >> '{}'",
        rig.p(to).display(),
        rig.p("wakes").display()
    )
}

#[test]
fn ev1_the_events_file_has_the_poll_and_the_mine_text_and_the_wake_gets_its_path() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12 to dev@host-a|"]).with_mine(&[(
        0,
        "demo/app#12                        Ready            P1  order 1\n",
    )]);
    let cmd = format!(
        "echo \"$CREWBOARD_EVENTS_FILE\" > '{}'; echo \"$CREWBOARD_RESULTS_DIR\" >> '{}'",
        rig.p("env").display(),
        rig.p("env").display()
    );
    let evf = ev(&rig);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", &cmd]),
        0
    );
    let t = read(&rig.p("ev.txt"));
    assert!(t.starts_with("== Events (crewboard poll, 20"), "{t}");
    assert!(
        t.contains(
            "\nEVENT 1: assign #12 to dev@host-a\n== Your tickets (crewboard mine):\ndemo/app#12 "
        ),
        "{t}"
    );
    assert_eq!(
        st.calls,
        vec!["poll --quiet", "poll", "mine"],
        "the cursor moves with the second poll"
    );
    assert_eq!(
        read(&rig.p("env")),
        format!("{evf}\n{}/results\n", rig.state().display())
    );
    assert!(rig.has_no_temp_of("ev.txt"), "the temp file is renamed");
    // the other direction (EV3): without --events-file only `poll --quiet` runs
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &[]), 0);
    assert_eq!(st.calls, vec!["poll --quiet"]);
    assert!(st.acted_calls.is_empty());
}

#[test]
fn ev2_a_failed_wake_keeps_its_events_for_the_next_wake() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 7: question on #4|", "0||", "1||"]);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", "exit 9"]),
        70
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(kept.contains("EVENT 7: question on #4"), "{kept}");
    let cmd = copy_events(&rig, "seen");
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", &cmd]),
        0
    );
    let seen = read(&rig.p("seen"));
    assert!(seen.starts_with(crate::wake::events::EARLIER), "{seen}");
    assert!(
        seen.contains("EVENT 7: question on #4") && seen.contains("(no new events)"),
        "{seen}"
    );
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the next wake got them and took them"
    );
}

#[test]
fn a_signal_during_the_wake_keeps_the_events_and_exits_143() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let started = rig.p("started");
    let cmd = format!("touch '{}'; sleep 30", started.display());
    let mut st = Scripted::new(&["0||", "0|EVENT 5: question on #2|", "0||"]);
    let s = rig
        .settings(&["--events-file", &evf, "--wake-cmd", &cmd], &[])
        .unwrap();
    let rc = rig.run_signals(&mut st, &s, &|| started.exists().then_some(15));
    assert_eq!(rc, 143);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(kept.contains("EVENT 5: question on #2"), "{kept}");
    assert!(!rig.lock().exists());
    assert!(
        st.acted_calls.iter().all(|c| !c.contains("--since")),
        "no check after a stopped wake: {:?}",
        st.acted_calls
    );
}

#[test]
fn an_unreadable_kept_file_stops_the_run_before_the_poll() {
    let rig = Rig::new();
    // a directory: reading it is an error that is not "not found"
    std::fs::create_dir_all(rig.p("ev.txt.unhandled")).unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert_eq!(st.calls, vec!["poll --quiet"], "the cursor did not move");
    assert_eq!(rig.wakes(), 0);
    assert!(
        rig.log().contains("cannot read the kept events "),
        "{}",
        rig.log()
    );
    assert!(rig.p("ev.txt.unhandled").is_dir());
}

#[test]
fn kept_events_that_are_not_utf8_are_read_lossily_not_dropped() {
    let rig = Rig::new();
    std::fs::write(rig.p("ev.txt.unhandled"), b"== kept\nEVENT \xff7\n").unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "1||"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 0);
    let t = read(&rig.p("ev.txt"));
    assert!(t.contains("EVENT \u{fffd}7"), "{t}");
}

#[test]
fn a1_a_dev_agent_wake_runs_the_runtime_in_the_work_dir_with_its_environment() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let out = rig.p("rt.out");
    let bin = rig.p("fake-claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\n{{ pwd -P; echo \"wake=$CREWBOARD_WAKE events=$CREWBOARD_EVENTS_FILE\"; for a in \"$@\"; do echo \"arg=$a\"; done; }} > '{}'\n",
            out.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let (wds, bins) = (wd.to_str().unwrap(), bin.to_str().unwrap());
    let extra = [
        "--runtime",
        "claude",
        "--runtime-bin",
        bins,
        "--work-dir",
        wds,
        "--agent",
        "dev@host-a",
        "--initiative",
        "demo",
        "--no-events-hook",
    ];
    let mut st = Scripted::new(&["0||", "0|EVENT 1|", "0||"]);
    assert_eq!(rig.run(&mut st, &extra), 0);
    let t = read(&out);
    assert!(
        t.starts_with(&format!("{}\n", wd.canonicalize().unwrap().display())),
        "{t}"
    );
    let events = rig.state().join("events.txt");
    assert!(
        t.contains(&format!("wake=1 events={}", events.display())),
        "{t}"
    );
    assert!(
        t.contains("arg=-p\narg=--permission-mode\narg=dontAsk\n"),
        "{t}"
    );
    assert!(t.contains("arg=--\narg=Use the crewboard skill"), "{t}");
    assert!(
        events.is_file(),
        "the default events file is in the state dir"
    );
    assert!(
        rig.log().contains("events pending; waking: cd "),
        "{}",
        rig.log()
    );
    // a runtime that cannot start is a failed wake with its reason in the log
    std::fs::remove_file(&bin).unwrap();
    let s = rig.settings(&extra, &[]);
    assert!(s.unwrap_err().text.contains("is gone (moved or removed)"));
}

const MINE12: &str = "demo/app#12                        In Progress      P1  order 1\n";

#[test]
fn tb1_a_wake_over_its_budget_is_stopped_the_coordinator_hears_of_it_and_the_run_exits_0() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let wds = wd.to_str().unwrap();
    let mut st = Scripted::new(&["0||", "0||"]).with_mine(&[(0, MINE12)]);
    let t0 = std::time::Instant::now();
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--agent",
            "dev@host-a",
            "--work-dir",
            wds,
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "the sleep was stopped"
    );
    let say = st
        .calls
        .iter()
        .find(|c| c.starts_with("say "))
        .unwrap()
        .clone();
    assert!(
        say.starts_with("say demo/app#12 anyway=true stopped at the time budget (1s) of the wake wrapper. Ticket demo/app#12, "),
        "{say}"
    );
    assert!(
        say.ends_with(". The next wake continues from here."),
        "{say}"
    );
    let log = rig.log();
    assert!(
        log.contains("wake stopped at the time budget of 1s"),
        "{log}"
    );
    assert!(
        log.contains("said on demo/app#12: stopped at the time budget (1s)"),
        "{log}"
    );
    assert!(
        !log.contains("wake finished OK") && !log.contains("wake command failed"),
        "{log}"
    );
    assert!(
        rig.state_text("last-wake").is_some(),
        "a stopped wake starts the back-off too"
    );
    assert!(!rig.lock().exists());
    // twin (TB2): a wake inside its budget ends by itself: no say
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]).with_mine(&[(0, MINE12)]);
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", "exit 0", "--max-wake", "20s"]),
        0
    );
    assert_eq!(st.count("say"), 0);
    assert!(rig.log().contains("wake finished OK"));
}

#[test]
fn tb7_with_an_events_file_the_unread_events_are_read_before_say_and_kept() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&[
        "0||",
        "0|EVENT 1|",
        "0|MESSAGE from coordinator on #12: please rebase|",
        "0||",
    ])
    .with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    let order: Vec<&str> = st
        .calls
        .iter()
        .map(String::as_str)
        .filter(|c| c.starts_with("poll") || c.starts_with("say"))
        .collect();
    assert_eq!(order.len(), 4, "{order:?}");
    assert_eq!(order[..3], ["poll --quiet", "poll", "poll"], "{order:?}");
    assert!(
        order[3].starts_with("say demo/app#12 anyway=false "),
        "no --override-unread with an events file: {order:?}"
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("== Events read at the time budget end (20"),
        "{kept}"
    );
    assert!(kept.contains("please rebase"), "{kept}");
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.email=t@x",
            "-c",
            "user.name=t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// A checkout with one commit; with `dirty` a changed tracked file and a new file.
fn checkout(dir: &Path, dirty: bool) {
    std::fs::create_dir_all(dir).unwrap();
    git_in(dir, &["init", "-q"]);
    std::fs::write(dir.join("a.txt"), "one\n").unwrap();
    git_in(dir, &["add", "a.txt"]);
    git_in(dir, &["commit", "-q", "-m", "one"]);
    if dirty {
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
    }
}

/// One wake that hits its budget with an events file, then the next run.
fn budget_twice(rig: &Rig, wd: &Path) -> (String, String, Scripted) {
    let evf = ev(rig);
    let wds = wd.to_str().unwrap();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12|", "0||", "0||"])
        .with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-dir",
            wds,
            "--work-backoff",
            "off",
        ],
    );
    assert_eq!(rc, 0);
    let kept = read(&rig.p("ev.txt.unhandled"));
    let say = st
        .calls
        .iter()
        .find(|c| c.starts_with("say "))
        .cloned()
        .unwrap_or_default();
    // the next run: a quiet board, but the kept events wake the agent again
    let cmd = copy_events(rig, "seen");
    let mut next = Scripted::new(&["1||", "1||"]);
    assert_eq!(
        rig.run(
            &mut next,
            &[
                "--wake-cmd",
                &cmd,
                "--events-file",
                &evf,
                "--work-backoff",
                "off",
                "--max-wake",
                "off"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1, "the next tick wakes the agent again");
    (kept, say, next)
}

#[test]
fn a_wake_at_its_budget_keeps_its_events_and_names_uncommitted_work() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, true);
    let head = git_in(&wd, &["rev-parse", "HEAD"]);
    let porcelain = git_in(&wd, &["status", "--porcelain"]);
    let (kept, say, _) = budget_twice(&rig, &wd);
    let header = format!(
        "The last wake hit the time budget. Uncommitted work in {}: commit or continue it first.",
        wd.display()
    );
    assert_eq!(kept.lines().next(), Some(header.as_str()), "{kept}");
    assert!(
        kept.contains("EVENT 1: assign #12"),
        "the events file of the wake is kept: {kept}"
    );
    assert!(
        say.contains(&format!(
            " Uncommitted work in {}. The next wake continues from here.",
            wd.display()
        )),
        "{say}"
    );
    let seen = read(&rig.p("seen"));
    assert!(
        seen.contains(&header) && seen.contains("EVENT 1: assign #12"),
        "{seen}"
    );
    // the wrapper reads the tree and changes nothing: no commit, no staging
    assert_eq!(git_in(&wd, &["rev-parse", "HEAD"]), head);
    assert_eq!(git_in(&wd, &["status", "--porcelain"]), porcelain);
    assert_eq!(std::fs::read_to_string(wd.join("a.txt")).unwrap(), "two\n");
}

#[test]
fn a_wake_at_its_budget_with_a_clean_tree_keeps_its_events_without_the_header() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, false);
    let (kept, say, _) = budget_twice(&rig, &wd);
    assert!(!kept.contains("Uncommitted work"), "{kept}");
    assert!(kept.contains("EVENT 1: assign #12"), "{kept}");
    assert!(!say.contains("Uncommitted"), "{say}");
    assert!(say.contains("Ticket demo/app#12, "), "{say}");
    // a work dir that is no checkout has no header either
    let rig = Rig::new();
    let plain = rig.p("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let (kept, _, _) = budget_twice(&rig, &plain);
    assert!(!kept.contains("Uncommitted work"), "{kept}");
}

#[test]
fn only_a_checkout_of_its_own_counts_not_a_dir_inside_one() {
    let d = tempfile::tempdir().unwrap();
    checkout(d.path(), true);
    std::fs::create_dir_all(d.path().join("sub")).unwrap();
    let dirty = describe_head::dirty_checkouts(d.path(), &mut |_| {});
    assert_eq!(dirty, [d.path().to_path_buf()]);
    // a clone under a plain work dir
    let parent = tempfile::tempdir().unwrap();
    checkout(&parent.path().join("app"), true);
    checkout(&parent.path().join("lib"), false);
    assert_eq!(
        describe_head::dirty_checkouts(parent.path(), &mut |_| {}),
        [parent.path().join("app")]
    );
    assert!(describe_head::dirty_checkouts(&parent.path().join("nope"), &mut |_| {}).is_empty());
}

/// One run of a wake that ends at its 1 s budget on ticket 12, with an events file.
fn budget_stop(rig: &Rig, plan: &[&str], backoff: &str) -> Scripted {
    let evf = ev(rig);
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let wds = wd.to_string_lossy().to_string();
    let mut st = Scripted::new(plan).with_mine(&[(0, MINE12), (0, MINE12)]);
    let rc = rig.run(
        &mut st,
        &[
            "--wake-cmd",
            "sleep 30",
            "--max-wake",
            "1s",
            "--events-file",
            &evf,
            "--agent",
            "dev@host-a",
            "--work-dir",
            &wds,
            "--work-backoff",
            backoff,
        ],
    );
    assert_eq!(rc, 0);
    st
}

fn said(st: &Scripted) -> String {
    st.calls
        .iter()
        .find(|c| c.starts_with("say "))
        .cloned()
        .unwrap_or_default()
}

const TWICE: &str = "hit the time budget twice in a row: split the ticket or raise --max-wake";

#[test]
fn the_second_budget_stop_in_a_row_waits_for_the_backoff_and_tells_the_coordinator() {
    let rig = Rig::new();
    // first stop: events kept, no streak line
    let st = budget_stop(
        &rig,
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        "600",
    );
    assert!(!said(&st).contains(TWICE), "{}", said(&st));
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("1 demo/app#12\n")
    );
    // the board is quiet: the kept events wake the agent at once (the back-off is not over)
    let st = budget_stop(&rig, &["1||", "1||", "1||", "0||"], "600");
    assert_eq!(st.count("mine --quiet"), 0, "woken by the kept events");
    let body = said(&st);
    assert!(body.contains(TWICE), "{body}");
    assert!(body.contains("stopped at the time budget (1s)"), "{body}");
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("2 demo/app#12\n")
    );
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("EVENT 1: assign #12"),
        "events are kept: {kept}"
    );
    assert_eq!(kept.matches("The last wake hit the time budget").count(), 0);
    // third run, inside the back-off: a quiet board and no wake, the events stay
    let evf = ev(&rig);
    let mut quiet = Scripted::new(&["1||"]);
    let wake = rig.wake_cmd();
    assert_eq!(
        rig.run(
            &mut quiet,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 0, "no wake inside the back-off");
    assert!(
        rig.p("ev.txt.unhandled").is_file(),
        "the events are still kept"
    );
    // after the back-off the agent wakes, and a wake that ends by itself ends the streak
    rig.clock.advance(601);
    let mut due = Scripted::new(&["1||", "1||"]).with_mine(&[(0, MINE12), (0, MINE12)]);
    assert_eq!(
        rig.run(
            &mut due,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1);
    assert_eq!(rig.state_text("budget-streak"), None);
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the events went to that wake"
    );
}

#[test]
fn a_budget_stop_on_another_ticket_starts_the_count_again() {
    let rig = Rig::new();
    rig.write_state("budget-streak", "1 demo/app#99\n");
    let st = budget_stop(
        &rig,
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        "off",
    );
    assert!(!said(&st).contains(TWICE), "{}", said(&st));
    assert_eq!(
        rig.state_text("budget-streak").as_deref(),
        Some("1 demo/app#12\n")
    );
}

#[test]
fn new_events_wake_the_agent_even_while_the_streak_holds_the_kept_events() {
    let rig = Rig::new();
    rig.write_state("budget-streak", "2 demo/app#12\n");
    std::fs::write(rig.p("ev.txt.unhandled"), "old kept\n").unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 2: message|"]).with_mine(&[(0, MINE12)]);
    let wake = copy_events(&rig, "seen");
    assert_eq!(
        rig.run(
            &mut st,
            &[
                "--wake-cmd",
                &wake,
                "--events-file",
                &evf,
                "--work-backoff",
                "600"
            ]
        ),
        0
    );
    assert_eq!(rig.wakes(), 1);
    let seen = read(&rig.p("seen"));
    assert!(
        seen.contains("old kept") && seen.contains("EVENT 2: message"),
        "{seen}"
    );
}

#[test]
fn the_header_of_an_earlier_budget_stop_is_dropped_so_the_new_one_is_first_and_alone() {
    let old =
        "The last wake hit the time budget. Uncommitted work in /a: commit or continue it first.";
    let new = "The last wake hit the time budget. Uncommitted work in /a, /b: commit or continue it first.";
    let text = format!(
        "== Events kept from an earlier wake:\n{old}\nEVENT 1\n\n== Events (crewboard poll, t):\nEVENT 2\n"
    );
    let out = String::from_utf8(with_budget_header(text.as_bytes(), Some(new))).unwrap();
    assert!(out.starts_with(&format!("{new}\n== Events kept")), "{out}");
    assert_eq!(
        out.matches("The last wake hit the time budget").count(),
        1,
        "{out}"
    );
    assert!(out.contains("EVENT 1") && out.contains("EVENT 2"));
    // the same header again stays once
    let again = String::from_utf8(with_budget_header(out.as_bytes(), Some(new))).unwrap();
    assert_eq!(again, out);
    // a clean tree drops the stale header and adds none
    let clean = String::from_utf8(with_budget_header(out.as_bytes(), None)).unwrap();
    assert!(!clean.contains("hit the time budget"), "{clean}");
    // bytes that are not UTF-8 pass through
    assert_eq!(with_budget_header(b"a\xffb\n", None), b"a\xffb\n");
}

#[test]
fn the_events_file_of_a_second_stop_keeps_the_header_once_and_first() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    checkout(&wd, true);
    let evf = ev(&rig);
    let wds = wd.to_str().unwrap();
    for plan in [
        &["0||", "0|EVENT 1: assign #12|", "0||", "0||"],
        &["1||", "1||", "1||", "0||"],
    ] {
        let mut st = Scripted::new(plan).with_mine(&[(0, MINE12), (0, MINE12)]);
        let rc = rig.run(
            &mut st,
            &[
                "--wake-cmd",
                "sleep 30",
                "--max-wake",
                "1s",
                "--events-file",
                &evf,
                "--agent",
                "dev@host-a",
                "--work-dir",
                wds,
                "--work-backoff",
                "600",
            ],
        );
        assert_eq!(rc, 0);
    }
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert_eq!(
        kept.matches("The last wake hit the time budget").count(),
        1,
        "{kept}"
    );
    assert!(
        kept.starts_with("The last wake hit the time budget."),
        "{kept}"
    );
    assert!(kept.contains("EVENT 1: assign #12"), "{kept}");
}

#[test]
fn a_checkout_where_git_status_fails_is_a_log_line_not_a_clean_tree() {
    let d = tempfile::tempdir().unwrap();
    checkout(d.path(), false);
    std::fs::write(d.path().join(".git/index"), "not an index").unwrap();
    let mut lines = Vec::new();
    let dirty = describe_head::dirty_checkouts(d.path(), &mut |l| lines.push(l.to_string()));
    assert!(dirty.is_empty());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("git status failed"), "{lines:?}");
    // twin: a sound checkout logs nothing
    let ok = tempfile::tempdir().unwrap();
    checkout(ok.path(), true);
    let mut none = Vec::new();
    assert_eq!(
        describe_head::dirty_checkouts(ok.path(), &mut |l| none.push(l.to_string())).len(),
        1
    );
    assert!(none.is_empty(), "{none:?}");
}

#[test]
fn a_hidden_dir_under_the_work_dir_is_no_checkout() {
    let parent = tempfile::tempdir().unwrap();
    checkout(&parent.path().join(".cache"), true);
    checkout(&parent.path().join("app"), true);
    assert_eq!(
        describe_head::dirty_checkouts(parent.path(), &mut |_| {}),
        [parent.path().join("app")]
    );
}

// --- stopping the wake ---

/// A process table that a test scripts, and the signals that were sent to it.
struct FakeOs {
    tables: RefCell<std::collections::VecDeque<Vec<Proc>>>,
    group_members: Cell<bool>,
    sent: RefCell<Vec<String>>,
}

impl FakeOs {
    fn new(tables: Vec<Vec<Proc>>, group_members: bool) -> FakeOs {
        FakeOs {
            tables: RefCell::new(tables.into()),
            group_members: Cell::new(group_members),
            sent: RefCell::new(Vec::new()),
        }
    }
    fn sent(&self) -> Vec<String> {
        self.sent.borrow().clone()
    }
}

impl Os for FakeOs {
    fn snapshot(&self, _: &dyn Fn(&str)) -> Vec<Proc> {
        let mut t = self.tables.borrow_mut();
        if t.len() > 1 {
            t.pop_front().unwrap()
        } else {
            t.front().cloned().unwrap_or_default()
        }
    }
    fn signal_group(&self, _: u32, sig: i32) {
        self.sent.borrow_mut().push(format!("group {sig}"));
    }
    fn signal_pid(&self, pid: u32, sig: i32) {
        self.sent.borrow_mut().push(format!("pid {pid} {sig}"));
    }
    fn group_alive(&self, _: u32) -> bool {
        self.group_members.get()
    }
}

fn proc_row(pid: u32, ppid: u32, pgid: u32, started: &str) -> Proc {
    Proc {
        pid,
        ppid,
        pgid,
        started: started.into(),
    }
}

/// A child that has already ended, in a group of its own.
fn ended_child() -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new("sh");
    c.args(["-c", "exit 0"]);
    c.process_group(0);
    c.spawn().unwrap()
}

/// The table of a wake whose leader is `leader`: one grandchild in a session of its own.
fn tree(leader: u32, started: &str) -> Vec<Proc> {
    vec![proc_row(4242, leader, 4242, started)]
}

#[test]
fn when_everything_ends_on_term_no_kill_is_sent() {
    let mut child = ended_child();
    let leader = child.id();
    // a member of the wake's own group (5000) has the TERM from the group signal only
    let mut table = tree(leader, "Sat Oct 24 10:00:00 2026");
    table.push(proc_row(5000, leader, leader, "Sat Oct 24 10:00:01 2026"));
    let os = FakeOs::new(vec![table, Vec::new()], false);
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_secs(5));
    let sent = os.sent();
    assert!(
        sent.iter()
            .all(|s| s.ends_with(&format!(" {}", libc::SIGTERM))),
        "{sent:?}"
    );
    assert!(
        sent.contains(&format!("pid 4242 {}", libc::SIGTERM)),
        "{sent:?}"
    );
    assert!(!sent.iter().any(|s| s.starts_with("pid 5000")), "{sent:?}");
}

#[test]
fn a_pid_that_another_process_took_is_not_killed_one_that_is_the_same_is() {
    // the grandchild ignores TERM: the same process (same start time) gets the KILL
    let mut child = ended_child();
    let leader = child.id();
    let t = tree(leader, "Sat Oct 24 10:00:00 2026");
    let os = FakeOs::new(vec![t.clone(), t.clone()], false);
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    assert!(
        os.sent().contains(&format!("pid 4242 {}", libc::SIGKILL)),
        "{:?}",
        os.sent()
    );
    // a stranger has the pid now (other start time): no KILL to it, none to the group
    let mut child = ended_child();
    let leader = child.id();
    let stranger = tree(leader, "Sat Oct 24 10:05:00 2026");
    let os = FakeOs::new(
        vec![tree(leader, "Sat Oct 24 10:00:00 2026"), stranger],
        false,
    );
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    let sent = os.sent();
    assert!(
        !sent
            .iter()
            .any(|s| s.ends_with(&format!(" {}", libc::SIGKILL))),
        "{sent:?}"
    );
    // a pgid number that a stranger group took after the leader was reaped: it has no process
    // of the first scan, so the group gets no KILL either
    let mut child = ended_child();
    let leader = child.id();
    let os = FakeOs::new(
        vec![tree(leader, "A"), vec![proc_row(9999, 1, leader, "B")]],
        true,
    );
    stop_group_with(&mut child, &|_| {}, &os, Duration::from_millis(100));
    let sent = os.sent();
    assert!(
        !sent
            .iter()
            .any(|s| s.ends_with(&format!(" {}", libc::SIGKILL))),
        "{sent:?}"
    );
}

#[test]
fn the_snapshot_keeps_the_start_time_and_a_short_row_has_none() {
    let t = procs_of("  10     1    10 Sat Oct 24 10:00:00 2026\n11 10\nx y\n");
    assert_eq!(t.len(), 2);
    assert_eq!((t[0].pid, t[0].ppid, t[0].pgid), (10, 1, 10));
    assert_eq!(t[0].started, "Sat Oct 24 10:00:00 2026");
    assert_eq!((t[1].pgid, t[1].started.as_str()), (0, ""));
    // a row without a start time is never "the same process"
    assert!(same_processes(&t[1..], &t[1..]).is_empty());
    assert_eq!(same_processes(&t[..1], &t[..1]), [10]);
}

// --- host locks ---

fn lock_dir(rig: &Rig, name: &str, owner: &str, pid: u32, extra: &str) -> PathBuf {
    let d = rig.p(name);
    std::fs::create_dir_all(&d).unwrap();
    let host = crate::lock::sys::short_hostname().unwrap_or_else(|| "host".to_string());
    std::fs::write(
        d.join("owner"),
        format!("agent={owner}\nhost={host}\npid={pid}\nstarted=x\nstarted_epoch={NOW}\nreason=r\n{extra}"),
    )
    .unwrap();
    d
}

fn release_run(rig: &Rig, locks: Vec<PathBuf>, agent: Option<&str>) {
    let mut s = rig.settings(&[], &[]).unwrap();
    s.host_locks = locks;
    s.agent = agent.map(str::to_string);
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&[]);
    let flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    flow.release_host_locks();
}

#[test]
fn the_host_locks_of_the_agent_are_released_except_a_running_or_detached_job_and_a_stranger() {
    let rig = Rig::new();
    let gone = lock_dir(&rig, "gone.lock.d", "dev@host-a", dead_pid(), "");
    let running = lock_dir(&rig, "running.lock.d", "dev@host-a", std::process::id(), "");
    let detached = lock_dir(
        &rig,
        "detached.lock.d",
        "dev@host-a",
        dead_pid(),
        "detached=1\n",
    );
    let other = lock_dir(&rig, "other.lock.d", "other@host-b", std::process::id(), "");
    release_run(
        &rig,
        vec![
            gone.clone(),
            running.clone(),
            detached.clone(),
            other.clone(),
        ],
        Some("dev@host-a"),
    );
    assert!(!gone.exists(), "released: {}", rig.log());
    assert!(running.exists() && detached.exists(), "kept: {}", rig.log());
    assert!(other.exists(), "not ours: {}", rig.log());
    let log = rig.log();
    assert!(log.contains("running.lock.d: kept for dev@host-a"), "{log}");
    assert!(
        log.contains("detached.lock.d: kept for dev@host-a"),
        "{log}"
    );
    assert!(
        log.contains("gone.lock.d: released for dev@host-a"),
        "{log}"
    );
    assert!(log.contains("other.lock.d: not released"), "{log}");
    // without an agent nothing is released
    let rig = Rig::new();
    let gone = lock_dir(&rig, "gone.lock.d", "dev@host-a", dead_pid(), "");
    release_run(&rig, vec![gone.clone()], None);
    assert!(gone.exists());
    assert!(
        rig.log()
            .contains("host locks not released: --agent is not set")
    );
}

#[test]
fn a_signal_during_the_wake_stops_it_and_releases_the_host_locks() {
    let rig = Rig::new();
    let marker = rig.p("started");
    let wake = format!("touch '{}'; sleep 30", marker.display());
    let mut s = rig.settings(&["--wake-cmd", &wake], &[]).unwrap();
    let lock = lock_dir(&rig, "held.lock.d", "dev@host-a", dead_pid(), "");
    s.host_locks = vec![lock.clone()];
    s.agent = Some("dev@host-a".into());
    let t0 = std::time::Instant::now();
    let rc = rig.run_signals(&mut Scripted::new(&["0||"]), &s, &|| {
        marker.exists().then_some(libc::SIGTERM)
    });
    assert_eq!(rc, 128 + u8::try_from(libc::SIGTERM).unwrap());
    assert!(
        t0.elapsed() < Duration::from_secs(20),
        "the sleep was stopped"
    );
    assert!(!lock.exists(), "released on the signal path: {}", rig.log());
    assert!(!rig.lock().exists());
}

#[test]
fn no_in_progress_ticket_is_one_log_line_and_no_message() {
    let rig = Rig::new();
    // `mine` exits 1 when no owned ticket needs the agent now, and still prints the rows
    let mut st = Scripted::new(&["0||"]).with_mine(&[(1, MINE12)]);
    let rc = rig.run(&mut st, &["--wake-cmd", "sleep 30", "--max-wake", "1s"]);
    assert_eq!(rc, 0);
    assert_eq!(st.count("say"), 0);
    assert!(
        rig.log()
            .contains("no In Progress ticket: no message to the coordinator"),
        "{}",
        rig.log()
    );
    // a say that fails is logged with its exit code
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "2||refused [rule=x]: no"]).with_mine(&[(0, MINE12)]);
    assert_eq!(
        rig.run(&mut st, &["--wake-cmd", "sleep 30", "--max-wake", "1s"]),
        0
    );
    assert!(
        rig.log()
            .contains("say on demo/app#12 failed: exit 2: refused [rule=x]: no"),
        "{}",
        rig.log()
    );
}

#[test]
fn tb3_a_bad_budget_or_heartbeat_is_exit_4_and_logged() {
    let rig = Rig::new();
    for (flag, bad_value, text) in [
        (
            "--max-wake",
            "soon",
            "--max-wake needs a duration (45m, 90s, 2h) or off, got 'soon'",
        ),
        ("--max-wake", "1m30s", "--max-wake needs a duration"),
        (
            "--heartbeat-every",
            "x",
            "--heartbeat-every needs whole seconds, got 'x'",
        ),
        (
            "--heartbeat-every",
            "-1",
            "--heartbeat-every needs whole seconds",
        ),
        (
            "--heartbeat-every",
            "+5",
            "--heartbeat-every needs whole seconds",
        ),
    ] {
        let e = rig.settings(&[flag, bad_value], &[]).unwrap_err();
        assert!(e.text.contains(text), "{flag} {bad_value}: {}", e.text);
        assert_eq!(e.code, 4);
        assert!(e.paths.is_some(), "the line goes to the log");
    }
    let s = rig.settings(&["--max-wake", "off"], &[]).unwrap();
    assert_eq!((s.max_wake.secs, s.max_wake.text.as_str()), (0, "off"));
    let s = rig.settings(&["--max-wake", "2h"], &[]).unwrap();
    assert_eq!(s.max_wake.secs, 7200);
    // the default is 45m and 300 s; the environment comes before it and the flag before that
    let s = rig.settings(&[], &[]).unwrap();
    assert_eq!((s.max_wake.secs, s.heartbeat_every), (2700, 300));
    let s = rig
        .settings(
            &["--heartbeat-every", "7"],
            &[
                ("CREWBOARD_MAX_WAKE", "90s"),
                ("CREWBOARD_HEARTBEAT_EVERY", "9"),
            ],
        )
        .unwrap();
    assert_eq!((s.max_wake.secs, s.heartbeat_every), (90, 7));
    let s = rig
        .settings(&[], &[("CREWBOARD_HEARTBEAT_EVERY", "9")])
        .unwrap();
    assert_eq!(s.heartbeat_every, 9);
}

/// Counts the heartbeats of the thread of a wake; the other calls do not happen.
struct Beats(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Steps for Beats {
    fn heartbeat(&mut self) -> StepRc {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        StepRc::ok()
    }
    fn poll(&mut self, _: bool, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn mine_quiet(&mut self) -> StepRc {
        unreachable!()
    }
    fn mine_text(&mut self, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn say(&mut self, _: &str, _: &str, _: bool) -> StepRc {
        unreachable!()
    }
    fn tick(&mut self, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn acted_list(&mut self, _: &Path, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
        unreachable!()
    }
    fn pr_head(&mut self, _: &str, _: &str) -> Result<Option<(String, String)>, StepRc> {
        unreachable!()
    }
    fn open_prs(&mut self, _: &str) -> Result<Vec<(String, String, String)>, StepRc> {
        unreachable!()
    }
}

#[test]
fn p12_a_long_wake_sends_heartbeats_from_its_own_thread_and_none_after_it() {
    let beats = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = || beats.load(std::sync::atomic::Ordering::SeqCst);
    let run = |every: &str, cmd: &str| {
        let rig = Rig::new();
        let s = rig
            .settings(&["--wake-cmd", cmd, "--heartbeat-every", every], &[])
            .unwrap();
        std::fs::create_dir_all(&s.paths.dir).unwrap();
        let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
        let mut st = Scripted::new(&["0||"]);
        let b = beats.clone();
        let make = move || -> Box<dyn Steps> { Box::new(Beats(b.clone())) };
        let rc = Flow {
            s: &s,
            steps: &mut st,
            clock: &rig.clock,
            pids: &SystemProcs,
            log: &log,
            host: "testhost",
            sig: Cell::new(None),
            signals: &|| None,
            heartbeat_steps: Some(&make),
        }
        .execute();
        assert_eq!(rc, 0);
    };
    run("1", "sleep 2.5");
    let during = count();
    assert!(
        (1..=3).contains(&during),
        "heartbeats during a 2.5 s wake: {during}"
    );
    // a real wait: "no beat after the wake" has no event to wait for; a stray thread would beat
    // within this 1.5 s (interval 1 s)
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(count(), during, "none after the wake");
    // a fast wake sends none; `--heartbeat-every 0` sends none at all
    run("5", "exit 0");
    run("0", "sleep 1.5");
    assert_eq!(count(), during);
}

/// The descendants of `root` in a snapshot of `ps -axo pid=,ppid=` (lines `<pid> <ppid>`), the
/// children first as the snapshot lists them. `root` itself is not in the result.
fn descendants(snapshot: &str, root: u32) -> Vec<u32> {
    descendant_procs(&procs_of(snapshot), root)
        .into_iter()
        .map(|p| p.pid)
        .collect()
}

#[test]
fn descendants_come_from_a_ps_snapshot() {
    let ps = "  1     0\n 10     1\n 11    10\n 12    11\n 13    10\n 20     1\n 21    20\n junk\n";
    let mut d = descendants(ps, 10);
    d.sort_unstable();
    assert_eq!(d, [11, 12, 13]);
    assert!(descendants(ps, 12).is_empty());
    assert!(descendants("", 10).is_empty());
    // a loop in the table ends
    assert_eq!(descendants("5 6\n6 5\n", 5), [6]);
}

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

/// The child command of a wake with an env file that tries to replace what the wake sets.
#[test]
fn the_env_file_cannot_replace_what_the_wake_sets() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let shim = rig.p("agent/bin");
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::create_dir_all(rig.p("agent/gh-empty-config")).unwrap();
    let gh = shim.join("gh");
    std::fs::write(&gh, "#!/bin/sh\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let file = rig.p("agent.env");
    std::fs::write(
        &file,
        "GH_TOKEN=tok-in-file\nGH_CONFIG_DIR=/evil\nCREWBOARD_LOCK_RUN_DIRS=/etc\nCREWBOARD_WAKE=0\nCREWBOARD_EVENTS_FILE=/evil\nCREWBOARD_WAKE_REASON=evil\nCREWBOARD_RESULTS_DIR=/evil\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let loaded = crate::agent_env::load(&rig.p("cfg"), Some(&file)).unwrap();
    let lockdir = rig.p("locks/heavy.lock.d");
    let s = rig
        .settings(
            &[
                "--runtime",
                "claude",
                "--no-events-hook",
                "--agent",
                "dev@host-a",
                "--initiative",
                "demo",
                "--work-dir",
                wd.to_str().unwrap(),
                "--shim-dir",
                shim.to_str().unwrap(),
                "--host-lock",
                lockdir.to_str().unwrap(),
            ],
            &[("PATH", "/usr/bin:/bin")],
        )
        .unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let mut st = Scripted::new(&[]);
    let flow = Flow {
        s: &s,
        steps: &mut st,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    let c = flow.wake_child("", Reason::Events, Some(&loaded));
    let envs: std::collections::HashMap<String, Option<String>> = c
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    let get = |k: &str| envs.get(k).cloned().flatten();
    assert_eq!(get("CREWBOARD_WAKE").as_deref(), Some("1"));
    assert_eq!(
        get("CREWBOARD_LOCK_RUN_DIRS").as_deref(),
        Some(lockdir.to_str().unwrap())
    );
    assert_eq!(
        get("GH_CONFIG_DIR").as_deref(),
        Some(rig.p("agent/gh-empty-config").to_str().unwrap())
    );
    assert_eq!(get("CREWBOARD_WAKE_REASON").as_deref(), Some("events"));
    assert_eq!(
        get("CREWBOARD_RESULTS_DIR"),
        Some(s.paths.results.to_string_lossy().into_owned())
    );
    assert_eq!(
        get("CREWBOARD_EVENTS_FILE"),
        Some(
            rig.state()
                .join("events.txt")
                .to_string_lossy()
                .into_owned()
        )
    );
    assert!(
        get("PATH")
            .unwrap()
            .starts_with(&format!("{}:", shim.display())),
        "{:?}",
        get("PATH")
    );
    assert_eq!(c.get_current_dir(), Some(wd.as_path()));
    // a token variable of the caller is removed from the child when an env file is in force
    let plain = rig.p("plain.env");
    std::fs::write(&plain, "GH_TOKEN=tok-plain\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let loaded2 = crate::agent_env::load(&rig.p("cfg"), Some(&plain)).unwrap();
    let c3 = flow.wake_child("", Reason::Events, Some(&loaded2));
    assert!(
        c3.get_envs()
            .any(|(k, v)| k == "GITHUB_TOKEN" && v.is_none()),
        "GITHUB_TOKEN of the caller is removed from the child"
    );
    // twin: a plain `sh -c` wake has none of the agent variables
    let s2 = rig.settings(&[], &[]).unwrap();
    let mut st2 = Scripted::new(&[]);
    let flow2 = Flow {
        s: &s2,
        steps: &mut st2,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    };
    let c2 = flow2.wake_child("true", Reason::Work, None);
    assert!(
        c2.get_envs()
            .all(|(k, _)| k != "CREWBOARD_WAKE" && k != "GH_CONFIG_DIR")
    );
    assert_eq!(c2.get_program(), "sh");
}

#[test]
fn relative_paths_of_the_state_are_made_absolute_for_the_child() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let s = rig
        .settings(
            &[
                "--runtime",
                "hermes",
                "--agent",
                "dev@host-a",
                "--initiative",
                "demo",
                "--work-dir",
                wd.to_str().unwrap(),
                "--state-dir",
                "rel/state",
                "--log",
                "rel/wake.log",
                "--events-file",
                "rel/ev.txt",
            ],
            &[],
        )
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(s.paths.dir, cwd.join("rel/state"));
    assert_eq!(s.paths.log, cwd.join("rel/wake.log"));
    assert_eq!(s.events_file, Some(cwd.join("rel/ev.txt")));
    let prompt = s.direct.unwrap().argv.pop().unwrap();
    assert!(
        prompt.contains(&format!(
            "Your events are in {}",
            cwd.join("rel/ev.txt").display()
        )),
        "{prompt}"
    );
}

#[test]
fn an_env_file_that_cannot_be_loaded_is_exit_4_each_run_and_leaves_no_stop_file() {
    let rig = Rig::new();
    let mut s = rig.settings(&[], &[]).unwrap();
    s.env_load_failed = true;
    let msg = "4||env file /x/a.env has mode 644; group and others must have no access. Run: chmod 600 /x/a.env";
    let mut st = Scripted::new(&[msg]);
    assert_eq!(rig.run_settings(&mut st, &s), 4);
    assert_eq!(rig.state_text("stopped"), None, "chmod could not lift it");
    let log = rig.log();
    assert!(
        log.contains("has mode 644") && log.contains("fix it"),
        "{log}"
    );
    assert_eq!(st.count("say"), 0, "nobody can be told with this file");
    // the next run, after the fix: it polls again at once
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rig.run_settings(&mut st, &s), 0);
    assert_eq!(st.count("poll --quiet"), 1);
    // twin: a failure of another kind stops the wakes, as before
    let rig = Rig::new();
    let mut st = Scripted::new(&["4||bad credentials"]).with_mine(&[(1, String::new().as_str())]);
    assert_eq!(rig.run(&mut st, &[]), 4);
    assert!(rig.state_text("stopped").is_some());
}

#[test]
fn print_cmd_is_a_plan_not_a_run() {
    let rig = Rig::new();
    let wd = rig.p("wd");
    std::fs::create_dir_all(&wd).unwrap();
    let a = parse(&[
        "--runtime",
        "hermes",
        "--work-dir",
        wd.to_str().unwrap(),
        "--agent",
        "dev@host-a",
        "--initiative",
        "demo",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    match resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap() {
        Plan::PrintCmd(t) => {
            assert!(
                t.starts_with(&format!(
                    "cd {} && hermes chat -Q -s crewboard -q 'Use the crewboard skill",
                    wd.display()
                )),
                "{t}"
            );
        }
        _ => panic!("not a print"),
    }
    // the tick line of a headless coordinator
    let a = parse(&[
        "--tick",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    match resolve(&a, &GlobalFlags::default(), &|_| None, None).unwrap() {
        Plan::PrintCmd(t) => assert!(
            t.starts_with("crewboard tick (no model; the lines go to "),
            "{t}"
        ),
        _ => panic!("not a print"),
    }
    // --print-cmd without a command to print is a wrong flag
    let a = parse(&[
        "--wake-cmd",
        "true",
        "--state-dir",
        rig.state().to_str().unwrap(),
        "--print-cmd",
    ]);
    assert!(
        resolve(&a, &GlobalFlags::default(), &|_| None, None)
            .unwrap_err()
            .text
            .contains("--print-cmd needs")
    );
}

#[test]
fn the_kept_events_stay_when_the_new_file_cannot_be_written() {
    let rig = Rig::new();
    // a directory where the events file should go: the rename fails
    std::fs::create_dir_all(rig.p("ev.txt")).unwrap();
    std::fs::write(rig.p("ev.txt.unhandled"), "== kept\nEVENT 7\n").unwrap();
    std::fs::create_dir_all(rig.state().join("results")).unwrap();
    std::fs::write(
        rig.state().join("results/7.result"),
        "rc=0 end=2026-10-21T10:00:00Z log=l\nticket=7\n",
    )
    .unwrap();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert_eq!(rig.wakes(), 0, "no wake without its events");
    assert!(
        rig.log().contains("cannot write the events file "),
        "{}",
        rig.log()
    );
    assert_eq!(
        read(&rig.p("ev.txt.unhandled")),
        "== kept\nEVENT 7\n",
        "nothing is lost"
    );
    assert!(
        !rig.state().join("results/7.result.seen").exists(),
        "the result is not marked"
    );
    assert!(rig.has_no_temp_of("ev.txt"));
    assert!(!rig.lock().exists());
    // twin: a file that can be written removes `.unhandled` and marks the result
    std::fs::remove_dir_all(rig.p("ev.txt")).unwrap();
    let mut st = Scripted::new(&["0||", "0|EVENT 8|"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 0);
    assert!(!rig.p("ev.txt.unhandled").exists());
    assert!(rig.state().join("results/7.result.seen").exists());
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("EVENT 7") && t.contains("detached job finished: ticket 7 rc=0 "),
        "{t}"
    );
}

#[test]
fn a_poll_that_fails_for_the_events_file_is_told_to_the_model_not_fatal() {
    let rig = Rig::new();
    let evf = ev(&rig);
    // poll --quiet says 0; the second poll fails with 2; mine fails with 2
    let mut st = Scripted::new(&["0||", "2||refused something"]).with_mine(&[(2, "")]);
    assert_eq!(
        rig.run(&mut st, &["--events-file", &evf, "--wake-cmd", "true"]),
        0
    );
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains(
            "(crewboard poll failed with exit 2: run crewboard watch --timeout 1m yourself)"
        ),
        "{t}"
    );
    assert!(t.contains("(crewboard mine failed with exit 2)"), "{t}");
    assert!(
        rig.log()
            .contains("crewboard poll for the events file failed: exit 2: refused something")
    );
}

#[test]
fn exit_4_while_the_events_file_is_built_stops_the_wakes() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let mut st = Scripted::new(&["0||", "4||key gone"]);
    assert_eq!(rig.run(&mut st, &["--events-file", &evf]), 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" poll: key gone")
    );
    assert!(!rig.p("ev.txt").exists() && rig.has_no_temp_of("ev.txt"));
    assert_eq!(rig.wakes(), 0);
}

fn acw(rig: &Rig, st: &mut Scripted, cmd: &str) -> u8 {
    let evf = ev(rig);
    rig.run(
        st,
        &[
            "--events-file",
            &evf,
            "--wake-cmd",
            cmd,
            "--work-backoff",
            "off",
        ],
    )
}

#[test]
fn ac1_receipts_are_logged_and_a_wake_that_acted_gets_no_redelivery() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: assign #12|"]);
    st.acted_list = "demo/app#12 assignment\ndemo/app#7 info\nowner-message\n".into();
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let lg = rig.log();
    for r in ["demo/app#12 assignment", "demo/app#7 info", "owner-message"] {
        assert!(
            lg.contains(&format!("poll-and-wake: event delivered to wake: {r}\n")),
            "{lg}"
        );
    }
    assert!(!lg.contains("not acted on"));
    assert!(!rig.p("ev.txt.unhandled").exists());
    assert_eq!(st.acted_calls.len(), 2);
    let evf = ev(&rig);
    assert!(
        st.acted_calls[1].starts_with(&format!("--events-file {evf} --since 20")),
        "{:?}",
        st.acted_calls
    );
    assert!(st.acted_calls[1].ends_with(&format!(" --redeliver-to {evf}.unhandled")));
}

#[test]
fn ac2_one_miss_is_delivered_again_once_and_a_second_miss_only_logs() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|", "1||", "1||"]);
    st.acted_list = "demo/app#12 question\n".into();
    st.acted_check = vec!["demo/app#12 question".into()];
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert_eq!(
        kept.matches("REDELIVERED demo/app#12 question").count(),
        1,
        "{kept}"
    );
    assert!(
        rig.log()
            .contains("poll-and-wake: event not acted on: demo/app#12 question; delivered again\n")
    );
    // the next wake gets it at the top of its file; a second miss only logs
    let cmd = copy_events(&rig, "seen");
    st.plan.push_back((0, String::new(), String::new()));
    assert_eq!(acw(&rig, &mut st, &cmd), 0);
    let seen = read(&rig.p("seen"));
    assert!(seen.starts_with(crate::wake::events::EARLIER), "{seen}");
    assert!(
        seen.lines()
            .any(|l| l == "REDELIVERED demo/app#12 question"),
        "{seen}"
    );
    let lg = rig.log();
    assert!(
        lg.contains(
            "poll-and-wake: event not acted on twice: demo/app#12 question; not delivered again\n"
        ),
        "{lg}"
    );
    assert_eq!(
        lg.matches("event not acted on: ").count(),
        1,
        "the first miss is logged once"
    );
    assert!(
        !rig.p("ev.txt.unhandled").exists(),
        "the second miss delivers nothing again"
    );
}

#[test]
fn ac3_a_failing_wake_keeps_the_whole_file_and_runs_no_check() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|"]);
    st.acted_list = "demo/app#12 question\n".into();
    st.acted_check = vec!["demo/app#12 question".into()];
    assert_eq!(acw(&rig, &mut st, "exit 9"), 70);
    let kept = read(&rig.p("ev.txt.unhandled"));
    assert!(
        kept.contains("EVENT 1: ask on #12") && !kept.contains("REDELIVERED"),
        "{kept}"
    );
    assert_eq!(st.acted_calls.len(), 1, "only the receipts, no check");
    assert!(!rig.log().contains("not acted on"));
}

#[test]
fn ac5_a_check_that_fails_is_logged_and_changes_nothing() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||", "0|EVENT 1: ask on #12|"]);
    st.acted_rc = 2;
    assert_eq!(acw(&rig, &mut st, "exit 0"), 0);
    let lg = rig.log();
    assert!(lg.contains("event receipts not written: crewboard acted exit 2: error: unrecognized subcommand 'acted'"), "{lg}");
    assert!(
        lg.contains("event check failed: crewboard acted exit 2: "),
        "{lg}"
    );
    assert!(lg.contains("wake finished OK"));
    assert!(!rig.p("ev.txt.unhandled").exists());
}

#[test]
fn ac5b_exit_4_of_the_check_stops_the_wakes() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let s = rig
        .settings(
            &[
                "--events-file",
                &evf,
                "--wake-cmd",
                "exit 0",
                "--work-backoff",
                "off",
            ],
            &[],
        )
        .unwrap();
    std::fs::create_dir_all(&s.paths.dir).unwrap();
    // the receipts pass, the check says 4
    let mut steps = FourOnCheck {
        inner: Scripted::new(&["0||", "0|EVENT 1|"]),
    };
    let log = Log::with_err(s.paths.log.clone(), &rig.clock, Box::new(rig.err.clone()));
    let rc = Flow {
        s: &s,
        steps: &mut steps,
        clock: &rig.clock,
        pids: &SystemProcs,
        log: &log,
        host: "testhost",
        sig: Cell::new(None),
        signals: &|| None,
        heartbeat_steps: None,
    }
    .execute();
    assert_eq!(rc, 4);
    assert!(
        rig.state_text("stopped")
            .unwrap()
            .contains(" acted: key gone")
    );
    assert!(!rig.log().contains("wake finished OK"));
}

/// Steps whose second `acted` call (the check) exits with 4.
struct FourOnCheck {
    inner: Scripted,
}

impl Steps for FourOnCheck {
    fn poll(&mut self, q: bool, o: &mut Vec<u8>) -> StepRc {
        self.inner.poll(q, o)
    }
    fn mine_quiet(&mut self) -> StepRc {
        self.inner.mine_quiet()
    }
    fn mine_text(&mut self, o: &mut Vec<u8>) -> StepRc {
        self.inner.mine_text(o)
    }
    fn say(&mut self, t: &str, b: &str, a: bool) -> StepRc {
        self.inner.say(t, b, a)
    }
    fn tick(&mut self, o: &mut Vec<u8>) -> StepRc {
        self.inner.tick(o)
    }
    fn acted_list(&mut self, e: &Path, o: &mut Vec<u8>) -> StepRc {
        self.inner.acted_list(e, o)
    }
    fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
        StepRc::from_stderr(4, "key gone")
    }
    fn heartbeat(&mut self) -> StepRc {
        self.inner.heartbeat()
    }
    fn pr_head(&mut self, r: &str, n: &str) -> Result<Option<(String, String)>, StepRc> {
        self.inner.pr_head(r, n)
    }
    fn open_prs(&mut self, r: &str) -> Result<Vec<(String, String, String)>, StepRc> {
        self.inner.open_prs(r)
    }
}

#[test]
fn ac6_without_an_events_file_there_is_no_receipt_and_no_check() {
    let rig = Rig::new();
    let mut st = Scripted::new(&["0||"]);
    assert_eq!(rig.run(&mut st, &["--wake-cmd", "exit 0"]), 0);
    assert!(st.acted_calls.is_empty());
}

fn rsp(rig: &Rig, st: &mut Scripted) -> u8 {
    let cmd = copy_events(rig, "seen");
    let evf = ev(rig);
    rig.run(
        st,
        &[
            "--events-file",
            &evf,
            "--wake-cmd",
            &cmd,
            "--work-backoff",
            "off",
        ],
    )
}

fn put_result(rig: &Rig, name: &str, text: &str) {
    std::fs::create_dir_all(rig.state().join("results")).unwrap();
    std::fs::write(rig.state().join("results").join(name), text).unwrap();
}

#[test]
fn rs1_a_finished_result_wakes_a_quiet_poll_once_with_its_event_line() {
    let rig = Rig::new();
    let log = rig.state().join("results/7.result.log");
    put_result(
        &rig,
        "7.result",
        &format!(
            "rc=0 end=2026-10-21T10:00:00Z log={}\nticket=7\n",
            log.display()
        ),
    );
    let mut st = Scripted::new(&["1||", "1||", "1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    let want = format!(
        "detached job finished: ticket 7 rc=0 log={} result={}/results/7.result",
        log.display(),
        rig.state().display()
    );
    assert!(t.lines().any(|l| l == want), "{t}");
    assert!(
        rig.log()
            .contains("a detached job has finished (result file in ")
    );
    // the result is marked: the next poll is quiet
    std::fs::remove_file(rig.p("seen")).unwrap();
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    assert!(!rig.p("seen").exists());
    // a new run of the same job is reported again, without a ticket line
    put_result(
        &rig,
        "7.result",
        "rc=1 end=2026-10-21T11:00:00Z log=a.log\n",
    );
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 2);
    assert!(read(&rig.p("ev.txt")).contains("detached job finished: ticket unknown rc=1 "));
}

#[test]
fn rs2_an_unfinished_result_wakes_nothing_and_is_not_marked() {
    let rig = Rig::new();
    put_result(&rig, "8.result", "ticket=8\n");
    let mut st = Scripted::new(&["1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 0);
    put_result(
        &rig,
        "10.result",
        "rc=0 end=2026-10-21T10:00:00Z log=a.log\nticket=10\n",
    );
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("ticket 10 rc=0") && !t.contains("ticket 8"),
        "{t}"
    );
    assert!(!rig.state().join("results/8.result.seen").exists());
    // a script with rc= at column 0 is no result
    put_result(
        &rig,
        "239-chain.sh",
        "#!/bin/bash\nrc=${PIPESTATUS[0]}\nrc=0 end=x log=y\n",
    );
    let before = rig.wakes();
    let mut st = Scripted::new(&["1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), before);
}

#[test]
fn rs3_a_result_of_another_agents_dir_is_not_read() {
    let rig = Rig::new();
    std::fs::create_dir_all(rig.p("other/results")).unwrap();
    std::fs::write(
        rig.p("other/results/9.result"),
        "rc=0 end=x log=x.log\nticket=9\n",
    )
    .unwrap();
    let mut st = Scripted::new(&["1||", "1||"]);
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 0);
    put_result(&rig, "11.result", "rc=3 end=x log=y.log\nticket=11\n");
    assert_eq!(rsp(&rig, &mut st), 0);
    assert_eq!(rig.wakes(), 1);
    let t = read(&rig.p("ev.txt"));
    assert!(
        t.contains("ticket 11 rc=3") && !t.contains("ticket 9"),
        "{t}"
    );
    assert!(!rig.p("other/results/9.result.seen").exists());
}

#[test]
fn events_kept_by_an_earlier_wake_wake_a_quiet_board_only_when_they_have_content() {
    let rig = Rig::new();
    let evf = ev(&rig);
    let go = |st: &mut Scripted| rig.run(st, &["--events-file", &evf, "--work-backoff", "off"]);
    let mut st = Scripted::new(&["1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 0);
    std::fs::write(rig.p("ev.txt.unhandled"), "").unwrap();
    let mut st = Scripted::new(&["1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 0, "an empty .unhandled is nothing");
    std::fs::write(rig.p("ev.txt.unhandled"), "== events\n").unwrap();
    let mut st = Scripted::new(&["1||", "1||"]);
    go(&mut st);
    assert_eq!(rig.wakes(), 1);
    assert!(rig.log().contains(&format!(
        "events kept from an earlier wake are in {evf}.unhandled"
    )));
    // without an events file the kept events are not looked at
    std::fs::write(rig.p("ev.txt.unhandled"), "== events\n").unwrap();
    let mut st = Scripted::new(&["1||"]);
    rig.run(&mut st, &["--work-backoff", "off"]);
    assert_eq!(rig.wakes(), 1);
}
