//! `crewboard monitor`: a silent loop around `watch` for a Monitor tool (a tool that wakes the
//! model once per line of standard output).
//!
//! The contract is the token-economy rule: the model wakes only on output.
//!
//! - `watch` exit 1 (no events): print nothing, wait `--pause`, run `watch` again.
//! - `watch` exit 0 (events): print the events once, exit 0. The agent handles them and starts
//!   the monitor again.
//! - `watch` exit 2 or more: print ONE line `crewboard-watch: exit <code>: <reason> (<status>)`
//!   and exit with that code. With `--json` the line is
//!   `{"monitor":"exit","code":..,"reason":"..","status":".."}`.
//!
//! The loop takes a [`Cycle`], so a test scripts the cycles with no process and no sleep. The
//! cycle of the command runs `watch` in process ([`InProcess`]). The hidden test seam
//! `--crewboard BIN` runs another binary's `watch` instead ([`Subprocess`]) and reads its output,
//! its stderr and its exit code.

use super::AgentCtx;
use super::watch::{PollOpts, watch};
use crate::base::ignore::best_effort_output;
use crate::clock::{Clock, format_ts};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The default of `--pause`.
pub(crate) const DEFAULT_PAUSE: Duration = Duration::from_secs(2);

/// The longest reason, status or log excerpt, in characters.
const CUT: usize = 200;

/// What one run of `watch` gave.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CycleResult {
    /// The exit code of `watch`: 0 events, 1 no events, 2 or more an error.
    pub(crate) code: u8,
    /// What `watch` printed to standard output.
    pub(crate) out: String,
    /// The first line of the error, without control characters. Empty when there is none.
    pub(crate) reason: String,
    /// The status line (`cursor=... polls=... last_ok=...`). Empty when there is none.
    pub(crate) status: String,
}

/// One run of `watch`.
pub(crate) trait Cycle {
    fn run(&mut self) -> CycleResult;
}

/// How the loop behaves.
#[derive(Debug, Clone)]
pub(crate) struct LoopOpts {
    /// The wait after a silent cycle.
    pub(crate) pause: Duration,
    /// Stop with exit 0 after this many silent cycles; 0 = never.
    pub(crate) max_cycles: u64,
    /// One line per cycle, in its own file, never on standard output.
    pub(crate) log: Option<PathBuf>,
    /// The final error line as JSON.
    pub(crate) json: bool,
}

/// `--pause`: whole seconds with an optional unit (`2`, `90s`, `5m`), or a fraction of a second.
pub(crate) fn parse_pause(s: &str) -> Option<std::time::Duration> {
    const MAX: u64 = 86_400;
    crate::clock::parse_secs(s)
        .filter(|n| *n <= MAX)
        .map(std::time::Duration::from_secs)
        .or_else(|| {
            s.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 0.0 && *v < MAX as f64)
                .map(std::time::Duration::from_secs_f64)
        })
}

/// [`run_loop`] on the wall clock.
pub(crate) fn run_on_wall_clock(cycle: &mut dyn Cycle, opts: &LoopOpts, out: &mut dyn Write) -> u8 {
    run_loop(cycle, &crate::clock::SystemClock, opts, out)
}

/// Control characters out, cut to 200 characters.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(CUT).collect()
}

/// The first (or the last) line that is not blank, cleaned.
fn pick_line(text: &str, last: bool) -> String {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let line = if last {
        lines.next_back()
    } else {
        lines.next()
    };
    line.map(clean).unwrap_or_default()
}

pub(crate) fn first_line(text: &str) -> String {
    pick_line(text, false)
}

fn last_line(text: &str) -> String {
    pick_line(text, true)
}

/// The status line that `watch` prints with every "no events" and every error:
/// `cursor=<word> polls=<digits> last_ok=...`.
fn is_status_line(l: &str) -> bool {
    let mut w = l.split(' ');
    matches!((w.next(), w.next(), w.next()), (Some(c), Some(p), Some(o))
        if c.starts_with("cursor=")
            && p.strip_prefix("polls=").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            && o.starts_with("last_ok="))
}

