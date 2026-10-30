//! The options of `crewboard wake` and what a run makes of them: [`WakeArgs`] (the flags),
//! [`GlobalFlags`] (the global flags that `wake` reads) and [`Settings`], plain data that
//! [`resolve`] computes once at the start of a run from the flags, the agent config and the
//! environment. A flag wins over the environment, the environment over the default. No phase
//! of the run reads the environment again.

use super::job::{self as wake_job, Direct};
use crate::config::AgentConfig;
use crate::wake::policy::{self, WorkBackoff};
use crate::wake::state::{self, Paths, StateKey};
use clap::Args;
use std::path::{Path, PathBuf};

/// The options of `wake`. The wake itself is the hidden `--wake-cmd`; the options of the model
/// part (runtime, prompt, budget, heartbeat, events) are not here yet.
#[derive(Debug, Args, Clone, Default)]
pub struct WakeArgs {
    /// The config dir of the agent: reads `<dir>/<initiative>.toml` for the agent name and the
    /// initiative, and uses the dir as `--config-dir`.
    #[arg(long)]
    pub agent_dir: Option<PathBuf>,
    /// The initiative (default: the only one of the agent dir).
    #[arg(long)]
    pub initiative: Option<String>,
    /// Run `crewboard tick` under the lock on every run and start no model.
    #[arg(long)]
    pub tick: bool,
    /// The agent name.
    #[arg(long)]
    pub agent: Option<String>,
    /// Minimum seconds between wakes for owned work (default 900, or $CREWBOARD_WORK_BACKOFF);
    /// `off` never wakes for owned work.
    #[arg(long, hide = true)]
    pub work_backoff: Option<String>,
    /// Seconds to wait before each retry of a call that exited with 3 (default '10 30', or
    /// $CREWBOARD_RETRY_DELAYS; an empty value means no retry).
    #[arg(long, hide = true)]
    pub retry_delays: Option<String>,
    /// A wake lock held longer than this many seconds is stale (default 14400).
    #[arg(long, allow_negative_numbers = true, hide = true)]
    pub stale_after: Option<i64>,
    /// A lock directory without a pid file is stale after this many seconds (default 30, or
    /// $CREWBOARD_EMPTY_GRACE).
    #[arg(long, allow_negative_numbers = true, hide = true)]
    pub empty_grace: Option<i64>,
    /// The state of this agent (default: see `--print-state-dir`).
    #[arg(long, hide = true)]
    pub state_dir: Option<PathBuf>,
    /// The wake lock dir (default `<state dir>/wake.lock`).
    #[arg(long, hide = true)]
    pub lock_dir: Option<PathBuf>,
    /// The log file (default `<state dir>/wake.log`).
    #[arg(long, hide = true)]
    pub log: Option<PathBuf>,
    /// The events file of the wake: `poll` and `mine` are written there before the wake command
    /// starts, and the command gets its path as `CREWBOARD_EVENTS_FILE`.
    #[arg(long, hide = true)]
    pub events_file: Option<PathBuf>,
    /// Time budget of one wake: 45m (default, or $CREWBOARD_MAX_WAKE), 90s, 2h, whole seconds, or
    /// `off`. At the end the wake command is stopped, the coordinator is told, and the host locks
    /// of the agent are released.
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub max_wake: Option<String>,
    /// Seconds between two heartbeats during a wake (default 300, or $CREWBOARD_HEARTBEAT_EVERY;
    /// 0 sends none).
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub heartbeat_every: Option<String>,
    /// Print the state dir and exit 0. Starts nothing.
    #[arg(long, hide = true)]
    pub print_state_dir: bool,
    #[command(flatten)]
    pub agent_job: super::job::AgentArgs,
    /// Test seam: the command that the wake runs, with `sh -c` (default $CREWBOARD_WAKE_CMD).
    #[arg(long, hide = true, allow_hyphen_values = true)]
    pub wake_cmd: Option<String>,
    /// Test seam: make the calls to the board through this binary instead of in this process.
    #[arg(long, hide = true)]
    pub crewboard: Option<String>,
}

/// The global options of the command line that `wake` reads.
#[derive(Debug, Clone, Default)]
pub struct GlobalFlags {
    pub config_dir: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
    /// This program, for the command of the events hook (default: the running binary).
    pub exe: Option<PathBuf>,
}

