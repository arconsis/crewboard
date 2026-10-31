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
use crate::protocol::acts::{LATE_KEY, LATE_PREFIX};
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

impl Late {
    fn ticket(&self) -> Option<(&str, &str, u64, &str, Option<&str>)> {
        match &self.subject {
            Subject::Ticket {
                key,
                repo,
                issue,
                item_id,
                agent,
            } => Some((key, repo, *issue, item_id, agent.as_deref())),
            Subject::Agent { .. } => None,
        }
    }

    /// `owner/repo#n` of the ticket, or the name of the agent.
    pub fn subject_name(&self) -> String {
        match &self.subject {
            Subject::Ticket { key, .. } => key.clone(),
            Subject::Agent { name, .. } => name.clone(),
        }
    }

    /// Is the subject the coordinator?
    pub fn is_coordinator(&self) -> bool {
        matches!(
            &self.subject,
            Subject::Agent {
                coordinator: true,
                ..
            }
        )
    }

    /// The key of one report to the owner: `<wait>:<ticket or agent>:<since>`. For `dead`
    /// `since` is the heartbeat value the agent was last seen with, so one dead episode has one
    /// key, and an agent that comes back and dies again gets a new one. For `verify`
    /// the key ends with `:<coordinator_since>` (the baseline of the late coordinator), so a
    /// coordinator that comes back and goes away again gives a new report for the same ticket.
    pub fn report_key(&self, coordinator_since: Option<i64>) -> String {
        let mut key = format!(
            "{}:{}:{}",
            self.wait.word(),
            self.subject_name(),
            self.since
        );
        if self.wait == Wait::Verify
            && let Some(c) = coordinator_since
        {
            key.push_str(&format!(":{c}"));
        }
        key
    }

    pub fn repo(&self) -> Option<&str> {
        self.ticket().map(|t| t.1)
    }

    pub fn issue(&self) -> Option<u64> {
        self.ticket().map(|t| t.2)
    }

    pub fn item_id(&self) -> Option<&str> {
        self.ticket().map(|t| t.3)
    }

    pub fn agent(&self) -> Option<&str> {
        self.ticket().and_then(|t| t.4)
    }

    /// The key in the seen map.
    pub fn key(&self) -> Option<String> {
        self.item_id().map(|i| seen_key(i, self.wait))
    }

    /// What distinguishes two events of the same wait: the head SHA, else `-`.
    pub fn value(&self) -> String {
        self.sha.clone().unwrap_or_else(|| "-".into())
    }

    /// The value of the seen entry of what the coordinator was told: the head SHA (or `-`) and
    /// the time given.
    pub fn told_up_value(&self, now: i64) -> String {
        format!("{}|{now}", self.value())
    }

    pub fn to_json(&self, e: &Expect, coordinator_late: bool, now: i64) -> Value {
        let told: Vec<String> = tells(self, e, coordinator_late, now)
            .iter()
            .map(Who::label)
            .collect();
        json!({
            "wait": self.wait.word(),
            "subject": self.subject_name(),
            "since": crate::clock::format_ts(self.since),
            "age_s": self.age,
            "expected_s": self.expected,
            "head_sha": self.sha,
            "pr": self.pr,
            "told": told,
        })
    }

