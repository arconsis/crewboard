use super::*;
use crate::events::{Event, EventClass, TicketSnap};
use crate::protocol::names::Kind;
use crate::rules::fields::Field;
use crate::rules::status::{BoardStatus, Status};
use std::collections::BTreeMap;

/// A ticket's item id from the signed `kind=info` comment of the coordinator
/// (spec/board#item-ids-are-the-source-of-truth).
pub fn info_body(item_id: &str, project: u64) -> String {
    format!(
        "{}\nitem={item_id}\nproject={project}\n",
        crate::protocol::acts::board_announce_head()
    )
}

/// The line of the creation info that records the PR a new ticket adopts:
/// `adopts-pr: <url> branch: <branch>`.
pub const ADOPTS_PR: &str = "adopts-pr:";

/// [`info_body`] of a ticket that adopts an open PR of an older, Done ticket.
pub fn info_body_adopting(item_id: &str, project: u64, pr_url: &str, branch: &str) -> String {
    format!(
        "{}{ADOPTS_PR} {pr_url} branch: {branch}\n",
        info_body(item_id, project)
    )
}

/// The head branch that the creation info of a ticket allows for PR `repo#number`.
/// Only a signed `info` of a coordinator counts: a dev agent, or the owner without a header,
/// cannot widen the branch rule of a ticket. The branch is the text after ` branch: `.
///
/// A second source: the latest reassign notice of a coordinator names the open PR that the old
/// assignee left ([`earlier_work`]). The new assignee may finish that PR (`take --continue-pr`), so
/// its branch is allowed for this ticket too.
pub fn adopted_branch(msgs: &[VMsg], repo: &str, number: u64) -> Option<String> {
    let of = |url: &str, branch: &str| {
        let pr = crate::rules::evidence::parse_pr_url(url.trim()).ok()?;
        let branch = branch.trim();
        (pr.repo.eq_ignore_ascii_case(repo) && pr.number == number && !branch.is_empty())
            .then(|| branch.to_string())
    };
    msgs.iter()
        .filter(|m| m.sender.actor().is_coordinator() && m.kind == Some(Kind::Info))
        .flat_map(|m| m.body.lines())
        .find_map(|l| {
            let rest = l.strip_prefix(ADOPTS_PR)?.trim();
            let (url, branch) = rest.split_once(" branch: ")?;
            of(url, branch)
        })
        .or_else(|| {
            let e = earlier_work(msgs)?;
            of(e.pr.as_deref()?, &e.branch)
        })
}

/// The work that the old assignee left, from the latest reassign notice of a coordinator. Only
/// a signed `info` of a coordinator counts, and only the latest notice: an older notice names
/// work of an earlier round.
pub fn earlier_work(msgs: &[VMsg]) -> Option<crate::protocol::acts::EarlierWork> {
    let m = msgs.iter().rev().find(|m| {
        m.sender.actor().is_coordinator()
            && crate::protocol::acts::parse(m.kind, &m.body)
                == crate::protocol::acts::Act::Reassigned
    })?;
    crate::protocol::acts::earlier_work_of(&m.body)
}

pub fn find_item_id(msgs: &[VMsg]) -> Option<String> {
    msgs.iter()
        .rev()
        .filter(|m| m.sender.actor().is_coordinator() && m.kind == Some(Kind::Info))
        .find_map(|m| {
            m.body.lines().find_map(|l| {
                let v = l.strip_prefix("item=")?.trim();
                v.starts_with("PVTI_").then(|| v.to_string())
            })
        })
}

/// One ticket as the board shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRow {
    pub repo: String,
    pub issue: u64,
    pub item_id: String,
    pub status: Option<BoardStatus>,
    pub agent: Option<String>,
    /// The optional `Order` field (number): the position in the queue of the agent, lowest first.
    pub order: Option<i64>,
    /// `updatedAt` of the item (the last change on the board), Unix seconds.
    pub updated: Option<i64>,
}

