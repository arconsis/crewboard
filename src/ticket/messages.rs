use super::*;
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::protocol::header::Message;
use crate::protocol::names::{Kind, Re};
use crate::protocol::replay::Seen;
use crate::registry::{CREW_PREFIX, Trust};
use crate::rules::status::{BlockReason, Status};
use crate::rules::{Actor, Role};

/// Who wrote a verified comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    /// The owner (a comment of the owner's account without a crew header).
    Owner,
    Agent {
        name: String,
        role: Role,
    },
}

impl Sender {
    pub fn actor(&self) -> Actor {
        match self {
            Sender::Owner => Actor::Owner,
            Sender::Agent { role, .. } => Actor::Agent(*role),
        }
    }
}

/// A comment that is valid: by the owner without a header, or a signed message of a trusted
/// agent that was not edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VMsg {
    pub comment_id: u64,
    /// Creation time of the comment (GitHub's clock), Unix seconds.
    pub created: i64,
    /// The signed `ts` of the header (empty for an owner comment).
    pub ts: String,
    pub sender: Sender,
    pub to: String,
    /// `None` for an owner comment.
    pub kind: Option<Kind>,
    pub re: Re,
    pub body: String,
}

/// Keep what is valid, in comment order. Foreign authors, edited messages (an owner comment
/// counts unless somebody but the owner edited it), bad signatures and untrusted senders are
/// dropped here (`watch` reports them as security events). So is a copy: a later comment with
/// the same sender and signature as an earlier valid one ([`Seen`]).
pub fn verified_messages(comments: &[Comment], cfg: &BoardConfig, trust: &Trust) -> Vec<VMsg> {
    let authors = cfg.crew_authors();
    let mut sorted: Vec<&Comment> = comments.iter().collect();
    sorted.sort_by_key(|c| c.id);
    let mut out = Vec::new();
    let mut seen = Seen::default();
    for c in sorted {
        if c.body.starts_with(CREW_PREFIX) {
            if !authors.iter().any(|a| c.by(a)) || c.edited() {
                continue;
            }
            let Ok(msg) = Message::parse(&c.body) else {
                continue;
            };
            let Some(sender) = trust.at(msg.header.from.as_str(), c.id) else {
                continue;
            };
            if c.verify(&msg, &sender.key, cfg.former_for(&c.repo))
                .is_err()
                || c.check_ts(&msg).is_err()
            {
                continue;
            }
            if seen
                .first_use(msg.header.from.as_str(), &msg.header.sig, c.id)
                .is_err()
            {
                continue;
            }
            out.push(VMsg {
                comment_id: c.id,
                created: c.created_epoch,
                ts: msg.header.ts.to_string(),
                sender: Sender::Agent {
                    name: sender.name.to_string(),
                    role: sender.role,
                },
                to: msg.header.to.to_string(),
                kind: Some(msg.header.kind),
                re: msg.header.re,
                body: msg.body,
            });
        } else if c.by(&cfg.owner_login) {
            // An owner comment that anyone else edited is not the owner's word
            // (spec/security-details#edited-owner-comments); `watch` reports it.
            if c.untrusted_owner_edit(&cfg.owner_login).is_some() {
                continue;
            }
            out.push(VMsg {
                comment_id: c.id,
                created: c.created_epoch,
                ts: String::new(),
                sender: Sender::Owner,
                to: "coordinator".into(),
                kind: None,
                re: Re::None,
                body: c.body.clone(),
            });
        }
    }
    out
}

impl VMsg {
    /// What the message means ([`crate::protocol::acts::parse`]): the one reader of message acts.
    pub fn act(&self) -> Act {
        crate::protocol::acts::parse(self.kind, &self.body)
    }
}

fn is_owner(m: &VMsg) -> bool {
    m.sender == Sender::Owner
}

/// A `kind=question` addressed to the owner that counts on a ticket: its sender is a coordinator
/// or the assignee. A question of another dev agent opens nothing and blocks nothing.
fn is_owner_question(m: &VMsg, assignee: Option<&str>) -> bool {
    m.kind == Some(Kind::Question)
        && m.to == "owner"
        && match &m.sender {
            Sender::Agent { name, role } => {
                *role == Role::Coordinator || assignee == Some(name.as_str())
            }
            Sender::Owner => false,
        }
}

/// Is `m` a coordinator `kind=answer` that withdraws the question?
pub fn withdraws_question(m: &VMsg) -> bool {
    m.sender.actor().is_coordinator() && m.act() == Act::WithdrawsQuestion
}

/// spec/tickets#status-flow-and-writers: the question to the owner that is open on the ticket.
/// It is the latest `kind=question` to the owner, and no owner message (a headerless comment of
/// the owner account) and no coordinator `answer` with the line `withdraws: question` comes
/// after it. Only a question of the assignee or of a coordinator counts. A plain answer of the coordinator or any message of a dev agent does not close it.
pub fn question_open<'a>(msgs: &'a [VMsg], assignee: Option<&str>) -> Option<&'a VMsg> {
    let at = msgs.iter().rposition(|m| is_owner_question(m, assignee))?;
    msgs[at + 1..]
        .iter()
        .all(|m| !is_owner(m) && !withdraws_question(m))
        .then(|| &msgs[at])
}

/// Does an answer follow the message at position `at`? An answer is an owner message or a
/// coordinator `kind=answer` that [`withdraws_question`]: the two that close a question to the
/// owner ([`question_open`]).
pub fn answered_after(msgs: &[VMsg], at: usize) -> bool {
    msgs.get(at + 1..)
        .is_some_and(|rest| rest.iter().any(|m| is_owner(m) || withdraws_question(m)))
}

