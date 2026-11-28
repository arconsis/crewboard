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
    let state = agent.and_then(|(dir, initiative)| {
        health::state_dir(
            dir,
            initiative,
            env("XDG_STATE_HOME").as_deref(),
            env("HOME").as_deref(),
        )
    });
    let today: String = crate::clock::now_ts().chars().take(10).collect();
    let Some(state) = state else {
        let why = match agent {
            None => "needs the agent config",
            Some(_) => "no state dir: HOME is not set",
        };
        return vec![("wake".to_string(), CheckVerdict::Skip(why.into()))];
    };
    let health = health::read(&state);
    // a unit with a model of the unattended coordinator: the tick and the model apart
    match health::coordinator(&state) {
        Some(w) => vec![
            ("wake".to_string(), tick_verdict(&health, &state)),
            (
                "unattended coordinator".to_string(),
                model_verdict(&w, &health::model(&state), &today, &state),
            ),
        ],
        None => vec![("wake".to_string(), wake_verdict(&health, &state))],
    }
}

/// The line of the tick of a unit with a model. A model that fails does not stop the tick, so
/// it is no fault of this line; a stop and a network fault are.
fn tick_verdict(
    h: &crate::wake::health::Health,
    state: &std::path::Path,
) -> ops::doctor::CheckVerdict {
    use crate::wake::health::Health;
    match h {
        Health::Running { .. } | Health::Failing { .. } => ops::doctor::CheckVerdict::Ok(
            "the tick runs (a unit with a model of the unattended coordinator)".into(),
        ),
        other => wake_verdict(other, state),
    }
}

/// The line of the model of the unattended coordinator: a model that fails in a row is a fault
/// (the tick still runs); a stored day later than today is a warning (no model starts until
/// then); else the mode, the runtime and the count of the last model wake, and the wakes in a
/// row that ended at the time budget.
fn model_verdict(
    w: &crate::wake::policy::CoordWakes,
    m: &crate::wake::health::Model,
    today: &str,
    state: &std::path::Path,
) -> ops::doctor::CheckVerdict {
    use ops::doctor::CheckVerdict;
    if m.fails >= crate::wake::health::FAILING_FROM {
        return CheckVerdict::failed(
            format!(
                "the model failed {} times in a row; the tick still runs, and no ticket gets a judgment",
                m.fails
            ),
            format!(
                "read the last lines of {} (a login of the runtime that ended, a missing program), repair it; the next model wake goes on by itself",
                state.join("wake.log").display()
            ),
        );
    }
    if w.day.as_str() > today {
        return CheckVerdict::Ok(format!(
            "warning: the count of model wakes is of {}, a later day than today ({today}, UTC): the clock of this host went back or was ahead, and no model starts until that day. Check the clock; to start again now, remove {}",
            w.day,
            state.join("coordinator-wakes").display()
        ));
    }
    let mut line = coordinator_line(w);
    if m.budget_stops >= 2 {
        line.push_str(&format!(
            "; warning: the last {} model wakes ended at the time budget (--max-wake), not by themselves",
            m.budget_stops
        ));
    }
    CheckVerdict::Ok(line)
}

