//! `crewboard agent add`: set up one agent in one command, and make a rerun safe.
//!
//! Seven steps: the agent dir, `join`, the `gh` shim, the crew id, the work dir, the git identity
//! and `start`. Each step says `done` or `already done`. A failure names the step, shows the last
//! lines of its output, says what to do and names the log. The log is `<agent dir>/bootstrap.log`;
//! no token goes into it. The end is the one next step: for a start, the command to paste on its own line.
//!
//! The steps that talk to GitHub run in this process. With the hidden option `--crewboard BIN`
//! (a test seam that only a build with debug assertions reads) `join`, `start` and the crew id
//! lookup go through another binary instead. `start` always runs as a child of a crewboard binary: it
//! checks the working directory and the PATH of the agent, so it needs its own process.

mod capture;
mod checkout;
mod next;
mod output;
mod plan;
mod setup;
mod start;

pub(super) use next::scheduler_for;
pub(super) use output::{Fail, PREFIX, env_fix, fail_lines};
pub use plan::AddArgs;
pub(super) use plan::{default_env_file, strip_prefix};
pub(super) use setup::{is_linked, make_agent_dir};

use super::install_wake;
use super::{AddRole, absolute};
use crate::agent_env;
use crate::exit::Exit;
use crate::github::api::users::noreply_email;
use checkout::{step_git, step_work_dir};
use next::{after_start_lines, after_wake_line, approve_line};
use output::{Run, TOTAL, redact, say};
use plan::{Early, Plan, plan, seam_bin};
use setup::{step_crew_id, step_dir, step_join, step_shim};
use start::{Start, step_start, wait_for_approval};
use std::path::{Path, PathBuf};

/// The token values of the env file of the flags, for the texts before step 1. None when the file
/// does not load.
fn early_secrets(a: &AddArgs) -> Vec<String> {
    a.env_file
        .as_deref()
        .map(absolute)
        .and_then(|f| agent_env::load(f.parent().unwrap_or(Path::new(".")), Some(&f)).ok())
        .map(|e| e.token_values())
        .unwrap_or_default()
}

/// Run `agent add` for a dev or a coordinator (`agent::run_add` sends the headless coordinator to
/// `coordinator`). It prints its own lines. The result is the exit code. With `--token-stdin` the
/// token is stored first and the steps then use that env file.
pub(super) fn run_add_steps(a: &AddArgs) -> u8 {
    if a.token_stdin {
        return match store_token(a) {
            Ok(env) => run_add_plan(&AddArgs {
                env_file: Some(env),
                token_stdin: false,
                ..a.clone()
            }),
            Err(code) => code,
        };
    }
    run_add_plan(a)
}

/// The refusal of `--install-wake` for the headless coordinator. The exit code.
pub(super) fn refuse_install_wake_headless() -> u8 {
    eprintln!(
        "{PREFIX} --install-wake does not go with --role headless-coordinator (install its tick with: crewboard wake install --tick --agent-dir <dir>)"
    );
    4
}

/// The refusal of `--token-stdin` for the headless coordinator, which uses the env file of the
/// interactive one. The exit code.
pub(super) fn refuse_token_stdin_headless() -> u8 {
    eprintln!(
        "{PREFIX} --token-stdin does not go with --role headless-coordinator (it uses the env file of the interactive coordinator)"
    );
    4
}

/// `--token-stdin`: read the token, check it and write the env file that the run then uses (the
/// global `--env-file`, else the default file of the initiative). The exit code of a failure.
fn store_token(a: &AddArgs) -> Result<PathBuf, u8> {
    let fail = |m: String| {
        eprintln!("{PREFIX} {m}");
        4
    };
    let env = match (&a.env_file, &a.initiative) {
        (Some(f), _) => absolute(f),
        (None, Some(i)) => {
            crate::ops::crew_token::default_env_file(i).map_err(|e| fail(strip_prefix(&e)))?
        }
        (None, None) => {
            return Err(fail(
                "--token-stdin needs --initiative <name> (for the env file ~/.config/crewboard/<name>.env) or --env-file <file>".into(),
            ));
        }
    };
    let report = crate::ops::crew_token::read_token().and_then(|t| {
        crate::ops::crew_token::store(&t, &env, a.crew_login.as_deref(), a.insecure_one_account)
    });
    match report {
        Ok(text) => {
            for l in text.lines() {
                println!("{PREFIX} {}", l.trim_start_matches("crew-token: "));
            }
            Ok(env)
        }
        Err(e) => {
            eprintln!("{PREFIX} {}", strip_prefix(&e));
            Err(e.exit().code())
        }
    }
}

