//! `pause`, `resume`, `stop` (spec/board#lifecycle) and the owner commands `/crew pause|resume|stop
//! <name>`.
//!
//! The `State` of an agent is written by the agent itself (spec/board#fields). An owner command is
//! an owner comment in the registry issue; the agent it names reads it in its next `watch` or
//! `poll`, applies it, and answers with a signed `kind=ack` (`re` = the owner's comment). The
//! ack is the record that the command was applied, so it is applied once, and the board alone
//! (no local file) tells what is still pending.
//!
//! - `pause`: `active` or `working` -> `paused`. The agent finishes its current step and takes
//!   nothing new; `assign` and `take` refuse it. It keeps its heartbeat and answers pings.
//! - `resume`: `paused` -> `active`. A pause by the owner is lifted only by the owner.
//! - `stop`: a working agent releases first (`working` -> `active`), then `active` or `paused`
//!   -> `draining`. The coordinator returns the agent's open tickets to `Backlog` with a comment
//!   (reason: stopping); when none is left the agent becomes `left`. A stop that finds no open
//!   ticket ends in `left` at once.

use super::say::send_signed;
use super::tickets::{load_world_cached, me_of};
use super::{AgentCtx, Global, World, resolve_board_config};
use crate::clock::now_ts;
use crate::comments::Comment;
use crate::ctx::registry_msgs;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::late::liveness::reassignable_parsed;
use crate::owner_cmd::{self, OwnerCommand, Verb};
use crate::protocol::names::{Kind, Re, To};
use crate::registry::CREW_PREFIX;
use crate::rules::lifecycle::{State, Who, check_lifecycle, check_resume};
use crate::services::ticket_index::{discover_tickets, read_ticket_rows};
use crate::ticket::{Sender, TicketRow, VMsg};

/// The owner's lifecycle commands in the registry comments, in comment order.
pub fn owner_commands(
    registry_comments: &[Comment],
    owner_login: &str,
) -> Vec<(u64, OwnerCommand)> {
    let mut v: Vec<(u64, OwnerCommand)> = registry_comments
        .iter()
        .filter(|c| c.by(owner_login) && !c.body.starts_with(CREW_PREFIX))
        .filter(|c| c.untrusted_owner_edit(owner_login).is_none())
        .filter_map(|c| match owner_cmd::parse(&c.body) {
            Ok(Some(cmd)) if owner_cmd::lifecycle_of(&cmd).is_some() => Some((c.id, cmd)),
            _ => None,
        })
        .collect();
    v.sort_by_key(|(id, _)| *id);
    v
}

/// Did the owner pause this agent, and not resume it since?
pub fn owner_paused(cmds: &[(u64, OwnerCommand)], me: &str) -> bool {
    matches!(owner_cmd::latest_verb_for(cmds, me), Some((_, Verb::Pause)))
}

/// Owner commands for `me` that no valid ack of `me` answers yet.
pub fn pending_commands(
    cmds: &[(u64, OwnerCommand)],
    registry_msgs: &[VMsg],
    registry_issue: u64,
    me: &str,
) -> Vec<(u64, Verb)> {
    cmds.iter()
        .filter_map(|(id, c)| {
            let (v, n) = owner_cmd::lifecycle_of(c)?;
            (n.as_str() == me).then_some((*id, v))
        })
        .filter(|(id, _)| {
            !registry_msgs.iter().any(|m| {
                m.kind == Some(Kind::Ack)
                    && matches!(&m.sender, Sender::Agent { name, .. } if name == me)
                    && m.re
                        == Re::Comment {
                            issue: registry_issue,
                            comment: *id,
                        }
            })
        })
        .collect()
}

fn set_state(ac: &AgentCtx, s: State) -> CbResult<()> {
    ac.ctx
        .board
        .set_select(&ac.ctx.client, &ac.agent.item_id, "State", s.name())
}

fn set_doing(ac: &AgentCtx, text: &str) -> CbResult<()> {
    ac.ctx
        .board
        .set_text(&ac.ctx.client, &ac.agent.item_id, "Doing", text)
}

/// A signed notice in the registry issue (the audit trail of departures,
/// spec/board#registry-issue). It goes to the coordinator; the coordinator itself writes to the
/// owner.
fn notice(ac: &AgentCtx, world: &World, text: &str) -> CbResult<()> {
    let me_coordinator = world
        .trust
        .get(ac.name.as_str())
        .is_some_and(|t| t.role == crate::rules::Role::Coordinator);
    send_signed(
        ac,
        ac.ctx.main_repo(),
        ac.ctx.cfg.registry_issue,
        if me_coordinator {
            To::Owner
        } else {
            To::Coordinator
        },
        Kind::Info,
        Re::None,
        text,
        &now_ts(),
    )?;
    Ok(())
}

