//! Late work (spec/events#stale-work): the GitHub side of the late waits, and the `poll`
//! duty.
//!
//! The rules are in [`crate::late`]. This file reads GitHub for them and counts the calls, so
//! `status` can show the cost.
//!
//! Calls (each only for a ticket that can hold the wait, and only if the wait was not given as an
//! event for this head SHA yet):
//!
//! - `push` (`no-push`): one `matching-refs` read per In Progress ticket over its time.
//! - `done` (`green-not-done`): one PR list per repo; for the PR of the ticket the CI reads of
//!   `fetch_ci` (4 or 5 reads; none if the green time of the SHA is in the cache); one read of the
//!   ticket comments when CI is green long enough (none in `status`, which has them).
//! - `ci-finished` (spec/events#stale-work): no kind of read of its own. It uses the PR list and
//!   the CI reads of `done`, which the probe keeps for the run, so one head is read once. A head
//!   that has its event is not read. Its throttle is its own: every [`CI_CHECK_EVERY_S`] while
//!   the agent has a ticket In Progress, so a short CI is not held back by the waits.
//! - `verify` (`review-not-verified`), `take`: none. They use the age of the board item.
//! - `answer`, `owner`, `owner-merge` (on by default; `off` in `[late]` stops them): one read of
//!   the ticket comments.
//! - the branch prefix: one read of AUTHORITY.md, only when a read above is needed.
//!
//! The coordinator hears of a dev wait when it is still there `escalate` after its time. It reads the waits of In Progress tickets that are older than `time + escalate`; a
//! younger ticket costs no read.
//!
//! `poll` reads GitHub for these at most once per [`CHECK_EVERY_S`].
//!
//! The owner hears through ONE signed `info` per report ([`late_owner_duty`]): the first line
//! `late: <wait> <subject>`, the line `late-key: <key>`. Whoever computes the late wait and finds
//! no `info` with that key in the log posts it ([`late_info_exists`]): the coordinator's `poll`,
//! the headless `tick`, or a running dev agent for a late coordinator. A second note from
//! another agent is accepted.

use super::say::{Posted, send_signed};
use super::{AgentCtx, CbResult, Ctx, World};
use crate::base::TicketRef;
use crate::comments::Comment;
use crate::ctx::is_coordinator;
use crate::domain::ci::evaluate::not_green_is_pending;
use crate::events::{Event, EventClass};
use crate::late::liveness::derive;
use crate::late::{
    AgentLate, CHECK_EVERY_S, CI_CHECK_EVERY_S, CI_FINISHED, CI_NONE_SEEN, CiFinished, CiState,
    Expect, Inputs, Late, OpenPr, OpenWait, Probe, Scope, Subject, VerifyState, Wait, Who,
    agent_late, ci_scan, coordinator_hears_again, expect, green_since_of, parse_open_prs,
    prune_seen, scan, tells, told_up_key,
};
use crate::protocol::names::{Kind, Re, To};
use crate::registry::Trust;
use crate::rules::Role;
use crate::rules::evidence::CiFacts;
use crate::rules::fields::Field;
use crate::rules::status::Status;
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::state::StateCache;
use crate::ticket::{
    Act, Sender, TicketRow, VMsg, answer_after_block, is_owner_route_done, kv, latest_done,
    question_open, verified_messages, verify_report,
};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

/// The default branch prefix when AUTHORITY.md does not say.
const DEFAULT_PREFIX: &str = "crew/";

/// The verified messages by ticket (`None`: could not be read).
type TicketMsgs = BTreeMap<TicketRef, Option<Vec<VMsg>>>;

pub struct GhProbe<'a> {
    ctx: &'a Ctx,
    trust: &'a Trust,
    /// Comments that the caller has read already (`status` reads the whole feed).
    comments: Option<&'a [Comment]>,
    calls: Cell<u32>,
    prefix: RefCell<Option<String>>,
    prs: RefCell<BTreeMap<String, Option<Vec<OpenPr>>>>,
    green: RefCell<BTreeMap<String, i64>>,
    /// The CI state of the heads read in this run.
    states: RefCell<BTreeMap<String, Option<CiState>>>,
    touched: RefCell<BTreeSet<String>>,
    /// The verified messages of the tickets read in this run, one read per ticket.
    msgs: RefCell<TicketMsgs>,
}

