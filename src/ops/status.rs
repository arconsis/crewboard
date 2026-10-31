//! `status` (basic, spec/cli#commands, spec/board#item-ids-are-the-source-of-truth): agents with
//! state and heartbeat age, items that are known by id but missing from the item listing, foreign
//! comment count, rate limit.

use super::agent_row::{AgentRow, caps_of_row, declared_tags, tier_of};
use super::review_wait::ReviewWait;
use super::{AgentCtx, Ctx, Global, load_world, new_client, resolve_board_config};
use crate::clock::{fmt_age, format_ts, now_epoch, parse_ts};
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::error::CbResult;
use crate::events::{is_foreign, security_scan};
use crate::feed;
use crate::owner_cmd::role_name;
use crate::registry::{CREW_PREFIX, Trust};
use crate::rules::fields::Field;
use crate::rules::status::Status;
use crate::services::ticket_index::{discover_tickets, read_ticket_rows};
use crate::state::StateCache;
use crate::state::TicketIndex;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The liveness word of the agent of a ticket that it can still lose: `late` or `dead`, and only
/// while the ticket is one a derived reassign returns (a legacy `Needs Human` reads as Blocked).
fn agent_problem(word: String, status: Option<&str>) -> Option<String> {
    (word == "late" || word == "dead")
        .then_some(word)
        .filter(|_| status.is_some_and(crate::late::liveness::reassignable_status))
}

/// The warnings about Ready tickets: a paused agent holds Ready tickets that nothing starts, and
/// a Ready ticket has no Agent (an older `reassign` left it so; a ticket in Ready has an agent
/// now). Pure.
pub fn ready_warnings(rows: &[AgentRow], tickets: &[TicketLine]) -> Vec<String> {
    let mut out = Vec::new();
    for r in rows {
        let keys: Vec<String> = tickets
            .iter()
            .filter(|t| t.agent.as_deref() == Some(r.name.as_str()))
            .filter(|t| Status::Ready.is(t.status.as_deref()))
            .map(|t| t.key.clone())
            .collect();
        if r.state.as_deref() == Some("paused") && !keys.is_empty() {
            out.push(crate::late::liveness::paused_ready_text(&r.name, &keys));
        }
    }
    for t in tickets {
        if Status::Ready.is(t.status.as_deref()) && t.agent.is_none() {
            out.push(format!(
                "warning: {} is Ready without an Agent (an older reassign left it so): `crewboard assign {} --to <agent>`",
                t.key, t.key
            ));
        }
    }
    out
}

/// One ticket as the board shows it, with the liveness of its agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketLine {
    pub key: String,
    pub item_id: String,
    pub status: Option<String>,
    pub agent: Option<String>,
    /// The optional `Order` field: the position in the queue of the agent.
    pub order: Option<i64>,
    /// Seconds since the last change of the item on the board.
    pub age: Option<i64>,
    /// `stale` or `dead`: the agent of this ticket does not answer (a stale ticket).
    pub agent_problem: Option<String>,
    /// The PR of the ticket was merged without a passed `verify` of crewboard.
    pub merged_without_verify: bool,
    /// A Review ticket with a signed `done`: since when it waits for `verify`.
    pub review_wait: Option<ReviewWait>,
    /// A Review ticket whose latest `done` passed `verify` on the current PR head and waits for
    /// its merge: the time of that verify (Unix seconds).
    pub verified_waiting_since: Option<i64>,
    /// What the signed log shows, when the board `Status` contradicts it (a Done without proof is
    /// detected here, not prevented). `None`: the board agrees with the log, or the log
    /// says nothing.
    pub log_status: Option<String>,
}

/// The tickets whose board `Status` the signed log does not support, as `(ticket, board status,
/// status of the log)`. The same judgement as the `field-mismatch` of `audit`
/// (`ticket::log::status_contradicts`).
pub fn log_mismatches(tickets: &[TicketLine]) -> Vec<(&str, &str, &str)> {
    tickets
        .iter()
        .filter_map(|t| {
            Some((
                t.key.as_str(),
                t.status.as_deref().unwrap_or("empty"),
                t.log_status.as_deref()?,
            ))
        })
        .collect()
}

/// The keys of the tickets of `agent` that are In Progress, in key order. "Busy" is derived from
/// the board, not from the agent State (spec/board#lifecycle). Pure.
pub fn in_progress_keys(rows: &[crate::ticket::TicketRow], agent: &str) -> Vec<String> {
    let mut keys: Vec<String> = rows
        .iter()
        .filter(|r| r.is_agent(agent) && r.in_status(Status::InProgress))
        .map(|r| r.key())
        .collect();
    keys.sort();
    keys
}

/// The word of the text output: `busy <key>` (`busy <k1>, <k2>` for an old board), else empty.
fn busy_text(r: &AgentRow) -> String {
    if r.busy.is_empty() {
        String::new()
    } else {
        format!("busy {}  ", r.busy.join(", "))
    }
}

/// Agents that hold more than one ticket In Progress (spec/tickets#status-flow-and-writers):
/// `(agent, keys)` in agent order. The rule is one per agent; this state comes from
/// old boards and from a stale State. Pure.
pub fn multiple_in_progress(tickets: &[TicketLine]) -> Vec<(String, Vec<String>)> {
    let mut by: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for t in tickets {
        if Status::InProgress.is(t.status.as_deref())
            && let Some(a) = t.agent.as_deref()
        {
            by.entry(a.to_string()).or_default().push(t.key.clone());
        }
    }
    by.into_iter().filter(|(_, k)| k.len() > 1).collect()
}

