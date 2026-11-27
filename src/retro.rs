//! `retro` (spec/retro): what the signed log of the tickets says about the work of a period.
//!
//! A retrospective counts classes, never agents: why work went back (the classes of the failed
//! verify results), where work stopped (questions, blocks) and how long the steps took. The
//! report ranks no agent.
//!
//! The caller passes only messages that count: the verified messages of each ticket
//! ([`crate::ticket::verified_messages`]), and the messages that fail only the replay window
//! ([`crate::ticket::out_of_window_messages`]). An unsigned or foreign comment never gets here.
//! Who may do an act is checked here, from the verified sender, with the rules of the ticket log
//! ([`crate::ticket::log::fold_ticket`]): only a coordinator assigns, reassigns, verifies and
//! closes; only the assignee of a ticket that has not ended delivers or parks; a late note is a
//! message to the owner.
//!
//! Pure: no reads, no writes, no clock.

use crate::base::{TicketRef, ids};
use crate::owner_cmd::{self, OwnerCommand};
use crate::protocol::acts::{
    CloseKind, FailClass, dropped_merged_pr, fail_classes, parse_evidence,
};
use crate::protocol::names::Kind;
use crate::rules::Role;
use crate::ticket::log::{Expected, fold_ticket};
use crate::ticket::{Act, Sender, VMsg, question_open};
use std::collections::BTreeMap;

/// The class word of a `PR head moved` fail that was accepted: the next verify result of the
/// ticket is a pass, or the ticket closes as merged, with no other fail in between. An artifact
/// of the flow, not rework.
pub const ACCEPTED_AFTER_HEAD_MOVED: &str = "accepted-after-head-moved";

/// How many tickets the section "rounds" lists.
pub const TOP_ROUNDS: usize = 5;

/// The counted messages of one issue, in comment order, with the URL of each comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketLog {
    pub ticket: TicketRef,
    pub msgs: Vec<VMsg>,
    /// Comment id to comment URL.
    pub urls: BTreeMap<u64, String>,
    /// How many of `msgs` fail only the replay window.
    pub outside_window: usize,
}

/// One message that the report lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub ticket: TicketRef,
    pub comment_id: u64,
    /// The URL of the comment.
    pub url: String,
    /// Creation time of the comment (GitHub's clock), Unix seconds.
    pub at: i64,
    /// The sender. For the JSON output only: the text report names no agent.
    pub from: String,
    /// The addressee. For the JSON output only.
    pub to: String,
}

/// The verify numbers. The rework numbers leave out the fails that are
/// [`ACCEPTED_AFTER_HEAD_MOVED`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyStats {
    /// Tickets with at least one verify result.
    pub verified: usize,
    /// Tickets whose first verify result is a pass.
    pub first_pass: usize,
    /// Tickets whose first verify result is a pass or a fail that was accepted.
    pub first_pass_or_accepted: usize,
    pub tickets_with_fail: usize,
    pub fails: usize,
    /// Tickets with at least one fail that is rework.
    pub rework_tickets: usize,
    /// Tickets with two or more fails that are rework: the work went back more than once.
    pub repeat_rework_tickets: usize,
    /// Fails that are rework.
    pub rework_fails: usize,
    /// Verify results that are a pass.
    pub passes: usize,
    /// Passes whose local-run line the tool wrote (`verify --run-check`): a machine ran the
    /// command part of the Gate, not a model.
    pub tool_run_passes: usize,
}

/// The tickets of one kind of Gate, and how their first verify went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GateRow {
    pub tickets: usize,
    /// Tickets with at least one verify result.
    pub verified: usize,
    /// Tickets whose first verify result is a pass.
    pub first_pass: usize,
}

/// The tickets by the kind of their signed Gate (the `gate:` line of the latest assignment; the
/// issue body is not read). It answers how much of the work a machine can decide: the headless
/// coordinator verifies and merges only a ticket whose Gate is `ci`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GateStats {
    /// The Gate is exactly `ci`: CI alone decides.
    pub ci_only: GateRow,
    /// The Gate is `checks: a, b`: named checks of AUTHORITY.md that the tool runs.
    pub checks: GateRow,
    /// Any other Gate: a command, a check by hand, a second condition.
    pub other: GateRow,
    /// No signed gate line (an assignment of an old binary).
    pub unsigned: GateRow,
}

