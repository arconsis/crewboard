//! The commands that plan the queue: `assign`, `ticket`, `reassign` and `unblock`.

use super::{Cli, Outcome, PlanCmd, Sel, TicketAction, done, fail, global, refused};
use crate::ops::{self, AgentCtx};

fn split_cmd(cli: &Cli, sel: &Sel, issue: &str, title: &str, body_file: &str) -> Outcome {
    done(
        AgentCtx::load(&global(cli, sel, false))
            .and_then(|mut ac| ops::tickets::ticket_split(&mut ac, issue, title, body_file)),
    )
}

/// `pause`, `resume`, `stop`: the agent's own command, or the owner's with `--agent`.
/// `unblock` has its own function so that the stack frame of `dispatch` stays small: a Windows
/// main thread has 1 MiB of stack, and a debug build adds the locals of every arm to one frame.
#[inline(never)]
fn unblock_cmd(
    cli: &Cli,
    sel: &Sel,
    issue: &str,
    body_file: Option<&str>,
    body: Option<&str>,
    anyway: bool,
) -> Outcome {
    // Secret scan and size limit first, like `say`.
    let text = match ops::say::prepare_body(body_file, body) {
        Ok(t) => t,
        Err(e) => return fail(e),
    };
    done(
        AgentCtx::load(&global(cli, sel, false))
            .and_then(|ac| ops::tickets::unblock(&ac, issue, Some(text), anyway)),
    )
}

/// The checks of `assign` that need no config and no network. `Some` is the outcome to return.
fn check_assign(
    to: Option<&str>,
    order: Option<&str>,
    why: Option<&str>,
    ov: ops::tickets::CapOverride<'_>,
) -> Option<Outcome> {
    // the reason is checked before any config or network
    if let Err(r) = crate::capabilities::check_override_args(ov.flag, ov.reason) {
        return Some(refused(r));
    }
    if let Some(r) = why
        && let Err(e) = ops::say::check_body_text(r)
    {
        return Some(fail(e));
    }
    if let Err(r) = ops::tickets::check_assign_args(to, order, ov) {
        return Some(refused(r));
    }
    if let Some(o) = order
        && let Err(r) = ops::tickets::parse_order(o)
    {
        return Some(refused(r));
    }
    None
}

/// `ticket new`, `ticket split` and `ticket adopt`.
#[inline(never)]
fn ticket_cmd(cli: &Cli, action: &TicketAction) -> Outcome {
    match action {
        TicketAction::New { template: true, .. } => {
            done(Ok(ops::tickets::BODY_TEMPLATE.to_string()))
        }
        TicketAction::New {
            title,
            body_file,
            repo,
            backlog: _,
            assign,
            continue_pr,
            gate_ci,
            sel,
            template: _,
        } => done(AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| {
            ops::tickets::ticket_new(
                &mut ac,
                ops::tickets::NewArgs {
                    title,
                    body_file,
                    repo: repo.as_deref(),
                    assign: assign.as_deref(),
                    adopt_pr: continue_pr.as_deref(),
                    gate_ci: *gate_ci,
                },
            )
        })),
        TicketAction::Split {
            issue,
            title,
            body_file,
            sel,
        } => split_cmd(cli, sel, issue, title, body_file),
        TicketAction::Adopt { issue, assign, sel } => done(
            AgentCtx::load(&global(cli, sel, false))
                .and_then(|mut ac| ops::tickets::ticket_adopt(&mut ac, issue, assign.as_deref())),
        ),
    }
}

#[inline(never)]
pub(super) fn dispatch(cli: &Cli, c: &PlanCmd) -> Outcome {
    match c {
        PlanCmd::Unblock {
            issue,
            body_file,
            body,
            override_unread,
            sel,
        } => unblock_cmd(
            cli,
            sel,
            issue,
            body_file.as_deref(),
            body.as_deref(),
            *override_unread,
        ),
        PlanCmd::Assign {
            issue,
            to,
            override_capabilities,
            why,
            order,
            sel,
        } => {
            let ov = ops::tickets::CapOverride {
                flag: *override_capabilities,
                reason: why.as_deref(),
            };
            if let Some(out) = check_assign(to.as_deref(), order.as_deref(), why.as_deref(), ov) {
                return out;
            }
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| {
                ops::tickets::assign(&mut ac, issue, to.as_deref(), ov, order.as_deref())
            }))
        }
        PlanCmd::Ticket { action } => ticket_cmd(cli, action),
        PlanCmd::Reassign {
            issue,
            reason,
            why,
            derived,
            to,
            override_capabilities,
            sel,
        } => {
            if let Some(r) = why
                && let Err(e) = ops::say::check_body_text(r)
            {
                return fail(e);
            }
            done(AgentCtx::load(&global(cli, sel, false)).and_then(|mut ac| {
                let how = if *derived {
                    ops::tickets::Why::Derived
                } else {
                    ops::tickets::Why::Stated {
                        reason: crate::rules::reassign::Reason::parse(
                            reason.as_deref().unwrap_or(""),
                        )?,
                        detail: why.as_deref(),
                    }
                };
                ops::tickets::reassign(&mut ac, issue, how, to.as_deref(), *override_capabilities)
            }))
        }
    }
}