impl<'a> GhProbe<'a> {
    pub fn new(
        ctx: &'a Ctx,
        trust: &'a Trust,
        comments: Option<&'a [Comment]>,
        known_green: BTreeMap<String, i64>,
    ) -> Self {
        GhProbe {
            ctx,
            trust,
            comments,
            calls: Cell::new(0),
            prefix: RefCell::new(None),
            prs: RefCell::new(BTreeMap::new()),
            green: RefCell::new(known_green),
            states: RefCell::new(BTreeMap::new()),
            touched: RefCell::new(BTreeSet::new()),
            msgs: RefCell::new(BTreeMap::new()),
        }
    }

    /// The branch prefix, if a read needed it.
    pub fn branch_prefix_if_read(&self) -> Option<String> {
        self.prefix.borrow().clone()
    }

    /// GitHub reads made so far.
    pub fn calls(&self) -> u32 {
        self.calls.get()
    }

    /// The green times to keep: those of the SHAs that this run looked at.
    pub fn green_to_keep(&self) -> BTreeMap<String, i64> {
        let touched = self.touched.borrow();
        self.green
            .borrow()
            .iter()
            .filter(|(k, _)| touched.contains(*k))
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    fn read_ci(&self, repo: &str, pr: &OpenPr) -> Option<CiState> {
        let bodies: RefCell<Vec<Value>> = RefCell::new(Vec::new());
        let q = CiQuery {
            repo,
            sha: &pr.head_sha,
            base: &pr.base_ref,
            ..CiQuery::default()
        };
        let (facts, _) = fetch_ci(
            &|p| {
                let r = self.get(p)?;
                bodies.borrow_mut().push(r.body.clone());
                Ok(r)
            },
            &q,
        )
        .ok()?;
        match facts {
            CiFacts::Green { .. } => {
                // The time of the last green part may be unreadable: nothing is guessed.
                let t = green_since_of(&bodies.borrow(), &pr.head_sha);
                if let Some(t) = t {
                    self.green.borrow_mut().insert(pr.head_sha.clone(), t);
                }
                Some(CiState::Green(t))
            }
            CiFacts::NotQueried(_) => None,
            CiFacts::NoCi(_) | CiFacts::OnlyInformation { .. } => Some(CiState::NoCi),
            CiFacts::NotGreen(reasons) => Some(if not_green_is_pending(&reasons) {
                CiState::Pending
            } else {
                CiState::Red(reasons)
            }),
        }
    }

    fn get(&self, path: &str) -> Result<crate::github::Rest, crate::github::GhError> {
        self.calls.set(self.calls.get() + 1);
        self.ctx.client.get(path)
    }
}

impl Probe for GhProbe<'_> {
    fn branch_prefix(&self) -> String {
        if let Some(p) = self.prefix.borrow().as_ref() {
            return p.clone();
        }
        self.calls.set(self.calls.get() + 1);
        let p = crate::ctx::load_authority_quiet(self.ctx)
            .map_or_else(|| DEFAULT_PREFIX.to_string(), |a| a.branch_prefix());
        *self.prefix.borrow_mut() = Some(p.clone());
        p
    }

    fn has_branch(&self, repo: &str, prefix: &str, agent: &str, issue: u64) -> Option<bool> {
        let path = format!("repos/{repo}/git/matching-refs/heads/{prefix}{agent}/{issue}-");
        let r = self.get(&path).ok()?;
        Some(!r.body.as_array()?.is_empty())
    }

    fn open_prs(&self, repo: &str) -> Option<Vec<OpenPr>> {
        if let Some(c) = self.prs.borrow().get(repo) {
            return c.clone();
        }
        let got = self
            .get(&format!(
                "repos/{repo}/pulls?state=open&per_page=100&sort=updated&direction=desc"
            ))
            .ok()
            .and_then(|r| parse_open_prs(&r.body));
        self.prs.borrow_mut().insert(repo.to_string(), got.clone());
        got
    }

    fn ci_state(&self, repo: &str, pr: &OpenPr) -> Option<CiState> {
        self.touched.borrow_mut().insert(pr.head_sha.clone());
        if let Some(t) = self.green.borrow().get(&pr.head_sha) {
            return Some(CiState::Green(Some(*t)));
        }
        if let Some(st) = self.states.borrow().get(&pr.head_sha) {
            return st.clone();
        }
        let st = self.read_ci(repo, pr);
        self.states
            .borrow_mut()
            .insert(pr.head_sha.clone(), st.clone());
        st
    }

    fn has_done(&self, t: &TicketRef, agent: &str, sha: &str) -> Option<bool> {
        let msgs = self.ticket_msgs(t)?;
        let named = latest_done(&msgs, agent).is_some_and(|(_, ev)| {
            let (a, b) = (ev.sha.to_ascii_lowercase(), sha.to_ascii_lowercase());
            !a.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
        });
        Some(named)
    }

