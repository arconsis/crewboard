//! `agent profile set`, `agent profile show` and the fact detection of `join` and `start` (spec/profiles).
//!
//! The `Capabilities` field of the agent item holds the facts and the declared tags; the body
//! of the agent's draft item holds the background text and the full facts (with versions).
//! A change is written in this order: pure checks, the signed record (agent: a signed
//! message in the registry issue; owner: an owner comment), then the field, then the body. A
//! failure after the record leaves a record without its effect, never an effect without a
//! record, and the same command run again completes it.

use super::say::{require_approved, send_signed};
use super::tickets::load_world_cached;
use super::{AgentCtx, Ctx, Global, World, load_world, new_client, resolve_board_config};
use crate::capabilities::facts::{self, RealProbe};
use crate::capabilities::{Applied, Body, Caps, Change, apply_change, tier_rank};
use crate::clock::now_ts;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::items::ItemSnapshot;
use crate::owner_cmd;
use crate::profile;
use crate::protocol::names::{AgentName, Kind, Re, To};
use crate::rules::fields::{Field, Item, WriteCtx, check_field_write};
use crate::rules::{Actor, Role};
use serde_json::json;

/// The longest background text that `agent profile set` accepts. The text is for the other agents
/// and the owner; a longer one belongs in a document that the text points to.
pub const MAX_ABOUT_CHARS: usize = 4000;

pub const FIELD: &str = "Capabilities";

/// The raw arguments of `agent profile set`.
#[derive(Debug, Clone, Default)]
pub struct SetArgs {
    pub tags: Vec<String>,
    pub untags: Vec<String>,
    pub tier: Option<String>,
    pub clear_tier: bool,
    pub about_file: Option<String>,
}

/// The checks that need no config and no network: tag syntax, the background text (read,
/// size, secret scan), and that something is to be changed.
pub fn prepare(a: &SetArgs) -> CbResult<Change> {
    let about = match a.about_file.as_deref() {
        None => None,
        Some(f) => {
            let text = super::say::prepare_body(Some(f), None)?;
            if text.chars().count() > MAX_ABOUT_CHARS {
                return Err(CbError::refused(
                    "profile-about-too-long",
                    format!(
                        "the background text has {} characters; the limit is {MAX_ABOUT_CHARS}. Write a short text and point to a document",
                        text.chars().count()
                    ),
                ));
            }
            Some(text.trim().to_string())
        }
    };
    let c = Change {
        tags: Change::parse_tags(&a.tags)?,
        untags: Change::parse_tags(&a.untags)?,
        tier: a.tier.clone(),
        clear_tier: a.clear_tier,
        about,
    };
    if c.is_empty() {
        return Err(CbError::config(
            "nothing to change: give --tag, --untag, --tier, --clear-tier or --about-file",
        ));
    }
    Ok(c)
}

/// Who may write the `Capabilities` field of which agent
/// (spec/profiles#who-may-change-a-profile-and-the-history): the agent itself, or the owner. A dev
/// agent and the coordinator cannot write the profile of another agent.
pub fn judge_target(actor: Actor, target: &str, me: Option<&str>) -> CbResult<()> {
    let ctx = WriteCtx {
        actor,
        is_subject: me == Some(target),
        is_assigned: false,
        owner_approval: false,
        approved: true,
    };
    check_field_write(Item::Agent, Field::Capabilities, &ctx)?;
    Ok(())
}

fn board_has_field(ctx: &Ctx) -> bool {
    ctx.board.fields.contains_key(FIELD)
}

fn missing_field_error(ctx: &Ctx) -> CbError {
    CbError::config(format!(
        "the board has no field '{FIELD}'; the owner runs `crewboard init --initiative {} --repo {} --project {}` once (it is idempotent)",
        ctx.cfg.initiative,
        ctx.main_repo(),
        ctx.cfg.project_number
    ))
}

pub fn caps_of(item: Option<&ItemSnapshot>) -> Caps {
    Caps::parse(item.and_then(|i| i.field(FIELD)).unwrap_or(""))
}

