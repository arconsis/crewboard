//! `block`, `unblock` and `ask`: a ticket waits for an answer, or an open question gets its answer.

use super::in_progress::write_idle_when_free;
use super::{
    check_status_unchanged, facts, load_world_cached, me_of, needs_owner_lines, post, remember,
    set_select_status, to_agent, unread_guard,
};
use crate::base::ids;
use crate::error::{CbError, CbResult};
use crate::ops::{AgentCtx, parse_issue_arg};
use crate::protocol::acts::withdraw_body;
use crate::protocol::names::{Kind, Re, To};
use crate::rules::Role;
use crate::rules::status::{Status, ask_moves_to, check_ask, check_status_transition};
use crate::services::ticket_loader::load_ticket;
use crate::ticket::{Sender, VMsg, answer_after_block, blocked_by_owner_question};

/// `block <issue> --why ...` (dev).
pub fn block(ac: &AgentCtx, issue_arg: &str, reason: &str, anyway: bool) -> CbResult<String> {
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    if reason.trim().is_empty() {
        return Err(CbError::refused(
            ids::BLOCK_NEEDS_REASON,
            "block needs a reason (--why)",
        ));
    }
    crate::ops::say::check_free_text(Kind::Blocked, reason.trim())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    t.check_agent_signed()?;
    let unread_line = unread_guard(ac, &world, &t, anyway)?;
    let unread_note = crate::unread::note(unread_line.as_deref());
    let from = t.status()?;
    let tr = facts(&t, &me, from).plain();
    check_status_transition(Some(from), Status::Blocked, me.actor, &tr)?;
    // A ticket that was moved since it was read gets no message.
    check_status_unchanged(ac, &t.item_id, &t.key(), from, None)?;
    let posted = post(
        ac,
        &t,
        To::Coordinator,
        Kind::Blocked,
        Re::None,
        &crate::unread::with_override(reason.trim(), unread_line.as_deref()),
    )?;
    set_select_status(ac, &t.item_id, Status::Blocked)?;
    write_idle_when_free(ac, &world, &t)?;
    remember(ac, &t, Status::Blocked, t.agent());
    Ok(format!(
        "{} blocked; message {} ({})\n{unread_note}",
        t.key(),
        posted.id,
        posted.url
    ))
}

/// What `unblock` posts: the text, and whether it withdraws an open question to the owner.
#[derive(Debug, PartialEq, Eq)]
struct UnblockPlan {
    body: String,
    withdraws: bool,
}

/// The answer that `unblock` posts. When the latest blocking message is a question to the owner,
/// a plain answer would not unblock the ticket (`answer_after_block`), so the answer withdraws the
/// question with the line that `protocol::acts` writes. Otherwise it is the plain answer.
fn plan_unblock(
    msgs: &[VMsg],
    assignee: &str,
    text: &str,
    unread_line: Option<&str>,
) -> UnblockPlan {
    let withdraws = blocked_by_owner_question(msgs, Some(assignee));
    let text = if withdraws {
        withdraw_body(text.trim())
    } else {
        text.trim().to_string()
    };
    UnblockPlan {
        body: crate::unread::with_override(&text, unread_line),
        withdraws,
    }
}

/// Does the message that `unblock` is about to post unblock the ticket? It runs
/// `answer_after_block` on the messages and the new one, so `unblock` never says "unblocked"
/// for a message that `take` would not accept.
fn plan_unblocks(msgs: &[VMsg], me: &str, role: Role, assignee: &str, body: &str) -> bool {
    let mut all = msgs.to_vec();
    all.push(VMsg {
        comment_id: u64::MAX,
        created: 0,
        ts: String::new(),
        sender: Sender::Agent {
            name: me.to_string(),
            role,
        },
        to: assignee.to_string(),
        kind: Some(Kind::Answer),
        re: Re::None,
        body: body.to_string(),
    });
    answer_after_block(&all, Status::Blocked, Some(assignee))
}

/// `unblock <issue> [--body ...]` (coordinator): post the signed `kind=answer` to the
/// assignee of a Blocked ticket, so that `take` resumes it. The posting is the one of `say`. When
/// an open question to the owner blocks the ticket, the answer also withdraws that question: a
/// plain answer does not unblock it.
pub fn unblock(
    ac: &AgentCtx,
    issue_arg: &str,
    body: Option<String>,
    anyway: bool,
) -> CbResult<String> {
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    t.check_agent_signed()?;
    let from = t.status()?;
    let assignee = t.agent().map(str::to_string);
    crate::rules::status::check_unblock(from, me.actor, assignee.is_some())?;
    let assignee = assignee.unwrap_or_default();
    let unread_line = unread_guard(ac, &world, &t, anyway)?;
    let unread_note = crate::unread::note(unread_line.as_deref());
    let text = body
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| format!("resume allowed: run take {}", tref.issue));
    let plan = plan_unblock(&t.msgs, &assignee, &text, unread_line.as_deref());
    if !plan_unblocks(&t.msgs, &me.name, me.role, &assignee, &plan.body) {
        return Err(CbError::refused(
            ids::UNBLOCK_NOT_EFFECTIVE,
            format!(
                "{} stays Blocked: the answer would not satisfy `take` (nothing was written)",
                t.key()
            ),
        ));
    }
    let posted = post(
        ac,
        &t,
        to_agent(&assignee)?,
        Kind::Answer,
        Re::None,
        &plan.body,
    )?;
    let what = if plan.withdraws {
        "unblocked: the open question to the owner is withdrawn;"
    } else {
        "unblocked:"
    };
    Ok(format!(
        "{} {what} answer to {assignee} as comment {} in {}#{}\nref: #{}/c-{}\n{}\n{unread_note}",
        t.key(),
        posted.id,
        posted.repo,
        posted.issue,
        posted.issue,
        posted.id,
        posted.url
    ))
}

