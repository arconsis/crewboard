//! What an agent does in every `watch`/`poll` cycle besides reading events (spec/board#fields,
//! spec/board#lifecycle, spec/messages#signature).
//!
//! Everything is derived from the board, never from a local memory, so a restart repeats
//! nothing and a lost cache loses nothing: a heartbeat is on the board or it is not, a report
//! is in the registry issue or it is not.
//!
//! - **every agent**: applies the owner's `/crew pause|resume|stop` commands that name it; leaves
//!   when it is draining and holds no open ticket. It does not answer a `ping` (the kind is
//!   retired as a liveness probe; an old coordinator may still send one).
//! - **a running dev agent**: judges the coordinator by its heartbeat. If the coordinator is
//!   late, one signed `kind=info` to the owner says so (`ops::late::late_owner_duty`).
//! - **the coordinator**: judges every other agent by the age of its heartbeat (alive, late,
//!   dead). It sends nothing to a late agent. A dead agent's open tickets go back to `Backlog` (no agent, so `assign` again)
//!   (reassign with a derived justification), and a signed report and a signed `kind=info` to
//!   the owner (the late path) say so. The tickets of an agent that the registry log stops or
//!   ends (its signed stopping notice or `lifecycle: left` marker, the owner's `/crew stop` or
//!   `/crew reject`) go back to `Backlog` (no agent, so `assign` again) the same way. A board
//!   `State` of `draining` or `left` without such a log entry returns nothing: the owner hears of
//!   it once.
//!   Security events (bad signature, edited message, unknown sender, foreign author, and a
//!   deleted note of the coordinator, `own_notes`) are reported to the owner as a signed
//!   `kind=info`, once per event. Without a coordinator on the board, the agent that sees the
//!   event reports it.

use super::late::{Note, agent_waits, late_coordinator, late_owner_duty};
use super::lifecycle::{apply_owner_commands, finish_draining};
use super::registry_post::{liveness_event, post_registry, report_security, state_event_key};
use super::tickets::{reassign_derived, tag_earlier_work};
use super::{AgentCtx, World};
use crate::audit::reported_events;
use crate::ctx::{agent_liveness, registry_msgs};
use crate::error::CbResult;
use crate::events::{Event, SecurityEvent};
use crate::late::liveness::{
    DEAD_INTERVALS, LATE_INTERVALS, Liveness, derived_reassign_reason, heartbeat_state,
    reassignable_parsed,
};
use crate::late::{AgentLate, agent_late};
use crate::protocol::names::{Kind, Re, To};
use crate::registry::Departure;
use crate::rules::Role;
use crate::rules::lifecycle::State;
use crate::rules::status::Status;
use crate::state::{StateCache, TicketSnap};
use crate::ticket::{TicketRow, VMsg};
use std::collections::HashSet;

/// What the duties of one cycle produced for the output and the ticket snapshot.
#[derive(Debug, Default)]
pub struct Duty {
    pub events: Vec<Event>,
    /// Tickets that this cycle changed itself: item id -> what they look like now. The cycle
    /// stores them, so the next cycle does not report its own change as an event.
    pub snaps: Vec<(String, TicketSnap)>,
    /// State of this agent after the duties (it may have been paused, stopped, ...).
    pub state: Option<State>,
    /// Security events that the duties found themselves (`deleted-message`, `own_notes`). The
    /// cycle shows them with the security events of the feed.
    pub security: Vec<SecurityEvent>,
}

/// The duties of every agent. Reads `rows` (the tickets, by id) and writes only to the agent's
/// own item and to the registry issue.
pub fn agent_duties(ac: &AgentCtx, world: &World, rows: &[TicketRow], now: i64) -> CbResult<Duty> {
    let me = ac.name.to_string();
    let mut duty = Duty {
        state: world
            .items
            .get(&ac.agent.item_id)
            .and_then(|i| i.field("State"))
            .and_then(State::parse),
        ..Duty::default()
    };
    let running = world
        .items
        .get(&ac.agent.item_id)
        .and_then(|i| i.field("State"))
        .is_some_and(heartbeat_state);
    let msgs = registry_msgs(ac, world);
    // 1. owner commands that name this agent
    let (lines, state) = apply_owner_commands(ac, world, duty.state, rows)?;
    duty.state = state;
    for l in lines {
        duty.events.push(liveness_event(ac, "owner-command", l));
    }
    // 2. a draining agent without open tickets leaves
    if let Some(text) = finish_draining(ac, world, duty.state, rows)? {
        duty.state = Some(State::Left);
        duty.events
            .push(liveness_event(ac, "left", text.trim_end().to_string()));
    }
    // 3. a running dev agent watches the coordinator: if it is late, the owner hears once
    //    (spec/agents#coordinator-failure-one-owner-alert). No event; the model is not woken.
    let is_dev = world.trust.get(&me).is_none_or(|t| t.role == Role::Dev);
    if running && is_dev {
        tell_owner_coordinator_late(ac, world, &msgs, now)?;
    }
    Ok(duty)
}

