//! The command line of `crewboard lock`. The words come in raw (clap only passes them on), so
//! crewboard owns the texts of a usage error, and the exit code is 64. An option that does not
//! apply to the subcommand is refused, it is not ignored. All times take one syntax: `90`, `90s`,
//! `30m`, `2h`.

use crate::clock::parse_secs;
use std::ffi::OsString;
use std::time::Duration;

/// What the process knows besides the parsed options.
#[derive(Debug, Clone, Default)]
pub struct RunCtx {
    /// The arguments of this process after the program name (the supervisor of a detached job
    /// is started with the same ones).
    pub orig_args: Vec<OsString>,
    /// `$HOST_LOCK_SUPERVISE`: this process is the supervisor of a detached job, started by the
    /// process with this pid.
    pub supervise: Option<String>,
    /// `run` starts the command in this directory (default: the current one).
    pub cwd: Option<std::path::PathBuf>,
    /// `run` writes stdout and stderr of the command to this file, truncated first (default: the
    /// ones of this process).
    pub output: Option<std::path::PathBuf>,
    /// Variables that the job must not inherit (the token variables of an agent dir).
    pub env_remove: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Take,
    Release,
    Status,
    Break,
    Run,
    CiWait,
    CiDone,
    CiStatus,
}

impl Cmd {
    fn parse(s: &str) -> Option<Cmd> {
        Some(match s {
            "take" => Cmd::Take,
            "release" => Cmd::Release,
            "status" => Cmd::Status,
            "break" => Cmd::Break,
            "run" => Cmd::Run,
            "ci-wait" => Cmd::CiWait,
            "ci-done" => Cmd::CiDone,
            "ci-status" => Cmd::CiStatus,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Cmd::Take => "take",
            Cmd::Release => "release",
            Cmd::Status => "status",
            Cmd::Break => "break",
            Cmd::Run => "run",
            Cmd::CiWait => "ci-wait",
            Cmd::CiDone => "ci-done",
            Cmd::CiStatus => "ci-status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PidArg {
    /// Not given: the parent process (`take`) or this process (`run`).
    Default,
    /// A number as typed; `0` means no pid check.
    Number(String),
    /// The nearest `Runner.Worker` ancestor.
    Runner,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub cmd: Cmd,
    pub dir: String,
    pub owner: String,
    pub reason: String,
    pub wait: u64,
    pub poll: Duration,
    pub pid: PidArg,
    pub expect: Option<u64>,
    pub max_age: u64,
    pub empty_grace: u64,
    pub ci_max_age: u64,
    pub ci_mode: bool,
    pub gate: bool,
    pub no_board: bool,
    /// The test seam: ask this binary for the board (`crewboard status --json`).
    pub crewboard: Option<String>,
    pub config_dir: Option<String>,
    pub initiative: Option<String>,
    pub log: Option<String>,
    pub detach: bool,
    pub result: Option<String>,
    pub ticket: String,
    pub sha: String,
    pub run_cmd: Vec<String>,
}

#[derive(Debug)]
pub enum Parse {
    Run(Box<Parsed>),
    Help,
}

/// The defaults that the environment gives.
#[derive(Debug, Clone, Default)]
pub struct ArgEnv {
    /// `$HOST_LOCK_MAX_AGE`
    pub max_age: Option<String>,
    /// `$HOST_LOCK_EMPTY_GRACE`
    pub empty_grace: Option<String>,
    /// `$CREWBOARD_CONFIG_DIR`, or the global `--config-dir` before `lock`
    pub config_dir: Option<String>,
}

pub const USAGE: &str =
    "usage: crewboard lock take|release|status|run <dir> [options] (see --help)";

fn dur(v: &str, opt: &str) -> Result<u64, String> {
    // `off` is not a lock duration
    parse_secs(v).ok_or_else(|| format!("{opt} needs a duration (90, 90s, 30m or 2h), got '{v}'"))
}

fn poll(v: &str) -> Result<Duration, String> {
    let bad = || format!("--poll needs a duration (0.5, 5, 5s, 1m), got '{v}'");
    if v.is_empty()
        || !v
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b's' | b'm' | b'h'))
    {
        return Err(bad());
    }
    if v.ends_with(['s', 'm', 'h']) {
        return dur(v, "--poll").map(Duration::from_secs);
    }
    v.parse::<f64>()
        .ok()
        .and_then(|x| Duration::try_from_secs_f64(x).ok())
        .ok_or_else(bad)
}

/// The option values as the caller wrote them: text until `finish` has checked and converted it.
struct Raw {
    max_age: String,
    empty_grace: String,
    ci_max_age: String,
    wait: String,
    poll: String,
    expect: Option<String>,
    pid: String,
    if_dead: bool,
    /// the options that the caller wrote, for "does this option apply to the subcommand"
    given: Vec<String>,
}

/// The two options that may stand before the subcommand (the hidden test seam and the config
/// dir): `crewboard lock --crewboard BIN take ...`. Returns the index of the first other word.
fn leading_options(
    args: &[String],
    crewboard: &mut Option<String>,
    config_dir: &mut Option<String>,
) -> Result<usize, String> {
    let mut i = 0;
    while i < args.len() && matches!(args[i].as_str(), "--crewboard" | "--config-dir") {
        let v = args
            .get(i + 1)
            .ok_or_else(|| format!("{} needs a value", args[i]))?;
        if args[i] == "--crewboard" {
            *crewboard = Some(v.clone());
        } else {
            *config_dir = Some(v.clone());
        }
        i += 2;
    }
    Ok(i)
}

/// Read the options after `<dir>` into `p` and `raw`. `Ok(false)` means the caller asked for help.
fn read_options(
    args: &[String],
    mut i: usize,
    p: &mut Parsed,
    raw: &mut Raw,
) -> Result<bool, String> {
    while i < args.len() {
        let a = args[i].as_str();
        if a.starts_with("--") {
            raw.given.push(a.to_string());
        }
        macro_rules! value {
            () => {{
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("{a} needs a value"))?
            }};
        }
        match a {
            "--holder" => p.owner = value!(),
            "--reason" => p.reason = value!(),
            "--wait" => raw.wait = value!(),
            "--poll" => raw.poll = value!(),
            "--pid" => raw.pid = value!(),
            "--expect" => raw.expect = Some(value!()),
            "--max-age" => raw.max_age = value!(),
            "--no-board" => p.no_board = true,
            "--crewboard" => p.crewboard = Some(value!()),
            "--config-dir" => p.config_dir = Some(value!()),
            "--initiative" => p.initiative = Some(value!()),
            "--log" => p.log = Some(value!()),
            "--empty-grace" => raw.empty_grace = value!(),
            "--ci" => p.ci_mode = true,
            "--gate" => p.gate = true,
            "--if-dead" => raw.if_dead = true,
            "--detach" => p.detach = true,
            "--result" => p.result = Some(value!()),
            "--ticket" => p.ticket = value!(),
            "--sha" => p.sha = value!(),
            "--ci-max-age" => raw.ci_max_age = value!(),
            "--" => {
                p.run_cmd = args[i + 1..].to_vec();
                break;
            }
            "-h" | "--help" => return Ok(false),
            _ => return Err(format!("unknown option {a}")),
        }
        i += 1;
    }
    Ok(true)
}

