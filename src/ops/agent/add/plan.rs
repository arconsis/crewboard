//! The plan of `agent add`: every value of the run, from the flags and the derived defaults.

use crate::agent_env;
use crate::config::{BoardConfig, split_repo};
use crate::error::CbError;
use crate::fsutil::find_on_path;
use crate::github::{Client, Token};
use crate::ops::agent::install_wake::{Installer, WakeSpec};
use crate::ops::agent::scheduler::AddScheduler;
use crate::ops::agent::shim::{scrub_tokens, valid_login};
use crate::ops::agent::wait::Wait;
use crate::ops::agent::{AddRole, AgentRuntime, absolute};
use crate::ops::join;
use crate::protocol::names::AgentName;
use std::path::{Path, PathBuf};
use std::process::Command;

impl AgentRuntime {
    fn all() -> [AgentRuntime; 3] {
        [
            AgentRuntime::Claude,
            AgentRuntime::Codex,
            AgentRuntime::Hermes,
        ]
    }
}

/// The options of `agent add` as the command line gave them.
#[derive(Debug, Default, Clone)]
pub struct AddArgs {
    pub target: Option<String>,
    pub board_file: Option<PathBuf>,
    pub repo: Option<String>,
    pub agent_dir: Option<PathBuf>,
    pub name: Option<String>,
    pub runtime: Option<AgentRuntime>,
    pub work_dir: Option<PathBuf>,
    /// The global `--env-file`.
    pub env_file: Option<PathBuf>,
    pub crew_login: Option<String>,
    pub model: Option<String>,
    pub initiative: Option<String>,
    pub role: Option<AddRole>,
    pub from: Option<PathBuf>,
    /// The global `--config-dir`: the agent dir when `--agent-dir` is not given.
    pub config_dir: Option<PathBuf>,
    /// The test seam `--crewboard BIN`.
    pub seam: Option<String>,
    /// `--wait-approval`: after the join, print the approve command for the owner and poll until
    /// the approval is there, then run `start`.
    pub wait_approval: bool,
    /// Test seams: seconds between two polls (default 15) and the longest wait (default 1800).
    pub wait_interval: Option<u64>,
    pub wait_max: Option<u64>,
    /// `--token-stdin`: read the agent token from standard input, check it and write the env file
    /// first (`agent token` for this host).
    pub token_stdin: bool,
    /// `--insecure-one-account`: a one-person trial; the token of `--token-stdin` may have the
    /// `workflow` scope (it is the owner's).
    pub insecure_one_account: bool,
    /// `--install-wake`: after a good `start`, install the scheduler unit of this dev agent
    /// (`wake install`, same defaults), with `--scheduler` and `--every` for its options.
    pub install_wake: bool,
    pub scheduler: Option<AddScheduler>,
    pub every: Option<String>,
    /// `--allow` (with `--install-wake`): extra tools for the wake, passed to `wake install`.
    pub allow: Vec<String>,
    /// What installs the unit (the command layer sets it: `wake install` in this process).
    pub installer: Option<Installer>,
}

/// Why no plan could be made. Nothing was written.
#[derive(Debug)]
pub(super) enum Early {
    /// A value that does not fit the host or the files: one line, exit 4.
    Msg(String),
    /// A missing or malformed argument: one line, exit 64 (usage).
    Usage(String),
    /// The env file does not load: the report of step 1.
    Env(String),
}

impl Early {
    /// The one line, without its exit code.
    fn text(self) -> String {
        match self {
            Early::Msg(m) | Early::Usage(m) | Early::Env(m) => m,
        }
    }
}

impl From<String> for Early {
    fn from(m: String) -> Self {
        Early::Msg(m)
    }
}

impl From<&str> for Early {
    fn from(m: &str) -> Self {
        Early::Msg(m.to_string())
    }
}

