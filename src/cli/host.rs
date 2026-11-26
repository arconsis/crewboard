//! The commands of the host: `doctor`, `lock`, `repeat`, `skill`, `update`, `wake`, `hook` and,
//! through `agent`, the agent setup.

#[cfg(not(unix))]
use super::refused;
use super::{
    Cli, HookAction, HostCmd, Outcome, RepeatArgs, SkillAction, agent, done, fail, global,
};
use crate::base::ignore::best_effort_output;
use crate::exit::Exit;
use crate::ops;
#[cfg(not(unix))]
use crate::rules::Refusal;
use std::path::PathBuf;

/// `repeat`, and its old name `gate run`: the same options, the same exit codes.
fn repeat_cmd(cli: &Cli, a: &RepeatArgs) -> Outcome {
    let opts = ops::repeat_cmd::Options {
        runs: a.times.clone(),
        lock: a.lock.clone(),
        owner: a.holder.clone(),
        lock_wait: a.lock_wait.clone(),
        log_dir: a.log_dir.clone(),
        repo: a.work_dir.clone(),
        result_re: a.result_re.clone(),
        fail_re: a.fail_re.clone(),
        no_ci_check: a.no_ci_check,
        ci_poll: a.ci_poll.clone(),
        ci_max_wait: a.ci_max_wait.clone(),
        cmd: a.cmd.clone(),
    };
    let code = ops::repeat_cmd::run(cli.config_dir.as_deref(), &opts);
    Outcome {
        exit: if code == 0 { Exit::Ok } else { Exit::Refused },
        code: Some(u8::try_from(code).unwrap_or(1)),
        message: None,
    }
}

/// `agent token` (old name `crew-token set`): read the token, check it, write the env file.
pub(super) fn token_cmd(
    initiative: Option<&str>,
    crew_login: Option<&str>,
    trial: bool,
) -> Outcome {
    done(ops::crew_token::run_set(&ops::crew_token::SetArgs {
        initiative,
        crew_login,
        trial,
    }))
}

/// `skill install` and `skill path`. Exit 1 when a link was not made (a code outside `Exit`).
#[inline(never)]
fn skill_cmd(action: &SkillAction) -> Outcome {
    match action {
        SkillAction::Path => done(ops::skill_cmd::path_line()),
        SkillAction::Install {
            runtimes,
            link,
            from,
            dry_run,
            ..
        } => {
            let args = ops::skill_cmd::InstallArgs {
                runtimes: runtimes.clone(),
                link: *link,
                from: from.clone(),
                dry_run: *dry_run,
            };
            match ops::skill_cmd::run_install(&args) {
                Ok(0) => done(Ok(String::new())),
                Ok(code) => Outcome {
                    exit: Exit::Refused,
                    code: Some(code),
                    message: None,
                },
                Err(e) => fail(e),
            }
        }
    }
}

/// `update`. Exit 3 when a step failed (a code that is the same as `Exit::Network`).
fn update_cmd(args: &ops::update_cmd::UpdateArgs) -> Outcome {
    match ops::update_cmd::run_update(args) {
        Ok(0) => done(Ok(String::new())),
        Ok(code) => Outcome {
            exit: Exit::Network,
            code: Some(code),
            message: None,
        },
        Err(e) => fail(e),
    }
}

/// `wake`. A build without Unix process control refuses in `run` before it gets here.
fn wake_cmd(cli: &Cli, w: &crate::wake::cmd::WakeArgs) -> Outcome {
    #[cfg(not(unix))]
    {
        let _ = (cli, w);
        refused(Refusal::new(
            crate::base::ids::UNIX_ONLY,
            crate::wake::cmd::UNIX_ONLY,
        ))
    }
    #[cfg(unix)]
    {
        let g = crate::wake::cmd::GlobalFlags {
            config_dir: cli.config_dir.clone(),
            env_file: cli.env_file.clone(),
            exe: None,
        };
        let code = crate::wake::cmd::run(w, &g);
        Outcome {
            exit: match code {
                0 => Exit::Ok,
                3 => Exit::Network,
                4 => Exit::Config,
                64 => Exit::Usage,
                _ => Exit::Refused,
            },
            code: Some(code),
            message: None,
        }
    }
}

