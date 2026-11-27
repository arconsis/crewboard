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
use crate::ticket::{Act, Sender, VMsg, closes_question, kv, question_open};
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

/// A merge that the signed log of a ticket records: the lines `pr:` and `merge-commit:` of the
/// note of a merge, of a close after an owner merge, or of a drop with a merged PR. A note of a
/// merge without a `pr:` line (`close --verified`) takes the PR of the latest `done` before it.
/// A close with no PR and no merge commit gives no `MergedRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedRef {
    /// The close note.
    pub close: Event,
    /// The repo of the PR, as the message names it (it may be a former name); the repo of the
    /// ticket when no PR is known.
    pub repo: String,
    /// The number of the PR.
    pub pr: Option<u64>,
    /// The full sha of the merge commit, in lower case. `None` when the note has no sha
    /// (`merge-commit: unknown`).
    pub merge_commit: Option<String>,
    /// From when on a commit can revert this merge: the creation time of the latest `done`
    /// before the close note, or of the close note when the log has no `done` before it.
    pub since: i64,
}

/// The names of the repos of the initiative. Plain data of the config, so this module needs
/// no config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoNames {
    /// The repos of the initiative, by their current names.
    pub repos: Vec<String>,
    /// A former name of a repo, and its current name.
    pub former: Vec<(String, String)>,
}

impl RepoNames {
    /// The current name of the repo that `name` names, in the spelling of `repos`. `None`:
    /// `name` is no repo of the initiative.
    pub fn current(&self, name: &str) -> Option<&str> {
        let name = self
            .former
            .iter()
            .find(|(old, _)| old.eq_ignore_ascii_case(name))
            .map_or(name, |(_, now)| now.as_str());
        self.repos
            .iter()
            .find(|r| r.eq_ignore_ascii_case(name))
            .map(String::as_str)
    }
}

/// The commits of the default branch of `repo` that the report needs: those from `since` on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// The current name of the repo.
    pub repo: String,
    /// The earliest [`MergedRef::since`] of the merges in this repo.
    pub since: i64,
}

/// One commit of the default branch of a repo, as the caller read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCommit {
    pub sha: String,
    /// The commit time, Unix seconds.
    pub at: i64,
    pub message: String,
}

/// Why the commits of a repo are not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFault {
    /// The list is longer than the caller reads.
    Cut,
    /// The read failed. The text is a short cause (`HTTP 404`, `bad answer`).
    Failed(String),
}

/// What the caller read for one [`Window`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCommits {
    /// The repo of the window.
    pub repo: String,
    pub commits: Result<Vec<BaseCommit>, ReadFault>,
}

/// A repo whose commits are not known: its merged tickets are in no number of [`Reverted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unread {
    pub repo: String,
    pub fault: ReadFault,
    /// The start of the window.
    pub since: i64,
}

/// A merged ticket whose merge a later commit reverts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revert {
    /// The close note whose merge is reverted.
    pub close: Event,
    /// The commit that reverts it: the earliest one.
    pub sha: String,
    pub at: i64,
}

/// The merged tickets that a commit of the default branch reverts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reverted {
    /// Merged tickets with a [`MergedRef`] in a repo whose commits were read: the tickets
    /// that a revert was looked up for.
    pub known: usize,
    /// Merged tickets whose every [`MergedRef`] is in a repo that is none of the initiative:
    /// not looked up.
    pub outside: usize,
    /// The repos with a merge whose commits were not read. Empty: every needed repo was read.
    pub unread: Vec<Unread>,
    /// At most one per ticket, in ticket order.
    pub reverts: Vec<Revert>,
}

impl Reverted {
    /// The commits of every repo with a merge were read.
    pub fn read(&self) -> bool {
        self.unread.is_empty()
    }
}

