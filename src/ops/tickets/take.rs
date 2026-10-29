//! `take`: a dev agent starts work on a ticket assigned to it.

use super::earlier_work::{Adopted, check_adopt, hint};
use super::in_progress::{choose_park, in_progress_key};
use super::{
    Me, check_status_unchanged, facts, load_world_cached, me_of, post, remember, set_select_status,
    sync_needs_owner, unread_guard,
};
use crate::error::{CbError, CbResult};
use crate::ops::{AgentCtx, World, parse_issue_arg};
use crate::protocol::acts::park_body;
use crate::protocol::names::{Kind, Re, To};
use crate::rules::ids;
use crate::rules::lifecycle::{State, check_take};
use crate::rules::status::{BlockReason, Status, Transition, check_status_transition};
use crate::rules::text::shell_quote;
use crate::rules::{Refusal, Role};
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::question_open;

/// The State check of `take` for a new ticket: only an `active` dev agent takes work. A
/// paused agent gets `agent-paused`, every other State gets `agent-not-active`. Whether the
/// agent is busy is judged by the board (`in_progress_key`, `choose_park`), not by the State.
pub(crate) fn check_take_state(
    state: Option<State>,
    role: Role,
    agent: &str,
) -> Result<(), Refusal> {
    check_take(Some(role))?;
    match state {
        Some(State::Active) => Ok(()),
        Some(State::Paused) => crate::rules::lifecycle::check_not_paused(state),
        other => {
            let name = other.map_or("without a State", State::name);
            let fix = match other {
                Some(State::Approved) => "run `crewboard start`",
                Some(State::Requested) | None => {
                    "the owner approves it with `/crew approve`, then run `crewboard start`"
                }
                _ => "it is leaving: run `crewboard stop` to finish, then `crewboard join` again",
            };
            Err(Refusal::new(
                "agent-not-active",
                format!("{agent} is {name}, not active: {fix}"),
            ))
        }
    }
}

/// What `take` has judged before it writes anything.
struct TakePlan {
    t: Ticket,
    /// The ticket was Blocked (after an answer, or parked by this agent): a resume.
    resume: bool,
    /// The agent parked this ticket itself.
    parked_back: bool,
    /// `--interrupt`: the ticket In Progress that is parked first, and the body of the park.
    park: Option<(Ticket, String)>,
    /// `--continue-pr`: the open PR of an earlier agent that this agent continues on.
    adopt: Option<Adopted>,
}

/// Judges a `take`: the status move, the single In Progress ticket per agent, the State, and
/// the ticket to park. Reads only.
fn plan_take(
    ac: &AgentCtx,
    world: &World,
    me: &Me,
    (repo, n): (String, u64),
    interrupt: Option<&str>,
    park: Option<&str>,
    adopt_pr: Option<&str>,
) -> CbResult<TakePlan> {
    // A paused agent finishes its current step and takes nothing new (spec/board#lifecycle).
    // Resuming a blocked ticket after an answer is the current step, so only a new ticket is
    // refused.
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    t.check_agent_signed()?;
    if t.status()? == Status::Ready {
        crate::rules::lifecycle::check_not_paused(me.state)?;
    }
    let from = t.status()?;
    let ctx = facts(&t, me, from);
    // `--interrupt` takes a Ready ticket, or an answered Blocked ticket of this agent: the
    // status check above has judged the answer, so only the assignee is left to check.
    let ok =
        from == Status::Ready || (from == Status::Blocked && t.agent() == Some(me.name.as_str()));
    if interrupt.is_some() && !ok {
        return Err(CbError::refused(
            "interrupt-needs-ready-ticket",
            format!(
                "--interrupt needs a Ready ticket or an answered Blocked ticket of yours, {} is {}",
                t.key(),
                from.name()
            ),
        ));
    }
    let tr = match from {
        Status::Blocked => ctx.resume(),
        // a failed verify is the only way from Review to In Progress, and the take has none
        Status::Review => Transition::AfterVerify {
            verify: None,
            assignee_busy: false,
        },
        _ => ctx.plain(),
    };
    if let Err(r) = check_status_transition(Some(from), Status::InProgress, me.actor, &tr) {
        return Err(explain_unblock_refusal(&t, from, r));
    }
    let resume = from != Status::Ready;
    let adopt = match adopt_pr {
        None => None,
        Some(_) if resume => {
            return Err(CbError::refused(
                "adopt-pr-needs-ready-ticket",
                format!(
                    "--continue-pr goes with the first take of a Ready ticket, {} is {}",
                    t.key(),
                    from.name()
                ),
            ));
        }
        Some(url) => Some(check_adopt(ac, &t, url)?),
    };
    let parked_back =
        from == Status::Blocked && matches!(ctx.own_block, BlockReason::Parked { .. });
    // One ticket In Progress per agent, judged by the board and not by the State. The
    // State can be stale (`active` after pause + resume or a repair), so the agent's own rows
    // are read for every take (one read more for an agent that is not `working`).
    let rows = crate::ops::lifecycle::my_rows(ac, world)?;
    if interrupt.is_none()
        && let Some(busy) = in_progress_key(&rows, &me.name, &t.item_id)
    {
        return Err(CbError::refused(
            "take-agent-busy",
            format!(
                "{} already has {busy} In Progress; free the agent first with `block`, `ask` or `done`, then take {}; \
                     for a ticket that is ahead of it in your queue use `crewboard take <n> --interrupt --why \"<why>\"`",
                me.name,
                t.key()
            ),
        ));
    }
    // A new ticket needs an `active` agent. A resume of a Blocked ticket is the current step,
    // so it passes for every State (a paused agent finishes it).
    if !resume {
        check_take_state(me.state, me.role, &me.name)?;
    }
    let park = match interrupt {
        None => None,
        Some(reason) => Some(plan_park(ac, world, me, &t, &rows, park, reason)?),
    };
    Ok(TakePlan {
        t,
        resume,
        parked_back,
        park,
        adopt,
    })
}

