//! `stop --force --agent <name>` (spec/board#lifecycle): the owner or the coordinator ends an
//! agent that cannot drain (a crashed host, a lost key): its open tickets go back as a derived
//! `reassign` does (to Backlog), and its State becomes `left`.
//!
//! A normal `stop` needs the agent itself: it drains, the coordinator returns the tickets, and
//! the agent signs the `left` marker. A dead agent does none of this, so a plain `stop` waits
//! for ever. The forced stop does NOT end the key's span in the registry (only the agent's
//! marker and the owner's `/crew reject` do): the key stays valid. To revoke it, the owner runs
//! `/crew reject <name>`.
//!
//! The coordinator returns the tickets at once, writes `State = left`, and signs one notice to
//! the owner. The owner has no signing identity, so the owner's forced stop posts the owner
//! command `/crew stop <name>` in the registry issue (the log records the stop) and writes
//! `State = left` as the board mirror. The coordinator's next cycle returns the tickets of an
//! agent that the log stops. A board `State = left` alone returns nothing (spec/board#lifecycle).

use super::lifecycle::{open_tickets, rows_of};
use super::registry_post::state_event_key;
use super::tickets::{load_world_cached, me_of, reassign_derived, tag_earlier_work};
use super::{AgentCtx, Global, World, new_client, resolve_board_config};
use crate::clock::now_ts;
use crate::ctx::Ctx;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::protocol::names::{AgentName, Kind, Re, To};
use crate::registry::Departure;
use crate::rules::Role;
use crate::rules::lifecycle::{State, check_force_stop};