/// `pause`. Returns the new state and the text for the user.
pub fn pause(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    who: Who,
) -> CbResult<(State, String)> {
    check_lifecycle(state, State::Paused, who)?;
    set_state(ac, State::Paused)?;
    notice(
        ac,
        world,
        &format!(
            "{} is paused: it finishes its current step and takes nothing new.",
            ac.name
        ),
    )?;
    Ok((
        State::Paused,
        format!(
            "{} paused (was {}); it takes nothing new, finishes its current step, and keeps its heartbeat\n",
            ac.name,
            state.map_or("?", State::name)
        ),
    ))
}

/// `resume`.
pub fn resume(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    who: Who,
    owner_paused: bool,
) -> CbResult<(State, String)> {
    check_resume(who, owner_paused)?;
    check_lifecycle(state, State::Active, who)?;
    set_state(ac, State::Active)?;
    ac.ctx
        .board
        .set_text(&ac.ctx.client, &ac.agent.item_id, "Heartbeat", &now_ts())?;
    notice(ac, world, &format!("{} resumed.", ac.name))?;
    Ok((
        State::Active,
        format!("{} resumed: State active\n", ac.name),
    ))
}

/// Open tickets of this agent that the coordinator returns to `Backlog` when it stops.
pub fn open_tickets<'a>(rows: &'a [TicketRow], me: &str) -> Vec<&'a TicketRow> {
    rows.iter()
        .filter(|r| r.is_agent(me))
        .filter(|r| r.status_board().is_some_and(reassignable_parsed))
        .collect()
}

/// `stop`: release, drain, and leave if nothing is open. Idempotent: a draining agent that
/// calls it again only finishes.
pub fn stop(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    who: Who,
    rows: &[TicketRow],
) -> CbResult<(State, String)> {
    let mut out = String::new();
    let mut state = state;
    if state == Some(State::Approved) {
        // spec/board#lifecycle: an approved agent that never started leaves directly. It holds
        // no ticket and posted no heartbeat, so there is nothing to drain.
        return finish(ac, world, state, who);
    }
    if state != Some(State::Draining) {
        check_lifecycle(state, State::Draining, who)?;
        set_state(ac, State::Draining)?;
        set_doing(ac, "draining")?;
        out.push_str(&format!(
            "{} draining (was {})\n",
            ac.name,
            state.map_or("?", State::name)
        ));
        state = Some(State::Draining);
    } else {
        out.push_str(&format!("{} is draining already\n", ac.name));
    }
    let open = open_tickets(rows, ac.name.as_str());
    let list: Vec<String> = open
        .iter()
        .map(|r| format!("{} ({})", r.key(), r.status_str().unwrap_or("?")))
        .collect();
    // The signed notice is the stop in the registry log: the coordinator returns the tickets of
    // an agent that the log stops, never of a board State `draining` alone.
    notice(
        ac,
        world,
        &crate::protocol::acts::stopping_body(
            ac.name.as_str(),
            &if list.is_empty() {
                "none".to_string()
            } else {
                list.join(", ")
            },
        ),
    )?;
    if open.is_empty() {
        let (s, t) = finish(ac, world, state, who)?;
        out.push_str(&t);
        return Ok((s, out));
    }
    out.push_str(&format!(
        "waiting for the coordinator to return {} open ticket(s) to Backlog: {}\nrun `crewboard stop` (or `watch`/`poll`) again after that: the agent then becomes left\n",
        open.len(),
        list.join(", ")
    ));
    Ok((State::Draining, out))
}

/// `draining` -> `left`. The agent first signs the marker `lifecycle: left` in the registry
/// issue: it ends the span in which the agent may speak (spec/identity#trust-at-signing-time),
/// and counts itself. If the post fails, the State stays `draining`: no marker, no `left`.
fn finish(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    who: Who,
) -> CbResult<(State, String)> {
    check_lifecycle(state, State::Left, who)?;
    notice(
        ac,
        world,
        &crate::protocol::acts::left_body(ac.name.as_str()),
    )?;
    set_state(ac, State::Left)?;
    set_doing(ac, "left")?;
    Ok((State::Left, format!("{} left (State left)\n", ac.name)))
}

