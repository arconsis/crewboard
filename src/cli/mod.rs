//! Command tree (spec/cli#commands) and dispatch.
//!
//! Every command of the spec exists and does real work. `args` holds the clap types, `scope`
//! the verdict of the headless scope for each command, and one module per command group runs
//! the commands: `crew`, `dev`, `plan`, `review`, `watch`, `host` and `agent`. `dispatch` here only routes.

// The doc comments of the args are the `--help` texts that clap prints as they are; see `args`.
#![allow(rustdoc::invalid_html_tags, rustdoc::bare_urls)]

mod agent;
mod args;
mod crew;
mod dev;
mod host;
pub mod overview;
mod plan;
mod review;
mod scope;
mod watch;

pub use args::*;
pub use watch::monitor_usage_line;

use crate::error::CbError;
use crate::exit::Exit;
use crate::ops::{self, Global};
use crate::rules::Refusal;
use std::path::PathBuf;

/// What a command did: an exit code and an optional line for stderr.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub exit: Exit,
    /// A raw exit code, for the commands that have codes outside the five of `Exit`
    /// (`lock`: 0, 1, 2, 3, 64, 75, 76, 77, or the code of its command; `skill install`: 1 for a
    /// `SKIP:`, 3 for a `FAILED:` step).
    pub code: Option<u8>,
    pub message: Option<String>,
}

impl Outcome {
    /// The code of the process: the raw code when there is one, else the one of `exit`.
    pub fn exit_code(&self) -> u8 {
        self.code.unwrap_or(self.exit.code())
    }
}

fn refused(r: Refusal) -> Outcome {
    Outcome {
        exit: Exit::Refused,
        code: None,
        message: Some(r.to_string()),
    }
}

fn fail(e: CbError) -> Outcome {
    Outcome {
        exit: e.exit(),
        code: None,
        message: Some(e.to_string()),
    }
}

/// An error of `watch`/`poll` also prints the status line (spec/cli#exit-codes).
fn fail_with_line(e: CbError, line: String) -> Outcome {
    Outcome {
        exit: e.exit(),
        code: None,
        message: Some(format!("{e}\n{line}")),
    }
}

fn done(r: Result<String, CbError>) -> Outcome {
    match r {
        Ok(text) => {
            print!("{text}");
            Outcome {
                exit: Exit::Ok,
                code: None,
                message: None,
            }
        }
        Err(e) => fail(e),
    }
}

fn global(cli: &Cli, sel: &Sel, insecure_one_account: bool) -> Global {
    Global {
        config_dir: cli.config_dir.clone(),
        initiative: sel.initiative.clone(),
        initiative_file: sel.initiative_file.clone(),
        insecure_one_account,
    }
}

/// Does the command read the token? Those that do not must work when the env file is broken,
/// so that `doctor` can report it and `agent exec` can start a shell.
fn needs_env_file(c: &Command) -> bool {
    // `lock take|release|status|break|run` need no token; their board check loads the file
    // itself (`ci-wait`, `ci-done` and `ci-status` load it here)
    let lock_without_token = matches!(
        c,
        Command::Host(HostCmd::Lock { args }) if matches!(
            args.first().map(String::as_str),
            Some("take" | "release" | "status" | "break" | "run")
        )
    );
    // `wake --print-state-dir` starts nothing and reads no token
    let print_only = matches!(
        c,
        Command::Host(HostCmd::Wake { w, action: None }) if w.print_state_dir
    ) || matches!(
        c,
        // install, uninstall and status write or read units, never the board
        Command::Host(HostCmd::Wake {
            action: Some(_),
            ..
        })
    );
    !lock_without_token
        && !print_only
        && !matches!(
            c,
            Command::Host(HostCmd::Doctor { .. })
                | Command::Host(HostCmd::Update { .. })
                | Command::Plan(PlanCmd::Ticket {
                    action: TicketAction::New { template: true, .. }
                })
                | Command::Host(HostCmd::Agent {
                    action: AgentAction::Exec { .. }
                        | AgentAction::Env { .. }
                        | AgentAction::Add { .. }
                        | AgentAction::Shim { .. }
                        | AgentAction::Token { .. }
                })
        )
}

/// The config dir that the env file loader and the commands use: `--config-dir`, except for
/// `wake --agent-dir`, which names the config dir of the agent.
fn config_dir_of(cli: &Cli) -> Option<PathBuf> {
    match &cli.command {
        Command::Host(HostCmd::Wake { w, .. }) => w.config_dir(cli.config_dir.as_deref()),
        _ => cli.config_dir.clone(),
    }
}

