//! The contexts of a command: the options of the CLI (`Global`), the board client (`Ctx`), an
//! agent's own context (`AgentCtx`) and the registry world (`World`). The token checks are in
//! `token`.
//!
//! This module sits below the commands (`ops`), the wake flow and the unread helpers: they all
//! import it, and it imports none of them.

mod authority_file;
mod registry_view;
mod token;

use crate::agent_env::{AgentEnv, OwnerSource};
use crate::board::{Board, FieldValue, ItemRef};
use crate::comments::Comment;
use crate::config::{AgentConfig, BoardConfig, ConfigDir, load_key};
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::github::api::contents;
use crate::github::{Client, GhError, optional, resolve_owner_token, resolve_token};
use crate::items::ItemSnapshot;
use crate::protocol::names::AgentName;
use crate::registry::{Registry, Trust, build_trust, resolve};
use crate::rules::fields::{Field, WriteCtx};
use crate::rules::lifecycle::State;
use crate::services::Reader;
use crate::state::StateCache;
use ssh_key::PrivateKey;
use std::collections::HashMap;
use std::path::PathBuf;

pub use authority_file::{authority_text, load_authority, load_authority_quiet};
pub use registry_view::{
    agent_liveness, agent_since, is_coordinator, registry_late_msgs, registry_msgs,
};
pub use token::{
    IdentityMode, judge_agent, judge_agent_account, judge_owner, require_agent, require_owner,
};

/// The text of the authority file on the default branch of the main repo; `None` if it is not
/// there. A REST read by `owner/name`, so it follows a transfer or rename redirect.
pub fn fetch_authority_text(client: &Client, repo: &str, path: &str) -> CbResult<Option<String>> {
    let got = optional(contents::file(client, repo, path, None))?;
    Ok(got.map(|f| f.text.unwrap_or_default()))
}

/// Read AUTHORITY.md of the main repo once where the context loads. The text is kept in the
/// context ([`Ctx::authority_text_cached`]) and the former repo names (`former_repos`) come
/// from it; the signature checks get them as an argument (`BoardConfig::former_for`) and read
/// nothing. A read that fails, a missing file or a bad front matter give no former names (fail
/// closed: a message signed under a former name stays invalid) and one warning. A failed read
/// is not kept: a later [`authority_text`] reads again and reports its own error.
fn read_authority(
    client: &Client,
    cfg: &BoardConfig,
) -> (
    Vec<crate::protocol::former::FormerRepo>,
    Option<Option<String>>,
) {
    let read = fetch_authority_text(client, cfg.main_repo(), &cfg.authority);
    let former = read
        .as_ref()
        .map_err(|e| CbError::Network(GhError::Shape(e.to_string())))
        .and_then(|t| Ok(t.as_deref().map(crate::authority::parse).transpose()?));
    let (former, kept) = match (&read, former) {
        (Ok(t), Ok(Some(Some(a)))) => (a.former_repos, Some(t.clone())),
        (Ok(t), Ok(_)) => (Vec::new(), Some(t.clone())),
        (Ok(t), Err(e)) => {
            warn_former(&cfg.authority, &e);
            (Vec::new(), Some(t.clone()))
        }
        (Err(e), _) => {
            warn_former(&cfg.authority, e);
            (Vec::new(), None)
        }
    };
    (former, kept)
}

fn warn_former(authority: &str, e: &dyn std::fmt::Display) {
    eprintln!("warning: former repo names not read from {authority}: {e}");
}

/// Options shared by all commands.
#[derive(Debug, Clone, Default)]
pub struct Global {
    pub config_dir: Option<PathBuf>,
    pub initiative: Option<String>,
    pub initiative_file: Option<PathBuf>,
    /// `start` or `join` with `--insecure-one-account`: the agent's consent to a trial board
    /// (`insecure_one_account`), for this run. It never makes a crew board a trial.
    pub insecure_one_account: bool,
}

impl Global {
    pub fn dir(&self) -> CbResult<ConfigDir> {
        ConfigDir::resolve(self.config_dir.as_deref())
    }
}