/// A tag has this many review fails that are rework, or more: a repeated fault.
pub const REPEATED_FROM: usize = 3;

/// The review fails of one tag ([`crate::protocol::acts::review_tag`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagRow {
    pub tag: String,
    pub events: Vec<Event>,
}

/// The review fails by tag: what kind of fault came back, and how often.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    /// Every tag with a fail, most fails first; tags with the same count in name order.
    pub rows: Vec<TagRow>,
    /// Review fails that are rework and have no tag.
    pub untagged: usize,
}

/// The fails of one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassRow {
    pub class: &'static str,
    pub events: Vec<Event>,
}

/// One ticket of the section "rounds": its verify fails that are rework.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    pub ticket: TicketRef,
    pub fails: usize,
    /// The latest of these fails.
    pub latest: Event,
}

/// Where work stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Andon {
    pub questions_owner: Vec<Event>,
    /// Those of `questions_owner` that a coordinator posted at the round limit
    /// (spec/tickets#round-limit): the fixed first line says so.
    pub round_limit_stops: Vec<Event>,
    pub questions_coordinator: Vec<Event>,
    /// Questions to a named agent or to all.
    pub questions_other: Vec<Event>,
    pub blocked: Vec<Event>,
    pub parked: Vec<Event>,
    /// The question to the owner that is still open, of each ticket that has not ended.
    pub open_questions: Vec<Event>,
}

/// The mentions of one refusal id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusalRow {
    pub rule: &'static str,
    pub events: Vec<Event>,
}

/// The spread of a duration over tickets, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub tickets: usize,
    pub median_s: i64,
    pub p90_s: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flow {
    /// Every reassign notice.
    pub reassigns: Vec<Event>,
    pub reassigned_tickets: usize,
    /// The drop notice of each ticket that ended in Dropped.
    pub dropped: Vec<Event>,
    /// Those of `dropped` whose notice records a merged PR (the lines `pr:` and
    /// `merge-commit:`): the work was delivered, but the merge gave no Done.
    pub dropped_merged: Vec<Event>,
    /// The close of each ticket that has ended, by kind ([`close_word`]).
    pub closes: BTreeMap<&'static str, usize>,
    pub late_notes: Vec<Event>,
    /// From the first assignment to the first `done`.
    pub assign_to_done: Option<Spread>,
    /// From the first `done` to the close that made the ticket Done.
    pub done_to_close: Option<Spread>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Tickets: issues with a signed assignment of a coordinator.
    pub tickets: usize,
    /// Tickets with at least one `done` of the assignee.
    pub delivered: usize,
    /// The counted messages of the tickets.
    pub messages: usize,
    /// How many of them fail only the replay window (a wrong clock, not a copy).
    pub outside_window: usize,
    /// Creation time of the first and of the last counted message.
    pub first: Option<i64>,
    pub last: Option<i64>,
    pub verify: VerifyStats,
    pub gates: GateStats,
    /// Only classes with a fail, in the order of [`FailClass::ALL`].
    pub classes: Vec<ClassRow>,
    /// The review fails by the tag of their note.
    pub tags: Tags,
    pub rounds: Vec<Round>,
    pub andon: Andon,
    /// Only rule ids with a mention, most mentions first.
    pub refusals: Vec<RefusalRow>,
    pub flow: Flow,
    /// Merged work without an assignment: the drop notice with a merged PR of each issue that
    /// ended in Dropped and that no coordinator assigned. Such an issue is no ticket, so it
    /// is in no other number.
    pub merged_unassigned: Vec<Event>,
}

/// The word of a close kind in the report.
pub fn close_word(k: CloseKind) -> &'static str {
    match k {
        CloseKind::Verified => "verified",
        CloseKind::Merged => "merged",
        CloseKind::OwnerMerged => "owner-merged",
        CloseKind::Judgment => "judgment",
        CloseKind::Dropped => "dropped",
    }
}

/// A close that says the PR is merged.
fn is_merge_close(k: CloseKind) -> bool {
    matches!(
        k,
        CloseKind::Verified | CloseKind::Merged | CloseKind::OwnerMerged
    )
}