/// Detect the facts of this host, write them to the `Capabilities` field and the body of the
/// agent item. The declared tags, the tier and the background text stay as they are. Returns
/// the lines for the user. A board without the field gets a warning, not a failure: the agent
/// can still work, and the owner adds the field with `init`.
pub fn write_facts(
    ctx: &Ctx,
    name: &str,
    item_id: &str,
    item: Option<&ItemSnapshot>,
    tools: Option<&[String]>,
) -> CbResult<String> {
    if !board_has_field(ctx) {
        return Ok(format!(
            "warning: the board has no field '{FIELD}', so the facts of this host are not written. The owner runs `crewboard init --initiative {} --repo {} --project {}` (idempotent)\n",
            ctx.cfg.initiative,
            ctx.main_repo(),
            ctx.cfg.project_number
        ));
    }
    let tool_names = facts::tool_list(tools, &ctx.cfg.detect_tools)?;
    let found = facts::detect(&RealProbe, &tool_names);
    let old = caps_of(item);
    let new = old.clone().with_facts(found.tokens());
    let composed = new.compose()?;
    let me = WriteCtx::agent_self(Role::Dev);
    check_field_write(Item::Agent, Field::Capabilities, &me)?;
    let mut out = format!(
        "facts: {} ({} of {} tools)\n",
        found
            .tokens()
            .iter()
            .filter(|t| !t.starts_with("tool:"))
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        found.tools.len(),
        tool_names.len()
    );
    if item.and_then(|i| i.field(FIELD)) == Some(composed.as_str()) {
        out.push_str("capabilities: unchanged\n");
    } else {
        ctx.board.set_text(&ctx.client, item_id, FIELD, &composed)?;
        out.push_str(&format!("capabilities: {composed}\n"));
    }
    // the body: keep the background text, replace the facts
    if let Some(d) = ctx.board.read_draft(&ctx.client, item_id)? {
        let b = Body::parse(&d.body);
        let text = Body::render(name, &new, &b.about, &found.markdown());
        if text != d.body {
            ctx.board.set_draft_body(&ctx.client, &d.id, &text)?;
            out.push_str("agent item body: facts written\n");
        }
    }
    Ok(out)
}

/// The fact token that marks an agent whose toolchain check failed at `start`
/// (spec/agents#toolchain-check-of-start). `write_facts` replaces every fact, so the next `start`
/// that passes removes it.
pub const TOOLCHAIN_FLAG: &str = "toolchain:unreachable";

/// Write or remove [`TOOLCHAIN_FLAG`] in the `Capabilities` field of the agent item. The agent
/// writes its own field, as for the facts. Returns a line for the user. A board without the
/// field gets a warning, not a failure.
pub fn set_toolchain_flag(
    ctx: &Ctx,
    item_id: &str,
    item: Option<&ItemSnapshot>,
    failing: bool,
) -> CbResult<String> {
    if !board_has_field(ctx) {
        return Ok(format!(
            "warning: the board has no field '{FIELD}', so `status` cannot show the toolchain note\n"
        ));
    }
    let mut caps = caps_of(item);
    caps.facts.retain(|t| t != TOOLCHAIN_FLAG);
    if failing {
        caps.facts.push(TOOLCHAIN_FLAG.to_string());
    }
    let composed = caps.compose()?;
    let me = WriteCtx::agent_self(Role::Dev);
    check_field_write(Item::Agent, Field::Capabilities, &me)?;
    if item.and_then(|i| i.field(FIELD)) == Some(composed.as_str()) {
        return Ok(String::new());
    }
    ctx.board.set_text(&ctx.client, item_id, FIELD, &composed)?;
    Ok(format!("capabilities: {composed}\n"))
}

/// The notice target of an agent's own record, as in the lifecycle notices: a dev agent
/// writes to the coordinator, the coordinator to the owner.
fn notice_target(world: &World, me: &str) -> To {
    if world
        .trust
        .get(me)
        .is_some_and(|t| t.role == Role::Coordinator)
    {
        To::Owner
    } else {
        To::Coordinator
    }
}

fn describe(a: &Applied) -> String {
    let mut s = String::new();
    if !a.added.is_empty() {
        s.push_str(&format!("  added: {}\n", a.added.join(", ")));
    }
    if !a.removed.is_empty() {
        s.push_str(&format!("  removed: {}\n", a.removed.join(", ")));
    }
    if a.tier_changed {
        s.push_str(&format!(
            "  tier: {}\n",
            a.caps.tier.as_deref().unwrap_or("none")
        ));
    }
    if a.about_changed {
        s.push_str("  background text: changed\n");
    }
    s
}