/// A client, the board config and the board schema.
pub struct Ctx {
    pub client: Client,
    pub cfg: BoardConfig,
    pub board: Board,
    pub cd: ConfigDir,
    /// The heartbeat interval that judges every agent, in seconds (spec/board#fields):
    /// `heartbeat_interval` of the initiative file, else 10 minutes. Late, dead and the late scan
    /// use only this value, and so does the heartbeat that an agent writes.
    pub hb_interval: i64,
    /// The text of AUTHORITY.md (default branch of the main repo) that the context read when it
    /// loaded: `None` outer = not read (read failed, or forgotten); `Some(None)` = no such file.
    /// [`authority_text`] hands it out, so a command reads the file once.
    authority: std::sync::Mutex<Option<Option<String>>>,
}

impl Ctx {
    /// The kept text of AUTHORITY.md, if the context has it.
    pub(crate) fn authority_text_cached(&self) -> Option<Option<String>> {
        self.authority.lock().ok().and_then(|g| g.clone())
    }

    /// Keep `text` as the current AUTHORITY.md.
    pub(crate) fn keep_authority_text(&self, text: Option<String>) {
        if let Ok(mut g) = self.authority.lock() {
            *g = Some(text);
        }
    }

    /// Drop the kept text: the next read goes to GitHub. A loop (`watch`) calls it between
    /// cycles, so an edit of the owner reaches a running loop.
    pub fn forget_authority(&self) {
        if let Ok(mut g) = self.authority.lock() {
            *g = None;
        }
    }
}

/// The hidden test variable (for example `20s`): it sets the heartbeat interval of the board.
/// Not documented for users; the sandbox and component tests use it. Only a build with debug
/// assertions (the builds of `cargo test`) reads it; a release build ignores it.
pub const TEST_HEARTBEAT_ENV: &str = "CREWBOARD_TEST_HEARTBEAT_INTERVAL";

/// The value of [`TEST_HEARTBEAT_ENV`]; always `None` in a release build.
fn test_heartbeat_interval() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    crate::agent_env::var(TEST_HEARTBEAT_ENV)
}

pub fn new_client() -> CbResult<Client> {
    // a env file that did not load stops every command that talks to GitHub
    if let Some(e) = crate::agent_env::failure() {
        return Err(e.clone());
    }
    // an agent dir without agent.env must not fall back to the login of gh (the owner)
    if let Some(r) = crate::agent_env::check_token_source() {
        return Err(CbError::Refused(r));
    }
    let file_token = crate::agent_env::current().and_then(AgentEnv::token);
    Ok(Client::new(resolve_token(file_token)?)?)
}

/// The client of an owner command (spec/identity#author-and-signature): the token of
/// `--env-file`, else the owner's own `gh auth token`; `GH_TOKEN` and `GITHUB_TOKEN` are ignored,
/// and one note on stderr names a token variable that was set. In an agent dir without
/// `--env-file` the order of an agent command stays ([`new_client`]): there the login of `gh` is
/// the owner's, and an agent must not reach it through crewboard. The caller checks the login
/// against the board owner ([`require_owner`]).
pub fn new_owner_client() -> CbResult<Client> {
    if let Some(e) = crate::agent_env::failure() {
        return Err(e.clone());
    }
    let file_token = crate::agent_env::current().and_then(AgentEnv::explicit_token);
    match crate::agent_env::owner_source() {
        OwnerSource::AgentDir => new_client(),
        OwnerSource::GhLogin => {
            note_ignored_token_vars();
            Ok(Client::new(resolve_owner_token(file_token)?)?)
        }
        OwnerSource::EnvFile => Ok(Client::new(resolve_owner_token(file_token)?)?),
    }
}

/// The note of [`new_owner_client`], once per process (`approve a b` builds two clients).
fn note_ignored_token_vars() {
    static NOTED: std::sync::Once = std::sync::Once::new();
    NOTED.call_once(|| {
        if let Some(n) = crate::agent_env::owner_note(&crate::agent_env::caller_token_vars()) {
            eprintln!("{n}");
        }
    });
}

impl Ctx {
    pub fn load(client: Client, cfg: BoardConfig, cd: ConfigDir) -> CbResult<Ctx> {
        let hb_interval = crate::late::liveness::resolve_interval(
            cfg.heartbeat_interval.as_deref(),
            test_heartbeat_interval().as_deref(),
        )?;
        let board = Board::load(&client, &cfg.project_owner, cfg.project_number)?;
        let mut cfg = cfg;
        let (former, kept) = read_authority(&client, &cfg);
        cfg.former_repos = former;
        Ok(Ctx {
            authority: std::sync::Mutex::new(kept),
            client,
            cfg,
            board,
            cd,
            hb_interval,
        })
    }