/// Judge the coordinator by its heartbeat and, if it is late, post ONE signed `info` to the owner
/// in the registry issue ([`late_owner_duty`]). The note is no `Event`: it never reaches the
/// output or the exit code.
fn tell_owner_coordinator_late(
    ac: &AgentCtx,
    world: &World,
    msgs: &[VMsg],
    now: i64,
) -> CbResult<()> {
    let e = crate::late::expect(&ac.ctx.cfg);
    let waits = agent_waits(&ac.ctx, world, now);
    let Some(coordinator) = late_coordinator(&waits) else {
        return Ok(());
    };
    let note = Note {
        expect: &e,
        coordinator: Some(coordinator),
        now,
        detail: "",
    };
    if let Some(p) = late_owner_duty(ac, coordinator, &note, msgs)? {
        eprintln!(
            "crewboard: the coordinator {} is late ({}); told the owner ({}). You need not act.",
            coordinator.subject_name(),
            coordinator.wait.word(),
            p.url
        );
    }
    Ok(())
}

/// What a dead report of one episode starts with: the agent and the heartbeat value it was last
/// seen with. A revived agent writes a new heartbeat, so its next death is a new episode.
fn dead_report_head(agent: &str, since: i64) -> String {
    format!(
        "agent-dead: {agent}\nlast heartbeat: {} (",
        crate::clock::format_ts(since)
    )
}

/// The parts of a dead report.
struct DeadReport<'a> {
    agent: &'a str,
    since: i64,
    age: i64,
    interval: i64,
    reassigned: &'a [String],
    kept: &'a [String],
    failed: &'a [String],
}

fn dead_report_body(d: &DeadReport) -> String {
    let mut s = format!(
        "{}{} s ago; late after {} s, dead after {} s)\n",
        dead_report_head(d.agent, d.since),
        d.age,
        LATE_INTERVALS * d.interval,
        DEAD_INTERVALS * d.interval
    );
    s.push_str(&format!(
        "tickets returned to Backlog (assign them again): {}\n",
        if d.reassigned.is_empty() {
            "none".to_string()
        } else {
            d.reassigned.join(", ")
        }
    ));
    if !d.failed.is_empty() {
        s.push_str(&format!(
            "tickets that could not be returned (the next cycle tries again): {}\n",
            d.failed.join(", ")
        ));
    }
    if !d.kept.is_empty() {
        s.push_str(&format!(
            "tickets left as they are (Review: the evidence is delivered, verify needs no dev): {}\n",
            d.kept.join(", ")
        ));
    }
    s
}

/// What one cycle of the coordinator reads, the same for every duty.
struct Cycle<'a> {
    ac: &'a AgentCtx,
    world: &'a World,
    rows: &'a [TicketRow],
    msgs: Vec<VMsg>,
    now: i64,
    interval: i64,
}

/// One approved agent as the coordinator judges it.
struct Judged<'a> {
    name: String,
    /// What the registry log says: a stop or an end of the agent.
    departure: Option<Departure>,
    state: Option<String>,
    since: Option<i64>,
    live: Liveness,
    /// The tickets that name the agent.
    theirs: Vec<&'a TicketRow>,
}

/// What `return_tickets` did: the tickets that went back to Backlog (with a note on earlier
/// work) and the tickets whose return failed (with the error).
#[derive(Default)]
struct Returned {
    done: Vec<String>,
    failed: Vec<String>,
}