/// Compute the change against the stored profile, or `None` if there is nothing to do.
fn plan(
    ctx: &Ctx,
    item: &ItemSnapshot,
    change: &Change,
) -> CbResult<(Applied, Body, Option<crate::board::Draft>)> {
    if !board_has_field(ctx) {
        return Err(missing_field_error(ctx));
    }
    let draft = ctx.board.read_draft(&ctx.client, &item.id)?;
    let body = draft
        .as_ref()
        .map(|d| Body::parse(&d.body))
        .unwrap_or_default();
    let applied = apply_change(&caps_of(Some(item)), &body.about, change)?;
    // the field must fit before anything is posted
    applied.caps.compose()?;
    Ok((applied, body, draft))
}

fn apply(
    ctx: &Ctx,
    name: &str,
    item: &ItemSnapshot,
    applied: &Applied,
    body: &Body,
    draft: Option<&crate::board::Draft>,
    new_about: Option<&str>,
) -> CbResult<String> {
    let composed = applied.caps.compose()?;
    let mut out = String::new();
    if item.field(FIELD) != Some(composed.as_str()) {
        ctx.board
            .set_text(&ctx.client, &item.id, FIELD, &composed)?;
        out.push_str(&format!("capabilities: {composed}\n"));
    }
    if let Some(d) = draft {
        let about = new_about.unwrap_or(&body.about);
        let text = Body::render(name, &applied.caps, about, &body.facts_md);
        if text != d.body {
            ctx.board.set_draft_body(&ctx.client, &d.id, &text)?;
            out.push_str("agent item body: profile written\n");
        }
    } else {
        out.push_str("note: the agent item is not a draft issue, so the body was not written\n");
    }
    Ok(out)
}

/// `agent profile set` by the agent itself.
pub fn set_own(ac: &AgentCtx, change: &Change) -> CbResult<String> {
    let me = ac.name.to_string();
    let world = load_world_cached(ac)?;
    let trusted = require_approved(&world, &me)?;
    judge_target(Actor::Agent(trusted.role), &me, Some(&me))?;
    let item = world.items.get(&ac.agent.item_id).ok_or_else(|| {
        CbError::config(format!("agent item {} is not readable", ac.agent.item_id))
    })?;
    let (applied, body, draft) = plan(&ac.ctx, item, change)?;
    if !applied.changed() {
        return Ok(format!(
            "no change: the profile of {me} already is as asked\ntags: {}\n",
            tags_line(&applied.caps)
        ));
    }
    let to = notice_target(&world, &me);
    let posted = send_signed(
        ac,
        ac.ctx.main_repo(),
        ac.ctx.cfg.registry_issue,
        to,
        Kind::Info,
        Re::None,
        &profile::change_body(&applied),
        &now_ts(),
    )?;
    let mut out = format!(
        "profile change of {me} recorded (signed, comment {}):\n{}",
        posted.id,
        describe(&applied)
    );
    out.push_str(&apply(
        &ac.ctx,
        &me,
        item,
        &applied,
        &body,
        draft.as_ref(),
        change.about.as_deref(),
    )?);
    out.push_str(&format!("tags: {}\n", tags_line(&applied.caps)));
    Ok(out)
}

/// `agent profile set --agent <name>` by the owner (owner token).
pub fn set_agent(g: &Global, agent: &str, change: &Change) -> CbResult<String> {
    let name = AgentName::parse(agent)?;
    let cfg = resolve_board_config(g)?;
    let client = super::new_owner_client()?;
    super::require_owner(&client, &cfg, "may set the profile of another agent")?;
    judge_target(Actor::Owner, name.as_str(), None)?;
    let ctx = Ctx::load(client, cfg, g.dir()?)?;
    let world = load_world(&ctx, &[], &mut crate::state::StateCache::default())?;
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
    let join = entry
        .join
        .as_ref()
        .ok_or_else(|| CbError::refused("no-join-request", format!("{name} has no join")))?;
    let item = world.items.get(&join.item_id).ok_or_else(|| {
        CbError::config(format!("agent item {} is not readable by id", join.item_id))
    })?;
    let (applied, body, draft) = plan(&ctx, item, change)?;
    if !applied.changed() {
        return Ok(format!(
            "no change: the profile of {name} already is as asked\ntags: {}\n",
            tags_line(&applied.caps)
        ));
    }
    let text = owner_cmd::profile_text(&name, change);
    crate::ops::say::check_body_text(&text)?;
    let (id, url) =
        feed::post_comment(&ctx.client, ctx.main_repo(), ctx.cfg.registry_issue, &text)?;
    let mut out = format!(
        "owner profile change of {name} recorded (comment {id}, {url}):\n{}",
        describe(&applied)
    );
    out.push_str(&apply(
        &ctx,
        name.as_str(),
        item,
        &applied,
        &body,
        draft.as_ref(),
        change.about.as_deref(),
    )?);
    out.push_str(&format!("tags: {}\n", tags_line(&applied.caps)));
    Ok(out)
}