    pub fn main_repo(&self) -> &str {
        self.cfg.main_repo()
    }

    /// The parts of this context that the ticket services read with.
    pub(crate) fn reader(&self) -> Reader<'_> {
        Reader {
            client: &self.client,
            cfg: &self.cfg,
            board: &self.board,
        }
    }
}

/// What the unattended coordinator may still post in this command
/// (spec/agents#unattended-coordinator): the counts of the UTC day when the command was let in.
/// `say::send_signed` writes the line `unattended: <n>/<budget>` from it and counts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The UTC day (`YYYY-MM-DD`) of the counts.
    pub day: String,
    /// The verdicts of the day so far.
    pub used: u32,
    /// `unattended_budget` of AUTHORITY.md.
    pub budget: u32,
    /// The stamped messages of the day so far.
    pub messages: u32,
    /// The stamped questions to the owner of the day so far.
    pub asks: u32,
    /// The reservations that no message used yet: a command reserves its messages before its
    /// first write (`ops::unattended::reserve_message`, `reserve_verdict`), and each message
    /// then takes the reservation of its class and reserves nothing.
    pub pending: Vec<StampClass>,
    /// For a verdict: what the command read about the unattended verdicts of its ticket. The
    /// floor of the ticket is noted from it before the post.
    pub ticket: Option<StampTicket>,
}

/// What a message of the unattended coordinator counts as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampClass {
    /// A verify result.
    Verdict,
    /// A question to the owner.
    Ask,
    /// Any other message.
    Other,
}

/// The unattended verdicts of one ticket as a command read them ([`Stamp::ticket`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StampTicket {
    /// The key of the ticket in the state (`ops::unattended::floor_key`).
    pub key: String,
    /// The unattended verdicts that the signed log of the ticket has.
    pub count: u32,
    /// The highest comment id that the command read on the ticket.
    pub after: u64,
    /// The highest comment id of a message that starts the count again (an owner comment, an
    /// assignment without the `unattended:` line); 0 without one. A floor of the state that was
    /// noted at a lower id ended with that message.
    pub release: u64,
}

/// The place of the [`Stamp`] in an [`AgentCtx`]. Empty for every command but a judgment of the
/// unattended coordinator: only `ops::unattended::admit` fills it, and only in a headless
/// config dir. No flag and no environment variable does.
#[derive(Debug, Default)]
pub struct StampCell(std::sync::Mutex<Option<Stamp>>);

impl StampCell {
    /// The stamp, when this command is a judgment of the unattended coordinator.
    pub(crate) fn get(&self) -> Option<Stamp> {
        self.0.lock().ok().and_then(|g| g.clone())
    }

    pub(crate) fn set(&self, s: Stamp) {
        if let Ok(mut g) = self.0.lock() {
            *g = Some(s);
        }
    }
}

/// An agent's own context: config, key, and the board.
pub struct AgentCtx {
    pub ctx: Ctx,
    pub agent: AgentConfig,
    pub name: AgentName,
    pub key: PrivateKey,
    /// `--insecure-one-account` of this run (`Global.insecure_one_account`).
    pub consent_flag: bool,
    /// The stamp of the unattended coordinator; empty for every other command.
    pub stamp: StampCell,
}

impl AgentCtx {
    pub fn load(g: &Global) -> CbResult<AgentCtx> {
        let cd = g.dir()?;
        let agent = cd.load_agent_auto(g.initiative.as_deref())?;
        let name = agent.agent_name()?;
        let key = load_key(&cd.key_path(&name))?;
        let client = new_client()?;
        require_agent(&client, &agent.board)?;
        let ctx = Ctx::load(client, agent.board.clone(), cd)?;
        Ok(AgentCtx {
            ctx,
            agent,
            name,
            key,
            consent_flag: g.insecure_one_account,
            stamp: StampCell::default(),
        })
    }

    pub fn state_path(&self) -> PathBuf {
        self.ctx.cd.state_path(&self.ctx.cfg.initiative, &self.name)
    }

    pub fn load_cache(&self) -> StateCache {
        StateCache::load(&self.state_path())
    }