/// An option that does not apply to the subcommand is an error (exit 64), not ignored.
fn refuse_inapplicable(cmd: Cmd, given: &[String]) -> Result<(), String> {
    let refuse: &[&str] = match cmd {
        Cmd::Release | Cmd::Status | Cmd::Break => &[
            "--wait", "--poll", "--pid", "--expect", "--reason", "--ci", "--gate", "--result",
            "--ticket", "--sha",
        ],
        Cmd::CiDone | Cmd::CiStatus => &[
            "--wait", "--poll", "--pid", "--expect", "--reason", "--gate", "--result", "--ticket",
            "--sha",
        ],
        Cmd::CiWait => &[
            "--expect", "--reason", "--gate", "--result", "--ticket", "--sha",
        ],
        Cmd::Take | Cmd::Run => &[],
    };
    for o in refuse {
        if given.iter().any(|g| g == o) {
            return Err(format!("{o} does not apply to {} (see --help)", cmd.name()));
        }
    }
    Ok(())
}

/// Convert the durations and the pid, then check the options against each other and the
/// subcommand.
fn finish(p: &mut Parsed, raw: &Raw) -> Result<(), String> {
    let cmd = p.cmd;
    p.max_age = dur(&raw.max_age, "--max-age")?;
    p.empty_grace = dur(&raw.empty_grace, "--empty-grace")?;
    p.ci_max_age = dur(&raw.ci_max_age, "--ci-max-age")?;
    p.wait = dur(&raw.wait, "--wait")?;
    p.poll = poll(&raw.poll)?;
    if let Some(e) = &raw.expect {
        p.expect = Some(dur(e, "--expect")?);
    }
    if raw.pid == "runner" {
        p.pid = PidArg::Runner;
    } else if !raw.pid.is_empty() {
        if !raw.pid.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "--pid needs a number or 'runner', got '{}'",
                raw.pid
            ));
        }
        p.pid = PidArg::Number(raw.pid.clone());
    }
    if !matches!(cmd, Cmd::Status | Cmd::CiStatus | Cmd::Break) && p.owner.is_empty() {
        return Err("--holder is required".to_string());
    }
    if cmd == Cmd::Take && p.pid == PidArg::Number("0".into()) && p.expect.is_none() {
        return Err("--pid 0 needs --expect <duration> (for example --expect 30m): nobody can prove a pid=0 holder dead, so the lock is judged by the time you expect. Better: use 'run', which holds the lock exactly as long as your command runs".to_string());
    }
    if cmd == Cmd::Run && p.run_cmd.is_empty() {
        return Err("run needs a command after --".to_string());
    }
    check_detach(p)?;
    if !p.ticket.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "--ticket needs a ticket number, got '{}'",
            p.ticket
        ));
    }
    if !p.sha.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(format!("--sha needs a commit SHA, got '{}'", p.sha));
    }
    if cmd == Cmd::Break && !raw.if_dead {
        return Err("break needs --if-dead: it removes only a stale lock".to_string());
    }
    if raw.if_dead && cmd != Cmd::Break {
        return Err("--if-dead is for break only".to_string());
    }
    if p.gate && !matches!(cmd, Cmd::Run | Cmd::Take) {
        return Err("--gate is for take and run only".to_string());
    }
    if p.result.as_deref() == Some("") {
        p.result = None;
    }
    Ok(())
}

