//! `agent exec|env|with-env|add|shim|profile|token|selftest`.

#[cfg(not(unix))]
use super::refused;
use super::{AgentAction, Cli, Outcome, crew, host};
#[cfg(unix)]
use super::{done, fail};
#[cfg(unix)]
use crate::error::CbError;
#[cfg(unix)]
use crate::exit::Exit;
#[cfg(unix)]
use crate::ops;
#[cfg(not(unix))]
use crate::rules::Refusal;
#[cfg(unix)]
use std::path::PathBuf;

/// `agent profile|token|selftest` work on every host. The other actions need a Unix host.
pub(super) fn run(cli: &Cli, action: &AgentAction) -> Outcome {
    match action {
        AgentAction::Profile { action } => crew::profile_cmd(cli, action),
        AgentAction::Token {
            initiative,
            crew_login,
            insecure_one_account,
        } => host::token_cmd(
            initiative.as_deref(),
            crew_login.as_deref(),
            *insecure_one_account,
        ),
        AgentAction::Selftest { sel } => crew::selftest_cmd(cli, sel),
        _ => run_unix(cli, action),
    }
}

/// `agent exec|env|with-env|add|shim`. They need a Unix host: the refusal comes before any file
/// access.
fn run_unix(cli: &Cli, action: &AgentAction) -> Outcome {
    #[cfg(not(unix))]
    {
        let _ = (cli, action);
        refused(Refusal::new(
            ids::UNIX_ONLY,
            "crewboard agent needs a Unix host (macOS or Linux); this build has no loop on Windows",
        ))
    }
    #[cfg(unix)]
    {
        use ops::agent::Failed;
        let agent_dir = |d: &Option<PathBuf>| {
            crate::config::ConfigDir::resolve(d.as_deref().or(cli.config_dir.as_deref()))
        };
        let failed = |f: Failed| match f {
            Failed::Config(e) => fail(e),
            Failed::Start { code, text } => Outcome {
                exit: Exit::Config,
                code: Some(code),
                message: Some(text),
            },
        };
        match action {
            AgentAction::Profile { .. }
            | AgentAction::Token { .. }
            | AgentAction::Selftest { .. } => {
                unreachable!("`run` sends these three to their own commands")
            }
            AgentAction::Env { agent_dir: d } => match agent_dir(d) {
                Ok(dir) => match ops::agent::run_env(&dir.0) {
                    Ok(text) => done(Ok(text)),
                    Err(f) => failed(f),
                },
                Err(e) => fail(e),
            },
            AgentAction::Exec { agent_dir: d, cmd } => match agent_dir(d) {
                Ok(dir) => failed(ops::agent::run_exec(&dir.0, cmd)),
                Err(e) => fail(e),
            },
            AgentAction::WithEnv { cmd } => {
                if cli.env_file.is_none() {
                    return fail(CbError::usage("agent with-env: --env-file is required"));
                }
                failed(ops::agent::run_with_env(cmd))
            }
            AgentAction::Add {
                target,
                board_file,
                repo,
                agent_dir,
                agent,
                runtime,
                work_dir,
                crew_login,
                model,
                initiative,
                role,
                from,
                token_stdin,
                insecure_one_account,
                wait_approval,
                wait_interval,
                wait_max,
                install_wake,
                scheduler,
                every,
                allow,
                crewboard,
            } => {
                let code = ops::agent::run_add(&ops::agent::AddArgs {
                    target: target.clone(),
                    board_file: board_file.clone(),
                    repo: repo.clone(),
                    agent_dir: agent_dir.clone(),
                    name: agent.clone(),
                    runtime: *runtime,
                    work_dir: work_dir.clone(),
                    env_file: cli.env_file.clone(),
                    crew_login: crew_login.clone(),
                    model: model.clone(),
                    initiative: initiative.clone(),
                    role: *role,
                    from: from.clone(),
                    config_dir: cli.config_dir.clone(),
                    seam: crewboard.clone(),
                    token_stdin: *token_stdin,
                    insecure_one_account: *insecure_one_account,
                    wait_approval: *wait_approval,
                    wait_interval: *wait_interval,
                    wait_max: *wait_max,
                    install_wake: *install_wake,
                    scheduler: *scheduler,
                    every: every.clone(),
                    allow: allow.clone(),
                    installer: Some(wake_installer),
                });
                Outcome {
                    exit: match code {
                        0 => Exit::Ok,
                        2 => Exit::Refused,
                        3 => Exit::Network,
                        64 => Exit::Usage,
                        _ => Exit::Config,
                    },
                    code: Some(code),
                    message: None,
                }
            }
            AgentAction::Shim {
                agent_dir,
                work_dir,
                real_gh,
                crew_login,
                bin_dir,
                email,
                only_git,
            } => match ops::agent::run_shim(
                &ops::agent::ShimArgs {
                    agent_dir: agent_dir.clone(),
                    work_dir: work_dir.clone(),
                    real_gh: real_gh.clone(),
                    env_file: cli.env_file.clone(),
                    crew_login: crew_login.clone(),
                    bin_dir: bin_dir.clone(),
                    email: email.clone(),
                    only_git: *only_git,
                },
                cli.config_dir.as_deref(),
            ) {
                Ok(()) => Outcome {
                    exit: Exit::Ok,
                    code: None,
                    message: None,
                },
                Err(e) => fail(e),
            },
        }
    }
}

/// `--install-wake`: `wake install` in this process, for the agent that `agent add` set up. The
/// options are those of `crewboard wake install` for a dev agent, with the scheduler and the
/// interval of the flags (default `auto` and `5m`). It never passes `--replace`.
#[cfg(unix)]
fn wake_installer(
    spec: &ops::agent::WakeSpec,
    t: &ops::agent::WakeTarget<'_>,
) -> Result<String, CbError> {
    use crate::wake::install::{Flags, InstallArgs, Scheduler, WakeAction};
    use crate::wake::job::AgentArgs;
    use crate::wake::settings::WakeArgs;
    use ops::agent::AddScheduler;
    let scheduler = match spec.scheduler {
        AddScheduler::Auto => Scheduler::Auto,
        AddScheduler::Launchd => Scheduler::Launchd,
        AddScheduler::Systemd => Scheduler::Systemd,
        AddScheduler::Cron => Scheduler::Cron,
    };
    let args = InstallArgs {
        scheduler,
        every: spec.every.clone(),
        dry_run: false,
        replace: false,
        run: WakeArgs {
            agent_dir: Some(t.agent_dir.to_path_buf()),
            initiative: Some(t.initiative.to_string()),
            agent: Some(t.name.to_string()),
            agent_job: AgentArgs {
                runtime: Some(t.runtime.to_string()),
                work_dir: Some(t.work_dir.to_path_buf()),
                shim_dir: Some(t.agent_dir.join("bin")),
                allow: spec.allow.clone(),
                ..AgentArgs::default()
            },
            ..WakeArgs::default()
        },
    };
    let flags = Flags {
        config_dir: None,
        env_file: Some(t.env_file.to_path_buf()),
    };
    crate::wake::install::run(&WakeAction::Install(Box::new(args)), &flags)
}