    /// One line of the `late:` section of `status`.
    pub fn status_line(&self, e: &Expect, coordinator_late: bool, now: i64) -> String {
        let told = tells(self, e, coordinator_late, now)
            .iter()
            .map(Who::label)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "  {:<11} {:<34} since {}  age {}  expected {}  told: {}\n",
            self.wait.word(),
            self.subject_name(),
            crate::clock::format_ts(self.since),
            crate::clock::fmt_age(self.age),
            crate::clock::fmt_age(self.expected),
            if told.is_empty() { "nobody yet" } else { &told }
        )
    }

    /// The first line of an event body: `stale-work: <case>` for `push`, `done` and `verify`,
    /// else `late: <wait>`.
    fn first_line(&self) -> String {
        match self.wait {
            Wait::Push | Wait::Done | Wait::Verify => format!("stale-work: {}", self.wait.case()),
            other => format!("late: {}", other.word()),
        }
    }

    /// The text of the event for the one that must act. `prefix` is the crew branch prefix.
    pub fn event_body(&self, to: &Who, e: &Expect, prefix: &str) -> String {
        if matches!(to, Who::Coordinator) && coordinator_hears_again(self.wait) {
            return self.escalated_body(e);
        }
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let head = format!("{}\nticket: {}\n", self.first_line(), self.subject_name());
        match self.wait {
            Wait::Push => format!(
                "{head}It is In Progress for {age} (limit {limit}) and no branch `{prefix}{}/{}-...` is on GitHub.\n\
                 Push your work now, then run `done`. If you cannot finish, run `block` and say why.\n",
                self.agent().unwrap_or("<agent>"),
                self.issue().unwrap_or(0)
            ),
            Wait::Done => format!(
                "{head}PR {} has green CI on head {} for {age} (limit {limit}) and no `done` names that head.\n\
                 Run `done` now. If the work is not finished, push it or run `block` and say why.\n",
                self.pr.as_deref().unwrap_or("?"),
                self.sha
                    .as_deref()
                    .map_or("?".to_string(), |s| s.chars().take(7).collect())
            ),
            Wait::Verify => format!(
                "{head}It is in Review for {age} (limit {limit}) and its latest `done` has no passing `verify` on the current PR head.\n\
                 Run `verify` now. If you cannot, tell the owner why. A ticket that passed `verify` on the current head is not reported: if you hold its merge, nothing is late.\n"
            ),
            Wait::Take => format!(
                "{head}It is Ready for {age} (expected {limit}) and was not taken.\n\
                 Run `take` now. If you cannot, tell the coordinator why.\n"
            ),
            Wait::Answer => format!(
                "{head}It is Blocked for {age} (expected {limit}) and its question has no answer.\n\
                 Answer it, or tell the owner why you cannot.\n"
            ),
            Wait::OwnerMerge | Wait::Owner | Wait::Heartbeat | Wait::Dead => {
                format!("{head}It waits for {age} (expected {limit}).\n")
            }
        }
    }

    /// What clears the wait, for the `next:` line of the note to the owner.
    /// `None` for the waits that the owner never hears of (`take`, `push`, `done`).
    pub fn next_step(&self) -> Option<&'static str> {
        Some(match self.wait {
            Wait::Verify => "the interactive coordinator verifies, or the owner decides",
            Wait::OwnerMerge => "the owner reviews and merges the PR",
            Wait::Owner => "the owner answers the question on the ticket",
            Wait::Answer => "the coordinator answers, or the owner decides",
            Wait::Heartbeat if self.is_coordinator() => {
                "restart the coordinator session, or approve another agent with `crewboard approve <agent> --role coordinator`"
            }
            Wait::Heartbeat => {
                "the agent writes a heartbeat again, or the coordinator reassigns its tickets once it is dead"
            }
            Wait::Dead => "restart the agent session; its open tickets are back in Ready",
            Wait::Take | Wait::Push | Wait::Done => return None,
        })
    }

    /// The signed `info` to the owner (one per [`Late::report_key`]): the first line
    /// `late: <wait> <subject>`, the lines `late-key:`, `since:` and `next:`, then the text for a
    /// person. `detail` is added at the end (the facts of the caller, such as the coordinator
    /// heartbeat or the report of a dead agent).
    /// `coordinator_since` is the baseline of the late coordinator (see [`Late::report_key`]).
    /// `None` for a wait that the owner never hears of.
    pub fn owner_info(
        &self,
        owner_login: &str,
        detail: &str,
        coordinator_since: Option<i64>,
    ) -> Option<String> {
        let next = self.next_step()?;
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let since = crate::clock::format_ts(self.since);
        let text = match self.wait {
            Wait::Heartbeat => format!(
                "@{owner_login}: {} shows no sign of life for {} s (since {since}; late after {} s). It is not running.\n{}",
                if self.is_coordinator() {
                    format!("the coordinator {}", self.subject_name())
                } else {
                    format!("the agent {}", self.subject_name())
                },
                self.age,
                self.expected,
                if self.is_coordinator() {
                    "Nobody assigns, answers or verifies until it is back. Dev agents go on with their open tickets and take no coordinator duties.\n"
                } else {
                    ""
                }
            ),
            Wait::Dead => format!(
                "@{owner_login}: the agent {} is dead (no sign of life since {since}).\n",
                self.subject_name()
            ),
            Wait::Verify => format!(
                "@{owner_login}: the ticket is in Review for {age} (limit {limit}) and its latest `done` has no passing `verify` on the current PR head. The coordinator shows no sign of life, so nobody verifies it.\n"
            ),
            Wait::OwnerMerge => format!(
                "@{owner_login}: the ticket is in Review for {age} (expected {limit}). Its `done` names protected paths or an owner route, so you merge the PR.\n"
            ),
            Wait::Owner => {
                format!("@{owner_login}: a question waits for you for {age} (expected {limit}).\n")
            }
            Wait::Answer => format!(
                "@{owner_login}: the ticket is Blocked for {age} (expected {limit}) and its question has no answer.\n"
            ),
            Wait::Take | Wait::Push | Wait::Done => return None,
        };
        Some(format!(
            "{LATE_PREFIX}{} {}\n{LATE_KEY}: {}\nsince: {since}\nnext: {next}\n{text}{detail}",
            self.wait.word(),
            self.subject_name(),
            self.report_key(coordinator_since),
        ))
    }

    /// The line of the headless `tick` log.
    pub fn tick_line(&self) -> String {
        let what = match self.wait {
            Wait::Push | Wait::Done => format!("stale-work escalated: {}", self.wait.case()),
            other => format!("late: {}", other.word()),
        };
        format!(
            "left for the interactive coordinator: {}, {what} for {}, agent {}; the interactive coordinator runs `say`, `reassign` or `ticket split`\n",
            self.subject_name(),
            crate::clock::fmt_age(self.age),
            self.agent().unwrap_or("?")
        )
    }

    /// The text of the event for the coordinator when a dev wait goes on.
    /// `escalated: true` is a line of its own, so a prompt can match it.
    fn escalated_body(&self, e: &Expect) -> String {
        let age = crate::clock::fmt_age(self.age);
        let limit = crate::clock::fmt_age(self.expected);
        let more = crate::clock::fmt_age(e.escalate);
        let mut out = format!(
            "{}\nticket: {}\nescalated: true\nagent: {}\nage: {age}\nlimit: {limit}\nescalated after: {more} past the limit\n",
            self.first_line(),
            self.subject_name(),
            self.agent().unwrap_or("?"),
        );
        if let Some(sha) = &self.sha {
            out.push_str(&format!(
                "head: {}\n",
                sha.chars().take(7).collect::<String>()
            ));
        }
        if let Some(pr) = &self.pr {
            out.push_str(&format!("pr: {pr}\n"));
        }
        out.push_str(
            "The dev agent got this marker and did not act. Ask it with `say`, or `reassign` the ticket, or `ticket split` it.\n",
        );
        out
    }
}

