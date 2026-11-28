//! The unattended coordinator (spec/agents#unattended-coordinator): which tickets need the
//! judgment of a coordinator, and how many verdicts the unattended coordinator gave today.
//!
//! Pure. The callers read the board items and the comments; this module decides what they mean.
//!
//! - [`never`] names the tickets that are never on the list (N1 to N7), whatever else holds.
//! - [`need_of`] names the judgment that a ticket needs (L1 to L5), or `None`. No rule holds for
//!   a ticket whose messages the caller did not read: every ticket of the list passed N1 to N4.
//! - [`ticket_verdicts`], [`verdicts_on_day`] and [`used`] count the verdicts from the signed
//!   messages that carry the line `unattended: <n>/<budget>`.
//!
//! - [`check_act`] decides whether the unattended coordinator may run one command on one ticket.
//!
//! A board field is not signed. The Status of the board says which rule to try. The assignee
//! is the agent of the signed log ([`crate::ticket::log::fold_ticket`]): a board `Agent` that
//! the log does not support keeps the ticket off the list, and an empty board `Agent` does not
//! hide a question or a `done` of the signed assignee. The list is a view and gives no right:
//! every command that writes reads the ticket again and decides with [`check_act`].

use crate::authority::CoordMode;
use crate::base::fields::Field;
use crate::headless::{Gate, UAct};
use crate::items::ItemSnapshot;
use crate::protocol::acts::{has_unattended_line, is_local_gate, unattended_of};
use crate::protocol::names::Kind;
use crate::rules::status::{BlockReason, Status};
use crate::ticket::{Act, Sender, VMsg};

/// How many unattended verdicts one ticket may get before a person looks at it.
pub const MAX_VERDICTS_PER_TICKET: usize = 2;

/// How many stamped messages (verdicts and other messages) the unattended coordinator may post
/// in one UTC day. Fixed: no key of AUTHORITY.md changes it.
pub const MAX_MESSAGES_PER_DAY: u32 = 30;

/// How many questions to the owner the unattended coordinator may post in one UTC day. Fixed.
pub const MAX_ASKS_PER_DAY: u32 = 5;

/// The start of a line of `tick` for a ticket on the list.
pub const NEEDS_JUDGMENT: &str = "needs judgment";

/// What the caller knows about one ticket.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    /// The board item: the Status, the Agent, the Gate field and the state of the issue.
    pub item: &'a ItemSnapshot,
    /// The number of the issue (the verify result of a `done` names it).
    pub issue: u64,
    /// The verified messages of the ticket, in comment order. `None`: the caller did not read
    /// them, and a rule that needs them does not put the ticket on the list.
    pub msgs: Option<&'a [VMsg]>,
    /// The body of the issue, when the caller read the issue (an older ticket declares
    /// `CI: none` there and not in the signed assignment).
    pub body: Option<&'a str>,
    /// The issue is closed, as the caller read it. The state of the board item counts too.
    pub closed: bool,
    /// The issue has the label `needs-owner`, when the caller read the labels.
    pub needs_owner_label: bool,
    /// The floor of the unattended verdicts of the ticket that the local state holds
    /// ([`ticket_verdicts_at_least`]): the count, and the highest comment id of the ticket when
    /// it was noted.
    pub verdict_floor: Option<(u32, u64)>,
}

/// Why a ticket is never on the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Never {
    /// N1: a question to the owner is open.
    OwnerQuestion,
    /// N1: the round limit stopped the ticket.
    RoundStop,
    /// N1: the issue has the label `needs-owner`.
    NeedsOwnerLabel,
    /// N2: the latest `done` is on the owner route.
    OwnerRoute,
    /// N3: the ticket has its unattended verdicts ([`MAX_VERDICTS_PER_TICKET`]).
    VerdictCap,
    /// N4: the issue is closed.
    Closed,
    /// The board `Agent` is not the agent of the signed log (a hand edit of the board).
    AgentNotSigned,
    /// N5: in Review without a signed gate.
    NoGate,
    /// N5: in Review without an assignee.
    NoAssignee,
    /// N5: in Review without a `done` that has evidence.
    NoDone,
    /// N6: in Review with local-gate evidence and a Gate that names no checks.
    LocalGate,
    /// N6: in Review and the ticket declares `CI: none`.
    CiNone,
    /// N7: in Review and the latest `done` has a verify result.
    Verified,
}

/// The judgment that a ticket needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Need {
    /// L1: in Review with a Gate in prose, CI evidence and no verify result.
    ProseGate,
    /// L2: the assignee asked the coordinator, and nobody answered.
    Question,
    /// L3: the assignee blocked the ticket, and nobody answered.
    Blocked,
    /// L4: in Ready without an agent, on the board and in the signed log.
    ReadyNoAgent,
    /// L5: a reassign returned the ticket to Backlog.
    Returned,
}

impl Need {
    /// The reason in the line of `tick`.
    pub fn reason(self) -> &'static str {
        match self {
            Need::ProseGate => "prose Gate",
            Need::Question => "question",
            Need::Blocked => "blocked",
            Need::ReadyNoAgent => "ready, no agent",
            Need::Returned => "returned",
        }
    }
}

fn status_of(item: &ItemSnapshot) -> Option<Status> {
    item.get(Field::Status).and_then(Status::parse_board)
}

fn is_coordinator(m: &VMsg) -> bool {
    m.sender.actor().is_coordinator()
}

fn sent_by(m: &VMsg, agent: &str) -> bool {
    matches!(&m.sender, Sender::Agent { name, .. } if name == agent)
}

/// The latest `done` message for N2: of the assignee, or of any agent for a ticket without one.
fn latest_done_message<'a>(msgs: &'a [VMsg], assignee: Option<&str>) -> Option<&'a VMsg> {
    msgs.iter()
        .rev()
        .filter(|m| m.kind == Some(Kind::Done))
        .find(|m| assignee.is_none_or(|a| sent_by(m, a)))
}

/// Does the ticket declare `CI: none`? The signed line of the assignment decides; the body
/// decides for an assignment without the line, when the caller read the body.
fn ci_none(msgs: &[VMsg], body: Option<&str>) -> bool {
    match body {
        Some(b) => crate::ticket::ci_none_declared(msgs, b),
        None => crate::ticket::signed_ci_none(msgs) == Some(true),
    }
}

/// Is the ticket never on the list? The first rule that holds, in the order N4, the board
/// `Agent` against the signed log, N1 (the open question, the stop, then the label), N2, N3, and
/// for a ticket in Review N5, N6, N7. The rules N5 to N7 are about the verdict on a `done`, so they apply in Review
/// only: a ticket that went back to its agent after a failed verify, and is Blocked now, still
/// has the result of that verify.
///
/// `None` without the messages means "not judged", not "passed": [`need_of`] lists no ticket
/// whose messages were not read.
pub fn never(f: &Facts<'_>) -> Option<Never> {
    if let Some(n) = never_in_any_status(f) {
        return Some(n);
    }
    let msgs = f.msgs?;
    if status_of(f.item) != Some(Status::Review) {
        return None;
    }
    let assignee = f.item.get(Field::Agent);
    let Some(gate) = crate::ticket::signed_gate(msgs) else {
        return Some(Never::NoGate);
    };
    let Some(assignee) = assignee else {
        return Some(Never::NoAssignee);
    };
    let Some((done, ev)) = crate::ticket::latest_done(msgs, assignee) else {
        return Some(Never::NoDone);
    };
    if is_local_gate(&ev) && !matches!(Gate::parse(&gate), Gate::Checks(_)) {
        return Some(Never::LocalGate);
    }
    if ci_none(msgs, f.body) {
        return Some(Never::CiNone);
    }
    if crate::ticket::verify_result(msgs, done, f.issue).is_some() {
        return Some(Never::Verified);
    }
    None
}

/// The rules of [`never`] that hold in every Status: N4, the board `Agent` against the signed
/// log, N1 (the facts of the signed log before the label, which is not signed), N2 and N3.
fn never_in_any_status(f: &Facts<'_>) -> Option<Never> {
    if f.closed || f.item.issue_closed() {
        return Some(Never::Closed);
    }
    let log = f.msgs.map(crate::ticket::log::fold_ticket);
    // The signed assignee, also when the board `Agent` is empty: a cleared field does not hide
    // the question or the `done` of the agent that the log names.
    let signed = log.as_ref().and_then(|l| l.agent.as_deref());
    if let (Some(msgs), Some(log)) = (f.msgs, &log) {
        if crate::ticket::log::agent_contradicts(log, f.item.get(Field::Agent)) {
            return Some(Never::AgentNotSigned);
        }
        if owner_question_open(msgs, signed) {
            return Some(Never::OwnerQuestion);
        }
        if crate::ticket::fold_rounds(msgs).stopped {
            return Some(Never::RoundStop);
        }
    }
    if f.needs_owner_label {
        return Some(Never::NeedsOwnerLabel);
    }
    let msgs = f.msgs?;
    if latest_done_message(msgs, signed).is_some_and(crate::ticket::is_owner_route_done) {
        return Some(Never::OwnerRoute);
    }
    if ticket_verdicts_at_least(msgs, f.verdict_floor) >= MAX_VERDICTS_PER_TICKET {
        return Some(Never::VerdictCap);
    }
    None
}