    /// Save the cache; a failure is a warning. For caches that only hold what a run read
    /// (indexes, etags): a lost save repeats work at the next run.
    pub fn save_cache(&self, c: &mut StateCache) {
        // A lost save repeats work at the next run: say so, never stay silent.
        if let Err(e) = c.save(&self.state_path()) {
            eprintln!("warning: {e}");
        }
    }

    /// Write a field of this agent's own item: the agent itself is the writer
    /// (spec/board#fields). The field-writer table checks the write ([`Board::write`]).
    pub fn write_own(&self, field: Field, value: FieldValue<'_>) -> CbResult<()> {
        self.ctx.board.write(
            &self.ctx.client,
            ItemRef::agent(&self.agent.item_id),
            field,
            value,
            &WriteCtx::own_item(),
        )
    }

    /// Write the `State` of this agent's own item. The lifecycle check is the caller's.
    pub fn write_own_state(&self, to: State) -> CbResult<()> {
        self.ctx
            .board
            .write_state(&self.ctx.client, &self.agent.item_id, to)
    }

    /// Save the cache; a failure is an error (the message names the path). For `watch` (cursor,
    /// selftest nonces) and `tick` (throttle): a lost save would repeat events or work.
    pub fn save_cache_required(&self, c: &mut StateCache) -> crate::error::CbResult<()> {
        c.save(&self.state_path())
    }
}

/// The registry, the items it names, and who is trusted.
pub struct World {
    pub reg: Registry,
    pub trust: Trust,
    /// Items by id, for every id that the registry (and the caller) named.
    pub items: HashMap<String, ItemSnapshot>,
    /// Ids that were asked for but that GitHub did not return.
    pub unreadable: Vec<String>,
    pub registry_comments: Vec<Comment>,
}

/// Read the registry issue and every agent item by id
/// (spec/board#item-ids-are-the-source-of-truth), and build the trust table. `extra_ids` are read
/// as well (for example the agent's own item).
pub fn load_world(ctx: &Ctx, extra_ids: &[String], cache: &mut StateCache) -> CbResult<World> {
    let (comments, etag) = feed::issue_comments(
        &ctx.client,
        ctx.main_repo(),
        ctx.cfg.registry_issue,
        cache.poll.registry.as_ref(),
        &ctx.cfg.owner_login,
    )?;
    if let Some(e) = etag {
        cache.poll.registry = Some(crate::state::CachedComments {
            etag: e,
            comments: comments.clone(),
        });
    }
    let mut reg = resolve(&ctx.cfg, &comments);
    let mut ids: Vec<String> = reg
        .agents
        .values()
        .filter_map(|a| a.join.as_ref().map(|j| j.item_id.clone()))
        .chain(extra_ids.iter().cloned())
        .collect();
    ids.sort();
    ids.dedup();
    let read = ctx.board.read_items_tolerant(&ctx.client, &ids)?;
    let mut items = HashMap::new();
    let mut unreadable = Vec::new();
    for (id, it) in ids.iter().zip(read) {
        match it {
            Some(i) => {
                items.insert(id.clone(), i);
            }
            None => unreadable.push(id.clone()),
        }
    }
    for e in reg.agents.values() {
        let Some(join) = &e.join else { continue };
        if !unreadable.contains(&join.item_id) {
            continue;
        }
        if e.approved() && !e.departure.is_some_and(|d| d.ended()) {
            return Err(CbError::refused(
                crate::base::ids::AGENT_ITEM_MISSING,
                format!(
                    "{}: agent item {} is missing or inaccessible. Check Project access. If the item was deleted, the owner must post `/crew reject {}` in the registry issue {}#{} to retire this identity, then join and approve a new agent name. Retrying cannot restore a deleted item",
                    e.name,
                    join.item_id,
                    e.name,
                    ctx.main_repo(),
                    ctx.cfg.registry_issue
                ),
            ));
        }
        reg.notes.push(format!(
            "{}: agent item {} is missing or inaccessible; {}",
            e.name,
            join.item_id,
            e.departure.map_or("not approved", |d| d.reason())
        ));
    }
    let trust = build_trust(&reg, &items, ctx.cfg.project_number);
    Ok(World {
        reg,
        trust,
        items,
        unreadable,
        registry_comments: comments,
    })
}
