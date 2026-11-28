//! The unattended coordinator (spec/agents#unattended-coordinator): which tickets need the
//! judgment of a coordinator, and how many verdicts the unattended coordinator gave today.
//!
//! Pure. The callers read the board items and the comments; this module decides what they mean.
//!
//! - [`never`] names the tickets that are never on the list (N1 to N7), whatever else holds.
//! - [`need_of`] names the judgment that a ticket needs (L1 to L5), or `None`.
//! - [`ticket_verdicts`], [`verdicts_on_day`] and [`used`] count the verdicts from the signed
//!   messages that carry the line `unattended: <n>/<budget>`.
//!
//! A board field is not signed. The list is a view for a reader, so it reads the Status and the
//! Agent from the board; every command that writes checks the signed log on its own.

use crate::authority::CoordMode;
use crate::base::fields::Field;
use crate::headless::Gate;
use crate::items::ItemSnapshot;
use crate::protocol::acts::{has_unattended_line, is_local_gate, unattended_of};
use crate::protocol::names::Kind;
use crate::rules::status::{BlockReason, Status};
use crate::ticket::{Act, Sender, VMsg};

/// How many unattended verdicts one ticket may get before a person looks at it.
pub const MAX_VERDICTS_PER_TICKET: usize = 2;

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
    /// L4: in Ready without an agent.
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