/// A refusal of the Blocked -> In Progress move, with the case named: a question to the owner
/// blocks the ticket (a plain coordinator answer does not unblock), and a coordinator `info`
/// after the block does not unblock either.
fn explain_unblock_refusal(t: &Ticket, from: Status, r: Refusal) -> CbError {
    if r.rule == ids::STATUS_ANSWER_MISSING && from == Status::Blocked {
        if crate::ticket::blocked_by_owner_question(&t.msgs, t.agent()) {
            return CbError::refused(
                ids::STATUS_ANSWER_MISSING,
                format!(
                    "{} is Blocked by a question to the owner; it needs an owner comment or `crewboard unblock {}` by the coordinator (an answer that withdraws the question). Do not ask again: wait for the answer, then take.",
                    t.key(),
                    t.issue
                ),
            );
        }
        if crate::ticket::info_after_block(&t.msgs) {
            return CbError::refused(
                ids::STATUS_ANSWER_MISSING,
                format!(
                    "{} is Blocked: a coordinator info was posted after the block, but only an answer (or an owner message) unblocks; ask the coordinator for `crewboard unblock {}`",
                    t.key(),
                    t.issue
                ),
            );
        }
    }
    CbError::Refused(r)
}

/// The rule of `--interrupt`: the new ticket must come first, that is have a strictly lower
/// `Order` than the parked ticket.
fn check_interrupt_rule(t: &Ticket, cur: &crate::ticket::TicketRow) -> Result<(), Refusal> {
    let new = t.item.field("Order").and_then(|o| o.parse::<i64>().ok());
    crate::rules::status::check_interrupt_by_order(new, cur.order)
}

/// `--interrupt`: chooses and checks the ticket to park BEFORE any write. Returns that ticket
/// and the body of the park message.
fn plan_park(
    ac: &AgentCtx,
    world: &World,
    me: &Me,
    t: &Ticket,
    rows: &[crate::ticket::TicketRow],
    named: Option<&str>,
    reason: &str,
) -> CbResult<(Ticket, String)> {
    let named = named
        .map(|p| parse_issue_arg(p, ac.ctx.main_repo()))
        .transpose()?;
    let cur = choose_park(rows, named.as_ref().map(|(r, n)| (r.as_str(), *n)))?;
    check_interrupt_rule(t, cur)?;
    let ct = load_ticket(&ac.ctx.reader(), &world.trust, &cur.repo, cur.issue)?;
    ct.check_agent_signed()?;
    // The park posts a signed message on the old ticket, so the same guard as `block` runs
    // here, before any write. The new ticket needs no guard: `take` prints its unread
    // messages and marks them read below.
    let unread_line = unread_guard(ac, world, &ct, false)?;
    debug_assert!(unread_line.is_none());
    let park = facts(&ct, me, Status::InProgress).plain();
    check_status_transition(Some(Status::InProgress), Status::Blocked, me.actor, &park)?;
    Ok((ct, park_body(t.issue, reason)))
}

/// Parks the ticket of `--interrupt`: the signed message, the Status. Returns the output line.
fn park_current(ac: &AgentCtx, ct: &Ticket, body: &str) -> CbResult<String> {
    if let Ok(from) = ct.status() {
        check_status_unchanged(ac, &ct.item_id, &ct.key(), from, None)?;
    }
    let posted = post(ac, ct, To::Coordinator, Kind::Blocked, Re::None, body)?;
    set_select_status(ac, &ct.item_id, Status::Blocked)?;
    remember(ac, ct, Status::Blocked, ct.agent());
    Ok(format!(
        "{} parked ({body}); message {} ({})\n",
        ct.key(),
        posted.id,
        posted.url
    ))
}

