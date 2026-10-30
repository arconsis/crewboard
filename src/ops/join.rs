//! `join` (spec/board#lifecycle, spec/board#item-ids-are-the-source-of-truth): create the key, the
//! agent item and the signed join request.

use super::agent::AgentRuntime;
use super::{Ctx, Global, World, load_world, new_client, say};
use crate::board::FieldValue;
use crate::board::ItemRef;
use crate::clock::{now_epoch, now_ts};
use crate::config::{
    AgentConfig, BoardConfig, ConfigDir, INITIATIVE_FILE, load_or_create_key, public_line,
    split_repo,
};
use crate::cursor::Cursor;
use crate::error::{CbError, CbResult};
use crate::github::Client;
use crate::protocol::names::{AgentName, Kind, Re, To};
use crate::registry::{fingerprint, join_body};
use crate::rules::fields::{Field, Item, WriteCtx, check_field_write};
use crate::rules::lifecycle::State;
use crate::state::StateCache;
use serde_json::Value;
use ssh_key::PrivateKey;

pub struct JoinArgs<'a> {
    pub name: &'a str,
    pub runtime: AgentRuntime,
    /// The model; the runtime label is `AgentRuntime::label` of both.
    pub model: Option<&'a str>,
    pub host: Option<&'a str>,
    /// Fetch `.crewboard.toml` from this repo's default branch.
    pub repo: Option<&'a str>,
    /// The tools whose versions are detected (spec/profiles#facts); empty: keep the config's list,
    /// or the default list.
    pub tools: &'a [String],
}

/// `.crewboard.toml` of a repo's default branch, parsed, with the blob SHA of the file.
pub struct FetchedBoard {
    pub cfg: BoardConfig,
    pub sha: String,
}

impl FetchedBoard {
    /// The first 7 characters of the blob SHA.
    pub fn short_sha(&self) -> &str {
        &self.sha[..self.sha.len().min(7)]
    }
}

/// Read `.crewboard.toml` from the default branch of a repo: one GET. The one function for
/// `join`, `start`, `resume` and `status`.
pub fn fetch_board_file(client: &Client, repo: &str) -> CbResult<FetchedBoard> {
    split_repo(repo)?;
    let r = client.get(&format!("repos/{repo}/contents/{INITIATIVE_FILE}"))?;
    let text = crate::ctx::decode_contents(&r.body)
        .map_err(|why| CbError::config(format!("{repo}/{INITIATIVE_FILE} {why}")))?;
    let sha = r
        .body
        .get("sha")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(FetchedBoard {
        cfg: BoardConfig::parse(&text)?,
        sha,
    })
}

/// The board config of the initiative: from `--initiative-file`, from the default branch of
/// `--repo`, or from the checkout. Returns the sha of the file when it came from a repo.
fn board_config_of(
    g: &Global,
    client: &Client,
    repo: Option<&str>,
) -> CbResult<(BoardConfig, Option<String>)> {
    if let Some(p) = &g.initiative_file {
        let text = std::fs::read_to_string(p)
            .map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
        Ok((BoardConfig::parse(&text)?, None))
    } else if let Some(r) = repo {
        let f = fetch_board_file(client, r)?;
        Ok((f.cfg, (!f.sha.is_empty()).then_some(f.sha)))
    } else if let Ok(t) = std::fs::read_to_string(INITIATIVE_FILE) {
        Ok((BoardConfig::parse(&t)?, None))
    } else {
        Err(CbError::config(format!(
            "no board config: pass --repo <owner/repo> (reads {INITIATIVE_FILE} from its default branch), --initiative-file, or run in a checkout that has {INITIATIVE_FILE}"
        )))
    }
}

