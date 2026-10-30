//! `audit` (spec/cli#commands, spec/identity#phases): what the signed comments show, nothing else.
//!
//! GitHub keeps no change history for project field values, and in one-account mode the `creator` of a
//! field value is always the owner's login. So the audit reads only comments:
//!
//! - **headerless owner-account comments posted while an agent session ran** (one-account
//!   mode, spec/identity#phases): there the agents use the owner's token, so such a comment may be
//!   the owner or an agent that posted without a header. The audit cannot tell; it lists them for
//!   the owner to confirm;
//! - **bad signatures**, **edited messages**, **messages of unknown senders**, **malformed
//!   headers** and **crew headers by a foreign author** (the security events of `watch`);
//! - **transitions by wrong writers**, as far as signed messages show them: an assignment, a
//!   board announcement, a verify result or a close notice by a non-coordinator, and an `ack`,
//!   `done` or `blocked` by an agent that the ticket was not assigned to.
//!
//! The one thing the audit reads besides comments is the board, to compare it with them: the
//! log is the truth and the board is a view (spec/overview#where-things-live). A **field
//! mismatch** is a board field that the signed log does not support: a ticket `Status` or
//! `Agent`, an agent `Role`, `Key` or `State`. The audit lists it and fixes nothing.
//!
//! Pure: the caller reads the comments, the registry, the heartbeats and the board fields.

use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::protocol::acts::{Act, parse};
use crate::protocol::header::Message;
use crate::protocol::names::{Kind, Re};
use crate::protocol::replay::Seen;
use crate::registry::{CREW_PREFIX, Registry, fingerprint, join_replay, verify_join};
use crate::rules::Role;
use crate::rules::lifecycle::State;
use crate::ticket::log::{fold_agent, fold_ticket, status_contradicts, status_mismatch_text};
use crate::ticket::{Sender, VMsg};
use serde::Serialize;
use ssh_key::PublicKey;
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A comment of the owner's account without a crew header, inside an agent session.
    HeaderlessOwner,
    BadSignature,
    Edited,
    UnknownSender,
    Malformed,
    /// A valid signature with a signed `ts` far from the GitHub creation time: a replayed copy.
    Replayed,
    /// A crew header by an author who is neither the owner nor the account of an approved
    /// agent, or a signed message posted by another account than the one the approval bound.
    ForeignAuthor,
    /// A transition that its signed message shows to be by the wrong writer.
    WrongWriter,
    /// A board field that the signed log does not support. Not a security event: an owner
    /// edit is not in the log either.
    FieldMismatch,
}

impl FindingKind {
    pub fn word(self) -> &'static str {
        match self {
            FindingKind::HeaderlessOwner => "headerless-owner-comment",
            FindingKind::BadSignature => "bad-signature",
            FindingKind::Edited => "edited-message",
            FindingKind::UnknownSender => "unknown-sender",
            FindingKind::Malformed => "malformed-message",
            FindingKind::Replayed => "replayed-message",
            FindingKind::ForeignAuthor => "foreign-author",
            FindingKind::WrongWriter => "wrong-writer",
            FindingKind::FieldMismatch => "field-mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub comment_id: u64,
    pub repo: String,
    pub issue: u64,
    pub url: String,
    pub author: String,
    pub created: String,
    #[serde(skip)]
    pub created_epoch: i64,
    pub detail: String,
    /// For security events: a signed `kind=info` to the owner exists for this event.
    pub reported_to_owner: Option<bool>,
}

/// The time span in which an agent was running, as far as its signed messages and its
/// heartbeat show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Window {
    pub agent: String,
    pub from: i64,
    pub to: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub scanned: usize,
    pub windows: Vec<Window>,
    pub findings: Vec<Finding>,
    /// Headerless owner-account comments outside every session window (not listed).
    pub owner_comments_outside_sessions: usize,
    /// Comments by authors who are not the owner, without a crew header.
    pub foreign_comments: usize,
}