/// In a `watch`/`poll` cycle: a draining agent that holds no open ticket any more leaves. Only
/// when the registry log stops it (its own signed notice, the owner's `/crew stop`): a board
/// State `draining` alone (a hand edit) must not make the agent end its own span.
pub fn finish_draining(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    rows: &[TicketRow],
) -> CbResult<Option<String>> {
    if state != Some(State::Draining) || !open_tickets(rows, ac.name.as_str()).is_empty() {
        return Ok(None);
    }
    let me = ac.name.as_str();
    if world.reg.agents.get(me).and_then(|e| e.departure).is_none() {
        eprintln!(
            "crewboard: {me} is draining on the board, but the registry log has no stop of it; it does not leave. `crewboard stop` signs the stop, or set the State back"
        );
        return Ok(None);
    }
    let (_, text) = finish(ac, world, state, Who::SelfAgent)?;
    Ok(Some(text))
}

/// The own tickets of this agent, read by id.
pub(crate) fn my_rows(ac: &AgentCtx, world: &World) -> CbResult<Vec<TicketRow>> {
    rows_of(ac, world, ac.name.as_str())
}

/// The tickets of the agent `agent`, read by id. The coordinator reads those of a dev agent
/// with it; `my_rows` is the case `agent` = the caller.
pub(crate) fn rows_of(ac: &AgentCtx, world: &World, agent: &str) -> CbResult<Vec<TicketRow>> {
    let mut cache = ac.load_cache();
    cache.index.drop_foreign(&ac.ctx.cfg.repos);
    if !cache.index.tickets_scanned {
        discover_tickets(&ac.ctx.reader(), &world.trust, &mut cache.index)?;
    }
    // A ticket assigned after the last scan is announced by a signed info comment. `watch`
    // learns it from the feed; a command that runs between two polls must do the same, or it
    // misses the fresh ticket (the item listing lags and is never used for this).
    let since = world
        .items
        .get(&ac.agent.item_id)
        .and_then(|i| i.field("Cursor"))
        .and_then(|c| crate::cursor::Cursor::decode(c).ok())
        .map(|c| c.since());
    if let Some(since) = since {
        for repo in &ac.ctx.cfg.repos {
            let feed = crate::feed::feed_since(
                &ac.ctx.client,
                repo,
                since,
                None,
                &ac.ctx.cfg.owner_login,
            )?;
            for (k, id) in
                crate::ticket::announced_item_ids(&feed.comments, &ac.ctx.cfg, &world.trust)
            {
                cache.index.tickets.insert(k, id);
            }
        }
    }
    let rows = read_ticket_rows(&ac.ctx.reader(), &mut cache.index.tickets)?;
    ac.save_cache(&cache);
    Ok(rows.into_iter().filter(|r| r.is_agent(agent)).collect())
}

/// The command line `crewboard pause|resume|stop` of an agent.
pub fn run_agent(ac: &AgentCtx, verb: Verb) -> CbResult<String> {
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let cmds = owner_commands(&world.registry_comments, &ac.ctx.cfg.owner_login);
    let (_, text) = match verb {
        Verb::Pause => pause(ac, &world, me.state, Who::SelfAgent)?,
        Verb::Resume => resume(
            ac,
            &world,
            me.state,
            Who::SelfAgent,
            owner_paused(&cmds, ac.name.as_str()),
        )?,
        Verb::Stop => {
            let rows = my_rows(ac, &world)?;
            stop(ac, &world, me.state, Who::SelfAgent, &rows)?
        }
    };
    Ok(text)
}

/// `heartbeat`: one heartbeat now (`watch` and `poll` also write them). Refused for an agent
/// that does not run (requested, approved, left): it has nothing to say.
pub fn heartbeat(ac: &AgentCtx) -> CbResult<String> {
    let world = load_world_cached(ac)?;
    me_of(ac, &world)?;
    let state = world
        .items
        .get(&ac.agent.item_id)
        .and_then(|i| i.field("State"))
        .unwrap_or("?");
    if !crate::late::liveness::heartbeat_state(state) {
        return Err(CbError::refused(
            "heartbeat-not-running",
            format!(
                "{} is in State '{state}': only a running agent writes heartbeats",
                ac.name
            ),
        ));
    }
    let now = now_ts();
    ac.ctx
        .board
        .set_text(&ac.ctx.client, &ac.agent.item_id, "Heartbeat", &now)?;
    Ok(format!(
        "heartbeat {now} (interval {} s: late after {} s)\n",
        ac.ctx.hb_interval,
        crate::late::liveness::LATE_INTERVALS * ac.ctx.hb_interval
    ))
}

