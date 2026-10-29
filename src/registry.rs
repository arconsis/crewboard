//! The registry issue as a source of truth for who may speak (spec/identity,
//! spec/board#registry-and-lifecycle).
//!
//! Inputs are the comments of the registry issue. Outputs are who joined, who the owner
//! approved, with which role, and which key. Everything here is pure; the caller reads the
//! comments and the agent items from GitHub.
//!
//! Binding rule (interpretation of spec/board#lifecycle, spec/messages#signature): an approval
//! binds the latest valid join of that name that precedes it. If the approval names
//! `key=<fingerprint>`, that join must have exactly this key, else the approval is void. If it
//! names `login=<github-login>`, the join must be a comment of that account, else the approval is
//! void. After an approval, later joins of the same name are ignored, so a forged second join
//! cannot swap the key or the account.
//!
//! Accounts (spec/identity#author-and-signature): a join is read from any GitHub account; the
//! account that wrote it is the login of the agent, bound by the approval together with the key.
//! A message of the agent counts only from that account.

use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::items::ItemSnapshot;
use crate::owner_cmd::{self, OwnerCommand};
use crate::protocol::former::FormerRepo;
use crate::protocol::header::Message;
use crate::protocol::names::{AgentName, Kind, To};
use crate::protocol::replay::{Replay, Seen};
use crate::rules::Role;
use ssh_key::{HashAlg, PublicKey};
use std::collections::{BTreeMap, HashMap};

pub const CREW_PREFIX: &str = crate::protocol::header::PREFIX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinInfo {
    pub name: AgentName,
    pub item_id: String,
    /// Public key line as posted.
    pub key_line: String,
    pub runtime: String,
    pub host: String,
    pub comment_id: u64,
    /// The GitHub login that wrote the join comment: the account of the agent.
    pub login: String,
}

impl JoinInfo {
    pub fn key(&self) -> Option<PublicKey> {
        PublicKey::from_openssh(&self.key_line).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalInfo {
    pub role: Role,
    pub key_fp: Option<String>,
    pub comment_id: u64,
}

/// A stretch of the registry log in which an agent may speak with one role. `from` and `to` are
/// comment ids of the registry issue. GitHub comment ids rise with time across all issues, so a
/// ticket comment id compares with them. A message counts when `from < id <= to` (the marker
/// that ends a span counts itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub role: Role,
    pub from: u64,
    pub to: Option<u64>,
}

impl Span {
    pub fn covers(&self, comment_id: u64) -> bool {
        self.from < comment_id && comment_id <= self.to.unwrap_or(u64::MAX)
    }
}

/// How the registry log stops or ends an agent (spec/board#lifecycle). The comment id is the
/// log entry. The coordinator returns the open tickets of such an agent; a board `State` alone
/// never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Departure {
    /// The agent's own signed notice that it drains. Its span stays open until its marker.
    Stopping(u64),
    /// The owner's `/crew stop <name>`. The span stays open: a running agent drains and signs
    /// its marker; the key of an agent that cannot drain stays valid until `/crew reject`.
    OwnerStop(u64),
    /// The agent's signed `lifecycle: left` marker. The span ended.
    Left(u64),
    /// The owner's `/crew reject <name>`. The span ended.
    Rejected(u64),
}

impl Departure {
    /// The span ended: the agent may not act any more.
    pub fn ended(self) -> bool {
        matches!(self, Departure::Left(_) | Departure::Rejected(_))
    }

