//! `approve` (spec/board#lifecycle, spec/board#coordinator-role): the owner approves an agent.
//!
//! Run with the owner's token. It posts the owner command `/crew approve ...` in the
//! registry issue (no crew header: an owner message, spec/messages#owner-messages), then sets
//! `State = approved` and `Role`, adds the agent name as an option of the `Agent` field
//! (complete-list merge, read-back check) and thereby freezes the key and the account: the
//! GitHub login that wrote the join (`login=`). A join of the owner's account is refused on a
//! board with a crew login (agents may not act as the owner).

use super::{Ctx, Global, World, load_world, new_client, resolve_board_config};
use crate::board::OptionInfo;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::owner_cmd::{approve_text, role_name};
use crate::protocol::names::AgentName;
use crate::registry::fingerprint;
use crate::rules::fields::{Field, Item, WriteCtx, check_field_write};
use crate::rules::lifecycle::{CrewMember, State, Who, check_lifecycle, check_single_coordinator};
use crate::rules::options::{
    AgentFacts, OPTION_LIMIT, OptionChange, OptionEntry, merge_options, plan_cap_prune,
};
use crate::rules::{Actor, Role};
use crate::state::StateCache;
use std::collections::{BTreeMap, BTreeSet};

/// The plan for the `Agent` option of one agent (spec/board#option-limit): it exists, it is
/// added, or it is added after the oldest unused options of retired agents are removed.
/// [`plan_agent_option`] reads and decides (a refusal writes nothing); [`apply_agent_option`]
/// writes.
pub struct AgentOptionPlan {
    name: String,
    exists: bool,
    current: Vec<OptionEntry>,
    prune: Vec<(OptionEntry, String)>,
    /// What the plan read and decided, for the output.
    pub notes: Vec<String>,
}

/// The result of [`ensure_agent_option`].
pub struct AgentOptionAdd {
    pub added: bool,
    /// One line per extra read and per removed option. Empty below the limit.
    pub lines: Vec<String>,
}

impl AgentOptionAdd {
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| format!("{l}\n")).collect()
    }
}

/// The facts about every agent of the registry: State and rejection from the registry and the
/// agent item, and the last sign of life (Heartbeat, else the time of the join comment). No
/// read: `world` holds all of it.
fn agent_facts(world: &World) -> BTreeMap<String, AgentFacts> {
    let mut out = BTreeMap::new();
    for e in world.reg.agents.values() {
        let item = e.join.as_ref().and_then(|j| world.items.get(&j.item_id));
        let joined = e.join.as_ref().and_then(|j| {
            world
                .registry_comments
                .iter()
                .find(|c| c.id == j.comment_id)
                .map(|c| c.created_epoch)
        });
        let heartbeat = item
            .and_then(|i| i.field("Heartbeat"))
            .and_then(crate::clock::parse_ts);
        out.insert(
            e.name.clone(),
            AgentFacts {
                state: item.and_then(|i| i.field("State")).and_then(State::parse),
                rejected: e.rejected,
                last_seen: heartbeat.or(joined),
            },
        );
    }
    out
}

/// Plan the `Agent` option of `name`. Reads the options again (another writer may have changed
/// them). Below the limit nothing else is read. When the add would exceed the limit, it reads
/// the `Agent` value of every listed item (one GraphQL read per 100 items) and plans the
/// prune; with too few free options it refuses with `agent-field-full`.
pub fn plan_agent_option(ctx: &mut Ctx, world: &World, name: &str) -> CbResult<AgentOptionPlan> {
    let fresh: Vec<OptionInfo> = ctx.board.read_options(&ctx.client, "Agent")?;
    if let Some(f) = ctx.board.fields.get_mut("Agent") {
        f.options = fresh;
    }
    let current = ctx.board.field("Agent")?.entries();
    let mut plan = AgentOptionPlan {
        name: name.to_string(),
        exists: current.iter().any(|o| o.name == name),
        current,
        prune: Vec::new(),
        notes: Vec::new(),
    };
    if plan.exists || plan.current.len() < OPTION_LIMIT {
        return Ok(plan);
    }
    let need = plan.current.len() + 1 - OPTION_LIMIT;
    let values = ctx.board.list_select_values(&ctx.client, "Agent")?;
    let mut used_ids = BTreeSet::new();
    let mut used_names = BTreeSet::new();
    for v in &values {
        used_ids.extend(v.option_id.clone());
        used_names.extend(v.name.clone());
    }
    plan.notes.push(format!(
        "the Agent field has {} options (limit {OPTION_LIMIT}): read the Agent value of {} listed item(s) ({} listing read(s)); {} item(s) carry a value",
        plan.current.len(),
        values.len(),
        values.len().div_ceil(100).max(1),
        values
            .iter()
            .filter(|v| v.option_id.is_some() || v.name.is_some())
            .count()
    ));
    let facts = agent_facts(world);
    let remove = plan_cap_prune(&plan.current, &facts, &used_ids, &used_names, need)?;
    for o in remove {
        let why = facts.get(&o.name).map_or_else(String::new, |f| {
            format!(
                "state {}{}, last seen {}",
                f.state
                    .map_or("unknown".to_string(), |s| format!("{s:?}").to_lowercase()),
                if f.rejected { ", rejected" } else { "" },
                f.last_seen
                    .map_or("unknown".to_string(), crate::clock::format_ts)
            )
        });
        plan.prune.push((o, why));
    }
    Ok(plan)
}