/// An open PR as the PR list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: u64,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
}

/// The CI of one head SHA, as `poll` needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiState {
    /// At least one run, check or status is queued or running: the CI is not finished.
    Pending,
    /// Everything is completed and green; the time of the last green part, if it can be read.
    Green(Option<i64>),
    /// Everything is completed and at least one item is not green (the reasons of 8.3).
    Red(Vec<String>),
    /// No CI for this head (also: only information runs, spec/ci#which-ci-runs-count).
    NoCi,
}

/// What the ticket comments say about the verify of the latest `done`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyState {
    /// No passed verify for the latest `done`.
    Unverified,
    /// The coordinator's verify of the latest `done` passed, on the head `head` (its `head_now:`
    /// line), at `since` (Unix seconds). It counts only while the PR still has that head.
    Verified {
        /// The head SHA that the verify judged.
        head: String,
        /// The creation time of the passing verify.
        since: i64,
    },
    /// The latest `done` has a `protected-paths:` or an `owner-route:` line: the owner merges.
    OwnerMerge,
}

/// What the waits need from GitHub. `None` is "could not be read": it never makes a late wait.
pub trait Probe {
    /// `AUTHORITY.md` `dev_branch_prefix`, ending with `/`.
    fn branch_prefix(&self) -> String;
    /// Does a branch `<prefix><agent>/<issue>-...` exist?
    fn has_branch(&self, repo: &str, prefix: &str, agent: &str, issue: u64) -> Option<bool>;
    /// The open PRs of the repo.
    fn open_prs(&self, repo: &str) -> Option<Vec<OpenPr>>;
    /// The CI of the head (the judgement of 8.3 and 27). `None`: could not be read.
    fn ci_state(&self, repo: &str, pr: &OpenPr) -> Option<CiState>;
    /// `Some(Some(t))`: CI is green on the head since `t`. `Some(None)`: not green.
    /// Green without a time that can be read gives `None`: nothing is guessed.
    fn green_since(&self, repo: &str, pr: &OpenPr) -> Option<Option<i64>> {
        match self.ci_state(repo, pr)? {
            CiState::Green(t) => t.map(Some),
            _ => Some(None),
        }
    }
    /// `AUTHORITY.md` has `ci_mode: local` (spec/ci#local-run-ci_mode-local): the CI of a head is
    /// no signal, so `ci-finished` and `green-not-done` stay quiet.
    fn ci_off(&self) -> bool {
        false
    }
    /// Does a signed `done` of `agent` on the ticket name this head SHA?
    fn has_done(&self, t: &TicketRef, agent: &str, sha: &str) -> Option<bool>;
    /// The verify state of the latest `done` of `assignee` on the ticket (not of any sender).
    /// `None`: could not be read, no assignee, or no such `done`.
    fn verify_state(&self, t: &TicketRef, assignee: Option<&str>) -> Option<VerifyState>;
    /// The open wait of a Blocked ticket of `assignee`: a `blocked` message or a question to the
    /// owner without an answer after it. `None`: nothing is open, or it could not be read.
    fn question_open(&self, t: &TicketRef, assignee: Option<&str>) -> Option<OpenWait>;
}

