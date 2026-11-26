//! Late work (spec/events#stale-work): one table of expected times, one
//! function that finds what waits longer than expected, and one function that says who must
//! hear of it.
//!
//! - [`Expect`]: how long each wait may last. From `[late]` of `.crewboard.toml`, else `[stale]`,
//!   else the defaults.
//! - [`late`]: the waits that are over their expected time. `no-push` ([`Wait::Push`]),
//!   `green-not-done` ([`Wait::Done`]) and `review-not-verified` ([`Wait::Verify`]) have defaults,
//!   and so have `answer` (4 h), `owner` and `owner-merge` (1 day). The wait `take` is off until
//!   the table gives it a time. A key set to `off` turns a wait off.
//! - [`tells`]: who hears of one late wait now: the dev, the coordinator or the owner.
//! - [`AgentLate`] and [`agent_late`]: the waits of an agent: a stale heartbeat
//!   ([`Wait::Heartbeat`]) and a dead agent ([`Wait::Dead`]).
//!
//! This file is pure: the GitHub reads sit behind [`Probe`], and `now` is a parameter. The real
//! probe is `ops::late`. `status` shows every late wait; `poll` gives each one as ONE event per
//! wait and head SHA (the seen map of the state cache decides what is new).

use crate::base::TicketRef;
use crate::config::BoardConfig;
use crate::rules::status::Status;
use crate::ticket::TicketRow;
use liveness::{DEAD_INTERVALS, LATE_INTERVALS, Liveness};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) mod liveness;

/// Default expected times in seconds: 2 h, 30 min, 30 min.
pub const DEFAULT_PUSH_S: i64 = 7200;
pub const DEFAULT_DONE_S: i64 = 1800;
pub const DEFAULT_VERIFY_S: i64 = 1800;
/// Default expected times of the waits for a person: a question without an answer (4 h) and a
/// wait for the owner, a question or a merge (1 day). `take` has no default: it is off.
pub const DEFAULT_ANSWER_S: i64 = 14_400;
pub const DEFAULT_OWNER_S: i64 = 86_400;
use crate::config::LATE_OFF as OFF;
/// The coordinator hears of a dev wait when it is still there this long after its expected
/// time: a fixed 30 min for every wait.
pub const DEFAULT_ESCALATE_S: i64 = 1800;
/// The coordinator hears of the same wait again after this long, if it is still there.
pub const DEFAULT_REPEAT_S: i64 = 7200;
/// `poll` reads GitHub for the waits at most once per this many seconds.
pub const CHECK_EVERY_S: i64 = 300;
/// `poll` reads GitHub for `ci-finished` at most once per this many seconds, while the agent has
/// a ticket In Progress. It is apart from [`CHECK_EVERY_S`]: a CI of ten seconds must not wait
/// five minutes for the next read that the waits allow.
pub const CI_CHECK_EVERY_S: i64 = 60;

/// The table of expected times, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect {
    /// In Progress, no crew branch on GitHub.
    pub push: i64,
    /// In Progress, an open PR with green CI, no `done` for its head.
    pub done: i64,
    /// Review, no passed verify.
    pub verify: i64,
    /// Extra time after the expected time of a dev wait before the coordinator hears.
    pub escalate: i64,
    /// The coordinator hears of the same wait again after this long.
    pub repeat: i64,
    /// Ready and assigned, not taken. `None`: off (the default).
    pub take: Option<i64>,
    /// Blocked, the question has no answer. `None`: off (`answer = "off"`). Default 4 h.
    pub answer: Option<i64>,
    /// A question waits for the owner, or Review waits for an owner merge. `None`: off
    /// (`owner = "off"`). Default 1 day.
    pub owner: Option<i64>,
}

impl Expect {
    pub fn defaults() -> Expect {
        Expect {
            push: DEFAULT_PUSH_S,
            done: DEFAULT_DONE_S,
            verify: DEFAULT_VERIFY_S,
            escalate: DEFAULT_ESCALATE_S,
            repeat: DEFAULT_REPEAT_S,
            take: None,
            answer: Some(DEFAULT_ANSWER_S),
            owner: Some(DEFAULT_OWNER_S),
        }
    }
}