impl WakeArgs {
    /// The config dir of the run: `--config-dir`, else the agent dir.
    pub fn config_dir(&self, global: Option<&Path>) -> Option<PathBuf> {
        global
            .map(Path::to_path_buf)
            .or_else(|| self.agent_dir.clone())
    }
}

/// The settings of one run.
#[derive(Debug, Clone)]
pub struct Settings {
    pub wake_cmd: Option<String>,
    pub tick: bool,
    pub agent: Option<String>,
    pub initiative: Option<String>,
    pub config_dir: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
    pub paths: Paths,
    pub stale_after: i64,
    pub empty_grace: i64,
    pub work_backoff: WorkBackoff,
    pub retry_delays: Vec<u64>,
    pub events_file: Option<PathBuf>,
    pub crewboard: Option<String>,
    /// The wake command of a dev agent (`--runtime`); else `wake_cmd` runs with `sh -c`.
    pub direct: Option<Direct>,
    pub max_wake: MaxWake,
    /// Seconds between heartbeats; 0 sends none.
    pub heartbeat_every: u64,
    pub host_locks: Vec<PathBuf>,
    pub work_dir: Option<PathBuf>,
    /// The per-initiative state dir of an older version, with the default per-agent state dir only.
    pub legacy_dir: Option<PathBuf>,
    /// The env file could not be loaded (not found, a mode or owner that is not private, a bad
    /// line, no token). Set by `cmd::run`; the calls of the run then fail with exit 4.
    pub env_load_failed: bool,
}

/// The time budget of a wake: the seconds (0: none) and the text as it was given, for the messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaxWake {
    pub secs: u64,
    pub text: String,
}

/// A flag or value that is wrong. `paths` is set when the state dir is known by then: the line
/// then also goes to the log. `code` is the exit code (4, or 64 for a negative number).
#[derive(Debug, Clone)]
pub struct BadFlag {
    pub text: String,
    pub paths: Option<Box<Paths>>,
    pub code: u8,
}

pub(super) fn bad(text: impl Into<String>) -> BadFlag {
    BadFlag {
        text: text.into(),
        paths: None,
        code: 4,
    }
}

/// A wrong value once the state dir is known: the line also goes to the log.
fn bad_logged(paths: &Paths, text: String) -> BadFlag {
    BadFlag {
        text,
        paths: Some(Box::new(paths.clone())),
        code: 4,
    }
}

/// What the flags ask for.
#[derive(Debug)]
pub enum Plan {
    /// `--print-state-dir`.
    PrintStateDir(PathBuf),
    /// `--print-cmd`: the text of the wake command.
    PrintCmd(String),
    Run(Box<Settings>),
}

fn nonempty(s: Option<&str>) -> Option<&str> {
    s.filter(|v| !v.is_empty())
}