/// Is a question to the owner open (N1)? For a ticket with a signed assignee: the question of
/// that agent or of a coordinator ([`crate::ticket::question_open`]). For a ticket without one
/// (a reassign returned it): also the question of ANY agent that no owner comment follows. The
/// agent that asked can be gone, and its question still waits for the owner; only the owner
/// ends that wait.
fn owner_question_open(msgs: &[VMsg], signed: Option<&str>) -> bool {
    if crate::ticket::question_open(msgs, signed).is_some() {
        return true;
    }
    signed.is_none()
        && msgs
            .iter()
            .rposition(|m| {
                m.kind == Some(Kind::Question)
                    && m.to == "owner"
                    && matches!(m.sender, Sender::Agent { .. })
            })
            .is_some_and(|at| !msgs[at + 1..].iter().any(|m| m.sender == Sender::Owner))
}

/// Does the signed log hold an assignment of a coordinator with a `gate:` line? Then a new
/// assignment copies the signed Gate, CI line and guard, and reads nothing from the issue body.
/// The body is not signed: a ticket without such an assignment is for a person, who reads the
/// body before the first assignment signs it.
pub fn has_signed_assignment(msgs: &[VMsg]) -> bool {
    crate::ticket::has_assignment(msgs) && crate::ticket::signed_gate(msgs).is_some()
}

/// The judgment that the ticket needs, or `None`. [`never`] is checked first, and no rule holds
/// without the messages. The first rule that holds, in the order L1 to L5. The assignee of L1 to
/// L3 is the board `Agent`, which [`never`] checked against the signed log.
pub fn need_of(f: &Facts<'_>) -> Option<Need> {
    if never(f).is_some() {
        return None;
    }
    let msgs = f.msgs?;
    let assignee = f.item.get(Field::Agent);
    match status_of(f.item)? {
        Status::Review => {
            if prose_gate(msgs) {
                return Some(Need::ProseGate);
            }
            open_question(msgs, assignee?).then_some(Need::Question)
        }
        Status::Blocked => {
            let assignee = assignee?;
            if open_question(msgs, assignee) {
                return Some(Need::Question);
            }
            plain_block_open(msgs, assignee).then_some(Need::Blocked)
        }
        // No agent on the board and no live assignment in the signed log: an `assign` of such a
        // ticket takes it from nobody. The signed log must allow Ready (a reassign, or a move of
        // the owner): a Status that somebody set on the board alone makes no ticket assignable.
        // And the log must hold an earlier assignment with a Gate, so the new one copies signed
        // lines: a ticket that was never assigned is for a person.
        Status::Ready => {
            let log = crate::ticket::log::fold_ticket(msgs);
            (assignee.is_none()
                && log.agent.is_none()
                && has_signed_assignment(msgs)
                && !crate::ticket::log::status_contradicts(&log, f.item.get(Field::Status)))
            .then_some(Need::ReadyNoAgent)
        }
        Status::Backlog => {
            (is_backlog_candidate(f.item) && returned(msgs) && has_signed_assignment(msgs))
                .then_some(Need::Returned)
        }
        Status::InProgress | Status::Done | Status::Dropped => None,
    }
}

/// The filter for L4 that needs the board fields only: in Ready, no Agent, the issue not
/// closed. The caller reads the messages of such a ticket.
pub fn is_ready_candidate(item: &ItemSnapshot) -> bool {
    status_of(item) == Some(Status::Ready)
        && item.get(Field::Agent).is_none()
        && !item.issue_closed()
}

/// The filter for L5 that needs the board fields only: in Backlog, no Agent, the Gate field set
/// and the issue not closed. The caller reads the messages of such a ticket and of no other
/// Backlog ticket.
pub fn is_backlog_candidate(item: &ItemSnapshot) -> bool {
    status_of(item) == Some(Status::Backlog)
        && item.get(Field::Agent).is_none()
        && item.get(Field::Gate).is_some()
        && !item.issue_closed()
}

/// L1, for a ticket in Review that [`never`] let pass (it has a signed gate, a `done` with
/// evidence and no verify result): the signed Gate is prose. A local gate with such a Gate is N6,
/// so the evidence is CI evidence here.
fn prose_gate(msgs: &[VMsg]) -> bool {
    crate::ticket::signed_gate(msgs).is_some_and(|g| matches!(Gate::parse(&g), Gate::Text(_)))
}

/// Is `m` addressed to the coordinator: to the role, or to the name of an agent that signed a
/// message of this log as a coordinator?
fn to_coordinator(m: &VMsg, msgs: &[VMsg]) -> bool {
    m.to == "coordinator" || msgs.iter().any(|x| is_coordinator(x) && sent_by(x, &m.to))
}

/// L2: the latest `question` of the assignee to the coordinator has no coordinator `answer` and
/// no owner comment after it.
fn open_question(msgs: &[VMsg], assignee: &str) -> bool {
    let Some(at) = msgs.iter().rposition(|m| {
        m.kind == Some(Kind::Question) && sent_by(m, assignee) && to_coordinator(m, msgs)
    }) else {
        return false;
    };
    !msgs[at + 1..]
        .iter()
        .any(|m| m.sender == Sender::Owner || (is_coordinator(m) && m.kind == Some(Kind::Answer)))
}

/// L3: the latest block is a plain `blocked` of the assignee (no park, no question to the owner),
/// and no answer came after it.
fn plain_block_open(msgs: &[VMsg], assignee: &str) -> bool {
    let Some(last) = msgs.iter().rev().find(|m| m.kind == Some(Kind::Blocked)) else {
        return false;
    };
    sent_by(last, assignee)
        && crate::ticket::block_reason_of(last) == BlockReason::Plain
        && !crate::ticket::blocked_by_owner_question(msgs, Some(assignee))
        && !crate::ticket::answer_after_block(msgs, Status::Blocked, Some(assignee))
}