/// Is the ticket never on the list? The first rule that holds, in the order N4, N1, N2, N3, and
/// for a ticket in Review N5, N6, N7. The rules N5 to N7 are about the verdict on a `done`, so
/// they apply in Review only: a ticket that went back to its agent after a failed verify, and is
/// Blocked now, still has that old result.
pub fn never(f: &Facts<'_>) -> Option<Never> {
    if f.closed || f.item.issue_closed() {
        return Some(Never::Closed);
    }
    if f.needs_owner_label {
        return Some(Never::NeedsOwnerLabel);
    }
    let msgs = f.msgs?;
    let assignee = f.item.get(Field::Agent);
    if crate::ticket::question_open(msgs, assignee).is_some() {
        return Some(Never::OwnerQuestion);
    }
    if crate::ticket::fold_rounds(msgs).stopped {
        return Some(Never::RoundStop);
    }
    if latest_done_message(msgs, assignee).is_some_and(crate::ticket::is_owner_route_done) {
        return Some(Never::OwnerRoute);
    }
    if ticket_verdicts(msgs) >= MAX_VERDICTS_PER_TICKET {
        return Some(Never::VerdictCap);
    }
    if status_of(f.item) != Some(Status::Review) {
        return None;
    }
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

/// The judgment that the ticket needs, or `None`. [`never`] is checked first. The first rule that
/// holds, in the order L1 to L5; a rule that needs the messages does not hold without them.
pub fn need_of(f: &Facts<'_>) -> Option<Need> {
    if never(f).is_some() {
        return None;
    }
    let assignee = f.item.get(Field::Agent);
    match status_of(f.item)? {
        Status::Review => {
            let msgs = f.msgs?;
            if prose_gate(msgs) {
                return Some(Need::ProseGate);
            }
            open_question(msgs, assignee?).then_some(Need::Question)
        }
        Status::Blocked => {
            let (msgs, assignee) = (f.msgs?, assignee?);
            if open_question(msgs, assignee) {
                return Some(Need::Question);
            }
            plain_block_open(msgs, assignee).then_some(Need::Blocked)
        }
        Status::Ready => assignee.is_none().then_some(Need::ReadyNoAgent),
        Status::Backlog => {
            (is_backlog_candidate(f.item) && returned(f.msgs?)).then_some(Need::Returned)
        }
        Status::InProgress | Status::Done | Status::Dropped => None,
    }
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
fn returned(msgs: &[VMsg]) -> bool {
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
/// assignment or a reassign of a coordinator that has no `unattended:` line. A line that is not
/// valid counts as a line: it cannot lower the count.
pub fn ticket_verdicts(msgs: &[VMsg]) -> usize {
    let mut n = 0;
    for m in msgs {
        if m.sender == Sender::Owner {
            n = 0;
            continue;
        }
        if !is_coordinator(m) {
            continue;
        }
        let stamped = has_unattended_line(&m.body);
        match m.act() {
            Act::Assignment { .. } | Act::Reassigned if !stamped => n = 0,
            Act::Verify { .. } if stamped => n += 1,
            _ => {}
        }
    }
    n
}

/// What the messages of one UTC day say about the verdict count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DayCount {
    /// The verify results of a coordinator with an `unattended:` line.
    pub verdicts: u32,
    /// The highest `n` of a valid `unattended: <n>/<budget>` line of a coordinator message.
    pub highest: u32,
}

impl DayCount {
    /// Both counts over two sets of messages (two issues, two repos).
    pub fn with(self, other: DayCount) -> DayCount {
        DayCount {
            verdicts: self.verdicts.saturating_add(other.verdicts),
            highest: self.highest.max(other.highest),
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
        if matches!(m.act(), Act::Verify { .. }) {
            c.verdicts = c.verdicts.saturating_add(1);
        }
        if let Some((n, _)) = unattended_of(&m.body) {
            c.highest = c.highest.max(n);
        }
    }
    c
}

/// The floor that the local state holds for `today` (`YYYY-MM-DD`): the count that a command
/// wrote before it posted a verdict. A floor of another day is 0.
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

/// The line of `tick` for one ticket on the list.
pub fn need_line(key: &str, need: Need) -> String {
    format!("{NEEDS_JUDGMENT}: {key}, {}\n", need.reason())
}

/// The line of `tick` with the mode and the verdicts of the day.
pub fn count_line(mode: CoordMode, used: u32, budget: u32) -> String {
    format!(
        "unattended: mode {}; verdicts {used}/{budget} today\n",
        mode.as_str()
    )
}

/// The line of `tick` when the verdicts of the day could not be read. The tick then lists no
/// ticket: nobody can tell whether a verdict is left.
pub fn count_unread_line(mode: CoordMode, why: &str) -> String {
    format!(
        "unattended: mode {}; the verdicts of today could not be read, so no ticket is listed: {}\n",
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
    fn l4_a_ready_ticket_without_an_agent_needs_the_board_fields_only() {
        let it = item("Ready", None);
        let board_only = Facts {
            msgs: None,
            ..facts(&it, &[])
        };
        assert_eq!(need_of(&board_only), Some(Need::ReadyNoAgent));
        assert_eq!(need("Ready", None, &[]), Some(Need::ReadyNoAgent));
        // not L4: an agent, another Status, a closed issue
        assert_eq!(need("Ready", Some(DEV), &[]), None);
        assert_eq!(need("In Progress", None, &[]), None);
        let mut closed = item("Ready", None);
        closed.issue_state = Some("CLOSED".into());
        assert_eq!(need_of(&facts(&closed, &[])), None);
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

            let mut stopped = base.clone();
            stopped.push(coord(
                8,
                "owner",
                Kind::Question,
                &round_limit_question(2, 2, "https://example.invalid/r"),
            ));
            assert!(crate::ticket::fold_rounds(&stopped).stopped);
            assert!(never_in(status, &stopped).is_some());
            assert_eq!(need(status, Some(DEV), &stopped), None);

            let it = item(status, Some(DEV));
            let labelled = Facts {
                needs_owner_label: true,
                ..facts(&it, &base)
            };
            assert_eq!(never(&labelled), Some(Never::NeedsOwnerLabel));
            assert_eq!(need_of(&labelled), None);
        }
        // a stop that the verify result alone shows (its question was not posted)
        let mut stopped = review_log();
        stopped.push(verdict(
            3,
            2,
            false,
            "case: round limit 2 of 2; the owner decides\n",
        ));
        assert_eq!(never_in("Blocked", &stopped), Some(Never::RoundStop));
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
        // an assignment or a reassign without the line starts the count again
        for reset in [
            assignment(8, PROSE),
            coord(8, DEV, Kind::Info, &reassigned_body("late")),
        ] {
            let mut l = log.clone();
            l.push(reset);
            assert_eq!(ticket_verdicts(&l), 0);
        }
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
        assert_eq!(never_in("Review", &forged), Some(Never::NoGate));
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
        ];
        let c = verdicts_on_day(&msgs, day);
        assert_eq!(
            c,
            DayCount {
                verdicts: 3,
                highest: 4
            }
        );
        assert_eq!(verdicts_on_day(&[], day), DayCount::default());
        // two issues together
        let both = c.with(DayCount {
            verdicts: 2,
            highest: 3,
        });
        assert_eq!(
            both,
            DayCount {
                verdicts: 5,
                highest: 4
            }
        );
    }

    #[test]
    fn the_used_count_never_goes_below_a_floor() {
        let day = |verdicts, highest| DayCount { verdicts, highest };
        assert_eq!(used(day(0, 0), 0), 0);
        assert_eq!(used(day(3, 3), 3), 3);
        // a deleted verdict: the count of the messages is lower than the `n` of one that stayed
        assert_eq!(used(day(2, 3), 0), 3);
        // every stamped message is gone: the floor of the local state holds
        assert_eq!(used(day(0, 0), 3), 3);
        // the log knows more than this host (a verdict from another host)
        assert_eq!(used(day(5, 4), 3), 5);
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
            "unattended: mode verify; verdicts 3/10 today\n"
        );
        assert_eq!(
            count_line(CoordMode::VerifyAndMerge, 0, 50),
            "unattended: mode verify-and-merge; verdicts 0/50 today\n"
        );
        assert_eq!(
            count_unread_line(CoordMode::Verify, "network: timeout\nmore"),
            "unattended: mode verify; the verdicts of today could not be read, so no ticket is listed: network: timeout\n"
        );
    }
}