/// One foreign comment (spec/messages#untrusted-input): its author is neither the owner nor, for a
/// comment with a crew header, the agent account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignComment {
    /// `owner/repo#n`.
    pub issue: String,
    pub comment_id: u64,
    pub url: String,
    pub author: String,
    pub created_at: String,
    /// Why it is foreign: `crew-header-by-foreign-author` (it wears a crew header),
    /// `headerless-crew-account-comment` (the agent account posts without a header: it is no
    /// owner message and no signed message) or `headerless-foreign-author`.
    pub reason: &'static str,
}

impl ForeignComment {
    pub fn to_json(&self) -> Value {
        json!({"issue": self.issue, "comment_id": self.comment_id, "url": self.url,
               "author": self.author, "created_at": self.created_at, "reason": self.reason})
    }
}

/// The foreign comments among `comments`, in feed order.
pub fn foreign_comments(
    comments: &[Comment],
    cfg: &BoardConfig,
    trust: &Trust,
) -> Vec<ForeignComment> {
    comments
        .iter()
        .filter(|c| is_foreign(c, cfg, trust))
        .map(|c| ForeignComment {
            issue: format!("{}#{}", c.repo, c.issue),
            comment_id: c.id,
            url: c.url.clone(),
            author: c.author.clone(),
            created_at: c.created.clone(),
            reason: if c.body.starts_with(CREW_PREFIX) {
                "crew-header-by-foreign-author"
            } else if trust.knows_login(&c.author) {
                "headerless-crew-account-comment"
            } else {
                "headerless-foreign-author"
            },
        })
        .collect()
}

/// Items named by id (the registry) that the item listing does not return.
pub fn missing_from_listing(known: &[String], listed: &[String]) -> Vec<String> {
    let set: BTreeSet<&String> = listed.iter().collect();
    let mut m: Vec<String> = known.iter().filter(|k| !set.contains(k)).cloned().collect();
    m.sort();
    m.dedup();
    m
}

/// `e2e,x86-builds tier:high`, or `-`.
pub fn tags_text(r: &AgentRow) -> String {
    let mut v = declared_tags(r).join(",");
    if let Some(t) = tier_of(r) {
        if !v.is_empty() {
            v.push(' ');
        }
        v.push_str(&format!("tier:{t}"));
    }
    if v.is_empty() { "-".to_string() } else { v }
}

/// True if the last `start` of this agent failed the toolchain check
/// (spec/agents#toolchain-check-of-start).
pub fn toolchain_unreachable(r: &AgentRow) -> bool {
    caps_of_row(r)
        .facts
        .iter()
        .any(|t| t == super::profile::TOOLCHAIN_FLAG)
}

pub fn heartbeat_age(hb: Option<&str>, now: i64) -> Option<i64> {
    hb.and_then(parse_ts).map(|t| now - t)
}

/// One row per agent of the registry, with its derived liveness (the `agents` of `status --json`).
fn build_rows(ctx: &Ctx, world: &super::World, now: i64) -> Vec<AgentRow> {
    let interval = ctx.hb_interval;
    let mut rows = Vec::new();
    for e in world.reg.agents.values() {
        let item = e.join.as_ref().and_then(|j| world.items.get(&j.item_id));
        let (_, _, live) = crate::ctx::agent_liveness(world, &e.name, now, interval);
        let hb = item
            .and_then(|i| i.get(Field::Heartbeat))
            .map(str::to_string);
        rows.push(AgentRow {
            name: e.name.clone(),
            approved: e.approved(),
            rejected: e.rejected,
            trusted: world.trust.get(&e.name).is_some(),
            role: e.approval.as_ref().map(|a| role_name(a.role).to_string()),
            state: item.and_then(|i| i.get(Field::State)).map(str::to_string),
            heartbeat_age: heartbeat_age(hb.as_deref(), now),
            heartbeat: hb,
            runtime: item.and_then(|i| i.get(Field::Runtime)).map(str::to_string),
            host: item.and_then(|i| i.get(Field::Host)).map(str::to_string),
            item_id: e.join.as_ref().map(|j| j.item_id.clone()),
            login: e.login().map(str::to_string),
            capabilities: item
                .and_then(|i| i.get(super::profile::FIELD))
                .map(str::to_string),
            liveness: if e.approved() {
                live.word().to_string()
            } else {
                "-".to_string()
            },
            busy: Vec::new(),
        });
    }
    rows
}

/// The context, the cache and the extra item ids of the board that `status` reads. A joined
/// agent gives its own config (and, with `with_diff`, the diff of its copy of the board config
/// against the default branch: one read); without an agent config the owner's resolution is used.
fn open_board(g: &Global, with_diff: bool) -> CbResult<Opened> {
    match AgentCtx::load(g) {
        Ok(ac) => {
            let cache = ac.load_cache();
            let extra = vec![ac.agent.item_id.clone()];
            let board_diff = with_diff.then(|| super::board_refresh::differs(&ac));
            Ok(Opened {
                ctx: ac.ctx,
                cache,
                extra,
                board_diff,
            })
        }
        Err(_) => {
            let cfg = resolve_board_config(g)?;
            let ctx = Ctx::load(new_client()?, cfg, g.dir()?)?;
            Ok(Opened {
                ctx,
                cache: StateCache::default(),
                extra: vec![],
                board_diff: None,
            })
        }
    }
}