/// L5: the last assignment or reassign of a coordinator in the log is a reassign.
pub fn returned(msgs: &[VMsg]) -> bool {
    msgs.iter()
        .rev()
        .filter(|m| is_coordinator(m))
        .find_map(|m| match m.act() {
            Act::Reassigned => Some(true),
            Act::Assignment { .. } => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

/// The unattended verdicts that count on one ticket (N3): the verify results of a coordinator
/// with an `unattended:` line. The count starts again after an owner comment and after an
/// assignment of a coordinator that has no `unattended:` line: a person or an attended
/// coordinator looked at the ticket. A reassign never starts it again, because the duty for a
/// dead agent posts one with nobody watching. A line that is not valid counts as a line: it
/// cannot lower the count.
pub fn ticket_verdicts(msgs: &[VMsg]) -> usize {
    let mut n = 0;
    for m in msgs {
        if starts_verdicts_again(m) {
            n = 0;
        } else if is_coordinator(m)
            && matches!(m.act(), Act::Verify { .. })
            && has_unattended_line(&m.body)
        {
            n += 1;
        }
    }
    n
}

/// Does `m` start the count of [`ticket_verdicts`] again: an owner comment, or an assignment of
/// a coordinator without an `unattended:` line?
fn starts_verdicts_again(m: &VMsg) -> bool {
    m.sender == Sender::Owner
        || (is_coordinator(m)
            && matches!(m.act(), Act::Assignment { .. })
            && !has_unattended_line(&m.body))
}

/// [`ticket_verdicts`], never below the floor of the local state. `floor` is `(count, after)`:
/// a command noted `count` before it posted a verdict, when the highest comment id of the
/// ticket was `after`. A stamped verdict that somebody deleted or edited later is gone from the
/// messages and still in the floor. The floor ends when a message with a higher id starts the
/// count again; no agent token can write such a message.
pub fn ticket_verdicts_at_least(msgs: &[VMsg], floor: Option<(u32, u64)>) -> usize {
    let counted = ticket_verdicts(msgs);
    match floor {
        Some((count, after))
            if !msgs
                .iter()
                .any(|m| m.comment_id > after && starts_verdicts_again(m)) =>
        {
            counted.max(count as usize)
        }
        _ => counted,
    }
}

/// Why the unattended coordinator must not run a command on a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// N3: the ticket has its unattended verdicts (refusal `unattended-budget`).
    Cap,
    /// Any other reason, as a clause for the refusal `headless-scope`.
    Scope(String),
}

fn never_clause(key: &str, n: Never) -> Block {
    Block::Scope(match n {
        Never::VerdictCap => return Block::Cap,
        Never::Closed => format!("{key} is closed"),
        Never::NeedsOwnerLabel => {
            format!("{key} has the label `needs-owner`: the owner decides")
        }
        Never::AgentNotSigned => {
            format!("the board Agent of {key} is not the agent of the signed log")
        }
        Never::OwnerQuestion => format!("{key} has an open question to the owner"),
        Never::RoundStop => format!("{key} is stopped at the round limit: the owner decides"),
        Never::OwnerRoute => {
            format!("the latest done of {key} is on the owner route: the owner merges it")
        }
        Never::NoGate => format!("{key} has no signed gate line"),
        Never::NoAssignee => format!("{key} has no assignee"),
        Never::NoDone => format!("{key} has no signed done with evidence"),
        Never::LocalGate => {
            format!("the evidence of {key} is a local gate, and its Gate names no checks")
        }
        Never::CiNone => format!("{key} declares `CI: none`"),
        Never::Verified => format!("the latest done of {key} has a verify result"),
    })
}

/// May the unattended coordinator run `act` on the ticket `key`? The decision comes from the
/// facts that the command just read, never from the list of an earlier tick: the rules N1 to N4
/// for every act, the board `Agent` and the board Status against the signed log, and the rule of
/// the act. `to` is the agent that an answer names.
///
/// - A verdict: the ticket is L1 (and so passed N5 to N7).
/// - An answer: `to` is the signed assignee.
/// - `unblock`: the ticket is L2 or L3 in Blocked.
/// - `assign`: the ticket is L4 or L5, a coordinator announced its board item in a signed
///   message, the signed log has no assignee, and it has an earlier assignment with a Gate
///   ([`has_signed_assignment`]).
/// - `ask`: N4, and no question to the owner is open on the ticket already.
pub fn check_act(act: UAct, key: &str, f: &Facts<'_>, to: Option<&str>) -> Result<(), Block> {
    let scope = |text: String| Err(Block::Scope(text));
    let Some(msgs) = f.msgs else {
        return scope(format!("the messages of {key} were not read"));
    };
    if act == UAct::Ask {
        if f.closed || f.item.issue_closed() {
            return Err(never_clause(key, Never::Closed));
        }
        let signed = crate::ticket::log::fold_ticket(msgs).agent;
        if owner_question_open(msgs, signed.as_deref()) {
            return scope(format!(
                "{key} has an open question to the owner already: one question waits for one answer"
            ));
        }
        return Ok(());
    }
    let blocked_by = match act {
        UAct::Verdict => never(f),
        _ => never_in_any_status(f),
    };
    if let Some(n) = blocked_by {
        return Err(never_clause(key, n));
    }
    let log = crate::ticket::log::fold_ticket(msgs);
    let board_status = f.item.get(Field::Status);
    if crate::ticket::log::status_contradicts(&log, board_status) {
        return scope(format!(
            "the board Status of {key} is {}, and the signed log says {}",
            board_status.unwrap_or("empty"),
            log.status.describe()
        ));
    }
    let need = need_of(f);
    match act {
        UAct::Verdict if need == Some(Need::ProseGate) => Ok(()),
        UAct::Verdict => scope(format!(
            "{key} is not a ticket in Review with a Gate in prose, CI evidence and no verify result"
        )),
        UAct::Answer => match (log.agent.as_deref(), to) {
            (Some(a), Some(t)) if a == t => Ok(()),
            (Some(a), _) => scope(format!(
                "the signed log assigns {key} to {a}: an answer goes to the assignee only"
            )),
            (None, _) => scope(format!("{key} has no assignee in the signed log")),
        },
        UAct::Unblock
            if status_of(f.item) == Some(Status::Blocked)
                && matches!(need, Some(Need::Blocked | Need::Question)) =>
        {
            Ok(())
        }
        UAct::Unblock => scope(format!(
            "{key} is not a ticket that its assignee blocked and that waits for an answer of the coordinator"
        )),
        UAct::Assign => {
            if log.agent.is_some() {
                return scope(format!("{key} has an assignee in the signed log"));
            }
            if crate::ticket::find_item_id(msgs).is_none() {
                return scope(format!(
                    "no signed message of a coordinator puts {key} on the board"
                ));
            }
            if !has_signed_assignment(msgs) {
                return scope(format!(
                    "the signed log of {key} has no assignment with a Gate to copy: the first assignment signs the Gate and the guard from the issue body, which a person reads first"
                ));
            }
            match need {
                Some(Need::ReadyNoAgent | Need::Returned) => Ok(()),
                _ => scope(format!(
                    "{key} is not a Ready ticket without an agent, and no reassign returned it to Backlog"
                )),
            }
        }
        UAct::Ask => Ok(()),
    }
}

/// What the messages of one UTC day say about the verdict count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCount {
    /// The verify results of a coordinator with an `unattended:` line.
    pub verdicts: u32,
    /// The highest `n` of a valid `unattended: <n>/<budget>` line of a coordinator message.
    pub highest: u32,
    /// The coordinator messages with an `unattended:` line (verdicts and other messages).
    pub messages: u32,
    /// The questions of a coordinator to the owner with an `unattended:` line.
    pub asks: u32,
}

impl DayCount {
    /// Both counts over two sets of messages (two issues, two repos).
    pub fn with(self, other: DayCount) -> DayCount {
        DayCount {
            verdicts: self.verdicts.saturating_add(other.verdicts),
            highest: self.highest.max(other.highest),
            messages: self.messages.saturating_add(other.messages),
            asks: self.asks.saturating_add(other.asks),
        }
    }

    /// This count, never below `seen`: what an earlier read of the same messages on the same
    /// day gave. A comment that was deleted or edited since then does not lower the count.
    pub fn at_least(self, seen: DayCount) -> DayCount {
        DayCount {
            verdicts: self.verdicts.max(seen.verdicts),
            highest: self.highest.max(seen.highest),
            messages: self.messages.max(seen.messages),
            asks: self.asks.max(seen.asks),
        }
    }
}

/// The start of the UTC day of `now` (Unix seconds).
pub fn day_start(now: i64) -> i64 {
    now - now.rem_euclid(86_400)
}

/// Count the verified messages of coordinators that GitHub created at or after `day_start`.
/// Only a signed message of a trusted coordinator is in `msgs` with that role, so no other
/// account can raise or lower the count.
pub fn verdicts_on_day(msgs: &[VMsg], day_start: i64) -> DayCount {
    let mut c = DayCount::default();
    for m in msgs
        .iter()
        .filter(|m| is_coordinator(m) && m.created >= day_start)
    {
        if !has_unattended_line(&m.body) {
            continue;
        }
        c.messages = c.messages.saturating_add(1);
        if m.kind == Some(Kind::Question) && m.to == "owner" {
            c.asks = c.asks.saturating_add(1);
        }
        if matches!(m.act(), Act::Verify { .. }) {
            c.verdicts = c.verdicts.saturating_add(1);
        }
        if let Some((n, _)) = unattended_of(&m.body) {
            c.highest = c.highest.max(n);
        }
    }
    c
}

/// The floor that the local state holds for `today` (`YYYY-MM-DD`): a count that a command
/// notes before it posts a verdict. A floor of another day is 0.
pub fn floor_on(stored: Option<&(String, u32)>, today: &str) -> u32 {
    stored
        .filter(|(day, _)| day == today)
        .map_or(0, |(_, n)| *n)
}

/// The verdicts that are used today: the count of the signed messages, never below the two
/// floors. A deleted comment lowers the count of the messages, but not the highest `n` of a
/// message that stayed, and not the floor of the local state.
pub fn used(day: DayCount, floor: u32) -> u32 {
    day.verdicts.max(day.highest).max(floor)
}

/// The stamped messages that are used today: the count of the signed messages, never below the
/// floor of the local state and never below the verdicts.
pub fn messages_used(day: DayCount, floor: u32) -> u32 {
    day.messages.max(day.verdicts).max(floor)
}

/// The line of `tick` for one ticket on the list.
pub fn need_line(key: &str, need: Need) -> String {
    format!("{NEEDS_JUDGMENT}: {key}, {}\n", need.reason())
}

/// The start of the lines of `tick` about the unattended coordinator. It is not the key of the
/// protocol line (`unattended:`), so a message that quotes a tick line is not a stamped message.
pub const TICK_LINE: &str = "unattended coordinator";

/// The line of `tick` with the mode and the verdicts of the day.
pub fn count_line(mode: CoordMode, used: u32, budget: u32) -> String {
    format!(
        "{TICK_LINE}: mode {}; verdicts {used}/{budget} today\n",
        mode.as_str()
    )
}

/// The line of `tick` for a read of the list that failed: the ticket (or the scan) is not
/// judged in this tick.
pub fn unread_line(what: &str, why: &str) -> String {
    format!(
        "{TICK_LINE}: {what} could not be read for the list: {}\n",
        why.lines().next().unwrap_or("").trim()
    )
}

/// The line of `tick` when the verdicts of the day could not be read. The tick then lists no
/// ticket: nobody can tell whether a verdict is left.
pub fn count_unread_line(mode: CoordMode, why: &str) -> String {
    format!(
        "{TICK_LINE}: mode {}; the verdicts of today could not be read, so no ticket is listed: {}\n",
        mode.as_str(),
        why.lines().next().unwrap_or("").trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::acts::{
        park_body, reassigned_body, round_limit_question, unattended_line, withdraw_body,
    };
    use crate::protocol::names::Re;
    use crate::rules::Role;
    use crate::testkit::{agent, msg};

    const COORD: &str = "agent-a@host-a";
    const DEV: &str = "agent-b@host-b";
    const OTHER: &str = "agent-c@host-c";
    const ISSUE: u64 = 5;
    const PROSE: &str = "the demo runs";

    fn item(status: &str, agent: Option<&str>) -> ItemSnapshot {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("Status".to_string(), status.to_string());
        fields.insert("Gate".to_string(), "from the body".to_string());
        if let Some(a) = agent {
            fields.insert("Agent".to_string(), a.to_string());
        }
        ItemSnapshot {
            id: "PVTI_5".into(),
            updated_at: "2026-11-28T08:00:00Z".into(),
            item_type: "ISSUE".into(),
            project_number: Some(15),
            issue_state: Some("OPEN".into()),
            fields,
        }
    }

    fn facts<'a>(item: &'a ItemSnapshot, msgs: &'a [VMsg]) -> Facts<'a> {
        Facts {
            item,
            issue: ISSUE,
            msgs: Some(msgs),
            body: None,
            closed: false,
            needs_owner_label: false,
            verdict_floor: None,
        }
    }

    fn coord(id: u64, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(id, agent(COORD, Role::Coordinator), to, Some(kind), body)
    }

    fn dev(id: u64, to: &str, kind: Kind, body: &str) -> VMsg {
        msg(id, agent(DEV, Role::Dev), to, Some(kind), body)
    }

    fn owner(id: u64, body: &str) -> VMsg {
        msg(id, Sender::Owner, "coordinator", None, body)
    }

    fn assignment(id: u64, gate: &str) -> VMsg {
        coord(
            id,
            DEV,
            Kind::Request,
            &format!("Assigned to you: Fix it\nticket: a/b#5\nitem=PVTI_5\ngate: {gate}\n"),
        )
    }

    fn done(id: u64, extra: &str) -> VMsg {
        dev(
            id,
            COORD,
            Kind::Done,
            &format!(
                "Done. Evidence follows.\npr: https://github.com/a/b/pull/7\nsha: abc\nci: success\n{extra}"
            ),
        )
    }

    /// A verify result of the coordinator for the `done` in comment `done`.
    fn verdict(id: u64, done: u64, pass: bool, extra: &str) -> VMsg {
        VMsg {
            re: Re::Comment {
                issue: ISSUE,
                comment: done,
            },
            ..coord(
                id,
                DEV,
                Kind::Report,
                &format!(
                    "verify: {}\nsha: abc\n{extra}",
                    if pass { "pass" } else { "fail" }
                ),
            )
        }
    }

    fn stamp(n: u32) -> String {
        format!("{}\n", unattended_line(n, 10))
    }

    /// A ticket in Review with a prose Gate, CI evidence and no verify result: L1.
    fn review_log() -> Vec<VMsg> {
        vec![assignment(1, PROSE), done(2, "")]
    }

    /// A ticket that its agent blocked with a plain `blocked`: L3.
    fn blocked_log() -> Vec<VMsg> {
        vec![
            assignment(1, PROSE),
            dev(2, "coordinator", Kind::Blocked, "the API key is missing"),
        ]
    }

    fn need(status: &str, agent: Option<&str>, msgs: &[VMsg]) -> Option<Need> {
        need_of(&facts(&item(status, agent), msgs))
    }

    fn never_in(status: &str, msgs: &[VMsg]) -> Option<Never> {
        never(&facts(&item(status, Some(DEV)), msgs))
    }

    #[test]
    fn l1_a_review_ticket_with_a_prose_gate_and_ci_evidence_is_on_the_list() {
        assert_eq!(
            need("Review", Some(DEV), &review_log()),
            Some(Need::ProseGate)
        );
        // a Gate of two body lines is prose too
        let two = vec![assignment(1, "ci | the demo runs"), done(2, "")];
        assert_eq!(need("Review", Some(DEV), &two), Some(Need::ProseGate));
        // not L1: a typed Gate (the tick handles it), and the same log in another Status
        for typed in ["ci", "CI", "checks: tests, lint"] {
            let log = vec![assignment(1, typed), done(2, "")];
            assert_eq!(need("Review", Some(DEV), &log), None, "{typed}");
        }
        for status in ["In Progress", "Done", "Dropped"] {
            assert_eq!(need(status, Some(DEV), &review_log()), None, "{status}");
        }
        // a rule that needs the messages does not hold without them
        let it = item("Review", Some(DEV));
        let unread = Facts {
            msgs: None,
            ..facts(&it, &[])
        };
        assert_eq!(need_of(&unread), None);
    }

    #[test]
    fn l2_a_question_of_the_assignee_to_the_coordinator_without_an_answer() {
        let ask = |to: &str| {
            vec![
                assignment(1, "ci"),
                done(2, ""),
                dev(3, to, Kind::Question, "which port?"),
            ]
        };
        for status in ["Review", "Blocked"] {
            assert_eq!(
                need(status, Some(DEV), &ask("coordinator")),
                Some(Need::Question),
                "{status}"
            );
            // by name: the coordinator signed the assignment of this log
            assert_eq!(
                need(status, Some(DEV), &ask(COORD)),
                Some(Need::Question),
                "{status}"
            );
        }
        // Review and Blocked only
        for status in ["In Progress", "Ready", "Backlog"] {
            assert_eq!(
                need(status, Some(DEV), &ask("coordinator")),
                None,
                "{status}"
            );
        }
        // not L2: a question to another dev, a question of another agent, no assignee
        assert_eq!(need("Blocked", Some(DEV), &ask(OTHER)), None);
        let foreign = vec![
            assignment(1, "ci"),
            msg(
                2,
                agent(OTHER, Role::Dev),
                "coordinator",
                Some(Kind::Question),
                "which port?",
            ),
        ];
        assert_eq!(need("Blocked", Some(DEV), &foreign), None);
        assert_eq!(need("Blocked", None, &ask("coordinator")), None);
        // answered: a coordinator answer, or an owner comment
        let mut answered = ask("coordinator");
        answered.push(coord(3, DEV, Kind::Answer, "8080"));
        assert_eq!(need("Blocked", Some(DEV), &answered), None);
        let mut by_owner = ask("coordinator");
        by_owner.push(owner(3, "8080"));
        assert_eq!(need("Blocked", Some(DEV), &by_owner), None);
        // not answered: an info of the coordinator, an answer of another dev, and a newer
        // question after an answer
        let mut info = ask("coordinator");
        info.push(coord(3, DEV, Kind::Info, "noted"));
        info.push(msg(
            4,
            agent(OTHER, Role::Dev),
            DEV,
            Some(Kind::Answer),
            "try 80",
        ));
        assert_eq!(need("Blocked", Some(DEV), &info), Some(Need::Question));
        answered.push(dev(4, "coordinator", Kind::Question, "and the host?"));
        assert_eq!(need("Blocked", Some(DEV), &answered), Some(Need::Question));
    }

    #[test]
    fn l3_a_plain_block_of_the_assignee_without_an_answer() {
        assert_eq!(
            need("Blocked", Some(DEV), &blocked_log()),
            Some(Need::Blocked)
        );
        // the legacy board option reads as Blocked
        assert_eq!(
            need("Needs Human", Some(DEV), &blocked_log()),
            Some(Need::Blocked)
        );
        // not L3: answered
        for answer in [
            coord(3, DEV, Kind::Answer, "use the test key"),
            owner(3, "go on"),
        ] {
            let mut log = blocked_log();
            log.push(answer);
            assert_eq!(need("Blocked", Some(DEV), &log), None);
        }
        // an info of the coordinator is no answer
        let mut log = blocked_log();
        log.push(coord(3, DEV, Kind::Info, "noted"));
        assert_eq!(need("Blocked", Some(DEV), &log), Some(Need::Blocked));
        // not L3: a park, a block of another agent, no block message, no assignee
        let parked = vec![
            assignment(1, PROSE),
            dev(2, "coordinator", Kind::Blocked, &park_body(9, "P0 first")),
        ];
        assert_eq!(need("Blocked", Some(DEV), &parked), None);
        let foreign = vec![
            assignment(1, PROSE),
            msg(
                2,
                agent(OTHER, Role::Dev),
                "coordinator",
                Some(Kind::Blocked),
                "stuck",
            ),
        ];
        assert_eq!(need("Blocked", Some(DEV), &foreign), None);
        assert_eq!(need("Blocked", Some(DEV), &[assignment(1, PROSE)]), None);
        assert_eq!(need("Blocked", None, &blocked_log()), None);
        // not L3: the latest block is a question to the owner that the owner answered
        let mut asked = blocked_log();
        asked.push(dev(3, "owner", Kind::Question, "may I delete it?"));
        asked.push(owner(4, "yes"));
        assert_eq!(need("Blocked", Some(DEV), &asked), None);
        // a Blocked ticket with an old failed verify is still on the list (N7 is for Review)
        let rework = vec![
            assignment(1, PROSE),
            done(2, ""),
            verdict(3, 2, false, ""),
            dev(4, "coordinator", Kind::Blocked, "the fix needs a key"),
        ];
        assert_eq!(need("Blocked", Some(DEV), &rework), Some(Need::Blocked));
    }

    #[test]
    fn l4_a_ready_ticket_without_an_agent_on_the_board_and_in_the_signed_log() {
        // returned by a reassign: the log has the assignment whose signed Gate the next one
        // copies
        let back = vec![
            assignment(1, PROSE),
            coord(2, DEV, Kind::Info, &reassigned_body("the agent is dead")),
        ];
        assert_eq!(need("Ready", None, &back), Some(Need::ReadyNoAgent));
        assert!(is_ready_candidate(&item("Ready", None)));
        // not L4: a ticket that was never assigned, also after a move of the owner. Its first
        // assignment signs the Gate from the body, and a person reads the body first
        assert_eq!(need("Ready", None, &[]), None);
        assert_eq!(need("Ready", None, &[owner(1, "/crew move Ready")]), None);
        // not L4: the assignment of the returned ticket is gone (somebody deleted the comment),
        // or it has no `gate:` line
        assert_eq!(need("Ready", None, &back[1..]), None);
        let no_gate = vec![
            coord(1, DEV, Kind::Request, "Assigned to you: Fix it\n"),
            back[1].clone(),
        ];
        assert_eq!(need("Ready", None, &no_gate), None);
        assert!(!has_signed_assignment(&no_gate) && has_signed_assignment(&back));
        // the board fields alone do not put a ticket on the list
        let it = item("Ready", None);
        let board_only = Facts {
            msgs: None,
            ..facts(&it, &[])
        };
        assert_eq!(need_of(&board_only), None);
        // not L4: the signed log has a live assignment (somebody cleared the board Agent)
        assert_eq!(need("Ready", None, &[assignment(1, PROSE)]), None);
        // not L4: an agent, another Status, a closed issue
        assert_eq!(need("Ready", Some(DEV), &[assignment(1, PROSE)]), None);
        assert!(!is_ready_candidate(&item("Ready", Some(DEV))));
        assert_eq!(need("In Progress", None, &[]), None);
        let mut closed = item("Ready", None);
        closed.issue_state = Some("CLOSED".into());
        assert!(!is_ready_candidate(&closed));
        assert_eq!(need_of(&facts(&closed, &[])), None);
    }

    #[test]
    fn a_ready_ticket_without_an_agent_passes_n1_to_n3_first() {
        // Somebody set a stopped ticket to Ready and cleared its Agent on the board. The signed
        // log still has the stop, the open question and the assignee.
        let ready = |msgs: &[VMsg]| never(&facts(&item("Ready", None), msgs));
        let mut asked = blocked_log();
        asked.push(dev(3, "owner", Kind::Question, "may I delete it?"));
        assert_eq!(ready(&asked), Some(Never::OwnerQuestion));
        assert_eq!(need("Ready", None, &asked), None);
        // the same after a reassign (the agent that asked is dead, and the duty returned the
        // ticket): the question still waits for the owner, whoever asked it
        asked.push(coord(4, DEV, Kind::Info, &reassigned_body("late")));
        assert_eq!(ready(&asked), Some(Never::OwnerQuestion));
        assert_eq!(need("Ready", None, &asked), None);
        assert_eq!(need("Backlog", None, &asked), None);
        // an answer of a coordinator that withdraws does not end that wait; the owner does
        let mut withdrawn = asked.clone();
        withdrawn.push(coord(5, DEV, Kind::Answer, &withdraw_body("solved")));
        assert_eq!(ready(&withdrawn), Some(Never::OwnerQuestion));
        let mut answered = asked.clone();
        answered.push(owner(5, "yes, delete it"));
        assert_eq!(need("Ready", None, &answered), Some(Need::ReadyNoAgent));
        assert_eq!(need("Backlog", None, &answered), Some(Need::Returned));
        // a question of an agent that never was the assignee counts too on a returned ticket
        let mut stranger = blocked_log();
        stranger.push(coord(3, DEV, Kind::Info, &reassigned_body("late")));
        assert_eq!(need("Backlog", None, &stranger), Some(Need::Returned));
        stranger.push(msg(
            4,
            agent(OTHER, Role::Dev),
            "owner",
            Some(Kind::Question),
            "may I?",
        ));
        assert_eq!(need("Backlog", None, &stranger), None);
        // a question of a coordinator to the owner counts whoever the assignee is
        answered.push(coord(6, "owner", Kind::Question, "drop it?"));
        assert_eq!(ready(&answered), Some(Never::OwnerQuestion));

        let mut stopped = review_log();
        stopped.push(verdict(
            3,
            2,
            false,
            "case: round limit 2 of 2; the owner decides\n",
        ));
        assert_eq!(ready(&stopped), Some(Never::RoundStop));
        assert_eq!(need("Ready", None, &stopped), None);

        let route = vec![
            assignment(1, PROSE),
            done(2, "protected-paths: AUTHORITY.md\n"),
        ];
        assert_eq!(ready(&route), Some(Never::OwnerRoute));

        let mut capped = vec![assignment(1, PROSE), done(2, "")];
        capped.push(verdict(3, 2, false, &stamp(1)));
        capped.push(verdict(4, 2, false, &stamp(2)));
        capped.push(coord(5, DEV, Kind::Info, &reassigned_body("late")));
        assert_eq!(ready(&capped), Some(Never::VerdictCap));
        assert_eq!(need("Ready", None, &capped), None);
    }

    #[test]
    fn a_board_agent_that_the_signed_log_does_not_name_keeps_a_ticket_off_the_list() {
        // The ticket is assigned to DEV, and DEV has an open question to the owner. Another dev
        // agent writes its own name into the board Agent and posts a signed `blocked`.
        let other =
            |id, to: &str, kind, body: &str| msg(id, agent(OTHER, Role::Dev), to, Some(kind), body);
        let mut log = blocked_log();
        log.push(dev(3, "owner", Kind::Question, "may I delete it?"));
        log.push(other(4, "coordinator", Kind::Blocked, "stuck"));
        let it = item("Blocked", Some(OTHER));
        assert_eq!(never(&facts(&it, &log)), Some(Never::AgentNotSigned));
        assert_eq!(need_of(&facts(&it, &log)), None);
        // with the signed assignee on the board, the question of DEV keeps it off the list
        assert_eq!(never_in("Blocked", &log), Some(Never::OwnerQuestion));
        // and so it does with an empty board Agent
        assert_eq!(
            never(&facts(&item("Blocked", None), &log)),
            Some(Never::OwnerQuestion)
        );
        // the same in Review, with a `done` of the other agent
        let mut log = vec![
            assignment(1, PROSE),
            done(2, "protected-paths: AUTHORITY.md\n"),
        ];
        log.push(other(
            3,
            COORD,
            Kind::Done,
            "Done. Evidence follows.\npr: https://github.com/a/b/pull/8\nsha: def\nci: success\n",
        ));
        let it = item("Review", Some(OTHER));
        assert_eq!(never(&facts(&it, &log)), Some(Never::AgentNotSigned));
        assert_eq!(need_of(&facts(&it, &log)), None);
        assert_eq!(never_in("Review", &log), Some(Never::OwnerRoute));
        // a board Agent on a ticket whose log has no assignment at all
        let it = item("Blocked", Some(DEV));
        assert_eq!(never(&facts(&it, &[])), Some(Never::AgentNotSigned));
        // control: the agent that the log names passes
        assert_eq!(never_in("Blocked", &blocked_log()), None);
    }

    #[test]
    fn l5_a_ticket_that_a_reassign_returned_to_backlog() {
        let returned = vec![
            assignment(1, PROSE),
            coord(2, DEV, Kind::Info, &reassigned_body("the agent is dead")),
        ];
        assert_eq!(need("Backlog", None, &returned), Some(Need::Returned));
        // not L5: never assigned, or assigned again after the reassign
        assert_eq!(need("Backlog", None, &[]), None);
        assert_eq!(need("Backlog", None, &[assignment(1, PROSE)]), None);
        let mut again = returned.clone();
        again.push(assignment(3, PROSE));
        assert_eq!(need("Backlog", None, &again), None);
        // a reassign text of a dev agent is no reassign
        let forged = vec![
            assignment(1, PROSE),
            dev(2, DEV, Kind::Info, &reassigned_body("the agent is dead")),
        ];
        assert_eq!(need("Backlog", None, &forged), None);
        // not L5: an agent, no Gate field, a closed issue, messages not read
        assert_eq!(need("Backlog", Some(DEV), &returned), None);
        let mut no_gate = item("Backlog", None);
        no_gate.fields.remove("Gate");
        assert!(!is_backlog_candidate(&no_gate));
        assert_eq!(need_of(&facts(&no_gate, &returned)), None);
        let mut closed = item("Backlog", None);
        closed.issue_state = Some("CLOSED".into());
        assert!(!is_backlog_candidate(&closed));
        let it = item("Backlog", None);
        assert!(is_backlog_candidate(&it));
        let unread = Facts {
            msgs: None,
            ..facts(&it, &[])
        };
        assert_eq!(need_of(&unread), None);
    }

    #[test]
    fn n1_an_open_owner_question_a_round_stop_or_the_label_keeps_a_ticket_off_the_list() {
        // control: the two logs are on the list
        assert_eq!(never_in("Review", &review_log()), None);
        assert_eq!(never_in("Blocked", &blocked_log()), None);
        for (status, base) in [("Review", review_log()), ("Blocked", blocked_log())] {
            let mut asked = base.clone();
            asked.push(dev(8, "owner", Kind::Question, "may I delete it?"));
            assert_eq!(never_in(status, &asked), Some(Never::OwnerQuestion));
            assert_eq!(need(status, Some(DEV), &asked), None);
            // the owner answered: the question is closed
            asked.push(owner(9, "yes"));
            assert_eq!(never_in(status, &asked), None);
            // a withdrawn question is closed too
            let mut withdrawn = base.clone();
            withdrawn.push(coord(8, "owner", Kind::Question, "may it go?"));
            assert_eq!(never_in(status, &withdrawn), Some(Never::OwnerQuestion));
            withdrawn.push(coord(9, DEV, Kind::Answer, &withdraw_body("solved")));
            assert_eq!(never_in(status, &withdrawn), None);

            // the stop alone: the verify result at the limit, and no question to the owner
            let mut stopped = base.clone();
            stopped.push(verdict(
                8,
                2,
                false,
                "case: round limit 2 of 2; the owner decides\n",
            ));
            assert!(crate::ticket::question_open(&stopped, Some(DEV)).is_none());
            assert_eq!(never_in(status, &stopped), Some(Never::RoundStop));
            assert_eq!(need(status, Some(DEV), &stopped), None);
            // the question of the stop is a question to the owner too
            stopped.push(coord(
                9,
                "owner",
                Kind::Question,
                &round_limit_question(2, 2, "https://example.invalid/r"),
            ));
            assert_eq!(never_in(status, &stopped), Some(Never::OwnerQuestion));
            // an owner comment ends the stop and closes the question
            stopped.push(owner(10, "go on"));
            assert!(
                !matches!(
                    never_in(status, &stopped),
                    Some(Never::RoundStop | Never::OwnerQuestion)
                ),
                "{status}"
            );

            let it = item(status, Some(DEV));
            let labelled = Facts {
                needs_owner_label: true,
                ..facts(&it, &base)
            };
            assert_eq!(never(&labelled), Some(Never::NeedsOwnerLabel));
            assert_eq!(need_of(&labelled), None);
        }
    }

    #[test]
    fn n2_a_done_on_the_owner_route_keeps_a_ticket_off_the_list() {
        for line in [
            "protected-paths: .github/workflows/ci.yml\n",
            "owner-route: fix.patch\n",
        ] {
            let log = vec![assignment(1, PROSE), done(2, line)];
            assert_eq!(never_in("Review", &log), Some(Never::OwnerRoute), "{line}");
            assert_eq!(need("Review", Some(DEV), &log), None);
            // a later done without the line is the latest done
            let mut later = log.clone();
            later.push(done(3, ""));
            assert_eq!(never_in("Review", &later), None, "{line}");
            // the done of another agent does not count for the assignee
            let foreign = vec![
                assignment(1, PROSE),
                done(2, ""),
                msg(
                    3,
                    agent(OTHER, Role::Dev),
                    COORD,
                    Some(Kind::Done),
                    &format!("Done.\n{line}"),
                ),
            ];
            assert_eq!(never_in("Review", &foreign), None, "{line}");
        }
        // a ticket without an agent: the latest done of the log counts
        let returned = vec![
            assignment(1, PROSE),
            done(2, "protected-paths: AUTHORITY.md\n"),
            coord(3, DEV, Kind::Info, &reassigned_body("late")),
        ];
        assert_eq!(
            never(&facts(&item("Backlog", None), &returned)),
            Some(Never::OwnerRoute)
        );
    }

    #[test]
    fn n3_two_unattended_verdicts_keep_a_ticket_off_the_list() {
        let mut log = vec![assignment(1, PROSE), done(2, "")];
        log.push(verdict(3, 2, false, &stamp(1)));
        log.push(done(4, ""));
        assert_eq!(ticket_verdicts(&log), 1);
        assert_eq!(need("Review", Some(DEV), &log), Some(Need::ProseGate));
        log.push(verdict(5, 4, false, &stamp(2)));
        log.push(done(6, ""));
        assert_eq!(ticket_verdicts(&log), MAX_VERDICTS_PER_TICKET);
        assert_eq!(never_in("Review", &log), Some(Never::VerdictCap));
        assert_eq!(need("Review", Some(DEV), &log), None);
        // the cap holds in every Status
        let mut blocked = log.clone();
        blocked.push(dev(7, "coordinator", Kind::Blocked, "stuck"));
        assert_eq!(never_in("Blocked", &blocked), Some(Never::VerdictCap));
        // an owner comment starts the count again
        let mut released = log.clone();
        released.push(owner(7, "go on"));
        assert_eq!(ticket_verdicts(&released), 0);
        assert_eq!(never_in("Review", &released), None);
    }

    #[test]
    fn the_verdicts_of_a_ticket_count_stamped_verify_results_of_a_coordinator_only() {
        let base = || vec![assignment(1, PROSE), done(2, "")];
        // an attended verify result has no line and does not count
        let mut log = base();
        log.push(verdict(3, 2, false, ""));
        assert_eq!(ticket_verdicts(&log), 0);
        // a pass counts as a fail does
        log.push(verdict(4, 2, true, &stamp(1)));
        assert_eq!(ticket_verdicts(&log), 1);
        // a stamped answer is no verdict
        log.push(coord(5, DEV, Kind::Answer, &format!("8080\n{}", stamp(1))));
        assert_eq!(ticket_verdicts(&log), 1);
        // a stamped report of a dev agent is no verdict
        log.push(VMsg {
            re: Re::Comment {
                issue: ISSUE,
                comment: 2,
            },
            ..dev(
                6,
                COORD,
                Kind::Report,
                &format!("verify: pass\n{}", stamp(2)),
            )
        });
        assert_eq!(ticket_verdicts(&log), 1);
        // a line that is not valid counts: it cannot lower the count
        log.push(verdict(7, 2, false, "unattended: many\n"));
        assert_eq!(ticket_verdicts(&log), 2);
        // an assignment without the line starts the count again: a person or an attended
        // coordinator looked at the ticket
        let mut l = log.clone();
        l.push(assignment(8, PROSE));
        assert_eq!(ticket_verdicts(&l), 0);
        // a reassign does not: the duty for a dead agent posts one with nobody watching
        let mut l = log.clone();
        l.push(coord(
            8,
            DEV,
            Kind::Info,
            &reassigned_body("the agent is dead"),
        ));
        assert_eq!(ticket_verdicts(&l), 2);
        // an assignment with the line (an unattended assign) does not
        let mut l = log.clone();
        l.push(coord(
            8,
            DEV,
            Kind::Request,
            &format!("Assigned to you: Fix it\ngate: {PROSE}\n{}", stamp(2)),
        ));
        assert_eq!(ticket_verdicts(&l), 2);
        // a dev message and a plain coordinator info change nothing
        l.push(done(9, ""));
        l.push(coord(10, DEV, Kind::Info, "noted"));
        assert_eq!(ticket_verdicts(&l), 2);
    }

    #[test]
    fn the_floor_of_a_ticket_holds_until_a_message_starts_the_count_again() {
        // two stamped verdicts were posted when the highest comment id was 4; both are gone
        let floor = Some((2, 4));
        let mut log = vec![assignment(1, PROSE), done(2, ""), done(4, ""), done(6, "")];
        assert_eq!(ticket_verdicts(&log), 0);
        assert_eq!(ticket_verdicts_at_least(&log, floor), 2);
        assert_eq!(ticket_verdicts_at_least(&log, None), 0);
        // the log knows more than the floor
        assert_eq!(ticket_verdicts_at_least(&log, Some((1, 4))), 1);
        let mut more = log.clone();
        more.push(verdict(7, 6, false, &stamp(1)));
        more.push(verdict(8, 6, false, &stamp(2)));
        more.push(verdict(9, 6, false, &stamp(3)));
        assert_eq!(ticket_verdicts_at_least(&more, floor), 3);
        // these do not end the floor: a dev message, a reassign, a stamped assignment, an info
        // of a coordinator, and an owner comment or an assignment from before the floor
        log.push(dev(7, "coordinator", Kind::Question, "why?"));
        log.push(coord(8, DEV, Kind::Info, &reassigned_body("late")));
        log.push(coord(
            9,
            DEV,
            Kind::Request,
            &format!("Assigned to you: Fix it\ngate: {PROSE}\n{}", stamp(2)),
        ));
        log.push(coord(10, DEV, Kind::Info, "noted"));
        assert_eq!(ticket_verdicts_at_least(&log, floor), 2);
        let mut early = vec![owner(3, "go on")];
        early.extend(log.iter().cloned());
        assert_eq!(ticket_verdicts_at_least(&early, floor), 2);
        let it = item("Blocked", None);
        let f = facts(&it, &log);
        let with_floor = Facts {
            verdict_floor: floor,
            ..f
        };
        assert_eq!(never(&with_floor), Some(Never::VerdictCap));
        assert_eq!(never(&f), None);
        // these end it: an owner comment, and an assignment without the line
        let mut by_owner = log.clone();
        by_owner.push(owner(11, "go on"));
        assert_eq!(ticket_verdicts_at_least(&by_owner, floor), 0);
        let mut assigned = log.clone();
        assigned.push(assignment(11, PROSE));
        assert_eq!(ticket_verdicts_at_least(&assigned, floor), 0);
        // and a verdict after that counts from the log
        assigned.push(done(12, ""));
        assigned.push(verdict(13, 12, false, &stamp(3)));
        assert_eq!(ticket_verdicts_at_least(&assigned, floor), 1);
    }

    fn act(a: UAct, status: &str, agent: Option<&str>, msgs: &[VMsg]) -> Result<(), Block> {
        check_act(a, "a/b#5", &facts(&item(status, agent), msgs), Some(DEV))
    }

    fn scope_text(r: Result<(), Block>) -> String {
        match r {
            Err(Block::Scope(t)) => t,
            other => panic!("wanted a scope refusal, got {other:?}"),
        }
    }

    const ANNOUNCE: &str = "Ticket is on the board.\nitem=PVTI_5\nproject=15\n";

    #[test]
    fn a_verdict_is_for_a_prose_gate_ticket_in_review_that_the_signed_log_supports() {
        assert_eq!(
            act(UAct::Verdict, "Review", Some(DEV), &review_log()),
            Ok(())
        );
        // a typed Gate, another Status, no messages
        for typed in ["ci", "checks: tests"] {
            let log = vec![assignment(1, typed), done(2, "")];
            assert!(
                scope_text(act(UAct::Verdict, "Review", Some(DEV), &log))
                    .contains("is not a ticket in Review with a Gate in prose"),
                "{typed}"
            );
        }
        // the board says Review, and the signed log says In Progress or Ready (a failed verify)
        let mut failed = review_log();
        failed.push(verdict(3, 2, false, ""));
        assert!(
            scope_text(act(UAct::Verdict, "Review", Some(DEV), &failed))
                .contains("has a verify result")
        );
        let acked = vec![
            assignment(1, PROSE),
            dev(2, COORD, Kind::Ack, "I start now."),
        ];
        assert!(
            scope_text(act(UAct::Verdict, "Review", Some(DEV), &acked))
                .contains("has no signed done")
        );
        let it = item("Review", Some(DEV));
        let unread = Facts {
            msgs: None,
            ..facts(&it, &[])
        };
        assert!(
            scope_text(check_act(UAct::Verdict, "a/b#5", &unread, None)).contains("were not read")
        );
        // N1 to N3
        let mut asked = review_log();
        asked.push(dev(3, "owner", Kind::Question, "may I?"));
        assert!(
            scope_text(act(UAct::Verdict, "Review", Some(DEV), &asked))
                .contains("open question to the owner")
        );
        let mut capped = vec![assignment(1, PROSE), done(2, "")];
        capped.push(verdict(3, 2, false, &stamp(1)));
        capped.push(done(4, ""));
        capped.push(verdict(5, 4, false, &stamp(2)));
        capped.push(done(6, ""));
        assert_eq!(
            act(UAct::Verdict, "Review", Some(DEV), &capped),
            Err(Block::Cap)
        );
        // the board Agent is not the signed assignee
        assert!(
            scope_text(act(UAct::Verdict, "Review", Some(OTHER), &review_log()))
                .contains("is not the agent of the signed log")
        );
    }

    #[test]
    fn an_answer_and_unblock_are_for_a_ticket_that_does_not_wait_for_the_owner() {
        let log = blocked_log();
        assert_eq!(act(UAct::Answer, "Blocked", Some(DEV), &log), Ok(()));
        assert_eq!(act(UAct::Unblock, "Blocked", Some(DEV), &log), Ok(()));
        // an answer names the signed assignee
        let f_item = item("Blocked", Some(DEV));
        let f = facts(&f_item, &log);
        assert!(
            scope_text(check_act(UAct::Answer, "a/b#5", &f, Some(OTHER)))
                .contains("goes to the assignee only")
        );
        assert!(
            scope_text(check_act(UAct::Answer, "a/b#5", &f, None))
                .contains("goes to the assignee only")
        );
        // an answer needs no block: the agent works and asked nothing
        let working = vec![
            assignment(1, PROSE),
            dev(2, COORD, Kind::Ack, "I start now."),
        ];
        assert_eq!(
            act(UAct::Answer, "In Progress", Some(DEV), &working),
            Ok(())
        );
        // `unblock` needs a ticket that its agent blocked or asked about, in Blocked
        assert!(
            scope_text(act(UAct::Unblock, "In Progress", Some(DEV), &working))
                .contains("is not a ticket that its assignee blocked")
        );
        let mut answered = log.clone();
        answered.push(coord(3, DEV, Kind::Answer, "use the test key"));
        assert!(
            scope_text(act(UAct::Unblock, "Blocked", Some(DEV), &answered))
                .contains("is not a ticket that its assignee blocked")
        );
        // N1: an open question to the owner, a round stop, the label; and N3, N4
        let mut asked = log.clone();
        asked.push(dev(3, "owner", Kind::Question, "may I?"));
        let mut stopped = vec![assignment(1, PROSE), done(2, "")];
        stopped.push(verdict(
            3,
            2,
            false,
            "case: round limit 2 of 2; the owner decides\n",
        ));
        for a in [UAct::Answer, UAct::Unblock] {
            assert!(
                scope_text(act(a, "Blocked", Some(DEV), &asked))
                    .contains("open question to the owner"),
                "{a:?}"
            );
            assert!(
                scope_text(act(a, "Blocked", Some(DEV), &stopped))
                    .contains("stopped at the round limit"),
                "{a:?}"
            );
            let labelled = Facts {
                needs_owner_label: true,
                ..facts(&f_item, &log)
            };
            assert!(
                scope_text(check_act(a, "a/b#5", &labelled, Some(DEV))).contains("needs-owner"),
                "{a:?}"
            );
            let closed = Facts {
                closed: true,
                ..facts(&f_item, &log)
            };
            assert!(
                scope_text(check_act(a, "a/b#5", &closed, Some(DEV))).contains("is closed"),
                "{a:?}"
            );
            let capped = Facts {
                verdict_floor: Some((2, 2)),
                ..facts(&f_item, &log)
            };
            assert_eq!(
                check_act(a, "a/b#5", &capped, Some(DEV)),
                Err(Block::Cap),
                "{a:?}"
            );
            // the board Status against the signed log (the log says Review)
            assert!(
                scope_text(act(a, "Blocked", Some(DEV), &review_log()))
                    .contains("the signed log says Review"),
                "{a:?}"
            );
        }
    }

    #[test]
    fn assign_is_for_a_free_ticket_that_a_coordinator_announced() {
        let announced = || vec![coord(1, "owner", Kind::Info, ANNOUNCE)];
        let returned = || {
            let mut l = announced();
            l.push(assignment(2, PROSE));
            l.push(coord(3, DEV, Kind::Info, &reassigned_body("late")));
            l
        };
        // L5 and L4 after a reassign
        assert_eq!(act(UAct::Assign, "Backlog", None, &returned()), Ok(()));
        assert_eq!(act(UAct::Assign, "Ready", None, &returned()), Ok(()));
        // never assigned, and the owner moved it to Ready: the first assignment would sign the
        // Gate, the guard and the CI line from the issue body, which nobody signed
        let mut moved = announced();
        moved.push(owner(2, "/crew move Ready"));
        assert!(
            scope_text(act(UAct::Assign, "Ready", None, &moved))
                .contains("has no assignment with a Gate to copy")
        );
        // returned, and the assignment comment is gone
        let mut lost = announced();
        lost.push(coord(3, DEV, Kind::Info, &reassigned_body("late")));
        for status in ["Backlog", "Ready"] {
            assert!(
                scope_text(act(UAct::Assign, status, None, &lost))
                    .contains("has no assignment with a Gate to copy"),
                "{status}"
            );
        }
        // a Status that somebody set on the board alone: the signed log says Backlog
        assert!(
            scope_text(act(UAct::Assign, "Ready", None, &announced()))
                .contains("the signed log says Backlog")
        );
        assert_eq!(need("Ready", None, &announced()), None);
        // in Backlog and never assigned
        assert!(
            scope_text(act(UAct::Assign, "Backlog", None, &announced()))
                .contains("has no assignment with a Gate to copy")
        );
        // no signed message puts the ticket on the board
        let bare = vec![
            assignment(1, PROSE),
            coord(2, DEV, Kind::Info, &reassigned_body("late")),
        ];
        assert!(scope_text(act(UAct::Assign, "Backlog", None, &bare)).contains("on the board"));
        // a live assignment in the signed log, whatever the board Agent says
        let mut live = announced();
        live.push(assignment(2, PROSE));
        for agent in [None, Some(DEV)] {
            assert!(
                scope_text(act(UAct::Assign, "Ready", agent, &live))
                    .contains("has an assignee in the signed log"),
                "{agent:?}"
            );
        }
        // N1 and N3 hold for a returned ticket too
        let mut asked = returned();
        asked.push(coord(4, "owner", Kind::Question, "drop it?"));
        assert!(
            scope_text(act(UAct::Assign, "Backlog", None, &asked))
                .contains("open question to the owner")
        );
        let it = item("Backlog", None);
        let log = returned();
        let capped = Facts {
            verdict_floor: Some((2, 3)),
            ..facts(&it, &log)
        };
        assert_eq!(
            check_act(UAct::Assign, "a/b#5", &capped, None),
            Err(Block::Cap)
        );
    }

    #[test]
    fn ask_is_for_an_open_ticket_without_an_open_question_to_the_owner() {
        for (status, log) in [
            ("Blocked", blocked_log()),
            ("Review", review_log()),
            ("Backlog", vec![]),
        ] {
            assert_eq!(act(UAct::Ask, status, Some(DEV), &log), Ok(()), "{status}");
        }
        // one question waits for one answer: of the assignee, of a coordinator, and on a ticket
        // without an assignee of any agent
        let mut asked = blocked_log();
        asked.push(dev(3, "owner", Kind::Question, "may I?"));
        let mut by_coord = blocked_log();
        by_coord.push(coord(3, "owner", Kind::Question, "drop it?"));
        let mut returned = asked.clone();
        returned.push(coord(4, DEV, Kind::Info, &reassigned_body("late")));
        for (agent, log) in [
            (Some(DEV), &asked),
            (Some(DEV), &by_coord),
            (None, &returned),
        ] {
            assert!(
                scope_text(act(UAct::Ask, "Blocked", agent, log))
                    .contains("has an open question to the owner already"),
                "{agent:?}"
            );
        }
        // the owner answered: a new question is allowed
        asked.push(owner(4, "yes"));
        assert_eq!(act(UAct::Ask, "Blocked", Some(DEV), &asked), Ok(()));
        let it0 = item("Review", Some(DEV));
        let unread = Facts {
            msgs: None,
            ..facts(&it0, &[])
        };
        assert!(scope_text(check_act(UAct::Ask, "a/b#5", &unread, None)).contains("not read"));
        let it = item("Review", Some(DEV));
        let closed = Facts {
            closed: true,
            ..facts(&it, &[])
        };
        assert!(scope_text(check_act(UAct::Ask, "a/b#5", &closed, None)).contains("is closed"));
    }

    #[test]
    fn n4_a_closed_issue_keeps_a_ticket_off_the_list() {
        let log = review_log();
        let it = item("Review", Some(DEV));
        let closed = Facts {
            closed: true,
            ..facts(&it, &log)
        };
        assert_eq!(never(&closed), Some(Never::Closed));
        assert_eq!(need_of(&closed), None);
        // the state of the board item counts too, also without the messages
        let mut by_item = item("Blocked", Some(DEV));
        by_item.issue_state = Some("CLOSED".into());
        let f = Facts {
            msgs: None,
            ..facts(&by_item, &[])
        };
        assert_eq!(never(&f), Some(Never::Closed));
        // control
        assert_eq!(never(&facts(&it, &log)), None);
    }

    #[test]
    fn n5_a_review_ticket_needs_a_signed_gate_an_assignee_and_a_done() {
        // no signed gate: an assignment of an older binary
        let no_gate = vec![
            coord(1, DEV, Kind::Request, "Assigned to you: Fix it\n"),
            done(2, ""),
        ];
        assert_eq!(never_in("Review", &no_gate), Some(Never::NoGate));
        // a gate line of a dev agent is not signed by a coordinator
        let forged = vec![
            dev(
                1,
                DEV,
                Kind::Request,
                "Assigned to you: Fix it\ngate: anything\n",
            ),
            done(2, ""),
        ];
        assert_eq!(never_in("Review", &forged), Some(Never::AgentNotSigned));
        assert_eq!(
            never(&facts(&item("Review", None), &review_log())),
            Some(Never::NoAssignee)
        );
        assert_eq!(
            never_in("Review", &[assignment(1, PROSE)]),
            Some(Never::NoDone)
        );
        // a done without evidence is no done
        let bare = vec![assignment(1, PROSE), dev(2, COORD, Kind::Done, "Done.\n")];
        assert_eq!(never_in("Review", &bare), Some(Never::NoDone));
        // the rule is for Review: a Blocked ticket needs no done
        assert_eq!(never_in("Blocked", &[assignment(1, PROSE)]), None);
    }

    #[test]
    fn n6_a_local_gate_without_checks_and_ci_none_keep_a_review_ticket_off_the_list() {
        let local = |gate: &str| {
            vec![
                assignment(1, gate),
                dev(
                    2,
                    COORD,
                    Kind::Done,
                    "Done. Evidence follows.\npr: https://github.com/a/b/pull/7\nsha: abc\nci: local-gate\nlocal-gate: host-a: make test rc=0\n",
                ),
            ]
        };
        assert_eq!(never_in("Review", &local(PROSE)), Some(Never::LocalGate));
        assert_eq!(never_in("Review", &local("ci")), Some(Never::LocalGate));
        // a Gate of checks with a local gate is the work of the tick: not N6, and not L1
        assert_eq!(never_in("Review", &local("checks: tests")), None);
        assert_eq!(need("Review", Some(DEV), &local("checks: tests")), None);
        // `CI: none`, signed in the assignment
        let signed = vec![
            coord(
                1,
                DEV,
                Kind::Request,
                &format!("Assigned to you: Fix it\ngate: {PROSE}\nci: none\n"),
            ),
            done(2, ""),
        ];
        assert_eq!(never_in("Review", &signed), Some(Never::CiNone));
        // `CI: none` in the body of a ticket whose assignment has no `ci:` line
        let it = item("Review", Some(DEV));
        let log = review_log();
        let by_body = Facts {
            body: Some("Goal: x\nCI: none\n"),
            ..facts(&it, &log)
        };
        assert_eq!(never(&by_body), Some(Never::CiNone));
        // the signed line `ci: required` wins over a body that was edited later
        let required = vec![
            coord(
                1,
                DEV,
                Kind::Request,
                &format!("Assigned to you: Fix it\ngate: {PROSE}\nci: required\n"),
            ),
            done(2, ""),
        ];
        let edited = Facts {
            body: Some("CI: none\n"),
            ..facts(&it, &required)
        };
        assert_eq!(never(&edited), None);
    }

    #[test]
    fn n7_a_verify_result_for_the_latest_done_keeps_a_review_ticket_off_the_list() {
        for pass in [true, false] {
            let mut log = review_log();
            log.push(verdict(3, 2, pass, ""));
            assert_eq!(never_in("Review", &log), Some(Never::Verified), "{pass}");
            assert_eq!(need("Review", Some(DEV), &log), None);
            // a newer done has no result yet
            log.push(done(4, ""));
            assert_eq!(never_in("Review", &log), None, "{pass}");
            assert_eq!(need("Review", Some(DEV), &log), Some(Need::ProseGate));
        }
        // a report of a dev agent is no verify result
        let mut forged = review_log();
        forged.push(VMsg {
            re: Re::Comment {
                issue: ISSUE,
                comment: 2,
            },
            ..dev(3, COORD, Kind::Report, "verify: pass\n")
        });
        assert_eq!(never_in("Review", &forged), None);
    }

    #[test]
    fn the_verdicts_of_a_day_come_from_stamped_coordinator_messages_of_that_day() {
        let day = day_start(1_795_000_000);
        assert_eq!(day % 86_400, 0);
        assert!(day <= 1_795_000_000 && 1_795_000_000 - day < 86_400);
        assert_eq!(day_start(day), day);
        assert_eq!(day_start(day - 1), day - 86_400);
        let at = |m: VMsg, created: i64| VMsg { created, ..m };
        let msgs = vec![
            // yesterday: not counted
            at(verdict(1, 9, true, &stamp(9)), day - 1),
            // the first second of the day counts
            at(verdict(2, 9, true, &stamp(1)), day),
            at(verdict(3, 9, false, &stamp(2)), day + 60),
            // attended: no line
            at(verdict(4, 9, true, ""), day + 61),
            // a stamped answer is no verdict, but its `n` is a floor
            at(
                coord(5, DEV, Kind::Answer, &format!("8080\n{}", stamp(4))),
                day + 62,
            ),
            // a dev agent cannot raise the count
            at(
                dev(
                    6,
                    COORD,
                    Kind::Report,
                    &format!("verify: pass\n{}", stamp(10)),
                ),
                day + 63,
            ),
            // the owner neither
            at(owner(7, "unattended: 10/10"), day + 64),
            // a line that is not valid counts as a verdict and gives no `n`
            at(verdict(8, 9, true, "unattended: 99/10\n"), day + 65),
            // a stamped question of the coordinator to the owner is an ask; one without the line
            // and one to an agent are not
            at(
                coord(
                    9,
                    "owner",
                    Kind::Question,
                    &format!("drop it?\n{}", stamp(3)),
                ),
                day + 66,
            ),
            at(coord(10, "owner", Kind::Question, "drop it?"), day + 67),
            at(
                coord(11, DEV, Kind::Question, &format!("which?\n{}", stamp(3))),
                day + 68,
            ),
        ];
        let c = verdicts_on_day(&msgs, day);
        assert_eq!(
            c,
            DayCount {
                verdicts: 3,
                highest: 4,
                // the two verdicts, the answer, the verdict with the line that is not valid and
                // the two stamped questions
                messages: 6,
                asks: 1,
            }
        );
        assert_eq!(verdicts_on_day(&[], day), DayCount::default());
        // two issues together
        let both = c.with(DayCount {
            verdicts: 2,
            highest: 3,
            messages: 2,
            asks: 2,
        });
        assert_eq!(
            both,
            DayCount {
                verdicts: 5,
                highest: 4,
                messages: 8,
                asks: 3,
            }
        );
    }

    #[test]
    fn the_used_count_never_goes_below_a_floor() {
        let day = |verdicts, highest| DayCount {
            verdicts,
            highest,
            messages: verdicts,
            asks: 0,
        };
        assert_eq!(used(day(0, 0), 0), 0);
        assert_eq!(used(day(3, 3), 3), 3);
        // a deleted verdict: the count of the messages is lower than the `n` of one that stayed
        assert_eq!(used(day(2, 3), 0), 3);
        // every stamped message is gone: the floor of the local state holds
        assert_eq!(used(day(0, 0), 3), 3);
        // what an earlier read of the day gave is a floor too
        assert_eq!(day(1, 1).at_least(day(2, 2)), day(2, 2));
        assert_eq!(day(3, 2).at_least(day(2, 4)), day(3, 4));
        assert_eq!(day(3, 3).at_least(DayCount::default()), day(3, 3));
        // the log knows more than this host (a verdict from another host)
        assert_eq!(used(day(5, 4), 3), 5);
        // the stamped messages: the count, never below the verdicts and the floor
        let msgs = |verdicts, messages| DayCount {
            verdicts,
            highest: 0,
            messages,
            asks: 0,
        };
        assert_eq!(messages_used(msgs(2, 7), 0), 7);
        assert_eq!(messages_used(msgs(2, 7), 9), 9);
        assert_eq!(messages_used(msgs(3, 1), 0), 3);
        assert_eq!(
            msgs(1, 2).at_least(msgs(1, 5)),
            msgs(1, 5),
            "an earlier read of the day is a floor for the messages too"
        );
        // the floor is of one day
        let stored = ("2026-11-28".to_string(), 4);
        assert_eq!(floor_on(Some(&stored), "2026-11-28"), 4);
        assert_eq!(floor_on(Some(&stored), "2026-11-29"), 0);
        assert_eq!(floor_on(None, "2026-11-28"), 0);
    }

    #[test]
    fn the_lines_of_the_tick() {
        assert_eq!(
            need_line("a/b#5", Need::ProseGate),
            "needs judgment: a/b#5, prose Gate\n"
        );
        let reasons: Vec<&str> = [
            Need::ProseGate,
            Need::Question,
            Need::Blocked,
            Need::ReadyNoAgent,
            Need::Returned,
        ]
        .into_iter()
        .map(Need::reason)
        .collect();
        assert_eq!(
            reasons,
            [
                "prose Gate",
                "question",
                "blocked",
                "ready, no agent",
                "returned"
            ]
        );
        assert_eq!(
            count_line(CoordMode::Verify, 3, 10),
            "unattended coordinator: mode verify; verdicts 3/10 today\n"
        );
        assert_eq!(
            count_line(CoordMode::VerifyAndMerge, 0, 50),
            "unattended coordinator: mode verify-and-merge; verdicts 0/50 today\n"
        );
        assert_eq!(
            count_unread_line(CoordMode::Verify, "network: timeout\nmore"),
            "unattended coordinator: mode verify; the verdicts of today could not be read, so no ticket is listed: network: timeout\n"
        );
        assert_eq!(
            unread_line("a/b#6", "network: timeout\nmore"),
            "unattended coordinator: a/b#6 could not be read for the list: network: timeout\n"
        );
        // no line of the tick is a protocol line: a message that quotes one is not stamped
        for line in [
            need_line("a/b#5", Need::ProseGate),
            count_line(CoordMode::Verify, 3, 10),
            count_unread_line(CoordMode::Verify, "x"),
            unread_line("a/b#6", "x"),
        ] {
            assert!(!has_unattended_line(&line), "{line}");
            assert_eq!(unattended_of(&line), None, "{line}");
            assert!(!has_unattended_line(&format!("  > {line}")), "{line}");
        }
        // control: the protocol line is one
        assert!(has_unattended_line(&unattended_line(3, 10)));
    }
}