/// The exact next steps after a take (branch, PR command, done), so the agent needs no rule
/// text for them. One read: AUTHORITY.md for the branch prefix.
fn next_steps_after_take(ac: &AgentCtx, me: &Me, t: &Ticket) -> String {
    let prefix = match crate::ctx::authority_text(&ac.ctx) {
        Ok(None) => Ok(crate::authority::Authority::default().branch_prefix()),
        Ok(Some(text)) => match crate::authority::parse(&text) {
            Ok(a) => Ok(a.unwrap_or_default().branch_prefix()),
            Err(e) => Err(e.to_string()),
        },
        Err(e) => Err(e.to_string()),
    };
    take_next_steps(
        prefix.as_deref().map_err(String::as_str),
        me.name.as_str(),
        &t.repo,
        t.issue,
        &t.info.title,
        crate::owner_route::declared(&t.info.body),
    )
}

/// `take <issue>` (dev): Ready -> In Progress with an ack; also the resume of a Blocked
/// ticket after an answer, or of a ticket the agent parked itself. With
/// `interrupt = Some(reason)` (`--interrupt --why`): first park the ticket that is In
/// Progress (spec/tickets#status-flow-and-writers), then take this one. With
/// `adopt_pr = Some(url)` (`--continue-pr`): continue on the open PR that an earlier agent left
/// (spec/tickets#work-that-an-earlier-agent-left).
pub fn take(
    ac: &AgentCtx,
    issue_arg: &str,
    interrupt: Option<&str>,
    park: Option<&str>,
    adopt_pr: Option<&str>,
) -> CbResult<String> {
    let issue = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    if interrupt.is_some_and(|r| r.trim().is_empty()) {
        return Err(CbError::refused(
            "interrupt-needs-reason",
            "--interrupt needs a reason (--why)",
        ));
    }
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    crate::ops::say::refresh_heartbeat(ac, &world);
    // The coordinator never takes (spec/board#coordinator-role), whatever the ticket looks like.
    check_take(Some(me.role))?;
    let plan = plan_take(ac, &world, &me, issue, interrupt, park, adopt_pr)?;
    act_take(ac, &world, &me, plan)
}

/// The writes of a `take`: the park of `--interrupt`, the ack, the Status, the Doing field.
fn act_take(ac: &AgentCtx, world: &World, me: &Me, plan: TakePlan) -> CbResult<String> {
    let TakePlan {
        t,
        resume,
        parked_back,
        park,
        adopt,
    } = plan;
    // spec/events#unread-messages-on-a-ticket: the messages before the ack are printed here and
    // count as read. The watch cursor does not move, so `watch` still delivers them once.
    let read_lines = crate::unread::render_read(
        &crate::unread::unread_events(
            ac,
            world,
            &t.comments,
            &t.repo,
            t.issue,
            t.agent() == Some(ac.name.as_str()),
            true,
        )
        .unwrap_or_default(),
    );
    let park_lines = match &park {
        None => String::new(),
        Some((ct, body)) => park_current(ac, ct, body)?,
    };
    // A ticket that the coordinator moved since the plan gets no ack.
    if let Ok(from) = t.status() {
        check_status_unchanged(ac, &t.item_id, &t.key(), from, None)?;
    }
    let ack = post(
        ac,
        &t,
        To::Coordinator,
        Kind::Ack,
        Re::None,
        &if parked_back {
            format!("Resumed my parked ticket.\nitem={}\n", t.item_id)
        } else if resume {
            format!("Resumed after an answer.\nitem={}\n", t.item_id)
        } else {
            let adopts = adopt.as_ref().map_or(String::new(), |a| {
                format!(
                    "{} {} branch: {}\n",
                    crate::ticket::ADOPTS_PR,
                    a.url,
                    a.branch
                )
            });
            format!("Taken.\nitem={}\n{adopts}", t.item_id)
        },
    )?;
    set_select_status(ac, &t.item_id, Status::InProgress)?;
    ac.ctx
        .board
        .set_text(&ac.ctx.client, &ac.agent.item_id, "Doing", &t.key())?;
    remember(ac, &t, Status::InProgress, t.agent());
    let label_line = sync_needs_owner(ac, &t, question_open(&t.msgs, t.agent()).is_some());
    let next = match &adopt {
        Some(a) => adopted_next_steps(&t, a),
        None if resume => next_steps_after_take(ac, me, &t),
        None => format!(
            "{}{}",
            hint(ac, &t, &me.name, true),
            next_steps_after_take(ac, me, &t)
        ),
    };
    Ok(format!(
        "{park_lines}{} {} by {}; Status In Progress; ack {} ({})\n{label_line}{read_lines}{next}",
        t.key(),
        if resume { "resumed" } else { "taken" },
        me.name,
        ack.id,
        ack.url
    ))
}