    fn verify_state(&self, t: &TicketRef, assignee: Option<&str>) -> Option<VerifyState> {
        let msgs = self.ticket_msgs(t)?;
        // the done of the assignee, as the tick takes it (a stray `done` of another sender or
        // with incomplete evidence does not count)
        let (done, _) = latest_done(&msgs, assignee?)?;
        if is_owner_route_done(done) {
            return Some(VerifyState::OwnerMerge);
        }
        Some(match verify_report(&msgs, done, t.issue) {
            Some((m, true)) => kv(&m.body, "head_now").map_or(VerifyState::Unverified, |head| {
                VerifyState::Verified {
                    head,
                    since: m.created,
                }
            }),
            _ => VerifyState::Unverified,
        })
    }

    fn question_open(&self, t: &TicketRef, assignee: Option<&str>) -> Option<OpenWait> {
        open_wait(&self.ticket_msgs(t)?, assignee)
    }
}

/// The wait of a Blocked ticket: the latest of its `blocked` message and its open question to the
/// owner, as [`answer_after_block`] takes it. A question that a later `blocked` message follows
/// is a `blocked` wait. `None`: the latest one has its answer.
fn open_wait(msgs: &[VMsg], assignee: Option<&str>) -> Option<OpenWait> {
    let blocked_at = msgs.iter().rposition(|m| m.kind == Some(Kind::Blocked));
    let question_at = question_open(msgs, assignee)
        .and_then(|q| msgs.iter().position(|m| m.comment_id == q.comment_id));
    if let Some(q) = question_at
        && blocked_at.is_none_or(|b| q > b)
    {
        return Some(OpenWait {
            created: msgs[q].created,
            to_owner: true,
        });
    }
    let b = blocked_at?;
    if answer_after_block(msgs, Status::Blocked, assignee) {
        return None;
    }
    Some(OpenWait {
        created: msgs[b].created,
        to_owner: false,
    })
}

impl GhProbe<'_> {
    /// The verified messages of one ticket: from the comments the caller read, else one read.
    fn ticket_msgs(&self, t: &TicketRef) -> Option<Vec<VMsg>> {
        if let Some(m) = self.msgs.borrow().get(t) {
            return m.clone();
        }
        let got = self.read_ticket_msgs(t);
        self.msgs.borrow_mut().insert(t.clone(), got.clone());
        got
    }

    fn read_ticket_msgs(&self, t: &TicketRef) -> Option<Vec<VMsg>> {
        let owned: Vec<Comment>;
        let all: &[Comment] = match self.comments {
            Some(c) => c,
            None => {
                self.calls.set(self.calls.get() + 1);
                owned = crate::feed::issue_comments(
                    &self.ctx.client,
                    &t.repo,
                    t.issue,
                    None,
                    &self.ctx.cfg.owner_login,
                )
                .ok()?
                .0;
                &owned
            }
        };
        let mine: Vec<Comment> = all
            .iter()
            .filter(|c| c.repo == t.repo && c.issue == t.issue)
            .cloned()
            .collect();
        Some(verified_messages(&mine, &self.ctx.cfg, self.trust))
    }
}

/// The agents whose heartbeat is judged: every approved agent, with its [`agent_since`] and its
/// liveness.
pub fn agent_inputs(ctx: &Ctx, world: &World, now: i64) -> Vec<AgentLate> {
    world
        .reg
        .agents
        .values()
        .filter(|e| e.approved())
        .map(|e| {
            let item = e.join.as_ref().and_then(|j| world.items.get(&j.item_id));
            let since = crate::ctx::agent_since(world, &e.name);
            AgentLate {
                name: e.name.clone(),
                coordinator: e
                    .approval
                    .as_ref()
                    .is_some_and(|a| a.role == Role::Coordinator),
                since,
                live: derive(
                    item.and_then(|i| i.get(Field::State)),
                    since,
                    now,
                    ctx.hb_interval,
                ),
            }
        })
        .collect()
}

/// The waits of the agents (`heartbeat`, `dead`) in this cycle.
pub fn agent_waits(ctx: &Ctx, world: &World, now: i64) -> Vec<Late> {
    agent_inputs(ctx, world, now)
        .iter()
        .filter_map(|a| agent_late(a, now, ctx.hb_interval))
        .collect()
}

/// The late coordinator among the waits of agents, if there is one.
pub fn late_coordinator(waits: &[Late]) -> Option<&Late> {
    waits.iter().find(|l| l.is_coordinator())
}