/// The text of the error line: `reason (status)`, the reason alone, the status alone, or
/// `(no stderr)`.
fn message(reason: &str, status: &str) -> String {
    match (reason.is_empty(), status.is_empty()) {
        (false, false) => format!("{reason} ({status})"),
        (false, true) => reason.to_string(),
        (true, false) => status.to_string(),
        (true, true) => "(no stderr)".to_string(),
    }
}

/// The one line of an error exit: `crewboard-watch: exit <code>: <reason> (<status>)`, or the
/// JSON object when `json` is set. No line break at the end.
pub(crate) fn exit_line(code: u8, reason: &str, status: &str, json: bool) -> String {
    if json {
        let s = |v: &str| serde_json::to_string(v).unwrap_or_else(|_| "\"\"".into());
        format!(
            "{{\"monitor\":\"exit\",\"code\":{code},\"reason\":{},\"status\":{}}}",
            s(reason),
            s(status)
        )
    } else {
        format!("crewboard-watch: exit {code}: {}", message(reason, status))
    }
}

fn log_line(log: Option<&Path>, clock: &dyn Clock, line: &str) {
    if let Some(p) = log
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
    {
        best_effort_output(writeln!(f, "{} {line}", format_ts(clock.now())));
    }
}

/// Run `cycle` until it has events, fails, or `--max-cycles` silent cycles are done. Prints to
/// `out` only the events or the error line. Returns the exit code of the process.
pub(crate) fn run_loop(
    cycle: &mut dyn Cycle,
    clock: &dyn Clock,
    opts: &LoopOpts,
    out: &mut dyn Write,
) -> u8 {
    let log = opts.log.as_deref();
    let mut cycles = 0u64;
    let mut silent = 0u64;
    loop {
        let r = cycle.run();
        cycles += 1;
        match r.code {
            1 => {
                silent += 1;
                log_line(
                    log,
                    clock,
                    &format!(
                        "cycle={cycles} silent={silent} exit=1 last=\"{}\"",
                        last_line(&r.out)
                    ),
                );
                if opts.max_cycles > 0 && silent >= opts.max_cycles {
                    return 0;
                }
                if !opts.pause.is_zero() {
                    clock.sleep(opts.pause);
                }
            }
            0 => {
                // The cursor has moved. When the events cannot be written (the reader is gone),
                // they are not delivered: say so in the log and exit 1.
                let text = format!("{}\n", r.out.trim_end_matches('\n'));
                if out
                    .write_all(text.as_bytes())
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    log_line(
                        log,
                        clock,
                        &format!("cycle={cycles} silent={silent} exit=1 write failed"),
                    );
                    return 1;
                }
                log_line(
                    log,
                    clock,
                    &format!("cycle={cycles} silent={silent} exit=0 events"),
                );
                return 0;
            }
            code => {
                log_line(
                    log,
                    clock,
                    &format!(
                        "cycle={cycles} silent={silent} exit={code} {}",
                        message(&r.reason, &r.status)
                    ),
                );
                best_effort_output(writeln!(
                    out,
                    "{}",
                    exit_line(code, &r.reason, &r.status, opts.json)
                ));
                best_effort_output(out.flush());
                return code;
            }
        }
    }
}

/// `watch` in this process, with one `AgentCtx` for all cycles.
pub(crate) struct InProcess<'a> {
    pub(crate) ac: &'a AgentCtx,
    pub(crate) timeout: Duration,
    pub(crate) interval: Duration,
    pub(crate) opts: PollOpts,
    /// The retry notices of `watch` go here (with the cycle lines), not to standard error.
    pub(crate) log: Option<PathBuf>,
}

