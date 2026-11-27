//! The commands of a dev agent on its tickets: `say`, `take`, `done`, `block` and `ask`.

use super::{Cli, DevCmd, Outcome, done, fail, global};
use crate::ops::{self, AgentCtx};

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &DevCmd) -> Outcome {
    match c {
        DevCmd::Say {
            issue,
            to,
            kind,
            re,
            body_file,
            body,
            override_unread,
            sel,
        } => {
            // Secret scan and size limit first: they need no config and no network.
            let text = match ops::say::prepare_body(body_file.as_deref(), body.as_deref()) {
                Ok(t) => t,
                Err(e) => return fail(e),
            };
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| {
                ops::say::run(
                    &ac,
                    ops::say::SayArgs {
                        issue,
                        to: to.clone(),
                        kind: *kind,
                        re: re.as_deref(),
                        body: text,
                        anyway: *override_unread,
                    },
                )
            }))
        }
        DevCmd::Take {
            issue,
            interrupt,
            why,
            park,
            continue_pr,
            sel,
        } => done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| {
            ops::tickets::take(
                &ac,
                issue,
                interrupt.then(|| why.as_deref().unwrap_or("")),
                park.as_deref(),
                continue_pr.as_deref(),
            )
        })),
        DevCmd::Done {
            issue,
            pr,
            sha,
            override_unread,
            runs,
            local_run,
            run_check,
            must_fail_at,
            work_dir,
            sel,
        } => {
            // spec/ci#local-run-ci_mode-local: a blank or bad text is refused before any read.
            if let Some(t) = local_run
                && let Err(e) = ops::tickets::check_local_gate_text(t)
            {
                return fail(e);
            }
            // `--run-check`: the tool runs the checks inside `done`, in a new work tree
            let dir = work_dir
                .clone()
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let log_root = if run_check.is_empty() {
                None
            } else {
                match super::check_log_root() {
                    Ok(r) => Some(r),
                    Err(e) => return fail(e),
                }
            };
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|ac| {
                let run = log_root.as_deref().map(|log_root| ops::tickets::RunArgs {
                    names: run_check,
                    repo_dir: &dir,
                    log_root,
                    unattended: false,
                    must_fail_at: must_fail_at.as_deref(),
                });
                ops::tickets::done(
                    &ac,
                    ops::tickets::DoneArgs {
                        issue,
                        pr,
                        sha: sha.as_deref(),
                        // one flag passes both checks of `done`
                        anyway: *override_unread,
                        ignore_events: *override_unread,
                        runs,
                        local_gate: local_run.as_deref(),
                        run: run.as_ref(),
                    },
                )
            }))
        }
        DevCmd::Block {
            issue,
            why,
            override_unread,
            sel,
        } => done(
            AgentCtx::load(&global(cli, sel, false))
                .and_then(|ac| ops::tickets::block(&ac, issue, why, *override_unread)),
        ),
        DevCmd::Ask {
            issue,
            body_file,
            body,
            override_unread,
            sel,
        } => {
            let text = match ops::say::prepare_body(body_file.as_deref(), body.as_deref()) {
                Ok(t) => t,
                Err(e) => return fail(e),
            };
            done(
                AgentCtx::load(&global(cli, sel, false))
                    .and_then(|ac| ops::tickets::ask(&ac, issue, &text, *override_unread)),
            )
        }
    }
}