fn nonempty_path(p: Option<&PathBuf>) -> Option<&Path> {
    p.map(PathBuf::as_path)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Who the run is: the config dir, the initiative and the agent name.
struct Names {
    config_dir: Option<PathBuf>,
    initiative: Option<String>,
    agent: Option<String>,
}

impl Names {
    fn of(a: &WakeArgs, g: &GlobalFlags, agent: Option<&AgentConfig>) -> Names {
        Names {
            config_dir: a
                .config_dir(g.config_dir.as_deref())
                .filter(|p| !p.as_os_str().is_empty()),
            initiative: nonempty(a.initiative.as_deref())
                .map(str::to_string)
                .or_else(|| agent.map(|c| c.board.initiative.clone())),
            agent: nonempty(a.agent.as_deref())
                .map(str::to_string)
                .or_else(|| agent.map(|c| c.name.clone())),
        }
    }
}

/// The paths that the flags name, made absolute: the wake command starts in another dir, and the
/// prompt and the hook name these paths.
struct Named {
    state_dir: Option<PathBuf>,
    lock_dir: Option<PathBuf>,
    log: Option<PathBuf>,
}

impl Named {
    fn of(a: &WakeArgs) -> Named {
        Named {
            state_dir: nonempty_path(a.state_dir.as_ref()).map(wake_job::abs),
            lock_dir: nonempty_path(a.lock_dir.as_ref()).map(wake_job::abs),
            log: nonempty_path(a.log.as_ref()).map(wake_job::abs),
        }
    }

    /// `--state-dir` or `--lock-dir` is given: no default root, and no older state dir.
    fn explicit(&self) -> bool {
        self.state_dir.is_some() || self.lock_dir.is_some()
    }

    fn key<'a>(&'a self, n: &'a Names) -> StateKey<'a> {
        StateKey {
            state_dir: self.state_dir.as_deref(),
            lock_dir: self.lock_dir.as_deref(),
            config_dir: n.config_dir.as_deref(),
            agent: n.agent.as_deref(),
            initiative: n.initiative.as_deref(),
        }
    }
}

/// The state dir of this agent.
fn state_dir_of(
    key: &StateKey,
    named: &Named,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<PathBuf, BadFlag> {
    if named.explicit() {
        return Ok(state::state_dir(key, Path::new("")));
    }
    let root = state::state_root(env("XDG_STATE_HOME").as_deref(), env("HOME").as_deref())
        .ok_or_else(|| bad("no state dir: HOME is not set; pass --state-dir"))?;
    Ok(state::state_dir(key, &root))
}

/// The state dir of an older per-initiative version, with the default state dir only.
fn legacy_dir_of(
    key: &StateKey,
    named: &Named,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<PathBuf> {
    if named.explicit() {
        return None;
    }
    state::state_root(env("XDG_STATE_HOME").as_deref(), env("HOME").as_deref())
        .and_then(|root| state::legacy_dir(key, &root))
}

/// The wake command of a dev agent, from its options; `None` without `--runtime`.
fn direct_of(
    a: &WakeArgs,
    g: &GlobalFlags,
    env: &dyn Fn(&str) -> Option<String>,
    names: &Names,
    paths: &Paths,
    events_file: &Path,
) -> Result<Option<Direct>, BadFlag> {
    wake_job::build(&wake_job::Inputs {
        a: &a.agent_job,
        tick: a.tick,
        has_wake_cmd: nonempty(a.wake_cmd.as_deref()).is_some(),
        agent: names.agent.as_deref(),
        initiative: names.initiative.as_deref(),
        config_dir: names.config_dir.as_deref(),
        paths,
        events_file,
        crewboard: nonempty(a.crewboard.as_deref()),
        exe: g.exe.as_deref(),
        path_env: env("PATH").as_deref(),
        home: env("HOME")
            .as_deref()
            .filter(|h| !h.is_empty())
            .map(Path::new),
    })
    .map_err(|text| BadFlag {
        paths: text
            .contains("is gone (moved or removed)")
            .then(|| Box::new(paths.clone())),
        ..bad(text)
    })
}

/// The numbers of a run, each from its flag, its variable or its default.
struct Numbers {
    work_backoff: WorkBackoff,
    retry_delays: Vec<u64>,
    empty_grace: i64,
    heartbeat_every: u64,
    max_wake: MaxWake,
    stale_after: i64,
}

fn numbers(
    a: &WakeArgs,
    env: &dyn Fn(&str) -> Option<String>,
    paths: &Paths,
) -> Result<Numbers, BadFlag> {
    let bad_logged = |text: String| bad_logged(paths, text);
    let wb_raw = a
        .work_backoff
        .clone()
        .or_else(|| env("CREWBOARD_WORK_BACKOFF"))
        .unwrap_or_else(|| policy::DEFAULT_WORK_BACKOFF.to_string());
    let work_backoff = WorkBackoff::parse(&wb_raw).ok_or_else(|| {
        bad_logged(format!(
            "--work-backoff needs whole seconds or off, got '{wb_raw}'"
        ))
    })?;
    let rd_raw = a
        .retry_delays
        .clone()
        .or_else(|| env("CREWBOARD_RETRY_DELAYS"))
        .unwrap_or_else(|| policy::DEFAULT_RETRY_DELAYS.to_string());
    let retry_delays = policy::parse_delays(&rd_raw).ok_or_else(|| {
        bad_logged(format!(
            "--retry-delays needs whole seconds, got '{rd_raw}'"
        ))
    })?;
    let empty_grace = match (a.empty_grace, env("CREWBOARD_EMPTY_GRACE")) {
        (Some(n), _) => n,
        (None, None) => policy::DEFAULT_EMPTY_GRACE,
        (None, Some(v)) => v.trim().parse().map_err(|_| {
            bad_logged(format!(
                "CREWBOARD_EMPTY_GRACE needs a number of seconds, got '{v}'"
            ))
        })?,
    };
    let hb_raw = a
        .heartbeat_every
        .clone()
        .or_else(|| env("CREWBOARD_HEARTBEAT_EVERY"))
        .unwrap_or_else(|| "300".to_string());
    let heartbeat_every = hb_raw
        .parse::<u64>()
        .ok()
        .filter(|_| hb_raw.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| {
            bad_logged(format!(
                "--heartbeat-every needs whole seconds, got '{hb_raw}'"
            ))
        })?;
    let mw_raw = a
        .max_wake
        .clone()
        .or_else(|| env("CREWBOARD_MAX_WAKE"))
        .unwrap_or_else(|| "45m".to_string());
    let max_wake = MaxWake {
        secs: super::policy::parse_max_wake(&mw_raw).ok_or_else(|| {
            bad_logged(format!(
                "--max-wake needs a duration (45m, 90s, 2h) or off, got '{mw_raw}'"
            ))
        })?,
        text: mw_raw,
    };
    let stale_after = a.stale_after.unwrap_or(policy::DEFAULT_STALE_AFTER);
    for (name, v) in [
        ("--stale-after", stale_after),
        ("--empty-grace", empty_grace),
    ] {
        if v < 0 {
            return Err(BadFlag {
                code: 64,
                ..bad_logged(format!(
                    "{name} needs a number of seconds of 0 or more, got {v}"
                ))
            });
        }
    }
    Ok(Numbers {
        work_backoff,
        retry_delays,
        empty_grace,
        heartbeat_every,
        max_wake,
        stale_after,
    })
}

/// The text of `--print-cmd`: the wake command, or the tick command.
fn print_cmd_text(direct: &Option<Direct>, tick: bool, paths: &Paths) -> Result<String, BadFlag> {
    match (direct, tick) {
        (Some(d), _) => Ok(d.text.clone()),
        (None, true) => Ok(wake_job::tick_text(&paths.log)),
        (None, false) => Err(bad("--print-cmd needs --runtime or --tick")),
    }
}

/// Read the flags, the agent config of `--agent-dir` and the environment into [`Settings`]. A
/// flag wins over the environment, the environment over the default. `env` reads one variable.
pub fn resolve(
    a: &WakeArgs,
    g: &GlobalFlags,
    env: &dyn Fn(&str) -> Option<String>,
    agent: Option<&AgentConfig>,
) -> Result<Plan, BadFlag> {
    let names = Names::of(a, g, agent);
    let named = Named::of(a);
    let key = named.key(&names);
    let dir = state_dir_of(&key, &named, env)?;
    if a.print_state_dir {
        return Ok(Plan::PrintStateDir(dir));
    }
    let legacy_dir = legacy_dir_of(&key, &named, env);
    let wake_cmd = nonempty(a.wake_cmd.as_deref())
        .map(str::to_string)
        .or_else(|| env("CREWBOARD_WAKE_CMD").filter(|v| !v.is_empty()));
    let paths = Paths::new(dir, named.lock_dir.clone(), named.log.clone());
    // the events file of a dev agent is in the state dir
    let events_file = nonempty_path(a.events_file.as_ref()).map(wake_job::abs);
    let default_events = paths.dir.join("events.txt");
    let direct = direct_of(
        a,
        g,
        env,
        &names,
        &paths,
        events_file.as_deref().unwrap_or(&default_events),
    )?;
    if a.agent_job.print_cmd {
        return print_cmd_text(&direct, a.tick, &paths).map(Plan::PrintCmd);
    }
    let wake_cmd = if direct.is_some() { None } else { wake_cmd };
    if !a.tick && wake_cmd.is_none() && direct.is_none() {
        return Err(bad(
            "no wake command (--wake-cmd, CREWBOARD_WAKE_CMD or --tick)",
        ));
    }
    let n = numbers(a, env, &paths)?;
    Ok(Plan::Run(Box::new(Settings {
        wake_cmd,
        tick: a.tick,
        agent: names.agent,
        initiative: names.initiative,
        config_dir: names.config_dir,
        env_file: g.env_file.clone(),
        paths,
        stale_after: n.stale_after,
        empty_grace: n.empty_grace,
        work_backoff: n.work_backoff,
        retry_delays: n.retry_delays,
        events_file: events_file.or_else(|| direct.as_ref().map(|_| default_events.clone())),
        crewboard: nonempty(a.crewboard.as_deref()).map(str::to_string),
        direct,
        max_wake: n.max_wake,
        heartbeat_every: n.heartbeat_every,
        host_locks: a.agent_job.host_lock.clone(),
        work_dir: a.agent_job.work_dir.clone(),
        legacy_dir,
        env_load_failed: false,
    })))
}