/// A Blocked ticket that waits for an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenWait {
    /// The creation time (Unix seconds) of the message that waits.
    pub created: i64,
    /// The message is a question to the owner, not a `blocked` message.
    pub to_owner: bool,
}

/// Whose waits a run looks at.
#[derive(Debug, Clone, Copy)]
pub enum Scope<'a> {
    /// `status`: every ticket, every wait.
    All,
    /// `poll` of a dev agent: its own tickets, the waits the dev hears. A wait that the seen map
    /// holds for the same value is not read again.
    Dev(&'a str),
    /// `poll` of the coordinator: the waits the coordinator hears. A dev wait is read only when
    /// it is over its expected time by `escalate` more.
    Coordinator,
}

pub struct Inputs<'a> {
    pub rows: &'a [TicketRow],
    pub expect: &'a Expect,
    pub now: i64,
    pub scope: Scope<'a>,
    /// Read GitHub in this run (`poll` throttles it). Without it only the waits that the board
    /// alone shows can be found.
    pub github: bool,
    /// Waits that were given as events already: [`seen_key`] -> value.
    pub seen: &'a BTreeMap<String, String>,
    /// The agents whose heartbeat is judged (`heartbeat`, `dead`); the caller says which.
    pub agents: &'a [AgentLate],
    /// The heartbeat interval in seconds (stale and dead come from it).
    pub interval: i64,
}

/// What [`scan`] found.
#[derive(Debug, Default)]
pub struct Scan {
    /// The late waits, sorted by ticket and wait.
    pub late: Vec<Late>,
    /// Waits that a successful read showed to be gone (or too young again): the caller removes
    /// what it remembers of them. A failed read lands in neither list.
    pub clear: Vec<(String, Wait)>,
    /// GitHub was read for the waits in this run.
    pub read: bool,
    /// Review tickets (item id, time of the passing verify) that passed `verify` on the current PR
    /// head and wait for their merge. They have no `verify` wait; `status` shows them.
    pub verified: Vec<(String, i64)>,
}

/// Do two head SHAs name the same commit (one may be abbreviated)? An empty one never does.
fn same_head(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
}

/// What a read of a Review ticket past its verify time shows.
enum Verdict {
    /// The latest `done` has a passing `verify` on the head the open PR has now (its time).
    Verified(i64),
    /// The latest `done` names the owner route: `verify` refuses it, the owner merges.
    OwnerMerge,
    /// No such verify, a newer head, no PR, or a failed read.
    Unverified,
}

/// Judges the latest `done` of a Review ticket against the open PR head.
fn review_verdict(probe: &dyn Probe, r: &TicketRow, prefix: &mut Option<String>) -> Verdict {
    // The comments first: only a passing verify makes the PR list worth a read.
    let (head, since) = match probe.verify_state(&r.tref(), r.agent.as_deref()) {
        Some(VerifyState::Verified { head, since }) => (head, since),
        Some(VerifyState::OwnerMerge) => return Verdict::OwnerMerge,
        _ => return Verdict::Unverified,
    };
    let Some(agent) = r.agent.as_deref() else {
        return Verdict::Unverified;
    };
    let prefix = prefix.get_or_insert_with(|| probe.branch_prefix());
    let want = format!("{prefix}{agent}/{}-", r.issue);
    let current = probe
        .open_prs(&r.repo)
        .and_then(|prs| prs.into_iter().find(|p| p.head_ref.starts_with(&want)));
    match current {
        Some(pr) if same_head(&head, &pr.head_sha) => Verdict::Verified(since),
        _ => Verdict::Unverified,
    }
}