struct Opened {
    ctx: Ctx,
    cache: StateCache,
    extra: Vec<String>,
    /// For a joined agent: the copy of the board config against the default branch.
    board_diff: Option<Result<Option<String>, String>>,
}

/// The agent rows of the board and nothing else (three reads of GitHub): what `status --json`
/// prints under `agents`. The board check of `crewboard lock` asks for it in process.
pub fn agent_rows(g: &Global) -> CbResult<Vec<AgentRow>> {
    let Opened {
        ctx,
        mut cache,
        extra,
        ..
    } = open_board(g, false)?;
    let world = load_world(&ctx, &extra, &mut cache)?;
    Ok(build_rows(&ctx, &world, now_epoch()))
}

/// Everything `status` reads from the board and the feed. `status --queue` stops here.
struct Snapshot {
    ctx: Ctx,
    world: super::World,
    board_diff: Option<Result<Option<String>, String>>,
    now: i64,
    rows: Vec<AgentRow>,
    trows: Vec<crate::ticket::TicketRow>,
    tickets: Vec<TicketLine>,
    /// Items that the board lists.
    listed: usize,
    /// Items known by id but absent from the (eventually consistent) listing.
    missing: Vec<String>,
    /// The whole feed of every repo.
    comments: Vec<Comment>,
    late: LateView,
    foreign_list: Vec<ForeignComment>,
    security_list: Vec<crate::events::SecurityEvent>,
}

/// The late waits of every ticket and agent, with the GitHub reads they cost.
struct LateView {
    waits: Vec<crate::late::Late>,
    expect: crate::late::Expect,
    reads: u32,
    coordinator_late: bool,
}

/// What `status` reads in addition for the full output: the PR column, the option count and
/// the rate limits.
struct Extras {
    pr_entries: Vec<(String, String, Option<super::pr_state::PrView>)>,
    many: Vec<(String, Vec<String>)>,
    agent_options: usize,
    option_warning: Option<String>,
    core: crate::github::RateLimit,
    gql: crate::github::RateLimit,
}

/// The tickets: the local index plus the open `crew:work` issues, each read by id. Sets `busy`
/// on the agent rows and returns the ticket rows with one board line per ticket.
fn read_tickets(
    ctx: &Ctx,
    world: &super::World,
    cache: &StateCache,
    rows: &mut [AgentRow],
    now: i64,
) -> CbResult<(Vec<crate::ticket::TicketRow>, Vec<TicketLine>)> {
    let mut tmp = StateCache {
        index: TicketIndex {
            tickets: cache.index.tickets.clone(),
            ..TicketIndex::default()
        },
        ..StateCache::default()
    };
    discover_tickets(&ctx.reader(), &world.trust, &mut tmp.index)?;
    let trows = read_ticket_rows(&ctx.reader(), &mut tmp.index.tickets)?;
    for r in rows.iter_mut() {
        r.busy = in_progress_keys(&trows, &r.name);
    }
    let word_of = |name: &str| {
        rows.iter()
            .find(|r| r.name == name)
            .map(|r| r.liveness.clone())
    };
    let mut tickets: Vec<TicketLine> = trows
        .iter()
        .map(|t| TicketLine {
            key: t.key(),
            item_id: t.item_id.clone(),
            status: t.status_str().map(str::to_string),
            agent: t.agent.clone(),
            order: t.order,
            age: t.updated.map(|u| now - u),
            agent_problem: t
                .agent
                .as_deref()
                .and_then(word_of)
                .and_then(|w| agent_problem(w, t.status_str())),
            merged_without_verify: false,
            review_wait: None,
            verified_waiting_since: None,
            log_status: None,
        })
        .collect();
    tickets.sort_by(|a, b| a.key.cmp(&b.key));
    Ok((trows, tickets))
}

/// Per ticket: the board `Status` against the signed log, a PR merged without a passed verify,
/// and how long a Review ticket has waited for verify. The comments are read already; a note
/// on the ticket needs no read, a Review ticket costs one PR GET.
fn mark_ticket_proof(
    ctx: &Ctx,
    world: &super::World,
    tickets: &mut [TicketLine],
    comments: &[Comment],
    now: i64,
) {
    for t in tickets {
        let Some((repo, n)) = t.key.rsplit_once('#') else {
            continue;
        };
        let own: Vec<Comment> = comments
            .iter()
            .filter(|c| c.repo == repo && n.parse::<u64>().ok() == Some(c.issue))
            .cloned()
            .collect();
        let msgs = crate::ticket::verified_messages(&own, &ctx.cfg, &world.trust);
        let log = crate::ticket::log::fold_ticket(&msgs);
        if crate::ticket::log::status_contradicts(&log, t.status.as_deref()) {
            t.log_status = Some(log.status.describe());
        }
        t.merged_without_verify = super::tickets::merged_unverified::status_marked(
            ctx,
            t.status.as_deref(),
            t.agent.as_deref(),
            &msgs,
        );
        // How long a Review ticket has waited for verify: from the latest `done` of its agent.
        if Status::Review.is(t.status.as_deref())
            && let Some(agent) = t.agent.as_deref()
            && let Some((m, _)) = crate::ticket::latest_done(&msgs, agent)
        {
            t.review_wait = Some(ReviewWait::new(m.created, now));
        }
    }
}