/// Apply the owner's commands that name this agent and have no ack yet. Each is answered with
/// a signed ack (also when it is refused by the lifecycle: the owner reads why). Returns the
/// lines for the cycle output and the state afterwards.
pub fn apply_owner_commands(
    ac: &AgentCtx,
    world: &World,
    state: Option<State>,
    rows: &[TicketRow],
) -> CbResult<(Vec<String>, Option<State>)> {
    let msgs = registry_msgs(ac, world);
    let cmds = owner_commands(&world.registry_comments, &ac.ctx.cfg.owner_login);
    let pending = pending_commands(&cmds, &msgs, ac.ctx.cfg.registry_issue, ac.name.as_str());
    let mut lines = Vec::new();
    let mut state = state;
    for (id, verb) in pending {
        let result = match verb {
            Verb::Pause => pause(ac, world, state, Who::Owner),
            // the owner's resume lifts the owner's pause, so `owner_paused` is false here
            Verb::Resume => resume(ac, world, state, Who::Owner, false),
            Verb::Stop => stop(ac, world, state, Who::Owner, rows),
        };
        let (answer, line) = match result {
            Ok((s, text)) => {
                state = Some(s);
                (
                    format!(
                        "applied: /crew {} {}\nState now {}",
                        verb.word(),
                        ac.name,
                        s.name()
                    ),
                    format!(
                        "owner command /crew {} applied: {}",
                        verb.word(),
                        text.trim_end()
                    ),
                )
            }
            Err(CbError::Refused(r)) => (
                format!("refused: /crew {} {}\n{r}", verb.word(), ac.name),
                format!("owner command /crew {} refused: {r}", verb.word()),
            ),
            Err(e) => return Err(e),
        };
        send_signed(
            ac,
            ac.ctx.main_repo(),
            ac.ctx.cfg.registry_issue,
            To::Owner,
            Kind::Ack,
            Re::Comment {
                issue: ac.ctx.cfg.registry_issue,
                comment: id,
            },
            &answer,
            &now_ts(),
        )?;
        lines.push(line);
    }
    Ok((lines, state))
}