/// Write the plan through the one option writer: the complete list with every kept id, minus
/// the pruned options, plus the new one. Read back and checked (`option-id-lost`,
/// `option-not-removed`).
pub fn apply_agent_option(ctx: &mut Ctx, plan: AgentOptionPlan) -> CbResult<AgentOptionAdd> {
    let mut lines = plan.notes;
    if plan.exists {
        return Ok(AgentOptionAdd {
            added: false,
            lines,
        });
    }
    let removing: Vec<String> = plan.prune.iter().map(|(o, _)| o.id.clone()).collect();
    let mut desired = merge_options(
        &plan.current,
        &OptionChange::Add {
            name: plan.name.clone(),
        },
    )?;
    desired.retain(|d| d.id.as_ref().is_none_or(|id| !removing.contains(id)));
    if removing.is_empty() {
        ctx.board.write_options(&ctx.client, "Agent", &desired)?;
    } else {
        ctx.board
            .write_options_removing(&ctx.client, "Agent", &desired, &removing)?;
    }
    for (o, why) in &plan.prune {
        lines.push(format!(
            "pruned Agent option {} (id {}; {why}; no item uses it)",
            o.name, o.id
        ));
    }
    Ok(AgentOptionAdd { added: true, lines })
}

/// Make sure `name` is an option of the `Agent` field, through the one option writer. At the
/// limit it prunes first ([`plan_agent_option`]).
pub fn ensure_agent_option(ctx: &mut Ctx, world: &World, name: &str) -> CbResult<AgentOptionAdd> {
    let plan = plan_agent_option(ctx, world, name)?;
    apply_agent_option(ctx, plan)
}

/// What `approve` did for one agent: the text to print, and whether the board changed (an
/// approval, an option, a Role or a State). The CLI adds the next-step line only for a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub text: String,
    pub changed: bool,
}

