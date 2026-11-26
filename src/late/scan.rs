//! The scan: what the board and the PRs say ([`Probe`]), and which waits are late ([`scan`]).

use super::*;

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
    /// The open PR of the repo whose branch starts with `head_prefix`. `Some(None)`: none.
    fn open_pr(&self, repo: &str, head_prefix: &str) -> Option<Option<OpenPr>>;
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
    let current = probe.open_pr(&r.repo, &want).flatten();
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
        let Some(found) = probe.open_pr(&r.repo, &want) else {
            return;
        };
        let Some(pr) = found else {
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