/// The table of the initiative: `[late]`, else `[stale]` (`no_push` is `push`, `green` is `done`,
/// `review` is `verify`), else the defaults.
pub fn expect(cfg: &BoardConfig) -> Expect {
    let mut e = Expect::defaults();
    let get = |v: &Option<String>| {
        v.as_deref()
            .and_then(|t| crate::clock::parse_interval(t).ok())
    };
    if let Some(l) = &cfg.late {
        e.push = get(&l.push).unwrap_or(e.push);
        e.done = get(&l.done).unwrap_or(e.done);
        e.verify = get(&l.verify).unwrap_or(e.verify);
        e.escalate = get(&l.escalate).unwrap_or(e.escalate);
        e.repeat = get(&l.repeat).unwrap_or(e.repeat);
        // Not set: the default. `off`: no wait. A bad value is refused when the file loads.
        let opt = |v: &Option<String>, default: Option<i64>| match v.as_deref() {
            None => default,
            Some(t) if t.eq_ignore_ascii_case(OFF) => None,
            Some(_) => get(v).or(default),
        };
        e.take = opt(&l.take, e.take);
        e.answer = opt(&l.answer, e.answer);
        e.owner = opt(&l.owner, e.owner);
    } else if let Some(s) = &cfg.stale {
        e.push = get(&s.no_push).unwrap_or(e.push);
        e.done = get(&s.green).unwrap_or(e.done);
        e.verify = get(&s.review).unwrap_or(e.verify);
        e.escalate = get(&s.escalate).unwrap_or(e.escalate);
        e.repeat = get(&s.repeat).unwrap_or(e.repeat);
    }
    e
}

/// What a late thing waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Wait {
    /// Ready and assigned, not taken.
    Take,
    /// In Progress, no crew branch on GitHub.
    Push,
    /// In Progress, green CI on an open PR, no `done` for its head.
    Done,
    /// Review, no passed verify.
    Verify,
    /// Review, and the done names protected paths or an owner route: the owner merges.
    OwnerMerge,
    /// Blocked, the `blocked` message has no answer.
    Answer,
    /// Blocked, a question to the owner has no answer.
    Owner,
    /// A running agent wrote no heartbeat for over 3 intervals.
    Heartbeat,
    /// A running agent wrote no heartbeat for over 5 intervals.
    Dead,
}

impl Wait {
    /// The word of the table and of `[late]`: `push`, `done`, `verify`, ...
    pub fn word(self) -> &'static str {
        match self {
            Wait::Take => "take",
            Wait::Push => "push",
            Wait::Done => "done",
            Wait::Verify => "verify",
            Wait::OwnerMerge => "owner-merge",
            Wait::Answer => "answer",
            Wait::Owner => "owner",
            Wait::Heartbeat => "heartbeat",
            Wait::Dead => "dead",
        }
    }

    /// The word of the event and of the seen map: `no-push`, `green-not-done`,
    /// `review-not-verified`, else the word of the table.
    pub fn case(self) -> &'static str {
        match self {
            Wait::Push => "no-push",
            Wait::Done => "green-not-done",
            Wait::Verify => "review-not-verified",
            other => other.word(),
        }
    }
}

/// What is late.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    Ticket {
        /// `owner/repo#n`.
        key: String,
        repo: String,
        issue: u64,
        item_id: String,
        agent: Option<String>,
    },
    /// An agent, by its name. `coordinator`: it is the coordinator.
    Agent { name: String, coordinator: bool },
}

/// One late wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Late {
    pub wait: Wait,
    pub subject: Subject,
    /// Unix seconds: since when the wait holds. For a ticket the `updatedAt` of its board item;
    /// for `done` the time the CI turned green.
    pub since: i64,
    /// How long the wait holds, in seconds.
    pub age: i64,
    /// The expected time of the wait, in seconds.
    pub expected: i64,
    /// The head SHA (`done`).
    pub sha: Option<String>,
    /// `owner/repo#n` of the PR (`done`).
    pub pr: Option<String>,
}

/// Who must hear of a late wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    Dev(String),
    Coordinator,
    Owner,
}

impl Who {
    /// `dev <name>`, `coordinator` or `owner`.
    pub fn label(&self) -> String {
        match self {
            Who::Dev(n) => format!("dev {n}"),
            Who::Coordinator => "coordinator".into(),
            Who::Owner => "owner".into(),
        }
    }
}

/// The age at which the coordinator hears of a wait with this expected time: `expected` plus
/// `escalate` for a dev wait (`take`, `push`, `done`), `expected` for `verify` and `answer`.
/// `None`: the coordinator never hears of it (`owner`, `owner-merge`).
pub fn coordinator_hears_at(wait: Wait, expected: i64, escalate: i64) -> Option<i64> {
    match wait {
        Wait::Take | Wait::Push | Wait::Done => Some(expected + escalate),
        Wait::Verify | Wait::Answer => Some(expected),
        Wait::Owner | Wait::OwnerMerge | Wait::Heartbeat | Wait::Dead => None,
    }
}