/// `wake install|uninstall|status`: the scheduler unit of an agent.
fn wake_manage(cli: &Cli, action: &crate::wake::install::WakeAction) -> Outcome {
    let f = crate::wake::install::Flags {
        config_dir: cli.config_dir.clone(),
        env_file: cli.env_file.clone(),
    };
    done(crate::wake::install::run(action, &f))
}

/// `hook post-tool-use`: the hook JSON, if any, goes to standard output. The exit code is 0.
fn hook_cmd(cli: &Cli, action: &HookAction) -> Outcome {
    let HookAction::PostToolUse(a) = action;
    if let Some(text) = crate::wake::hook_cmd::run(a, cli.config_dir.clone()) {
        // a closed standard output must not turn the hook into a panic or a non-zero exit
        use std::io::Write;
        best_effort_output(writeln!(std::io::stdout(), "{text}"));
    }
    Outcome {
        exit: Exit::Ok,
        code: Some(0),
        message: None,
    }
}

#[inline(never)]
/// The check `wake` of `doctor --setup`: the health of the scheduled wakes of the agent, from
/// its state dir on this host. Without the agent config there is nothing to read.
fn wake_check(agent: Option<(&std::path::Path, &str)>) -> Vec<ops::doctor::ExtraCheck> {
    use crate::wake::health;
    use ops::doctor::CheckVerdict;
    let env = |k: &str| std::env::var(k).ok();
    let verdict = match agent {
        None => CheckVerdict::Skip("needs the agent config".into()),
        Some((dir, initiative)) => match health::state_dir(
            dir,
            initiative,
            env("XDG_STATE_HOME").as_deref(),
            env("HOME").as_deref(),
        ) {
            None => CheckVerdict::Skip("no state dir: HOME is not set".into()),
            Some(state) => wake_verdict(&health::read(&state), &state),
        },
    };
    vec![("wake".to_string(), verdict)]
}

/// The line of one wake health. Only a state that stops the work of the agent is a FAIL.
fn wake_verdict(
    h: &crate::wake::health::Health,
    state: &std::path::Path,
) -> ops::doctor::CheckVerdict {
    use crate::wake::health::Health;
    use ops::doctor::CheckVerdict;
    let log = state.join("wake.log");
    match h {
        Health::NeverRan => CheckVerdict::Skip(
            "no scheduled wake has run for this agent on this host (crewboard wake install sets one up)".into(),
        ),
        Health::Running { last: None } => {
            CheckVerdict::Ok("the wakes run; no wake command has run yet".into())
        }
        Health::Running { last: Some(t) } => CheckVerdict::Ok(format!(
            "the wakes run; the last wake command ended {}",
            crate::clock::format_ts(*t)
        )),
        Health::Failing { fails } => CheckVerdict::failed(
            format!("the wake command failed {fails} times in a row: the agent does no work, and the board still shows it alive"),
            format!(
                "read the last lines of {} (a login of the runtime that ended, a missing program), repair it; the next wake goes on by itself",
                log.display()
            ),
        ),
        Health::Stopped { line } => CheckVerdict::failed(
            format!("the wakes are stopped ({line})"),
            format!(
                "repair the config or the token, then remove {}; the board is also tried once per hour",
                state.join("stopped").display()
            ),
        ),
        Health::Network { fails } => CheckVerdict::failed(
            format!("GitHub or the network failed in {fails} runs in a row"),
            format!("check the network and the token; see {}", log.display()),
        ),
    }
}