/// The median (the mean of the two middle values for an even count) and the 90th percentile
/// (nearest rank: the value at position `ceil(0.9 n)`) of `values`. `None` for no value.
pub fn spread(values: &[i64]) -> Option<Spread> {
    let mut v = values.to_vec();
    v.sort_unstable();
    let n = v.len();
    if n == 0 {
        return None;
    }
    let median_s = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2
    };
    Some(Spread {
        tickets: n,
        median_s,
        p90_s: v[(9 * n).div_ceil(10) - 1],
    })
}

/// The refusal ids of the catalog that `body` names as a whole word, in catalog order, each
/// once. A longer word that contains an id (`xdone-needs-pr`, `done-needs-pr-2`) is no mention.
pub fn refusals_mentioned(body: &str) -> Vec<&'static str> {
    let part_of_word = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    ids::ALL
        .iter()
        .map(|id| id.as_str())
        .filter(|id| {
            body.match_indices(id).any(|(at, _)| {
                let before = body[..at].chars().next_back();
                let after = body[at + id.len()..].chars().next();
                !before.is_some_and(part_of_word) && !after.is_some_and(part_of_word)
            })
        })
        .collect()
}

/// One verify result of a ticket.
struct Verified {
    pass: bool,
    /// The classes of a fail ([`fail_classes`]); empty for a pass.
    classes: Vec<FailClass>,
    /// A `PR head moved` fail that was accepted: not rework.
    accepted: bool,
    /// The tag of a review fail.
    tag: Option<String>,
    /// A pass whose local-run line the tool wrote.
    tool_run: bool,
    event: Event,
}

/// What the log of one ticket says. Every list is in comment order.
#[derive(Default)]
struct TicketFacts {
    /// Creation time of the first assignment.
    assigned: Option<i64>,
    /// The signed Gate of the latest assignment; `None`: no signed gate line.
    gate: Option<crate::headless::Gate>,
    /// Creation time of each `done` of the assignee.
    dones: Vec<i64>,
    verifies: Vec<Verified>,
    closes: Vec<(CloseKind, Event)>,
    /// The comment ids of the drop notices that record a merged PR.
    drops_with_pr: Vec<u64>,
    reassigns: Vec<Event>,
    late: Vec<Event>,
    questions_owner: Vec<Event>,
    /// The questions of the round limit: of a coordinator, with the fixed first line.
    round_limit_stops: Vec<Event>,
    questions_coordinator: Vec<Event>,
    questions_other: Vec<Event>,
    blocked: Vec<Event>,
    parked: Vec<Event>,
    refusals: Vec<(&'static str, Event)>,
}

/// One step of a ticket that the artifact rule reads, in comment order. A `done` is no step:
/// the dev agent runs it again for the new head, and that is no rework.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The index of the verify result in `TicketFacts::verifies`.
    Fail(usize),
    Pass,
    Close(CloseKind),
}

fn event_of(log: &TicketLog, m: &VMsg) -> Event {
    Event {
        ticket: log.ticket.clone(),
        comment_id: m.comment_id,
        url: log.urls.get(&m.comment_id).cloned().unwrap_or_default(),
        at: m.created,
        from: match &m.sender {
            Sender::Agent { name, .. } => name.clone(),
            Sender::Owner => "owner".to_string(),
        },
        to: m.to.clone(),
    }
}