    /// What the log says, after the agent name: the reason of a derived reassign.
    pub fn reason(self) -> &'static str {
        match self {
            Departure::Stopping(_) => "is stopping (its signed notice in the registry)",
            Departure::OwnerStop(_) => "is stopping (the owner's /crew stop)",
            Departure::Left(_) => "has left (its signed lifecycle: left marker)",
            Departure::Rejected(_) => "was rejected (the owner's /crew reject)",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEntry {
    pub name: String,
    /// The join that is bound by the approval, or the latest pending join.
    pub join: Option<JoinInfo>,
    /// Undecided joins of this name with another key than `join`, replaced by a later join. An
    /// approval without `key=` is void while this is not empty (it would bind a key that the
    /// owner did not see); an approval with `key=` may name one of them.
    pub earlier_joins: Vec<JoinInfo>,
    /// The CURRENT approval; `role` is the role now. The history is in `spans`.
    pub approval: Option<ApprovalInfo>,
    pub rejected: bool,
    /// Where the agent may speak, in registry comment order: approve, role change, reject and
    /// the `lifecycle: left` marker. `/crew stop` does not end a span.
    pub spans: Vec<Span>,
    /// The log entry that stops or ends the agent now: an end (marker, reject) replaces a stop.
    pub departure: Option<Departure>,
}

fn join_fp(j: &JoinInfo) -> Option<String> {
    j.key().map(|k| fingerprint(&k))
}

impl AgentEntry {
    /// A later undecided join of the name replaces the earlier one. An earlier join with
    /// another key stays in `earlier_joins`: the name is ambiguous until the owner decides.
    fn replace_join(&mut self, new: JoinInfo) {
        let new_fp = join_fp(&new);
        if let Some(old) = self.join.take()
            && join_fp(&old) != new_fp
        {
            self.earlier_joins.retain(|j| join_fp(j) != join_fp(&old));
            self.earlier_joins.push(old);
        }
        self.earlier_joins.retain(|j| join_fp(j) != new_fp);
        self.join = Some(new);
    }

    /// Record a stop or an end. The stronger entry wins (agent notice, owner stop, marker,
    /// reject); an entry of the same strength keeps the first.
    fn depart(&mut self, d: Departure) {
        let rank = |d: Departure| match d {
            Departure::Stopping(_) => 0,
            Departure::OwnerStop(_) => 1,
            Departure::Left(_) => 2,
            Departure::Rejected(_) => 3,
        };
        if self.departure.is_none_or(|old| rank(d) > rank(old)) {
            self.departure = Some(d);
        }
    }

    fn open_span(&self) -> Option<&Span> {
        self.spans.last().filter(|s| s.to.is_none())
    }

    fn close_span(&mut self, comment_id: u64) {
        if let Some(s) = self.spans.last_mut()
            && s.to.is_none()
        {
            s.to = Some(comment_id);
        }
    }

    /// The span that covers a message with this comment id.
    pub fn span_at(&self, comment_id: u64) -> Option<&Span> {
        self.spans.iter().find(|s| s.covers(comment_id))
    }
}

impl AgentEntry {
    pub fn approved(&self) -> bool {
        self.approval.is_some() && !self.rejected
    }

    /// The GitHub login of the agent: the account of the bound join (or of the latest pending
    /// join). Its messages count only from this account.
    pub fn login(&self) -> Option<&str> {
        self.join.as_ref().map(|j| j.login.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    pub agents: BTreeMap<String, AgentEntry>,
    /// Things that were ignored on purpose (void approvals, late joins, second
    /// coordinator). `status` shows them.
    pub notes: Vec<String>,
}

impl Registry {
    /// `login` is the account of an agent that the owner approved (an agent with a span).
    pub fn knows_login(&self, login: &str) -> bool {
        self.agents
            .values()
            .filter(|e| !e.spans.is_empty())
            .filter_map(AgentEntry::login)
            .any(|l| l.eq_ignore_ascii_case(login))
    }
}

pub fn fingerprint(k: &PublicKey) -> String {
    k.fingerprint(HashAlg::Sha256).to_string()
}

/// Body of a join request: `key=value` lines (spec/board#item-ids-are-the-source-of-truth: carries
/// the agent item id).
pub fn join_body(item_id: &str, key_line: &str, runtime: &str, host: &str) -> String {
    format!("Join request.\nitem={item_id}\nkey={key_line}\nruntime={runtime}\nhost={host}\n")
}

/// `login` is the GitHub author of the comment.
pub fn parse_join(msg: &Message, comment_id: u64, login: &str) -> Option<JoinInfo> {
    if msg.header.kind != Kind::Request || msg.header.to != To::Owner {
        return None;
    }
    let (mut item, mut key, mut runtime, mut host) = (None, None, None, None);
    for l in msg.body.lines() {
        if let Some((k, v)) = l.split_once('=') {
            match k {
                "item" => item = Some(v.trim().to_string()),
                "key" => key = Some(v.trim().to_string()),
                "runtime" => runtime = Some(v.trim().to_string()),
                "host" => host = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    Some(JoinInfo {
        name: msg.header.from.clone(),
        item_id: item.filter(|s| !s.is_empty())?,
        key_line: key.filter(|s| !s.is_empty())?,
        runtime: runtime.unwrap_or_default(),
        host: host.unwrap_or_default(),
        comment_id,
        login: login.to_string(),
    })
}

/// A join comment whose signature verifies against the key it carries.
fn signed_join(c: &Comment, former: &[FormerRepo]) -> Option<(Message, JoinInfo)> {
    let msg = Message::parse(&c.body).ok()?;
    let join = parse_join(&msg, c.id, &c.author)?;
    let key = join.key()?;
    c.verify(&msg, &key, former).ok()?;
    Some((msg, join))
}

/// The signature of the header of a comment, if it parses.
fn join_sig(c: &Comment) -> Option<String> {
    Message::parse(&c.body).ok().map(|m| m.header.sig)
}

/// Verify a join comment against the key it carries and the replay window. Returns the join if
/// it is valid. `former` is the list of former names for the repo of `c`
/// (`BoardConfig::former_for`).
pub fn verify_join(c: &Comment, former: &[FormerRepo]) -> Option<JoinInfo> {
    let (msg, join) = signed_join(c, former)?;
    c.check_ts(&msg).ok()?;
    Some(join)
}

/// The replay finding of a join comment that is signed correctly but outside the replay window.
pub fn join_replay(c: &Comment, former: &[FormerRepo]) -> Option<Replay> {
    let (msg, _) = signed_join(c, former)?;
    c.check_ts(&msg).err()
}

/// Read the registry issue comments (any order; they are sorted by id here). A join is read from
/// any author; a left marker or a stopping notice only from the account of the agent; an owner
/// command only from the owner. Every other comment is ignored.
pub fn resolve(cfg: &BoardConfig, comments: &[Comment]) -> Registry {
    let (main_repo, issue) = (cfg.main_repo(), cfg.registry_issue);
    let mut sorted: Vec<&Comment> = comments
        .iter()
        .filter(|c| c.repo == main_repo && c.issue == issue)
        .collect();
    sorted.sort_by_key(|c| c.id);
    let mut reg = Registry::default();
    let mut seen = Seen::default();
    for c in sorted {
        if c.body.starts_with(CREW_PREFIX) {
            // Edited messages are invalid (spec/messages#signature).
            if c.edited() {
                continue;
            }
            let former = cfg.former_for(&c.repo);
            let Some(join) = verify_join(c, former) else {
                apply_left_marker(&mut reg, c, former, &mut seen);
                continue;
            };
            // A copy of a join inside the window changes nothing (spec/messages#replay-window).
            if let Some(sig) = join_sig(c)
                && let Err(r) = seen.first_use(join.name.as_str(), &sig, c.id)
            {
                reg.notes
                    .push(format!("join comment {} ignored: {r}", c.id));
                continue;
            }
            let key = join.name.to_string();
            let entry = reg.agents.entry(key.clone()).or_insert_with(|| AgentEntry {
                name: key.clone(),
                join: None,
                earlier_joins: Vec::new(),
                approval: None,
                rejected: false,
                spans: Vec::new(),
                departure: None,
            });
            if entry.approval.is_some() || entry.rejected {
                reg.notes.push(format!(
                    "join of {key} (comment {}) ignored: the agent was already decided",
                    c.id
                ));
            } else {
                entry.replace_join(join);
            }
        } else if c.by(&cfg.owner_login) {
            if let Some(editor) = c.untrusted_owner_edit(&cfg.owner_login) {
                reg.notes.push(format!(
                    "owner comment {} ignored: it was edited by {editor}",
                    c.id
                ));
                continue;
            }
            match owner_cmd::parse(&c.body) {
                Ok(Some(cmd)) => apply_owner(&mut reg, cfg, c.id, cmd),
                Ok(None) => {}
                Err(r) => reg
                    .notes
                    .push(format!("owner comment {}: {}", c.id, r.message)),
            }
        }
    }
    reg
}

/// A signed `info` of an agent with an open span and the line `lifecycle: left` ends that span.
/// A signed `info` that the agent is stopping (draining) records a stop and keeps the span open.
/// The signature is checked with the join key, and the author must be the account of the agent.
/// A marker of another name or account, with a bad signature, or from an agent without an open
/// span, is ignored.
fn apply_left_marker(reg: &mut Registry, c: &Comment, former: &[FormerRepo], seen: &mut Seen) {
    use crate::protocol::acts::{Act, parse};
    let Ok(msg) = Message::parse(&c.body) else {
        return;
    };
    let left = match parse(Some(msg.header.kind), &msg.body) {
        Act::Left => true,
        Act::Stopping { name } if name == msg.header.from.as_str() => false,
        _ => return,
    };
    let Some(entry) = reg.agents.get_mut(msg.header.from.as_str()) else {
        return;
    };
    let Some(key) = entry.join.as_ref().and_then(|j| j.key()) else {
        return;
    };
    if entry.login().is_none_or(|l| !c.by(l)) {
        return;
    }
    if entry.open_span().is_none()
        || c.verify(&msg, &key, former).is_err()
        || c.check_ts(&msg).is_err()
    {
        return;
    }
    if seen
        .first_use(msg.header.from.as_str(), &msg.header.sig, c.id)
        .is_err()
    {
        return;
    }
    if left {
        entry.close_span(c.id);
        entry.depart(Departure::Left(c.id));
    } else {
        entry.depart(Departure::Stopping(c.id));
    }
}

/// The coordinator now: an agent with an open span of the role coordinator.
fn other_coordinator(reg: &Registry, name: &str) -> Option<String> {
    reg.agents
        .values()
        .find(|a| {
            a.name != name
                && a.approved()
                && a.open_span().is_some_and(|s| s.role == Role::Coordinator)
        })
        .map(|a| a.name.clone())
}

fn apply_owner(reg: &mut Registry, cfg: &BoardConfig, comment_id: u64, cmd: OwnerCommand) {
    match cmd {
        OwnerCommand::Approve {
            name,
            role,
            key_fp,
            item,
            login,
        } => {
            let n = name.to_string();
            if role == Role::Coordinator
                && let Some(other) = other_coordinator(reg, &n)
            {
                reg.notes.push(format!(
                    "approval of {n} as coordinator (comment {comment_id}) is void: {other} is the coordinator"
                ));
                return;
            }
            let Some(entry) = reg.agents.get_mut(&n) else {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: no join"
                ));
                return;
            };
            if entry.approval.is_some() || entry.rejected {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) ignored: already decided"
                ));
                return;
            }
            let Some(latest) = entry.join.clone() else {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: no join"
                ));
                return;
            };
            // An approval without `key=` binds the one join that exists. With more than one
            // undecided join (another key) it would bind a key that the owner did not see.
            if key_fp.is_none() && !entry.earlier_joins.is_empty() {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: {} joins with other keys exist for this name; approve with key=<fingerprint>",
                    entry.earlier_joins.len() + 1
                ));
                return;
            }
            let join = match &key_fp {
                Some(want) => std::iter::once(&latest)
                    .chain(entry.earlier_joins.iter())
                    .find(|j| join_fp(j).as_ref() == Some(want))
                    .cloned(),
                None => Some(latest),
            };
            let Some(join) = join else {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: key {} is not the key of a join",
                    key_fp.as_deref().unwrap_or("?")
                ));
                return;
            };
            let fp = join_fp(&join);
            if let Some(want) = &item
                && want != &join.item_id
            {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: item {want} is not the item of the join"
                ));
                return;
            }
            if let Some(want) = &login
                && !want.eq_ignore_ascii_case(&join.login)
            {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: login {want} is not the account of the join ({})",
                    join.login
                ));
                return;
            }
            if let Some(why) = owner_account_refusal(cfg, &join.login) {
                reg.notes.push(format!(
                    "approval of {n} (comment {comment_id}) is void: {why}"
                ));
                return;
            }
            entry.join = Some(join);
            entry.earlier_joins.clear();
            entry.approval = Some(ApprovalInfo {
                role,
                key_fp: fp,
                comment_id,
            });
            entry.spans.push(Span {
                role,
                from: comment_id,
                to: None,
            });
        }
        OwnerCommand::Reject { name } => {
            if let Some(e) = reg.agents.get_mut(name.as_str()) {
                e.rejected = true;
                e.close_span(comment_id);
                e.depart(Departure::Rejected(comment_id));
            }
        }
        // The owner's stop is in the log: the coordinator returns the tickets of the agent. The
        // span stays open, so a running agent drains and signs its marker (spec/board#lifecycle).
        OwnerCommand::Stop { name } => {
            if let Some(e) = reg.agents.get_mut(name.as_str())
                && e.approved()
            {
                e.depart(Departure::OwnerStop(comment_id));
            }
        }
        OwnerCommand::SetRole { name, role } => {
            let n = name.to_string();
            if role == Role::Coordinator
                && let Some(other) = other_coordinator(reg, &n)
            {
                reg.notes.push(format!(
                    "role change of {n} (comment {comment_id}) is void: {other} is the coordinator"
                ));
                return;
            }
            if let Some(e) = reg.agents.get_mut(&n)
                && let Some(a) = e.approval.as_mut()
            {
                a.role = role;
                // The history keeps the old role: close the open span, start a new one. An
                // agent that has left or was rejected has no open span and gets none.
                if e.open_span().is_some() {
                    e.close_span(comment_id);
                    e.spans.push(Span {
                        role,
                        from: comment_id,
                        to: None,
                    });
                }
            }
        }
        // a profile change is applied to the item by the command; it does not decide who may
        // speak. A move belongs on a ticket; in the registry issue it changes nothing.
        OwnerCommand::Profile { .. }
        | OwnerCommand::Pause { .. }
        | OwnerCommand::Resume { .. }
        | OwnerCommand::Move { .. } => {}
    }
}

