//! `crewboard agent exec`, `agent env`, `agent with-env`, `agent add` and `agent shim`: run a
//! command as one agent, and set an agent up.
//!
//! An agent dir is the config dir of one agent. `exec` and `env` build the environment that
//! makes a shell an agent shell: the `gh` shim first on the PATH, an empty `gh` config dir, the
//! config dir. No token goes into it (the shim loads the crew token for each `gh` call), and the
//! token variables of the caller go out. `with-env` runs one command with the token of an env
//! file. `add` sets one agent up in seven steps (and the headless tick of a coordinator in
//! three); `shim` writes the `gh` stub. All of them need a Unix host.

#[cfg(unix)]
mod add;
#[cfg(unix)]
mod coordinator;
mod env;
mod exec;
#[cfg(unix)]
mod install_wake;
mod scheduler;
#[cfg(unix)]
mod shim;
#[cfg(unix)]
mod wait;
mod with_env;

#[cfg(unix)]
pub use add::AddArgs;
pub use env::{env_lines, run_env};
pub use exec::{exec_env_plan, run_exec};
#[cfg(unix)]
pub use install_wake::{Installer, Target as WakeTarget, WakeSpec};
pub use scheduler::AddScheduler;
#[cfg(unix)]
pub use shim::{ShimArgs, run_shim};
pub use with_env::run_with_env;

/// Run `agent add`. It prints its own lines. The result is the exit code.
#[cfg(unix)]
pub fn run_add(a: &AddArgs) -> u8 {
    match a.role.unwrap_or(AddRole::Dev) {
        AddRole::HeadlessCoordinator if a.token_stdin => add::refuse_token_stdin_headless(),
        AddRole::HeadlessCoordinator if a.install_wake => add::refuse_install_wake_headless(),
        AddRole::HeadlessCoordinator => coordinator::run_headless(a),
        _ => add::run_add_steps(a),
    }
}

/// The runtime of an agent that `agent add` sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AgentRuntime {
    Claude,
    Codex,
    Hermes,
}

/// What `agent add` sets up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum AddRole {
    Dev,
    Coordinator,
    HeadlessCoordinator,
}

use crate::error::CbError;
use std::path::{Component, Path, PathBuf};

/// Why a command that should replace this process did not.
#[derive(Debug)]
pub enum Failed {
    /// A configuration problem (exit 4).
    Config(CbError),
    /// The command could not start. `code` is 127 (not found) or 126 (cannot run), as a shell says.
    Start { code: u8, text: String },
}

impl From<CbError> for Failed {
    fn from(e: CbError) -> Self {
        Failed::Config(e)
    }
}

/// The agent dir as an absolute path, without symlink resolution: the way `cd DIR && pwd` shows
/// it. `$PWD` is used for a relative path when it names the current directory.
pub(crate) fn absolute(dir: &Path) -> PathBuf {
    let base = if dir.is_absolute() {
        PathBuf::new()
    } else {
        logical_cwd()
    };
    let mut out = base;
    for c in dir.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn logical_cwd() -> PathBuf {
    let real = std::env::current_dir().unwrap_or_default();
    match std::env::var_os("PWD").map(PathBuf::from) {
        Some(p)
            if p.is_absolute()
                && std::fs::canonicalize(&p).ok() == std::fs::canonicalize(&real).ok() =>
        {
            p
        }
        _ => real,
    }
}

/// The error of a missing agent dir (exit 4).
pub(crate) fn missing_dir(dir: &Path) -> CbError {
    CbError::config(format!(
        "agent dir not found: {} (make it with: crewboard agent add ...)",
        dir.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn absolute_keeps_an_absolute_path_and_folds_dots() {
        assert_eq!(absolute(Path::new("/a/b")), PathBuf::from("/a/b"));
        assert_eq!(absolute(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn absolute_puts_a_relative_path_under_the_current_dir() {
        let a = absolute(Path::new("x/y"));
        assert!(a.is_absolute(), "{a:?}");
        assert!(a.ends_with("x/y"), "{a:?}");
    }
}