/// Load the env file once, before the headless guard reads the config dir and before any
/// command. A failure ends the command with exit 4; `monitor` prints it as its one line.
fn load_agent_env(cli: &Cli) -> Option<Outcome> {
    if !needs_env_file(&cli.command) {
        // the load still runs for `doctor` and `agent exec|env`, which report or ignore it.
        // `agent add` loads the env file itself after its first step, `agent shim` never needs it
        if (matches!(cli.command, Command::Host(HostCmd::Doctor { .. }))
            || matches!(
                cli.command,
                Command::Host(HostCmd::Agent {
                    action: AgentAction::Exec { .. } | AgentAction::Env { .. }
                })
            ))
            && let Ok(dir) = crate::config::ConfigDir::resolve(config_dir_of(cli).as_deref())
        {
            crate::agent_env::init(&dir.0, cli.env_file.as_deref());
        }
        return None;
    }
    // no config dir (no home) is a failure too: nothing may fall back to another token
    let loaded = match crate::config::ConfigDir::resolve(config_dir_of(cli).as_deref()) {
        Ok(dir) => crate::agent_env::init(&dir.0, cli.env_file.as_deref()),
        // An explicit env file needs no config dir: `agent with-env` (the gh stub) must work
        // without HOME, as a runtime may strip it. The file wins over everything, so no other
        // token can take its place. The dir is a path that holds no `agent.env`.
        Err(_) if cli.env_file.is_some() => {
            crate::agent_env::init(std::path::Path::new("/dev/null"), cli.env_file.as_deref())
        }
        Err(e) => crate::agent_env::init_error(e),
    };
    match loaded {
        // `wake` records the failure and goes on: its calls fail with exit 4 themselves, so the run
        // writes the stop file and tells the coordinator if it still can
        // `hook` ends with exit 0 whatever happens: its poll fails with exit 4 itself and says so in its log
        Err(_)
            if matches!(
                cli.command,
                Command::Host(HostCmd::Wake { .. }) | Command::Host(HostCmd::Hook { .. })
            ) =>
        {
            None
        }
        Err(e) => Some(match &cli.command {
            Command::Watch(WatchCmd::Monitor { m }) => {
                let reason = ops::monitor::first_line(&e.to_string());
                watch::monitor_fail(e.exit().code(), &reason, "", m.json)
            }
            // `watch --loop` is `monitor`: it ends with the same one line
            Command::Watch(WatchCmd::Watch {
                watch_loop: true,
                json,
                ..
            }) => {
                let reason = ops::monitor::first_line(&e.to_string());
                watch::monitor_fail(e.exit().code(), &reason, "", *json)
            }
            _ => fail(e.clone()),
        }),
        Ok(env) => {
            // `with-env` runs for every `gh` call of an agent and `wake` runs every few minutes:
            // both stay silent (`wake` hands the other keys to the wake command)
            if !matches!(
                cli.command,
                Command::Host(HostCmd::Agent {
                    action: AgentAction::WithEnv { .. }
                }) | Command::Host(HostCmd::Wake { .. })
            ) {
                for w in env.ignored_warnings() {
                    eprintln!("{w}");
                }
            }
            None
        }
    }
}

pub fn run(cli: &Cli) -> Outcome {
    // a build without Unix process control refuses before it reads any file
    #[cfg(not(unix))]
    if matches!(cli.command, Command::Host(HostCmd::Wake { .. })) {
        return refused(Refusal::new(
            crate::base::ids::UNIX_ONLY,
            crate::wake::cmd::UNIX_ONLY,
        ));
    }
    if let Some(early) = load_agent_env(cli) {
        return early;
    }
    let out = dispatch(cli);
    // an outcome with a raw code has the codes of its own command
    debug_assert!(out.code.is_some() || out.exit.allowed_for(cli.command.family()));
    out
}

/// Route a command to the module of its group. `Command` has one variant per group, so the
/// match is exhaustive and each group function matches its own enum without a catch-all arm.
///
/// Every group function is `#[inline(never)]`: a debug build adds the locals of every arm to one
/// stack frame, and a Windows main thread has 1 MiB of stack. `main` runs the work on a 16 MiB
/// thread; the attribute guards the fallback path, where that thread cannot start.
fn dispatch(cli: &Cli) -> Outcome {
    // spec/agents#headless-coordinator: a headless config refuses what is out of scope, before any
    // config load or request.
    if let Err(e) = scope::guard(&cli.command, cli.config_dir.as_deref()) {
        return fail(e);
    }
    match &cli.command {
        Command::Crew(c) => crew::dispatch(cli, c),
        Command::Dev(c) => dev::dispatch(cli, c),
        Command::Plan(c) => plan::dispatch(cli, c),
        Command::Review(c) => review::dispatch(cli, c),
        Command::Watch(c) => watch::dispatch(cli, c),
        Command::Host(c) => host::dispatch(cli, c),
    }
}

#[cfg(test)]
mod tests;
