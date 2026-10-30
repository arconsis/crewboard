//! `mine`: the open work of this agent, for people and for `crewboard wake`.
//!
//! A headless dev agent handles its events and stops. A ticket that is In Progress then stands
//! still, because `poll --quiet` finds no events. The wake asks `crewboard mine --quiet`
//! instead. Exit 0 only when an owned ticket needs the agent NOW; exit 1 when every
//! owned ticket only waits (CI of the PR head still runs, a block or a question has no answer
//! yet, or the ticket is in Review). In a trial run, 83 percent of the owned-work wakes did nothing,
//! because they came while the ticket waited ([`judge`] has the rules).
use super::AgentCtx;
use super::behind::{behind_by, mine_hint};
use super::lifecycle::my_rows;
use super::tickets::load_world_cached;
use crate::domain::ci::evaluate::not_green_is_pending;
use crate::error::CbResult;
use crate::rules::evidence::{CiFacts, parse_pr_url};
use crate::rules::status::Status;
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_loader::read_messages;
use crate::ticket::{
    TicketRow, VMsg, answer_after_block, kv, latest_done, parked_for, question_open, verify_report,
};
use serde_json::Value;

/// A ticket of `mine`; `parked_for` is the number of the interrupting ticket when this one is
/// a Blocked ticket the agent parked itself (`take --interrupt`,
/// spec/tickets#status-flow-and-writers).
#[derive(Debug, Clone)]
pub struct MineRow {
    pub row: TicketRow,
    pub parked_for: Option<u64>,
}

/// The tickets of `me` that are In Progress or Ready, or parked by `me`: In Progress first,
/// then the parked ones, then Ready in the order of the queue: lowest `Order`
/// first (a ticket without `Order` after all that have one), then lower issue number. The first Ready ticket of the list is the next ticket of
/// the agent. `parked` maps a ticket key (`repo#issue`) to the number of the interrupting
/// ticket. A Blocked ticket that is not in `parked` is not listed.
pub fn select_mine_parked(
    rows: &[TicketRow],
    me: &str,
    parked: &std::collections::HashMap<String, u64>,
) -> Vec<MineRow> {
    let order = |r: &TicketRow| match r.status_exact() {
        Some(Status::InProgress) => Some(0u8),
        Some(Status::Blocked) if parked.contains_key(&r.key()) => Some(1),
        Some(Status::Ready) => Some(2),
        _ => None,
    };
    let mut v: Vec<MineRow> = rows
        .iter()
        .filter(|r| r.is_agent(me) && order(r).is_some())
        .map(|r| MineRow {
            row: r.clone(),
            parked_for: parked
                .get(&r.key())
                .copied()
                .filter(|_| r.in_status(Status::Blocked)),
        })
        .collect();
    v.sort_by_key(|m| {
        (
            order(&m.row),
            m.row.order.unwrap_or(i64::MAX),
            m.row.repo.clone(),
            m.row.issue,
        )
    });
    v
}

/// The In Progress and Ready tickets of `me`, in the order of [`select_mine_parked`], without
/// parked tickets. `status --queue` and `resume` use it.
pub fn select_mine(rows: &[TicketRow], me: &str) -> Vec<TicketRow> {
    select_mine_parked(rows, me, &std::collections::HashMap::new())
        .into_iter()
        .map(|m| m.row)
        .collect()
}

pub fn render(rows: &[MineRow]) -> String {
    let mut s = String::new();
    for m in rows {
        let r = &m.row;
        let status = match m.parked_for {
            Some(n) => format!("parked for #{n}"),
            None => r.status_str().unwrap_or("?").to_string(),
        };
        s.push_str(&format!(
            "{:<34} {:<16} order {}\n",
            r.key(),
            status,
            r.order.map_or("-".to_string(), |o| o.to_string())
        ));
    }
    s
}

/// The CI of the PR head of an In Progress ticket, as `mine` sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrCi {
    /// No `done` names a PR, and no open PR has the branch of the ticket.
    NoPr,
    /// A run, check or status of the head is not finished yet.
    Pending {
        pr: String,
    },
    /// The CI of the head is finished and not green.
    Red {
        pr: String,
        why: String,
    },
    Green {
        pr: String,
    },
    /// No counted CI run for the head.
    NoRun {
        pr: String,
    },
    /// The PR or its CI could not be read.
    Unreadable {
        pr: String,
        why: String,
    },
}

