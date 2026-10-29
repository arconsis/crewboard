//! Ticket commands (spec/tickets, spec/cli#commands): `assign`, `take`, `done`, `block`, `ask`,
//! `verify`, `close`, `reassign`.
//!
//! Every status change goes through `rules::status::check_status_transition`, with facts
//! that are derived here from the board (item fields read by id) and from the signed
//! comments of the ticket (`crate::ticket`): who the assignee is, whether an answer followed
//! a block, which `done` message is the latest, whether a verify passed for it.
//!
//! One file per command: `assign`, `take`, `done`, `block` (`block`, `unblock`, `ask`), `verify`,
//! `close`, `reassign`, `merge`, `new`, `order`, `split`, `tick`. This file holds what several of
//! them share; `pr_evidence`, `local_gate` and `in_progress` hold the shared reads and checks.
//!
//! Order of every command: pure checks (writer, status, arguments), then reads, then the
//! evidence, then the writes. A refusal therefore leaves the board as it was.

use super::say::{require_approved, send_signed};
use super::{AgentCtx, World, load_world};
use crate::clock::{Clock, SystemClock, format_ts};
use crate::error::{CbError, CbResult};
use crate::protocol::names::{AgentName, Kind, Re, To};
use crate::rules::fields::WriteCtx;
use crate::rules::lifecycle::State;
use crate::rules::status::{BlockReason, Status, TicketFacts, check_not_owner_route, check_verify};
use crate::rules::{Actor, Refusal, Role};
use crate::services::ticket_loader::Ticket;
use crate::state::TicketSnap;
use crate::ticket::{answer_after_block, latest_done, own_block_reason, question_open};

mod assign;
mod assign_checks;
mod block;
mod board_put;
mod close;
mod done;
mod earlier_work;
mod in_progress;
mod local_gate;
pub mod merge;
pub(crate) mod merged_pr;
pub(crate) mod merged_unverified;
mod new;
mod order;
pub(crate) mod owner_applied;
pub(crate) mod owner_merge;
pub(crate) mod owner_net;
mod pr_evidence;
pub(crate) mod pr_read;
mod reassign;
mod split;
mod take;
mod tick;
mod verify;
pub use assign::{assign, check_assign_args};
pub use assign_checks::CapOverride;
pub use block::{ask, block, unblock};
pub use close::{CloseHow, close};
pub use done::{DoneArgs, done};
pub(crate) use earlier_work::tagged as tag_earlier_work;
pub use local_gate::check_local_gate_text;
pub use merge::{merge_ticket, parse_method as merge_method};
pub use new::{BODY_TEMPLATE, NewArgs, ticket_adopt, ticket_new};
pub use order::parse_order;
pub(crate) use reassign::reassign_derived;
pub use reassign::{Why, reassign};
pub use split::ticket_split;
pub use take::take;
pub use tick::tick;
pub use verify::{check_verify_fail_note, verify};

pub(crate) struct Me {
    pub(crate) name: String,
    pub(crate) role: Role,
    pub(crate) actor: Actor,
    /// The `State` of this agent's own item.
    pub(crate) state: Option<State>,
}

pub(crate) fn me_of(ac: &AgentCtx, world: &World) -> CbResult<Me> {
    let name = ac.name.to_string();
    let t = require_approved(world, &name)?;
    Ok(Me {
        role: t.role,
        actor: Actor::Agent(t.role),
        state: world
            .items
            .get(&ac.agent.item_id)
            .and_then(|i| i.field("State"))
            .and_then(State::parse),
        name,
    })
}

pub(crate) fn load_world_cached(ac: &AgentCtx) -> CbResult<World> {
    let mut cache = ac.load_cache();
    let world = load_world(&ac.ctx, std::slice::from_ref(&ac.agent.item_id), &mut cache)?;
    ac.save_cache(&cache);
    Ok(world)
}