/// spec/tickets#status-flow-and-writers: was an answer posted after the message that blocked the
/// ticket?
///
/// - `Blocked`: the latest of the `kind=blocked` and `kind=question` (to the owner) messages
///   counts. After a `blocked`: an owner message or a coordinator `kind=answer`. After a
///   `question`: an owner message or a coordinator `answer` that [`withdraws_question`].
/// - any other status, or no blocking message at all: `false` (nothing to answer).
pub fn answer_after_block(msgs: &[VMsg], status: Status, assignee: Option<&str>) -> bool {
    if status != Status::Blocked {
        return false;
    }
    let Some(at) = latest_block(msgs, assignee) else {
        return false;
    };
    let accepts: fn(&VMsg) -> bool = if is_owner_question(&msgs[at], assignee) {
        |m| is_owner(m) || withdraws_question(m)
    } else {
        |m| is_owner(m) || (m.sender.actor().is_coordinator() && m.kind == Some(Kind::Answer))
    };
    msgs[at + 1..].iter().any(accepts)
}

/// The position of the latest blocking message: a `kind=blocked` or an owner question that counts
/// ([`is_owner_question`]).
fn latest_block(msgs: &[VMsg], assignee: Option<&str>) -> Option<usize> {
    msgs.iter()
        .rposition(|m| m.kind == Some(Kind::Blocked) || is_owner_question(m, assignee))
}

/// Is the latest blocking message of the ticket a question to the owner? Then a plain coordinator
/// answer does not unblock it: it needs an owner message or an answer that [`withdraws_question`].
/// Use it only for a Blocked ticket; it reads the messages and not the status.
pub fn blocked_by_owner_question(msgs: &[VMsg], assignee: Option<&str>) -> bool {
    latest_block(msgs, assignee).is_some_and(|at| is_owner_question(&msgs[at], assignee))
}

/// A coordinator `kind=info` message follows the latest `blocked` message. An `info`
/// does not unblock a ticket; the dev's `take` names this case in its refusal.
pub fn info_after_block(msgs: &[VMsg]) -> bool {
    let Some(at) = msgs.iter().rposition(|m| m.kind == Some(Kind::Blocked)) else {
        return false;
    };
    msgs[at + 1..]
        .iter()
        .any(|m| m.sender.actor().is_coordinator() && m.kind == Some(Kind::Info))
}

/// The reason of a `kind=blocked` message: a park (`parked for #<n>: <reason>`) or a plain block.
pub fn block_reason_of(m: &VMsg) -> BlockReason {
    match m.act() {
        Act::Parked { for_issue } => BlockReason::Parked { for_issue },
        _ => BlockReason::Plain,
    }
}

/// spec/tickets#status-flow-and-writers: is the ticket parked by `agent`? True (with the number of
/// the interrupting ticket) when the latest `blocked` message was written by `agent` itself and is
/// a "parked for #N" message. A park message of another agent, or an older park followed by a
/// normal block, does not count.
pub fn parked_for(msgs: &[VMsg], agent: &str) -> Option<u64> {
    match own_block_reason(msgs, agent) {
        BlockReason::Parked { for_issue } => Some(for_issue),
        BlockReason::Plain => None,
    }
}

/// The reason of the latest `blocked` message when `agent` wrote it; [`BlockReason::Plain`] for a
/// block of another sender or no block at all.
pub fn own_block_reason(msgs: &[VMsg], agent: &str) -> BlockReason {
    let Some(last) = msgs.iter().rev().find(|m| m.kind == Some(Kind::Blocked)) else {
        return BlockReason::Plain;
    };
    match &last.sender {
        Sender::Agent { name, .. } if name == agent => block_reason_of(last),
        _ => BlockReason::Plain,
    }
}

/// The link line that `ticket split` appends to the old body (spec/tickets#ticket-split).
pub fn split_line(key: &str, title: &str) -> String {
    format!("Split off: {key} - {title}")
}

/// The `(key, title)` of every `Split off:` line of a ticket body.
pub fn split_off_lines(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Split off: ")?;
            let (key, t) = rest.split_once(" - ")?;
            Some((key.trim().to_string(), t.trim().to_string()))
        })
        .collect()
}

/// The signed note that `ticket split` posts on the old ticket (spec/tickets#ticket-split).
pub fn split_note(old_key: &str, new_key: &str, title: &str) -> String {
    format!(
        "The scope of {old_key} shrank. The open part moved to {new_key} ({title}), which is in your queue.\nRun `done {old_key} --pr <PR>` for the finished part. No `take` is needed, whatever the status of {old_key} is.\n"
    )
}

/// Is `m` the split note of the coordinator for `new_key`, sent to `agent`? The sender is
/// checked by the signature (`verified_messages`) and by its role.
pub fn is_split_note(m: &VMsg, agent: &str, old_key: &str, new_key: &str) -> bool {
    m.kind == Some(Kind::Info)
        && m.sender.actor().is_coordinator()
        && m.to == agent
        && m.body.contains(&format!(
            "The scope of {old_key} shrank. The open part moved to {new_key} ("
        ))
}

/// Did a coordinator split this ticket? The body has a `Split off: <key> - ...`
/// line AND a signed split note of a coordinator for the same `<key>` is on the ticket. The
/// body line alone proves nothing: anyone who can edit the issue can write it. The note is
/// signed, so only a trusted coordinator makes it. Pure.
pub fn split_off_proven(body: &str, msgs: &[VMsg], agent: &str, old_key: &str) -> bool {
    split_off_lines(body)
        .iter()
        .any(|(key, _)| msgs.iter().any(|m| is_split_note(m, agent, old_key, key)))
}