impl TicketRow {
    /// The row of a board item. `key` is `owner/repo#number`; `None` when it has no number.
    pub fn from_item(it: &crate::items::ItemSnapshot, key: &str) -> Option<TicketRow> {
        let (repo, n) = key.rsplit_once('#')?;
        Some(TicketRow {
            repo: repo.to_string(),
            issue: n.parse().ok()?,
            item_id: it.id.clone(),
            status: it.get(Field::Status).map(BoardStatus::parse),
            agent: it.get(Field::Agent).map(str::to_string),
            order: it.get(Field::Order).and_then(|o| o.parse::<i64>().ok()),
            updated: crate::clock::parse_ts(&it.updated_at),
        })
    }

    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.issue)
    }

    pub fn snap(&self) -> TicketSnap {
        TicketSnap {
            status: self.status_str().map(str::to_string),
            agent: self.agent.clone(),
        }
    }

    /// The Status text as the board shows it.
    pub fn status_str(&self) -> Option<&str> {
        self.status.as_ref().map(BoardStatus::as_str)
    }

    /// True when the Status is exactly `status`.
    pub fn in_status(&self, status: Status) -> bool {
        self.status.as_ref().is_some_and(|s| s.is(status))
    }

    /// True when the ticket has ended: its Status is exactly Done or Dropped ([`Status::ended`]).
    pub fn ended(&self) -> bool {
        self.status_exact().is_some_and(Status::ended)
    }

    /// The Status with the legacy `Needs Human` read as Blocked; `None` when unknown or empty.
    pub fn status_board(&self) -> Option<Status> {
        self.status.as_ref().and_then(BoardStatus::board)
    }

    /// The exact Status; `None` when unknown, legacy or empty.
    pub fn status_exact(&self) -> Option<Status> {
        self.status.as_ref().and_then(BoardStatus::exact)
    }

    /// True when the ticket belongs to `name`.
    pub fn is_agent(&self, name: &str) -> bool {
        self.agent.as_deref() == Some(name)
    }
}

fn ticket_event(row: &TicketRow, me: &str, kind: &str, body: String, now: &str) -> Event {
    Event {
        class: EventClass::Ticket,
        comment_id: 0,
        repo: row.repo.clone(),
        issue: row.issue,
        url: format!("https://github.com/{}/issues/{}", row.repo, row.issue),
        author: "board".into(),
        from: "board".into(),
        to: me.to_string(),
        kind: kind.into(),
        re: "none".into(),
        ts: now.to_string(),
        verified: false,
        body: crate::events::decode_html_entities(&body),
    }
}

/// Compare the tickets with the snapshot of the last cycle (spec/events#watch-semantics:
/// assignments and status changes of this agent's tickets; the coordinator sees every ticket's
/// status change). Returns the events and the snapshot to store after they are printed.
///
/// A ticket seen for the first time produces an event only if it is assigned to this agent
/// and has not ended, Done or Dropped (a fresh cache must not replay history).
pub fn ticket_events(
    me: &str,
    coordinator: bool,
    rows: &[TicketRow],
    prev: &BTreeMap<String, TicketSnap>,
    now: &str,
) -> (Vec<Event>, BTreeMap<String, TicketSnap>) {
    let mut events = Vec::new();
    let mut next = BTreeMap::new();
    let show = |s: Option<&str>| s.unwrap_or("(none)").to_string();
    for row in rows {
        next.insert(row.item_id.clone(), row.snap());
        let mine = row.is_agent(me);
        match prev.get(&row.item_id) {
            None => {
                if mine && !row.ended() {
                    events.push(ticket_event(
                        row,
                        me,
                        "assignment",
                        format!("assigned to you; status {}", show(row.status_str())),
                        now,
                    ));
                }
            }
            Some(p) => {
                if p.agent != row.agent && mine {
                    events.push(ticket_event(
                        row,
                        me,
                        "assignment",
                        format!("assigned to you; status {}", show(row.status_str())),
                        now,
                    ));
                } else if p.agent != row.agent && p.agent.as_deref() == Some(me) {
                    events.push(ticket_event(
                        row,
                        me,
                        "unassigned",
                        format!(
                            "no longer assigned to you; Agent is now {}; status {}",
                            show(row.agent.as_deref()),
                            show(row.status_str())
                        ),
                        now,
                    ));
                } else if p.status.as_deref() != row.status_str() && (mine || coordinator) {
                    events.push(ticket_event(
                        row,
                        me,
                        "status-changed",
                        format!(
                            "status {} -> {}",
                            show(p.status.as_deref()),
                            show(row.status_str())
                        ),
                        now,
                    ));
                }
            }
        }
    }
    (events, next)
}