/// Remember a ticket and its new fields in the local cache, so that this agent's own next
/// `watch` does not report its own change as an event.
fn remember(ac: &AgentCtx, t: &Ticket, status: Status, agent: Option<&str>) {
    let mut cache = ac.load_cache();
    cache.index.tickets.insert(t.key(), t.item_id.clone());
    cache.index.ticket_snaps.insert(
        t.item_id.clone(),
        TicketSnap {
            status: Some(status.name().to_string()),
            agent: agent.map(str::to_string),
        },
    );
    ac.save_cache(&cache);
}

/// Write the Status of a ticket on the board. The table check is the caller's. The Status that
/// the command read is re-read right before the write ([`write_status`]).
fn set_status(ac: &AgentCtx, t: &Ticket, to: Status) -> CbResult<()> {
    write_status(ac, &t.item_id, &t.key(), t.status().ok(), to)
}

/// The one Status write behind every command that reads Status, decides and writes
/// (spec/board#concurrent-writers). `expected` is the Status the command read and judged. The item is
/// read again right before the write: when its Status is not `expected` any more, somebody else
/// moved the ticket meanwhile, and the write is refused with `status-changed-meanwhile` (nothing
/// is written). A Status that already is `to` is left as it is. The window that is left is the
/// round trip between this read and the write. `expected: None` (a new item, no Status read)
/// writes without the check.
pub(crate) fn write_status(
    ac: &AgentCtx,
    item_id: &str,
    key: &str,
    expected: Option<Status>,
    to: Status,
) -> CbResult<()> {
    if let Some(exp) = expected
        && check_status_unchanged(ac, item_id, key, exp, Some(to))? == StatusNow::AlreadyThere
    {
        return Ok(());
    }
    ac.ctx
        .board
        .set_select(&ac.ctx.client, item_id, "Status", to.name())?;
    Ok(())
}

/// The Status write of a command that has called [`check_status_unchanged`] before its signed
/// message: no second read.
pub(crate) fn set_select_status(ac: &AgentCtx, item_id: &str, to: Status) -> CbResult<()> {
    ac.ctx
        .board
        .set_select(&ac.ctx.client, item_id, "Status", to.name())?;
    Ok(())
}

/// What [`check_status_unchanged`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StatusNow {
    /// The Status is still the one that the command read.
    Unchanged,
    /// The Status already is the wanted one: the write is not needed.
    AlreadyThere,
}

/// Read the item again and compare its Status with `expected`, the one the command judged.
/// `status-changed-meanwhile` when it moved. A command that posts a signed message before the
/// Status write calls this before the post, so a moved ticket gets no message.
pub(crate) fn check_status_unchanged(
    ac: &AgentCtx,
    item_id: &str,
    key: &str,
    expected: Status,
    wanted: Option<Status>,
) -> CbResult<StatusNow> {
    let now = ac.ctx.board.read_item(&ac.ctx.client, item_id)?;
    let seen = now.field("Status").and_then(Status::parse_board);
    if seen == Some(expected) {
        return Ok(StatusNow::Unchanged);
    }
    if wanted.is_some() && seen == wanted {
        return Ok(StatusNow::AlreadyThere);
    }
    Err(CbError::refused(
        "status-changed-meanwhile",
        format!(
            "{key} was {} when this command read it and is {} now: somebody moved it meanwhile. Read it again (`crewboard status`) and decide again; nothing was written",
            expected.name(),
            now.field("Status").unwrap_or("without a Status")
        ),
    ))
}

fn post(
    ac: &AgentCtx,
    t: &Ticket,
    to: To,
    kind: Kind,
    re: Re,
    body: &str,
) -> CbResult<crate::ops::say::Posted> {
    post_with(ac, t, &SystemClock, to, kind, re, body)
}

/// [`post`] with the time of `clock` in the signed header.
fn post_with(
    ac: &AgentCtx,
    t: &Ticket,
    clock: &dyn Clock,
    to: To,
    kind: Kind,
    re: Re,
    body: &str,
) -> CbResult<crate::ops::say::Posted> {
    let ts = format_ts(clock.now());
    send_signed(ac, &t.repo, t.issue, to, kind, re, body, &ts)
}