/// `crewboard pause|resume|stop --agent <name>` with the owner's token: post the owner command.
pub fn run_owner(g: &Global, verb: Verb, agent: &str) -> CbResult<String> {
    let name = crate::protocol::names::AgentName::parse(agent)?;
    let cfg = resolve_board_config(g)?;
    let client = super::new_owner_client()?;
    super::require_owner(&client, &cfg, "may command another agent")?;
    let ctx = super::Ctx::load(client, cfg, g.dir()?)?;
    let world = super::load_world(&ctx, &[], &mut crate::state::StateCache::default())?;
    let entry = world.reg.agents.get(name.as_str()).ok_or_else(|| {
        CbError::refused(
            "no-join-request",
            format!("{name} has no valid join request in the registry issue"),
        )
    })?;
    if !entry.approved() {
        return Err(CbError::refused(
            "agent-not-approved",
            format!("{name} is not an approved agent"),
        ));
    }
    let text = format!("/crew {} {name}", verb.word());
    let (id, url) =
        feed::post_comment(&ctx.client, ctx.main_repo(), ctx.cfg.registry_issue, &text)?;
    Ok(format!(
        "owner command posted: {text} (comment {id}, {url})\n{name} applies it at its next `watch` or `poll` and answers with a signed ack\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::tests::comment;

    fn cmds(texts: &[(u64, &str)]) -> Vec<(u64, OwnerCommand)> {
        let cs: Vec<Comment> = texts
            .iter()
            .map(|(i, t)| comment(*i, "owner-login", t))
            .collect();
        owner_commands(&cs, "owner-login")
    }

    #[test]
    fn only_lifecycle_commands_of_the_owner_are_collected_in_comment_order() {
        let mut cs = vec![
            comment(30, "owner-login", "/crew stop a@x"),
            comment(10, "owner-login", "/crew pause a@x"),
            comment(20, "owner-login", "/crew approve b@x"),
            comment(25, "owner-login", "just text"),
            comment(26, "owner-login", "/crew pause"),
        ];
        // a stranger's command and a header comment are not the owner's command
        cs.push(comment(11, "mallory", "/crew stop b@x"));
        cs.push(comment(
            12,
            "owner-login",
            "<!-- crew v=1 -->\n/crew stop b@x",
        ));
        let got = owner_commands(&cs, "owner-login");
        let ids: Vec<u64> = got.iter().map(|(i, _)| *i).collect();
        assert_eq!(ids, vec![10, 30]);
    }

    #[test]
    fn an_owner_pause_holds_until_the_owner_resumes() {
        assert!(owner_paused(&cmds(&[(1, "/crew pause a@x")]), "a@x"));
        assert!(!owner_paused(
            &cmds(&[(1, "/crew pause a@x"), (2, "/crew resume a@x")]),
            "a@x"
        ));
        assert!(owner_paused(
            &cmds(&[
                (1, "/crew pause a@x"),
                (2, "/crew resume a@x"),
                (3, "/crew pause a@x")
            ]),
            "a@x"
        ));
        // another agent's pause is not mine; nothing at all is no hold
        assert!(!owner_paused(&cmds(&[(1, "/crew pause b@x")]), "a@x"));
        assert!(!owner_paused(&[], "a@x"));
    }

    #[test]
    fn a_command_is_pending_until_the_agent_acks_exactly_that_comment() {
        let c = cmds(&[
            (10, "/crew pause a@x"),
            (20, "/crew pause b@x"),
            (30, "/crew resume a@x"),
        ]);
        let ack = |id: u64, name: &str, re_comment: u64| VMsg {
            ts: String::new(),
            comment_id: id,
            created: 0,
            sender: Sender::Agent {
                name: name.into(),
                role: crate::rules::Role::Dev,
            },
            to: "owner".into(),
            kind: Some(Kind::Ack),
            re: Re::Comment {
                issue: 2,
                comment: re_comment,
            },
            body: String::new(),
        };
        let p = pending_commands(&c, &[], 2, "a@x");
        assert_eq!(p, vec![(10, Verb::Pause), (30, Verb::Resume)]);
        // an ack of a@x for 10 leaves 30; an ack of b@x for 30 or in another issue counts for nothing
        let acks = vec![ack(100, "a@x", 10), ack(101, "b@x", 30)];
        assert_eq!(
            pending_commands(&c, &acks, 2, "a@x"),
            vec![(30, Verb::Resume)]
        );
        let mut wrong_issue = ack(102, "a@x", 30);
        wrong_issue.re = Re::Comment {
            issue: 9,
            comment: 30,
        };
        assert_eq!(
            pending_commands(&c, &[wrong_issue], 2, "a@x").len(),
            2,
            "an ack in another issue answers nothing"
        );
        // both acked: nothing is pending
        let all = vec![ack(100, "a@x", 10), ack(103, "a@x", 30)];
        assert!(pending_commands(&c, &all, 2, "a@x").is_empty());
        // the ack must be from the agent itself and kind=ack
        let mut not_ack = ack(104, "a@x", 10);
        not_ack.kind = Some(Kind::Info);
        assert_eq!(pending_commands(&c, &[not_ack], 2, "a@x").len(), 2);
    }

    #[test]
    fn open_tickets_are_the_ones_a_coordinator_can_return() {
        let row = |n: u64, status: &str, agent: &str| TicketRow {
            updated: None,
            repo: "a/b".into(),
            issue: n,
            item_id: format!("PVTI_{n}"),
            status: Some(status.into()),
            agent: Some(agent.into()),
            order: None,
        };
        let rows = vec![
            row(1, "In Progress", "me@x"),
            row(2, "Ready", "me@x"),
            row(3, "Blocked", "me@x"),
            row(4, "Review", "me@x"),
            row(5, "Blocked", "me@x"),
            row(6, "Done", "me@x"),
            row(7, "In Progress", "other@x"),
            row(8, "Needs Human", "me@x"),
        ];
        let open: Vec<u64> = open_tickets(&rows, "me@x")
            .iter()
            .map(|r| r.issue)
            .collect();
        // the legacy option reads as Blocked
        assert_eq!(open, vec![1, 2, 3, 5, 8]);
        assert!(open_tickets(&rows, "nobody@x").is_empty());
    }

    #[test]
    fn state_names_match_the_board_options() {
        for (s, n) in [
            (State::Requested, "requested"),
            (State::Approved, "approved"),
            (State::Active, "active"),
            (State::Paused, "paused"),
            (State::Draining, "draining"),
            (State::Left, "left"),
        ] {
            assert_eq!(s.name(), n);
            assert_eq!(State::parse(n), Some(s));
        }
        // An old board may still hold the legacy value: it reads as `active`.
        assert_eq!(State::parse("working"), Some(State::Active));
    }
}