/// Whether merged work was right, and how much the owner had to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Merged tickets: tickets with at least one close that says the PR is merged, or with a
    /// drop whose note records a merged PR.
    pub merged: usize,
    /// For each merged ticket that was opened again after such a close: the first message that
    /// opened it.
    pub came_back: Vec<Event>,
    pub reverted: Reverted,
    /// Tickets with at least one owner comment after their first assignment.
    pub owner_touched: usize,
    /// The owner comments per ticket of `owner_touched`. The values of this [`Spread`] are
    /// counts, not seconds.
    pub owner_comments: Option<Spread>,
    /// The seconds from a question to the owner to the first message that closes it. One value
    /// per answered question: `Spread::tickets` is the number of questions.
    pub owner_wait: Option<Spread>,
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
    pub outcome: Outcome,
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
    /// A close says that the PR is merged, or a drop note records a merged PR.
    merged: bool,
    /// What the notes of those closes record.
    merge_refs: Vec<MergedRef>,
    /// The first message that opened the ticket again after such a close.
    came_back: Option<Event>,
    /// The headerless comments of the owner from the first assignment on.
    owner_comments: usize,
    /// For each answered question to the owner: the seconds to the message that closed it.
    owner_waits: Vec<i64>,
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
    // The creation times of the questions to the owner that no message has closed yet.
    let mut asked: Vec<i64> = Vec::new();
    // The latest `done` of the assignee so far: its creation time and its PR.
    let mut last_done: Option<Delivery> = None;
    for m in &log.msgs {
        if closes_question(m) {
            f.owner_waits
                .extend(asked.drain(..).map(|q| (m.created - q).max(0)));
        }
        // a message that opens the ticket again: after a merge, the work came back
        let opens = |f: &mut TicketFacts, closed: &mut bool| {
            if *closed && f.merged && f.came_back.is_none() {
                f.came_back = Some(event_of(log, m));
            }
            *closed = false;
        };
        let Sender::Agent { name, role } = &m.sender else {
            // the talk before the first assignment is not work on the ticket
            f.owner_comments += usize::from(f.assigned.is_some());
            if let Ok(Some(OwnerCommand::Move { .. })) = owner_cmd::parse(&m.body) {
                opens(&mut f, &mut closed);
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
                | Act::Verify { pass: false } => opens(&mut f, &mut closed),
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
                let drop_with_pr = k == CloseKind::Dropped && dropped_merged_pr(&m.body).is_some();
                if drop_with_pr {
                    f.drops_with_pr.push(m.comment_id);
                }
                if is_merge_close(k) || drop_with_pr {
                    f.merged = true;
                    f.merge_refs.extend(merged_ref(
                        log,
                        ev(),
                        &m.body,
                        last_done.as_ref(),
                        is_merge_close(k),
                    ));
                }
            }
            // a late note is a report to the owner, of the coordinator or of a dev agent
            // (`ops::late::late_info_exists`)
            (_, Act::Late { .. }) if m.to == "owner" => f.late.push(ev()),
            (Some(Kind::Done), _) if by_assignee && parse_evidence(&m.body).is_some() => {
                f.dones.push(m.created);
                last_done = Some(Delivery {
                    at: m.created,
                    pr: parse_evidence(&m.body).and_then(|e| pr_of(&e.pr)),
                });
            }
            (Some(Kind::Question), _) => match m.to.as_str() {
                "owner" => {
                    f.questions_owner.push(ev());
                    asked.push(m.created);
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

/// At least this many hex digits of a commit message name a merge commit.
pub const MIN_SHA_PREFIX: usize = 12;

fn is_sha(s: &str) -> bool {
    s.len() >= MIN_SHA_PREFIX && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The repo and the number of a PR URL (`.../<owner>/<repo>/pull/<n>`). The host is not checked:
/// the note is signed, and only the repo and the number are compared.
fn pr_of(url: &str) -> Option<(String, u64)> {
    let mut parts = url.trim().trim_end_matches('/').rsplit('/');
    let number = parts.next()?.parse().ok()?;
    if parts.next()? != "pull" {
        return None;
    }
    let (repo, owner) = (parts.next()?, parts.next()?);
    (!repo.is_empty() && !owner.is_empty()).then(|| (format!("{owner}/{repo}"), number))
}

/// A `done` of the assignee: its creation time, and the repo and the number of its PR.
struct Delivery {
    at: i64,
    pr: Option<(String, u64)>,
}

/// What the close note `body` records about the merge. `delivery` is the latest `done` before
/// the note: the window starts there. With `pr_of_delivery`, a note without a `pr:` line takes
/// the PR of that `done`.
fn merged_ref(
    log: &TicketLog,
    close: Event,
    body: &str,
    delivery: Option<&Delivery>,
    pr_of_delivery: bool,
) -> Option<MergedRef> {
    let pr = kv(body, "pr")
        .and_then(|url| pr_of(&url))
        .or_else(|| delivery.filter(|_| pr_of_delivery)?.pr.clone());
    let merge_commit = kv(body, "merge-commit")
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| is_sha(s));
    if pr.is_none() && merge_commit.is_none() {
        return None;
    }
    Some(MergedRef {
        since: delivery.map_or(close.at, |d| d.at.min(close.at)),
        close,
        repo: pr
            .as_ref()
            .map_or_else(|| log.ticket.repo.to_string(), |(repo, _)| repo.clone()),
        pr: pr.map(|(_, n)| n),
        merge_commit,
    })
}

/// What a commit message says that the commit reverts.
#[derive(Debug, Default, PartialEq, Eq)]
struct Reverts {
    /// The hex digits after each `This reverts commit `, anywhere in the message, in lower
    /// case.
    shas: Vec<String>,
    /// The PR of each line that starts with `Reverts <owner>/<repo>#<n>`, `Reverts #<n>` or
    /// `Reverts <PR URL>`. Without a repo, the PR is in the repo of the commit.
    prs: Vec<(Option<String>, u64)>,
}

fn reverts_named(message: &str) -> Reverts {
    const BY_SHA: &str = "This reverts commit ";
    let shas = message
        .match_indices(BY_SHA)
        .map(|(at, _)| {
            message[at + BY_SHA.len()..]
                .chars()
                .take_while(char::is_ascii_hexdigit)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|s| is_sha(s))
        .collect();
    let prs = message
        .lines()
        .filter_map(|l| {
            let word = l
                .trim()
                .strip_prefix("Reverts ")?
                .split_whitespace()
                .next()?
                .trim_end_matches(['.', ',', ')', ':', ';']);
            if word.contains("/pull/") {
                return pr_of(word).map(|(repo, n)| (Some(repo), n));
            }
            let (repo, n) = word.split_once('#')?;
            let n = n.parse().ok()?;
            match repo {
                "" => Some((None, n)),
                r if r.contains('/') => Some((Some(r.to_string()), n)),
                _ => None,
            }
        })
        .collect();
    Reverts { shas, prs }
}

/// Does a commit of `repo`, whose message names `named`, revert the merge `r` of the same
/// repo? The sha of the message is the merge commit or a prefix of it ([`MIN_SHA_PREFIX`]
/// digits or more).
fn reverts(named: &Reverts, r: &MergedRef, repo: &str, names: &RepoNames) -> bool {
    let by_sha = r
        .merge_commit
        .as_ref()
        .is_some_and(|full| named.shas.iter().any(|s| full.starts_with(s.as_str())));
    let by_pr = r.pr.is_some_and(|n| {
        named.prs.iter().any(|(of, m)| {
            *m == n
                && of.as_ref().is_none_or(|x| {
                    names
                        .current(x)
                        .is_some_and(|c| c.eq_ignore_ascii_case(repo))
                })
        })
    });
    by_sha || by_pr
}

/// The repos whose commits the report needs, in the order of `names.repos`: each repo of the
/// initiative with a merge, from the earliest start of such a merge.
fn windows(refs: &[&MergedRef], names: &RepoNames) -> Vec<Window> {
    names
        .repos
        .iter()
        .filter_map(|repo| {
            let since = refs
                .iter()
                .filter(|r| names.current(&r.repo) == Some(repo.as_str()))
                .map(|r| r.since)
                .min()?;
            Some(Window {
                repo: repo.clone(),
                since,
            })
        })
        .collect()
}

/// The outcome numbers. `lists` holds what the caller read for each of [`windows`]; a window
/// without a list counts as a failed read.
fn outcome(facts: &[TicketFacts], names: &RepoNames, lists: &[RepoCommits]) -> Outcome {
    let list_of = |repo: &str| lists.iter().find(|l| l.repo.eq_ignore_ascii_case(repo));
    // the commits that revert something, by repo
    let named: Vec<(&str, Vec<(&BaseCommit, Reverts)>)> = lists
        .iter()
        .filter_map(|l| {
            let commits = l.commits.as_ref().ok()?;
            let reverting = commits
                .iter()
                .map(|c| (c, reverts_named(&c.message)))
                .filter(|(_, n)| *n != Reverts::default())
                .collect();
            Some((l.repo.as_str(), reverting))
        })
        .collect();
    let mut out = Outcome::default();
    let refs: Vec<&MergedRef> = facts.iter().flat_map(|f| &f.merge_refs).collect();
    out.reverted.unread = windows(&refs, names)
        .into_iter()
        .filter_map(|w| {
            let fault = match list_of(&w.repo).map(|l| &l.commits) {
                Some(Ok(_)) => return None,
                Some(Err(fault)) => fault.clone(),
                None => ReadFault::Failed("no read".to_string()),
            };
            Some(Unread {
                repo: w.repo,
                fault,
                since: w.since,
            })
        })
        .collect();
    let (mut comments, mut waits) = (Vec::new(), Vec::new());
    for f in facts {
        if f.owner_comments > 0 {
            out.owner_touched += 1;
            comments.push(f.owner_comments as i64);
        }
        waits.extend(&f.owner_waits);
        if !f.merged {
            continue;
        }
        out.merged += 1;
        out.came_back.extend(f.came_back.clone());
        if f.merge_refs.is_empty() {
            continue;
        }
        // the merges in a repo of the initiative, each with the current name of its repo
        let inside: Vec<(&MergedRef, &str)> = f
            .merge_refs
            .iter()
            .filter_map(|r| Some((r, names.current(&r.repo)?)))
            .collect();
        if inside.is_empty() {
            out.reverted.outside += 1;
            continue;
        }
        // (the merge, the commits of its repo that revert something)
        let read: Vec<_> = inside
            .iter()
            .filter_map(|(r, repo)| {
                let (_, commits) = named.iter().find(|(l, _)| l.eq_ignore_ascii_case(repo))?;
                Some((*r, *repo, commits))
            })
            .collect();
        if read.is_empty() {
            // every merge of the ticket is in a repo that was not read
            continue;
        }
        out.reverted.known += 1;
        let earliest = read
            .iter()
            .flat_map(|(r, repo, commits)| {
                commits
                    .iter()
                    .filter(move |(_, n)| reverts(n, r, repo, names))
                    .map(move |(c, _)| (*c, *r))
            })
            .min_by_key(|(c, _)| c.at);
        if let Some((c, r)) = earliest {
            out.reverted.reverts.push(Revert {
                close: r.close.clone(),
                sha: c.sha.clone(),
                at: c.at,
            });
        }
    }
    out.owner_comments = spread(&comments);
    out.owner_wait = spread(&waits);
    out
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

/// The logs of a period, folded one time: the tickets with what their logs say.
pub struct Folded<'a> {
    /// The issues with a signed assignment of a coordinator.
    tickets: Vec<&'a TicketLog>,
    /// What the log of each ticket says, in the order of `tickets`.
    facts: Vec<TicketFacts>,
    merged_unassigned: Vec<Event>,
}

/// Fold `logs`: one entry per issue, each with its counted messages in comment order. An issue
/// without a signed assignment of a coordinator is no ticket and counts nowhere, with one
/// exception: its drop notice with a merged PR is in `merged_unassigned`, and only there.
pub fn fold(logs: &[TicketLog]) -> Folded<'_> {
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
    Folded {
        tickets,
        facts,
        merged_unassigned,
    }
}

impl Folded<'_> {
    /// The merges that the logs of the tickets record: what a revert is looked up for.
    pub fn merged_refs(&self) -> Vec<&MergedRef> {
        self.facts.iter().flat_map(|f| &f.merge_refs).collect()
    }

    /// The commits that [`Folded::report`] needs: one window for each repo of the initiative
    /// with a merge. A former name counts as its current repo.
    pub fn windows(&self, names: &RepoNames) -> Vec<Window> {
        windows(&self.merged_refs(), names)
    }

    /// The report. `lists` holds what the caller read for each of [`Folded::windows`].
    pub fn report(&self, names: &RepoNames, lists: &[RepoCommits]) -> Report {
        let (tickets, facts) = (&self.tickets, &self.facts);
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
            verify: verify_stats(facts),
            gates: gate_stats(facts),
            classes: class_rows(facts),
            tags: tag_rows(facts),
            rounds: rounds(tickets, facts),
            andon: andon(tickets, facts),
            refusals: refusal_rows(facts),
            flow: flow(tickets, facts),
            merged_unassigned: self.merged_unassigned.clone(),
            outcome: outcome(facts, names, lists),
        }
    }
}

#[cfg(test)]
#[path = "retro_tests.rs"]
mod tests;