/// Does the coordinator hear of this wait again after `repeat`? A dev wait does; `verify` and
/// `answer` come once.
pub fn coordinator_hears_again(wait: Wait) -> bool {
    matches!(wait, Wait::Take | Wait::Push | Wait::Done)
}

/// Who must hear of `l` now, by its age at `now`:
///
/// | Wait | Dev | Coordinator | Owner |
/// |---|---|---|---|
/// | take, push, done | at the expected time | after `escalate` more | never |
/// | verify | never | at the expected time | at the expected time, if the coordinator is late |
/// | owner-merge, owner | never | never | at the expected time |
/// | answer | never | at the expected time | after `escalate` more |
/// | heartbeat of the coordinator | never | never | at the expected time (3 intervals) |
/// | heartbeat of a dev agent | never | never (it is only shown) | never |
/// | dead | never | the reassign (an action, no message) | at once |
///
/// `coordinator_late`: the coordinator is stale or dead (see [`coordinator_is_late`]). Pure. The
/// memory of what was told already (and the repeat) is the seen map of the caller, and for the
/// owner the signed `info` on the board.
pub fn tells(l: &Late, e: &Expect, coordinator_late: bool, now: i64) -> Vec<Who> {
    let age = now - l.since;
    let mut out = Vec::new();
    if matches!(l.wait, Wait::Take | Wait::Push | Wait::Done)
        && age > l.expected
        && let Some(a) = l.agent()
    {
        out.push(Who::Dev(a.to_string()));
    }
    if coordinator_hears_at(l.wait, l.expected, e.escalate).is_some_and(|t| age > t) {
        out.push(Who::Coordinator);
    }
    let owner = match l.wait {
        Wait::Verify => coordinator_late && age > l.expected,
        Wait::OwnerMerge | Wait::Owner => age > l.expected,
        Wait::Answer => age > l.expected + e.escalate,
        Wait::Heartbeat => l.is_coordinator() && age >= l.expected,
        // A `dead` late exists only when the heartbeat is over 5 intervals old.
        Wait::Dead => true,
        Wait::Take | Wait::Push | Wait::Done => false,
    };
    if owner {
        out.push(Who::Owner);
    }
    out
}

/// Is the coordinator stale or dead in `found`?
pub fn coordinator_is_late(found: &[Late]) -> bool {
    found
        .iter()
        .any(|l| matches!(l.wait, Wait::Heartbeat | Wait::Dead) && l.is_coordinator())
}

/// What `late` needs to know of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentLate {
    pub name: String,
    pub coordinator: bool,
    /// Since when the agent shows no sign of life: its last heartbeat, else the newer of its
    /// approval and its start. `None`: no baseline.
    pub since: Option<i64>,
    /// The judgement of [`crate::late::liveness::derive`] with `since` as the heartbeat.
    pub live: Liveness,
}

/// The wait of one agent: `heartbeat` when it is late, `dead` when it is dead, else none.
/// `interval` is the heartbeat interval in seconds.
pub fn agent_late(a: &AgentLate, now: i64, interval: i64) -> Option<Late> {
    // The coordinator is never "dead": nobody reassigns its tickets, and the owner note for a
    // silent coordinator (restart the session) is the `heartbeat` one at any age.
    let (wait, expected) = match &a.live {
        Liveness::Late { .. } => (Wait::Heartbeat, LATE_INTERVALS * interval),
        Liveness::Dead { .. } if a.coordinator => (Wait::Heartbeat, LATE_INTERVALS * interval),
        Liveness::Dead { .. } => (Wait::Dead, DEAD_INTERVALS * interval),
        Liveness::NotRunning | Liveness::Alive { .. } => return None,
    };
    let since = a.since?;
    Some(Late {
        wait,
        subject: Subject::Agent {
            name: a.name.clone(),
            coordinator: a.coordinator,
        },
        since,
        age: now - since,
        expected,
        sha: None,
        pr: None,
    })
}

/// The key in the seen map: one entry per ticket and wait.
pub fn seen_key(item_id: &str, wait: Wait) -> String {
    format!("{item_id}|{}", wait.case())
}

/// The seen key of what the coordinator was told: one entry per ticket and wait. `ns` keeps the
/// entries of `poll` apart from those that the headless `tick` logged.
pub fn told_up_key(item_id: &str, wait: Wait, ns: &str) -> String {
    format!("{item_id}|{}|{ns}", wait.case())
}

mod ci;
mod render;
mod scan;

pub use ci::*;
pub use scan::*;

#[cfg(test)]
#[path = "unit_tests.rs"]
mod tests;
