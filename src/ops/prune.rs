//! `prune` (spec/board#fields, spec/cli#commands): the coordinator removes `Agent` options of
//! agents that have `left` and that no ticket uses.
//!
//! Removing an option clears that value on every item that has it, so the check
//! "no ticket uses it" has to be right. "Used" means any ticket on the board in any Status
//! (also `Done`): the tickets are found from the signed info comments of ALL `crew:work`
//! issues, closed ones too, from the local ticket index, and from the item listing (which
//! lags, so it is only an addition). The write goes through the one option writer: complete
//! list with ids, read back, every other id still there. After it, the `Agent` value of every
//! ticket that was read is compared with the value from before.

use super::AgentCtx;
use super::tickets::{load_world_cached, me_of};
use crate::error::{CbError, CbResult};
use crate::rules::fields::Field;
use crate::rules::lifecycle::State;
use crate::rules::options::{OptionChange, OptionEntry, merge_options, plan_prune};
use crate::services::ticket_index::{all_item_ids, read_items_tolerant};
use std::collections::BTreeMap;

pub fn run(ac: &mut AgentCtx) -> CbResult<String> {
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    if !me.actor.is_coordinator() {
        return Err(CbError::refused(
            "prune-coordinator-only",
            "prune is a coordinator command (the Agent options have one writer, docs/design/spec/board.md#fields)",
        ));
    }
    // The options as GitHub has them now.
    let fresh = ac.ctx.board.read_options(&ac.ctx.client, Field::Agent)?;
    ac.ctx.board.cache_options(Field::Agent, fresh.clone());
    let options: Vec<OptionEntry> = fresh
        .iter()
        .map(|o| OptionEntry {
            id: o.id.clone(),
            name: o.name.clone(),
        })
        .collect();
    // State of every agent that the registry knows.
    let mut states: BTreeMap<String, State> = BTreeMap::new();
    for e in world.reg.agents.values() {
        let st = e
            .join
            .as_ref()
            .and_then(|j| world.items.get(&j.item_id))
            .and_then(|i| i.get(Field::State))
            .and_then(State::parse);
        if let Some(st) = st {
            states.insert(e.name.clone(), st);
        }
    }
    // Every ticket that exists, by id (the agent items are not tickets).
    let cache = ac.load_cache();
    let mut ids = all_item_ids(&ac.ctx.reader(), &world.reg, &world.trust, &cache.index)?;
    let agent_items: Vec<&String> = world
        .reg
        .agents
        .values()
        .filter_map(|e| e.join.as_ref().map(|j| &j.item_id))
        .collect();
    ids.retain(|i| !agent_items.contains(&i));
    let before = agent_values(ac, &ids)?;
    let mut used: BTreeMap<String, usize> = BTreeMap::new();
    for v in before.values().flatten() {
        *used.entry(v.clone()).or_insert(0) += 1;
    }
    let plan = plan_prune(&options, &states, &used);
    let mut out = format!(
        "{} item(s) read by id to find the users of the Agent options\n",
        ids.len()
    );
    for (name, why) in &plan.kept {
        out.push_str(&format!("kept: {name}: {why}\n"));
    }
    if plan.remove.is_empty() {
        out.push_str("nothing to prune\n");
        return Ok(out);
    }
    // Complete list with ids, minus the options that are to go.
    let mut desired = merge_options(
        &options,
        &OptionChange::Remove {
            id: plan.remove[0].id.clone(),
        },
    )?;
    for r in &plan.remove[1..] {
        let current: Vec<OptionEntry> = desired
            .iter()
            .filter_map(|d| {
                d.id.as_ref().map(|id| OptionEntry {
                    id: id.clone(),
                    name: d.name.clone(),
                })
            })
            .collect();
        desired = merge_options(&current, &OptionChange::Remove { id: r.id.clone() })?;
    }
    let removing: Vec<String> = plan.remove.iter().map(|r| r.id.clone()).collect();
    ac.ctx
        .board
        .write_options_removing(&ac.ctx.client, Field::Agent, &desired, &removing)?;
    for r in &plan.remove {
        out.push_str(&format!("removed option: {} (id {})\n", r.name, r.id));
    }
    // The values of the tickets must be what they were.
    let after = agent_values(ac, &ids)?;
    let changed: Vec<String> = before
        .iter()
        .filter(|(id, v)| after.get(*id) != Some(*v))
        .map(|(id, v)| format!("{id}: {v:?} -> {:?}", after.get(id)))
        .collect();
    if !changed.is_empty() {
        return Err(CbError::refused(
            "prune-changed-ticket-values",
            format!(
                "the Agent value of {} ticket(s) changed during the prune: {}",
                changed.len(),
                changed.join("; ")
            ),
        ));
    }
    out.push_str(&format!(
        "checked: the Agent value of all {} item(s) read before is unchanged\n",
        before.len()
    ));
    Ok(out)
}

/// `Agent` value (or `None`) of each item, by id.
fn agent_values(ac: &AgentCtx, ids: &[String]) -> CbResult<BTreeMap<String, Option<String>>> {
    let items = read_items_tolerant(&ac.ctx.reader(), ids)?;
    Ok(items
        .into_iter()
        .map(|i| {
            let v = i.get(Field::Agent).map(str::to_string);
            (i.id, v)
        })
        .collect())
}