/// An `info` to the owner that is on the board for this report: a signed message of an agent
/// (a dev or the coordinator) to `owner`, first line `late: ...`, with the line `late-key: <key>`.
pub fn late_info_exists(msgs: &[VMsg], key: &str) -> bool {
    msgs.iter().any(|m| {
        m.to == "owner"
            && matches!(m.sender, Sender::Agent { .. })
            && matches!(m.act(), Act::Late { key: Some(k) } if k == key)
    })
}

/// What the note to the owner needs besides the late wait.
pub struct Note<'a> {
    pub expect: &'a Expect,
    /// The late coordinator (stale or dead), if the agent that computes the note sees one.
    pub coordinator: Option<&'a Late>,
    pub now: i64,
    /// Facts of the caller, added at the end of the note.
    pub detail: &'a str,
}

/// Tell the owner of `l` once: if `tells` names the owner and `msgs` (the log of the registry
/// issue for an agent, of the ticket for a ticket) holds no `info` with the key of this report,
/// post it. `Some` is the note that was posted.
pub fn late_owner_duty(
    ac: &AgentCtx,
    l: &Late,
    note: &Note<'_>,
    msgs: &[VMsg],
) -> CbResult<Option<Posted>> {
    let coordinator_since = note.coordinator.map(|c| c.since);
    if !tells(l, note.expect, note.coordinator.is_some(), note.now).contains(&Who::Owner) {
        return Ok(None);
    }
    let cfg = &ac.ctx.cfg;
    let Some(body) = l.owner_info(&cfg.owner_login, note.detail, coordinator_since) else {
        return Ok(None);
    };
    if late_info_exists(msgs, &l.report_key(coordinator_since)) {
        return Ok(None);
    }
    let (repo, issue, re) = match &l.subject {
        Subject::Agent { .. } => (cfg.main_repo().to_string(), cfg.registry_issue, Re::None),
        Subject::Ticket { repo, issue, .. } => (repo.clone(), *issue, Re::None),
    };
    send_signed(
        ac,
        &repo,
        issue,
        To::Owner,
        Kind::Info,
        re,
        &body,
        &crate::clock::format_ts(note.now),
    )
    .map(Some)
}

/// For the headless `tick`: the `verify` waits of the Review tickets in `rows` and the late
/// coordinator (stale or dead), if any. It reads nothing from GitHub.
pub fn review_lates(
    ctx: &Ctx,
    world: &World,
    rows: &[TicketRow],
    e: &Expect,
    now: i64,
) -> (Vec<Late>, Option<Late>) {
    let verify = rows
        .iter()
        .filter_map(|r| crate::late::verify_wait(r, e, now))
        .collect();
    let agents = agent_waits(ctx, world, now);
    (verify, late_coordinator(&agents).cloned())
}

/// What `status` shows: the late waits of every ticket and agent, the table and the number of
/// GitHub reads they cost. Nothing is cached.
pub fn status_late(
    ctx: &Ctx,
    world: &World,
    rows: &[TicketRow],
    comments: &[Comment],
    now: i64,
) -> StatusLate {
    let seen = BTreeMap::new();
    let e = expect(&ctx.cfg);
    let agents = agent_inputs(ctx, world, now);
    let probe = GhProbe::new(ctx, &world.trust, Some(comments), BTreeMap::new());
    let inp = Inputs {
        rows,
        expect: &e,
        now,
        scope: Scope::All,
        github: true,
        seen: &seen,
        agents: &agents,
        interval: ctx.hb_interval,
    };
    let found = scan(&inp, &probe);
    StatusLate {
        waits: found.late,
        verified: found.verified,
        expect: e,
        reads: probe.calls(),
    }
}

/// What [`status_late`] found.
pub struct StatusLate {
    /// The late waits of every ticket and agent.
    pub waits: Vec<Late>,
    /// Review tickets (item id, time of the passing verify) that wait for their merge.
    pub verified: Vec<(String, i64)>,
    /// The table of the waits.
    pub expect: Expect,
    /// The GitHub reads the scan cost.
    pub reads: u32,
}

fn late_event(
    ac: &AgentCtx,
    l: &Late,
    who: &Who,
    e: &Expect,
    prefix: &str,
    now_text: &str,
) -> Event {
    let (repo, issue) = (l.repo().unwrap_or_default(), l.issue().unwrap_or(0));
    Event {
        class: EventClass::Ticket,
        comment_id: 0,
        repo: repo.to_string(),
        issue,
        url: format!("https://github.com/{repo}/issues/{issue}"),
        author: "board".into(),
        from: "board".into(),
        to: ac.name.to_string(),
        kind: "stale-work".into(),
        re: "none".into(),
        ts: now_text.to_string(),
        verified: false,
        body: crate::events::decode_html_entities(&l.event_body(who, e, prefix)),
    }
}