/// One ticket as the board shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardTicket {
    /// `owner/repo#number`.
    pub key: String,
    pub item_id: String,
    pub status: Option<String>,
    pub agent: Option<String>,
}

/// One agent item as the board shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardAgent {
    pub name: String,
    pub role: Option<String>,
    /// The public key line of the `Key` field.
    pub key: Option<String>,
    pub state: Option<String>,
}

/// The board fields that the audit compares with the log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardView {
    pub tickets: Vec<BoardTicket>,
    pub agents: Vec<BoardAgent>,
}

/// The key that identifies one security event, also in the owner message that reports it:
/// `<repo>#<issue>/c-<comment id>:<reason>`, the repo in lower case (GitHub names are
/// case-insensitive, so `Org/Repo` and `org/repo` give one key).
pub fn event_key(repo: &str, issue: u64, comment_id: u64, reason: &str) -> String {
    format!(
        "{}#{issue}/c-{comment_id}:{reason}",
        repo.to_ascii_lowercase()
    )
}

/// An `event=` key with the repo part (before the first `#`) in lower case, so that a key that an
/// older build wrote with another spelling of the repo still matches. A key without `#` (an agent
/// state) stays as it is.
fn normalized_key(k: &str) -> String {
    match k.split_once('#') {
        Some((repo, rest)) => format!("{}#{rest}", repo.to_ascii_lowercase()),
        None => k.to_string(),
    }
}