/// Reads the board, the tickets and the feed.
fn read_snapshot(g: &Global) -> CbResult<Snapshot> {
    let Opened {
        ctx,
        mut cache,
        extra,
        board_diff,
    } = open_board(g, true)?;
    let world = load_world(&ctx, &extra, &mut cache)?;
    let now = now_epoch();
    let mut rows = build_rows(&ctx, &world, now);
    let (trows, mut tickets) = read_tickets(&ctx, &world, &cache, &mut rows, now)?;
    let known: Vec<String> = rows.iter().filter_map(|r| r.item_id.clone()).collect();
    let listed = ctx.board.list_item_ids(&ctx.client)?;
    let missing = missing_from_listing(&known, &listed);
    let mut comments: Vec<Comment> = Vec::new();
    for repo in &ctx.cfg.repos {
        comments
            .extend(feed::feed_since(&ctx.client, repo, 0, None, &ctx.cfg.owner_login)?.comments);
    }
    // Late work (spec/events#stale-work): the late waits of every ticket, with the GitHub reads
    // they cost.
    let found = super::late::status_late(&ctx, &world, &trows, &comments, now);
    for t in &mut tickets {
        if let Some((_, since)) = found.verified.iter().find(|(id, _)| *id == t.item_id) {
            t.verified_waiting_since = Some(*since);
        }
    }
    let late = LateView {
        coordinator_late: crate::late::coordinator_is_late(&found.waits),
        waits: found.waits,
        expect: found.expect,
        reads: found.reads,
    };
    mark_ticket_proof(&ctx, &world, &mut tickets, &comments, now);
    let foreign_list = foreign_comments(&comments, &ctx.cfg, &world.trust);
    // Foreign comments and security events come from the whole feed.
    let security_list = security_scan(&comments, &ctx.cfg, &world.trust);
    Ok(Snapshot {
        ctx,
        world,
        board_diff,
        now,
        rows,
        trows,
        tickets,
        listed: listed.len(),
        missing,
        comments,
        late,
        foreign_list,
        security_list,
    })
}

/// The reads that only the full output needs.
fn read_extras(snap: &Snapshot) -> CbResult<Extras> {
    let ctx = &snap.ctx;
    // The PR column: the `done` messages come from the feed read above.
    let mut pr_entries: Vec<(String, String, Option<super::pr_state::PrView>)> = Vec::new();
    for t in &snap.trows {
        let (Some(st), Some(agent)) = (t.status_str(), t.agent.as_deref()) else {
            continue;
        };
        if !super::pr_state::wants_pr(Some(st)) {
            continue;
        }
        let view = super::pr_state::done_pr(
            &snap.comments,
            &ctx.cfg,
            &snap.world.trust,
            &t.repo,
            t.issue,
            agent,
        )
        .map(|(repo, n)| super::pr_state::read_pr(&ctx.client, &repo, n));
        pr_entries.push((t.key(), st.to_string(), view));
    }
    let many = multiple_in_progress(&snap.tickets);
    // spec/board#option-limit: the options came with the board load, no extra read
    let agent_options = ctx
        .board
        .has_field(Field::Agent)
        .map_or(0, |f| f.options.len());
    let option_warning = crate::rules::options::agent_options_warning(agent_options);
    let core = ctx.client.rate_limit_of("core")?;
    let gql = ctx.client.rate_limit_of("graphql")?;
    Ok(Extras {
        pr_entries,
        many,
        agent_options,
        option_warning,
        core,
        gql,
    })
}

pub fn run(g: &Global, json_out: bool, verbose: bool, queue: bool) -> CbResult<String> {
    let snap = read_snapshot(g)?;
    if queue && !json_out {
        return Ok(queue_text(&snap));
    }
    let extras = read_extras(&snap)?;
    Ok(if json_out {
        render_json(&snap, &extras, queue)
    } else {
        render_text(&snap, &extras, verbose)
    })
}

fn heading(ctx: &Ctx) -> String {
    format!(
        "initiative {} - project {} of {}\n\n",
        ctx.cfg.initiative, ctx.cfg.project_number, ctx.cfg.project_owner
    )
}

/// `--queue`: per agent the In Progress ticket and the Ready queue in order.
fn queue_text(snap: &Snapshot) -> String {
    let queues = super::queue::build(&snap.rows, &snap.trows);
    let unassigned = super::queue::unassigned_ready(&snap.trows);
    let imbalance = super::queue::imbalance_note(&queues);
    format!(
        "{}{}",
        heading(&snap.ctx),
        super::queue::render(
            &queues,
            &unassigned,
            imbalance.as_deref(),
            snap.ctx.board.has_field(Field::Order).is_some()
        )
    )
}

fn agents_json(rows: &[AgentRow]) -> Vec<Value> {
    rows.iter()
        .map(|r| {
            json!({"name": r.name, "approved": r.approved, "rejected": r.rejected, "trusted": r.trusted, "role": r.role,
                   "state": r.state, "heartbeat": r.heartbeat, "heartbeat_age_s": r.heartbeat_age,
                   "liveness": r.liveness, "in_progress": r.busy,
                   "runtime": r.runtime, "host": r.host, "item_id": r.item_id, "login": r.login,
                   "tags": declared_tags(r), "tier": tier_of(r), "capabilities": r.capabilities,
                   "toolchain_unreachable": toolchain_unreachable(r)})
        })
        .collect()
}