fn ci_finished_event(ac: &AgentCtx, c: &CiFinished, now_text: &str) -> Event {
    Event {
        class: EventClass::Ticket,
        comment_id: 0,
        repo: c.repo.clone(),
        issue: c.issue,
        url: format!("https://github.com/{}/issues/{}", c.repo, c.issue),
        author: "board".into(),
        from: "board".into(),
        to: ac.name.to_string(),
        kind: "ci-finished".into(),
        re: "none".into(),
        ts: now_text.to_string(),
        verified: false,
        body: crate::events::decode_html_entities(&c.event_body()),
    }
}

/// The namespace of what the coordinator's `poll` was told; the headless `tick` has its own.
const TOLD_POLL: &str = "escalated";
const TOLD_TICK: &str = "escalated-tick";

/// Is the seen entry of what the coordinator was told old enough for another message? The entry
/// is `<value>|<time given>`; another value (a new head SHA) or `repeat` seconds make it due.
fn told_again(
    seen: &BTreeMap<String, String>,
    key: &str,
    value: &str,
    now: i64,
    repeat: i64,
) -> bool {
    let Some(entry) = seen.get(key) else {
        return true;
    };
    let Some((v, t)) = entry.rsplit_once('|') else {
        return true;
    };
    let Ok(t) = t.parse::<i64>() else {
        return true;
    };
    v != value || t > now || now - t >= repeat
}

/// The messages that are due for the agent `me`, and the seen entries they set. A dev agent
/// gets the waits of its own tickets once per value. The coordinator gets `verify` and `answer`
/// once, and a dev wait (`take`, `push`, `done`) when `told_again` says so.
fn due_events<'a>(
    found: &'a [Late],
    e: &Expect,
    me: &str,
    coordinator: bool,
    coordinator_late: bool,
    now: i64,
    seen: &mut BTreeMap<String, String>,
) -> Vec<(&'a Late, Who)> {
    let mut out = Vec::new();
    for l in found {
        let (Some(item), Some(key)) = (l.item_id(), l.key()) else {
            continue;
        };
        for who in tells(l, e, coordinator_late, now) {
            let due = match &who {
                Who::Dev(name) if name == me && !coordinator => {
                    let due = seen.get(&key) != Some(&l.value());
                    if due {
                        seen.insert(key.clone(), l.value());
                    }
                    due
                }
                Who::Coordinator if coordinator && !coordinator_hears_again(l.wait) => {
                    let due = seen.get(&key) != Some(&l.value());
                    if due {
                        seen.insert(key.clone(), l.value());
                    }
                    due
                }
                Who::Coordinator if coordinator => {
                    let up = told_up_key(item, l.wait, TOLD_POLL);
                    let due = told_again(seen, &up, &l.value(), now, e.repeat);
                    if due {
                        seen.insert(up, l.told_up_value(now));
                    }
                    due
                }
                _ => false,
            };
            if due {
                out.push((l, who));
            }
        }
    }
    out
}

/// The coordinator's `poll` tells the owner of the ticket waits that the owner must hear
/// ([`late_owner_duty`]). A failed read or post is logged and tried again in the next cycle.
fn tell_owner_of_tickets(ac: &AgentCtx, probe: &GhProbe<'_>, found: &[Late], e: &Expect, now: i64) {
    let note = Note {
        expect: e,
        coordinator: None,
        now,
        detail: "",
    };
    for l in found {
        let (Some(repo), Some(issue)) = (l.repo(), l.issue()) else {
            continue;
        };
        if !tells(l, e, false, now).contains(&Who::Owner) {
            continue;
        }
        let Some(msgs) = probe.ticket_msgs(&TicketRef::new(repo, issue)) else {
            continue;
        };
        match late_owner_duty(ac, l, &note, &msgs) {
            Ok(Some(p)) => eprintln!(
                "crewboard: {} is late ({}); told the owner ({}).",
                l.subject_name(),
                l.wait.word(),
                p.url
            ),
            Ok(None) => {}
            Err(err) => eprintln!(
                "crewboard: could not tell the owner of {}: {err}",
                l.subject_name()
            ),
        }
    }
}