/// Return `tickets` to Backlog. A failure of one ticket does not stop the others: the successes
/// stay in `duty` and in the result, the failures are listed so that the report can say them.
fn return_tickets(c: &Cycle, tickets: &[&TicketRow], why: &str, duty: &mut Duty) -> Returned {
    let mut out = Returned::default();
    for r in tickets {
        match reassign_derived(c.ac, r, why) {
            Ok(earlier) => {
                duty.snaps.push((
                    r.item_id.clone(),
                    TicketSnap {
                        status: Some(Status::Backlog.name().into()),
                        agent: None,
                    },
                ));
                out.done.push(tag_earlier_work(&r.key(), earlier.as_ref()));
            }
            Err(e) => out.failed.push(format!("{} ({e})", r.key())),
        }
    }
    out
}

/// The tickets of `j` that go back to Backlog: the open ones, and those in Backlog that still name
/// the agent (a return whose last write, the clear of the Agent, failed; `reassign_derived`
/// finishes it).
fn reassignable<'a>(j: &Judged<'a>) -> Vec<&'a TicketRow> {
    j.theirs
        .iter()
        .copied()
        .filter(|r| {
            r.status_board()
                .is_some_and(|s| reassignable_parsed(s) || s == Status::Backlog)
        })
        .collect()
}

/// A paused agent takes nothing new: its Ready tickets wait. One event per change of the set
/// (`late: take` is not the default, so nothing else tells the coordinator).
fn paused_ready_duty(c: &Cycle, j: &Judged, cache: &mut StateCache, duty: &mut Duty) {
    let held = crate::late::liveness::paused_ready(j.state.as_deref(), &j.name, c.rows);
    let note_key = format!("paused-ready:{}", j.name);
    if held.is_empty() {
        cache.liveness.liveness.remove(&note_key);
    } else if cache.liveness.liveness.get(&note_key) != Some(&held.join(",")) {
        cache.liveness.liveness.insert(note_key, held.join(","));
        duty.events.push(liveness_event(
            c.ac,
            "agent-paused-ready",
            crate::late::liveness::paused_ready_text(&j.name, &held),
        ));
    }
}

/// Agents that the registry log stops or ends: their open tickets go back to Backlog, whatever
/// the heartbeat and the board `State` say.
fn departed_duty(c: &Cycle, j: &Judged, d: Departure, duty: &mut Duty) -> CbResult<()> {
    let why = derived_reassign_reason(
        &j.name,
        Some(d.reason()),
        j.state.as_deref(),
        &j.live,
        c.interval,
    )?;
    let ret = return_tickets(c, &reassignable(j), &why, duty);
    if ret.done.is_empty() && ret.failed.is_empty() {
        return Ok(());
    }
    let mut text = format!(
        "{} {}: returned {} ticket(s) to Backlog (assign them again): {}",
        j.name,
        d.reason(),
        ret.done.len(),
        ret.done.join(", ")
    );
    if !ret.failed.is_empty() {
        text.push_str(&format!(
            "\ncould not return (the next cycle tries again): {}",
            ret.failed.join(", ")
        ));
    }
    duty.events
        .push(liveness_event(c.ac, "agent-stopped", text));
    Ok(())
}

/// A board `State` of `draining` or `left` that no log entry supports: nothing is returned (one
/// field write must not take the tickets of an agent), and the owner hears of it ONCE (the line
/// `event=` is the key, read back from the registry issue). The first sight may be a write in
/// flight (an agent writes `draining` a moment before it signs its notice), so the note waits
/// for the next cycle that still sees it, as the Status check does (`log_check`).
fn unlogged_state_duty(
    c: &Cycle,
    j: &Judged,
    cache: &mut StateCache,
    duty: &mut Duty,
) -> CbResult<()> {
    let seen_key = format!("state-unlogged:{}", j.name);
    let Some(state) = j
        .state
        .as_deref()
        .filter(|s| matches!(*s, "draining" | "left"))
    else {
        cache.liveness.liveness.remove(&seen_key);
        return Ok(());
    };
    if cache.liveness.liveness.get(&seen_key).map(String::as_str) != Some(state) {
        cache.liveness.liveness.insert(seen_key, state.to_string());
        return Ok(());
    }
    let key = state_event_key(&j.name, state);
    if reported_events(&c.msgs).contains(&key) {
        return Ok(());
    }
    let name = &j.name;
    let body = format!(
        "Board State without a stop in the log: {name}\nevent={key}\nThe board State of {name} is {state}, but the registry log has no stop of it (no /crew stop, no signed notice or lifecycle: left marker of the agent, no /crew reject). The log decides: {name} may still act and keeps its tickets. To stop it, run `crewboard stop --agent {name} --force`; else set the field back.\n"
    );
    let p = post_registry(c.ac, To::Owner, Kind::Info, Re::None, &body)?;
    duty.events.push(liveness_event(
        c.ac,
        "agent-state-unlogged",
        format!(
            "{name} is {state} on the board, but the registry log has no stop of it: its tickets stay; told the owner: {}",
            p.url
        ),
    ));
    Ok(())
}