fn ticket_late(
    r: &TicketRow,
    wait: Wait,
    since: i64,
    expected: i64,
    sha: Option<String>,
    pr: Option<String>,
    now: i64,
) -> Late {
    Late {
        wait,
        subject: Subject::Ticket {
            key: r.key(),
            repo: r.repo.clone(),
            issue: r.issue,
            item_id: r.item_id.clone(),
            agent: r.agent.clone(),
        },
        since,
        age: now - since,
        expected,
        sha,
        pr,
    }
}

/// The `verify` wait of one ticket in Review: it is over `e.verify`. It needs no read.
pub fn verify_wait(r: &TicketRow, e: &Expect, now: i64) -> Option<Late> {
    let age = now - r.updated?;
    (r.in_status(Status::Review) && age > e.verify)
        .then(|| ticket_late(r, Wait::Verify, now - age, e.verify, None, None, now))
}

/// The state of one [`scan`]: the inputs, the scope as flags, the branch prefix (read once, on
/// the first use) and the result.
struct Scanner<'a> {
    inp: &'a Inputs<'a>,
    probe: &'a dyn Probe,
    coordinator: bool,
    dev: Option<&'a str>,
    all: bool,
    prefix: Option<String>,
    out: Scan,
}

impl Scanner<'_> {
    /// The age from which a wait is read: its expected time, for the coordinator the time at which
    /// it hears of it.
    fn need(&self, wait: Wait, expected: i64) -> i64 {
        if self.coordinator {
            coordinator_hears_at(wait, expected, self.inp.expect.escalate).unwrap_or(expected)
        } else {
            expected
        }
    }

    /// Was this wait of the ticket given as an event for this value already (dev scope only)?
    fn seen_is(&self, r: &TicketRow, wait: Wait, value: &str) -> bool {
        self.dev.is_some()
            && self
                .inp
                .seen
                .get(&seen_key(&r.item_id, wait))
                .map(String::as_str)
                == Some(value)
    }

    fn mk(
        &self,
        r: &TicketRow,
        wait: Wait,
        since: i64,
        expected: i64,
        sha: Option<String>,
        pr: Option<String>,
    ) -> Late {
        ticket_late(r, wait, since, expected, sha, pr, self.inp.now)
    }

    fn clear(&mut self, r: &TicketRow, wait: Wait) {
        self.out.clear.push((r.item_id.clone(), wait));
    }

    /// In Progress: the `push` wait (no branch yet) and the `done` wait (a green PR head without
    /// a `done`).
    fn in_progress(&mut self, r: &TicketRow, age: Option<i64>, agent: &str) {
        let e = self.inp.expect;
        let now = self.inp.now;
        if self.coordinator
            && !age.is_some_and(|a| {
                a > self
                    .need(Wait::Push, e.push)
                    .min(self.need(Wait::Done, e.done))
            })
        {
            self.clear(r, Wait::Push);
            self.clear(r, Wait::Done);
            return;
        }
        self.out.read = true;
        let probe = self.probe;
        let prefix = self
            .prefix
            .get_or_insert_with(|| probe.branch_prefix())
            .clone();
        match age {
            Some(a) if a > self.need(Wait::Push, e.push) => {
                if !self.seen_is(r, Wait::Push, "-") {
                    match probe.has_branch(&r.repo, &prefix, agent, r.issue) {
                        Some(false) => {
                            let l = self.mk(r, Wait::Push, now - a, e.push, None, None);
                            self.out.late.push(l);
                        }
                        Some(true) => self.clear(r, Wait::Push),
                        None => {}
                    }
                }
            }
            _ => self.clear(r, Wait::Push),
        }
        if !age.is_some_and(|a| a > self.need(Wait::Done, e.done)) && self.coordinator {
            self.clear(r, Wait::Done);
            return;
        }
        let want = format!("{prefix}{agent}/{}-", r.issue);
        let Some(prs) = probe.open_prs(&r.repo) else {
            return;
        };
        let Some(pr) = prs.into_iter().find(|p| p.head_ref.starts_with(&want)) else {
            self.clear(r, Wait::Done);
            return;
        };
        if self.seen_is(r, Wait::Done, &pr.head_sha) {
            return;
        }
        if probe.ci_off() {
            self.clear(r, Wait::Done);
            return;
        }
        match probe.green_since(&r.repo, &pr) {
            None => {}
            Some(None) => self.clear(r, Wait::Done),
            Some(Some(since)) if now - since > self.need(Wait::Done, e.done) => {
                match probe.has_done(&r.tref(), agent, &pr.head_sha) {
                    Some(false) => {
                        let l = self.mk(
                            r,
                            Wait::Done,
                            since,
                            e.done,
                            Some(pr.head_sha.clone()),
                            Some(format!("{}#{}", r.repo, pr.number)),
                        );
                        self.out.late.push(l);
                    }
                    Some(true) => self.clear(r, Wait::Done),
                    None => {}
                }
            }
            Some(Some(_)) => self.clear(r, Wait::Done),
        }
    }

    /// Review: the `verify` wait, and the `owner-merge` wait.
    fn review(&mut self, r: &TicketRow, age: Option<i64>) {
        let (inp, e, now) = (self.inp, self.inp.expect, self.inp.now);
        // A scan that cannot read raises no verify wait: the next reading scan (at most
        // `CHECK_EVERY_S` later) judges the ticket. It clears nothing either.
        if let Some(l) = verify_wait(r, e, now)
            && (inp.github || r.agent.is_none())
        {
            // Over the verify time. A passing verify of the latest `done` on the current head
            // ends the wait (a held merge); an owner route has no verify, its `owner-merge`
            // wait covers it.
            let verdict = (inp.github && r.agent.is_some()).then(|| {
                self.out.read = true;
                review_verdict(self.probe, r, &mut self.prefix)
            });
            match verdict {
                Some(Verdict::Verified(since)) => {
                    self.out.verified.push((r.item_id.clone(), since))
                }
                Some(Verdict::OwnerMerge) => {}
                Some(Verdict::Unverified) | None => self.out.late.push(l),
            }
        }
        if (self.all || self.coordinator)
            && inp.github
            && let (Some(a), Some(t)) = (age, e.owner)
            && a > t
        {
            self.out.read = true;
            if self.probe.verify_state(&r.tref(), r.agent.as_deref())
                == Some(VerifyState::OwnerMerge)
            {
                let l = self.mk(r, Wait::OwnerMerge, now - a, t, None, None);
                self.out.late.push(l);
            }
        }
    }

    /// Blocked: the `answer` and `owner` waits of an open question.
    fn blocked(&mut self, r: &TicketRow, age: Option<i64>) {
        let e = self.inp.expect;
        let (answer, owner) = (e.answer, e.owner.filter(|_| self.all || self.coordinator));
        if age.is_some_and(|a| answer.is_some_and(|t| a > t) || owner.is_some_and(|t| a > t)) {
            self.out.read = true;
            if let Some(w) = self.probe.question_open(&r.tref(), r.agent.as_deref()) {
                let (wait, limit) = if w.to_owner {
                    (Wait::Owner, owner)
                } else {
                    (Wait::Answer, answer)
                };
                if let Some(t) = limit
                    && self.inp.now - w.created > t
                {
                    let l = self.mk(r, wait, w.created, t, None, None);
                    self.out.late.push(l);
                }
            }
        }
    }

    /// The waits of one ticket.
    fn ticket(&mut self, r: &TicketRow) {
        let (e, now) = (self.inp.expect, self.inp.now);
        let age = r.updated.map(|u| now - u);
        let board_status = r.status_board();
        match board_status {
            Some(Status::InProgress) if self.inp.github && r.agent.is_some() => {
                if let Some(agent) = r.agent.as_deref() {
                    self.in_progress(r, age, agent);
                }
            }
            Some(Status::Review) if self.dev.is_none() => self.review(r, age),
            Some(Status::Ready) if r.agent.is_some() => {
                if let (Some(a), Some(t)) = (age, e.take)
                    && a > t
                {
                    let l = self.mk(r, Wait::Take, now - a, t, None, None);
                    self.out.late.push(l);
                }
            }
            Some(Status::Blocked) if self.dev.is_none() && self.inp.github => {
                self.blocked(r, age);
            }
            _ => {}
        }
    }
}