/// What `mine` knows about one owned ticket. `answered` and `question_open` are for Blocked
/// and Review, `ci` and `verify_failed` for In Progress.
#[derive(Debug, Clone)]
pub struct Owned {
    pub row: TicketRow,
    /// A Blocked ticket that the agent parked itself (`take --interrupt`).
    pub parked_for: Option<u64>,
    /// An answer came after the block or the question ([`answer_after_block`]).
    pub answered: bool,
    /// A question to the owner is open on the ticket ([`question_open`]).
    pub question_open: bool,
    /// The messages of the ticket could not be read.
    pub unreadable: bool,
    pub ci: Option<PrCi>,
    /// The coordinator failed the latest `done`, and the PR head has not moved since.
    pub verify_failed: bool,
    /// The PR base branch and the number of commits the PR head is behind it (> 0). Advice
    /// only: it never changes the verdict of [`judge`].
    pub behind: Option<(String, u64)>,
}

impl Owned {
    pub fn new(row: TicketRow) -> Owned {
        Owned {
            row,
            parked_for: None,
            answered: false,
            question_open: false,
            unreadable: false,
            ci: None,
            verify_failed: false,
            behind: None,
        }
    }
}

/// Does an owned ticket need the agent now? The text says why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Now {
    Work(String),
    Wait(String),
}

/// The verdict for each owned ticket, in the order of `owned`. Pure.
///
/// - In Progress: work after a failed verify (the head has not moved), with red or green CI,
///   with no PR, with no CI run, or when the PR or its CI cannot be read. Wait while the CI
///   of the PR head is pending.
/// - Parked (Blocked by the agent itself) and Ready: work when the agent has no In Progress
///   ticket; else wait (the agent can take only one ticket at a time).
/// - Blocked: work when an answer came after the block or the
///   question; wait for the owner while a question is open; else wait. Messages that cannot be
///   read: wait.
/// - Review: wait for the owner's answer while a question is open; else wait for the
///   coordinator.
pub fn judge(owned: &[Owned]) -> Vec<(String, Now)> {
    let busy = owned
        .iter()
        .find(|o| o.row.in_status(Status::InProgress))
        .map(|o| o.row.key());
    owned
        .iter()
        .filter_map(|o| {
            let n = o.row.issue;
            let now = match o.row.status_board() {
                Some(Status::InProgress) => in_progress(o),
                Some(Status::Blocked) if o.parked_for.is_some() => {
                    let p = o.parked_for.unwrap_or(0);
                    match &busy {
                        Some(k) => Now::Wait(format!("parked for #{p}; {k} is In Progress")),
                        None => Now::Work(format!(
                            "parked for #{p}; nothing is In Progress: `crewboard take {n}`"
                        )),
                    }
                }
                Some(Status::Ready) => match &busy {
                    Some(k) => Now::Wait(format!("Ready, after {k}")),
                    None => Now::Work(format!("Ready: `crewboard take {n}`")),
                },
                Some(Status::Blocked) => {
                    if o.answered {
                        Now::Work(format!(
                            "Blocked, answered: `crewboard take {n}` resumes it"
                        ))
                    } else if o.unreadable {
                        Now::Wait("Blocked; its messages are unreadable".into())
                    } else if o.question_open {
                        Now::Wait("Blocked, waits for the owner".into())
                    } else {
                        Now::Wait("Blocked, no answer yet".into())
                    }
                }
                Some(Status::Review) if o.question_open => {
                    Now::Wait("Review, waits for the owner's answer".into())
                }
                Some(Status::Review) => Now::Wait("Review, waits for the coordinator".into()),
                _ => return None,
            };
            Some((o.row.key(), now))
        })
        .collect()
}

fn in_progress(o: &Owned) -> Now {
    if o.verify_failed {
        return Now::Work("verify failed and the PR head has not moved since".into());
    }
    if o.unreadable {
        return Now::Work("its messages are unreadable".into());
    }
    match o.ci.as_ref().unwrap_or(&PrCi::NoPr) {
        PrCi::NoPr => Now::Work("no PR yet".into()),
        PrCi::Pending { pr } => Now::Wait(format!("CI of {pr} is still running")),
        PrCi::Red { pr, why } => Now::Work(format!("CI of {pr} is red: {why}")),
        PrCi::Green { pr } => Now::Work(format!("CI of {pr} is green: deliver with `done`")),
        PrCi::NoRun { pr } => Now::Work(format!("no CI run for the head of {pr}")),
        PrCi::Unreadable { pr, why } => Now::Work(format!("CI of {pr} is unreadable: {why}")),
    }
}