impl Cycle for InProcess<'_> {
    fn run(&mut self) -> CycleResult {
        let mut buf: Vec<u8> = Vec::new();
        let mut notices: Vec<u8> = Vec::new();
        let r = watch(
            self.ac,
            self.timeout,
            self.interval,
            self.opts,
            &mut buf,
            &mut notices,
        );
        for l in String::from_utf8_lossy(&notices).lines() {
            log_line(self.log.as_deref(), &crate::clock::SystemClock, l);
        }
        let out = String::from_utf8_lossy(&buf).into_owned();
        match r {
            Ok(exit) => CycleResult {
                code: exit.code(),
                out,
                ..CycleResult::default()
            },
            Err((e, status)) => CycleResult {
                code: e.exit().code(),
                out,
                reason: first_line(&e.to_string()),
                status: clean(&status),
            },
        }
    }
}

/// `watch` of another binary (the test seam `--crewboard BIN`): `BIN [--config-dir D] watch ...`.
pub(crate) struct Subprocess {
    pub(crate) bin: String,
    /// The arguments before `watch` and after it, as the caller built them.
    pub(crate) before: Vec<String>,
    pub(crate) after: Vec<String>,
}

impl Cycle for Subprocess {
    fn run(&mut self) -> CycleResult {
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(&self.bin);
        cmd.args(&self.before)
            .arg("watch")
            .args(&self.after)
            .stdin(Stdio::null());
        // the other binary gets the token and the settings of the env file
        if let Some(env) = crate::agent_env::current() {
            env.apply(&mut cmd);
        }
        let out = crate::spawn::output_retrying(&mut cmd);
        let out = match out {
            Ok(o) => o,
            Err(e) => {
                return CycleResult {
                    code: 4,
                    reason: clean(&format!("cannot run {}: {e}", self.bin)),
                    ..CycleResult::default()
                };
            }
        };
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        // The first non-blank stderr line that is not the status line names the reason; the
        // status line comes last.
        let mut reason = stderr
            .lines()
            .find(|l| !l.trim().is_empty() && !is_status_line(l))
            .map(clean)
            .unwrap_or_default();
        let status = stderr
            .lines()
            .rev()
            .find(|l| is_status_line(l))
            .map(clean)
            .unwrap_or_default();
        let code = match out.status.code() {
            Some(c) => u8::try_from(c).unwrap_or(1),
            None => {
                if reason.is_empty() {
                    reason = "the process was killed by a signal".into();
                }
                4
            }
        };
        if code >= 2 && reason.is_empty() && status.is_empty() {
            reason = last_line(&stdout);
        }
        CycleResult {
            code,
            out: stdout,
            reason,
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::collections::VecDeque;

    /// A cycle that plays a script and records how often it ran.
    struct Script {
        plan: VecDeque<CycleResult>,
        runs: usize,
    }

    impl Script {
        fn new(plan: Vec<CycleResult>) -> Script {
            Script {
                plan: plan.into(),
                runs: 0,
            }
        }
    }

    impl Cycle for Script {
        fn run(&mut self) -> CycleResult {
            self.runs += 1;
            // a plan that ends keeps giving "no events"
            self.plan.pop_front().unwrap_or_else(silent)
        }
    }

    fn silent() -> CycleResult {
        CycleResult {
            code: 1,
            out: "no events: cursor=abc polls=3 last_ok=now\n".into(),
            ..CycleResult::default()
        }
    }

    fn events(text: &str) -> CycleResult {
        CycleResult {
            code: 0,
            out: format!("{text}\n"),
            ..CycleResult::default()
        }
    }

    fn failed(code: u8, reason: &str, status: &str) -> CycleResult {
        CycleResult {
            code,
            reason: reason.into(),
            status: status.into(),
            ..CycleResult::default()
        }
    }

    fn opts() -> LoopOpts {
        LoopOpts {
            pause: Duration::ZERO,
            max_cycles: 0,
            log: None,
            json: false,
        }
    }

    /// Runs the loop; returns the exit code and what it printed.
    fn go(cycle: &mut Script, o: &LoopOpts) -> (u8, String) {
        let clock = FakeClock::at(1_700_000_000);
        let mut out = Vec::new();
        let code = run_loop(cycle, &clock, o, &mut out);
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn pause_is_seconds_with_an_optional_unit_or_a_fraction() {
        let secs = |s: &str| parse_pause(s).map(|d| d.as_millis());
        assert_eq!(secs("0"), Some(0));
        assert_eq!(secs("2"), Some(2000));
        assert_eq!(secs("90s"), Some(90_000));
        assert_eq!(secs("5m"), Some(300_000));
        assert_eq!(secs("0.5"), Some(500));
        assert_eq!(secs("86400"), Some(86_400_000));
        for bad in ["", "-1", "soon", "inf", "NaN", "86401", "99999", "25h"] {
            assert_eq!(secs(bad), None, "{bad}");
        }
    }

    /// A writer whose reader is gone.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn events_that_cannot_be_written_are_logged_and_exit_1() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("loop.log");
        let o = LoopOpts {
            log: Some(log.clone()),
            ..opts()
        };
        let clock = FakeClock::at(1_700_000_000);
        let mut s = Script::new(vec![silent(), events("EVENT")]);
        assert_eq!(run_loop(&mut s, &clock, &o, &mut Closed), 1);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains("cycle=2 silent=1 exit=1 write failed"),
            "{text}"
        );
        assert!(!text.contains("events\n"), "{text}");
        // twin: a writer that works gives exit 0
        let mut s = Script::new(vec![events("EVENT")]);
        assert_eq!(go(&mut s, &opts()).0, 0);
        // an error line that cannot be written does not panic and keeps its code
        let mut s = Script::new(vec![failed(3, "net", "")]);
        assert_eq!(run_loop(&mut s, &clock, &opts(), &mut Closed), 3);
    }

    #[test]
    fn w1_twenty_silent_cycles_print_nothing() {
        let mut s = Script::new(vec![]);
        let o = LoopOpts {
            max_cycles: 20,
            ..opts()
        };
        let (code, out) = go(&mut s, &o);
        assert_eq!((code, out.as_str(), s.runs), (0, "", 20));
        // the twin: 19 silent cycles are not enough to stop the loop
        let mut s = Script::new((0..19).map(|_| silent()).collect());
        s.plan.push_back(events("EVENT"));
        let (code, out) = go(&mut s, &o);
        assert_eq!((code, s.runs), (0, 20));
        assert_eq!(out, "EVENT\n");
    }

    #[test]
    fn w2_events_after_silent_cycles_print_one_block() {
        let mut plan: Vec<CycleResult> = (0..7).map(|_| silent()).collect();
        plan.push(events(
            "EVENT 1: assign #12 to agent-a@host-a\nEVENT 2: owner message",
        ));
        let mut s = Script::new(plan);
        let (code, out) = go(&mut s, &opts());
        assert_eq!(code, 0);
        assert_eq!(
            out,
            "EVENT 1: assign #12 to agent-a@host-a\nEVENT 2: owner message\n"
        );
        assert_eq!(s.runs, 8);
    }

    #[test]
    fn w2b_cursor_line_does_not_leak() {
        let mut plan: Vec<CycleResult> = (0..3).map(|_| silent()).collect();
        plan.push(events("EVENT"));
        let (_, out) = go(&mut Script::new(plan), &opts());
        assert!(!out.contains("cursor="), "{out}");
        // the twin: the silent cycles alone print nothing either
        let (_, out) = go(
            &mut Script::new(vec![]),
            &LoopOpts {
                max_cycles: 3,
                ..opts()
            },
        );
        assert_eq!(out, "");
    }

    #[test]
    fn w3_network_error_prints_one_line_and_exits_3() {
        let mut s = Script::new(vec![
            silent(),
            silent(),
            failed(3, "HTTP 502 from api.github.com", ""),
        ]);
        let (code, out) = go(&mut s, &opts());
        assert_eq!(code, 3);
        assert_eq!(
            out,
            "crewboard-watch: exit 3: HTTP 502 from api.github.com\n"
        );
        assert_eq!(s.runs, 3);
    }

    #[test]
    fn w3b_exit_4_and_2_pass_through() {
        for code in [2u8, 4] {
            let (c, out) = go(&mut Script::new(vec![failed(code, "refused", "")]), &opts());
            assert_eq!(c, code);
            assert_eq!(out, format!("crewboard-watch: exit {code}: refused\n"));
        }
    }

    #[test]
    fn w3d_reason_and_status() {
        let status = "cursor=unknown polls=0 last_ok=never";
        let (code, out) = go(
            &mut Script::new(vec![failed(
                4,
                "configuration error: no board file for initiative demo",
                status,
            )]),
            &opts(),
        );
        assert_eq!(code, 4);
        assert_eq!(
            out,
            format!(
                "crewboard-watch: exit 4: configuration error: no board file for initiative demo ({status})\n"
            )
        );
        // the JSON form prints the object and not the text
        let (_, out) = go(
            &mut Script::new(vec![failed(4, "no \"board\"", status)]),
            &LoopOpts {
                json: true,
                ..opts()
            },
        );
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["monitor"], "exit");
        assert_eq!(v["code"], 4);
        assert_eq!(v["reason"], "no \"board\"");
        assert_eq!(v["status"], status);
        assert!(
            out.starts_with("{\"monitor\":\"exit\",\"code\":4,"),
            "{out}"
        );
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn w3e_status_alone_reason_alone() {
        let line = |r: &str, s: &str| exit_line(3, r, s, false);
        assert_eq!(
            line("", "cursor=c1 polls=4 last_ok=2026-10-18T10:00:00Z"),
            "crewboard-watch: exit 3: cursor=c1 polls=4 last_ok=2026-10-18T10:00:00Z"
        );
        assert_eq!(
            line(
                "refused [rule=not-approved] agent-a@host-a is not approved",
                ""
            ),
            "crewboard-watch: exit 3: refused [rule=not-approved] agent-a@host-a is not approved"
        );
        assert_eq!(line("", ""), "crewboard-watch: exit 3: (no stderr)");
    }

    #[test]
    fn reason_is_the_first_nonblank_line_without_control_characters_cut_to_200() {
        let text = format!(
            "\n  \nconfiguration error: x\u{1b}[31m {}\nhint: more",
            "Z".repeat(300)
        );
        let r = first_line(&text);
        assert!(r.starts_with("configuration error: x[31m ZZZ"), "{r}");
        assert_eq!(r.chars().count(), 200);
        assert!(!r.contains('\u{1b}') && !r.contains("hint"));
        assert_eq!(first_line("\n \n"), "");
    }

    #[test]
    fn status_lines_are_told_from_reasons() {
        assert!(is_status_line("cursor=unknown polls=0 last_ok=never"));
        assert!(is_status_line(
            "cursor=c1 polls=4 last_ok=2026-10-18T10:00:00Z"
        ));
        assert!(!is_status_line("cursor=c1 polls=x last_ok=never"));
        assert!(!is_status_line("cursor=c1"));
        assert!(!is_status_line(
            "configuration error: cursor=c1 polls=4 last_ok=n"
        ));
    }

    #[test]
    fn w6_log_counts_silent_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("loop.log");
        let o = LoopOpts {
            max_cycles: 5,
            log: Some(log.clone()),
            ..opts()
        };
        let mut s = Script::new(vec![]);
        let (code, out) = go(&mut s, &o);
        assert_eq!((code, out.as_str()), (0, ""));
        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert!(
            lines[4].starts_with(
                "2023-11-14T22:13:20Z cycle=5 silent=5 exit=1 last=\"no events: cursor=abc"
            ),
            "{}",
            lines[4]
        );
        // events and errors are logged too
        let mut s = Script::new(vec![silent(), events("E")]);
        go(&mut s, &o);
        let mut s = Script::new(vec![failed(4, "configuration error: x", "")]);
        go(&mut s, &o);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("cycle=2 silent=1 exit=0 events"), "{text}");
        assert!(
            text.contains("cycle=1 silent=0 exit=4 configuration error: x"),
            "{text}"
        );
    }

    #[test]
    fn the_pause_waits_after_a_silent_cycle_only() {
        struct Counting(std::cell::Cell<u32>);
        impl Clock for Counting {
            fn now(&self) -> i64 {
                0
            }
            fn sleep(&self, d: Duration) {
                assert_eq!(d, Duration::from_secs(7));
                self.0.set(self.0.get() + 1);
            }
        }
        let clock = Counting(std::cell::Cell::new(0));
        let mut s = Script::new(vec![silent(), silent(), events("E")]);
        let o = LoopOpts {
            pause: Duration::from_secs(7),
            ..opts()
        };
        let mut out = Vec::new();
        assert_eq!(run_loop(&mut s, &clock, &o, &mut out), 0);
        assert_eq!(clock.0.get(), 2);
        // the last silent cycle before --max-cycles does not wait
        let clock = Counting(std::cell::Cell::new(0));
        let o = LoopOpts { max_cycles: 3, ..o };
        run_loop(&mut Script::new(vec![]), &clock, &o, &mut out);
        assert_eq!(clock.0.get(), 2);
    }

    #[cfg(unix)]
    mod seam {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn stub(dir: &Path, body: &str) -> String {
            let p = dir.join("stub");
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.to_string_lossy().into_owned()
        }

        #[test]
        fn w5_args_reach_watch() {
            let dir = tempfile::tempdir().unwrap();
            let rec = dir.path().join("argv");
            let bin = stub(
                dir.path(),
                &format!("echo \"$*\" >>'{}'; echo EVENT", rec.display()),
            );
            let mut c = Subprocess {
                bin,
                before: vec!["--config-dir".into(), "/x/cfg".into()],
                after: vec![
                    "--timeout".into(),
                    "7m".into(),
                    "--initiative".into(),
                    "demo".into(),
                    "--json".into(),
                ],
            };
            let r = c.run();
            assert_eq!((r.code, r.out.as_str()), (0, "EVENT\n"));
            assert_eq!(
                std::fs::read_to_string(&rec).unwrap(),
                "--config-dir /x/cfg watch --timeout 7m --initiative demo --json\n"
            );
        }

        #[test]
        fn the_subprocess_cycle_reads_reason_status_and_code_from_the_binary() {
            let dir = tempfile::tempdir().unwrap();
            let bin = stub(
                dir.path(),
                "echo 'configuration error: no key' >&2; echo 'hint' >&2; \
                 echo 'cursor=c polls=2 last_ok=never' >&2; exit 4",
            );
            let mut c = Subprocess {
                bin,
                before: vec![],
                after: vec![],
            };
            let r = c.run();
            assert_eq!(r.code, 4);
            assert_eq!(r.reason, "configuration error: no key");
            assert_eq!(r.status, "cursor=c polls=2 last_ok=never");
            // no stderr: the last stdout line is the reason
            let bin = stub(dir.path(), "echo first; echo ''; echo last; exit 3");
            let r = Subprocess {
                bin,
                before: vec![],
                after: vec![],
            }
            .run();
            assert_eq!(
                (r.code, r.reason.as_str(), r.status.as_str()),
                (3, "last", "")
            );
            // a binary that is not there is a configuration error
            let r = Subprocess {
                bin: dir.path().join("none").to_string_lossy().into_owned(),
                before: vec![],
                after: vec![],
            }
            .run();
            assert_eq!(r.code, 4);
            assert!(r.reason.starts_with("cannot run"), "{}", r.reason);
        }
    }
}