/// The options that go with `--detach`.
fn check_detach(p: &Parsed) -> Result<(), String> {
    if p.detach && p.cmd != Cmd::Run {
        return Err("--detach is for run only".to_string());
    }
    if p.detach && p.result.as_deref().is_none_or(str::is_empty) {
        return Err(
            "--detach needs --result FILE (the job writes rc=<n> end=<utc> log=<path> there)"
                .to_string(),
        );
    }
    if !p.detach && p.result.as_deref().is_some_and(|r| !r.is_empty()) {
        return Err("--result needs --detach".to_string());
    }
    if !p.detach && (!p.ticket.is_empty() || !p.sha.is_empty()) {
        return Err("--ticket and --sha need --detach".to_string());
    }
    Ok(())
}

/// Parse the words after `lock`. `Err` is the text of a usage error (exit 64).
pub fn parse(args: &[String], env: &ArgEnv) -> Result<Parse, String> {
    let mut crewboard: Option<String> = None;
    let mut config_dir = env.config_dir.clone().filter(|s| !s.is_empty());
    let mut i = leading_options(args, &mut crewboard, &mut config_dir)?;
    let word = args.get(i).map(String::as_str).unwrap_or("");
    let cmd = match Cmd::parse(word) {
        Some(c) => c,
        None if matches!(word, "-h" | "--help" | "help") => return Ok(Parse::Help),
        None => return Err(USAGE.to_string()),
    };
    i += 1;
    let Some(dir) = args.get(i) else {
        return Err("missing <dir>".to_string());
    };
    if matches!(dir.as_str(), "-h" | "--help") {
        return Ok(Parse::Help);
    }
    let dir = dir.clone();
    i += 1;

    let mut p = Parsed {
        cmd,
        dir,
        owner: String::new(),
        reason: String::new(),
        wait: 0,
        poll: Duration::from_secs(5),
        pid: PidArg::Default,
        expect: None,
        max_age: 0,
        empty_grace: 0,
        ci_max_age: 0,
        ci_mode: false,
        gate: false,
        no_board: false,
        crewboard,
        config_dir,
        initiative: None,
        log: None,
        detach: false,
        result: None,
        ticket: String::new(),
        sha: String::new(),
        run_cmd: Vec::new(),
    };
    let mut raw = Raw {
        max_age: env.max_age.clone().unwrap_or_else(|| "14400".into()),
        empty_grace: env.empty_grace.clone().unwrap_or_else(|| "30".into()),
        ci_max_age: "21600".to_string(),
        wait: "0".to_string(),
        poll: "5".to_string(),
        expect: None,
        pid: String::new(),
        if_dead: false,
        given: Vec::new(),
    };
    if !read_options(args, i, &mut p, &mut raw)? {
        return Ok(Parse::Help);
    }
    refuse_inapplicable(cmd, &raw.given)?;
    finish(&mut p, &raw)?;
    Ok(Parse::Run(Box::new(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn ok(s: &str) -> Parsed {
        match parse(&w(s), &ArgEnv::default()) {
            Ok(Parse::Run(p)) => *p,
            other => panic!("{s}: {other:?}"),
        }
    }

    fn err(s: &str) -> String {
        match parse(&w(s), &ArgEnv::default()) {
            Err(e) => e,
            other => panic!("{s} should be refused: {other:?}"),
        }
    }

    #[test]
    fn owner_is_no_name_of_holder_any_more() {
        assert!(err("take /x/l --owner a@x --reason cargo").contains("--owner"));
        let p = ok("take /x/l --holder a@x --reason cargo");
        assert_eq!(p.owner, "a@x");
        let p = ok("run /x/l --holder b -- echo --owner c");
        assert_eq!(
            (p.owner.as_str(), p.run_cmd.as_slice()),
            ("b", &["echo".to_string(), "--owner".into(), "c".into()][..])
        );
        assert_eq!(err("take /x"), "--holder is required");
    }

    #[test]
    fn take_with_the_usual_options() {
        let p = ok("take /x/l --holder a@x --reason cargo --wait 30m --poll 0.3 --pid 77");
        assert_eq!(p.cmd, Cmd::Take);
        assert_eq!((p.wait, p.poll), (1800, Duration::from_secs_f64(0.3)));
        assert_eq!(p.pid, PidArg::Number("77".into()));
        assert_eq!((p.max_age, p.empty_grace, p.ci_max_age), (14400, 30, 21600));
    }

    #[test]
    fn the_command_after_two_dashes_is_taken_raw() {
        let p = ok("run /x/l --holder a -- echo --holder b -h");
        assert_eq!(p.run_cmd, vec!["echo", "--holder", "b", "-h"]);
        assert_eq!(p.owner, "a");
    }

    #[test]
    fn the_seam_and_the_config_dir_may_stand_before_the_subcommand() {
        let p = ok("--crewboard /stub --config-dir /cfg take /x/l --holder a");
        assert_eq!(p.crewboard.as_deref(), Some("/stub"));
        assert_eq!(p.config_dir.as_deref(), Some("/cfg"));
        // the option after the dir beats the variable and the early one
        let p = ok("take /x/l --holder a --config-dir /opt --initiative i1");
        assert_eq!(p.config_dir.as_deref(), Some("/opt"));
        assert_eq!(p.initiative.as_deref(), Some("i1"));
        let env = ArgEnv {
            config_dir: Some("/env".into()),
            ..Default::default()
        };
        let Ok(Parse::Run(p)) = parse(&w("take /x/l --holder a"), &env) else {
            panic!()
        };
        assert_eq!(p.config_dir.as_deref(), Some("/env"));
    }

    #[test]
    fn help_in_every_place() {
        for s in [
            "--help",
            "-h",
            "help",
            "take --help",
            "take /x -h",
            "take /x --holder a --help",
        ] {
            assert!(
                matches!(parse(&w(s), &ArgEnv::default()), Ok(Parse::Help)),
                "{s}"
            );
        }
    }

    #[test]
    fn usage_errors_have_the_texts_of_the_script() {
        assert_eq!(err(""), USAGE);
        assert_eq!(err("frobnicate"), USAGE);
        assert_eq!(err("take"), "missing <dir>");
        assert_eq!(err("take /x --holder a --bogus"), "unknown option --bogus");
        assert_eq!(err("take /x"), "--holder is required");
        assert_eq!(err("run /x --holder a"), "run needs a command after --");
        assert_eq!(err("take /x --holder a --wait"), "--wait needs a value");
    }

    #[test]
    fn op1_to_op3_an_option_that_does_not_apply_is_refused_with_its_name() {
        assert_eq!(
            err("release /x --holder a --wait 60"),
            "--wait does not apply to release (see --help)"
        );
        assert_eq!(
            err("status /x --no-board --pid 1"),
            "--pid does not apply to status (see --help)"
        );
        assert_eq!(
            err("break /x --if-dead --reason x"),
            "--reason does not apply to break (see --help)"
        );
        assert_eq!(
            err("ci-done /x --holder a --gate"),
            "--gate does not apply to ci-done (see --help)"
        );
        // the other direction: they apply where the help names them
        ok("ci-wait /x --holder a --pid 5 --wait 3 --ci-max-age 1h");
        ok("take /x --holder a --ci --gate");
        ok("status /x --max-age 1h --no-board");
    }

    #[test]
    fn e1_pid_zero_needs_expect_on_take() {
        let e = err("take /x --holder a --pid 0");
        assert!(e.contains("--expect") && e.contains("'run'"), "{e}");
        let p = ok("take /x --holder a --pid 0 --expect 1");
        assert_eq!(p.expect, Some(1));
        // a real pid needs no expect
        assert_eq!(ok("take /x --holder a --pid 12").expect, None);
        // and `run` has no pid rule
        ok("run /x --holder a -- true");
    }

    #[test]
    fn e5b_bad_durations_are_usage_errors() {
        assert!(
            err("take /x --holder a --pid 0 --expect 2d").contains("--expect needs a duration")
        );
        assert!(err("take /x --holder a --wait soon").contains("--wait needs a duration"));
        assert!(err("take /x --holder a --pid 0 --expect m").contains("got 'm'"));
        assert!(err("take /x --holder a --wait off").contains("--wait needs a duration"));
        assert_eq!(
            err("take /x --holder a --poll x"),
            "--poll needs a duration (0.5, 5, 5s, 1m), got 'x'"
        );
        assert!(err("take /x --holder a --max-age 1.5h").contains("--max-age needs a duration"));
        // a number that does not fit is refused, not a panic
        assert!(
            err("take /x --holder a --wait 99999999999999999999")
                .contains("--wait needs a duration")
        );
        assert_eq!(
            err("take /x --holder a --poll 99999999999999999999"),
            "--poll needs a duration (0.5, 5, 5s, 1m), got '99999999999999999999'"
        );
    }

    #[test]
    fn the_env_gives_max_age_and_grace_and_a_bad_one_is_refused() {
        let env = ArgEnv {
            max_age: Some("1h".into()),
            empty_grace: Some("7".into()),
            ..Default::default()
        };
        let Ok(Parse::Run(p)) = parse(&w("status /x"), &env) else {
            panic!()
        };
        assert_eq!((p.max_age, p.empty_grace), (3600, 7));
        let bad = ArgEnv {
            max_age: Some("soon".into()),
            ..Default::default()
        };
        assert!(parse(&w("status /x"), &bad).is_err());
    }

    #[test]
    fn pid_must_be_a_number_or_runner() {
        assert_eq!(ok("take /x --holder a --pid runner").pid, PidArg::Runner);
        assert_eq!(
            err("take /x --holder a --pid abc"),
            "--pid needs a number or 'runner', got 'abc'"
        );
    }

    #[test]
    fn dt4_detach_and_result_go_together_and_only_with_run() {
        assert_eq!(
            err("run /x --holder a --detach -- true"),
            "--detach needs --result FILE (the job writes rc=<n> end=<utc> log=<path> there)"
        );
        assert_eq!(
            err("run /x --holder a --result /r -- true"),
            "--result needs --detach"
        );
        assert_eq!(
            err("take /x --holder a --detach --result /r"),
            "--detach is for run only"
        );
        let p = ok("run /x --holder a --detach --result /r --ticket 42 --sha abc123 -- true");
        assert_eq!((p.ticket.as_str(), p.sha.as_str()), ("42", "abc123"));
    }

    #[test]
    fn dt2c_ticket_needs_digits_and_detach_sha_needs_alphanumerics() {
        assert_eq!(
            err("run /x --holder a --detach --result /r --ticket x1 -- true"),
            "--ticket needs a ticket number, got 'x1'"
        );
        assert_eq!(
            err("run /x --holder a --ticket 5 -- true"),
            "--ticket and --sha need --detach"
        );
        assert_eq!(
            err("run /x --holder a --detach --result /r --sha a-b -- true"),
            "--sha needs a commit SHA, got 'a-b'"
        );
    }

    #[test]
    fn h10g_break_needs_if_dead_and_if_dead_is_for_break_only() {
        assert_eq!(
            err("break /x"),
            "break needs --if-dead: it removes only a stale lock"
        );
        assert_eq!(
            err("take /x --holder a --if-dead"),
            "--if-dead is for break only"
        );
        ok("break /x --if-dead --no-board --log /l");
    }

    #[test]
    fn gate_is_for_take_and_run_only() {
        // not for ci-wait: it is refused by the table, with its own text
        assert_eq!(
            err("ci-wait /x --holder a --gate"),
            "--gate does not apply to ci-wait (see --help)"
        );
        ok("run /x --holder a --gate -- true");
    }
}