fn run_add_plan(a: &AddArgs) -> u8 {
    let role = a.role.unwrap_or(AddRole::Dev);
    match plan(a, role == AddRole::Coordinator) {
        Ok(p) => run_steps(&p),
        Err(Early::Msg(m)) => {
            eprintln!("{PREFIX} {}", redact(&m, &early_secrets(a)));
            4
        }
        Err(Early::Usage(m)) => {
            eprintln!("{PREFIX} {}", redact(&m, &early_secrets(a)));
            Exit::Usage.code()
        }
        Err(Early::Env(m)) => {
            // the message of the loader holds no value, and is cleaned all the same
            let m = redact(&m, &early_secrets(a));
            let env = a.env_file.as_deref().map(absolute).unwrap_or_default();
            let f = Fail {
                step: 1,
                name: "agent dir",
                what: m.clone(),
                fix: env_fix(&m, &env),
                code: 4,
            };
            for l in fail_lines(&f, TOTAL, "", None) {
                eprintln!("{l}");
            }
            f.code
        }
    }
}

fn run_steps(p: &Plan) -> u8 {
    let mut run = Run::new();

    match step_dir(p, &mut run) {
        Ok(r) => say(1, "agent dir", &r),
        Err(f) => return run.fail(f),
    }
    if seam_bin(p).is_none() {
        // from here the token of the agent account is the token of this process
        if let Err(e) = agent_env::init(&p.agent_dir, None) {
            eprintln!(
                "{PREFIX} cannot load the env file of the agent: {}",
                redact(&strip_prefix(e), &run.secrets)
            );
            return e.exit().code();
        }
    }

    let (conf, joined_now) = match step_join(p, &mut run) {
        Ok((r, conf, now)) => {
            say(2, "join", &r);
            (conf, now)
        }
        Err(f) => return run.fail(f),
    };
    let initiative = conf
        .as_deref()
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.initiative.clone());

    match step_shim(p, &mut run) {
        Ok(r) => say(3, "gh shim", &r),
        Err(f) => return run.fail(f),
    }
    let shim = p.agent_dir.join("bin").join("gh");

    let (login, id) = match step_crew_id(p, &mut run, &shim) {
        Ok((r, l, i)) => {
            say(4, "crew id", &r);
            (l, i)
        }
        Err(f) => return run.fail(f),
    };
    let email = noreply_email(&login, &id);

    match step_work_dir(p, &mut run, &shim, &login) {
        Ok(r) => say(5, "work dir", &r),
        Err(f) => return run.fail(f),
    }
    match step_git(p, &mut run, &shim, &login, &email) {
        Ok(r) => say(6, "git config", &r),
        Err(f) => return run.fail(f),
    }

    let not_yet = || {
        println!(
            "{PREFIX} [7/{TOTAL}] start: not yet: the owner has not approved {}",
            p.name
        );
        println!("{PREFIX} {}", approve_line(p, &initiative));
        0
    };
    let done = || {
        println!("{PREFIX} [7/{TOTAL}] start: done ({} is active)", p.name);
        finish(p, &initiative)
    };
    if joined_now {
        return match &p.wait {
            Some(w) => match wait_for_approval(p, &mut run, &initiative, w) {
                Start::Done => done(),
                Start::NotApproved => not_yet(),
                Start::Failed(f) => run.fail(f),
            },
            None => not_yet(),
        };
    }
    match step_start(p, &mut run) {
        Start::Done => done(),
        Start::NotApproved => match &p.wait {
            Some(w) => match wait_for_approval(p, &mut run, &initiative, w) {
                Start::Done => done(),
                Start::NotApproved => not_yet(),
                Start::Failed(f) => run.fail(f),
            },
            None => not_yet(),
        },
        Start::Failed(f) => run.fail(f),
    }
}

/// The end of a good run: with `--install-wake` the scheduler unit is installed and its lines
/// come first, then the one next step; else the command to paste on its own line. The exit code.
fn finish(p: &Plan, initiative: &str) -> u8 {
    let Some((spec, installer)) = &p.wake else {
        let [intro, command, hint] = after_start_lines(p, initiative);
        println!("{PREFIX} {intro}");
        println!("{command}");
        println!("{PREFIX} {hint}");
        return 0;
    };
    let target = install_wake::Target {
        agent_dir: &p.agent_dir,
        initiative,
        name: &p.name,
        runtime: p.runtime.bin(),
        work_dir: &p.work_dir,
        env_file: &p.env_file,
    };
    match install_wake::install(*installer, spec, &target) {
        Ok(done) => {
            for l in &done.status {
                println!("{PREFIX} wake: {l}");
            }
            if let Some(n) = &done.next {
                println!("{PREFIX} wake: next: {n}");
            }
            println!("{PREFIX} {}", after_wake_line(p, done.next.as_deref()));
            0
        }
        Err(e) => {
            // the agent is set up and started; only the unit is missing, and a rerun does just that
            for l in strip_prefix(&e).lines() {
                eprintln!("{PREFIX} wake install: {l}");
            }
            eprintln!(
                "{PREFIX} the agent is set up and started ({} is active); only the scheduler unit is missing. Fix the cause and run this command again: every step says already done",
                p.name
            );
            e.exit().code()
        }
    }
}