/// The line of the model wakes of the unattended coordinator: the mode and the runtime of the
/// last start, and the count of its day. The mode is the one that the tick read then; the key
/// of AUTHORITY.md decides at each run.
fn coordinator_line(w: &crate::wake::policy::CoordWakes) -> String {
    if w.count == 0 {
        return format!(
            "mode {} and runtime {}; no model wake yet",
            w.mode, w.runtime
        );
    }
    format!(
        "mode {} and runtime {} at the last model wake ({}); {} of {} model wakes on {} (UTC)",
        w.mode,
        w.runtime,
        crate::clock::format_ts(w.last),
        w.count,
        crate::wake::policy::MAX_COORD_WAKES_PER_DAY,
        w.day
    )
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
        HostCmd::Check {
            names,
            rev,
            must_fail_at,
            work_dir,
            authority_file,
            sel,
        } => {
            let dir = work_dir
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let log_root = match super::check_log_root() {
                Ok(r) => r,
                Err(e) => return fail(e),
            };
            // a scope binds the caller as it binds `lock run`: a headless config dir, or a wake
            let scoped = match crate::headless::in_scope(cli.config_dir.as_deref()) {
                Ok(s) => s,
                Err(e) => return fail(e),
            };
            let wake = std::env::var_os("CREWBOARD_WAKE").is_some_and(|v| !v.is_empty());
            done(ops::tickets::check(
                &global(cli, sel, false),
                &ops::tickets::CheckArgs {
                    names,
                    rev,
                    must_fail_at: must_fail_at.as_deref(),
                    repo_dir: &dir,
                    log_root: &log_root,
                    authority_file: authority_file.as_deref(),
                    unattended: scoped || wake,
                },
            ))
        }
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
    fn the_line_of_the_unattended_coordinator_names_the_mode_the_runtime_and_the_count() {
        let w = crate::wake::policy::CoordWakes {
            day: "2026-11-28".into(),
            count: 2,
            last: 0,
            hash: "00ff".into(),
            mode: "verify".into(),
            runtime: "codex".into(),
            pending: false,
        };
        assert_eq!(
            super::coordinator_line(&w),
            "mode verify and runtime codex at the last model wake (1970-01-01T00:00:00Z); 2 of 6 model wakes on 2026-11-28 (UTC)"
        );
    }

    #[test]
    fn a_unit_with_a_model_shows_the_tick_and_the_model_apart() {
        use crate::wake::health::Model;
        let state = Path::new("/s/demo-coord");
        let w = |day: &str, count: u32| crate::wake::policy::CoordWakes {
            day: day.into(),
            count,
            last: 0,
            hash: "00ff".into(),
            mode: "verify".into(),
            runtime: "hermes".into(),
            pending: false,
        };
        let ok = |v: CheckVerdict| match v {
            CheckVerdict::Ok(t) => t,
            v => panic!("{v:?}"),
        };
        // a model that fails is no fault of the tick line, and a fault of the model line
        let tick = ok(super::tick_verdict(&Health::Failing { fails: 7 }, state));
        assert!(tick.starts_with("the tick runs"), "{tick}");
        assert!(!tick.contains("no wake command has run yet"), "{tick}");
        assert!(matches!(
            super::tick_verdict(&Health::Network { fails: 3 }, state),
            CheckVerdict::Fail { .. }
        ));
        let failing = Model {
            fails: 3,
            budget_stops: 0,
        };
        let CheckVerdict::Fail { what, fix } =
            super::model_verdict(&w("2026-11-28", 2), &failing, "2026-11-28", state)
        else {
            panic!("fail expected");
        };
        assert!(
            what.contains("the model failed 3 times in a row; the tick still runs"),
            "{what}"
        );
        assert!(!what.contains("does no work"), "{what}");
        assert!(fix.contains("/s/demo-coord/wake.log"), "{fix}");
        // a stored day after today is a warning, not a plain OK line
        let fine = Model::default();
        let t = ok(super::model_verdict(
            &w("2026-11-29", 1),
            &fine,
            "2026-11-28",
            state,
        ));
        assert!(
            t.starts_with(
                "warning: the count of model wakes is of 2026-11-29, a later day than today"
            ),
            "{t}"
        );
        assert!(t.contains("/s/demo-coord/coordinator-wakes"), "{t}");
        // the normal line, a unit that noted a list and started no model, and the budget stops
        let t = ok(super::model_verdict(
            &w("2026-11-28", 2),
            &fine,
            "2026-11-28",
            state,
        ));
        assert!(
            t.starts_with("mode verify and runtime hermes at the last model wake"),
            "{t}"
        );
        let t = ok(super::model_verdict(
            &w("2026-11-28", 0),
            &fine,
            "2026-11-28",
            state,
        ));
        assert_eq!(t, "mode verify and runtime hermes; no model wake yet");
        let slow = Model {
            fails: 0,
            budget_stops: 3,
        };
        let t = ok(super::model_verdict(
            &w("2026-11-28", 2),
            &slow,
            "2026-11-28",
            state,
        ));
        assert!(
            t.ends_with("warning: the last 3 model wakes ended at the time budget (--max-wake), not by themselves"),
            "{t}"
        );
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