/// [`late`] with the waits that are gone and the read flag.
pub fn scan(inp: &Inputs<'_>, probe: &dyn Probe) -> Scan {
    let dev = match inp.scope {
        Scope::Dev(me) => Some(me),
        _ => None,
    };
    let mut sc = Scanner {
        inp,
        probe,
        coordinator: matches!(inp.scope, Scope::Coordinator),
        dev,
        all: matches!(inp.scope, Scope::All),
        prefix: None,
        out: Scan::default(),
    };
    let mut sorted: Vec<&TicketRow> = inp
        .rows
        .iter()
        .filter(|r| dev.is_none_or(|me| r.is_agent(me)))
        .collect();
    sorted.sort_by_key(|r| r.key());
    for r in sorted {
        sc.ticket(r);
    }
    let mut out = sc.out;
    out.late.extend(
        inp.agents
            .iter()
            .filter_map(|a| agent_late(a, inp.now, inp.interval)),
    );
    out.late.sort_by_key(|a| (a.subject_name(), a.wait));
    out
}

/// The result word of a `ci-finished` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiResult {
    Green,
    Red,
    None,
}

impl CiResult {
    pub fn word(self) -> &'static str {
        match self {
            CiResult::Green => "green",
            CiResult::Red => "red",
            CiResult::None => "none",
        }
    }
}