/// One agent per config dir: the name is read from the file itself, so a config that does not
/// load still holds its agent.
fn check_one_agent_per_config(cd: &ConfigDir, cfg: &BoardConfig, name: &AgentName) -> CbResult<()> {
    match cd.agent_name_on_disk(&cfg.initiative) {
        Some(have) if have != name.as_str() => Err(CbError::refused(
            "one-agent-per-config",
            format!(
                "{} already holds agent {} for initiative {}; use another --config-dir for {name}",
                cd.0.display(),
                have,
                cfg.initiative
            ),
        )),
        _ => Ok(()),
    }
}

/// A name that exists with another key is taken (the key is frozen after approval).
fn check_name_free(world: &World, name: &AgentName, key: &PrivateKey) -> CbResult<()> {
    if let Some(entry) = world.reg.agents.get(name.as_str())
        && let Some(j) = &entry.join
        && j.key().map(|k| fingerprint(&k)) != Some(fingerprint(key.public_key()))
    {
        return Err(CbError::refused(
            "name-taken",
            format!("{name} is registered with another key; choose another name"),
        ));
    }
    Ok(())
}

/// The config of the agent. A re-join keeps the tool list, the `[loop]` and `[wake]` tables and
/// the consent that the config already has (unless `--tool` gives a new tool list); the consent
/// of this run is written only after the registration succeeded.
fn agent_config(
    a: &JoinArgs<'_>,
    runtime: &str,
    cfg: &BoardConfig,
    existing: Option<&AgentConfig>,
    item_id: String,
    host: String,
    board_sha: Option<String>,
) -> AgentConfig {
    AgentConfig {
        name: a.name.to_string(),
        runtime: runtime.to_string(),
        host,
        item_id,
        board_sha,
        board: cfg.clone(),
        tools: if a.tools.is_empty() {
            existing.and_then(|e| e.tools.clone())
        } else {
            Some(a.tools.to_vec())
        },
        loop_cfg: existing.and_then(|e| e.loop_cfg.clone()),
        wake: existing.and_then(|e| e.wake.clone()),
        insecure_one_account: existing.is_some_and(|e| e.insecure_one_account),
    }
}

/// The fields of the agent item. Each write is checked against the field-writer table first.
/// Kind and Key are written only before approval; after it the key is frozen and a second
/// `join` must not try to write it (the rule table would refuse it).
fn write_agent_fields(
    ctx: &Ctx,
    item_id: &str,
    approved: bool,
    key_line: &str,
    host: &str,
    runtime: &str,
) -> CbResult<()> {
    let w = WriteCtx {
        approved,
        ..WriteCtx::own_item()
    };
    // all checks before the first request: a refused field writes nothing
    let mut fields = vec![Field::Host, Field::Runtime, Field::Cursor];
    if !approved {
        fields.extend([Field::Kind, Field::Key]);
    }
    for f in fields {
        check_field_write(Item::Agent, f, &w)?;
    }
    let at = ItemRef::agent(item_id);
    let put = |field, value| ctx.board.write(&ctx.client, at, field, value, &w);
    let current = ctx.board.read_item(&ctx.client, item_id)?;
    if !approved {
        put(Field::Kind, FieldValue::Option("Agent"))?;
        if current.get(Field::State).is_none() {
            ctx.board
                .write_state(&ctx.client, item_id, State::Requested)?;
        }
        put(Field::Key, FieldValue::Text(key_line))?;
    }
    put(Field::Host, FieldValue::Text(host))?;
    put(Field::Runtime, FieldValue::Text(runtime))?;
    if current.get(Field::Cursor).is_none() {
        put(
            Field::Cursor,
            FieldValue::Text(&Cursor::at(now_epoch()).encode()),
        )?;
    }
    Ok(())
}