/// `ask <issue> --body ...` (coordinator or dev): a question to the owner and the label
/// `needs-owner`. The assignee's ticket that is In Progress also moves to Blocked, so the agent
/// is free for its next ticket; any other status stays.
pub fn ask(ac: &AgentCtx, issue_arg: &str, question: &str, anyway: bool) -> CbResult<String> {
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    if question.trim().is_empty() {
        return Err(CbError::refused(
            ids::ASK_NEEDS_TEXT,
            "ask needs the question (--body or --body-file)",
        ));
    }
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    t.check_agent_signed()?;
    let from = t.status()?;
    let is_assignee = t.agent() == Some(me.name.as_str());
    check_ask(&t.key(), me.actor, is_assignee, from)?;
    let unread_line = unread_guard(ac, &world, &t, anyway)?;
    let unread_note = crate::unread::note(unread_line.as_deref());
    let moves_to = ask_moves_to(from, is_assignee);
    let body = crate::unread::with_override(question.trim(), unread_line.as_deref());
    if moves_to.is_some() {
        check_status_unchanged(ac, &t.item_id, &t.key(), from, None)?;
    }
    let posted = post(ac, &t, To::Owner, Kind::Question, Re::None, &body)?;
    let mut out = format!(
        "{} question {} ({}); label needs-owner\n",
        t.key(),
        posted.id,
        posted.url
    );
    // The label comes right after the post: nothing adds a missing label later, so a failed
    // Status or idle write below must not leave an open question without it.
    let label = needs_owner_lines(ac, &t.repo, t.issue, true);
    if let Some(to) = moves_to {
        set_select_status(ac, &t.item_id, to)?;
        write_idle_when_free(ac, &world, &t)?;
        remember(ac, &t, to, t.agent());
        out.push_str(&format!("Status {}\n", to.name()));
    }
    out.push_str(&unread_note);
    out.push_str(&label);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::msg;

    const DEV: &str = "hermes@b";

    fn agent(name: &str, role: Role) -> Sender {
        Sender::Agent {
            name: name.to_string(),
            role,
        }
    }

    fn question(id: u64) -> VMsg {
        msg(
            id,
            agent(DEV, Role::Dev),
            "owner",
            Some(Kind::Question),
            "Which layout?",
        )
    }

    fn blocked(id: u64) -> VMsg {
        msg(
            id,
            agent(DEV, Role::Dev),
            "coordinator",
            Some(Kind::Blocked),
            "stuck",
        )
    }

    #[test]
    fn an_owner_question_is_unblocked_by_an_answer_with_the_withdraws_line() {
        let msgs = [question(1)];
        let plan = plan_unblock(&msgs, DEV, "resume allowed: run take 7", None);
        assert!(plan.withdraws);
        assert!(plan.body.starts_with("resume allowed: run take 7\n"));
        assert!(
            plan.body
                .lines()
                .any(|l| l == crate::protocol::acts::WITHDRAWS_QUESTION_LINE)
        );
        assert!(plan_unblocks(
            &msgs,
            "claude@a",
            Role::Coordinator,
            DEV,
            &plan.body
        ));
        // the unread override line stays and the withdraws line stays a line of its own
        let plan = plan_unblock(&msgs, DEV, "go", Some("override: x"));
        assert!(plan.body.ends_with("\noverride: x"));
        assert!(plan_unblocks(
            &msgs,
            "claude@a",
            Role::Coordinator,
            DEV,
            &plan.body
        ));
    }

    #[test]
    fn a_plain_block_is_unblocked_by_a_plain_answer() {
        let msgs = [blocked(1)];
        let plan = plan_unblock(&msgs, DEV, " go on ", None);
        assert_eq!(
            plan,
            UnblockPlan {
                body: "go on".into(),
                withdraws: false
            }
        );
        assert!(plan_unblocks(
            &msgs,
            "claude@a",
            Role::Coordinator,
            DEV,
            &plan.body
        ));
        // a question that a later block follows is a plain block again
        let msgs = [question(1), blocked(2)];
        assert!(!plan_unblock(&msgs, DEV, "go", None).withdraws);
        // the latest block is the question
        let msgs = [blocked(1), question(2)];
        assert!(plan_unblock(&msgs, DEV, "go", None).withdraws);
    }

    #[test]
    fn a_plain_answer_to_an_owner_question_is_not_effective() {
        // the control of the check itself: the plain text does not satisfy answer_after_block
        let msgs = [question(1)];
        assert!(!plan_unblocks(
            &msgs,
            "claude@a",
            Role::Coordinator,
            DEV,
            "go on"
        ));
        // and a dev message never does, whatever it says
        let with_line = crate::protocol::acts::withdraw_body("go");
        assert!(!plan_unblocks(
            &msgs,
            "hermes@b",
            Role::Dev,
            DEV,
            &with_line
        ));
        assert!(plan_unblocks(
            &msgs,
            "claude@a",
            Role::Coordinator,
            DEV,
            &with_line
        ));
    }
}