/// A comment of the registry issue with the header of a join request (`kind=request`,
/// `to=owner`). Whether it is a valid join, the signature decides later. Any account may post
/// one.
pub fn is_join_request(c: &Comment, cfg: &BoardConfig) -> bool {
    c.repo == cfg.main_repo()
        && c.issue == cfg.registry_issue
        && Message::parse(&c.body)
            .is_ok_and(|m| m.header.kind == Kind::Request && m.header.to == To::Owner)
}

/// The detail of a signed message that another account than the sender's posted.
pub fn wrong_account(author: &str, from: &str, login: &str) -> String {
    format!(
        "comment with a crew header from={from} by {author}; the owner approved {from} as the account {login}"
    )
}

/// Why a join of `login` may not be approved: the owner's account on a crew board (agents may
/// not act as the owner). Only the one-person trial (`insecure_one_account`) lets the agents use
/// the owner's account (spec/identity#identity-modes). `None`: the account may be approved.
pub fn owner_account_refusal(cfg: &BoardConfig, login: &str) -> Option<String> {
    (!cfg.insecure_one_account && login.eq_ignore_ascii_case(&cfg.owner_login)).then(|| {
        format!(
            "the join is a comment of the owner account {login}; agents may not act as the owner on a crew board (only a one-person trial with insecure_one_account allows it)"
        )
    })
}

/// The item States that fit an approved agent whose span is open. Any other State is a
/// contradiction that `status` reports; it does not mute the agent (the log decides).
pub fn state_may_speak(state: &str) -> bool {
    matches!(
        state,
        "approved" | "active" | "working" | "paused" | "draining"
    )
}

#[derive(Debug, Clone)]
pub struct TrustedAgent {
    pub name: AgentName,
    pub role: Role,
    pub item_id: String,
    /// The GitHub login that the approval bound: the account of the agent.
    pub login: String,
}

/// An agent in one span of the registry log: who it was, with which role and key, for the
/// comments `from < id <= to`. The key comes from the join, not from the agent item.
#[derive(Debug, Clone)]
pub struct TrustSpan {
    pub name: AgentName,
    pub role: Role,
    pub key: PublicKey,
    /// The GitHub login of the agent in this span: a message counts only from this account.
    pub login: String,
    pub from: u64,
    pub to: Option<u64>,
}

impl TrustSpan {
    fn covers(&self, comment_id: u64) -> bool {
        self.from < comment_id && comment_id <= self.to.unwrap_or(u64::MAX)
    }