/// The `poll` duty: one `stale-work` event per wait and head SHA for the agent that runs the
/// cycle. A dev agent gets `no-push`, `green-not-done` and the `take` wait of its own tickets.
/// The coordinator gets `review-not-verified` and the `answer` wait of every ticket once, and
/// the dev waits that went on `escalate` longer (`escalated: true`) again after `repeat`. What was
/// given is stored in `cache.late.late_seen`.
pub fn duty(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    cache: &mut StateCache,
    now: i64,
) -> Vec<Event> {
    let me = ac.name.to_string();
    let coordinator = is_coordinator(world, &me);
    let scope = if coordinator {
        Scope::Coordinator
    } else {
        Scope::Dev(&me)
    };
    let due = cache
        .late
        .late_checked
        .is_none_or(|t| now - t >= CHECK_EVERY_S || t > now);
    let e = expect(&ac.ctx.cfg);
    let probe = GhProbe::new(&ac.ctx, &world.trust, None, cache.late.late_green.clone());
    let seen_before = cache.late.late_seen.clone();
    let inp = Inputs {
        rows,
        expect: &e,
        now,
        scope,
        github: due,
        seen: &seen_before,
        agents: &[],
        interval: ac.ctx.hb_interval,
    };
    let found = scan(&inp, &probe);
    // `ci-finished` has its own, shorter throttle (it is what the dev agent waits for).
    let ci_due = due
        || cache
            .late
            .late_ci_checked
            .is_none_or(|t| now - t >= CI_CHECK_EVERY_S || t > now);
    // The coordinator that runs this cycle is running: it never counts itself as late.
    let coordinator_late = false;
    // `ci-finished` (spec/events#stale-work) goes after the waits: it uses their PR and CI reads.
    let ci = ci_scan(
        &Inputs {
            github: ci_due,
            ..inp
        },
        &probe,
    );
    if found.read {
        cache.late.late_checked = Some(now);
    }
    if ci.read {
        cache.late.late_ci_checked = Some(now);
    }
    if found.read || ci.read {
        cache.late.late_green = probe.green_to_keep();
    }
    let prefix = probe
        .branch_prefix_if_read()
        .unwrap_or_else(|| DEFAULT_PREFIX.to_string());
    let now_text = crate::clock::format_ts(now);
    // A wait that a read showed to be gone: forget what the coordinator was told about it.
    for (item, wait) in &found.clear {
        cache
            .late
            .late_seen
            .remove(&told_up_key(item, *wait, TOLD_POLL));
    }
    if coordinator && due {
        tell_owner_of_tickets(ac, &probe, &found.late, &e, now);
    }
    let mut events: Vec<Event> = due_events(
        &found.late,
        &e,
        &me,
        coordinator,
        coordinator_late,
        now,
        &mut cache.late.late_seen,
    )
    .into_iter()
    .map(|(l, who)| late_event(ac, l, &who, &e, &prefix, &now_text))
    .collect();
    for c in &ci.events {
        cache
            .late
            .late_seen
            .insert(format!("{}|{CI_FINISHED}", c.item_id), c.sha.clone());
        events.push(ci_finished_event(ac, c, &now_text));
    }
    for (item, sha) in &ci.none_seen {
        cache
            .late
            .late_seen
            .insert(format!("{item}|{CI_NONE_SEEN}"), sha.clone());
    }
    prune_seen(&mut cache.late.late_seen, rows);
    // keep the events in a stable order for the output
    events.sort_by_key(|e| (e.repo.clone(), e.issue, e.body.clone()));
    events
}