fn tickets_json(tickets: &[TicketLine], verify_limit: i64) -> Vec<Value> {
    tickets
        .iter()
        .map(|t| {
            json!({"ticket": t.key, "item_id": t.item_id, "status": t.status,
        "agent": t.agent, "order": t.order, "age_s": t.age, "agent_problem": t.agent_problem,
        "merged_without_verify": t.merged_without_verify, "log_status": t.log_status,
        "review_wait_s": t.review_wait.map(|w| w.waited),
        "review_done_at": t.review_wait.map(|w| format_ts(w.since)),
        "verified_waiting_since": t.verified_waiting_since.map(format_ts),
        "waiting_for_verify": t.verified_waiting_since.is_none()
            && t.review_wait.is_some_and(|w| w.is_over(verify_limit))})
        })
        .collect()
}

fn security_json(list: &[crate::events::SecurityEvent]) -> Vec<Value> {
    list.iter()
        .map(|s| {
            json!({
        "kind": s.reason.as_str(), "url": s.url, "claimed_from": s.claimed_from,
        "issue": format!("{}#{}", s.repo, s.issue), "comment_id": s.comment_id,
        "author": s.author, "created_at": s.created_at, "detail": s.detail})
        })
        .collect()
}

fn render_json(snap: &Snapshot, ex: &Extras, queue: bool) -> String {
    let Snapshot {
        ctx,
        world,
        rows,
        tickets,
        late,
        now,
        ..
    } = snap;
    let queue_json = queue.then(|| {
        let queues = super::queue::build(rows, &snap.trows);
        let unassigned = super::queue::unassigned_ready(&snap.trows);
        let imbalance = super::queue::imbalance_note(&queues);
        let mut q = super::queue::to_json(&queues, &unassigned, imbalance.as_deref());
        q["board_has_order_field"] = json!(ctx.board.has_field(Field::Order).is_some());
        q
    });
    let verify_limit = late.expect.verify;
    let mut out = json!({
        "initiative": ctx.cfg.initiative,
        "project": ctx.cfg.project_number,
        "board_config_differs": snap.board_diff.as_ref().and_then(|d| d.as_ref().ok()).map(Option::is_some),
        "heartbeat_interval_s": ctx.hb_interval,
        "agents": agents_json(rows),
        "tickets": tickets_json(tickets, verify_limit),
        "status_log_mismatches": log_mismatches(tickets).iter().map(|(k, b, l)| json!({
            "ticket": k, "board_status": b, "log_status": l})).collect::<Vec<_>>(),
        "late": late.waits.iter().map(|l| l.to_json(&late.expect, late.coordinator_late, *now)).collect::<Vec<_>>(),
        "late_github_reads": late.reads,
        "ready_warnings": ready_warnings(rows, tickets),
        "multiple_in_progress": ex.many.iter().map(|(a, k)| json!({"agent": a, "tickets": k})).collect::<Vec<_>>(),
        "pull_requests": ex.pr_entries.iter().filter_map(|(k, s, v)| v.as_ref().map(|v| super::pr_state::to_json(k, s, v))).collect::<Vec<_>>(),
        "items_known_by_id_missing_from_listing": snap.missing,
        "items_listed": snap.listed,
        "agent_options": {"count": ex.agent_options, "limit": crate::rules::options::OPTION_LIMIT,
                          "warning": ex.option_warning},
        "unreadable_item_ids": world.unreadable,
        "foreign_comments": snap.foreign_list.len(),
        "security_event_count": snap.security_list.len(),
        "security_event_list": security_json(&snap.security_list),
        "foreign_comment_list": snap.foreign_list.iter().map(ForeignComment::to_json).collect::<Vec<_>>(),
        "comments_scanned": snap.comments.len(),
        "trust_problems": world.trust.problems,
        "registry_notes": world.reg.notes,
        "rate_limit": {"core": {"remaining": ex.core.remaining, "limit": ex.core.limit},
                       "graphql": {"remaining": ex.gql.remaining, "limit": ex.gql.limit}},
    });
    if let Some(q) = queue_json {
        out["queue"] = q;
    }
    out.to_string() + "\n"
}

/// The first lines of the text output: the board, then the identity and config notes.
fn header_text(snap: &Snapshot) -> String {
    let ctx = &snap.ctx;
    let mut s = format!(
        "initiative {} - project {} of {}\n\nagents ({}):\n",
        ctx.cfg.initiative,
        ctx.cfg.project_number,
        ctx.cfg.project_owner,
        snap.rows.len()
    );
    if ctx.cfg.insecure_one_account {
        s.insert_str(
            s.find('\n').map_or(0, |i| i + 1),
            "identity: ONE ACCOUNT (insecure)\n",
        );
    }
    match &snap.board_diff {
        Some(Ok(Some(line))) => {
            s.insert_str(s.find('\n').map_or(0, |i| i + 1), &format!("{line}\n"))
        }
        Some(Err(note)) => s.insert_str(s.find('\n').map_or(0, |i| i + 1), note),
        _ => {}
    }
    s
}

fn agents_text(snap: &Snapshot, s: &mut String) {
    let interval = snap.ctx.hb_interval;
    for r in &snap.rows {
        s.push_str(&format!(
            "  {:<22} {:<9} role={:<11} state={:<9} {}liveness={:<6} heartbeat={}{}  tags={}  login={}\n",
            r.name,
            trust_word(r),
            r.role.as_deref().unwrap_or("-"),
            r.state.as_deref().unwrap_or("?"),
            busy_text(r),
            r.liveness,
            r.heartbeat.as_deref().unwrap_or("none"),
            r.heartbeat_age
                .map(|a| format!(" ({a}s ago)"))
                .unwrap_or_default(),
            tags_text(r),
            r.login.as_deref().unwrap_or("-")
        ));
        if toolchain_unreachable(r) {
            s.push_str("    note: toolchain unreachable at the last `start` (the agent cannot run the build tools of the repo); do not assign build tickets to it\n");
        }
    }
    s.push_str(&format!(
        "\n(heartbeat interval {interval} s: late after {} s, dead after {} s)\n",
        crate::late::liveness::LATE_INTERVALS * interval,
        crate::late::liveness::DEAD_INTERVALS * interval
    ));
}

