//! Reads of the registry world that more than one command needs: the verified registry
//! messages, the liveness of one agent and the coordinator test.

use super::{AgentCtx, World};
use crate::late::liveness::{Liveness, derive, last_seen};
use crate::rules::Actor;
use crate::ticket::{VMsg, verified_messages};

pub fn registry_msgs(ac: &AgentCtx, world: &World) -> Vec<VMsg> {
    verified_messages(&world.registry_comments, &ac.ctx.cfg, &world.trust)
}

/// The baseline of an agent: its last heartbeat, else the newer of its approval and its start.
/// `None`: no baseline, nothing is judged.
fn baseline(hb: Option<i64>, approved: Option<i64>, started: Option<i64>) -> Option<i64> {
    hb.or_else(|| approved.max(started))
}

/// [`baseline`] of one agent, from the data of the cycle: the `Heartbeat` and the last write
/// (`updatedAt`) of its item, and the creation time of its approval in the registry issue.
pub fn agent_since(world: &World, name: &str) -> Option<i64> {
    let e = world.reg.agents.get(name)?;
    let item = e.join.as_ref().and_then(|j| world.items.get(&j.item_id));
    let approved = e
        .approval
        .as_ref()
        .and_then(|ap| {
            world
                .registry_comments
                .iter()
                .find(|c| c.id == ap.comment_id)
        })
        .map(|c| c.created_epoch);
    let updated = item.and_then(|i| crate::clock::parse_ts(&i.updated_at));
    let heartbeat = item
        .and_then(|i| i.field("Heartbeat"))
        .and_then(crate::clock::parse_ts);
    baseline(
        heartbeat.and_then(|h| last_seen(Some(h), updated)),
        approved,
        updated,
    )
}

/// State, last sign of life and liveness of one agent, from the registry and the board (by id).
/// The last sign of life is [`agent_since`]: the heartbeat capped at the `updatedAt` of the item,
/// else the newer of the approval and the item's last write.
pub fn agent_liveness(
    world: &World,
    agent: &str,
    now: i64,
    interval: i64,
) -> (Option<String>, Option<i64>, Liveness) {
    let item = world
        .reg
        .agents
        .get(agent)
        .and_then(|e| e.join.as_ref())
        .and_then(|j| world.items.get(&j.item_id));
    let state = item.and_then(|i| i.field("State")).map(str::to_string);
    let since = agent_since(world, agent);
    let live = derive(state.as_deref(), since, now, interval);
    (state, since, live)
}

/// Does this agent hold the coordinator role?
pub fn is_coordinator(world: &World, me: &str) -> bool {
    world
        .trust
        .get(me)
        .is_some_and(|t| Actor::Agent(t.role).is_coordinator())
}

#[cfg(test)]
mod tests {
    use super::*;
    const T0: i64 = 1_792_000_000;

    #[test]
    fn the_baseline_is_the_heartbeat_else_the_newer_of_approval_and_start() {
        // a heartbeat beats an older or newer approval and start
        assert_eq!(baseline(Some(T0), Some(T0 - 9), Some(T0 - 9)), Some(T0));
        assert_eq!(baseline(Some(T0), Some(T0 + 9), Some(T0 + 9)), Some(T0));
        // no heartbeat: the newer of the two
        assert_eq!(baseline(None, Some(T0), Some(T0 - 9)), Some(T0));
        assert_eq!(baseline(None, Some(T0 - 9), Some(T0)), Some(T0));
        // only one of them
        assert_eq!(baseline(None, Some(T0), None), Some(T0));
        assert_eq!(baseline(None, None, Some(T0)), Some(T0));
        // nothing: no baseline
        assert_eq!(baseline(None, None, None), None);
    }
}