fn tags_line(c: &Caps) -> String {
    let mut v: Vec<String> = c.declared.iter().cloned().collect();
    if let Some(t) = &c.tier {
        v.push(format!("tier:{t}"));
    }
    if v.is_empty() {
        "(none)".to_string()
    } else {
        v.join(" ")
    }
}

/// `agent profile show [--agent <name>]`: the stored profile, the facts, and the signed history.
pub fn show(g: &Global, agent: Option<&str>, json_out: bool) -> CbResult<String> {
    let (ctx, mut cache, own_name, extra) = match AgentCtx::load(g) {
        Ok(ac) => {
            let c = ac.load_cache();
            let extra = vec![ac.agent.item_id.clone()];
            let n = ac.name.to_string();
            (ac.ctx, c, Some(n), extra)
        }
        Err(_) => {
            let cfg = resolve_board_config(g)?;
            (
                Ctx::load(new_client()?, cfg, g.dir()?)?,
                crate::state::StateCache::default(),
                None,
                vec![],
            )
        }
    };
    let target = match (agent, own_name) {
        (Some(a), _) => AgentName::parse(a)?.to_string(),
        (None, Some(n)) => n,
        (None, None) => {
            return Err(CbError::config(
                "no agent config here: pass --agent <name> to show the profile of an agent",
            ));
        }
    };
    let world = load_world(&ctx, &extra, &mut cache)?;
    let entry = world.reg.agents.get(&target).ok_or_else(|| {
        CbError::refused(
            "no-join-request",
            format!("{target} has no valid join request in the registry issue"),
        )
    })?;
    let item = entry
        .join
        .as_ref()
        .and_then(|j| world.items.get(&j.item_id))
        .ok_or_else(|| CbError::config(format!("the item of {target} is not readable by id")))?;
    let caps = caps_of(Some(item));
    let draft = ctx.board.read_draft(&ctx.client, &item.id)?;
    let body = draft
        .as_ref()
        .map(|d| Body::parse(&d.body))
        .unwrap_or_default();
    let hist = profile::history(&world.registry_comments, &ctx.cfg, &world.trust, &target);
    if json_out {
        return Ok(json!({
            "agent": target,
            "tags": caps.declared,
            "tier": caps.tier,
            "facts": caps.facts,
            "tools_cut": caps.tools_cut,
            "unknown_tokens": caps.unknown,
            "capabilities": item.field(FIELD),
            "about": body.about,
            "history": hist.iter().map(|h| json!({"comment": h.comment_id, "by": h.by, "change": h.summary})).collect::<Vec<_>>(),
        })
        .to_string()
            + "\n");
    }
    let mut s = format!("profile of {target}\n");
    s.push_str(&format!("tags: {}\n", tags_line(&caps)));
    s.push_str(&format!(
        "tier: {} (rank {})\n",
        caps.tier.as_deref().unwrap_or("none"),
        tier_rank(caps.tier.as_deref()).0
    ));
    s.push_str(&format!(
        "facts: {}{}\n",
        if caps.facts.is_empty() {
            "(none detected yet)".to_string()
        } else {
            caps.facts.join(" ")
        },
        if caps.tools_cut > 0 {
            format!(" [tool list cut: {} not shown]", caps.tools_cut)
        } else {
            String::new()
        }
    ));
    if !caps.unknown.is_empty() {
        s.push_str(&format!(
            "unreadable tokens in the field (dropped at the next write): {}\n",
            caps.unknown.join(" ")
        ));
    }
    s.push_str(&format!(
        "background: {}\n",
        if body.about.is_empty() {
            "(none)".to_string()
        } else {
            body.about.clone()
        }
    ));
    s.push_str(&format!("changes ({}):\n", hist.len()));
    for h in hist.iter().rev().take(10) {
        s.push_str(&format!(
            "  comment {} by {}: {}\n",
            h.comment_id, h.by, h.summary
        ));
    }
    Ok(s)
}