/// The target agent: approved, a dev agent, not ended by the registry log. Returns its item id,
/// its State and what the log says about it. The State checks are the caller's.
fn target<'a>(
    world: &'a World,
    name: &str,
    me: Option<&str>,
) -> CbResult<(&'a str, Option<State>, Option<Departure>)> {
    if me == Some(name) {
        return Err(CbError::refused(
            "stop-force-self",
            "an agent stops itself with a plain `crewboard stop`; --force is for another agent",
        ));
    }
    let entry = world.reg.agents.get(name).ok_or_else(|| {
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
    if world
        .trust
        .get(name)
        .is_some_and(|t| t.role == Role::Coordinator)
    {
        return Err(CbError::refused(
            "stop-force-not-dev",
            format!(
                "{name} is the coordinator: the owner hands the role over (`/crew role {name} dev`), then stops it"
            ),
        ));
    }
    let item_id = entry
        .join
        .as_ref()
        .map(|j| j.item_id.as_str())
        .ok_or_else(|| {
            CbError::refused(
                "no-join-request",
                format!("{name} has no agent item on the board"),
            )
        })?;
    if let Some(d) = entry.departure.filter(|d| d.ended()) {
        return Err(CbError::refused(
            "stop-force-already-left",
            format!("the registry log ended {name} already: it {}", d.reason()),
        ));
    }
    let state = world
        .items
        .get(item_id)
        .and_then(|i| i.field("State"))
        .and_then(State::parse);
    Ok((item_id, state, entry.departure))
}

/// Write `State left` and `Doing left` on the item of the agent.
fn mark_left(ctx: &Ctx, item_id: &str) -> CbResult<()> {
    ctx.board
        .set_select(&ctx.client, item_id, "State", State::Left.name())?;
    ctx.board.set_text(&ctx.client, item_id, "Doing", "left")
}

/// `stop --force --agent <name>`. A config dir without an agent config is the owner's, and so
/// is the token of the owner on a board with a crew account: the owner path. Any other call is
/// the coordinator's (a dev agent is refused there).
pub fn run(g: &Global, agent: &str) -> CbResult<String> {
    let name = AgentName::parse(agent)?;
    let cd = g.dir()?;
    let cfg = resolve_board_config(g)?;
    let client = new_client()?;
    let owner_token = cfg.crew_login.is_some()
        && client
            .viewer_login()?
            .eq_ignore_ascii_case(&cfg.owner_login);
    if owner_token || cd.agent_initiatives().is_empty() {
        super::require_owner(&client, &cfg, "may stop another agent by force")?;
        let ctx = Ctx::load(client, cfg, cd)?;
        return owner_stop(&ctx, &name);
    }
    drop((client, cfg));
    coordinator_stop(&AgentCtx::load(g)?, &name)
}

/// The owner's forced stop: `/crew stop <name>` in the registry issue (unless the log has the
/// owner's stop already), then `State = left` as the mirror. A State `left` without a stop in the
/// log (a hand edit, the coordinator's forced stop) is what this command is for, so it is not
/// refused; with the stop in the log and State `left` there is nothing left to do.
fn owner_stop(ctx: &Ctx, name: &AgentName) -> CbResult<String> {
    let world = crate::ctx::load_world(ctx, &[], &mut crate::state::StateCache::default())?;
    let (item_id, state, departure) = target(&world, name.as_str(), None)?;
    let logged = matches!(departure, Some(Departure::OwnerStop(_)));
    if state == Some(State::Left) && logged {
        return Err(CbError::refused(
            "stop-force-already-left",
            format!(
                "{name} is stopped already: the log has the owner's /crew stop and State is left"
            ),
        ));
    }
    if state != Some(State::Left) {
        check_force_stop(state)?;
    }
    let posted = if logged {
        "the registry log has the owner's /crew stop already\n".to_string()
    } else {
        let text = format!("/crew stop {name}");
        let (id, url) =
            feed::post_comment(&ctx.client, ctx.main_repo(), ctx.cfg.registry_issue, &text)?;
        format!(
            "owner command posted: {text} (comment {id}, {url}): the registry log records the stop\n"
        )
    };
    if state != Some(State::Left) {
        mark_left(ctx, item_id)?;
    }
    let out = format!(
        "{name} stopped by force (was {}): State left\n{posted}the coordinator returns the open tickets of {name} to Backlog at its next watch or poll cycle\nthe key of {name} stays valid: `/crew reject {name}` revokes it\n",
        state.map_or("?", State::name)
    );
    Ok(out)
}

fn coordinator_stop(ac: &AgentCtx, name: &AgentName) -> CbResult<String> {
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    if me.role != Role::Coordinator {
        return Err(CbError::refused(
            "stop-force-needs-coordinator",
            "stop --force is for the coordinator or the owner; a dev agent stops itself with `crewboard stop`",
        ));
    }
    let (item_id, state, _) = target(&world, name.as_str(), Some(ac.name.as_str()))?;
    check_force_stop(state)?;
    let rows = rows_of(ac, &world, name.as_str())?;
    let why =
        format!("agent {name} is stopping: the coordinator stopped it by force (it cannot drain)");
    let mut returned: Vec<String> = Vec::new();
    for r in open_tickets(&rows, name.as_str()) {
        let earlier = reassign_derived(ac, r, &why)?;
        returned.push(tag_earlier_work(&r.key(), earlier.as_ref()));
    }
    mark_left(&ac.ctx, item_id)?;
    let list = if returned.is_empty() {
        "none".to_string()
    } else {
        returned.join(", ")
    };
    crate::ops::say::send_signed(
        ac,
        ac.ctx.main_repo(),
        ac.ctx.cfg.registry_issue,
        To::Owner,
        Kind::Info,
        Re::None,
        &format!(
            "{name} was stopped by force by {}: State left. Tickets returned to Backlog: {list}. The registry log does not stop {name} (a coordinator cannot end the span of an agent): the key stays valid until `/crew reject {name}`, and `crewboard stop --agent {name} --force` records the stop in the log.\nevent={}",
            ac.name,
            state_event_key(name.as_str(), State::Left.name())
        ),
        &now_ts(),
    )?;
    Ok(format!(
        "{name} stopped by force (was {}): State left\ntickets returned to Backlog (assign them again): {list}\nthe key of {name} stays valid: the owner revokes it with `/crew reject {name}`\n",
        state.map_or("?", State::name)
    ))
}