/// spec/events#unread-messages-on-a-ticket: refuse while a message for this agent is unread on the
/// ticket. Reads nothing new (the ticket's comments are loaded anyway) and never moves the cursor.
fn unread_guard(
    ac: &AgentCtx,
    world: &World,
    t: &Ticket,
    anyway: bool,
) -> CbResult<Option<String>> {
    let mine = t.agent() == Some(ac.name.as_str());
    // the ticket was loaded from the board: it is a ticket
    crate::unread::guard(ac, world, &t.comments, &t.repo, t.issue, mine, true, anyway)
}

fn to_agent(name: &str) -> CbResult<To> {
    Ok(To::Agent(AgentName::parse(name)?))
}

/// Set the label `agent: <new>` and remove the other agent labels. The label is a
/// help for people; the board field is the truth. A failed write is a warning line.
fn agent_label_lines(
    ac: &AgentCtx,
    repo: &str,
    issue: u64,
    existing: &[String],
    new: Option<&str>,
) -> String {
    match crate::labels::sync_agent_label(&ac.ctx.client, repo, issue, existing, new) {
        Ok(s) => s,
        Err(e) => format!("warning: the agent label of {repo}#{issue} was not updated: {e}\n"),
    }
}

/// `needs-owner` on or off. A failed write is a warning line.
fn needs_owner_lines(ac: &AgentCtx, repo: &str, issue: u64, on: bool) -> String {
    match crate::labels::set_needs_owner(&ac.ctx.client, repo, issue, on) {
        Ok(()) => format!(
            "label 'needs-owner' {}\n",
            if on { "set" } else { "removed" }
        ),
        Err(e) => {
            format!("warning: the label needs-owner of {repo}#{issue} was not updated: {e}\n")
        }
    }
}

/// The time of the question to the owner that is open on the ticket, if one is.
fn question_open_since(t: &Ticket) -> Option<String> {
    question_open(&t.msgs, t.agent()).map(|m| crate::clock::format_ts(m.created))
}

/// The label `needs-owner` follows the log: it goes when the ticket carries it and no question
/// to the owner is open (`still_open`). A ticket without the label costs no request.
fn sync_needs_owner(ac: &AgentCtx, t: &Ticket, still_open: bool) -> String {
    let labelled = t
        .info
        .labels
        .iter()
        .any(|l| l == crate::labels::NEEDS_OWNER);
    if labelled && !still_open {
        needs_owner_lines(ac, &t.repo, t.issue, false)
    } else {
        String::new()
    }
}

/// The facts for a transition, derived from the ticket and the actor.
fn facts(t: &Ticket, me: &Me, status: Status) -> TicketFacts {
    TicketFacts {
        // the signed log decides the assignee; a board `Agent` that it does not support counts
        // for nothing (the commands refuse that case first: `Ticket::check_agent_signed`)
        actor_is_assignee: t.agent() == Some(me.name.as_str()) && t.check_agent_signed().is_ok(),
        answer_after_block: answer_after_block(&t.msgs, status, t.agent()),
        own_block: if status == Status::Blocked {
            own_block_reason(&t.msgs, me.name.as_str())
        } else {
            BlockReason::Plain
        },
        split_off_proven: status == Status::Blocked
            && t.agent() == Some(me.name.as_str())
            && crate::ticket::split_off_proven(&t.info.body, &t.msgs, &me.name, &t.key()),
    }
}

