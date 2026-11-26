//! The commands that verify and end tickets, and report: `verify`, `merge`, `close`,
//! `report-status`, `retro` and `tick`.

use super::{Cli, Outcome, ReviewCmd, done, fail, global, refused};
use crate::base::ignore::best_effort_output;
use crate::error::CbError;
use crate::exit::Exit;
use crate::ops::{self, AgentCtx};

/// Where the logs of `verify --run-check` are kept: `check-runs` in the state dir of crewboard.
/// A host without a home has none: a directory in the shared temp dir is no place for a
/// checkout that code runs in.
fn check_log_root() -> Result<std::path::PathBuf, CbError> {
    let env = |k: &str| std::env::var(k).ok();
    crate::wake::state::state_root(env("XDG_STATE_HOME").as_deref(), env("HOME").as_deref())
        .map(|r| r.join("check-runs"))
        .ok_or_else(|| {
            CbError::config(
                "--run-check keeps its logs in the state dir of crewboard, and HOME is not set",
            )
        })
}

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &ReviewCmd) -> Outcome {
    match c {
        ReviewCmd::Verify {
            issue,
            body,
            fail: gate_failed,
            runs,
            local_run,
            run_check,
            work_dir,
            sel,
        } => {
            let note = body.clone().unwrap_or_default();
            if let Some(t) = local_run
                && let Err(e) = ops::tickets::check_local_gate_text(t)
            {
                return fail(e);
            }
            // A failed Gate check is recorded with what the coordinator found.
            if let Err(r) = ops::tickets::check_verify_fail_note(*gate_failed, &note) {
                return fail(r.into());
            }
            if let Err(e) = ops::say::check_body_text(&note) {
                return fail(e);
            }
            if let Err(r) = ops::tickets::check_note_has_no_gate_line(&note) {
                return fail(r.into());
            }
            // `--run-check`: the tool runs the checks inside `verify`, in a new work tree
            let dir = work_dir
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let log_root = if run_check.is_empty() {
                None
            } else {
                match check_log_root() {
                    Ok(r) => Some(r),
                    Err(e) => return fail(e),
                }
            };
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| {
                let run = log_root.as_deref().map(|log_root| ops::tickets::RunArgs {
                    names: run_check,
                    repo_dir: &dir,
                    log_root,
                    unattended: false,
                });
                ops::tickets::verify(
                    &mut ac,
                    issue,
                    &note,
                    runs,
                    *gate_failed,
                    local_run.as_deref(),
                    run.as_ref(),
                )
            }))
        }
        ReviewCmd::Merge {
            issue,
            method,
            runs,
            sel,
        } => {
            if let Err(r) = ops::tickets::merge_method(method) {
                return refused(r);
            }
            done(
                AgentCtx::load(&global(cli, sel, false))
                    .and_then(|ac| ops::tickets::merge_ticket(&ac, issue, method, runs)),
            )
        }
        ReviewCmd::Close {
            issue,
            verified,
            why,
            sel,
        } => done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| {
            ops::tickets::close(
                &ac,
                issue,
                ops::tickets::CloseHow {
                    verified: *verified,
                    reason: why.as_deref(),
                },
            )
        })),
        ReviewCmd::ReportStatus {
            health,
            body,
            body_file,
            sel,
        } => {
            let t = match ops::say::prepare_body(body_file.as_deref(), body.as_deref()) {
                Ok(t) => t,
                Err(e) => return fail(e),
            };
            if ops::report_status::api_status(health).is_none() {
                return fail(CbError::usage(format!(
                    "'{health}' is not a status: use on-track, at-risk, off-track, inactive or complete"
                )));
            }
            done(
                AgentCtx::load(&global(cli, sel, false))
                    .and_then(|ac| ops::report_status::run(&ac, health, &t)),
            )
        }
        ReviewCmd::Retro { json, since, sel } => {
            match ops::retro::run(&global(cli, sel, false), *json, since.as_deref()) {
                Ok(out) => {
                    print!("{}", out.text);
                    Outcome {
                        // Exit 1 = no ticket in the period, as `mine` exit 1 = nothing to do
                        exit: if out.empty { Exit::NoEvents } else { Exit::Ok },
                        code: None,
                        message: None,
                    }
                }
                Err(e) => fail(e),
            }
        }
        // The output first, then the reported deleted notes are forgotten (`TickOut::settle`).
        ReviewCmd::Tick { work_dir, sel } => match AgentCtx::load(&global(cli, sel, false))
            .and_then(|mut ac| {
                // a checkout and a home for the logs: both, or the checks do not run
                let checks = match work_dir {
                    Some(d) => Some(ops::tickets::TickChecks {
                        repo_dir: d.clone(),
                        log_root: check_log_root()?,
                    }),
                    None => None,
                };
                ops::tickets::tick(&mut ac, checks.as_ref()).map(|t| (ac, t))
            }) {
            Ok((ac, t)) => {
                print!("{}", t.text);
                best_effort_output(std::io::Write::flush(&mut std::io::stdout()));
                done(t.settle(&ac).map(|()| String::new()))
            }
            Err(e) => fail(e),
        },
    }
}