    /// The comment is by the account of the agent.
    pub fn by_agent(&self, c: &Comment) -> bool {
        c.by(&self.login)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Trust {
    /// Who may act NOW: approved, the span open, the item readable on this board. The item
    /// `Key` and `State` do not decide it. For `me_of`, assignment, `status`.
    pub agents: BTreeMap<String, TrustedAgent>,
    /// Who was an agent at which comment, from the registry log alone. For judging a PAST
    /// message (`at`): a handover, a leave or a reject does not change history.
    pub spans: BTreeMap<String, Vec<TrustSpan>>,
    /// Why an approved agent is not trusted (span ended, item missing), and the item fields that
    /// contradict the log (`Key`, `State`) of an agent that stays trusted.
    pub problems: Vec<String>,
}

impl Trust {
    /// The agent now (present tense).
    pub fn get(&self, name: &str) -> Option<&TrustedAgent> {
        self.agents.get(name)
    }

    /// The agent as it was when `comment_id` was written: the span that covers the comment.
    /// `None` if no span covers it (before the approval, after a leave or a reject).
    pub fn at(&self, name: &str, comment_id: u64) -> Option<&TrustSpan> {
        self.spans.get(name)?.iter().find(|s| s.covers(comment_id))
    }

    pub fn coordinator_exists(&self) -> bool {
        self.agents.values().any(|a| a.role == Role::Coordinator)
    }

    /// `login` is the account of an agent that the owner approved (in any span). A comment with a
    /// crew header by any other account than the owner's is foreign
    /// (spec/messages#untrusted-input).
    pub fn knows_login(&self, login: &str) -> bool {
        self.spans
            .values()
            .flatten()
            .any(|s| s.login.eq_ignore_ascii_case(login))
    }
}

/// Combine the registry with the agent items read by id.
///
/// An approved agent is trusted if the registry log has not ended its span and its item exists
/// on this board. A `Key` or `State` of the item that contradicts the log is reported in
/// `problems`; it does not mute the agent (spec/board#fields: the signed log decides).
pub fn build_trust(
    reg: &Registry,
    items: &HashMap<String, ItemSnapshot>,
    project_number: u64,
) -> Trust {
    let mut t = Trust::default();
    // The spans come from the registry log alone: the item need not be readable.
    for e in reg.agents.values().filter(|e| !e.spans.is_empty()) {
        let Some(join) = &e.join else { continue };
        let Some(key) = join.key() else { continue };
        t.spans.insert(
            e.name.clone(),
            e.spans
                .iter()
                .map(|s| TrustSpan {
                    name: join.name.clone(),
                    role: s.role,
                    key: key.clone(),
                    login: join.login.clone(),
                    from: s.from,
                    to: s.to,
                })
                .collect(),
        );
    }
    for e in reg.agents.values().filter(|e| e.approved()) {
        let (Some(join), Some(appr)) = (&e.join, &e.approval) else {
            continue;
        };
        let problem = |m: &str| format!("{}: {m}", e.name);
        // The registry log decides first: an agent whose span ended (a left marker; a reject
        // already makes it not approved) may not act, whatever its item says.
        if e.open_span().is_none() {
            t.problems.push(problem(
                "the registry log ended this agent (left marker); an item State cannot undo it",
            ));
            continue;
        }
        let Some(item) = items.get(&join.item_id) else {
            t.problems.push(problem("agent item not readable by id"));
            continue;
        };
        if item.project_number.is_some_and(|n| n != project_number) {
            t.problems
                .push(problem("agent item belongs to another project"));
            continue;
        }
        let Some(join_key) = join.key() else {
            t.problems.push(problem("join key unreadable"));
            continue;
        };
        let item_key = item
            .field("Key")
            .and_then(|k| PublicKey::from_openssh(k).ok());
        if item_key.as_ref().map(|k| k.key_data()) != Some(join_key.key_data()) {
            // The key of the signed join decides who signs; the item `Key` is a mirror. A hand
            // edit of it must not silence the agent, so the agent stays trusted and the
            // contradiction is reported (`status`, `audit`).
            t.problems.push(problem(
                "Key field of the item differs from the key of the join; the signed join decides, fix the field",
            ));
        }
        // The State is the agent's own mirror. A forced stop that the log records (the owner's
        // `/crew stop`, the agent's notice) fits `left`. Anything else that does not fit an open
        // span is reported; the agent stays trusted, so one field write cannot mute it.
        let state = item.field("State").unwrap_or("");
        let logged_stop = state == "left" && e.departure.is_some();
        if !state_may_speak(state) && !logged_stop {
            t.problems.push(problem(&format!(
                "item State is '{state}' but the registry log has not stopped or ended this agent; the log decides, so it may still act (a hand edit, a forced stop by the coordinator, or an agent that has not joined fully yet)"
            )));
        }
        t.agents.insert(
            e.name.clone(),
            TrustedAgent {
                name: join.name.clone(),
                role: appr.role,
                item_id: join.item_id.clone(),
                login: join.login.clone(),
            },
        );
    }
    t
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::comments::tests::comment;
    use crate::protocol::names::{Re, Timestamp};
    use crate::protocol::signature::{Outgoing, generate_key, sign_message};
    use ssh_key::PrivateKey;

    /// A one-person trial board (`insecure_one_account`): the agents use the owner's account,
    /// so the helpers below post every comment as `owner-login`.
    pub fn cfg() -> BoardConfig {
        BoardConfig::parse(
            r#"
initiative = "t"
project_owner = "example-org"
project_number = 15
repos = ["example-org/app", "example-org/app-2"]
owner_login = "owner-login"
registry_issue = 2
insecure_one_account = true
"#,
        )
        .unwrap()
    }

    /// The same board as a crew: with the shared crew account `crew` (`crew_login`), or with
    /// `None` one own account per agent.
    pub fn crew_cfg(crew: Option<&str>) -> BoardConfig {
        let mut c = cfg();
        c.insecure_one_account = false;
        c.crew_login = crew.map(str::to_string);
        c
    }

    pub fn pub_line(k: &PrivateKey, name: &str) -> String {
        let mut p = k.public_key().clone();
        p.set_comment(name);
        p.to_openssh().unwrap()
    }

    /// A signed message as a comment of the registry issue, by `author`.
    pub fn signed(
        id: u64,
        author: &str,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        body: &str,
    ) -> Comment {
        let mut c = comment(id, author, "");
        let msg = sign_message(
            key,
            &c.issue_ref(),
            Outgoing {
                from: AgentName::parse(from).unwrap(),
                to: To::parse(to).unwrap(),
                kind,
                re: Re::None,
                ts: Timestamp::parse(&crate::clock::format_ts(c.created_epoch)).unwrap(),
            },
            body,
        )
        .unwrap();
        c.body = msg.to_comment();
        c
    }

    pub fn join_comment(id: u64, name: &str, key: &PrivateKey, item: &str) -> Comment {
        signed(
            id,
            "owner-login",
            key,
            name,
            "owner",
            Kind::Request,
            &join_body(item, &pub_line(key, name), "rt", "h"),
        )
    }

    pub fn owner(id: u64, text: &str) -> Comment {
        comment(id, "owner-login", text)
    }

    fn fp(k: &PrivateKey) -> String {
        fingerprint(k.public_key())
    }

    /// The marker that an agent posts when it leaves, signed by `key`, as `from`.
    pub fn left_marker(id: u64, from: &str, key: &PrivateKey) -> Comment {
        signed(
            id,
            "owner-login",
            key,
            from,
            "coordinator",
            Kind::Info,
            &crate::protocol::acts::left_body(from),
        )
    }

    fn no_items() -> HashMap<String, ItemSnapshot> {
        HashMap::new()
    }

    /// An edited owner comment, as the feed shows it after the editors were read.
    fn edited_by(mut c: Comment, editors: &[&str]) -> Comment {
        c.updated = "2026-10-15T14:09:00Z".into();
        c.updated_epoch += 540;
        c.editors = Some(editors.iter().map(|e| e.to_string()).collect());
        c
    }

    #[test]
    fn an_approval_that_somebody_else_edited_is_ignored_with_a_note_but_an_own_edit_counts() {
        let k = generate_key().unwrap();
        let text = format!("/crew approve claude@a role=dev key={} item=PVTI_a", fp(&k));
        for (editors, approved) in [
            (vec!["owner-login"], true),
            (vec!["owner-login", "crew-bot"], false),
            (vec![], false),
        ] {
            let reg = resolve(
                &cfg(),
                &[
                    join_comment(1, "claude@a", &k, "PVTI_a"),
                    edited_by(owner(2, &text), &editors),
                ],
            );
            assert_eq!(reg.agents["claude@a"].approved(), approved, "{editors:?}");
            assert_eq!(reg.notes.len(), usize::from(!approved), "{:?}", reg.notes);
        }
    }

    #[test]
    fn a_join_and_a_left_marker_outside_the_replay_window_do_not_count() {
        let k = generate_key().unwrap();
        // the same comments, posted again an hour after they were signed
        let late = |mut c: Comment| {
            c.created_epoch += 3_600;
            c
        };
        assert!(verify_join(&join_comment(1, "claude@a", &k, "PVTI_a"), &[]).is_some());
        let copy = late(join_comment(1, "claude@a", &k, "PVTI_a"));
        assert!(verify_join(&copy, &[]).is_none());
        assert!(join_replay(&copy, &[]).is_some());
        let approve = format!("/crew approve claude@a role=dev key={} item=PVTI_a", fp(&k));
        let comments = |marker: Comment| {
            vec![
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, &approve),
                marker,
            ]
        };
        let fresh = resolve(&cfg(), &comments(left_marker(3, "claude@a", &k)));
        assert!(fresh.agents["claude@a"].spans[0].to.is_some(), "control");
        let replayed = resolve(&cfg(), &comments(late(left_marker(3, "claude@a", &k))));
        assert!(replayed.agents["claude@a"].spans[0].to.is_none());
    }

    #[test]
    fn join_then_approve_makes_the_agent_approved() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(
                    2,
                    &format!("/crew approve claude@a role=dev key={} item=PVTI_a", fp(&k)),
                ),
            ],
        );
        let e = &reg.agents["claude@a"];
        assert!(e.approved());
        assert_eq!(e.approval.as_ref().unwrap().role, Role::Dev);
        assert_eq!(e.join.as_ref().unwrap().item_id, "PVTI_a");
        assert!(reg.notes.is_empty(), "{:?}", reg.notes);
    }

    #[test]
    fn with_a_crew_login_the_crew_account_joins_but_only_the_owner_approves() {
        let c = crew_cfg(Some("crew-bot"));
        let k = generate_key().unwrap();
        let mut join = join_comment(1, "claude@a", &k, "PVTI_a");
        join.author = "crew-bot".into();
        // the crew account tries to approve itself with a headerless command
        let mut self_approve = owner(2, "/crew approve claude@a role=coordinator");
        self_approve.author = "crew-bot".into();
        let reg = resolve(&c, &[join.clone(), self_approve]);
        assert!(
            reg.agents["claude@a"].join.is_some(),
            "the crew join counts"
        );
        assert!(
            !reg.agents["claude@a"].approved(),
            "a crew-authored command is no approval"
        );
        // control: the same command by the owner approves
        let reg = resolve(
            &c,
            &[join, owner(3, "/crew approve claude@a role=coordinator")],
        );
        assert!(reg.agents["claude@a"].approved());
    }

    #[test]
    fn no_approval_means_not_approved_and_unsigned_join_is_ignored() {
        let k = generate_key().unwrap();
        let reg = resolve(&cfg(), &[join_comment(1, "claude@a", &k, "PVTI_a")]);
        assert!(!reg.agents["claude@a"].approved());
        // a join signed by another key than the one in its body is not a join
        let other = generate_key().unwrap();
        let mut forged = join_comment(3, "hermes@b", &k, "PVTI_b");
        let wrong = signed(
            3,
            "owner-login",
            &other,
            "hermes@b",
            "owner",
            Kind::Request,
            &join_body("PVTI_b", &pub_line(&k, "hermes@b"), "rt", "h"),
        );
        forged.body = wrong.body;
        let reg = resolve(&cfg(), &[forged]);
        assert!(
            reg.agents.is_empty(),
            "body key != signing key must not register"
        );
    }

    #[test]
    fn approval_by_a_non_owner_or_with_a_wrong_key_is_void() {
        let k = generate_key().unwrap();
        let other = generate_key().unwrap();
        let join = join_comment(1, "claude@a", &k, "PVTI_a");
        // someone else posts the approval text
        let by_stranger = comment(2, "mallory", "/crew approve claude@a");
        let reg = resolve(&cfg(), &[join.clone(), by_stranger]);
        assert!(!reg.agents["claude@a"].approved());
        // owner names another key
        let wrong_key = owner(2, &format!("/crew approve claude@a key={}", fp(&other)));
        let reg = resolve(&cfg(), &[join.clone(), wrong_key]);
        assert!(!reg.agents["claude@a"].approved());
        assert!(
            reg.notes.iter().any(|n| n.contains("void")),
            "{:?}",
            reg.notes
        );
        // approval for a name that never joined
        let reg = resolve(&cfg(), &[owner(2, "/crew approve ghost@x")]);
        assert!(reg.agents.is_empty());
        assert!(reg.notes.iter().any(|n| n.contains("no join")));
    }

    #[test]
    fn a_late_second_join_cannot_swap_the_key_after_approval() {
        let k = generate_key().unwrap();
        let evil = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a"),
                join_comment(3, "claude@a", &evil, "PVTI_evil"),
            ],
        );
        let j = reg.agents["claude@a"].join.as_ref().unwrap();
        assert_eq!(j.item_id, "PVTI_a");
        assert_eq!(j.key().unwrap().key_data(), k.public_key().key_data());
    }

    #[test]
    fn an_approval_without_a_key_is_void_when_more_than_one_key_joined() {
        let k1 = generate_key().unwrap();
        let k2 = generate_key().unwrap();
        let joins = [
            join_comment(1, "claude@a", &k1, "PVTI_1"),
            join_comment(2, "claude@a", &k2, "PVTI_2"),
        ];
        let with = |approve: String| {
            let mut c = joins.to_vec();
            c.push(owner(3, &approve));
            resolve(&cfg(), &c)
        };
        // a bare approval binds no key that the owner saw
        let reg = with("/crew approve claude@a".into());
        assert!(!reg.agents["claude@a"].approved());
        assert!(
            reg.notes
                .iter()
                .any(|n| n.contains("void") && n.contains("key=<fingerprint>")),
            "{:?}",
            reg.notes
        );
        // the same with role and item, but no key
        let reg = with("/crew approve claude@a role=dev item=PVTI_2".into());
        assert!(!reg.agents["claude@a"].approved());
        // naming the later key approves the later join
        let reg = with(format!("/crew approve claude@a key={}", fp(&k2)));
        let e = &reg.agents["claude@a"];
        assert!(e.approved() && e.join.as_ref().unwrap().item_id == "PVTI_2");
        // naming the earlier key approves the earlier join (a later join cannot lock it out)
        let reg = with(format!("/crew approve claude@a key={}", fp(&k1)));
        let e = &reg.agents["claude@a"];
        assert!(e.approved() && e.join.as_ref().unwrap().item_id == "PVTI_1");
        assert_eq!(
            e.join.as_ref().unwrap().key().unwrap().key_data(),
            k1.public_key().key_data()
        );
        // a key that no join has
        let other = generate_key().unwrap();
        let reg = with(format!("/crew approve claude@a key={}", fp(&other)));
        assert!(!reg.agents["claude@a"].approved());
        // a join after the approval changes nothing
        let mut c = joins.to_vec();
        c.push(owner(3, &format!("/crew approve claude@a key={}", fp(&k1))));
        c.push(join_comment(4, "claude@a", &other, "PVTI_x"));
        let e = &resolve(&cfg(), &c).agents["claude@a"].clone();
        assert_eq!(e.join.as_ref().unwrap().item_id, "PVTI_1");
    }

    #[test]
    fn a_rejoin_with_the_same_key_keeps_a_bare_approval_valid() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_1"),
                join_comment(2, "claude@a", &k, "PVTI_2"),
                owner(3, "/crew approve claude@a"),
            ],
        );
        let e = &reg.agents["claude@a"];
        assert!(e.approved(), "{:?}", reg.notes);
        // before approval the latest join of the one key wins
        assert_eq!(e.join.as_ref().unwrap().item_id, "PVTI_2");
    }

    #[test]
    fn edited_join_is_ignored() {
        let k = generate_key().unwrap();
        let mut j = join_comment(1, "claude@a", &k, "PVTI_a");
        j.updated = "2026-10-15T15:00:00Z".into();
        assert!(resolve(&cfg(), &[j]).agents.is_empty());
    }

    #[test]
    fn only_one_coordinator_and_reject_works() {
        let (k1, k2) = (generate_key().unwrap(), generate_key().unwrap());
        let comments = [
            join_comment(1, "claude@a", &k1, "PVTI_a"),
            join_comment(2, "hermes@b", &k2, "PVTI_b"),
            owner(3, "/crew approve claude@a role=coordinator"),
            owner(4, "/crew approve hermes@b role=coordinator"),
        ];
        let reg = resolve(&cfg(), &comments);
        assert!(reg.agents["claude@a"].approved());
        assert!(
            !reg.agents["hermes@b"].approved(),
            "second coordinator is refused"
        );
        assert!(reg.notes.iter().any(|n| n.contains("is the coordinator")));
        // handover: owner makes claude a dev, then hermes can be coordinator
        let mut more = comments[..3].to_vec();
        more.push(owner(4, "/crew role claude@a dev"));
        more.push(owner(5, "/crew approve hermes@b role=coordinator"));
        let reg = resolve(&cfg(), &more);
        assert!(reg.agents["hermes@b"].approved());
        assert_eq!(
            reg.agents["claude@a"].approval.as_ref().unwrap().role,
            Role::Dev
        );
        // reject
        let reg = resolve(
            &cfg(),
            &[comments[0].clone(), owner(5, "/crew reject claude@a")],
        );
        assert!(!reg.agents["claude@a"].approved());
    }

    #[test]
    fn spans_follow_the_comment_order() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a role=dev"),
                owner(4, "/crew role claude@a coordinator"),
                owner(6, "/crew reject claude@a"),
            ],
        );
        let sp = |role, from, to| Span { role, from, to };
        assert_eq!(
            reg.agents["claude@a"].spans,
            vec![sp(Role::Dev, 2, Some(4)), sp(Role::Coordinator, 4, Some(6))]
        );
        // the current approval keeps the role now, for the callers that read it
        assert_eq!(
            reg.agents["claude@a"].approval.as_ref().unwrap().role,
            Role::Coordinator
        );
        let t = build_trust(&reg, &no_items(), 15);
        assert!(t.agents.is_empty(), "no item: nobody may act now");
        assert_eq!(
            t.spans["claude@a"].len(),
            2,
            "the spans do not need the item"
        );
        assert_eq!(t.at("claude@a", 3).unwrap().role, Role::Dev);
        assert_eq!(t.at("claude@a", 5).unwrap().role, Role::Coordinator);
        assert_eq!(
            t.at("claude@a", 5).unwrap().key.key_data(),
            k.public_key().key_data(),
            "the key is the key of the join"
        );
        assert!(t.at("hermes@b", 5).is_none(), "unknown name");
    }

    #[test]
    fn a_role_change_keeps_the_old_role_for_old_messages() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &ka, "PVTI_a"),
                join_comment(2, "hermes@b", &kb, "PVTI_b"),
                owner(3, "/crew approve claude@a role=coordinator"),
                owner(4, "/crew approve hermes@b role=dev"),
                owner(20, "/crew role claude@a dev"),
                owner(21, "/crew role hermes@b coordinator"),
            ],
        );
        assert!(reg.notes.is_empty(), "{:?}", reg.notes);
        let t = build_trust(&reg, &no_items(), 15);
        assert_eq!(t.at("claude@a", 10).unwrap().role, Role::Coordinator);
        assert_eq!(t.at("claude@a", 30).unwrap().role, Role::Dev);
        assert_eq!(t.at("hermes@b", 10).unwrap().role, Role::Dev);
        assert_eq!(t.at("hermes@b", 30).unwrap().role, Role::Coordinator);
    }

    #[test]
    fn a_rejected_agent_keeps_old_messages_and_loses_new_ones() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a role=dev"),
                owner(10, "/crew reject claude@a"),
                // a role change after the reject opens nothing
                owner(12, "/crew role claude@a coordinator"),
            ],
        );
        let t = build_trust(&reg, &no_items(), 15);
        assert!(t.at("claude@a", 9).is_some(), "before the reject");
        assert!(t.at("claude@a", 11).is_none(), "after the reject");
        assert!(
            t.at("claude@a", 13).is_none(),
            "a role change does not reopen"
        );
    }

    #[test]
    fn a_left_marker_ends_the_span_and_counts_itself() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a role=dev"),
                left_marker(8, "claude@a", &k),
                owner(9, "/crew role claude@a coordinator"),
            ],
        );
        let t = build_trust(&reg, &no_items(), 15);
        assert!(t.at("claude@a", 7).is_some());
        assert!(t.at("claude@a", 8).is_some(), "the marker counts itself");
        assert!(t.at("claude@a", 9).is_none(), "a message after the marker");
        assert!(
            t.at("claude@a", 50).is_none(),
            "and a role change does not reopen"
        );
        assert_eq!(reg.agents["claude@a"].spans.len(), 1);
    }

    #[test]
    fn a_left_marker_of_another_agent_or_with_a_bad_signature_is_ignored() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let base = [
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            owner(3, "/crew approve claude@a role=dev"),
            owner(4, "/crew approve hermes@b role=dev"),
        ];
        // b signs a marker that names a: the signature does not match a's key
        let mut forged = base.to_vec();
        forged.push(left_marker(8, "claude@a", &kb));
        let t = build_trust(&resolve(&cfg(), &forged), &no_items(), 15);
        assert!(
            t.at("claude@a", 9).is_some(),
            "a forged marker ends nothing"
        );
        // b's own marker ends b and not a
        let mut own = base.to_vec();
        own.push(left_marker(8, "hermes@b", &kb));
        let t = build_trust(&resolve(&cfg(), &own), &no_items(), 15);
        assert!(t.at("claude@a", 9).is_some());
        assert!(t.at("hermes@b", 9).is_none());
        // the line in a message of another kind is no marker
        let mut other_kind = base.to_vec();
        other_kind.push(signed(
            8,
            "owner-login",
            &ka,
            "claude@a",
            "coordinator",
            Kind::Report,
            crate::protocol::acts::LEFT_MARKER,
        ));
        let t = build_trust(&resolve(&cfg(), &other_kind), &no_items(), 15);
        assert!(t.at("claude@a", 9).is_some());
        // an edited marker is invalid
        let mut edited = base.to_vec();
        let mut m = left_marker(8, "claude@a", &ka);
        m.updated = "2026-10-15T15:00:00Z".into();
        edited.push(m);
        let t = build_trust(&resolve(&cfg(), &edited), &no_items(), 15);
        assert!(t.at("claude@a", 9).is_some());
        // a marker by a stranger account is no marker
        let mut stranger = base.to_vec();
        let mut m = left_marker(8, "claude@a", &ka);
        m.author = "mallory".into();
        stranger.push(m);
        let t = build_trust(&resolve(&cfg(), &stranger), &no_items(), 15);
        assert!(t.at("claude@a", 9).is_some());
    }

    #[test]
    fn a_message_before_the_approval_is_not_trusted() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(3, "/crew approve claude@a role=dev"),
            ],
        );
        let t = build_trust(&reg, &no_items(), 15);
        assert!(t.at("claude@a", 2).is_none(), "before the approval");
        assert!(
            t.at("claude@a", 3).is_none(),
            "the approval itself is not a message"
        );
        assert!(t.at("claude@a", 4).is_some());
        // an agent that was never approved has no span
        let reg = resolve(&cfg(), &[join_comment(1, "claude@a", &k, "PVTI_a")]);
        assert!(build_trust(&reg, &no_items(), 15).spans.is_empty());
    }

    #[test]
    fn a_left_coordinator_does_not_block_a_new_coordinator() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &ka, "PVTI_a"),
                join_comment(2, "hermes@b", &kb, "PVTI_b"),
                owner(3, "/crew approve claude@a role=coordinator"),
                owner(4, "/crew approve hermes@b role=coordinator"),
                left_marker(5, "claude@a", &ka),
                owner(6, "/crew approve hermes@b role=coordinator"),
            ],
        );
        // the first approval of b (4) is void: a was the coordinator; after a left, 6 works
        assert!(reg.agents["hermes@b"].approved(), "{:?}", reg.notes);
        let t = build_trust(&reg, &no_items(), 15);
        assert_eq!(t.at("hermes@b", 7).unwrap().role, Role::Coordinator);
        assert_eq!(t.at("claude@a", 4).unwrap().role, Role::Coordinator);
        assert!(t.at("claude@a", 7).is_none());
    }

    fn item(id: &str, key: &str, state: &str, project: u64) -> ItemSnapshot {
        let mut fields = BTreeMap::new();
        fields.insert("Key".to_string(), key.to_string());
        fields.insert("State".to_string(), state.to_string());
        ItemSnapshot {
            id: id.into(),
            updated_at: "2026-10-15T14:00:00Z".into(),
            item_type: "DRAFT_ISSUE".into(),
            project_number: Some(project),
            issue_state: None,
            fields,
        }
    }

    #[test]
    fn trust_needs_an_open_span_and_a_readable_item_and_reports_key_and_state() {
        let k = generate_key().unwrap();
        let reg = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a role=coordinator"),
            ],
        );
        let line = pub_line(&k, "claude@a");
        let ok = |state: &str, key: &str, proj: u64| {
            let mut m = HashMap::new();
            m.insert("PVTI_a".to_string(), item("PVTI_a", key, state, proj));
            build_trust(&reg, &m, 15).agents.contains_key("claude@a")
        };
        assert!(ok("active", &line, 15), "key, state and project agree");
        // The item State is a mirror too: a State that does not fit an open span is reported
        // and does not mute the agent (one field write must not silence it).
        for state in ["requested", "left", ""] {
            assert!(ok(state, &line, 15), "state '{state}': the log decides");
            let mut m = HashMap::new();
            m.insert("PVTI_a".to_string(), item("PVTI_a", &line, state, 15));
            let t = build_trust(&reg, &m, 15);
            assert!(
                t.problems.iter().any(|p| p.contains(&format!(
                    "item State is '{state}' but the registry log has not"
                ))),
                "{state}: {:?}",
                t.problems
            );
        }
        // control: a State that fits reports nothing
        for state in ["approved", "active", "paused", "draining"] {
            let mut m = HashMap::new();
            m.insert("PVTI_a".to_string(), item("PVTI_a", &line, state, 15));
            assert!(build_trust(&reg, &m, 15).problems.is_empty(), "{state}");
        }
        let other = pub_line(&generate_key().unwrap(), "claude@a");
        // The item Key is a mirror: a contradiction reports a problem and does not silence
        assert!(
            ok("active", &other, 15),
            "key differs: the signed join decides"
        );
        assert!(
            ok("active", "garbage", 15),
            "key unreadable: the signed join decides"
        );
        let mut m = HashMap::new();
        m.insert("PVTI_a".to_string(), item("PVTI_a", &other, "active", 15));
        let t = build_trust(&reg, &m, 15);
        assert!(
            t.problems.iter().any(|p| p.contains("Key field")),
            "{:?}",
            t.problems
        );
        assert!(!ok("active", &line, 99), "other project");
        // a left marker ends the agent in the registry: an item State of `active` cannot undo it
        let left = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew approve claude@a role=dev"),
                left_marker(3, "claude@a", &k),
            ],
        );
        let mut m = HashMap::new();
        m.insert("PVTI_a".to_string(), item("PVTI_a", &line, "active", 15));
        let t = build_trust(&left, &m, 15);
        assert!(!t.agents.contains_key("claude@a"));
        assert!(
            t.problems[0].contains("registry log ended"),
            "{:?}",
            t.problems
        );
        // item not readable
        let t = build_trust(&reg, &HashMap::new(), 15);
        assert!(t.agents.is_empty());
        assert!(t.problems[0].contains("not readable"));
        // role comes from the approval
        let mut m = HashMap::new();
        m.insert("PVTI_a".to_string(), item("PVTI_a", &line, "active", 15));
        let t = build_trust(&reg, &m, 15);
        assert_eq!(t.get("claude@a").unwrap().role, Role::Coordinator);
        assert!(t.coordinator_exists());
    }

    fn items_with(id: &str, key: &str, state: &str) -> HashMap<String, ItemSnapshot> {
        let mut m = HashMap::new();
        m.insert(id.to_string(), item(id, key, state, 15));
        m
    }

    #[test]
    fn the_owner_stop_is_a_departure_in_the_log_that_keeps_the_span_open() {
        let k = generate_key().unwrap();
        let line = pub_line(&k, "claude@a");
        let base = vec![
            join_comment(1, "claude@a", &k, "PVTI_a"),
            owner(2, "/crew approve claude@a role=dev"),
        ];
        // control: no stop, no departure
        assert_eq!(resolve(&cfg(), &base).agents["claude@a"].departure, None);
        let mut stopped = base.clone();
        stopped.push(owner(5, "/crew stop claude@a"));
        stopped.push(owner(6, "/crew stop claude@a"));
        let reg = resolve(&cfg(), &stopped);
        let e = &reg.agents["claude@a"];
        assert_eq!(e.departure, Some(Departure::OwnerStop(5)), "the first stop");
        assert_eq!(
            e.departure.unwrap().reason(),
            "is stopping (the owner's /crew stop)"
        );
        assert!(!e.departure.unwrap().ended());
        // the span stays open: the agent drains and signs its marker, which counts
        let t = build_trust(&reg, &items_with("PVTI_a", &line, "draining"), 15);
        assert!(t.at("claude@a", 9).is_some());
        assert!(t.get("claude@a").is_some());
        // the State `left` of a forced stop fits a logged stop: no problem line
        let t = build_trust(&reg, &items_with("PVTI_a", &line, "left"), 15);
        assert!(t.get("claude@a").is_some());
        assert!(t.problems.is_empty(), "{:?}", t.problems);
        // the marker after the stop ends the span; a reject after it replaces it
        stopped.push(left_marker(8, "claude@a", &k));
        let reg = resolve(&cfg(), &stopped);
        assert_eq!(reg.agents["claude@a"].departure, Some(Departure::Left(8)));
        assert!(build_trust(&reg, &no_items(), 15).get("claude@a").is_none());
        stopped.push(owner(9, "/crew reject claude@a"));
        let reg = resolve(&cfg(), &stopped);
        assert_eq!(
            reg.agents["claude@a"].departure,
            Some(Departure::Rejected(9))
        );
        // a stop before the approval, or of a name without a join, is no departure
        let early = resolve(
            &cfg(),
            &[
                join_comment(1, "claude@a", &k, "PVTI_a"),
                owner(2, "/crew stop claude@a"),
                owner(3, "/crew stop nobody@x"),
                owner(4, "/crew approve claude@a role=dev"),
            ],
        );
        assert_eq!(early.agents["claude@a"].departure, None);
        assert!(!early.agents.contains_key("nobody@x"));
    }

    #[test]
    fn the_signed_stopping_notice_of_the_agent_itself_is_a_departure() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let base = vec![
            join_comment(1, "claude@a", &ka, "PVTI_a"),
            join_comment(2, "hermes@b", &kb, "PVTI_b"),
            owner(3, "/crew approve claude@a role=dev"),
            owner(4, "/crew approve hermes@b role=dev"),
        ];
        let notice = |id: u64, from: &str, named: &str, key: &PrivateKey| {
            signed(
                id,
                "owner-login",
                key,
                from,
                "coordinator",
                Kind::Info,
                &crate::protocol::acts::stopping_body(named, "none"),
            )
        };
        let mut own = base.clone();
        own.push(notice(7, "claude@a", "claude@a", &ka));
        let reg = resolve(&cfg(), &own);
        assert_eq!(
            reg.agents["claude@a"].departure,
            Some(Departure::Stopping(7))
        );
        assert!(
            build_trust(&reg, &no_items(), 15)
                .at("claude@a", 9)
                .is_some()
        );
        // b names a, or signs for a with its own key: neither is a's notice
        for forged in [
            notice(7, "hermes@b", "claude@a", &kb),
            notice(7, "claude@a", "claude@a", &kb),
        ] {
            let mut cs = base.clone();
            cs.push(forged);
            assert_eq!(resolve(&cfg(), &cs).agents["claude@a"].departure, None);
        }
    }

    fn by(mut c: Comment, login: &str) -> Comment {
        c.author = login.into();
        c
    }

    #[test]
    fn a_join_of_any_account_is_read_and_the_approval_binds_its_account() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let reg = resolve(
            &crew_cfg(Some("crew-bot")),
            &[
                by(join_comment(1, "claude@a", &ka, "PVTI_a"), "dev-a"),
                by(join_comment(2, "hermes@b", &kb, "PVTI_b"), "dev-b"),
                owner(3, "/crew approve claude@a role=dev"),
                owner(
                    4,
                    &format!(
                        "/crew approve hermes@b role=dev key={} login=dev-b",
                        fp(&kb)
                    ),
                ),
            ],
        );
        assert!(reg.notes.is_empty(), "{:?}", reg.notes);
        assert_eq!(reg.agents["claude@a"].login(), Some("dev-a"));
        assert_eq!(reg.agents["hermes@b"].login(), Some("dev-b"));
        assert!(reg.agents.values().all(AgentEntry::approved));
        assert!(reg.knows_login("Dev-A") && reg.knows_login("dev-b"));
        assert!(
            !reg.knows_login("crew-bot"),
            "no agent uses the shared account"
        );
        let t = build_trust(&reg, &no_items(), 15);
        assert_eq!(t.at("claude@a", 5).unwrap().login, "dev-a");
        assert!(t.knows_login("dev-b") && !t.knows_login("mallory"));
    }

    #[test]
    fn the_shared_crew_account_still_works_for_every_agent() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let reg = resolve(
            &crew_cfg(Some("crew-bot")),
            &[
                by(join_comment(1, "claude@a", &ka, "PVTI_a"), "crew-bot"),
                by(join_comment(2, "hermes@b", &kb, "PVTI_b"), "crew-bot"),
                owner(3, "/crew approve claude@a role=coordinator"),
                owner(4, "/crew approve hermes@b"),
                by(left_marker(5, "hermes@b", &kb), "crew-bot"),
            ],
        );
        assert!(reg.agents["claude@a"].approved(), "{:?}", reg.notes);
        assert_eq!(reg.agents["hermes@b"].departure, Some(Departure::Left(5)));
        let t = build_trust(&reg, &no_items(), 15);
        assert_eq!(t.at("claude@a", 9).unwrap().login, "crew-bot");
    }

    #[test]
    fn an_approval_with_another_login_than_the_join_is_void() {
        let k = generate_key().unwrap();
        let join = by(join_comment(1, "claude@a", &k, "PVTI_a"), "dev-a");
        let approve = |login: &str| {
            owner(
                2,
                &format!("/crew approve claude@a key={} login={login}", fp(&k)),
            )
        };
        let reg = resolve(
            &crew_cfg(Some("crew-bot")),
            &[join.clone(), approve("dev-b")],
        );
        assert!(!reg.agents["claude@a"].approved());
        assert!(
            reg.notes
                .iter()
                .any(|n| n.contains("login dev-b is not the account of the join (dev-a)")),
            "{:?}",
            reg.notes
        );
        // control: the account of the join approves, in any case
        let reg = resolve(&crew_cfg(Some("crew-bot")), &[join, approve("Dev-A")]);
        assert!(reg.agents["claude@a"].approved(), "{:?}", reg.notes);
    }

    #[test]
    fn a_join_of_the_owner_account_is_refused_on_both_crew_modes_and_approved_in_the_trial() {
        let k = generate_key().unwrap();
        let comments = [
            join_comment(1, "claude@a", &k, "PVTI_a"),
            owner(2, "/crew approve claude@a"),
        ];
        // a shared crew account, and one own account per agent (no crew_login)
        for board in [crew_cfg(Some("crew-bot")), crew_cfg(None)] {
            let reg = resolve(&board, &comments);
            assert!(!reg.agents["claude@a"].approved());
            assert!(
                reg.notes
                    .iter()
                    .any(|n| n.contains("void") && n.contains("owner account")),
                "{:?}",
                reg.notes
            );
        }
        // the one-person trial (insecure_one_account): the agents act as the owner
        let reg = resolve(&cfg(), &comments);
        assert!(reg.agents["claude@a"].approved(), "{:?}", reg.notes);
    }

    #[test]
    fn own_accounts_without_a_crew_login_join_and_are_bound() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let reg = resolve(
            &crew_cfg(None),
            &[
                by(join_comment(1, "claude@a", &ka, "PVTI_a"), "dev-a"),
                by(join_comment(2, "hermes@b", &kb, "PVTI_b"), "dev-b"),
                owner(3, "/crew approve claude@a role=coordinator login=dev-a"),
                owner(4, "/crew approve hermes@b"),
            ],
        );
        assert!(reg.notes.is_empty(), "{:?}", reg.notes);
        assert_eq!(reg.agents["claude@a"].login(), Some("dev-a"));
        assert_eq!(reg.agents["hermes@b"].login(), Some("dev-b"));
        assert!(reg.agents.values().all(AgentEntry::approved));
    }

    #[test]
    fn a_marker_by_another_account_than_the_agents_is_ignored() {
        let (ka, kb) = (generate_key().unwrap(), generate_key().unwrap());
        let base = vec![
            by(join_comment(1, "claude@a", &ka, "PVTI_a"), "dev-a"),
            by(join_comment(2, "hermes@b", &kb, "PVTI_b"), "dev-b"),
            owner(3, "/crew approve claude@a"),
            owner(4, "/crew approve hermes@b"),
        ];
        // a's own marker, posted from the account of b: ends nothing
        let mut cs = base.clone();
        cs.push(by(left_marker(5, "claude@a", &ka), "dev-b"));
        assert_eq!(
            resolve(&crew_cfg(Some("crew-bot")), &cs).agents["claude@a"].departure,
            None
        );
        // control: from a's account it ends a's span
        let mut cs = base;
        cs.push(by(left_marker(5, "claude@a", &ka), "dev-a"));
        assert_eq!(
            resolve(&crew_cfg(Some("crew-bot")), &cs).agents["claude@a"].departure,
            Some(Departure::Left(5))
        );
    }
}