/// Fold the messages of one issue. Without `assigned`, no coordinator assigned it: it is no
/// ticket of the report.
fn facts_of(log: &TicketLog) -> TicketFacts {
    let mut f = TicketFacts::default();
    let mut assignee: Option<&str> = None;
    // The ticket has ended (Done or Dropped), as `fold_ticket` tracks it: an owner move or a
    // later coordinator act opens it again.
    let mut closed = false;
    let mut steps: Vec<Step> = Vec::new();
    for m in &log.msgs {
        let Sender::Agent { name, role } = &m.sender else {
            if let Ok(Some(OwnerCommand::Move { .. })) = owner_cmd::parse(&m.body) {
                closed = false;
            }
            continue;
        };
        let coordinator = *role == Role::Coordinator;
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
        // the assignee of a ticket that has not ended
        let by_assignee = !closed && assignee == Some(name.as_str());
        let ev = || event_of(log, m);
        match (m.kind, act) {
            (_, Act::Assignment { gate, .. }) if coordinator => {
                f.assigned.get_or_insert(m.created);
                // the latest assignment decides, as for the headless coordinator
                f.gate = gate.as_deref().map(crate::headless::Gate::parse);
                assignee = Some(m.to.as_str());
            }
            (_, Act::Reassigned) if coordinator => {
                assignee = None;
                f.reassigns.push(ev());
            }
            (_, Act::Verify { pass }) if coordinator => {
                steps.push(if pass {
                    Step::Pass
                } else {
                    Step::Fail(f.verifies.len())
                });
                f.verifies.push(Verified {
                    pass,
                    classes: if pass {
                        Vec::new()
                    } else {
                        fail_classes(&m.body)
                    },
                    accepted: false,
                    tag: if pass {
                        None
                    } else {
                        crate::protocol::acts::review_tag(&m.body)
                    },
                    tool_run: pass && crate::protocol::acts::is_tool_run_verify(&m.body),
                    event: ev(),
                });
            }
            (_, Act::Closed(k)) if coordinator => {
                steps.push(Step::Close(k));
                f.closes.push((k, ev()));
                if k == CloseKind::Dropped && dropped_merged_pr(&m.body).is_some() {
                    f.drops_with_pr.push(m.comment_id);
                }
            }
            // a late note is a report to the owner, of the coordinator or of a dev agent
            // (`ops::late::late_info_exists`)
            (_, Act::Late { .. }) if m.to == "owner" => f.late.push(ev()),
            (Some(Kind::Done), _) if by_assignee && parse_evidence(&m.body).is_some() => {
                f.dones.push(m.created);
            }
            (Some(Kind::Question), _) => match m.to.as_str() {
                "owner" => {
                    f.questions_owner.push(ev());
                    if coordinator && crate::protocol::acts::round_limit_rounds(&m.body).is_some() {
                        f.round_limit_stops.push(ev());
                    }
                }
                "coordinator" => f.questions_coordinator.push(ev()),
                _ => f.questions_other.push(ev()),
            },
            // `take --interrupt` parks the ticket that the agent itself works on
            (Some(Kind::Blocked), Act::Parked { .. }) if by_assignee => f.parked.push(ev()),
            // a park text of anyone else is a plain `blocked` message
            (Some(Kind::Blocked), _) => f.blocked.push(ev()),
            _ => {}
        }
        if *role == Role::Dev
            && matches!(m.kind, Some(Kind::Question | Kind::Blocked | Kind::Report))
        {
            f.refusals
                .extend(refusals_mentioned(&m.body).into_iter().map(|id| (id, ev())));
        }
    }
    mark_accepted(&mut f.verifies, &steps);
    f
}

/// The end of a ticket: its latest close, when the log still ends there (an owner move or a
/// new assignment opens a ticket again).
fn ended_close<'a>(log: &TicketLog, f: &'a TicketFacts) -> Option<&'a (CloseKind, Event)> {
    let ended = matches!(fold_ticket(&log.msgs).status, Expected::Is(s) if s.ended());
    f.closes.last().filter(|_| ended)
}

/// The drop notice that ended the issue, when it records a merged PR.
fn ended_drop_with_pr<'a>(log: &TicketLog, f: &'a TicketFacts) -> Option<&'a Event> {
    let (kind, ev) = ended_close(log, f)?;
    (*kind == CloseKind::Dropped && f.drops_with_pr.contains(&ev.comment_id)).then_some(ev)
}

/// The artifact rule: a fail whose only class is `PR head moved` is accepted when the next
/// verify result or close after it is a pass or a close that says the PR is merged. A fail with
/// a second class (a review text, another machine class) stays rework, and so does a fail that
/// another fail follows, or whose ticket is dropped or still open.
fn mark_accepted(verifies: &mut [Verified], steps: &[Step]) {
    for (at, step) in steps.iter().enumerate() {
        let Step::Fail(i) = *step else {
            continue;
        };
        // a fail that is a round of repair is never an artifact of the flow
        if crate::protocol::acts::counts_as_round(&verifies[i].classes) {
            continue;
        }
        verifies[i].accepted = match steps.get(at + 1) {
            Some(Step::Pass) => true,
            Some(Step::Close(k)) => is_merge_close(*k),
            Some(Step::Fail(_)) | None => false,
        };
    }
}

