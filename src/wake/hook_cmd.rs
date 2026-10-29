//! `crewboard hook post-tool-use` (hidden): the events hook that a Claude wake runs after each
//! tool call (see `crate::wake::hook`). It reads `$CREWBOARD_EVENTS_FILE` and
//! `$CREWBOARD_HOOK_EVERY`, never reads standard input, writes the hook JSON to standard output
//! when there are new events, and always exits 0.
//!
//! The config dir is the global `--config-dir` (the wake sets `$CREWBOARD_CONFIG_DIR` in the
//! environment of Claude). `--crewboard BIN` is a test seam: make the poll through another binary.

use crate::clock::SystemClock;
use crate::wake::hook::{self, Input};
use crate::wake::steps::{InProcess, Steps, Subprocess};
use clap::Args;
use std::path::PathBuf;

/// The options of `hook post-tool-use`.
#[derive(Debug, Args, Clone, Default)]
pub struct PostToolUseArgs {
    /// Seconds between two polls (default 300, or $CREWBOARD_HOOK_EVERY; 0 polls at once).
    #[arg(long)]
    pub every: Option<String>,
    /// Append one line to this file for each delivery and each failure.
    #[arg(long)]
    pub log: Option<PathBuf>,
    /// The initiative (default: the only one of the config dir).
    #[arg(long)]
    pub initiative: Option<String>,
    /// Test seam: make the poll through this binary instead of in this process.
    #[arg(long, hide = true)]
    pub crewboard: Option<String>,
}

/// Run the hook. Returns the text for standard output; the exit code is always 0.
pub fn run(a: &PostToolUseArgs, config_dir: Option<PathBuf>) -> Option<String> {
    let events = std::env::var_os("CREWBOARD_EVENTS_FILE").map(PathBuf::from);
    let every_env = std::env::var("CREWBOARD_HOOK_EVERY").ok();
    let input = Input {
        events_file: events.as_deref(),
        every: a.every.as_deref().or(every_env.as_deref()),
        log: a.log.as_deref(),
    };
    // outside a wake: no call to the board, not even the setup of one
    input.events_file.filter(|p| !p.as_os_str().is_empty())?;
    let initiative = a.initiative.clone().filter(|s| !s.is_empty());
    let mut steps: Box<dyn Steps> = match a.crewboard.as_deref() {
        Some(bin) if !crate::ops::lock_cmd::is_this_binary(bin) => Box::new(Subprocess {
            bin: bin.to_string(),
            config_dir,
            initiative,
        }),
        _ => Box::new(InProcess::new(crate::ctx::Global {
            config_dir,
            initiative,
            ..crate::ctx::Global::default()
        })),
    };
    hook::run(&input, &mut *steps, &SystemClock)
}