/// What `take --continue-pr` prints after the ack: the branch and the PR exist already, so only the
/// check-out, the PR body and `done` are left. Pure.
fn adopted_next_steps(t: &Ticket, a: &Adopted) -> String {
    let mut s = format!(
        "adopted the open PR of an earlier agent:\n  branch: {} (git fetch origin, then check it out; do not open a new PR)\n  PR:     {}\n",
        a.branch, a.url
    );
    if !a.closes && !crate::owner_route::declared(&t.info.body) {
        s.push_str(&format!(
            "  note:   the PR body does not say 'Closes #{}' yet: edit it before `done` (`done` refuses otherwise)\n",
            t.issue
        ));
    }
    s.push_str(&format!(
        "  done:   after the event `ci-finished green`: crewboard done {} --pr {} --sha <head sha>\n",
        t.issue, a.url
    ));
    s
}

/// The slug of a branch name from the ticket title: a-z, 0-9 and single `-`, at most 40
/// characters, cut at a word end. `work` when nothing is left.
fn branch_slug(title: &str) -> String {
    let mut s = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    let mut s = s.trim_end_matches('-').to_string();
    if s.len() > 40 {
        s.truncate(40);
        if let Some(i) = s.rfind('-').filter(|&i| i > 0) {
            s.truncate(i);
        }
    }
    if s.is_empty() { "work".into() } else { s }
}

/// What `take` prints after the ack: the branch (spec/tickets#branches), the PR command with
/// the keyword of the route (`Closes #n`, or `Refs #n` on the owner route) and the
/// `done` line. `prefix` is the `dev_branch_prefix` of AUTHORITY.md, or why it could not be
/// read. Pure.
fn take_next_steps(
    prefix: Result<&str, &str>,
    agent: &str,
    repo: &str,
    issue: u64,
    title: &str,
    owner_route: bool,
) -> String {
    let slug = branch_slug(title);
    let (branch, note) = match prefix {
        Ok(p) => (format!("{p}{agent}/{issue}-{slug}"), String::new()),
        Err(why) => (
            format!("<dev_branch_prefix>{agent}/{issue}-{slug}"),
            format!(" (AUTHORITY.md unreadable: {why}; take the prefix from it)"),
        ),
    };
    let keyword = if owner_route { "Refs" } else { "Closes" };
    let mut s = format!("next steps:\n  branch: {branch}{note}\n");
    if owner_route {
        s.push_str(&format!(
            "  route:  owner-workflow: do not push .github/workflows; put the change in {} and restore the workflow files\n",
            crate::owner_route::patch_path(issue)
        ));
    }
    s.push_str(&format!(
        "  PR:     gh pr create --repo {repo} --head {} --title {} --body {}\n",
        shell_quote(&branch),
        shell_quote(title),
        shell_quote(&format!("{keyword} #{issue}"))
    ));
    s.push_str(&format!(
        "  done:   after the event `ci-finished green`: crewboard done {issue} --pr <PR URL> --sha <head sha>\n"
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_state_matrix() {
        let dev = Role::Dev;
        // only `active` takes
        assert!(check_take_state(Some(State::Active), dev, "a@x").is_ok());
        // paused: its own rule, named as before
        assert_eq!(
            check_take_state(Some(State::Paused), dev, "a@x")
                .unwrap_err()
                .rule,
            "agent-paused"
        );
        // every other State: agent-not-active, the text ends with the command that fixes it
        for (st, fix) in [
            (Some(State::Approved), "`crewboard start`"),
            (Some(State::Requested), "`crewboard start`"),
            (Some(State::Draining), "`crewboard join` again"),
            (Some(State::Left), "`crewboard join` again"),
            (None, "`crewboard start`"),
        ] {
            let e = check_take_state(st, dev, "a@x").unwrap_err();
            assert_eq!(e.rule, "agent-not-active", "{st:?}");
            assert!(e.message.starts_with("a@x is "), "{}", e.message);
            assert!(e.message.ends_with(fix), "{}", e.message);
            assert!(!e.message.contains("Some("), "{}", e.message);
        }
        // a coordinator never takes, whatever its State
        assert_eq!(
            check_take_state(Some(State::Active), Role::Coordinator, "a@x")
                .unwrap_err()
                .rule,
            "coordinator-cannot-take"
        );
        // a legacy `working` State of an old board reads as active and takes
        assert!(check_take_state(State::parse("working"), dev, "a@x").is_ok());
    }
}
