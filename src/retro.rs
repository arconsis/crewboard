//! `retro` (spec/retro): what the signed log of the tickets says about the work of a period.
//!
//! A retrospective counts classes, never agents: why work went back (the classes of the failed
//! verify results), where work stopped (questions, blocks) and how long the steps took. The
//! report ranks no agent.
//!
//! The caller passes only messages that count: the verified messages of each ticket
//! ([`crate::ticket::verified_messages`]), and the messages that fail only the replay window
//! ([`crate::ticket::out_of_window_messages`]). An unsigned or foreign comment never gets here.
//! Who may do an act is checked here, from the verified sender: only a coordinator assigns,
//! reassigns, verifies and closes, and only the assignee delivers.
//!
//! Pure: no reads, no writes, no clock.

use crate::base::{TicketRef, ids};
use crate::protocol::acts::{CloseKind, FailClass, fail_classes, parse_evidence};
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
    /// Fails that are rework.
    pub rework_fails: usize,
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
    /// Only classes with a fail, in the order of [`FailClass::ALL`].
    pub classes: Vec<ClassRow>,
    pub rounds: Vec<Round>,
    pub andon: Andon,
    /// Only rule ids with a mention, most mentions first.
    pub refusals: Vec<RefusalRow>,
    pub flow: Flow,
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
    event: Event,
}

/// What the log of one ticket says. Every list is in comment order.
#[derive(Default)]
struct TicketFacts {
    /// Creation time of the first assignment.
    assigned: Option<i64>,
    /// Creation time of each `done` of the assignee.
    dones: Vec<i64>,
    verifies: Vec<Verified>,
    closes: Vec<(CloseKind, Event)>,
    reassigns: Vec<Event>,
    late: Vec<Event>,
    questions_owner: Vec<Event>,
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

/// Fold the messages of one issue. `None`: no coordinator assigned it, so it is no ticket of
/// the report.
fn facts_of(log: &TicketLog) -> Option<TicketFacts> {
    let mut f = TicketFacts::default();
    let mut assignee: Option<&str> = None;
    let mut steps: Vec<Step> = Vec::new();
    for m in &log.msgs {
        let Sender::Agent { name, role } = &m.sender else {
            continue;
        };
        let coordinator = *role == Role::Coordinator;
        let ev = || event_of(log, m);
        match (m.kind, m.act()) {
            (_, Act::Assignment { .. }) if coordinator => {
                f.assigned.get_or_insert(m.created);
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
                    event: ev(),
                });
            }
            (_, Act::Closed(k)) if coordinator => {
                steps.push(Step::Close(k));
                f.closes.push((k, ev()));
            }
            (_, Act::Late { .. }) => f.late.push(ev()),
            (Some(Kind::Done), _)
                if assignee == Some(name.as_str()) && parse_evidence(&m.body).is_some() =>
            {
                f.dones.push(m.created);
            }
            (Some(Kind::Question), _) => match m.to.as_str() {
                "owner" => f.questions_owner.push(ev()),
                "coordinator" => f.questions_coordinator.push(ev()),
                _ => f.questions_other.push(ev()),
            },
            (Some(Kind::Blocked), Act::Parked { .. }) => f.parked.push(ev()),
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
    f.assigned?;
    mark_accepted(&mut f.verifies, &steps);
    Some(f)
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
        if verifies[i].classes != [FailClass::HeadMoved] {
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
        // The end of a ticket: its latest close, when the log still ends there (an owner move or
        // a new assignment opens a ticket again).
        let ended = matches!(fold_ticket(&log.msgs).status, Expected::Is(s) if s.ended());
        if let (true, Some((kind, ev))) = (ended, f.closes.last()) {
            *out.closes.entry(close_word(*kind)).or_default() += 1;
            if *kind == CloseKind::Dropped {
                out.dropped.push(ev.clone());
            }
        }
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
/// An issue without a signed assignment of a coordinator is no ticket and counts nowhere.
pub fn retro(logs: &[TicketLog]) -> Report {
    let (tickets, facts): (Vec<&TicketLog>, Vec<TicketFacts>) = logs
        .iter()
        .filter_map(|log| Some((log, facts_of(log)?)))
        .unzip();
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
        classes: class_rows(&facts),
        rounds: rounds(&tickets, &facts),
        andon: andon(&tickets, &facts),
        refusals: refusal_rows(&facts),
        flow: flow(&tickets, &facts),
    }
}

#[cfg(test)]
#[path = "retro_tests.rs"]
mod tests;