/// The CI facts of a PR head, as a [`PrCi`].
pub fn pr_ci(pr: String, facts: CbResult<CiFacts>) -> PrCi {
    match facts {
        Ok(CiFacts::Green { .. }) => PrCi::Green { pr },
        Ok(CiFacts::NotGreen(r)) if not_green_is_pending(&r) => PrCi::Pending { pr },
        Ok(CiFacts::NotGreen(r)) => {
            let why: String = r.join("; ").chars().take(120).collect();
            PrCi::Red { pr, why }
        }
        Ok(CiFacts::NoCi(_) | CiFacts::OnlyInformation { .. }) => PrCi::NoRun { pr },
        Ok(CiFacts::NotQueried(why)) => PrCi::Unreadable { pr, why },
        Err(e) => PrCi::Unreadable {
            pr,
            why: e.to_string().chars().take(120).collect(),
        },
    }
}

/// Is `head_ref` the branch of ticket `issue` of `agent`, whatever the branch prefix
/// (`<prefix><agent>/<issue>-<slug>`, spec/tickets#branches)? `mine` does not read
/// AUTHORITY.md for the prefix.
pub fn is_ticket_branch(head_ref: &str, agent: &str, issue: u64) -> bool {
    let want = format!("{agent}/{issue}-");
    head_ref.find(&want).is_some_and(|i| {
        (i == 0 || head_ref[..i].ends_with('/')) && head_ref.len() > i + want.len()
    })
}