/// The rows of the section "verify fails by class": the classes in the order of
/// [`FailClass::ALL`], with [`ACCEPTED_AFTER_HEAD_MOVED`] after `head-moved`. A fail with
/// failures of two classes is in both rows.
fn class_rows(facts: &[TicketFacts]) -> Vec<ClassRow> {
    let fails = || facts.iter().flat_map(|f| &f.verifies).filter(|v| !v.pass);
    let mut rows = Vec::new();
    for class in FailClass::ALL {
        rows.push(ClassRow {
            class: class.word(),
            events: fails()
                .filter(|v| !v.accepted && v.classes.contains(&class))
                .map(|v| v.event.clone())
                .collect(),
        });
        if class == FailClass::HeadMoved {
            rows.push(ClassRow {
                class: ACCEPTED_AFTER_HEAD_MOVED,
                events: fails()
                    .filter(|v| v.accepted)
                    .map(|v| v.event.clone())
                    .collect(),
            });
        }
    }
    rows.retain(|r| !r.events.is_empty());
    rows
}

/// The review fails that are rework, by tag.
fn tag_rows(facts: &[TicketFacts]) -> Tags {
    let mut by: BTreeMap<String, Vec<Event>> = BTreeMap::new();
    let mut untagged = 0;
    for v in facts.iter().flat_map(|f| &f.verifies) {
        if v.pass || v.accepted || !v.classes.contains(&FailClass::Review) {
            continue;
        }
        match &v.tag {
            Some(t) => by.entry(t.clone()).or_default().push(v.event.clone()),
            None => untagged += 1,
        }
    }
    let mut rows: Vec<TagRow> = by
        .into_iter()
        .map(|(tag, events)| TagRow { tag, events })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.events.len()));
    Tags { rows, untagged }
}

fn verify_stats(facts: &[TicketFacts]) -> VerifyStats {
    let mut s = VerifyStats::default();
    for f in facts {
        let Some(first) = f.verifies.first() else {
            continue;
        };
        s.verified += 1;
        s.first_pass += usize::from(first.pass);
        s.first_pass_or_accepted += usize::from(first.pass || first.accepted);
        let fails = f.verifies.iter().filter(|v| !v.pass).count();
        let rework = f.verifies.iter().filter(|v| !v.pass && !v.accepted).count();
        s.fails += fails;
        s.rework_fails += rework;
        s.tickets_with_fail += usize::from(fails > 0);
        s.rework_tickets += usize::from(rework > 0);
        s.repeat_rework_tickets += usize::from(rework > 1);
        s.passes += f.verifies.iter().filter(|v| v.pass).count();
        s.tool_run_passes += f.verifies.iter().filter(|v| v.tool_run).count();
    }
    s
}

fn gate_stats(facts: &[TicketFacts]) -> GateStats {
    let mut s = GateStats::default();
    for f in facts {
        let row = match &f.gate {
            Some(g) if g.is_ci_only() => &mut s.ci_only,
            Some(crate::headless::Gate::Checks(_)) => &mut s.checks,
            Some(_) => &mut s.other,
            None => &mut s.unsigned,
        };
        row.tickets += 1;
        if let Some(first) = f.verifies.first() {
            row.verified += 1;
            row.first_pass += usize::from(first.pass);
        }
    }
    s
}

/// The tickets with the most fails that are rework, at most [`TOP_ROUNDS`]. Tickets with the
/// same count stay in ticket order.
fn rounds(logs: &[&TicketLog], facts: &[TicketFacts]) -> Vec<Round> {
    let mut out: Vec<Round> = logs
        .iter()
        .zip(facts)
        .filter_map(|(log, f)| {
            let rework: Vec<&Verified> = f
                .verifies
                .iter()
                .filter(|v| !v.pass && !v.accepted)
                .collect();
            Some(Round {
                ticket: log.ticket.clone(),
                fails: rework.len(),
                latest: rework.last()?.event.clone(),
            })
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.fails));
    out.truncate(TOP_ROUNDS);
    out
}