/// The text after a Review ticket: a verified one waits for its merge, the others for `verify`.
fn review_text(t: &TicketLine, verify_limit: i64, now: i64) -> String {
    if let Some(since) = t.verified_waiting_since {
        return format!(
            "  verified, waiting for merge since {} ({})",
            format_ts(since),
            fmt_age((now - since).max(0))
        );
    }
    t.review_wait
        .map(|w| w.text(verify_limit))
        .unwrap_or_default()
}

fn tickets_text(snap: &Snapshot, ex: &Extras, s: &mut String) {
    let verify_limit = snap.late.expect.verify;
    s.push_str(&format!("\ntickets ({}):\n", snap.tickets.len()));
    for t in &snap.tickets {
        s.push_str(&format!(
            "  {:<34} {:<12} {:<22} {:<3} age {:<5}{}\n",
            t.key,
            t.status.as_deref().unwrap_or("?"),
            t.agent.as_deref().unwrap_or("-"),
            t.order.map_or("-".to_string(), |o| o.to_string()),
            t.age.map_or("?".to_string(), fmt_age),
            t.agent_problem
                .as_ref()
                .map(|p| format!("  [agent is {p}]"))
                .unwrap_or_default()
                + &review_text(t, verify_limit, snap.now)
        ));
        if t.merged_without_verify {
            s.push_str(&format!(
                "    ! {}\n",
                super::tickets::merged_unverified::MARKER
            ));
        }
    }
    // The board Status against the signed log. Done without proof is detected here, not
    // prevented: a holder of the agent token can write the field.
    let mismatches = log_mismatches(&snap.tickets);
    s.push_str(&format!(
        "board against signed log: {} mismatch(es) (a Status that no signed message supports; a Done without proof is detected, not prevented)\n",
        mismatches.len()
    ));
    for (key, board, log) in &mismatches {
        s.push_str(&format!(
            "  ! {key}: board Status is {board}, the signed log shows {log}\n"
        ));
    }
    for w in ready_warnings(&snap.rows, &snap.tickets) {
        s.push_str(&format!("! {w}\n"));
    }
    for (agent, keys) in &ex.many {
        s.push_str(&format!(
            "! {agent} has {} tickets In Progress: {} (one per agent: park or finish all but one)\n",
            keys.len(),
            keys.join(", ")
        ));
    }
}

fn late_text(snap: &Snapshot, s: &mut String) {
    let late = &snap.late;
    s.push_str(&format!(
        "late: {} ({} GitHub read(s) for them)\n",
        late.waits.len(),
        late.reads
    ));
    for l in &late.waits {
        s.push_str(&l.status_line(&late.expect, late.coordinator_late, snap.now));
    }
}

/// The findings of the feed and the board: missing items, foreign comments, security events.
fn findings_text(snap: &Snapshot, verbose: bool, s: &mut String) {
    let missing = &snap.missing;
    s.push_str(&format!(
        "\nitems known by id but missing from the item listing: {}{}\n(listing returned {} items; the listing is eventually consistent)\n",
        missing.len(),
        if missing.is_empty() { String::new() } else { format!(" [{}]", missing.join(", ")) },
        snap.listed
    ));
    // A command that stopped half way (reassign, assign) can leave a Backlog ticket that still
    // names an agent. No agent works on it; `assign` overwrites the Agent.
    let half: Vec<String> = snap
        .trows
        .iter()
        .filter(|r| r.in_status(Status::Backlog))
        .filter_map(|r| r.agent.as_ref().map(|a| format!("{} ({a})", r.key())))
        .collect();
    if !half.is_empty() {
        s.push_str(&format!(
            "tickets in Backlog that still name an agent (a command stopped half way): {}\n  fix: crewboard assign <ticket> --to <agent>; the coordinator also clears it when the agent is dead or stopped\n",
            half.join(", ")
        ));
    }
    if !snap.world.unreadable.is_empty() {
        s.push_str(&format!(
            "item ids that cannot be read at all: {}\n",
            snap.world.unreadable.join(", ")
        ));
    }
    let foreign = snap.foreign_list.len();
    s.push_str(&format!(
        "foreign comments: {foreign} (of {} comments scanned)\n",
        snap.comments.len()
    ));
    s.push_str(&format!(
        "security events: {} (comments that claim to be crew messages but are not valid; selftest forgeries excluded)\n",
        snap.security_list.len()
    ));
    for x in &snap.security_list {
        s.push_str(&format!(
            "  {} {}#{} comment {} by {} claiming from={} at {}\n    {}\n",
            x.reason.as_str(),
            x.repo,
            x.issue,
            x.comment_id,
            x.author,
            x.claimed_from.as_deref().unwrap_or("(unreadable)"),
            x.created_at,
            x.url
        ));
    }
    if verbose {
        for f in &snap.foreign_list {
            s.push_str(&format!(
                "  {} comment {} by {} at {}: {}\n    {}\n",
                f.issue, f.comment_id, f.author, f.created_at, f.reason, f.url
            ));
        }
    } else if foreign > 0 {
        s.push_str("  (run `crewboard status --verbose` to list them)\n");
    }
}

