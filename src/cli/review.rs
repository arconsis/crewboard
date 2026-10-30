//! The commands that verify and end tickets, and report: `verify`, `merge`, `close`,
//! `report-status` and `tick`.

use super::{Cli, Outcome, ReviewCmd, done, fail, global, refused};
use crate::base::ignore::best_effort_output;
use crate::error::CbError;
use crate::ops::{self, AgentCtx};

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &ReviewCmd) -> Outcome {
    match c {
        ReviewCmd::Verify {
            issue,
            body,
            fail: gate_failed,
            runs,
            local_run,
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
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| {
                ops::tickets::verify(
                    &mut ac,
                    issue,
                    &note,
                    runs,
                    *gate_failed,
                    local_run.as_deref(),
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
                return fail(CbError::config(format!(
                    "'{health}' is not a status: use on-track, at-risk, off-track, inactive or complete"
                )));
            }
            done(
                AgentCtx::load(&global(cli, sel, false))
                    .and_then(|ac| ops::report_status::run(&ac, health, &t)),
            )
        }
        // The output first, then the reported deleted notes are forgotten (`TickOut::settle`).
        ReviewCmd::Tick { sel } => match AgentCtx::load(&global(cli, sel, false))
            .and_then(|mut ac| ops::tickets::tick(&mut ac).map(|t| (ac, t)))
        {
            Ok((ac, t)) => {
                print!("{}", t.text);
                best_effort_output(std::io::Write::flush(&mut std::io::stdout()));
                done(t.settle(&ac).map(|()| String::new()))
            }
            Err(e) => fail(e),
        },
    }
}