/// The events that a valid message `kind=info`, `to=owner` already reported (`event=` lines),
/// with the repo part of each key in lower case.
pub fn reported_events(msgs: &[VMsg]) -> HashSet<String> {
    msgs.iter()
        .filter(|m| m.kind == Some(Kind::Info) && m.to == "owner")
        .filter(|m| matches!(m.sender, Sender::Agent { .. }))
        .flat_map(|m| {
            m.body
                .lines()
                .filter_map(|l| l.strip_prefix("event="))
                .map(|k| normalized_key(k.trim()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A comment that carries a crew header, judged: a valid message or a finding kind.
enum Judged {
    Valid(Box<Message>, bool),
    Bad(FindingKind, String),
}

/// `is_join` is true for a join request (registry, `kind=request`, `to=owner`): it is signed
/// with the key it carries, and the registry decides later.
fn judge(c: &Comment, cfg: &BoardConfig, reg: &Registry, seen: &mut Seen) -> Judged {
    if !c.by(&cfg.owner_login)
        && !reg.knows_login(&c.author)
        && !crate::registry::is_join_request(c, cfg)
    {
        return Judged::Bad(
            FindingKind::ForeignAuthor,
            format!(
                "crew header by {}, who is neither the owner nor the account of an approved agent",
                c.author
            ),
        );
    }
    let msg = match Message::parse(&c.body) {
        Ok(m) => m,
        Err(e) => return Judged::Bad(FindingKind::Malformed, e.to_string()),
    };
    if c.edited() {
        return Judged::Bad(
            FindingKind::Edited,
            format!(
                "claims from={}; created {}, updated {}",
                msg.header.from, c.created, c.updated
            ),
        );
    }
    let in_registry = c.repo == cfg.main_repo() && c.issue == cfg.registry_issue;
    if in_registry
        && msg.header.kind == Kind::Request
        && msg.header.to == crate::protocol::names::To::Owner
    {
        let former = cfg.former_for(&c.repo);
        return if verify_join(c, former).is_some() {
            match seen.first_use(msg.header.from.as_str(), &msg.header.sig, c.id) {
                Ok(()) => Judged::Valid(Box::new(msg), true),
                Err(r) => Judged::Bad(
                    FindingKind::Replayed,
                    format!("claims from={}: {r}", msg.header.from),
                ),
            }
        } else if let Some(r) = join_replay(c, former) {
            Judged::Bad(
                FindingKind::Replayed,
                format!("claims from={}: {r}", msg.header.from),
            )
        } else {
            Judged::Bad(
                FindingKind::BadSignature,
                format!(
                    "claims from={}; the join request does not verify against its own key",
                    msg.header.from
                ),
            )
        };
    }
    // The key is the one of the join that the owner approved. A span of the registry log must
    // cover the comment (history: an agent that left or was rejected since still counts for
    // its old messages; a message after the leave or the reject, or before the approval, does
    // not).
    let join = reg
        .agents
        .get(msg.header.from.as_str())
        .filter(|e| e.span_at(c.id).is_some())
        .and_then(|e| e.join.as_ref());
    // The approval bound the agent to one GitHub account (spec/identity#author-and-signature).
    if let Some(j) = join
        && !c.by(&j.login)
    {
        return Judged::Bad(
            FindingKind::ForeignAuthor,
            crate::registry::wrong_account(&c.author, msg.header.from.as_str(), &j.login),
        );
    }
    let key = join.and_then(|j| j.key());
    let Some(key) = key else {
        return Judged::Bad(
            FindingKind::UnknownSender,
            format!(
                "{} is not an approved agent with a registered key",
                msg.header.from
            ),
        );
    };
    match c.verify(&msg, &key, cfg.former_for(&c.repo)) {
        Ok(()) => match c
            .check_ts(&msg)
            .and_then(|()| seen.first_use(msg.header.from.as_str(), &msg.header.sig, c.id))
        {
            Ok(()) => Judged::Valid(Box::new(msg), false),
            Err(r) => Judged::Bad(
                FindingKind::Replayed,
                format!("claims from={}: {r}", msg.header.from),
            ),
        },
        Err(e) => Judged::Bad(
            FindingKind::BadSignature,
            format!("claims from={}: {e}", msg.header.from),
        ),
    }
}

/// The first line of `s`, trimmed, cut to `n` characters with `...` when it is longer.
fn excerpt(s: &str, n: usize) -> String {
    let l = s.lines().next().unwrap_or("").trim();
    if l.chars().count() > n {
        format!("{}...", l.chars().take(n).collect::<String>())
    } else {
        l.to_string()
    }
}

/// One valid message with what the writer rules need.
struct Valid {
    id: u64,
    created: i64,
    url: String,
    repo: String,
    issue: u64,
    created_text: String,
    author: String,
    from: String,
    role: Option<Role>,
    to: String,
    kind: Kind,
    body: String,
}

fn wrong_writers(valid: &[Valid], cfg: &BoardConfig) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut per_issue: BTreeMap<(String, u64), Vec<&Valid>> = BTreeMap::new();
    for v in valid {
        // The registry issue carries joins, approvals, old pings and pongs and lifecycle notices.
        if v.repo == cfg.main_repo() && v.issue == cfg.registry_issue {
            continue;
        }
        per_issue
            .entry((v.repo.clone(), v.issue))
            .or_default()
            .push(v);
    }
    for msgs in per_issue.values_mut() {
        msgs.sort_by_key(|v| v.id);
        let mut assigned: Option<String> = None;
        for v in msgs.iter() {
            let coordinator = v.role == Some(Role::Coordinator);
            let wrong = |detail: String| Finding {
                kind: FindingKind::WrongWriter,
                comment_id: v.id,
                repo: v.repo.clone(),
                issue: v.issue,
                url: v.url.clone(),
                author: v.author.clone(),
                created: v.created_text.clone(),
                created_epoch: v.created,
                detail,
                reported_to_owner: None,
            };
            match parse(Some(v.kind), &v.body) {
                Act::Assignment { .. } => {
                    if coordinator {
                        assigned = Some(v.to.clone());
                    } else {
                        out.push(wrong(format!(
                            "{} assigned a ticket to {}, but only the coordinator assigns",
                            v.from, v.to
                        )));
                    }
                }
                Act::BoardAnnounce if !coordinator => {
                    out.push(wrong(format!(
                        "{} announced a board item for this ticket, but only the coordinator puts tickets on the board",
                        v.from
                    )));
                }
                Act::Reassigned => {
                    if coordinator {
                        assigned = None;
                    } else {
                        out.push(wrong(format!(
                            "{} reassigned the ticket, but only the coordinator does",
                            v.from
                        )));
                    }
                }
                Act::Closed(_) if !coordinator => {
                    out.push(wrong(format!(
                        "{} closed or dropped the ticket, but only the coordinator does",
                        v.from
                    )));
                }
                Act::Verify { .. } if !coordinator => {
                    out.push(wrong(format!(
                        "{} posted a verify result, but only the coordinator verifies",
                        v.from
                    )));
                }
                _ => {}
            }
            if matches!(v.kind, Kind::Ack | Kind::Done | Kind::Blocked)
                && assigned.as_deref() != Some(v.from.as_str())
            {
                out.push(wrong(format!(
                    "kind={} by {} but the ticket is {}",
                    v.kind,
                    v.from,
                    match &assigned {
                        Some(a) => format!("assigned to {a}"),
                        None => "not assigned to anybody".to_string(),
                    }
                )));
            }
        }
    }
    out
}

const OWNER_EDIT: &str = "an owner edit is not in the log";

fn mismatch(repo: &str, issue: u64, url: String, detail: String) -> Finding {
    Finding {
        kind: FindingKind::FieldMismatch,
        comment_id: 0,
        repo: repo.to_string(),
        issue,
        url,
        author: "board".to_string(),
        created: String::new(),
        // A mismatch is the state now, not an event in time: `--since` never hides it.
        created_epoch: i64::MAX,
        detail: format!("{detail}; {OWNER_EDIT}"),
        reported_to_owner: None,
    }
}

/// The messages of one ticket as the fold wants them, in comment order: the valid signed
/// messages and the owner comments that count (an owner `/crew move`, spec/tickets#owner-move).
fn ticket_msgs(valid: &[Valid], owner: &[&Comment], repo: &str, issue: u64) -> Vec<VMsg> {
    let mut msgs: Vec<&Valid> = valid
        .iter()
        .filter(|v| v.repo == repo && v.issue == issue)
        .collect();
    msgs.sort_by_key(|v| v.id);
    let owner_msgs = owner
        .iter()
        .filter(|c| c.repo == repo && c.issue == issue)
        .map(|c| VMsg {
            ts: String::new(),
            comment_id: c.id,
            created: c.created_epoch,
            sender: Sender::Owner,
            to: "coordinator".into(),
            kind: None,
            re: Re::None,
            body: c.body.clone(),
        });
    let mut out: Vec<VMsg> = msgs
        .into_iter()
        .filter_map(|v| {
            Some(VMsg {
                ts: String::new(),
                comment_id: v.id,
                created: v.created,
                sender: Sender::Agent {
                    name: v.from.clone(),
                    role: v.role?,
                },
                to: v.to.clone(),
                kind: Some(v.kind),
                re: Re::None,
                body: v.body.clone(),
            })
        })
        .chain(owner_msgs)
        .collect();
    out.sort_by_key(|m| m.comment_id);
    out
}

/// The states in which an agent may speak: none of them fits an agent that left or has no
/// approval.
fn speaks(state: &str) -> bool {
    matches!(
        State::parse(state),
        Some(State::Active | State::Paused | State::Draining)
    )
}

fn field_mismatches(
    valid: &[Valid],
    owner: &[&Comment],
    board: &BoardView,
    reg: &Registry,
    cfg: &BoardConfig,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in &board.tickets {
        let Some((repo, n)) = t.key.rsplit_once('#') else {
            continue;
        };
        let Ok(issue) = n.parse::<u64>() else {
            continue;
        };
        let url = format!("https://github.com/{repo}/issues/{issue}");
        let log = fold_ticket(&ticket_msgs(valid, owner, repo, issue));
        let status = t.status.as_deref();
        if status_contradicts(&log, status) {
            out.push(mismatch(
                repo,
                issue,
                url.clone(),
                status_mismatch_text(&t.key, status, &log),
            ));
        }
        if t.agent != log.agent {
            out.push(mismatch(
                repo,
                issue,
                url,
                format!(
                    "ticket {}: board Agent is {}, the log shows {}",
                    t.key,
                    t.agent.as_deref().unwrap_or("empty"),
                    log.agent.as_deref().unwrap_or("nobody")
                ),
            ));
        }
    }
    let (repo, issue) = (cfg.main_repo(), cfg.registry_issue);
    let url = format!("https://github.com/{repo}/issues/{issue}");
    for a in &board.agents {
        let log = fold_agent(reg, &a.name);
        let mut say = |what: String| {
            out.push(mismatch(
                repo,
                issue,
                url.clone(),
                format!("agent {}: {what}", a.name),
            ));
        };
        if let Some(role) = log.role {
            let now = a
                .role
                .as_deref()
                .and_then(|r| Role::parse(&r.to_lowercase()));
            if now != Some(role) {
                say(format!(
                    "board Role is {}, the registry shows {}",
                    a.role.as_deref().unwrap_or("empty"),
                    if role == Role::Coordinator {
                        "coordinator"
                    } else {
                        "dev"
                    }
                ));
            }
        }
        if let Some(fp) = &log.key_fp {
            let now = a
                .key
                .as_deref()
                .and_then(|k| PublicKey::from_openssh(k).ok())
                .map(|k| fingerprint(&k));
            if now.as_ref() != Some(fp) {
                say(format!(
                    "board Key ({}) is not the key of the join ({fp})",
                    now.as_deref().unwrap_or("empty or unreadable")
                ));
            }
        }
        let state = a.state.as_deref().unwrap_or("");
        if log.left && State::parse(state) != Some(State::Left) {
            say(format!(
                "board State is {}, the log shows a left marker",
                if state.is_empty() { "empty" } else { state }
            ));
        } else if log.ended && !log.left && speaks(state) {
            say(format!(
                "board State is {state}, the owner rejected the agent"
            ));
        } else if !log.approved && speaks(state) {
            say(format!(
                "board State is {state}, the log shows no approval of the agent"
            ));
        } else if log.approved
            && !log.stopped
            && matches!(State::parse(state), Some(State::Left | State::Draining))
        {
            say(format!(
                "board State is {state}, the log shows no stop of the agent (no /crew stop, no signed notice or marker of the agent, no /crew reject)"
            ));
        }
    }
    out
}

/// Audit all comments of all repos of the initiative.
///
/// `heartbeats`: the last heartbeat (seconds) of agents that run now; their window reaches
/// at least that far. `grace`: seconds added to the end of every window (3 heartbeat
/// intervals: an agent that is quiet for a while is still running).
/// `reported`: keys of security events that a signed message reported to the owner already.
/// `board`: the ticket and agent fields as the board shows them now.
pub fn audit(
    cfg: &BoardConfig,
    reg: &Registry,
    comments: &[Comment],
    heartbeats: &BTreeMap<String, i64>,
    grace: i64,
    reported: &HashSet<String>,
    board: &BoardView,
) -> Report {
    let mut sorted: Vec<&Comment> = comments.iter().collect();
    sorted.sort_by_key(|c| (c.created_epoch, c.id));
    let mut findings = Vec::new();
    let mut valid: Vec<Valid> = Vec::new();
    let mut foreign_comments = 0usize;
    // first and last signed message per agent
    let mut span: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    let mut owner_plain: Vec<&Comment> = Vec::new();
    let mut seen = Seen::default();
    for c in &sorted {
        if c.body.starts_with(CREW_PREFIX) {
            match judge(c, cfg, reg, &mut seen) {
                Judged::Valid(msg, _is_join) => {
                    let from = msg.header.from.to_string();
                    let e = span
                        .entry(from.clone())
                        .or_insert((c.created_epoch, c.created_epoch));
                    e.0 = e.0.min(c.created_epoch);
                    e.1 = e.1.max(c.created_epoch);
                    valid.push(Valid {
                        id: c.id,
                        created: c.created_epoch,
                        url: c.url.clone(),
                        repo: c.repo.clone(),
                        issue: c.issue,
                        created_text: c.created.clone(),
                        author: c.author.clone(),
                        role: reg
                            .agents
                            .get(&from)
                            .and_then(|a| a.span_at(c.id))
                            .map(|s| s.role),
                        from,
                        to: msg.header.to.to_string(),
                        kind: msg.header.kind,
                        body: msg.body.clone(),
                    });
                }
                Judged::Bad(kind, detail) => {
                    let key = event_key(&c.repo, c.issue, c.id, kind.word());
                    findings.push(Finding {
                        kind,
                        comment_id: c.id,
                        repo: c.repo.clone(),
                        issue: c.issue,
                        url: c.url.clone(),
                        author: c.author.clone(),
                        created: c.created.clone(),
                        created_epoch: c.created_epoch,
                        detail,
                        reported_to_owner: Some(reported.contains(&key)),
                    });
                }
            }
        } else if let (true, Some(editor)) = (
            c.by(&cfg.owner_login),
            c.untrusted_owner_edit(&cfg.owner_login),
        ) {
            // An owner comment that somebody else edited (spec/security-details#edited-owner-comments).
            let key = event_key(&c.repo, c.issue, c.id, FindingKind::Edited.word());
            findings.push(Finding {
                kind: FindingKind::Edited,
                comment_id: c.id,
                repo: c.repo.clone(),
                issue: c.issue,
                url: c.url.clone(),
                author: c.author.clone(),
                created: c.created.clone(),
                created_epoch: c.created_epoch,
                detail: format!(
                    "owner comment edited by {editor}; created {}, updated {}",
                    c.created, c.updated
                ),
                reported_to_owner: Some(reported.contains(&key)),
            });
        } else if c.by(&cfg.owner_login) {
            owner_plain.push(c);
        } else {
            // Neither the owner nor a crew header: a stranger, or an agent account posting
            // without a header (it is no owner message and no signed message).
            foreign_comments += 1;
        }
    }
    let mut windows: Vec<Window> = span
        .iter()
        .map(|(agent, (a, b))| {
            let end = heartbeats.get(agent).copied().unwrap_or(0).max(*b);
            Window {
                agent: agent.clone(),
                from: *a,
                to: end + grace,
            }
        })
        .collect();
    windows.sort_by(|a, b| (a.from, &a.agent).cmp(&(b.from, &b.agent)));
    let mut outside = 0usize;
    for c in owner_plain.iter().copied() {
        let running: Vec<&str> = windows
            .iter()
            .filter(|w| w.from <= c.created_epoch && c.created_epoch <= w.to)
            .map(|w| w.agent.as_str())
            .collect();
        if running.is_empty() {
            outside += 1;
            continue;
        }
        let what = if c.body.trim_start().starts_with("/crew") {
            "owner command"
        } else {
            "text"
        };
        findings.push(Finding {
            kind: FindingKind::HeaderlessOwner,
            comment_id: c.id,
            repo: c.repo.clone(),
            issue: c.issue,
            url: c.url.clone(),
            author: c.author.clone(),
            created: c.created.clone(),
            created_epoch: c.created_epoch,
            detail: format!(
                "{what} \"{}\" while {} ran",
                excerpt(&c.body, 50),
                running.join(", ")
            ),
            reported_to_owner: None,
        });
    }
    findings.extend(wrong_writers(&valid, cfg));
    findings.extend(field_mismatches(&valid, &owner_plain, board, reg, cfg));
    findings.sort_by_key(|f| (f.created_epoch, f.comment_id));
    Report {
        scanned: comments.len(),
        windows,
        findings,
        owner_comments_outside_sessions: outside,
        foreign_comments,
    }
}

#[cfg(test)]
#[path = "audit_tests.rs"]
mod tests;