/// The headless `tick`: one log line per dev wait that went on past `escalate`,
/// nothing written on the board or GitHub. It keeps its own seen entries and its own read
/// throttle (`late_tick_checked`) in the state cache, apart from those of `poll`, so a line
/// comes once per wait and head SHA, and again after `repeat`.
pub fn tick_lines(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    cache: &mut StateCache,
    now: i64,
) -> String {
    let due = cache
        .late
        .late_tick_checked
        .is_none_or(|t| now - t >= CHECK_EVERY_S || t > now);
    let e = expect(&ac.ctx.cfg);
    let probe = GhProbe::new(&ac.ctx, &world.trust, None, cache.late.late_green.clone());
    let seen_before = cache.late.late_seen.clone();
    let inp = Inputs {
        rows,
        expect: &e,
        now,
        scope: Scope::Coordinator,
        github: due,
        seen: &seen_before,
        agents: &[],
        interval: ac.ctx.hb_interval,
    };
    let found = scan(&inp, &probe);
    if !found.read {
        return String::new();
    }
    cache.late.late_tick_checked = Some(now);
    cache.late.late_green = probe.green_to_keep();
    for (item, wait) in &found.clear {
        cache
            .late
            .late_seen
            .remove(&told_up_key(item, *wait, TOLD_TICK));
    }
    let mut out = String::new();
    for l in &found.late {
        // The tick acts on `review-not-verified` itself; the other waits are left to the
        // coordinator.
        if l.wait == Wait::Verify || !tells(l, &e, false, now).contains(&Who::Coordinator) {
            continue;
        }
        let Some(item) = l.item_id() else { continue };
        let up = told_up_key(item, l.wait, TOLD_TICK);
        if told_again(&cache.late.late_seen, &up, &l.value(), now, e.repeat) {
            cache.late.late_seen.insert(up, l.told_up_value(now));
            out.push_str(&l.tick_line());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000;

    fn late_of(wait: Wait, agent: &str, n: u64, since_ago: i64, sha: Option<&str>) -> Late {
        Late {
            wait,
            subject: crate::late::Subject::Ticket {
                key: format!("o/r#{n}"),
                repo: "o/r".into(),
                issue: n,
                item_id: format!("PVTI_{n}"),
                agent: Some(agent.into()),
            },
            since: T0 - since_ago,
            age: since_ago,
            expected: 1800,
            sha: sha.map(str::to_string),
            pr: None,
        }
    }

    fn vmsg(
        id: u64,
        sender: crate::ticket::Sender,
        to: &str,
        kind: Option<Kind>,
        body: &str,
    ) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: id,
            created: T0 + id as i64,
            sender,
            to: to.into(),
            kind,
            re: crate::protocol::names::Re::None,
            body: body.into(),
        }
    }

    fn dev() -> crate::ticket::Sender {
        crate::ticket::Sender::Agent {
            name: "dev@host-a".into(),
            role: crate::rules::Role::Dev,
        }
    }

    fn coordinator() -> crate::ticket::Sender {
        crate::ticket::Sender::Agent {
            name: "coord@host-b".into(),
            role: crate::rules::Role::Coordinator,
        }
    }

    #[test]
    fn the_open_wait_is_the_latest_of_the_block_and_the_question() {
        let a = Some("dev@host-a");
        let q = vmsg(1, dev(), "owner", Some(Kind::Question), "which layout?");
        let b = vmsg(2, dev(), "coordinator", Some(Kind::Blocked), "need creds");
        let answer = vmsg(3, coordinator(), "dev@host-a", Some(Kind::Answer), "here");
        let owner = vmsg(4, crate::ticket::Sender::Owner, "coordinator", None, "ok");
        // a question alone waits for the owner
        let w = open_wait(std::slice::from_ref(&q), a).unwrap();
        assert!(w.to_owner && w.created == q.created);
        // a block alone waits for an answer
        let w = open_wait(std::slice::from_ref(&b), a).unwrap();
        assert!(!w.to_owner && w.created == b.created);
        // a block after the question ends the owner wait: it is a block wait now
        let w = open_wait(&[q.clone(), b.clone()], a).unwrap();
        assert!(!w.to_owner && w.created == b.created);
        // a coordinator answer then ends the answer wait
        assert_eq!(open_wait(&[q.clone(), b.clone(), answer.clone()], a), None);
        // a question after the block is an owner wait; a plain coordinator answer does not end it
        let w = open_wait(&[b.clone(), q.clone()], a).unwrap();
        assert!(w.to_owner && w.created == q.created);
        assert!(
            open_wait(&[b.clone(), q.clone(), answer], a)
                .unwrap()
                .to_owner
        );
        // the owner ends both
        assert_eq!(open_wait(&[b, q, owner], a), None);
        assert_eq!(open_wait(&[], a), None);
    }

    #[test]
    fn told_again_follows_the_repeat_and_a_new_value() {
        let mut seen = BTreeMap::new();
        let key = "PVTI_1|no-push|escalated";
        // nothing stored, or an entry that cannot be read: due
        assert!(told_again(&seen, key, "-", T0, 7200));
        seen.insert(key.into(), "garbage".into());
        assert!(told_again(&seen, key, "-", T0, 7200));
        seen.insert(key.into(), format!("-|{T0}"));
        // same value: due only after `repeat`
        assert!(!told_again(&seen, key, "-", T0 + 7199, 7200));
        assert!(told_again(&seen, key, "-", T0 + 7200, 7200));
        // a new head SHA is due at once
        seen.insert(key.into(), format!("aaa|{T0}"));
        assert!(told_again(&seen, key, "bbb", T0 + 1, 7200));
        assert!(!told_again(&seen, key, "aaa", T0 + 1, 7200));
        // a time in the future (clock moved back) is due
        assert!(told_again(&seen, key, "aaa", T0 - 10, 7200));
    }

    #[test]
    fn the_duty_routes_each_wait_to_the_one_that_hears_it() {
        let e = Expect::defaults();
        let found = [
            late_of(Wait::Push, "dev@h", 1, 4500, None), // dev and coordinator
            late_of(Wait::Push, "dev@h", 2, 2000, None), // dev only
            late_of(Wait::Verify, "dev@h", 3, 2000, None), // coordinator only
        ];
        let mut seen = BTreeMap::new();
        let who = |due: Vec<(&Late, Who)>| -> Vec<(String, Who)> {
            due.into_iter()
                .map(|(l, w)| (l.subject_name(), w))
                .collect()
        };
        // the dev: its two push waits, once
        let dev = who(due_events(&found, &e, "dev@h", false, false, T0, &mut seen));
        assert_eq!(
            dev,
            [
                ("o/r#1".to_string(), Who::Dev("dev@h".into())),
                ("o/r#2".to_string(), Who::Dev("dev@h".into())),
            ]
        );
        assert!(due_events(&found, &e, "dev@h", false, false, T0, &mut seen).is_empty());
        // another dev hears nothing
        let mut other = BTreeMap::new();
        assert!(due_events(&found, &e, "other@h", false, false, T0, &mut other).is_empty());
        // the coordinator: the escalated push of ticket 1 and the verify of ticket 3
        let mut seen = BTreeMap::new();
        let coord = who(due_events(&found, &e, "boss@h", true, false, T0, &mut seen));
        assert_eq!(
            coord,
            [
                ("o/r#1".to_string(), Who::Coordinator),
                ("o/r#3".to_string(), Who::Coordinator),
            ]
        );
        assert!(due_events(&found, &e, "boss@h", true, false, T0 + 60, &mut seen).is_empty());
        // verify comes once; the dev wait again after `repeat`
        let again = who(due_events(
            &found,
            &e,
            "boss@h",
            true,
            false,
            T0 + 7200,
            &mut seen,
        ));
        // (ticket 2 is old enough for the coordinator by then)
        assert_eq!(
            again,
            [
                ("o/r#1".to_string(), Who::Coordinator),
                ("o/r#2".to_string(), Who::Coordinator)
            ]
        );
        // a coordinator that owns the ticket does not get the dev event of it
        let mut seen = BTreeMap::new();
        let own = who(due_events(&found, &e, "dev@h", true, false, T0, &mut seen));
        assert!(own.iter().all(|(_, w)| *w == Who::Coordinator), "{own:?}");
    }

    fn note(id: u64, from: Sender, to: &str, kind: Kind, body: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: id,
            created: T0,
            sender: from,
            to: to.into(),
            kind: Some(kind),
            re: Re::None,
            body: body.into(),
        }
    }

    fn sender(name: &str, role: Role) -> Sender {
        Sender::Agent {
            name: name.into(),
            role,
        }
    }

    #[test]
    fn late_info_exists_finds_the_key_from_a_dev_or_the_coordinator() {
        let body = "late: heartbeat c@h\nlate-key: heartbeat:c@h:100\nsince: x\n";
        let key = "heartbeat:c@h:100";
        // the same key, from a dev and from the coordinator
        for role in [Role::Dev, Role::Coordinator] {
            let m = note(1, sender("a@h", role), "owner", Kind::Info, body);
            assert!(late_info_exists(&[m], key), "{role:?}");
        }
        let m = note(1, sender("a@h", Role::Dev), "owner", Kind::Info, body);
        // another key (another episode of the same agent, or another wait)
        assert!(!late_info_exists(
            std::slice::from_ref(&m),
            "heartbeat:c@h:101"
        ));
        assert!(!late_info_exists(std::slice::from_ref(&m), "dead:c@h:100"));
        // a prefix of the key is not the key
        assert!(!late_info_exists(
            std::slice::from_ref(&m),
            "heartbeat:c@h:10"
        ));
        // not an info, not to the owner, not from an agent, no `late:` first line
        let other_kind = note(2, sender("a@h", Role::Dev), "owner", Kind::Report, body);
        let to_all = note(3, sender("a@h", Role::Dev), "all", Kind::Info, body);
        let from_owner = note(4, Sender::Owner, "owner", Kind::Info, body);
        let no_head = note(
            5,
            sender("a@h", Role::Dev),
            "owner",
            Kind::Info,
            "coordinator-stale: c@h\nlate-key: heartbeat:c@h:100\n",
        );
        for m in [other_kind, to_all, from_owner, no_head] {
            assert!(!late_info_exists(&[m], key));
        }
        assert!(!late_info_exists(&[], key));
    }

    #[test]
    fn an_owner_comment_with_the_key_does_not_count_as_a_note() {
        let body = "late: heartbeat c@h\nlate-key: heartbeat:c@h:100\n";
        let m = note(1, Sender::Owner, "owner", Kind::Info, body);
        assert!(!late_info_exists(&[m], "heartbeat:c@h:100"));
    }
}