/// The case word in the seen map: `<item id>|ci-finished` -> head SHA.
pub const CI_FINISHED: &str = "ci-finished";
/// `<item id>|ci-none-seen` -> head SHA: the first answer "no CI" for a head. `none` comes with
/// the second one, one throttle period later, because GitHub needs some seconds after a push
/// to register the runs of a head.
pub const CI_NONE_SEEN: &str = "ci-none-seen";

/// The CI of a PR head of an In Progress ticket has finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiFinished {
    /// `owner/repo#n` of the ticket.
    pub ticket: String,
    pub repo: String,
    pub issue: u64,
    pub item_id: String,
    pub sha: String,
    /// `owner/repo#n` of the PR.
    pub pr: String,
    pub result: CiResult,
    /// Why the CI is red (the reasons of 8.3); empty otherwise.
    pub reasons: Vec<String>,
}

impl CiFinished {
    /// The URL of the PR, from `owner/repo#n`.
    fn pr_url(&self) -> String {
        match self.pr.split_once('#') {
            Some((repo, n)) => format!("https://github.com/{repo}/pull/{n}"),
            None => self.pr.clone(),
        }
    }

    /// The text of the event for the dev agent.
    pub fn event_body(&self) -> String {
        let head = format!(
            "ci-finished: {}\nticket: {}\npr: {}\nhead: {}\n",
            self.result.word(),
            self.ticket,
            self.pr,
            self.sha
        );
        let short: String = self.sha.chars().take(7).collect();
        match self.result {
            CiResult::Green => format!(
                "{head}The CI of PR {} is green on head {short}.\n\
                 next: crewboard done {} --pr {} --sha {}\n\
                 If more work is open, push it or run `block` and say why.\n",
                self.pr,
                self.issue,
                self.pr_url(),
                self.sha
            ),
            CiResult::Red => {
                let why: String = self
                    .reasons
                    .iter()
                    .take(5)
                    .map(|r| format!("- {r}\n"))
                    .collect();
                format!(
                    "{head}The CI of PR {} is red on head {short}:\n{why}\
                     Fix the cause and push. Do not run `done` on this head. The next head SHA gives a new event.\n",
                    self.pr
                )
            }
            CiResult::None => format!(
                "{head}No CI ran for head {short} of PR {}.\n\
                 If the ticket declares `CI: none`, run `done` now. Else check the PR: a crew branch needs a workflow that runs for it.\n",
                self.pr
            ),
        }
    }
}

/// What [`ci_scan`] found.
#[derive(Debug, Default)]
pub struct CiScan {
    /// GitHub was asked for the PR list of an In Progress ticket in this run.
    pub read: bool,
    pub events: Vec<CiFinished>,
    /// `(item id, head SHA)` where the first "no CI" answer came: store them in the seen map.
    pub none_seen: Vec<(String, String)>,
}