pub(super) fn dispatch(cli: &Cli, c: &HostCmd) -> Outcome {
    match c {
        HostCmd::Doctor {
            setup: true,
            work_dir,
            sel,
            ..
        } => {
            let wd = work_dir
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let (text, verdict) =
                ops::doctor::setup(&global(cli, sel, false), &wd, &|agent| wake_check(agent));
            print!("{text}");
            match verdict {
                Ok(()) => Outcome {
                    exit: Exit::Ok,
                    code: None,
                    message: None,
                },
                Err(e) => fail(e),
            }
        }
        HostCmd::Agent { action } => agent::run(cli, action),
        HostCmd::Wake {
            action: Some(a), ..
        } => wake_manage(cli, a),
        HostCmd::Wake { w, .. } => wake_cmd(cli, w),
        HostCmd::Hook { action } => hook_cmd(cli, action),
        HostCmd::Lock { args } => {
            let code = ops::lock_cmd::run(cli.config_dir.as_deref(), args);
            Outcome {
                exit: if code == 0 { Exit::Ok } else { Exit::Refused },
                code: Some(u8::try_from(code).unwrap_or(1)),
                message: None,
            }
        }
        HostCmd::Skill { action } => skill_cmd(action),
        HostCmd::Update {
            release,
            from,
            repo,
            check,
        } => update_cmd(&ops::update_cmd::UpdateArgs {
            release: release.clone(),
            from: from.clone(),
            repo: repo.clone(),
            check: *check,
        }),
        HostCmd::Repeat(a) => repeat_cmd(cli, a),
        HostCmd::Doctor {
            repo_move: true,
            sel,
            ..
        } => match ops::repo_migration::run(&global(cli, sel, false)) {
            Ok(text) => {
                print!("{text}");
                Outcome {
                    exit: Exit::Ok,
                    code: None,
                    message: None,
                }
            }
            Err(e) => fail(e),
        },
        HostCmd::Doctor { board, sel, .. } => {
            let (host, host_verdict) = ops::doctor::host_report(&ops::doctor::Host::probe());
            print!("{host}");
            let (text, verdict) = ops::doctor::run(cli.config_dir.as_deref());
            print!("{text}");
            if *board {
                print!(
                    "{}",
                    ops::doctor::board_report(&global(cli, sel, false), verdict.is_ok())
                );
            } else {
                println!("{}", ops::doctor::BOARD_NOT_READ);
            }
            println!("{}", ops::doctor::SETUP_HINT);
            match verdict.and(host_verdict) {
                Ok(()) => Outcome {
                    exit: Exit::Ok,
                    code: None,
                    message: None,
                },
                Err(e) => fail(e),
            }
        }
    }
}

#[cfg(test)]
mod wake_check_tests {
    use super::{wake_check, wake_verdict};
    use crate::ops::doctor::CheckVerdict;
    use crate::wake::health::Health;
    use std::path::Path;

    fn verdict(h: Health) -> CheckVerdict {
        wake_verdict(&h, Path::new("/s/demo-dev"))
    }

    #[test]
    fn wakes_that_run_and_wakes_that_never_ran_are_no_fault() {
        assert!(matches!(verdict(Health::NeverRan), CheckVerdict::Skip(_)));
        assert!(matches!(
            verdict(Health::Running { last: None }),
            CheckVerdict::Ok(_)
        ));
        let CheckVerdict::Ok(text) = verdict(Health::Running { last: Some(0) }) else {
            panic!("ok expected");
        };
        assert!(text.contains("1970-01-01"), "{text}");
    }

    #[test]
    fn failed_wakes_fail_the_check_and_the_fix_names_the_log() {
        let CheckVerdict::Fail { what, fix } = verdict(Health::Failing { fails: 7 }) else {
            panic!("fail expected");
        };
        assert!(what.contains("failed 7 times in a row"), "{what}");
        let log = Path::new("/s/demo-dev").join("wake.log");
        assert!(fix.contains(&log.display().to_string()), "{fix}");
    }

    #[test]
    fn a_stop_fails_the_check_and_the_fix_names_the_stop_file() {
        let CheckVerdict::Fail { what, fix } = verdict(Health::Stopped {
            line: "poll: HTTP 401".into(),
        }) else {
            panic!("fail expected");
        };
        assert!(what.contains("poll: HTTP 401"), "{what}");
        let stop = Path::new("/s/demo-dev").join("stopped");
        assert!(fix.contains(&stop.display().to_string()), "{fix}");
        assert!(matches!(
            verdict(Health::Network { fails: 3 }),
            CheckVerdict::Fail { .. }
        ));
    }

    #[test]
    fn without_the_agent_config_the_check_is_skipped() {
        let got = wake_check(None);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "wake");
        assert!(matches!(got[0].1, CheckVerdict::Skip(_)));
    }
}
