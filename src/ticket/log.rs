//! What the signed log says a board field must be (spec/overview#where-things-live).
//!
//! The log is the truth and the board is a view. This module folds the verified messages of one
//! ticket into the status and the agent that the log supports, and folds the registry into the
//! role, the key and the end of the span of one agent. `audit` compares both with the board.
//!
//! Pure: no reads, no writes.

use crate::owner_cmd::{self, OwnerCommand};
use crate::protocol::names::Kind;
use crate::registry::{Registry, fingerprint};
use crate::rules::Role;
use crate::rules::status::Status;
use crate::ticket::{Act, Sender, VMsg, parse_evidence};

/// The status that the log allows for a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Is(Status),
    /// The log allows more than one status (a failed verify: the assignee or the coordinator
    /// moves the ticket on).
    OneOf(Vec<Status>),
    /// No message of the log sets a status.
    Unknown,
}

impl Expected {
    /// `None` when the log says nothing.
    pub fn allows(&self, s: Status) -> Option<bool> {
        match self {
            Expected::Is(x) => Some(*x == s),
            Expected::OneOf(l) => Some(l.contains(&s)),
            Expected::Unknown => None,
        }
    }

    /// For a finding text.
    pub fn describe(&self) -> String {
        match self {
            Expected::Is(s) => s.name().to_string(),
            Expected::OneOf(l) => l.iter().map(|s| s.name()).collect::<Vec<_>>().join(" or "),
            Expected::Unknown => "unknown".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogTicket {
    pub agent: Option<String>,
    pub status: Expected,
}

/// Fold the verified messages of one ticket, in comment order. The kind and the act of a message
/// ([`crate::protocol::acts::parse`]) decide what it does; any other message changes nothing.
///
/// Done is final (spec/tickets#status-flow-and-writers): after a coordinator close, a later
/// message of the assignee changes nothing. Only the owner moves a Done ticket. The owner's
/// `/crew move <status>` ([`OwnerCommand::Move`], spec/tickets#owner-move) sets the status and
/// opens a closed ticket again, so later acts of the assignee count. A later coordinator act (an
/// announcement, an assignment, a reassign, a failed verify) also sets the status again. An owner
/// move keeps the agent. The caller passes only owner comments that count
/// ([`crate::ticket::verified_messages`]: no edit by another account).
pub fn fold_ticket(msgs: &[VMsg]) -> LogTicket {
    let mut agent: Option<String> = None;
    let mut status = Expected::Unknown;
    let mut closed = false;
    for m in msgs {
        if m.sender == Sender::Owner {
            if let Ok(Some(OwnerCommand::Move { status: s })) = owner_cmd::parse(&m.body) {
                // a move never names Done (`owner_cmd::move_target`), so it never closes
                status = Expected::Is(s);
                closed = false;
            }
            continue;
        }
        let coordinator = m.sender.actor().is_coordinator();
        let from = match &m.sender {
            Sender::Agent { name, .. } => Some(name.as_str()),
            Sender::Owner => None,
        };
        let assignee = !closed && from.is_some() && from == agent.as_deref();
        let act = m.act();
        if coordinator {
            match act {
                Act::Closed(_) => closed = true,
                Act::BoardAnnounce
                | Act::Assignment { .. }
                | Act::Reassigned
                | Act::Verify { pass: false } => closed = false,
                _ => {}
            }
        }
        match (m.kind, act) {
            (_, Act::BoardAnnounce) if coordinator => status = Expected::Is(Status::Backlog),
            (_, Act::Assignment { .. }) if coordinator => {
                status = Expected::Is(Status::Ready);
                agent = Some(m.to.clone());
            }
            // A reassign without a new agent ends in Backlog; a board that an older version
            // wrote left such a ticket in Ready. Both are allowed; an assignment message after
            // the notice (a reassign with an agent) sets Ready again.
            (_, Act::Reassigned) if coordinator => {
                status = Expected::OneOf(vec![Status::Backlog, Status::Ready]);
                agent = None;
            }
            (_, Act::Verify { pass: false }) if coordinator => {
                status = Expected::OneOf(vec![Status::InProgress, Status::Ready]);
            }
            (_, Act::Closed(_)) if coordinator => status = Expected::Is(Status::Done),
            (Some(Kind::Ack), _) if assignee => status = Expected::Is(Status::InProgress),
            (Some(Kind::Done), _) if assignee && parse_evidence(&m.body).is_some() => {
                status = Expected::Is(Status::Review);
            }
            (Some(Kind::Blocked), _) if assignee => status = Expected::Is(Status::Blocked),
            (Some(Kind::Question), _) if assignee && m.to == "owner" => {
                // `ask` moves an In Progress ticket to Blocked and leaves any other status; the
                // status before stays allowed (a ticket in Review stays in Review).
                let mut allowed = match &status {
                    Expected::Is(s) => vec![*s],
                    Expected::OneOf(l) => l.clone(),
                    Expected::Unknown => Vec::new(),
                };
                if !allowed.contains(&Status::Blocked) {
                    allowed.push(Status::Blocked);
                }
                status = Expected::OneOf(allowed);
            }
            _ => {}
        }
    }
    LogTicket { agent, status }
}

/// Does the board `Status` contradict the log? `board_status` is the text of the board field
/// (`None`: empty). A status that the log allows, and a ticket that the log says nothing about,
/// do not contradict it; a status that is not one of ours contradicts a log that says something.
/// This is the one judgement that `audit`, `status` and the coordinator's duty share.
pub fn status_contradicts(log: &LogTicket, board_status: Option<&str>) -> bool {
    match board_status.and_then(Status::parse_board) {
        Some(s) => log.status.allows(s) == Some(false),
        None => log.status != Expected::Unknown,
    }
}

/// Does the board `Agent` name an agent that the signed log does not support? The assignee is the
/// agent of the latest signed assignment of a verified coordinator, ended by a later signed
/// reassign (`fold_ticket`). An empty board field never contradicts: it gives nobody any right.
/// A board `Agent` that the log does not name is a hand edit (spec/tickets#assignment).
pub fn agent_contradicts(log: &LogTicket, board_agent: Option<&str>) -> bool {
    board_agent.is_some() && board_agent != log.agent.as_deref()
}

/// The finding text of a ticket whose board `Status` the log does not support.
pub fn status_mismatch_text(key: &str, board_status: Option<&str>, log: &LogTicket) -> String {
    format!(
        "ticket {key}: board Status is {}, the log shows {}",
        board_status.unwrap_or("empty"),
        log.status.describe()
    )
}

/// What the registry log says about one agent now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogAgent {
    /// The role of the open span; `None` when the agent has no open span.
    pub role: Option<Role>,
    /// The fingerprint of the key of the join.
    pub key_fp: Option<String>,
    /// The owner approved the agent (the approval is in the log, also after a reject).
    pub approved: bool,
    /// The span is closed: by a reject or by the marker `lifecycle: left`.
    pub ended: bool,
    /// The span was closed by the marker (the agent left, no reject).
    pub left: bool,
    /// The log stops or ends the agent (a stop of the owner, a stopping notice or marker of
    /// the agent, a reject).
    pub stopped: bool,
}

pub fn fold_agent(reg: &Registry, name: &str) -> LogAgent {
    let Some(e) = reg.agents.get(name) else {
        return LogAgent::default();
    };
    let open = e.spans.last().filter(|s| s.to.is_none());
    let closed = e.spans.last().is_some_and(|s| s.to.is_some());
    LogAgent {
        role: open.map(|s| s.role),
        key_fp: e
            .join
            .as_ref()
            .and_then(|j| j.key())
            .map(|k| fingerprint(&k)),
        approved: e.approval.is_some(),
        ended: closed || e.rejected,
        left: closed && !e.rejected,
        stopped: e.departure.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::names::Re;
    use crate::protocol::signature::generate_key;
    use crate::registry::resolve;
    use crate::registry::tests::{cfg, join_comment, left_marker, owner};

    fn msg(id: u64, from: &str, role: Role, to: &str, kind: Kind, body: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: id,
            created: id as i64,
            sender: Sender::Agent {
                name: from.into(),
                role,
            },
            to: to.into(),
            kind: Some(kind),
            re: Re::None,
            body: body.into(),
        }
    }

    fn coord(id: u64, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(id, "agent-a@host-a", Role::Coordinator, to, kind, body)
    }

    fn dev(id: u64, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(id, "agent-b@host-b", Role::Dev, to, kind, body)
    }

    const ASSIGN: &str = "Assigned to you: t";
    const EVIDENCE: &str = "Done.\npr: https://github.com/o/r/pull/1\nsha: abc\nci: success\n";

    fn assigned() -> Vec<VMsg> {
        vec![
            coord(1, "agent-b@host-b", Kind::Request, ASSIGN),
            dev(2, "coordinator", Kind::Ack, "Taken."),
        ]
    }

    fn status_of(msgs: &[VMsg]) -> Expected {
        fold_ticket(msgs).status
    }

    #[test]
    fn a_board_status_contradicts_the_log_only_when_the_log_says_something_else() {
        let log = |status| LogTicket {
            agent: None,
            status,
        };
        let review = log(Expected::Is(Status::Review));
        assert!(
            status_contradicts(&review, Some("Done")),
            "Done without proof"
        );
        assert!(!status_contradicts(&review, Some("Review")));
        assert!(status_contradicts(&review, None), "an empty Status");
        assert!(
            status_contradicts(&review, Some("Elsewhere")),
            "not one of ours"
        );
        // the log allows more than one status after a failed verify
        let failed = log(Expected::OneOf(vec![Status::InProgress, Status::Ready]));
        assert!(!status_contradicts(&failed, Some("Ready")));
        assert!(status_contradicts(&failed, Some("Done")));
        // a ticket that the log says nothing about is nobody's mismatch
        let none = log(Expected::Unknown);
        assert!(!status_contradicts(&none, Some("Done")));
        assert!(!status_contradicts(&none, None));
        assert_eq!(
            status_mismatch_text("o/r#7", Some("Done"), &review),
            "ticket o/r#7: board Status is Done, the log shows Review"
        );
    }

    #[test]
    fn a_board_agent_contradicts_the_log_unless_a_signed_assignment_names_it() {
        let log = |agent: Option<&str>| LogTicket {
            agent: agent.map(str::to_string),
            status: Expected::Unknown,
        };
        assert!(!agent_contradicts(&log(Some("a@x")), Some("a@x")));
        assert!(agent_contradicts(&log(Some("a@x")), Some("b@y")), "other");
        assert!(agent_contradicts(&log(None), Some("b@y")), "no assignment");
        assert!(!agent_contradicts(&log(None), None));
        assert!(!agent_contradicts(&log(Some("a@x")), None), "empty field");
        // through the fold: an unsigned agent, a reassign, a hand edit after a reassign
        let mut m = assigned();
        assert!(!agent_contradicts(&fold_ticket(&m), Some("agent-b@host-b")));
        assert!(agent_contradicts(&fold_ticket(&m), Some("agent-c@host-c")));
        m.push(coord(3, "owner", Kind::Info, "Reassigned.\nreason: x"));
        assert!(agent_contradicts(&fold_ticket(&m), Some("agent-b@host-b")));
        m.push(coord(4, "agent-c@host-c", Kind::Request, ASSIGN));
        assert!(!agent_contradicts(&fold_ticket(&m), Some("agent-c@host-c")));
        assert!(agent_contradicts(&fold_ticket(&m), Some("agent-b@host-b")));
        // an assignment by a dev is no assignment
        let forged = [dev(1, "agent-b@host-b", Kind::Request, ASSIGN)];
        assert!(agent_contradicts(
            &fold_ticket(&forged),
            Some("agent-b@host-b")
        ));
    }

    #[test]
    fn no_message_gives_an_unknown_status_and_no_agent() {
        assert_eq!(
            fold_ticket(&[]),
            LogTicket {
                agent: None,
                status: Expected::Unknown
            }
        );
    }

    #[test]
    fn the_board_announcement_is_backlog() {
        let m = [coord(1, "owner", Kind::Info, "Ticket is on the board: x")];
        assert_eq!(status_of(&m), Expected::Is(Status::Backlog));
    }

    #[test]
    fn an_assignment_is_ready_with_the_agent() {
        let t = fold_ticket(&assigned()[..1]);
        assert_eq!(t.status, Expected::Is(Status::Ready));
        assert_eq!(t.agent.as_deref(), Some("agent-b@host-b"));
    }

    #[test]
    fn a_reassignment_is_backlog_or_ready_without_an_agent_and_ready_after_a_new_assignment() {
        let mut m = assigned();
        m.push(coord(3, "owner", Kind::Info, "Reassigned.\nreason: x"));
        let t = fold_ticket(&m);
        // Backlog now; Ready on a board that an older version wrote
        assert_eq!(
            t.status,
            Expected::OneOf(vec![Status::Backlog, Status::Ready])
        );
        assert_eq!(t.agent, None);
        // a reassign with a new agent posts the assignment after the notice
        m.push(coord(4, "agent-b@host-b", Kind::Request, ASSIGN));
        let t = fold_ticket(&m);
        assert_eq!(t.status, Expected::Is(Status::Ready));
        assert_eq!(t.agent.as_deref(), Some("agent-b@host-b"));
    }

    #[test]
    fn the_ack_of_the_assignee_is_in_progress() {
        assert_eq!(status_of(&assigned()), Expected::Is(Status::InProgress));
    }

    #[test]
    fn the_ack_of_another_agent_changes_nothing() {
        let mut m = assigned()[..1].to_vec();
        m.push(msg(
            2,
            "agent-c@host-c",
            Role::Dev,
            "coordinator",
            Kind::Ack,
            "Taken.",
        ));
        assert_eq!(status_of(&m), Expected::Is(Status::Ready));
    }

    #[test]
    fn a_done_with_evidence_is_review_and_a_done_without_is_not() {
        let mut m = assigned();
        m.push(dev(3, "coordinator", Kind::Done, "Done, no evidence."));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
        m.push(dev(4, "coordinator", Kind::Done, EVIDENCE));
        assert_eq!(status_of(&m), Expected::Is(Status::Review));
    }

    #[test]
    fn a_block_of_the_assignee_is_blocked() {
        let mut m = assigned();
        m.push(dev(3, "coordinator", Kind::Blocked, "need a decision"));
        assert_eq!(status_of(&m), Expected::Is(Status::Blocked));
    }

    #[test]
    fn a_question_to_the_owner_keeps_the_status_before_and_allows_blocked() {
        let mut m = assigned();
        m.push(dev(3, "owner", Kind::Question, "which library?"));
        assert_eq!(
            status_of(&m),
            Expected::OneOf(vec![Status::InProgress, Status::Blocked])
        );
        // after a done: Review stays allowed
        let mut m = assigned();
        m.push(dev(3, "coordinator", Kind::Done, EVIDENCE));
        m.push(dev(4, "owner", Kind::Question, "ok to merge?"));
        assert_eq!(
            status_of(&m),
            Expected::OneOf(vec![Status::Review, Status::Blocked])
        );
        // a second question does not repeat a status; an owner message changes nothing
        m.push(dev(5, "owner", Kind::Question, "again?"));
        m.push(VMsg {
            ts: String::new(),
            comment_id: 6,
            created: 6,
            sender: Sender::Owner,
            to: "coordinator".into(),
            kind: None,
            re: Re::None,
            body: "yes".into(),
        });
        assert_eq!(
            status_of(&m),
            Expected::OneOf(vec![Status::Review, Status::Blocked])
        );
        // the ack after it is In Progress again
        m.push(dev(7, "coordinator", Kind::Ack, "Taken."));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    #[test]
    fn a_question_to_the_coordinator_or_by_another_agent_changes_nothing() {
        let mut m = assigned();
        m.push(dev(3, "coordinator", Kind::Question, "which library?"));
        m.push(msg(
            4,
            "agent-c@host-c",
            Role::Dev,
            "owner",
            Kind::Question,
            "x?",
        ));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    #[test]
    fn a_failed_verify_allows_in_progress_or_ready() {
        let mut m = assigned();
        m.push(coord(3, "agent-b@host-b", Kind::Report, "verify: fail\nx"));
        assert_eq!(
            status_of(&m),
            Expected::OneOf(vec![Status::InProgress, Status::Ready])
        );
    }

    #[test]
    fn each_close_line_of_the_coordinator_is_done() {
        for line in [
            "Closed after a passed verify",
            "Merged by the coordinator.\npr: x",
            "closed: superseded by o/r#12",
            "The ticket is closed: the owner merged the PR by hand.",
        ] {
            let mut m = assigned();
            m.push(coord(3, "owner", Kind::Info, line));
            assert_eq!(status_of(&m), Expected::Is(Status::Done), "{line}");
        }
    }

    #[test]
    fn done_is_final_for_the_assignee_after_a_close() {
        for (kind, body) in [
            (Kind::Ack, "Taken."),
            (Kind::Done, EVIDENCE),
            (Kind::Blocked, "stuck"),
        ] {
            let mut m = assigned();
            m.push(coord(
                3,
                "owner",
                Kind::Info,
                "Closed after a passed verify",
            ));
            m.push(dev(4, "coordinator", kind, body));
            m.push(dev(5, "owner", Kind::Question, "x?"));
            assert_eq!(
                fold_ticket(&m),
                LogTicket {
                    agent: Some("agent-b@host-b".into()),
                    status: Expected::Is(Status::Done)
                },
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_coordinator_act_after_a_close_sets_the_status_again() {
        // the owner moved the Done ticket back on the board, and the coordinator assigns it
        let mut m = assigned();
        m.push(coord(
            3,
            "owner",
            Kind::Info,
            "closed: superseded by o/r#12",
        ));
        m.push(coord(4, "agent-b@host-b", Kind::Request, ASSIGN));
        m.push(dev(5, "coordinator", Kind::Ack, "Taken again."));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    fn owner_says(id: u64, body: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: id,
            created: id as i64,
            sender: Sender::Owner,
            to: "coordinator".into(),
            kind: None,
            re: Re::None,
            body: body.into(),
        }
    }

    #[test]
    fn an_owner_move_sets_the_status_and_keeps_the_agent() {
        let mut m = assigned();
        m.push(owner_says(3, "/crew move Backlog\nnot now"));
        assert_eq!(
            fold_ticket(&m),
            LogTicket {
                agent: Some("agent-b@host-b".into()),
                status: Expected::Is(Status::Backlog)
            }
        );
        // the board that follows the move has no mismatch; a plain drag elsewhere still has one
        let log = fold_ticket(&m);
        assert!(!status_contradicts(&log, Some("Backlog")));
        assert!(status_contradicts(&log, Some("Ready")));
        // later acts fold as before: the coordinator assigns, the assignee takes
        m.push(coord(4, "agent-b@host-b", Kind::Request, ASSIGN));
        assert_eq!(status_of(&m), Expected::Is(Status::Ready));
        m.push(dev(5, "coordinator", Kind::Ack, "Taken."));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    #[test]
    fn an_owner_move_after_a_close_opens_the_ticket_again_for_the_assignee() {
        let mut m = assigned();
        m.push(coord(
            3,
            "owner",
            Kind::Info,
            "Closed after a passed verify",
        ));
        assert_eq!(status_of(&m), Expected::Is(Status::Done));
        m.push(owner_says(4, "/crew move Ready"));
        assert_eq!(
            fold_ticket(&m),
            LogTicket {
                agent: Some("agent-b@host-b".into()),
                status: Expected::Is(Status::Ready)
            }
        );
        // the assignee's acts count again after the move
        m.push(dev(5, "coordinator", Kind::Ack, "Taken again."));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
        m.push(dev(6, "coordinator", Kind::Done, EVIDENCE));
        assert_eq!(status_of(&m), Expected::Is(Status::Review));
        // a second close is Done again, and Done is final again
        m.push(coord(
            7,
            "owner",
            Kind::Info,
            "Closed after a passed verify",
        ));
        m.push(dev(8, "coordinator", Kind::Ack, "Taken."));
        assert_eq!(status_of(&m), Expected::Is(Status::Done));
    }

    #[test]
    fn a_move_line_that_is_not_an_owner_command_changes_nothing() {
        let mut m = assigned();
        // a signed message of the coordinator or of the assignee with the owner's words
        m.push(coord(3, "owner", Kind::Info, "/crew move Backlog"));
        m.push(dev(4, "owner", Kind::Info, "/crew move Ready"));
        // an owner comment that is not a valid move: Done, an unknown status, a word before it
        m.push(owner_says(5, "/crew move Done"));
        m.push(owner_says(6, "/crew move Somewhere"));
        m.push(owner_says(7, "please /crew move Backlog"));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    #[test]
    fn a_close_line_of_a_dev_changes_nothing() {
        let mut m = assigned();
        m.push(dev(3, "owner", Kind::Info, "Closed after a passed verify"));
        assert_eq!(status_of(&m), Expected::Is(Status::InProgress));
    }

    #[test]
    fn an_unknown_message_does_not_change_the_state() {
        let mut m = assigned();
        m.push(coord(3, "agent-b@host-b", Kind::Info, "Some other news"));
        m.push(dev(4, "coordinator", Kind::Report, "progress"));
        m.push(VMsg {
            ts: String::new(),
            comment_id: 5,
            created: 5,
            sender: Sender::Owner,
            to: "coordinator".into(),
            kind: None,
            re: Re::None,
            body: "a remark".into(),
        });
        assert_eq!(
            fold_ticket(&m),
            LogTicket {
                agent: Some("agent-b@host-b".into()),
                status: Expected::Is(Status::InProgress)
            }
        );
    }

    #[test]
    fn expected_allows_what_the_log_names() {
        assert_eq!(Expected::Is(Status::Done).allows(Status::Done), Some(true));
        assert_eq!(
            Expected::Is(Status::Done).allows(Status::Ready),
            Some(false)
        );
        let one = Expected::OneOf(vec![Status::Ready, Status::InProgress]);
        assert_eq!(one.allows(Status::Ready), Some(true));
        assert_eq!(one.allows(Status::Done), Some(false));
        assert_eq!(Expected::Unknown.allows(Status::Done), None);
    }

    #[test]
    fn agent_fold_reject_left_marker_and_role_change() {
        let (ka, kb, kc, kd) = (
            generate_key().unwrap(),
            generate_key().unwrap(),
            generate_key().unwrap(),
            generate_key().unwrap(),
        );
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "agent-a@host-a", &ka, "PVTI_a"),
                join_comment(2, "agent-b@host-b", &kb, "PVTI_b"),
                join_comment(3, "agent-c@host-c", &kc, "PVTI_c"),
                join_comment(4, "agent-d@host-d", &kd, "PVTI_d"),
                owner(5, "/crew approve agent-a@host-a role=coordinator"),
                owner(6, "/crew approve agent-b@host-b role=dev"),
                owner(7, "/crew approve agent-c@host-c role=dev"),
                owner(8, "/crew role agent-a@host-a dev"),
                owner(9, "/crew role agent-b@host-b coordinator"),
                left_marker(10, "agent-c@host-c", &kc),
                owner(11, "/crew reject agent-a@host-a"),
            ],
        );
        // a role change: the span role now
        let b = fold_agent(&reg, "agent-b@host-b");
        assert_eq!(b.role, Some(Role::Coordinator));
        assert!(b.approved && !b.ended && !b.left);
        assert_eq!(
            b.key_fp.as_deref(),
            Some(fingerprint(kb.public_key()).as_str())
        );
        // a left marker
        let c = fold_agent(&reg, "agent-c@host-c");
        assert_eq!((c.role, c.ended, c.left), (None, true, true));
        // a reject
        let a = fold_agent(&reg, "agent-a@host-a");
        assert_eq!((a.role, a.ended, a.left), (None, true, false));
        // a join without approval
        let d = fold_agent(&reg, "agent-d@host-d");
        assert!(!d.approved && !d.ended && d.role.is_none() && d.key_fp.is_some());
        // an unknown name
        assert_eq!(fold_agent(&reg, "nobody@nowhere"), LogAgent::default());
    }
}