pub fn run(g: &Global, name: &str, role: Role) -> CbResult<Approval> {
    let name = AgentName::parse(name)?;
    let cfg = resolve_board_config(g)?;
    let client = new_client()?;
    super::require_owner(&client, &cfg, "may approve")?;
    let mut ctx = Ctx::load(client, cfg, g.dir()?)?;
    let world = load_world(&ctx, &[], &mut StateCache::default())?;
    let entry = world.reg.agents.get(name.as_str()).ok_or_else(|| {
        CbError::refused(
            "no-join-request",
            format!("{name} has no valid join request in the registry issue"),
        )
    })?;
    if entry.rejected {
        return Err(CbError::refused(
            "agent-rejected",
            format!("{name} was rejected"),
        ));
    }
    let join = entry
        .join
        .clone()
        .ok_or_else(|| CbError::refused("no-join-request", format!("{name} has no join")))?;
    if entry.approval.is_none() && !entry.earlier_joins.is_empty() {
        return Err(ambiguous_join_refusal(&name, role, entry));
    }
    let item = world.items.get(&join.item_id).ok_or_else(|| {
        CbError::config(format!("agent item {} is not readable by id", join.item_id))
    })?;
    if let Some(why) = crate::registry::owner_account_refusal(&ctx.cfg, &join.login) {
        return Err(CbError::refused(
            "approve-owner-account",
            format!(
                "{name} cannot be approved: {why}. The agent joins again with the token of the crew account or of its own account"
            ),
        ));
    }
    let join_key = join
        .key()
        .ok_or_else(|| CbError::refused("bad-key", "the key of the join request is unreadable"))?;
    let fp = fingerprint(&join_key);
    // The Key field must be the key of the signed join (the agent wrote both).
    let item_key = item
        .field("Key")
        .and_then(|k| ssh_key::PublicKey::from_openssh(k).ok());
    if item_key.as_ref().map(|k| k.key_data()) != Some(join_key.key_data()) {
        return Err(CbError::refused(
            "key-mismatch",
            "the Key field of the agent item differs from the key in the signed join request",
        ));
    }
    let state = item.field("State").and_then(State::parse);
    // Idempotent: an approval that exists is not posted again, but the item is completed.
    let already = entry.approval.clone();
    if let Some(a) = &already
        && a.role != role
    {
        return Err(CbError::refused(
            "already-approved",
            format!(
                "{name} is approved as {}; change the role with `/crew role` (owner comment)",
                role_name(a.role)
            ),
        ));
    }
    if already.is_none() {
        check_lifecycle(state, State::Approved, Who::Owner)?;
        let crew: Vec<CrewMember> = world
            .reg
            .agents
            .values()
            .filter(|e| e.approved())
            .map(|e| CrewMember {
                name: e.name.clone(),
                role: e.approval.as_ref().map(|a| a.role),
                state: world
                    .items
                    .get(
                        &e.join
                            .as_ref()
                            .map(|j| j.item_id.clone())
                            .unwrap_or_default(),
                    )
                    .and_then(|i| i.field("State"))
                    .and_then(State::parse)
                    .unwrap_or(State::Approved),
            })
            .collect();
        check_single_coordinator(&crew, name.as_str(), role)?;
    }
    // The Agent option is planned before anything is written: a full field refuses here
    // (agent-field-full) and nothing is posted (spec/board#option-limit).
    let option_plan = plan_agent_option(&mut ctx, &world, name.as_str())?;
    // Nothing to do: approved, the option is there, the agent is active, the Role is set.
    let role_set = item
        .field("Role")
        .is_some_and(|r| r.eq_ignore_ascii_case(role_name(role)));
    if already.is_some() && option_plan.exists && state == Some(State::Active) && role_set {
        return Ok(Approval {
            text: format!("{name} is already approved and active; nothing to do\n"),
            changed: false,
        });
    }
    // 1. the owner command, as the owner (no header)
    let mut out = String::new();
    if already.is_none() {
        let (id, url) = feed::post_comment(
            &ctx.client,
            ctx.main_repo(),
            ctx.cfg.registry_issue,
            &approve_text(&name, role, &fp, &join.item_id, &join.login),
        )?;
        out.push_str(&format!("approval comment {id}: {url}\n"));
    } else {
        out.push_str("approval comment exists already\n");
    }
    // 2. the Agent option (merge with ids, prune at the limit, read back)
    let added = apply_agent_option(&mut ctx, option_plan)?;
    out.push_str(&added.text());
    out.push_str(if added.added {
        "Agent option added (existing option ids kept)\n"
    } else {
        "Agent option exists already\n"
    });
    // 3. State and Role on the agent item. Role needs the owner approval (just posted).
    let wc = WriteCtx {
        actor: Actor::Owner,
        is_subject: false,
        is_assigned: false,
        owner_approval: true,
        approved: false,
    };
    check_field_write(Item::Agent, Field::Role, &wc)?;
    ctx.board.set_select(
        &ctx.client,
        &join.item_id,
        "Role",
        match role {
            Role::Coordinator => "Coordinator",
            Role::Dev => "Dev",
        },
    )?;
    if state == Some(State::Requested) || state.is_none() {
        ctx.board
            .set_select(&ctx.client, &join.item_id, "State", "approved")?;
    }
    out.push_str(&format!(
        "{name} approved as {}; key {fp} and account {} are frozen; State=approved\n",
        role_name(role),
        join.login
    ));
    Ok(Approval {
        text: out,
        changed: true,
    })
}

/// Two undecided joins of one name with other keys: `approve` does not pick one. Any account
/// that may comment on the registry issue can post a join for a pending name
/// (spec/security-details#trust-roots), so the owner must name the key that the agent host
/// printed, and sees the account of each join.
fn ambiguous_join_refusal(
    name: &AgentName,
    role: Role,
    entry: &crate::registry::AgentEntry,
) -> CbError {
    let list: Vec<String> = entry
        .earlier_joins
        .iter()
        .chain(entry.join.iter())
        .map(|j| {
            format!(
                "key={} item={} login={}",
                j.key()
                    .map(|k| fingerprint(&k))
                    .unwrap_or_else(|| "unreadable".into()),
                j.item_id,
                j.login
            )
        })
        .collect();
    CbError::refused(
        "approve-ambiguous-join",
        format!(
            "{name} has {} undecided join requests with different keys ({}); compare the key with the one that `join` printed on the agent host, then comment on the registry issue: `/crew approve {name} role={} key=<fingerprint> item=<id> login=<login>`",
            list.len(),
            list.join("; "),
            role_name(role)
        ),
    )
}