/// `ci-finished` for the In Progress tickets of the dev agent of `inp.scope`
/// (spec/events#stale-work): one event per PR head SHA. Only `Scope::Dev`; the reads are those of
/// the late waits (`probe` caches them), and a head whose event was given is not read. A head whose
/// CI is not finished, and a head whose CI cannot be read, give nothing.
pub fn ci_scan(inp: &Inputs<'_>, probe: &dyn Probe) -> CiScan {
    let mut out = CiScan::default();
    let Scope::Dev(me) = inp.scope else {
        return out;
    };
    if !inp.github {
        return out;
    }
    let mut sorted: Vec<&TicketRow> = inp.rows.iter().collect();
    sorted.sort_by_key(|r| r.key());
    for r in sorted {
        if !r.in_status(Status::InProgress) || r.agent.as_deref() != Some(me) {
            continue;
        }
        if probe.ci_off() {
            return out;
        }
        let prefix = probe.branch_prefix();
        let want = format!("{prefix}{me}/{}-", r.issue);
        out.read = true;
        let Some(pr) = probe
            .open_prs(&r.repo)
            .and_then(|v| v.into_iter().find(|p| p.head_ref.starts_with(&want)))
        else {
            continue;
        };
        let seen = |case: &str| {
            inp.seen
                .get(&format!("{}|{case}", r.item_id))
                .map(String::as_str)
        };
        if seen(CI_FINISHED) == Some(pr.head_sha.as_str()) {
            continue;
        }
        let (result, reasons) = match probe.ci_state(&r.repo, &pr) {
            None | Some(CiState::Pending) => continue,
            Some(CiState::Green(_)) => (CiResult::Green, Vec::new()),
            Some(CiState::Red(why)) => (CiResult::Red, why),
            Some(CiState::NoCi) => {
                if seen(CI_NONE_SEEN) != Some(pr.head_sha.as_str()) {
                    out.none_seen.push((r.item_id.clone(), pr.head_sha.clone()));
                    continue;
                }
                (CiResult::None, Vec::new())
            }
        };
        out.events.push(CiFinished {
            ticket: r.key(),
            repo: r.repo.clone(),
            issue: r.issue,
            item_id: r.item_id.clone(),
            pr: format!("{}#{}", r.repo, pr.number),
            sha: pr.head_sha,
            result,
            reasons,
        });
    }
    out
}

/// The case word of a seen key (`<item id>|<case>` or `<item id>|<case>|<ns>`) names the status in
/// which the entry can hold.
fn status_of_case(case: &str) -> Status {
    match case {
        "review-not-verified" | "owner-merge" => Status::Review,
        "take" => Status::Ready,
        "answer" | "owner" => Status::Blocked,
        _ => Status::InProgress,
    }
}

/// Drop the seen entries of tickets that left the status in which the case can hold, so a
/// later case of the same ticket is a new event.
pub fn prune_seen(seen: &mut BTreeMap<String, String>, rows: &[TicketRow]) {
    seen.retain(|key, _| {
        let mut parts = key.split('|');
        let (Some(item), Some(case)) = (parts.next(), parts.next()) else {
            return false;
        };
        let status = status_of_case(case);
        rows.iter()
            .any(|r| r.item_id == item && r.status_board() == Some(status))
    });
}

/// The open PRs from the answer of `GET repos/{repo}/pulls?state=open`.
pub fn parse_open_prs(body: &Value) -> Option<Vec<OpenPr>> {
    let arr = body.as_array()?;
    let s = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    Some(
        arr.iter()
            .filter_map(|p| {
                Some(OpenPr {
                    number: p.get("number")?.as_u64()?,
                    head_ref: s(p, "/head/ref")?,
                    head_sha: s(p, "/head/sha")?,
                    base_ref: s(p, "/base/ref")?,
                })
            })
            .collect(),
    )
}

/// When did the CI of `sha` turn green? The latest completion among the counted workflow
/// runs, the successful check runs and the successful commit statuses in the answers that
/// were read for the CI judgement. `None` if no answer names a completion time (nothing is
/// guessed).
pub fn green_since_of(bodies: &[Value], sha: &str) -> Option<i64> {
    let ts = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .and_then(crate::clock::parse_ts)
    };
    let own = |v: &Value| {
        v.get("head_sha")
            .and_then(Value::as_str)
            .is_some_and(|h| h.eq_ignore_ascii_case(sha))
    };
    let mut best: Option<i64> = None;
    let mut take = |t: Option<i64>| {
        if let Some(t) = t {
            best = Some(best.map_or(t, |b| b.max(t)));
        }
    };
    for b in bodies {
        if let Some(runs) = b.get("workflow_runs").and_then(Value::as_array) {
            for r in runs.iter().filter(|r| own(r)) {
                let counted = r
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(|e| crate::domain::ci::evaluate::COUNTED_EVENTS.contains(&e));
                if counted && r.get("conclusion").and_then(Value::as_str) == Some("success") {
                    take(ts(r, "updated_at"));
                }
            }
        }
        if let Some(checks) = b.get("check_runs").and_then(Value::as_array) {
            for c in checks.iter().filter(|c| own(c)) {
                if c.get("conclusion").and_then(Value::as_str) == Some("success") {
                    take(ts(c, "completed_at"));
                }
            }
        }
        // The list of commit statuses is a plain array.
        if let Some(list) = b.as_array() {
            for s in list {
                if s.get("state").and_then(Value::as_str) == Some("success") {
                    take(ts(s, "updated_at"));
                }
            }
        }
    }
    best
}

#[cfg(test)]
#[path = "unit_tests.rs"]
mod tests;
