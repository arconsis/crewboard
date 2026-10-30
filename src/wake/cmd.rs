//! `crewboard wake` (hidden): the headless wake for cron, launchd, systemd timers and
//! Hermes cron. One run does this:
//!
//! 1. A stop file (an earlier exit 4) ends the run at once, silently, until the env file or the
//!    agent config is newer than it.
//! 2. A net back-off file (earlier exits 3) ends the run as a quiet poll while its pause lasts.
//! 3. The precheck: `poll --quiet`. Exit 1 means no events; the run goes on only for events that an
//!    earlier wake kept in `<events file>.unhandled`, a finished detached job that nobody reported,
//!    or owned work whose back-off is over (`mine --quiet`). Exit 0 goes on. Any other code ends
//!    the run (see `Flow::fail`). The back-off file is cleared only when the whole precheck succeeded.
//! 4. The wake lock (`crate::wake::lockdir`).
//! 5. `--tick`: `tick` instead of a model. Otherwise, with `--events-file`, the events file is
//!    written (`poll`, which moves the cursor, and `mine`; `crate::wake::events`), the finished
//!    results are reported once, and each event gets a receipt line in the log. Then the wake
//!    command runs under the lock, in its own process group. A wake that fails, or that a signal
//!    stops, keeps the events file in `<events file>.unhandled`; after a wake with exit 0 the
//!    events that need an act and got none go there once (`acted --since`). TERM, INT and HUP
//!    stop it (the group gets TERM, after a grace KILL), then the lock goes and the run exits with
//!    128 plus the signal.
//!
//! Exit codes: 0 nothing to do, woke OK, a wake already runs, or an earlier exit 4 stopped the
//! wakes; 2, 3, 4 from the board calls; 64 a negative number of seconds; 70 the wake command
//! failed; 71 the lock cannot be taken. Standard output stays empty. Messages go to standard
//! error and to the log.
//!
//! The calls to the board are a [`Steps`]; the hidden `--crewboard BIN` runs another binary
//! instead of the calls in this process. `CREWBOARD_KILL_GRACE` (seconds, default 10, at most
//! 3600) is a test seam for the grace between TERM and KILL.
//!
//! The parts: `settings` (the options and the settings of a run), `flow` (the run as a sequence of
//! phases: `preflight`, `precheck`, the lock, `tick` or `wake_events` and `finish`, with `runtime` and
//! `procs` for the wake command). This module is the entry: it makes the settings, the calls and
//! the log, and runs the flow.

#[cfg(unix)]
use super::flow::Flow;
#[cfg(unix)]
use super::job as wake_job;
#[cfg(unix)]
use super::procs::caught_signal;
#[cfg(unix)]
pub use super::settings::GlobalFlags;
pub use super::settings::WakeArgs;
#[cfg(unix)]
use super::settings::{BadFlag, Plan, Settings, bad, resolve};
#[cfg(unix)]
use crate::base::ignore::best_effort_output;
#[cfg(unix)]
use crate::clock::{Clock, SystemClock, format_ts};
#[cfg(unix)]
use crate::config::ConfigDir;
#[cfg(unix)]
use crate::lock::procs::SystemProcs;
#[cfg(unix)]
use crate::wake::state::Log;
#[cfg(unix)]
use crate::wake::steps::{InProcess, Steps, Subprocess};
#[cfg(unix)]
use std::cell::Cell;

/// The refusal of a build without Unix process control.
#[cfg(not(unix))]
pub const UNIX_ONLY: &str =
    "crewboard wake needs a Unix host (macOS or Linux); this build has no loop on Windows";

// --- the command -----------------------------------------------------------------------------

/// A bad flag goes to standard error; once the state dir is known also to the log.
#[cfg(unix)]
fn report_bad(b: &BadFlag, clock: &dyn Clock) {
    let line = format!("{} poll-and-wake: {}\n", format_ts(clock.now()), b.text);
    eprint!("{line}");
    if let Some(p) = &b.paths {
        std::fs::create_dir_all(&p.dir).ok(); // a missing dir shows as an error at the next write
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p.log)
        {
            use std::io::Write;
            best_effort_output(f.write_all(line.as_bytes()));
        }
    }
}

/// The calls to the board: in this process, or through `--crewboard BIN` when that is another
/// program.
#[cfg(unix)]
fn make_steps(s: &Settings, global: &crate::ctx::Global) -> Box<dyn Steps> {
    match s.crewboard.as_deref() {
        Some(bin) if !crate::ops::lock_cmd::is_this_binary(bin) => Box::new(Subprocess {
            bin: bin.to_string(),
            config_dir: s.config_dir.clone(),
            initiative: s.initiative.clone(),
        }),
        _ => Box::new(InProcess::new(global.clone())),
    }
}

/// Run `crewboard wake`. The result is the exit code. A build without Unix process control
/// refuses in `cli::run` before it gets here.
#[cfg(unix)]
pub fn run(a: &WakeArgs, g: &GlobalFlags) -> u8 {
    let clock = SystemClock;
    // A scheduler starts a program with a minimal PATH: add the install dirs of crewboard at the
    // end, for the wake command and for the calls through `--crewboard`.
    if let Some(path) = std::env::var_os("PATH").and_then(|p| p.into_string().ok()) {
        let home = std::env::var("HOME").ok();
        let fixed = wake_job::with_install_dirs(&path, home.as_deref(), &|d| d.is_dir());
        if fixed != path {
            // SAFETY: this runs at the start of the command, before any thread exists
            unsafe { std::env::set_var("PATH", fixed) };
        }
    }
    let g = &GlobalFlags {
        exe: g.exe.clone().or_else(|| std::env::current_exe().ok()),
        ..g.clone()
    };
    let env = |k: &str| std::env::var(k).ok();
    // the agent config of --agent-dir: its names
    let agent = match &a.agent_dir {
        Some(d) => match ConfigDir::resolve(Some(d))
            .and_then(|cd| cd.load_agent_auto(a.initiative.as_deref()))
        {
            Ok(c) => Some(c),
            Err(e) => {
                report_bad(&bad(e.to_string()), &clock);
                return e.exit().code();
            }
        },
        None => None,
    };
    let s = match resolve(a, g, &env, agent.as_ref()) {
        Ok(Plan::PrintStateDir(d)) => {
            println!("{}", d.display());
            return 0;
        }
        Ok(Plan::PrintCmd(text)) => {
            println!("{text}");
            return 0;
        }
        Ok(Plan::Run(s)) => *s,
        Err(b) => {
            report_bad(&b, &clock);
            return b.code;
        }
    };
    let mut s = s;
    s.env_load_failed = crate::agent_env::failure().is_some();
    let s = s;
    std::fs::create_dir_all(&s.paths.dir).ok(); // a missing dir shows as an error at the next write
    crate::lock::sys::catch_term_int_hup_restart();
    let log = Log::new(s.paths.log.clone(), &clock);
    let global = crate::ctx::Global {
        config_dir: s.config_dir.clone(),
        initiative: s.initiative.clone(),
        ..crate::ctx::Global::default()
    };
    let mut steps = make_steps(&s, &global);
    let host = crate::lock::sys::short_hostname().unwrap_or_else(|| "host".to_string());
    let mut flow = Flow {
        s: &s,
        steps: &mut *steps,
        clock: &clock,
        pids: &SystemProcs,
        log: &log,
        host: &host,
        sig: Cell::new(None),
        signals: &caught_signal,
        heartbeat_steps: Some(&|| make_steps(&s, &global)),
    };
    flow.execute()
}

#[cfg(all(test, unix))]
#[path = "cmd_tests.rs"]
mod tests;