/// The checks of `verify` and `merge` before any read about the PR: a headless coordinator
/// never acts on a ticket with an open question to the owner. A coordinator's ticket in Review
/// needs a `done` that is not on the owner route. Then the writer, the status and the open
/// question ([`check_verify`]). `gate_failed` (`verify --fail`) leaves an open question alone.
fn check_verify_ticket(
    ac: &AgentCtx,
    t: &Ticket,
    from: Status,
    me: &Me,
    gate_failed: bool,
) -> CbResult<()> {
    let open_since = question_open_since(t);
    if ac.ctx.cd.is_headless()? {
        crate::headless::check_no_open_question_headless(&t.key(), open_since.is_some())?;
    }
    if from == Status::Review && me.actor.is_coordinator() {
        check_owner_route(t)?;
    }
    check_verify(
        from,
        me.actor,
        &t.key(),
        open_since.as_deref().filter(|_| !gate_failed),
    )?;
    Ok(())
}

/// Is the latest `done` of the ticket on the owner route?
fn check_owner_route(t: &Ticket) -> Result<(), Refusal> {
    let on_owner_route = t
        .agent()
        .and_then(|a| latest_done(&t.msgs, a))
        .is_some_and(|(m, _)| crate::ticket::owner_route_patch(&m.body).is_some());
    check_not_owner_route(&t.key(), on_owner_route)
}

fn work_write_ctx(me: &Me, assigned: bool) -> WriteCtx {
    work_write_ctx_of(me.actor, assigned)
}

fn work_write_ctx_of(actor: Actor, assigned: bool) -> WriteCtx {
    WriteCtx {
        actor,
        is_subject: false,
        is_assigned: assigned,
        owner_approval: false,
        approved: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemSnapshot;
    use crate::services::ticket_loader::parse_issue;
    use serde_json::{Value, json};

    fn issue_json() -> Value {
        json!({"node_id": "I_x", "number": 12, "title": "t", "body": null, "state": "open",
               "labels": [{"name": "crew:work"}, {"name": "other"}]})
    }

    #[test]
    fn facts_come_from_the_ticket_and_not_from_flags() {
        // answer_after_block is false without a blocking message, whatever the status says
        let t = Ticket {
            repo: "a/b".into(),
            issue: 1,
            item_id: "PVTI_1".into(),
            item: ItemSnapshot {
                id: "PVTI_1".into(),
                updated_at: "x".into(),
                item_type: "ISSUE".into(),
                project_number: Some(15),
                issue_state: None,
                fields: [
                    ("Status".to_string(), "Blocked".to_string()),
                    ("Agent".to_string(), "hermes@b".to_string()),
                ]
                .into_iter()
                .collect(),
            },
            info: parse_issue(&issue_json()).unwrap(),
            comments: vec![],
            msgs: vec![crate::ticket::VMsg {
                ts: String::new(),
                comment_id: 1,
                created: 1,
                sender: crate::ticket::Sender::Agent {
                    name: "boss@a".into(),
                    role: Role::Coordinator,
                },
                to: "hermes@b".into(),
                kind: Some(Kind::Request),
                re: Re::None,
                body: crate::protocol::acts::assignment_body(
                    "t", "a/b#1", "PVTI_1", None, None, "",
                ),
            }],
        };
        let me = Me {
            name: "hermes@b".into(),
            role: Role::Dev,
            actor: Actor::Agent(Role::Dev),
            state: Some(State::Active),
        };
        let f = facts(&t, &me, Status::Blocked);
        assert!(f.actor_is_assignee && !f.answer_after_block);
        let other = Me {
            name: "codex@c".into(),
            ..me
        };
        assert!(!facts(&t, &other, Status::Blocked).actor_is_assignee);
        assert_eq!(t.status().unwrap(), Status::Blocked);
        // the board Agent without a signed assignment is nobody's right
        let unsigned = Ticket { msgs: vec![], ..t };
        assert!(!facts(&unsigned, &me_again(), Status::Blocked).actor_is_assignee);
        assert_eq!(
            unsigned.check_agent_signed().unwrap_err().rule,
            "ticket-agent-not-signed"
        );
    }

    fn me_again() -> Me {
        Me {
            name: "hermes@b".into(),
            role: Role::Dev,
            actor: Actor::Agent(Role::Dev),
            state: Some(State::Active),
        }
    }
}