/// A dead agent: its open tickets go back to Backlog, one report to all and one late note to the
/// owner per dead episode.
fn dead_duty(c: &Cycle, j: &Judged, age: i64, duty: &mut Duty) -> CbResult<()> {
    let (ac, now, interval) = (c.ac, c.now, c.interval);
    let me = ac.name.to_string();
    let why = derived_reassign_reason(&j.name, None, j.state.as_deref(), &j.live, interval)?;
    let ret = return_tickets(c, &reassignable(j), &why, duty);
    let kept: Vec<String> = j
        .theirs
        .iter()
        .filter(|r| r.in_status(Status::Review))
        .map(|r| format!("{} ({})", r.key(), r.status_str().unwrap_or("")))
        .collect();
    // `Dead` comes from `since`, so it is there.
    let seen = j.since.unwrap_or(now - age);
    let body = dead_report_body(&DeadReport {
        agent: &j.name,
        since: seen,
        age,
        interval,
        reassigned: &ret.done,
        kept: &kept,
        failed: &ret.failed,
    });
    // The report is posted once per dead episode: the heartbeat value that the agent was last
    // seen with is the episode, and a report of this coordinator that starts with it exists or
    // it does not. The owner hears through the late path, once per episode (`late-key`).
    let head = dead_report_head(&j.name, seen);
    let have_report = c.msgs.iter().any(|m| {
        m.kind == Some(Kind::Report)
            && m.to == "all"
            && matches!(&m.sender, crate::ticket::Sender::Agent { name: n, .. } if *n == me)
            && m.body.starts_with(&head)
    });
    // ONE event per dead episode: the one that posts the report (the board decides, not a local
    // cache), so a second poll or a restarted coordinator says nothing.
    let mut first = false;
    if !have_report {
        post_registry(ac, To::All, Kind::Report, Re::None, &body)?;
        first = true;
    }
    let late_dead = agent_late(
        &AgentLate {
            name: j.name.clone(),
            coordinator: false,
            since: j.since,
            live: j.live.clone(),
        },
        now,
        interval,
    );
    if let Some(l) = late_dead {
        let expect = crate::late::expect(&ac.ctx.cfg);
        let note = Note {
            expect: &expect,
            coordinator: None,
            now,
            detail: &body,
        };
        if late_owner_duty(ac, &l, &note, &c.msgs)?.is_some() {
            first = true;
        }
    }
    if first || !ret.done.is_empty() {
        duty.events.push(liveness_event(
            ac,
            "agent-dead",
            body.trim_end().to_string(),
        ));
    }
    Ok(())
}

/// Security events go to the owner once (the coordinator itself reads them in its output).
fn security_duty(c: &Cycle, security: &[SecurityEvent], duty: &mut Duty) -> CbResult<()> {
    let mut reported = reported_events(&c.msgs);
    for s in security {
        if let Some(url) = report_security(c.ac, s, &mut reported)? {
            duty.events.push(liveness_event(
                c.ac,
                "security-reported",
                format!(
                    "{} (comment {}) reported to the owner: {url}",
                    s.reason.as_str(),
                    s.comment_id
                ),
            ));
        }
    }
    Ok(())
}

/// The owner-route safety net. The headless coordinator may not write; its `tick` logs the
/// finding instead.
fn owner_route_duty(c: &Cycle, cache: &mut StateCache, duty: &mut Duty) -> CbResult<()> {
    if c.ac.ctx.cd.is_headless()? {
        return Ok(());
    }
    let run = super::tickets::owner_net::run(c.ac, c.world, c.rows, cache);
    for r in run.reopened {
        duty.snaps.push((
            r.item_id,
            TicketSnap {
                status: Some(Status::Done.name().into()),
                agent: r.agent,
            },
        ));
        duty.events
            .push(liveness_event(c.ac, "owner-route-reopened", r.text));
    }
    if let Some(e) = run.failure {
        duty.events.push(liveness_event(
            c.ac,
            "owner-route-check-failed",
            format!("the owner-route check of the Done tickets failed: {e}"),
        ));
    }
    Ok(())
}