/// The branch and the head SHA of a PR, for the message of a wake that ended at its time budget.
/// `None` when the PR does not exist.
pub fn pr_head(ac: &AgentCtx, repo: &str, number: &str) -> CbResult<Option<(String, String)>> {
    match ac.ctx.client.get(&format!("repos/{repo}/pulls/{number}")) {
        Ok(r) => Ok(head_of(&r.body)),
        Err(crate::github::GhError::Http { status: 404, .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// The open PRs of a repo (up to 100): number, branch, head SHA.
pub fn open_prs(ac: &AgentCtx, repo: &str) -> CbResult<Vec<(String, String, String)>> {
    let r = ac
        .ctx
        .client
        .get(&format!("repos/{repo}/pulls?state=open&per_page=100"))?;
    Ok(r.body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let (b, h) = head_of(p)?;
            Some((p.get("number")?.as_u64()?.to_string(), b, h))
        })
        .collect())
}

/// The branch and the head SHA of a PR object.
fn head_of(pr: &Value) -> Option<(String, String)> {
    let s = |p: &str| pr.pointer(p).and_then(Value::as_str);
    Some((s("/head/ref")?.to_string(), s("/head/sha")?.to_string()))
}

/// The PR of an In Progress ticket: `(owner/repo#n, head sha, base ref)`. The PR of the latest
/// `done` of the agent first (one PR read), else the open PR of the ticket branch (one list
/// read per repo of the initiative, the ticket repo first). `Ok(None)`: no PR.
fn find_pr(
    ac: &AgentCtx,
    msgs: &[VMsg],
    r: &TicketRow,
) -> CbResult<Option<(String, String, String)>> {
    let me = ac.name.as_str();
    let s = |v: &Value, p: &str| v.pointer(p).and_then(Value::as_str).map(str::to_string);
    if let Some((_, ev)) = latest_done(msgs, me)
        && let Ok(u) = parse_pr_url(&ev.pr)
    {
        let pr = ac
            .ctx
            .client
            .get(&format!("repos/{}/pulls/{}", u.repo, u.number))?;
        if let (Some(head), Some(base)) = (s(&pr.body, "/head/sha"), s(&pr.body, "/base/ref")) {
            return Ok(Some((format!("{}#{}", u.repo, u.number), head, base)));
        }
    }
    let mut repos: Vec<&String> = ac.ctx.cfg.repos.iter().collect();
    repos.sort_by_key(|x| **x != r.repo);
    for repo in repos {
        let list = ac
            .ctx
            .client
            .get(&format!("repos/{repo}/pulls?state=open&per_page=100"))?;
        for p in list.body.as_array().into_iter().flatten() {
            let (Some(branch), Some(head), Some(base), Some(n)) = (
                s(p, "/head/ref"),
                s(p, "/head/sha"),
                s(p, "/base/ref"),
                p.get("number").and_then(Value::as_u64),
            ) else {
                continue;
            };
            if is_ticket_branch(&branch, me, r.issue) {
                return Ok(Some((format!("{repo}#{n}"), head, base)));
            }
        }
    }
    Ok(None)
}

/// The CI and the verify facts of one In Progress ticket.
fn in_progress_facts(ac: &AgentCtx, msgs: &[VMsg], o: &mut Owned) {
    let found = match find_pr(ac, msgs, &o.row) {
        Ok(f) => f,
        Err(e) => {
            o.ci = Some(PrCi::Unreadable {
                pr: "the PR".into(),
                why: e.to_string().chars().take(120).collect(),
            });
            return;
        }
    };
    let Some((pr, head, base)) = found else {
        o.ci = Some(PrCi::NoPr);
        return;
    };
    let repo = pr.split('#').next().unwrap_or("").to_string();
    if let Some((done, _)) = latest_done(msgs, ac.name.as_str())
        && let Some((report, false)) = verify_report(msgs, done, o.row.issue)
    {
        o.verify_failed = kv(&report.body, "head_now").is_some_and(|h| h == head);
    }
    let q = CiQuery {
        repo: &repo,
        sha: &head,
        base: &base,
        ..CiQuery::default()
    };
    // One compare read; skipped silently on error. Advice, not a verdict.
    o.behind = behind_by(&ac.ctx.client, &repo, &base, &head)
        .filter(|n| *n > 0)
        .map(|n| (base.clone(), n));
    o.ci = Some(pr_ci(
        pr,
        fetch_ci(&|p| ac.ctx.client.get(p), &q).map(|(facts, _)| facts),
    ));
}

/// What `mine` prints and decides: the list of In Progress, parked and Ready tickets (as
/// before), and the verdict for every owned ticket.
#[derive(Default)]
pub struct MineView {
    /// The agent whose tickets these are.
    pub agent: String,
    pub rows: Vec<MineRow>,
    pub now: Vec<(String, Now)>,
    /// The PR of a ticket (`owner/repo#n`), by ticket key, for the tickets that have one.
    pub prs: std::collections::HashMap<String, String>,
    /// Ticket key to the base branch and the commits its PR head is behind (> 0). A hint:
    /// it is not part of `now`, so it never decides the exit code.
    pub behind: std::collections::HashMap<String, (String, u64)>,
}

impl MineView {
    /// Exit 0 of `mine`: one owned ticket needs the agent now.
    pub fn needs_agent(&self) -> bool {
        self.now.iter().any(|(_, n)| matches!(n, Now::Work(_)))
    }

    /// The first ticket to work on: the first In Progress row, else the first row.
    pub fn first_ticket(&self) -> Option<String> {
        self.rows
            .iter()
            .find(|m| m.row.in_status(Status::InProgress))
            .or_else(|| self.rows.first())
            .map(|m| m.row.key())
    }

    /// The `--json` form: one object with stable keys. `rows` are the lines of the text form,
    /// `now` the verdict for each owned ticket; `needs_agent` is the exit code 0 of `mine`.
    pub fn to_json(&self) -> Value {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|m| {
                let r = &m.row;
                serde_json::json!({
                    "ticket": r.key(),
                    "repo": r.repo,
                    "issue": r.issue,
                    "status": r.status_str(),
                    "parked_for": m.parked_for,
                    "order": r.order,
                })
            })
            .collect();
        let now: Vec<Value> = self
            .now
            .iter()
            .map(|(k, n)| {
                let (verdict, text) = match n {
                    Now::Work(t) => ("work", t),
                    Now::Wait(t) => ("wait", t),
                };
                serde_json::json!({
                    "ticket": k,
                    "verdict": verdict,
                    "text": text,
                    "pr": self.prs.get(k),
                    "behind_by": self.behind.get(k).map(|b| b.1),
                    "behind_base": self.behind.get(k).map(|b| b.0.as_str()),
                })
            })
            .collect();
        serde_json::json!({
            "agent": self.agent,
            "rows": rows,
            "now": now,
            "needs_agent": self.needs_agent(),
            "first_ticket": self.first_ticket(),
        })
    }

    pub fn render(&self) -> String {
        let mut s = render(&self.rows);
        if self.now.is_empty() {
            s.push_str("no owned ticket\n");
            return s;
        }
        s.push_str(if self.needs_agent() {
            "now: work to do\n"
        } else {
            "now: nothing to do; every owned ticket waits\n"
        });
        for (k, n) in &self.now {
            let (w, t) = match n {
                Now::Work(t) => ("work", t),
                Now::Wait(t) => ("wait", t),
            };
            s.push_str(&format!("  {k:<34} {w}: {t}\n"));
            if let Some(hint) = self.behind.get(k).and_then(|(b, n)| mine_hint(b, *n)) {
                s.push_str(&format!("  {:<34} hint: {hint}\n", ""));
            }
        }
        s
    }
}

/// The own open work of the agent, best first, and what it needs now.
///
/// Reads: the board rows (as before), the messages of each Blocked, Review and In
/// Progress ticket (one read each), and for each In Progress ticket its PR (one PR read, or
/// one open-PR list per repo) and the CI of the PR head (the reads of `done`). A dev agent
/// has one In Progress ticket, so this is about 7 REST reads more than a wake without the check, and only
/// when the wake script asks (after the back-off, with no events).
/// Each Review ticket of the agent adds one comment read (a question to the owner may be open).
/// Each In Progress ticket with a PR adds one compare read (how far the head is behind the base).
pub fn run(ac: &AgentCtx) -> CbResult<MineView> {
    let world = load_world_cached(ac)?;
    let rows = my_rows(ac, &world)?;
    let me = ac.name.as_str();
    let mut parked = std::collections::HashMap::new();
    let mut owned = Vec::new();
    for r in rows.iter().filter(|r| r.is_agent(me)) {
        let mut o = Owned::new(r.clone());
        let status = r.status_board();
        if matches!(
            status,
            Some(Status::Blocked | Status::Review | Status::InProgress)
        ) {
            match read_messages(&ac.ctx.reader(), &world.trust, &r.repo, r.issue) {
                Ok((_, msgs)) => match status {
                    Some(Status::InProgress) => in_progress_facts(ac, &msgs, &mut o),
                    Some(Status::Review) => {
                        o.question_open = question_open(&msgs, Some(me)).is_some();
                    }
                    Some(s) => {
                        o.question_open = question_open(&msgs, Some(me)).is_some();
                        if s == Status::Blocked
                            && let Some(n) = parked_for(&msgs, me)
                        {
                            parked.insert(r.key(), n);
                            o.parked_for = Some(n);
                        }
                        o.answered =
                            o.parked_for.is_none() && answer_after_block(&msgs, s, Some(me));
                    }
                    None => {}
                },
                // a ticket whose messages cannot be read is not listed as parked
                Err(_) => o.unreadable = true,
            }
        }
        owned.push(o);
    }
    let order = |o: &Owned| match o.row.status_board() {
        Some(Status::InProgress) => 0u8,
        Some(Status::Blocked) if o.parked_for.is_some() => 1,
        Some(Status::Ready) => 2,
        Some(Status::Blocked) => 3,
        _ => 4,
    };
    owned.sort_by_key(|o| (order(o), o.row.repo.clone(), o.row.issue));
    let prs = owned
        .iter()
        .filter_map(|o| match o.ci.as_ref()? {
            PrCi::Pending { pr }
            | PrCi::Red { pr, .. }
            | PrCi::Green { pr }
            | PrCi::NoRun { pr }
            | PrCi::Unreadable { pr, .. }
                if pr.contains('#') =>
            {
                Some((o.row.key(), pr.clone()))
            }
            _ => None,
        })
        .collect();
    let behind = owned
        .iter()
        .filter_map(|o| Some((o.row.key(), o.behind.clone()?)))
        .collect();
    Ok(MineView {
        agent: me.to_string(),
        rows: select_mine_parked(&rows, me, &parked),
        now: judge(&owned),
        prs,
        behind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn none() -> HashMap<String, u64> {
        HashMap::new()
    }

    fn row(n: u64, status: &str, agent: &str, _unused: Option<&str>) -> TicketRow {
        TicketRow {
            repo: "a/b".into(),
            issue: n,
            item_id: format!("PVTI_{n}"),
            status: Some(status.into()),
            agent: Some(agent.into()),
            order: None,
            updated: None,
        }
    }

    #[test]
    fn only_in_progress_and_ready_tickets_of_the_agent_count() {
        let rows = vec![
            row(1, "In Progress", "me@x", None),
            row(2, "Ready", "me@x", None),
            row(3, "Review", "me@x", None),
            row(4, "Blocked", "me@x", None),
            row(5, "Blocked", "me@x", None),
            row(9, "Needs Human", "me@x", None),
            row(6, "Done", "me@x", None),
            row(7, "Backlog", "me@x", None),
            row(8, "In Progress", "other@x", None),
        ];
        let got: Vec<u64> = select_mine_parked(&rows, "me@x", &none())
            .iter()
            .map(|r| r.row.issue)
            .collect();
        assert_eq!(got, [1, 2]);
        // control: nothing of the agent that owns no such ticket
        assert!(select_mine_parked(&rows, "nobody@x", &none()).is_empty());
        assert!(select_mine_parked(&rows[2..7], "me@x", &none()).is_empty());
    }

    fn ordered(n: u64, order: Option<i64>, prio: Option<&str>) -> TicketRow {
        TicketRow {
            order,
            ..row(n, "Ready", "me@x", prio)
        }
    }

    fn issues(rows: &[TicketRow]) -> Vec<u64> {
        select_mine(rows, "me@x").iter().map(|r| r.issue).collect()
    }

    #[test]
    fn the_lowest_order_is_the_next_ticket_whatever_the_number() {
        // #38 then #40 then #32 then #20, as the coordinator said in free text
        let rows = vec![
            ordered(20, Some(4), Some("P0")),
            ordered(32, Some(3), None),
            ordered(40, Some(2), Some("P3")),
            ordered(38, Some(1), None),
        ];
        assert_eq!(issues(&rows), [38, 40, 32, 20]);
        // control: without Order the same tickets go by issue number
        let plain: Vec<TicketRow> = rows
            .iter()
            .map(|r| TicketRow {
                order: None,
                ..r.clone()
            })
            .collect();
        assert_eq!(issues(&plain), [20, 32, 38, 40]);
    }

    #[test]
    fn equal_orders_go_by_issue_number_and_unordered_tickets_go_last() {
        let rows = vec![
            ordered(9, None, Some("P0")),
            ordered(7, Some(5), None),
            ordered(6, Some(5), Some("P2")),
            ordered(5, Some(5), Some("P2")),
            ordered(8, Some(5), Some("P0")),
        ];
        // order 5 ties go by number (5, 6, 7, 8); 9 has no order
        assert_eq!(issues(&rows), [5, 6, 7, 8, 9]);
        // an In Progress ticket still comes before every Ready ticket
        let mut with_ip = rows.clone();
        with_ip.push(TicketRow {
            order: Some(99),
            ..row(1, "In Progress", "me@x", None)
        });
        assert_eq!(issues(&with_ip)[0], 1);
    }

    #[test]
    fn in_progress_first_then_ready_by_number() {
        let rows = vec![
            row(10, "Ready", "me@x", Some("P3")),
            row(11, "Ready", "me@x", Some("P0")),
            row(12, "Ready", "me@x", None),
            row(13, "In Progress", "me@x", Some("P3")),
            row(14, "Ready", "me@x", Some("P1")),
        ];
        let got: Vec<u64> = select_mine_parked(&rows, "me@x", &none())
            .iter()
            .map(|r| r.row.issue)
            .collect();
        assert_eq!(got, [13, 10, 11, 12, 14]);
        assert!(render(&select_mine_parked(&rows, "me@x", &none())).starts_with("a/b#13"));
    }

    #[test]
    fn a_parked_ticket_is_listed_above_the_ready_queue() {
        let rows = vec![
            row(20, "Ready", "me@x", Some("P0")),
            row(21, "Blocked", "me@x", Some("P2")),
            row(22, "Blocked", "me@x", Some("P0")),
            row(23, "In Progress", "me@x", Some("P3")),
            row(24, "Blocked", "other@x", None),
        ];
        // only #21 is parked (by me); #22 is a normal block, #24 belongs to another agent
        let parked = HashMap::from([("a/b#21".to_string(), 23u64), ("a/b#24".to_string(), 1)]);
        let got = select_mine_parked(&rows, "me@x", &parked);
        let order: Vec<u64> = got.iter().map(|m| m.row.issue).collect();
        assert_eq!(order, [23, 21, 20]);
        assert_eq!(got[1].parked_for, Some(23));
        assert_eq!(got[0].parked_for, None);
        let text = render(&got);
        assert!(text.contains("parked for #23"), "{text}");
        assert!(!text.contains("a/b#22"), "{text}");
    }

    fn owned(n: u64, status: &str) -> Owned {
        Owned::new(row(n, status, "me@x", None))
    }

    fn with_ci(n: u64, ci: PrCi) -> Owned {
        Owned {
            ci: Some(ci),
            ..owned(n, "In Progress")
        }
    }

    fn pr() -> String {
        "a/b#45".to_string()
    }

    fn works(v: &[(String, Now)]) -> Vec<String> {
        v.iter()
            .filter(|(_, n)| matches!(n, Now::Work(_)))
            .map(|(k, _)| k.clone())
            .collect()
    }

    #[test]
    fn issue_145_every_owned_ticket_that_only_waits_gives_no_work() {
        // pending CI, a block and a question without answer, Review, and a Ready ticket behind
        // the In Progress one: nothing to do now
        let waiting = vec![
            with_ci(1, PrCi::Pending { pr: pr() }),
            owned(2, "Ready"),
            Owned {
                question_open: true,
                ..owned(3, "Blocked")
            },
            owned(4, "Blocked"),
            Owned {
                question_open: true,
                ..owned(5, "Review")
            },
        ];
        let v = judge(&waiting);
        assert_eq!(v.len(), 5, "{v:?}");
        assert_eq!(
            v[2].1,
            Now::Wait("Blocked, waits for the owner".into()),
            "{v:?}"
        );
        assert_eq!(
            v[4].1,
            Now::Wait("Review, waits for the owner's answer".into()),
            "{v:?}"
        );
        assert!(works(&v).is_empty(), "{v:?}");
        let view = MineView {
            now: v,
            ..MineView::default()
        };
        assert!(!view.needs_agent());
        let text = view.render();
        assert!(text.contains("nothing to do"), "{text}");
        assert!(text.contains("CI of a/b#45 is still running"), "{text}");
        // control: the same tickets, but the CI is red -> work on #1 only
        let mut red = waiting.clone();
        red[0] = with_ci(
            1,
            PrCi::Red {
                pr: pr(),
                why: "build failed".into(),
            },
        );
        assert_eq!(works(&judge(&red)), ["a/b#1"]);
    }

    #[test]
    fn issue_145_an_in_progress_ticket_needs_the_agent_unless_its_ci_runs() {
        for ci in [
            PrCi::NoPr,
            PrCi::Green { pr: pr() },
            PrCi::NoRun { pr: pr() },
            PrCi::Red {
                pr: pr(),
                why: "x".into(),
            },
            PrCi::Unreadable {
                pr: pr(),
                why: "HTTP 500".into(),
            },
        ] {
            let v = judge(&[with_ci(1, ci.clone())]);
            assert_eq!(works(&v), ["a/b#1"], "{ci:?}");
        }
        // no CI facts at all counts as "no PR yet"
        assert_eq!(works(&judge(&[owned(1, "In Progress")])), ["a/b#1"]);
        // a failed verify whose head has not moved is work, also while a CI runs
        let failed = Owned {
            verify_failed: true,
            ..with_ci(1, PrCi::Pending { pr: pr() })
        };
        assert_eq!(works(&judge(&[failed])), ["a/b#1"]);
        // messages that cannot be read: work
        let unreadable = Owned {
            unreadable: true,
            ..with_ci(1, PrCi::Pending { pr: pr() })
        };
        assert_eq!(works(&judge(&[unreadable])), ["a/b#1"]);
    }

    #[test]
    fn issue_145_an_answer_after_a_block_or_a_question_is_work() {
        // "Needs Human" is the legacy board option: it reads as Blocked
        for status in ["Blocked", "Needs Human"] {
            let answered = Owned {
                answered: true,
                ..owned(3, status)
            };
            let v = judge(&[answered]);
            assert_eq!(works(&v), ["a/b#3"], "{status}");
            assert!(
                matches!(&v[0].1, Now::Work(t) if t.contains("crewboard take 3")),
                "{v:?}"
            );
            // control: no answer -> wait; unreadable messages -> wait
            assert!(works(&judge(&[owned(3, status)])).is_empty(), "{status}");
            let unreadable = Owned {
                unreadable: true,
                ..owned(3, status)
            };
            assert!(works(&judge(&[unreadable])).is_empty(), "{status}");
        }
    }

    #[test]
    fn a_review_ticket_without_an_open_question_waits_for_the_coordinator() {
        let v = judge(&[owned(5, "Review")]);
        assert_eq!(
            v[0].1,
            Now::Wait("Review, waits for the coordinator".into())
        );
    }

    #[test]
    fn issue_145_ready_and_parked_tickets_wait_only_behind_an_in_progress_ticket() {
        let parked = Owned {
            parked_for: Some(9),
            ..owned(7, "Blocked")
        };
        let free = judge(&[parked.clone(), owned(8, "Ready")]);
        assert_eq!(works(&free), ["a/b#7", "a/b#8"]);
        let busy = judge(&[
            with_ci(9, PrCi::Pending { pr: pr() }),
            parked,
            owned(8, "Ready"),
        ]);
        assert!(works(&busy).is_empty(), "{busy:?}");
        // Backlog and Done tickets get no verdict; no ticket at all: no work
        assert!(judge(&[owned(10, "Backlog"), owned(11, "Done")]).is_empty());
        let none = MineView::default();
        assert!(!none.needs_agent());
        assert!(none.render().contains("no owned ticket"));
    }

    #[test]
    fn the_json_form_has_stable_keys_and_the_first_ticket_prefers_in_progress() {
        // a Ready row stands before the In Progress row in this list, as if the order differed
        let mine_row = |n, status: &str, parked: Option<u64>| MineRow {
            row: row(n, status, "me@x", Some("P1")),
            parked_for: parked,
        };
        let mut view = MineView {
            agent: "me@x".into(),
            rows: vec![mine_row(1, "Ready", None), mine_row(2, "In Progress", None)],
            now: vec![
                (
                    "a/b#2".into(),
                    Now::Wait("CI of a/b#45 is still running".into()),
                ),
                ("a/b#1".into(), Now::Work("Ready".into())),
            ],
            prs: [("a/b#2".to_string(), "a/b#45".to_string())].into(),
            behind: HashMap::new(),
        };
        let v = view.to_json();
        assert_eq!(v["first_ticket"], "a/b#2");
        assert_eq!(v["agent"], "me@x");
        assert_eq!(v["needs_agent"], true);
        assert_eq!(v["rows"][1]["ticket"], "a/b#2");
        assert_eq!(v["rows"][1]["status"], "In Progress");
        assert!(v["rows"][1].get("priority").is_none());
        assert!(v["rows"][1]["order"].is_null() && v["rows"][1]["parked_for"].is_null());
        assert_eq!(v["rows"][1]["issue"], 2);
        assert_eq!(v["now"][0]["verdict"], "wait");
        assert_eq!(v["now"][0]["pr"], "a/b#45");
        assert_eq!(v["now"][1]["verdict"], "work");
        assert!(v["now"][1]["pr"].is_null());
        // twin: no In Progress row -> the first row; no row -> null
        view.rows.remove(1);
        assert_eq!(view.to_json()["first_ticket"], "a/b#1");
        view.rows.clear();
        assert!(view.to_json()["first_ticket"].is_null());
        // a parked ticket keeps the status Blocked and names the interrupting ticket
        view.rows = vec![mine_row(3, "Blocked", Some(9))];
        let v = view.to_json();
        assert_eq!(
            (
                v["rows"][0]["status"].as_str(),
                v["rows"][0]["parked_for"].as_u64()
            ),
            (Some("Blocked"), Some(9))
        );
    }

    #[test]
    fn behind_main_is_a_hint_and_never_changes_the_verdict() {
        // pending CI waits, behind or not; green CI works, behind or not
        for (ci, work) in [
            (PrCi::Pending { pr: pr() }, false),
            (PrCi::Green { pr: pr() }, true),
        ] {
            let plain = with_ci(1, ci.clone());
            let behind = Owned {
                behind: Some(("main".into(), 4)),
                ..plain.clone()
            };
            assert_eq!(works(&judge(&[plain])).len() == 1, work);
            assert_eq!(works(&judge(&[behind])).len() == 1, work);
        }
        // the hint stands under the verdict line, and only for the ticket that is behind
        let view = MineView {
            now: vec![
                (
                    "a/b#1".into(),
                    Now::Wait("CI of a/b#45 is still running".into()),
                ),
                ("a/b#2".into(), Now::Wait("Ready, after a/b#1".into())),
            ],
            behind: [("a/b#1".to_string(), ("main".to_string(), 3))].into(),
            ..MineView::default()
        };
        assert!(!view.needs_agent());
        let text = view.render();
        assert!(
            text.contains("hint: behind main by 3 commits; merge main before done"),
            "{text}"
        );
        assert_eq!(text.matches("hint:").count(), 1, "{text}");
        let j = view.to_json();
        assert_eq!(j["now"][0]["behind_by"], 3);
        assert_eq!(j["now"][0]["behind_base"], "main");
        assert!(j["now"][1]["behind_by"].is_null());
        assert_eq!(j["needs_agent"], false);
    }

    #[test]
    fn issue_145_pending_and_red_ci_facts_differ() {
        let pending = vec!["run 9001 (push, in_progress), not completed".to_string()];
        assert!(not_green_is_pending(&pending));
        assert_eq!(
            pr_ci(pr(), Ok(CiFacts::NotGreen(pending))),
            PrCi::Pending { pr: pr() }
        );
        let red = vec!["run 9001 (push, failure)".to_string()];
        assert!(matches!(
            pr_ci(pr(), Ok(CiFacts::NotGreen(red))),
            PrCi::Red { .. }
        ));
        assert_eq!(
            pr_ci(pr(), Ok(CiFacts::NoCi(vec![]))),
            PrCi::NoRun { pr: pr() }
        );
        assert!(matches!(
            pr_ci(pr(), Ok(CiFacts::NotQueried("x".into()))),
            PrCi::Unreadable { .. }
        ));
    }

    #[test]
    fn the_ticket_branch_matches_any_prefix_but_only_this_agent_and_issue() {
        assert!(is_ticket_branch("crew/me@x/12-fix", "me@x", 12));
        assert!(is_ticket_branch("team/x/me@x/12-fix", "me@x", 12));
        assert!(is_ticket_branch("me@x/12-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/me@x/123-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/me@x/12-", "me@x", 12));
        assert!(!is_ticket_branch("crew/other@x/12-fix", "me@x", 12));
        assert!(!is_ticket_branch("crew/xme@x/12-fix", "me@x", 12));
    }
}
