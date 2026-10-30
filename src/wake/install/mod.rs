//! `crewboard wake install`, `uninstall` and `status`: the scheduler unit of an agent. The
//! unit names, the file names and the unit texts are stable across releases, so an earlier
//! install is found and replaced, not doubled.
//!
//! An install writes a wrapper script (it stores the PATH of the installing shell and the
//! absolute path of crewboard and runs `crewboard wake` with the options of the agent) and the
//! unit of the scheduler, then enables the unit. `--dry-run` prints both and the commands, and
//! changes nothing. The parts: `units` (the templates), `plan` (what to write and run, pure),
//! `apply` (the writes and the commands), `sys` (the host seam).

mod apply;
pub mod foreign;
pub mod plan;
pub mod sys;
pub mod units;

// The installer writes launchd, systemd and cron units: its tests check Unix paths and texts.
#[cfg(all(test, unix))]
mod tests;

use crate::wake::settings::WakeArgs;
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

/// The scheduler that runs the wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scheduler {
    /// launchd on macOS; systemd --user on Linux when it works, else cron; hermes for a Hermes agent.
    Auto,
    Launchd,
    Systemd,
    Cron,
    Hermes,
}

impl Scheduler {
    pub fn label(self) -> &'static str {
        match self {
            Scheduler::Auto => "auto",
            Scheduler::Launchd => "launchd",
            Scheduler::Systemd => "systemd",
            Scheduler::Cron => "cron",
            Scheduler::Hermes => "hermes",
        }
    }
}

/// The operating system, as far as the units differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux,
    Other,
}

/// The subcommands of `wake`.
#[derive(Debug, Subcommand, Clone)]
pub enum WakeAction {
    /// Install and enable the scheduler unit of one agent.
    ///
    /// The unit (launchd, systemd or cron) runs `crewboard wake` for the agent every few
    /// minutes. It writes a wrapper script next to the agent config and the unit, then enables it. An
    /// install of an earlier version (also of `agent-wake.sh`) is replaced, not doubled. Prints
    /// the one command that you still have to run, if there is one. With `--dry-run` it prints
    /// the files and the commands and changes nothing.
    Install(Box<InstallArgs>),
    /// Disable and remove the scheduler unit of one agent.
    ///
    /// Removes the unit and the wrapper. The state and the log stay.
    Uninstall(UnitArgs),
    /// Show the unit of one agent, if it is enabled, and its last run.
    Status(UnitArgs),
}

/// The options of `wake install`: the options of the wake (they go into the wrapper) and the
/// options of the unit.
#[derive(Debug, Args, Clone)]
pub struct InstallArgs {
    /// The scheduler of the unit.
    #[arg(long, value_enum, default_value = "auto")]
    pub scheduler: Scheduler,
    /// The interval between two runs, for example 5m, 90s or 1h.
    #[arg(long, default_value = "5m")]
    pub every: String,
    /// Print the files and the commands, and change nothing.
    #[arg(long)]
    pub dry_run: bool,
    /// Remove the wake jobs of other names that run this agent (a copy of each file stays as
    /// `<file>.bak-<date>`). Without it, such a job makes the install refuse with exit 2.
    #[arg(long)]
    pub replace: bool,
    #[command(flatten)]
    pub run: WakeArgs,
}

/// The options of `wake uninstall` and `wake status`.
#[derive(Debug, Args, Clone, Default)]
pub struct UnitArgs {
    /// The config dir of the agent (the names of the unit come from it).
    #[arg(long)]
    pub agent_dir: Option<PathBuf>,
    /// The initiative (default: the only one of the agent dir).
    #[arg(long)]
    pub initiative: Option<String>,
    /// Only this scheduler (default: every scheduler that has a unit of this agent).
    #[arg(long, value_enum)]
    pub scheduler: Option<Scheduler>,
}

/// The global flags that the subcommands read.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub config_dir: Option<PathBuf>,
    pub env_file: Option<PathBuf>,
}

/// Run a subcommand. The text goes to standard output; an error is a wrong option or a failed
/// scheduler command (exit 4).
pub fn run(action: &WakeAction, f: &Flags) -> Result<String, crate::error::CbError> {
    apply::run(action, f)
}