/// The duties of the coordinator. `security` are this cycle's security events.
pub fn coordinator_duties(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    security: &[SecurityEvent],
    cache: &mut StateCache,
    now: i64,
) -> CbResult<Duty> {
    let me = ac.name.to_string();
    let c = Cycle {
        ac,
        world,
        rows,
        msgs: registry_msgs(ac, world),
        now,
        interval: ac.ctx.hb_interval,
    };
    let mut duty = Duty::default();
    // Approved agents, and agents that the owner rejected after an approval: a reject is an
    // end in the log, so their tickets go back too.
    let names: Vec<(String, Option<Departure>)> = world
        .reg
        .agents
        .values()
        .filter(|e| e.approval.is_some() && e.name != me)
        .map(|e| (e.name.clone(), e.departure))
        .collect();
    for (name, departure) in names {
        let (state, since, live) = agent_liveness(world, &name, now, c.interval);
        let theirs = rows.iter().filter(|r| r.is_agent(name.as_str())).collect();
        let j = Judged {
            name,
            departure,
            state,
            since,
            live,
            theirs,
        };
        if let Some(d) = j.departure {
            departed_duty(&c, &j, d, &mut duty)?;
            cache.liveness.liveness.remove(&j.name);
            cache
                .liveness
                .liveness
                .remove(&format!("state-unlogged:{}", j.name));
            continue;
        }
        paused_ready_duty(&c, &j, cache, &mut duty);
        unlogged_state_duty(&c, &j, cache, &mut duty)?;
        match j.live {
            Liveness::NotRunning | Liveness::Alive { .. } => {
                cache.liveness.liveness.remove(&j.name);
            }
            // A late agent is only shown (`status`, the late table). Nothing is sent to it, and
            // the model hears of it only if the agent turns out dead.
            Liveness::Late { .. } => {}
            Liveness::Dead { age } => dead_duty(&c, &j, age, &mut duty)?,
        }
    }
    // A board Status that the signed log does not support is detected here (the crew token
    // can write the field, crewboard cannot prevent it).
    let own = duty.snaps.clone();
    let check = super::log_check::status_log_duty(ac, world, rows, &own, &c.msgs, cache, now);
    duty.events.extend(check.events);
    // A deleted note of this coordinator: the lists that the Status check read, then the tickets
    // whose board row changed (spec/security-details#deleted-notes).
    let own_ids: HashSet<&str> = own.iter().map(|(i, _)| i.as_str()).collect();
    let mut found = check.security;
    let notes = super::own_notes::cycle_duty(
        ac,
        world,
        rows,
        &cache.index.ticket_snaps,
        &own_ids,
        &check.read,
        &mut cache.notes,
        now,
    );
    found.extend(notes.security);
    duty.events.extend(
        notes
            .not_checked
            .into_iter()
            .map(|text| liveness_event(ac, super::own_notes::NOT_CHECKED, text)),
    );
    let all: Vec<SecurityEvent> = security.iter().chain(&found).cloned().collect();
    security_duty(&c, &all, &mut duty)?;
    duty.security = found;
    owner_route_duty(&c, cache, &mut duty)?;
    Ok(duty)
}

/// Without a coordinator on the board the agent that sees a security event reports it.
pub fn orphan_security_duty(
    ac: &AgentCtx,
    world: &World,
    security: &[SecurityEvent],
) -> CbResult<Vec<Event>> {
    if world.trust.coordinator_exists() || security.is_empty() {
        return Ok(Vec::new());
    }
    let msgs = registry_msgs(ac, world);
    let mut reported = reported_events(&msgs);
    let mut out = Vec::new();
    for s in security {
        if let Some(url) = report_security(ac, s, &mut reported)? {
            out.push(liveness_event(
                ac,
                "security-reported",
                format!(
                    "{} (comment {}) reported to the owner (no coordinator on the board): {url}",
                    s.reason.as_str(),
                    s.comment_id
                ),
            ));
        }
    }
    Ok(out)
}