fn refusal_rows(facts: &[TicketFacts]) -> Vec<RefusalRow> {
    let mut by: BTreeMap<&'static str, Vec<Event>> = BTreeMap::new();
    for (rule, ev) in facts.iter().flat_map(|f| &f.refusals) {
        by.entry(rule).or_default().push(ev.clone());
    }
    let mut rows: Vec<RefusalRow> = by
        .into_iter()
        .map(|(rule, events)| RefusalRow { rule, events })
        .collect();
    rows.sort_by_key(|x| std::cmp::Reverse(x.events.len()));
    rows
}

/// The flow numbers. `logs` and `facts` are the tickets, in the same order.
fn flow(logs: &[&TicketLog], facts: &[TicketFacts]) -> Flow {
    let mut out = Flow::default();
    let (mut to_done, mut to_close) = (Vec::new(), Vec::new());
    for (log, f) in logs.iter().zip(facts) {
        out.reassigned_tickets += usize::from(!f.reassigns.is_empty());
        out.reassigns.extend(f.reassigns.iter().cloned());
        out.late_notes.extend(f.late.iter().cloned());
        if let Some((kind, ev)) = ended_close(log, f) {
            *out.closes.entry(close_word(*kind)).or_default() += 1;
            if *kind == CloseKind::Dropped {
                out.dropped.push(ev.clone());
            }
        }
        out.dropped_merged
            .extend(ended_drop_with_pr(log, f).cloned());
        let (Some(assigned), Some(done)) = (f.assigned, f.dones.first().copied()) else {
            continue;
        };
        to_done.push(done - assigned);
        if let Some((_, close)) = f
            .closes
            .iter()
            .find(|(k, ev)| *k != CloseKind::Dropped && ev.at >= done)
        {
            to_close.push(close.at - done);
        }
    }
    out.assign_to_done = spread(&to_done);
    out.done_to_close = spread(&to_close);
    out
}

fn andon(logs: &[&TicketLog], facts: &[TicketFacts]) -> Andon {
    let mut a = Andon::default();
    for (log, f) in logs.iter().zip(facts) {
        a.questions_owner.extend(f.questions_owner.iter().cloned());
        a.round_limit_stops
            .extend(f.round_limit_stops.iter().cloned());
        a.questions_coordinator
            .extend(f.questions_coordinator.iter().cloned());
        a.questions_other.extend(f.questions_other.iter().cloned());
        a.blocked.extend(f.blocked.iter().cloned());
        a.parked.extend(f.parked.iter().cloned());
        let folded = fold_ticket(&log.msgs);
        let ended = matches!(folded.status, Expected::Is(s) if s.ended());
        if let (false, Some(q)) = (ended, question_open(&log.msgs, folded.agent.as_deref())) {
            a.open_questions.push(event_of(log, q));
        }
    }
    a
}

/// The report of `logs`: one entry per issue, each with its counted messages in comment order.
/// An issue without a signed assignment of a coordinator is no ticket and counts nowhere, with
/// one exception: its drop notice with a merged PR is in `merged_unassigned`, and only there.
pub fn retro(logs: &[TicketLog]) -> Report {
    let (mut tickets, mut facts): (Vec<&TicketLog>, Vec<TicketFacts>) = (Vec::new(), Vec::new());
    let mut merged_unassigned = Vec::new();
    for log in logs {
        let f = facts_of(log);
        if f.assigned.is_some() {
            tickets.push(log);
            facts.push(f);
        } else {
            merged_unassigned.extend(ended_drop_with_pr(log, &f).cloned());
        }
    }
    let times = || {
        tickets
            .iter()
            .flat_map(|l| &l.msgs)
            .filter(|m| m.sender != Sender::Owner)
            .map(|m| m.created)
    };
    Report {
        tickets: tickets.len(),
        delivered: facts.iter().filter(|f| !f.dones.is_empty()).count(),
        messages: times().count(),
        outside_window: tickets.iter().map(|l| l.outside_window).sum(),
        first: times().min(),
        last: times().max(),
        verify: verify_stats(&facts),
        gates: gate_stats(&facts),
        classes: class_rows(&facts),
        tags: tag_rows(&facts),
        rounds: rounds(&tickets, &facts),
        andon: andon(&tickets, &facts),
        refusals: refusal_rows(&facts),
        flow: flow(&tickets, &facts),
        merged_unassigned,
    }
}

#[cfg(test)]
#[path = "retro_tests.rs"]
mod tests;