pub fn run(g: &Global, a: JoinArgs<'_>) -> CbResult<String> {
    let name = AgentName::parse(a.name)?;
    let runtime = a.runtime.label(a.model);
    let cd = g.dir()?;
    let client = new_client()?;
    let (cfg, board_sha) = board_config_of(g, &client, a.repo)?;
    let existing = cd.load_agent(&cfg.initiative).ok();
    // the consent of an existing config, or the flag of this run. Only the board key makes a
    // trial; a crew board (with or without crew_login) ignores the consent.
    let consent = existing.as_ref().is_some_and(|e| e.insecure_one_account)
        || (g.insecure_one_account && cfg.insecure_one_account);
    super::require_agent(&client, &cfg)?;
    check_one_agent_per_config(&cd, &cfg, &name)?;
    let (key, key_created) = load_or_create_key(&cd.key_path(&name))?;
    let key_line = public_line(&key, &name)?;
    // Without `--host`, the host of the agent item is the host part of the agent name. The name
    // is public in every signed comment, so the item names no other host (a machine name can be
    // private).
    let host = a
        .host
        .map_or_else(|| name.host().to_string(), str::to_string);
    let ctx = Ctx::load(client, cfg.clone(), cd.clone())?;
    let mut cache = StateCache::default();
    let world = load_world(
        &ctx,
        &existing
            .iter()
            .map(|e| e.item_id.clone())
            .collect::<Vec<_>>(),
        &mut cache,
    )?;
    check_name_free(&world, &name, &key)?;
    let registered = world.reg.agents.get(name.as_str());
    let approved = registered.is_some_and(|e| e.approved());

    // 1. the agent item (reuse the one in the config, else create it)
    let item_id = match &existing {
        Some(e) => e.item_id.clone(),
        None => ctx.board.add_draft(
            &ctx.client,
            &format!("agent: {name}"),
            &format!("crewboard agent item of {name}"),
        )?,
    };
    let agent = agent_config(
        &a,
        &runtime,
        &cfg,
        existing.as_ref(),
        item_id.clone(),
        host.clone(),
        board_sha,
    );
    // Save at once: a crash after this point must not create a second item.
    cd.save_agent(&agent)?;

    // 2. the fields
    write_agent_fields(&ctx, &item_id, approved, &key_line, &host, &runtime)?;

    // 2b. the facts of this host (spec/profiles#facts): never typed, detected here and at every
    // start
    let facts_out = {
        let item = ctx.board.read_item(&ctx.client, &item_id).ok();
        super::profile::write_facts(
            &ctx,
            name.as_str(),
            &item_id,
            item.as_ref(),
            agent.tools.as_deref(),
        )?
    };

    // 3. the signed join request, with the item id (spec/board#item-ids-are-the-source-of-truth).
    //    Not twice.
    let already = registered
        .and_then(|e| e.join.as_ref())
        .is_some_and(|j| j.item_id == item_id);
    let ac = super::AgentCtx {
        ctx,
        agent,
        name: name.clone(),
        key,
        consent_flag: g.insecure_one_account,
    };
    let posted = if already {
        None
    } else {
        Some(say::send_signed(
            &ac,
            ac.ctx.main_repo(),
            ac.ctx.cfg.registry_issue,
            To::Owner,
            Kind::Request,
            Re::None,
            &join_body(&item_id, &key_line, &runtime, &host),
            &now_ts(),
        )?)
    };
    // The registration is done: the consent of `--insecure-one-account` stays.
    if consent && cfg.insecure_one_account {
        cd.set_agent_one_account(&cfg.initiative)?;
    }
    let mut out = format!(
        "agent {name} ({})\nkey: {} ({})\nitem: {item_id}\nboard: project {} of {}\n{facts_out}",
        runtime,
        fingerprint(ac.key.public_key()),
        if key_created { "created" } else { "existing" },
        ac.ctx.cfg.project_number,
        ac.ctx.cfg.project_owner,
    );
    match posted {
        Some(p) => out.push_str(&format!(
            "join request posted: {}\nwaiting for the owner: crewboard approve {name} [--role dev|coordinator]\n",
            p.url
        )),
        None => out.push_str("join request already in the registry; nothing posted\n"),
    }
    Ok(out)
}