/// `--install-wake` with its options, checked before any write. A coordinator has no scheduler
/// unit: it runs in an open session, and its headless tick is a second `agent add`.
fn wake_spec(a: &AddArgs, coordinator: bool) -> Result<Option<(WakeSpec, Installer)>, String> {
    if !a.install_wake {
        return Ok(None);
    }
    if coordinator {
        return Err("--install-wake is for a dev agent: the interactive coordinator runs in an open session (its headless tick: agent add --role headless-coordinator, then crewboard wake install --tick)".into());
    }
    let installer = a
        .installer
        .ok_or("--install-wake: no installer given (a bug of the caller)")?;
    WakeSpec::new(a.scheduler, a.every.as_deref(), &a.allow).map(|w| Some((w, installer)))
}

/// What the positional argument names.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Target {
    Repo(String),
    File(PathBuf),
}

/// A path that exists and ends in `.toml` is a board file; anything else must be `owner/repo`.
/// A board file that is not there is a file problem (exit 4); a form that is neither is a usage
/// error (64).
pub(super) fn classify_target(s: &str, is_file: bool) -> Result<Target, Early> {
    if s.ends_with(".toml") {
        return if is_file {
            Ok(Target::File(PathBuf::from(s)))
        } else {
            Err(Early::Msg(format!("board file not found: {s}")))
        };
    }
    split_repo(s).map_err(|_| {
        Early::Usage(format!(
            "'{s}' is neither a board file (.toml) nor <owner>/<repo>"
        ))
    })?;
    Ok(Target::Repo(s.to_string()))
}

/// The short host name, as a part of an agent name: lower case, the first label, every character
/// outside `a-z0-9-` becomes `-`.
pub(super) fn short_host(raw: &str) -> String {
    let first = raw.trim().split('.').next().unwrap_or("");
    let s: String = first
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect();
    if s.is_empty() { "host".into() } else { s }
}