fn render_text(snap: &Snapshot, ex: &Extras, verbose: bool) -> String {
    let mut s = header_text(snap);
    agents_text(snap, &mut s);
    if let Some(w) = &ex.option_warning {
        s.push_str(&format!("! {w}\n"));
    }
    tickets_text(snap, ex, &mut s);
    late_text(snap, &mut s);
    s.push_str(&super::pr_state::render_block(&ex.pr_entries));
    findings_text(snap, verbose, &mut s);
    for p in &snap.world.trust.problems {
        s.push_str(&format!("trust problem: {p}\n"));
    }
    for n in &snap.world.reg.notes {
        s.push_str(&format!("registry note: {n}\n"));
    }
    s.push_str(&format!(
        "rate limit: REST {}/{} left, GraphQL {}/{} left\n",
        ex.core.remaining, ex.core.limit, ex.gql.remaining, ex.gql.limit
    ));
    s
}

/// The trust column of the text output. `/crew reject` marks the agent rejected in the
/// registry and leaves the item State as it is, so the State column alone cannot show it.
fn trust_word(r: &AgentRow) -> &'static str {
    if r.rejected {
        "REJECTED"
    } else if r.trusted {
        "trusted"
    } else if r.approved {
        "UNTRUSTED"
    } else {
        "pending"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_or_dead_agent_marks_the_tickets_a_reassign_returns_also_for_the_legacy_status() {
        for status in ["Ready", "In Progress", "Blocked", "Needs Human"] {
            for word in ["late", "dead"] {
                assert_eq!(
                    agent_problem(word.into(), Some(status)).as_deref(),
                    Some(word),
                    "{status} {word}"
                );
            }
        }
        assert_eq!(agent_problem("alive".into(), Some("Blocked")), None);
        for status in ["Review", "Done", "Backlog"] {
            assert_eq!(agent_problem("dead".into(), Some(status)), None, "{status}");
        }
        assert_eq!(agent_problem("dead".into(), None), None);
    }

    #[test]
    fn missing_from_listing_finds_the_absent_ids_and_only_those() {
        let known = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let listed = vec!["a".to_string(), "c".to_string(), "zzz".to_string()];
        assert_eq!(missing_from_listing(&known, &listed), vec!["b".to_string()]);
        // control: everything listed means nothing missing
        assert!(missing_from_listing(&known, &known).is_empty());
        // an empty listing misses everything (the check can fire)
        assert_eq!(missing_from_listing(&known, &[]).len(), 3);
    }

    fn row(cap: Option<&str>) -> AgentRow {
        AgentRow {
            name: "a@x".into(),
            approved: true,
            rejected: false,
            trusted: true,
            role: Some("dev".into()),
            state: Some("active".into()),
            heartbeat: None,
            heartbeat_age: None,
            runtime: None,
            host: None,
            item_id: None,
            login: None,
            capabilities: cap.map(str::to_string),
            liveness: "alive".into(),
            busy: Vec::new(),
        }
    }

    fn line(key: &str, status: &str, agent: Option<&str>) -> TicketLine {
        TicketLine {
            key: key.into(),
            item_id: format!("PVTI_{key}"),
            status: Some(status.into()),
            agent: agent.map(str::to_string),
            order: None,
            age: None,
            agent_problem: None,
            merged_without_verify: false,
            review_wait: None,
            verified_waiting_since: None,
            log_status: None,
        }
    }

    #[test]
    fn status_warns_of_ready_tickets_of_a_paused_agent_and_of_a_ready_ticket_without_agent() {
        let mut paused = row(None);
        paused.state = Some("paused".into());
        let tickets = [
            line("o/r#1", "Ready", Some("a@x")),
            line("o/r#2", "In Progress", Some("a@x")),
            line("o/r#3", "Ready", None),
            line("o/r#4", "Backlog", None),
        ];
        let w = ready_warnings(&[paused], &tickets);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("a@x is paused and holds 1 Ready ticket(s): o/r#1"));
        assert!(w[1].contains("o/r#3 is Ready without an Agent"), "{}", w[1]);
        // control: an active agent and a Ready ticket with an agent give no warning
        assert!(ready_warnings(&[row(None)], &tickets[..2]).is_empty());
    }

    #[test]
    fn status_shows_the_declared_tags_and_tier_but_not_the_facts() {
        let r = row(Some(
            "x86-builds e2e tier:high arch:x86_64 os:macos-15 tool:cargo",
        ));
        assert_eq!(declared_tags(&r), ["e2e", "x86-builds"]);
        assert_eq!(tier_of(&r).as_deref(), Some("high"));
        assert_eq!(tags_text(&r), "e2e,x86-builds tier:high");
        // control: an agent with only facts, or no field at all, shows `-`
        assert_eq!(tags_text(&row(Some("arch:arm64 os:macos-15"))), "-");
        assert_eq!(tags_text(&row(None)), "-");
        assert_eq!(tags_text(&row(Some("tier:low"))), "tier:low");
    }

    #[test]
    fn busy_is_derived_from_the_in_progress_tickets_of_the_agent() {
        let t = |n: u64, status: &str, agent: &str| crate::ticket::TicketRow {
            repo: "a/b".into(),
            issue: n,
            item_id: format!("PVTI_{n}"),
            status: Some(status.into()),
            agent: Some(agent.into()),
            order: None,
            updated: None,
        };
        let rows = vec![
            t(1, "In Progress", "a@x"),
            t(2, "Ready", "a@x"),
            t(3, "In Progress", "b@x"),
            t(4, "Blocked", "a@x"),
        ];
        assert_eq!(in_progress_keys(&rows, "a@x"), vec!["a/b#1".to_string()]);
        // control: an agent without an In Progress ticket is not busy
        assert!(in_progress_keys(&rows, "c@x").is_empty());
        let busy = AgentRow {
            busy: in_progress_keys(&rows, "a@x"),
            ..row(None)
        };
        assert_eq!(busy_text(&busy), "busy a/b#1  ");
        assert_eq!(busy_text(&row(None)), "");
    }

    #[test]
    fn status_shows_a_rejected_agent_as_rejected() {
        // `/crew reject` marks the agent rejected in the registry; the item State stays.
        let rejected = AgentRow {
            approved: false,
            rejected: true,
            trusted: false,
            ..row(None)
        };
        assert_eq!(trust_word(&rejected), "REJECTED");
        // control: a pending join, an approved agent and an untrusted approved agent
        let pending = AgentRow {
            approved: false,
            trusted: false,
            ..row(None)
        };
        assert_eq!(trust_word(&pending), "pending");
        assert_eq!(trust_word(&row(None)), "trusted");
        let untrusted = AgentRow {
            trusted: false,
            ..row(None)
        };
        assert_eq!(trust_word(&untrusted), "UNTRUSTED");
    }

    #[test]
    fn status_shows_the_toolchain_note_only_for_the_flagged_agent() {
        assert!(toolchain_unreachable(&row(Some(
            "arch:arm64 toolchain:unreachable"
        ))));
        assert!(!toolchain_unreachable(&row(Some("arch:arm64 tool:cargo"))));
        assert!(!toolchain_unreachable(&row(None)));
    }

    #[test]
    fn heartbeat_age_is_now_minus_heartbeat() {
        let t = parse_ts("2026-10-15T14:05:00Z").unwrap();
        assert_eq!(
            heartbeat_age(Some("2026-10-15T14:05:00Z"), t + 90),
            Some(90)
        );
        assert_eq!(heartbeat_age(Some("garbage"), t), None);
        assert_eq!(heartbeat_age(None, t), None);
    }

    #[test]
    fn foreign_comments_are_listed_with_every_field_and_a_reason() {
        use crate::comments::tests::comment;
        use crate::registry::tests::{join_comment, owner};
        let cfg = crate::registry::tests::crew_cfg(Some("crew-bot"));
        // an approved agent that acts as crew-bot: its account is known
        let k = crate::protocol::signature::generate_key().unwrap();
        let mut join = join_comment(1, "x@y", &k, "PVTI_x");
        join.author = "crew-bot".into();
        let reg = crate::registry::resolve(&cfg, &[join, owner(2, "/crew approve x@y")]);
        let trust = crate::registry::build_trust(&reg, &Default::default(), 15);
        let crew_header =
            format!("{CREW_PREFIX}v=1 from=x@y to=all kind=info re=none ts=t sig=AA -->\nx");
        let comments = vec![
            comment(1, "owner-login", "owner comment"),
            comment(2, "mallory", "hello"),
            comment(3, "crew-bot", "headerless by the agent account"),
            comment(4, "mallory", &crew_header),
            comment(5, "crew-bot", &crew_header),
        ];
        let list = foreign_comments(&comments, &cfg, &trust);
        let ids: Vec<u64> = list.iter().map(|f| f.comment_id).collect();
        assert_eq!(ids, [2, 3, 4]);
        // control: without the approval, crew-bot is no agent account
        let none = Trust::default();
        let ids: Vec<u64> = foreign_comments(&comments, &cfg, &none)
            .iter()
            .map(|f| f.comment_id)
            .collect();
        assert_eq!(ids, [2, 3, 4, 5]);
        let reasons: Vec<&str> = list.iter().map(|f| f.reason).collect();
        assert_eq!(
            reasons,
            [
                "headerless-foreign-author",
                "headerless-crew-account-comment",
                "crew-header-by-foreign-author"
            ]
        );
        let j = list[0].to_json();
        assert_eq!(j["issue"], "example-org/app#2");
        assert_eq!(j["comment_id"], 2);
        assert_eq!(j["author"], "mallory");
        assert_eq!(j["created_at"], "2026-10-15T14:00:00Z");
        assert_eq!(j["reason"], "headerless-foreign-author");
        assert!(j["url"].as_str().unwrap().ends_with("#issuecomment-2"));
    }

    #[test]
    fn no_foreign_comment_gives_an_empty_list() {
        use crate::comments::tests::comment;
        let cfg = crate::registry::tests::cfg();
        let comments = vec![
            comment(1, "owner-login", "owner"),
            comment(2, "owner-login", "x"),
        ];
        assert!(foreign_comments(&comments, &cfg, &Trust::default()).is_empty());
        assert!(foreign_comments(&[], &cfg, &Trust::default()).is_empty());
    }

    #[test]
    fn ages_are_short_and_never_negative() {
        assert_eq!(fmt_age(0), "0s");
        assert_eq!(fmt_age(119), "119s");
        assert_eq!(fmt_age(120), "2m");
        assert_eq!(fmt_age(7199), "119m");
        assert_eq!(fmt_age(7200), "2h");
        assert_eq!(fmt_age(172_800), "2d");
        // the board clock may run a little ahead of this host
        assert_eq!(fmt_age(-5), "0s");
    }
}