/// The output of `hostname` (without the token variables), for the default agent name.
fn hostname_output() -> String {
    let mut c = Command::new("hostname");
    scrub_tokens(&mut c);
    c.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// The agent name `<runtime>@<short host>`. It must pass `AgentName::parse`.
pub(super) fn default_name(rt: AgentRuntime, host: &str) -> Result<String, String> {
    let n = format!("{}@{}", rt.bin(), short_host(host));
    AgentName::parse(&n)
        .map_err(|e| format!("the default agent name '{n}' is not valid ({e}); pass --agent"))?;
    Ok(n)
}

/// The first of `claude`, `codex` and `hermes` that `path` finds.
pub(super) fn first_runtime(path: &std::ffi::OsStr) -> Option<AgentRuntime> {
    AgentRuntime::all()
        .into_iter()
        .find(|r| find_on_path(r.bin(), path).is_some())
}

pub(super) fn basename(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Does a git remote URL name this repo? Lower case, without a trailing `/` or `.git`.
pub(super) fn origin_matches(url: &str, repo: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    let u = u.trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u);
    let r = repo.to_ascii_lowercase();
    u.ends_with(&format!("/{r}")) || u.ends_with(&format!(":{r}"))
}

/// All values of one run.
#[derive(Debug)]
pub(super) struct Plan {
    pub coordinator: bool,
    pub target: Target,
    pub board_file: Option<PathBuf>,
    pub repo: String,
    pub initiative: String,
    pub runtime: AgentRuntime,
    pub name: String,
    pub agent_dir: PathBuf,
    pub work_dir: PathBuf,
    pub env_file: PathBuf,
    pub crew_login: Option<String>,
    pub model: Option<String>,
    pub seam: Option<String>,
    /// `--wait-approval`: how often to poll and for how long.
    pub wait: Option<Wait>,
    /// `--install-wake`, and what installs the unit.
    pub wake: Option<(WakeSpec, Installer)>,
}

/// The values that come from the host. A test passes its own.
pub(super) struct HostFacts {
    pub home: Option<PathBuf>,
    pub path: std::ffi::OsString,
    pub host: String,
}

/// The default paths. Pure.
pub(super) fn default_agent_dir(home: &Path, init: &str, rt: AgentRuntime) -> PathBuf {
    home.join(".config")
        .join("crewboard")
        .join("agents")
        .join(format!("{init}-{}", rt.bin()))
}
pub(super) fn default_work_dir(home: &Path, init: &str, agent_dir: &Path) -> PathBuf {
    home.join("crew")
        .join(format!("{init}-{}", basename(agent_dir)))
}
pub(in crate::ops::agent) fn default_env_file(home: &Path, init: &str) -> PathBuf {
    crate::ops::crew_token::env_file_under(home, init)
}

/// Resolve every value of the run, before any write. `cfg` is the board config that the board
/// file or the repo gave, when it was read. `env_default_exists` tells whether the default env
/// file is there. Pure.
pub(super) fn derive(
    a: &AddArgs,
    coordinator: bool,
    cfg: Option<&BoardConfig>,
    facts: &HostFacts,
    env_default_exists: &dyn Fn(&Path) -> bool,
) -> Result<Plan, String> {
    // `load_board` ran `target_of` first, so this call cannot fail in a run; the text is for
    // a direct call of `derive` (the tests).
    let target = target_of(a, &|p: &str| Path::new(p).is_file()).map_err(Early::text)?;
    if a.target.is_some() && a.repo.is_some() && matches!(target, Target::Repo(_)) {
        return Err("pass the repository once: as the argument or with --repo".into());
    }
    let (board_file, repo) = match (&target, a.board_file.as_deref()) {
        (Target::File(_), Some(_)) => {
            return Err("pass the board file once: as the argument or with --board-file".into());
        }
        (Target::File(f), None) => (Some(f.clone()), a.repo.clone()),
        (Target::Repo(r), bf) => (bf.map(Path::to_path_buf), Some(r.clone())),
    };
    let repo = repo
        .or_else(|| cfg.and_then(|c| c.repos.first().cloned()))
        .ok_or("no repository: pass <owner>/<repo>, or --repo with a board file")?;
    split_repo(&repo).map_err(|e| e.to_string())?;
    let initiative = match (cfg, &a.initiative) {
        (Some(c), Some(i)) if *i != c.initiative => {
            return Err(format!(
                "--initiative {i} does not match the board config, which is for {}",
                c.initiative
            ));
        }
        (Some(c), _) => Some(c.initiative.clone()),
        (None, Some(i)) => Some(i.clone()),
        (None, None) => None,
    };
    // the initiative names the default paths only; a run with every path given needs none
    let need_init = || {
        initiative.clone().ok_or_else(|| {
            "cannot tell the initiative for the default names: pass --initiative, or a board file, or the paths (--agent-dir, --work-dir, --env-file)"
                .to_string()
        })
    };
    let runtime = match a.runtime {
        Some(r) => r,
        None => first_runtime(&facts.path).ok_or(
            "no runtime found: none of claude, codex or hermes is on the PATH; install one, or pass --runtime",
        )?,
    };
    let name = match &a.name {
        Some(n) => {
            AgentName::parse(n).map_err(|e| format!("--agent: {e}"))?;
            n.clone()
        }
        None => default_name(runtime, &facts.host)?,
    };
    let need_home = || {
        facts.home.clone().ok_or_else(|| {
            "cannot find the home directory; pass the path of every default".to_string()
        })
    };
    let agent_dir = match a.agent_dir.as_deref().or(a.config_dir.as_deref()) {
        Some(d) => absolute(d),
        None => default_agent_dir(&need_home()?, &need_init()?, runtime),
    };
    let work_dir = match a.work_dir.as_deref() {
        Some(d) => absolute(d),
        None => default_work_dir(&need_home()?, &need_init()?, &agent_dir),
    };
    let env_file = match a.env_file.as_deref() {
        Some(f) => absolute(f),
        None => {
            let d = default_env_file(&need_home()?, &need_init()?);
            if !env_default_exists(&d) {
                return Err(format!(
                    "--env-file is required: {} does not exist (the default env file is used only when it exists)",
                    d.display()
                ));
            }
            d
        }
    };
    if let Some(l) = &a.crew_login
        && !valid_login(l)
    {
        return Err(format!("--crew-login: '{l}' is not a GitHub login"));
    }
    if let Some(m) = &a.model {
        crate::ops::agent::parse_model(m)?;
    }
    if a.from.is_some() {
        return Err("--from is for --role headless-coordinator".into());
    }
    let wake = wake_spec(a, coordinator)?;
    Ok(Plan {
        coordinator,
        target,
        board_file: board_file.map(|f| absolute(&f)),
        repo,
        initiative: initiative.unwrap_or_else(|| "<initiative>".to_string()),
        runtime,
        name,
        agent_dir,
        work_dir,
        env_file,
        crew_login: a.crew_login.clone(),
        model: a.model.clone(),
        seam: a.seam.clone(),
        wait: a
            .wait_approval
            .then(|| Wait::new(a.wait_interval, a.wait_max)),
        wake,
    })
}

/// What the argument and the options name: the positional first, else `--board-file`, else
/// `--repo`. None of them is a missing argument: a usage error.
fn target_of(a: &AddArgs, is_file: &dyn Fn(&str) -> bool) -> Result<Target, Early> {
    if let Some(t) = &a.target {
        return classify_target(t, is_file(t));
    }
    if let Some(f) = &a.board_file {
        return Ok(Target::File(f.clone()));
    }
    match &a.repo {
        Some(r) => classify_target(r, false),
        None => Err(Early::Usage(
            "a repository <owner>/<repo> or a board file is required".into(),
        )),
    }
}

/// Read the board config that the flags name, for the defaults. A board file is read here; a
/// repo is read only when nothing else gives the initiative (the token of the env file reads it).
fn load_board(a: &AddArgs) -> Result<Option<BoardConfig>, Early> {
    let target = target_of(a, &|p: &str| Path::new(p).is_file())?;
    let file = match (&target, &a.board_file) {
        (Target::File(f), _) => Some(f.clone()),
        (_, Some(f)) => Some(f.clone()),
        _ => None,
    };
    if let Some(f) = file {
        if !f.is_file() {
            return Err(format!("board file not found: {}", f.display()).into());
        }
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        return BoardConfig::parse(&text)
            .map(Some)
            .map_err(|e| Early::Msg(format!("{}: {e}", f.display())));
    }
    let all_paths = (a.agent_dir.is_some() || a.config_dir.is_some())
        && a.work_dir.is_some()
        && a.env_file.is_some();
    if a.initiative.is_some() || all_paths {
        return Ok(None);
    }
    let Target::Repo(repo) = target else {
        return Ok(None);
    };
    let Some(env) = &a.env_file else {
        return Err(format!(
            "cannot read the board config of {repo} without a token: pass --env-file <file>, or --initiative <name>, or a board file"
        )
        .into());
    };
    let env = agent_env::load(env.parent().unwrap_or(Path::new(".")), Some(&absolute(env)))
        .map_err(|e| Early::Env(strip_prefix(&e)))?;
    let token = env.token().ok_or("the env file has no token")?.to_string();
    let client = Client::new(Token::new(token)).map_err(|e| e.to_string())?;
    join::fetch_board_file(&client, &repo)
        .map(|f| Some(f.cfg))
        .map_err(|e| {
            Early::Msg(format!(
                "cannot read the board config of {repo}: {}; pass --initiative, or a board file",
                strip_prefix(&e)
            ))
        })
}

pub(in crate::ops::agent) fn strip_prefix(e: &CbError) -> String {
    match e {
        CbError::Config(m) | CbError::Usage(m) => m.clone(),
        other => other.to_string(),
    }
}

/// The plan of this run: the board config, the host facts, the derived defaults.
pub(super) fn plan(a: &AddArgs, coordinator: bool) -> Result<Plan, Early> {
    let cfg = load_board(a)?;
    let facts = HostFacts {
        home: std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from),
        path: std::env::var_os("PATH").unwrap_or_default(),
        host: hostname_output(),
    };
    derive(a, coordinator, cfg.as_ref(), &facts, &|p: &Path| {
        p.is_file()
    })
    .map_err(Early::Msg)
}

/// The other binary of the test seam, when there is one.
pub(super) fn seam_bin(p: &Plan) -> Option<&str> {
    crate::ops::lock_cmd::test_seam(p.seam.as_deref())
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
