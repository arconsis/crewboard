//! `crewboard agent add`: set up one agent in one command, and make a rerun safe.
//!
//! Seven steps: the agent dir, `join`, the `gh` shim, the crew id, the work dir, the git identity
//! and `start`. Each step says `done` or `already done`. A failure names the step, shows the last
//! lines of its output, says what to do and names the log. The log is `<agent dir>/bootstrap.log`;
//! no token goes into it. The end is the one next step: for a start, the command to paste on its own line.
//!
//! The steps that talk to GitHub run in this process. With the hidden option `--crewboard BIN`
//! (a test seam) `join`, `start` and the crew id lookup go through another binary instead. `start` always runs as a child of a crewboard binary: it
//! checks the working directory and the PATH of the agent, so it needs its own process.

use super::install_wake::{self, Installer, WakeSpec};
use super::scheduler::AddScheduler;
use super::shim::{
    Identity, find_real_gh, helper_value, is_checkout, is_current_stub_for, make_empty_gh_config,
    noreply_email, read_crew_id, scrub_tokens, sq, stub_text, valid_login, write_stub,
};
use super::wait::{Wait, note_interrupt, poll_until};
use super::{AddRole, AgentRuntime, absolute, exec_env_plan};
use crate::agent_env;
use crate::base::ignore::{best_effort_cleanup, child_may_have_ended};
use crate::clock::SystemClock;
use crate::config::{BoardConfig, split_repo};
use crate::error::CbError;
use crate::fsutil::{find_on_path, set_mode};
use crate::github::{Client, Token};
use crate::ops::{Global, join};
use crate::protocol::names::AgentName;
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The prefix of every line.
pub(super) const PREFIX: &str = "agent add:";
const TOTAL: usize = 7;
/// How long one step command may run. A clone is the slow one.
const STEP_LIMIT: Duration = Duration::from_secs(180);
const CLONE_LIMIT: Duration = Duration::from_secs(900);
/// A value that is shorter than this is never treated as a token (it would garble the text).
const MIN_SECRET: usize = 8;

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
    /// `--token-stdin`: read the crew token from standard input, check it and write the env file
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
    /// A bad value or a missing one: one line.
    Msg(String),
    /// The env file does not load: the report of step 1.
    Env(String),
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

/// The token values of the env file of the flags, for the texts before step 1. None when the file
/// does not load.
fn early_secrets(a: &AddArgs) -> Vec<String> {
    a.env_file
        .as_deref()
        .map(absolute)
        .and_then(|f| agent_env::load(f.parent().unwrap_or(Path::new(".")), Some(&f)).ok())
        .map(|e| e.token_values())
        .unwrap_or_default()
}

/// What to do about an env file that does not load. `m` is the message of the loader.
pub(super) fn env_fix(m: &str, env: &Path) -> String {
    if m.contains("not found:") {
        format!(
            "store the token of the crew account: crewboard agent token --initiative <initiative> (it writes {} with mode 600), or run this command with --token-stdin. Trial mode (one account): use the env file that holds the owner's token, the one you gave to `crewboard init --insecure-one-account`; do not run `agent token`",
            env.display()
        )
    } else if m.contains(" has mode ") {
        format!("chmod 600 {}", env.display())
    } else {
        format!(
            "check the env file {}: one line GH_TOKEN=<token of the crew account>, mode 600, your own file",
            env.display()
        )
    }
}

/// Run `agent add` for a dev or a coordinator (`agent::run_add` sends the headless coordinator to
/// `coordinator`). It prints its own lines. The result is the exit code. With `--token-stdin` the
/// token is stored first and the steps then use that env file.
pub(super) fn run_add_steps(a: &AddArgs) -> u8 {
    if a.token_stdin {
        return match store_token(a) {
            Ok(env) => run_add_plan(&AddArgs {
                env_file: Some(env),
                token_stdin: false,
                ..a.clone()
            }),
            Err(code) => code,
        };
    }
    run_add_plan(a)
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

/// The refusal of `--install-wake` for the headless coordinator. The exit code.
pub(super) fn refuse_install_wake_headless() -> u8 {
    eprintln!(
        "{PREFIX} --install-wake does not go with --role headless-coordinator (install its tick with: crewboard wake install --tick --agent-dir <dir>)"
    );
    4
}

/// The refusal of `--token-stdin` for the headless coordinator, which uses the env file of the
/// interactive one. The exit code.
pub(super) fn refuse_token_stdin_headless() -> u8 {
    eprintln!(
        "{PREFIX} --token-stdin does not go with --role headless-coordinator (it uses the env file of the interactive coordinator)"
    );
    4
}

/// `--token-stdin`: read the token, check it and write the env file that the run then uses (the
/// global `--env-file`, else the default file of the initiative). The exit code of a failure.
fn store_token(a: &AddArgs) -> Result<PathBuf, u8> {
    let fail = |m: String| {
        eprintln!("{PREFIX} {m}");
        4
    };
    let env = match (&a.env_file, &a.initiative) {
        (Some(f), _) => absolute(f),
        (None, Some(i)) => {
            crate::ops::crew_token::default_env_file(i).map_err(|e| fail(strip_prefix(&e)))?
        }
        (None, None) => {
            return Err(fail(
                "--token-stdin needs --initiative <name> (for the env file ~/.config/crewboard/<name>.env) or --env-file <file>".into(),
            ));
        }
    };
    let report = crate::ops::crew_token::read_token().and_then(|t| {
        crate::ops::crew_token::store(&t, &env, a.crew_login.as_deref(), a.insecure_one_account)
    });
    match report {
        Ok(text) => {
            for l in text.lines() {
                println!("{PREFIX} {}", l.trim_start_matches("crew-token: "));
            }
            Ok(env)
        }
        Err(e) => {
            eprintln!("{PREFIX} {}", strip_prefix(&e));
            Err(e.exit().code())
        }
    }
}

fn run_add_plan(a: &AddArgs) -> u8 {
    let role = a.role.unwrap_or(AddRole::Dev);
    match plan(a, role == AddRole::Coordinator) {
        Ok(p) => run_steps(&p),
        Err(Early::Msg(m)) => {
            eprintln!("{PREFIX} {}", redact(&m, &early_secrets(a)));
            4
        }
        Err(Early::Env(m)) => {
            // the message of the loader holds no value, and is cleaned all the same
            let m = redact(&m, &early_secrets(a));
            let env = a.env_file.as_deref().map(absolute).unwrap_or_default();
            let f = Fail {
                step: 1,
                name: "agent dir",
                what: m.clone(),
                fix: env_fix(&m, &env),
                code: 4,
            };
            for l in fail_lines(&f, TOTAL, "", None) {
                eprintln!("{l}");
            }
            f.code
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the plan: every value of the run, from the flags and the derived defaults

/// What the positional argument names.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Target {
    Repo(String),
    File(PathBuf),
}

/// A path that exists and ends in `.toml` is a board file; anything else must be `owner/repo`.
pub(super) fn classify_target(s: &str, is_file: bool) -> Result<Target, String> {
    if s.ends_with(".toml") {
        return if is_file {
            Ok(Target::File(PathBuf::from(s)))
        } else {
            Err(format!("board file not found: {s}"))
        };
    }
    split_repo(s)
        .map_err(|_| format!("'{s}' is neither a board file (.toml) nor <owner>/<repo>"))?;
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

fn basename(p: &Path) -> String {
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
pub(super) fn default_env_file(home: &Path, init: &str) -> PathBuf {
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
    let target = target_of(a, &|p: &str| Path::new(p).is_file())?;
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
        super::parse_model(m)?;
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

/// What the argument and the options name: the positional first, else `--board-file`, else `--repo`.
fn target_of(a: &AddArgs, is_file: &dyn Fn(&str) -> bool) -> Result<Target, String> {
    if let Some(t) = &a.target {
        return classify_target(t, is_file(t));
    }
    if let Some(f) = &a.board_file {
        return Ok(Target::File(f.clone()));
    }
    match &a.repo {
        Some(r) => classify_target(r, false),
        None => Err("a repository <owner>/<repo> or a board file is required".into()),
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

pub(super) fn strip_prefix(e: &CbError) -> String {
    match e {
        CbError::Config(m) => m.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// output: the lines, the failure shape, the log

/// A failed step.
pub(super) struct Fail {
    pub step: usize,
    pub name: &'static str,
    pub what: String,
    pub fix: String,
    pub code: u8,
}

/// The lines of a failure, for standard error. Pure. `output` is the output of the step: its last
/// five lines are shown.
pub(super) fn fail_lines(f: &Fail, total: usize, output: &str, log: Option<&Path>) -> Vec<String> {
    let mut v = vec![format!(
        "{PREFIX} [{}/{total}] {}: FAIL: {}",
        f.step, f.name, f.what
    )];
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    for l in &lines[lines.len().saturating_sub(5)..] {
        v.push(format!("{PREFIX}   | {l}"));
    }
    v.push(format!("{PREFIX} fix: {}", f.fix));
    if let Some(l) = log {
        v.push(format!(
            "{PREFIX} the full output of each step is in {}",
            l.display()
        ));
    }
    v
}

/// Replace every occurrence of the secret. A secret shorter than [`MIN_SECRET`] is not replaced.
pub(super) fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= MIN_SECRET) {
        out = out.replace(s.as_str(), "***");
    }
    out
}

/// The state of one run: the step count, the secret to keep out of every text, the log.
pub(super) struct Run {
    pub secrets: Vec<String>,
    pub log: Option<PathBuf>,
    /// The output of the step that ran last.
    pub last: String,
}

impl Run {
    fn new() -> Self {
        Run {
            secrets: Vec::new(),
            log: None,
            last: String::new(),
        }
    }

    /// Keep the output of a step for the failure text, and write it into the log. The token is
    /// replaced first, so it reaches neither.
    pub(super) fn record(&mut self, title: &str, output: &str) {
        self.last = redact(output, &self.secrets);
        let Some(log) = &self.log else { return };
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log)
            .and_then(|mut f| {
                writeln!(
                    f,
                    "== {} {}\n{}",
                    crate::clock::now_ts(),
                    redact(title, &self.secrets),
                    self.last.trim_end()
                )
            });
        if let Err(e) = r {
            eprintln!(
                "{PREFIX} warning: cannot write the log {}: {e}",
                log.display()
            );
        }
    }

    /// The lines of a failure. The token is replaced in the reason and in the fix too.
    fn fail_text(&self, f: &Fail) -> Vec<String> {
        let clean = Fail {
            step: f.step,
            name: f.name,
            what: redact(&f.what, &self.secrets),
            fix: redact(&f.fix, &self.secrets),
            code: f.code,
        };
        fail_lines(&clean, TOTAL, &self.last, self.log.as_deref())
    }

    fn fail(&self, f: Fail) -> u8 {
        for l in self.fail_text(&f) {
            eprintln!("{l}");
        }
        f.code
    }
}

/// The result of a step that went well: it did the work, or the work was done already.
pub(super) struct Ok_ {
    already: bool,
    text: String,
}

fn say(step: usize, name: &str, r: &Ok_) {
    println!(
        "{PREFIX} [{step}/{TOTAL}] {name}: {} ({})",
        if r.already { "already done" } else { "done" },
        r.text
    );
}

// ---------------------------------------------------------------------------------------------
// running a command with a deadline

/// What a command printed (standard output, then standard error) and how it ended.
pub(super) struct Captured {
    pub text: String,
    /// The exit code; -1 when a signal ended it or it ran too long.
    pub code: i32,
    pub timed_out: bool,
}

/// Run a command; stop it after `limit`. Nothing here waits without a deadline: a child that
/// hangs is killed, and a pipe that a grandchild keeps open is given up after a few seconds.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
pub(super) fn run_captured(mut cmd: Command, limit: Duration) -> Captured {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match crate::spawn::spawn_retrying(&mut cmd) {
        Ok(c) => c,
        Err(e) => {
            let code = if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            };
            return Captured {
                text: format!("cannot start {}: {e}", cmd.get_program().to_string_lossy()),
                code,
                timed_out: false,
            };
        }
    };
    // A reader keeps what it has read in a shared buffer, so that a pipe that never closes (a
    // grandchild holds it) still gives the text up to that point.
    type Shared = (
        std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        mpsc::Receiver<()>,
    );
    let pipe = |r: Option<Box<dyn Read + Send>>| -> Shared {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let shared = buf.clone();
        std::thread::spawn(move || {
            if let Some(mut r) = r {
                let mut chunk = [0u8; 4096];
                while let Ok(n) = r.read(&mut chunk) {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut b) = shared.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            tx.send(()).ok(); // the receiver may be gone
        });
        (buf, rx)
    };
    let out = pipe(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = pipe(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let (code, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(st)) => break (st.code().unwrap_or(-1), false),
            Ok(None) if start.elapsed() >= limit => {
                child_may_have_ended(child.kill());
                child_may_have_ended(child.wait());
                break (-1, true);
            }
            Ok(None) => crate::clock::pause(Duration::from_millis(10)),
            Err(_) => {
                child_may_have_ended(child.kill());
                child_may_have_ended(child.wait());
                break (-1, false);
            }
        }
    };
    let grab = |(buf, done): Shared| {
        done.recv_timeout(Duration::from_secs(3)).ok(); // a timeout means the reader is still busy: take what it has
        buf.lock().map(|b| b.clone()).unwrap_or_default()
    };
    let mut text = String::from_utf8_lossy(&grab(out)).to_string();
    text.push_str(&String::from_utf8_lossy(&grab(err)));
    if timed_out {
        text.push_str(&format!(
            "\ntimed out after {} s and stopped",
            limit.as_secs()
        ));
    }
    Captured {
        text,
        code,
        timed_out,
    }
}

// ---------------------------------------------------------------------------------------------
// next-step text

/// The commands that this run printed or will print, for the owner to copy. Quoted for a shell.
fn agent_add_again(p: &Plan) -> String {
    let mut s = String::from("crewboard agent add ");
    match &p.target {
        Target::Repo(r) => s.push_str(r),
        Target::File(f) => {
            s.push_str(&sq(&f.display().to_string()));
            s.push_str(&format!(" --repo {}", p.repo));
        }
    }
    if let (Target::Repo(_), Some(f)) = (&p.target, &p.board_file) {
        s.push_str(&format!(" --board-file {}", sq(&f.display().to_string())));
    }
    s.push_str(&format!(
        " --agent-dir {} --agent {} --runtime {} --work-dir {} --env-file {}",
        sq(&p.agent_dir.display().to_string()),
        p.name,
        p.runtime.bin(),
        sq(&p.work_dir.display().to_string()),
        sq(&p.env_file.display().to_string()),
    ));
    if let Some(m) = &p.model {
        s.push_str(&format!(" --model {}", sq(m)));
    }
    if let Some(l) = &p.crew_login {
        s.push_str(&format!(" --crew-login {l}"));
    }
    if p.coordinator {
        s.push_str(" --role coordinator");
    }
    if let Some((w, _)) = &p.wake {
        s.push_str(&w.flags());
    }
    s
}

/// The line while the owner has not approved the agent.
pub(super) fn approve_line(p: &Plan, initiative: &str) -> String {
    let owner = if p.coordinator {
        format!(
            "crewboard approve {} --initiative {initiative} --role coordinator",
            p.name
        )
    } else {
        format!(
            "crewboard approve {} --initiative {initiative}; add --role coordinator for the coordinator",
            p.name
        )
    };
    format!(
        "next step: ask the owner to approve {} (owner: {owner}), then run: {}",
        p.name,
        agent_add_again(p)
    )
}

pub(super) fn scheduler_for(rt: AgentRuntime) -> &'static str {
    if rt == AgentRuntime::Hermes {
        "hermes"
    } else if cfg!(target_os = "macos") {
        "launchd"
    } else {
        "systemd"
    }
}

/// The first prompt of an interactive session of this agent, as one shell word. Claude Code and
/// Codex take it as the argument; Hermes runs from its cron job and gets none.
pub(super) fn first_prompt(p: &Plan, initiative: &str) -> Option<String> {
    if p.runtime == AgentRuntime::Hermes {
        return None;
    }
    let role = if p.coordinator {
        format!("the coordinator of {initiative}")
    } else {
        format!("a dev agent of {initiative}")
    };
    Some(sq(&format!(
        "Use the crewboard skill. You are {}, {role}. Run `crewboard resume`. Then start the watch command that it prints under Monitor (persistent).",
        p.name
    )))
}

/// The runtime command of the paste line: the executable and its first prompt.
fn runtime_words(p: &Plan, initiative: &str) -> String {
    match first_prompt(p, initiative) {
        Some(prompt) => format!("{} {prompt}", p.runtime.bin()),
        None => p.runtime.bin().to_string(),
    }
}

/// The lines after a good `start`. The first one introduces the command; the second is the bare
/// command to paste (no prefix, nothing after it); the third is an optional hint.
pub(super) fn after_start_lines(p: &Plan, initiative: &str) -> [String; 3] {
    let dir = sq(&p.agent_dir.display().to_string());
    let wd = sq(&p.work_dir.display().to_string());
    let env = sq(&p.env_file.display().to_string());
    let exec = format!("cd {wd} && crewboard agent exec --agent-dir {dir} --");
    let command = format!("{exec} {}", runtime_words(p, initiative));
    let hint = if p.coordinator {
        let head = format!("{}-headless", p.agent_dir.display());
        format!(
            "optional: the headless tick of this coordinator: crewboard agent add --role headless-coordinator --from {dir} --agent-dir {} --env-file {env}",
            sq(&head),
        )
    } else {
        format!(
            "optional: an unattended agent: crewboard wake install --scheduler {} --runtime {} --agent-dir {dir} --initiative {initiative} --env-file {env} --work-dir {wd} --shim-dir {}",
            scheduler_for(p.runtime),
            p.runtime.bin(),
            sq(&p.agent_dir.join("bin").display().to_string()),
        )
    };
    [
        "next step: start the runtime in the work dir:".to_string(),
        command,
        hint,
    ]
}

// ---------------------------------------------------------------------------------------------
// the seven steps

/// The plan of this run: the board config, the host facts, the derived defaults.
fn plan(a: &AddArgs, coordinator: bool) -> Result<Plan, Early> {
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
fn seam_bin(p: &Plan) -> Option<&str> {
    p.seam
        .as_deref()
        .filter(|b| !crate::ops::lock_cmd::is_this_binary(b))
}

/// The binary that runs `start` (and `join` with the seam): the seam, else this one.
fn self_bin(p: &Plan) -> Result<PathBuf, Fail> {
    match seam_bin(p) {
        Some(b) => Ok(PathBuf::from(b)),
        None => std::env::current_exe().map_err(|e| Fail {
            step: 7,
            name: "start",
            what: format!("cannot find the crewboard binary: {e}"),
            fix: "run crewboard by its full path".into(),
            code: 4,
        }),
    }
}

fn run_steps(p: &Plan) -> u8 {
    let mut run = Run::new();

    match step_dir(p, &mut run) {
        Ok(r) => say(1, "agent dir", &r),
        Err(f) => return run.fail(f),
    }
    if seam_bin(p).is_none() {
        // from here the token of the crew account is the token of this process
        if let Err(e) = agent_env::init(&p.agent_dir, None) {
            eprintln!(
                "{PREFIX} cannot load the env file of the agent: {}",
                redact(&strip_prefix(e), &run.secrets)
            );
            return e.exit().code();
        }
    }

    let (conf, joined_now) = match step_join(p, &mut run) {
        Ok((r, conf, now)) => {
            say(2, "join", &r);
            (conf, now)
        }
        Err(f) => return run.fail(f),
    };
    let initiative = conf
        .as_deref()
        .and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| p.initiative.clone());

    match step_shim(p, &mut run) {
        Ok(r) => say(3, "gh shim", &r),
        Err(f) => return run.fail(f),
    }
    let shim = p.agent_dir.join("bin").join("gh");

    let (login, id) = match step_crew_id(p, &mut run, &shim) {
        Ok((r, l, i)) => {
            say(4, "crew id", &r);
            (l, i)
        }
        Err(f) => return run.fail(f),
    };
    let email = noreply_email(&login, &id);

    match step_work_dir(p, &mut run, &shim, &login) {
        Ok(r) => say(5, "work dir", &r),
        Err(f) => return run.fail(f),
    }
    match step_git(p, &mut run, &shim, &login, &email) {
        Ok(r) => say(6, "git config", &r),
        Err(f) => return run.fail(f),
    }

    let not_yet = || {
        println!(
            "{PREFIX} [7/{TOTAL}] start: not yet: the owner has not approved {}",
            p.name
        );
        println!("{PREFIX} {}", approve_line(p, &initiative));
        0
    };
    let done = || {
        println!("{PREFIX} [7/{TOTAL}] start: done ({} is active)", p.name);
        finish(p, &initiative)
    };
    if joined_now {
        return match &p.wait {
            Some(w) => match wait_for_approval(p, &mut run, &initiative, w) {
                Start::Done => done(),
                Start::NotApproved => not_yet(),
                Start::Failed(f) => run.fail(f),
            },
            None => not_yet(),
        };
    }
    match step_start(p, &mut run) {
        Start::Done => done(),
        Start::NotApproved => match &p.wait {
            Some(w) => match wait_for_approval(p, &mut run, &initiative, w) {
                Start::Done => done(),
                Start::NotApproved => not_yet(),
                Start::Failed(f) => run.fail(f),
            },
            None => not_yet(),
        },
        Start::Failed(f) => run.fail(f),
    }
}

/// The end of a good run: with `--install-wake` the scheduler unit is installed and its lines
/// come first, then the one next step; else the command to paste on its own line. The exit code.
fn finish(p: &Plan, initiative: &str) -> u8 {
    let Some((spec, installer)) = &p.wake else {
        let [intro, command, hint] = after_start_lines(p, initiative);
        println!("{PREFIX} {intro}");
        println!("{command}");
        println!("{PREFIX} {hint}");
        return 0;
    };
    let target = install_wake::Target {
        agent_dir: &p.agent_dir,
        initiative,
        name: &p.name,
        runtime: p.runtime.bin(),
        work_dir: &p.work_dir,
        env_file: &p.env_file,
    };
    match install_wake::install(*installer, spec, &target) {
        Ok(done) => {
            for l in &done.status {
                println!("{PREFIX} wake: {l}");
            }
            if let Some(n) = &done.next {
                println!("{PREFIX} wake: next: {n}");
            }
            println!("{PREFIX} {}", after_wake_line(p, done.next.as_deref()));
            0
        }
        Err(e) => {
            // the agent is set up and started; only the unit is missing, and a rerun does just that
            for l in strip_prefix(&e).lines() {
                eprintln!("{PREFIX} wake install: {l}");
            }
            eprintln!(
                "{PREFIX} the agent is set up and started ({} is active); only the scheduler unit is missing. Fix the cause and run this command again: every step says already done",
                p.name
            );
            e.exit().code()
        }
    }
}

/// The last line after the unit is installed. Pure.
pub(super) fn after_wake_line(p: &Plan, next: Option<&str>) -> String {
    match next {
        Some(cmd) => format!(
            "next step: {cmd} (it keeps the loop of {} running after logout)",
            p.name
        ),
        None => format!(
            "next step: none; the scheduler wakes {} when work exists (check: crewboard wake status --agent-dir {})",
            p.name,
            sq(&p.agent_dir.display().to_string())
        ),
    }
}

/// Step 1: the agent dir (mode 700) and the link `agent.env` to the env file. The env file is
/// checked by the loader of the agent env, the one place that knows the rules.
fn step_dir(p: &Plan, run: &mut Run) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String| Fail {
        step: 1,
        name: "agent dir",
        what,
        fix,
        code: 4,
    };
    let env = &p.env_file;
    let loaded = agent_env::load(&p.agent_dir, Some(env)).map_err(|e| {
        let m = strip_prefix(&e);
        let fix = env_fix(&m, env);
        fail(m, fix)
    })?;
    run.secrets = loaded.token_values();

    let dir = &p.agent_dir;
    let link = dir.join("agent.env");
    if dir.is_dir() && is_linked(dir, env) {
        set_mode(dir, 0o700).map_err(|e| {
            fail(
                format!("cannot set the mode of {}: {e}", dir.display()),
                format!("chmod 700 {}", dir.display()),
            )
        })?;
        run.log = Some(dir.join("bootstrap.log"));
        return Ok(Ok_ {
            already: true,
            text: dir.display().to_string(),
        });
    }
    make_agent_dir(dir, env).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            fail(
                format!("{} exists and is not a link", link.display()),
                format!(
                    "move {} away (it may hold a token), then run this command again",
                    link.display()
                ),
            )
        } else {
            fail(
                format!(
                    "cannot make {} or link {}: {e}",
                    dir.display(),
                    link.display()
                ),
                format!(
                    "check the permissions of {}",
                    dir.parent().unwrap_or(dir).display()
                ),
            )
        }
    })?;
    run.log = Some(dir.join("bootstrap.log"));
    Ok(Ok_ {
        already: false,
        text: format!("{} -> {}", link.display(), env.display()),
    })
}

/// Does `<dir>/agent.env` link to the env file?
pub(super) fn is_linked(dir: &Path, env: &Path) -> bool {
    std::fs::read_link(dir.join("agent.env")).is_ok_and(|t| t == env)
}

/// Make the agent dir (mode 700) and link `agent.env` to the env file: a temp link in the dir,
/// renamed over the old link, so the link is never missing. A regular file at `agent.env` is not
/// replaced (it may be the only copy of a token): that is `AlreadyExists`.
pub(super) fn make_agent_dir(dir: &Path, env: &Path) -> std::io::Result<()> {
    let link = dir.join("agent.env");
    if let Ok(m) = link.symlink_metadata()
        && !m.file_type().is_symlink()
    {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;
    let tmp = dir.join(format!(".agent.env.tmp.{}", std::process::id()));
    best_effort_cleanup(std::fs::remove_file(&tmp));
    std::os::unix::fs::symlink(env, &tmp)?;
    std::fs::rename(&tmp, &link).inspect_err(|_| {
        best_effort_cleanup(std::fs::remove_file(&tmp));
    })
}

/// The agent config in the dir that names this agent. A file that `load` would refuse counts too.
fn find_conf(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = format!("name = \"{name}\"");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            let n = basename(f);
            n.ends_with(".toml") && !n.ends_with(".board.toml") && f.is_file()
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .find(|f| std::fs::read_to_string(f).is_ok_and(|t| t.lines().any(|l| l == want)))
}

/// A child process that has the env file of the agent (`apply` removes the token variables of the
/// caller and sets the keys of the file).
fn agent_command(p: &Plan, bin: &Path) -> Result<Command, String> {
    let env = agent_env::load(&p.agent_dir, None).map_err(|e| strip_prefix(&e))?;
    let mut c = Command::new(bin);
    env.apply(&mut c);
    Ok(c)
}

/// Step 2: `join`. An agent config in the dir that names this agent means joined.
fn step_join(p: &Plan, run: &mut Run) -> Result<(Ok_, Option<PathBuf>, bool), Fail> {
    if let Some(c) = find_conf(&p.agent_dir, &p.name) {
        let text = basename(&c);
        return Ok((
            Ok_ {
                already: true,
                text,
            },
            Some(c),
            false,
        ));
    }
    let fail = |what: String, code: u8| {
        Fail {
        step: 2,
        name: "join",
        what,
        fix: "read the lines above. Before the init pull request is merged, the repo has no .crewboard.toml; then pass --board-file <initiative>.board.toml (the board file of the owner)".into(),
        code,
    }
    };
    let mut title = format!("join --agent {} --runtime {}", p.name, p.runtime.bin());
    if let Some(m) = &p.model {
        title.push_str(&format!(" --model {m}"));
    }
    let result: Result<String, (String, u8)> = if let Some(bin) = seam_bin(p) {
        let mut c = agent_command(p, Path::new(bin)).map_err(|m| fail(m, 4))?;
        // the old name on purpose: the seam binary may be an older one, and every version accepts it
        c.arg("--config-dir").arg(&p.agent_dir).args([
            "join",
            "--name",
            &p.name,
            "--runtime",
            p.runtime.bin(),
        ]);
        if let Some(m) = &p.model {
            c.args(["--model", m]);
        }
        match &p.board_file {
            Some(f) => c.arg("--initiative-file").arg(f),
            None => c.args(["--repo", &p.repo]),
        };
        let cap = run_captured(c, STEP_LIMIT);
        if cap.code == 0 && !cap.timed_out {
            Ok(cap.text)
        } else {
            run.record(&title, &cap.text);
            Err((
                format!("crewboard join exit {}", cap.code),
                exit_of(cap.code),
            ))
        }
    } else {
        let g = Global {
            config_dir: Some(p.agent_dir.clone()),
            initiative: None,
            initiative_file: p.board_file.clone(),
            insecure_one_account: false,
        };
        let repo = p.board_file.is_none().then_some(p.repo.as_str());
        match join::run(
            &g,
            join::JoinArgs {
                name: &p.name,
                runtime: p.runtime,
                model: p.model.as_deref(),
                host: None,
                repo,
                tools: &[],
            },
        ) {
            Ok(t) => Ok(t),
            Err(e) => {
                run.record(&title, &e.to_string());
                Err((
                    format!("crewboard join exit {}", e.exit().code()),
                    e.exit().code(),
                ))
            }
        }
    };
    match result {
        Ok(text) => {
            run.record(&title, &text);
            Ok((
                Ok_ {
                    already: false,
                    text: format!("join request posted for {}", p.name),
                },
                find_conf(&p.agent_dir, &p.name),
                true,
            ))
        }
        Err((what, code)) => Err(fail(what, code)),
    }
}

/// A process exit code as the code of this command: a signal or a timeout is a failure (1).
fn exit_of(code: i32) -> u8 {
    u8::try_from(code).ok().filter(|c| *c != 0).unwrap_or(1)
}

/// Step 3: the `gh` stub and the empty `gh` config dir.
fn step_shim(p: &Plan, run: &mut Run) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String| Fail {
        step: 3,
        name: "gh shim",
        what,
        fix,
        code: 4,
    };
    let bin_dir = p.agent_dir.join("bin");
    let shim = bin_dir.join("gh");
    let empty = p.agent_dir.join("gh-empty-config");
    let env_link = p.agent_dir.join("agent.env");
    let is_exec = std::fs::metadata(&shim).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    if is_exec
        && empty.is_dir()
        && std::fs::read_to_string(&shim).is_ok_and(|t| is_current_stub_for(&t, &env_link))
    {
        return Ok(Ok_ {
            already: true,
            text: shim.display().to_string(),
        });
    }
    let real =
        find_real_gh(&std::env::var_os("PATH").unwrap_or_default(), &bin_dir).ok_or_else(|| {
            fail(
                "no real gh found on the PATH".into(),
                "install gh (https://cli.github.com) and run this command again".into(),
            )
        })?;
    let me = std::env::current_exe().map_err(|e| {
        fail(
            format!("cannot find the crewboard binary: {e}"),
            "run crewboard by its full path".into(),
        )
    })?;
    let text = stub_text(&p.agent_dir, &absolute(&me), &env_link, &real);
    write_stub(&bin_dir, &text).map_err(|e| {
        fail(
            strip_prefix(&e),
            format!("move {} away and run this command again", shim.display()),
        )
    })?;
    make_empty_gh_config(&empty).map_err(|e| {
        fail(
            strip_prefix(&e),
            format!("check the permissions of {}", p.agent_dir.display()),
        )
    })?;
    run.record(
        "gh shim",
        &format!("wrote {} (real gh: {})", shim.display(), real.display()),
    );
    Ok(Ok_ {
        already: false,
        text: shim.display().to_string(),
    })
}

/// Ask GitHub through the stub (the test seam): `gh api <path> --jq <jq>`, the output trimmed.
fn gh_api(run: &mut Run, shim: &Path, path: &str, jq: &str) -> Result<String, (String, u8)> {
    let mut c = Command::new(shim);
    c.args(["api", path, "--jq", jq]);
    scrub_tokens(&mut c);
    let cap = run_captured(c, STEP_LIMIT);
    run.record(&format!("gh api {path}"), &cap.text);
    if cap.code == 0 && !cap.timed_out {
        Ok(cap.text.split_whitespace().collect())
    } else {
        Err((
            format!("gh api {path} exit {}", cap.code),
            exit_of(cap.code),
        ))
    }
}

/// Step 4: the crew id, for the noreply e-mail `<id>+<login>@users.noreply.github.com`.
fn step_crew_id(p: &Plan, run: &mut Run, shim: &Path) -> Result<(Ok_, String, String), Fail> {
    let fail = |what: String, fix: String, code: u8| Fail {
        step: 4,
        name: "crew id",
        what,
        fix,
        code,
    };
    let cached = read_crew_id(&p.agent_dir);
    if let Some((l, i)) = &cached
        && p.crew_login.as_ref().is_none_or(|w| w == l)
    {
        return Ok((
            Ok_ {
                already: true,
                text: format!("{l}, id {i}"),
            },
            l.clone(),
            i.clone(),
        ));
    }
    let env = p.env_file.display();
    let via_seam = seam_bin(p).is_some();
    let client = if via_seam {
        None
    } else {
        Some(crate::ops::new_client().map_err(|e| {
            fail(
                strip_prefix(&e),
                format!("check the token in {env}"),
                e.exit().code(),
            )
        })?)
    };
    let login = match &p.crew_login {
        Some(l) => l.clone(),
        None => {
            let got = match &client {
                Some(c) => c.viewer_login().map_err(|e| {
                    let e = CbError::from(e);
                    run.record("GET /user", &e.to_string());
                    (format!("GET /user: {e}"), e.exit().code())
                }),
                None => gh_api(run, shim, "user", ".login"),
            };
            got.map_err(|(what, code)| {
                fail(
                    what,
                    format!(
                        "check the token in {env}: crewboard agent exec --agent-dir {} -- gh auth status",
                        sq(&p.agent_dir.display().to_string())
                    ),
                    code,
                )
            })?
        }
    };
    if !valid_login(&login) {
        return Err(fail(
            format!("no numeric id for {login}: it is not a GitHub login"),
            format!("check --crew-login and the token in {env}"),
            4,
        ));
    }
    let id_path = format!("users/{login}");
    let got = match &client {
        Some(c) => c
            .get(&id_path)
            .map_err(|e| {
                let e = CbError::from(e);
                run.record(&format!("GET /{id_path}"), &e.to_string());
                (format!("GET /{id_path}: {e}"), e.exit().code())
            })
            .map(|r| {
                r.body
                    .get("id")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
                    .unwrap_or_default()
            }),
        None => gh_api(run, shim, &id_path, ".id"),
    };
    let id = got.map_err(|(what, code)| {
        fail(
            what,
            format!("check --crew-login and the token in {env}"),
            code,
        )
    })?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(fail(
            format!("no numeric id for {login}"),
            format!("check --crew-login and the token in {env}"),
            4,
        ));
    }
    std::fs::write(p.agent_dir.join("crew-id"), format!("{login} {id}\n")).map_err(|e| {
        fail(
            format!("cannot write {}/crew-id: {e}", p.agent_dir.display()),
            format!("check the permissions of {}", p.agent_dir.display()),
            4,
        )
    })?;
    Ok((
        Ok_ {
            already: false,
            text: format!("{login}, id {id}"),
        },
        login,
        id,
    ))
}

fn git_command(args: &[&str], cwd: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c.env("GIT_TERMINAL_PROMPT", "0");
    scrub_tokens(&mut c);
    c
}

/// Step 5: the work dir is a checkout of the repo, or it is cloned through the stub.
fn step_work_dir(p: &Plan, run: &mut Run, shim: &Path, login: &str) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String, code: u8| Fail {
        step: 5,
        name: "work dir",
        what,
        fix,
        code,
    };
    let wd = &p.work_dir;
    if is_checkout(wd) {
        let url = git_origin(wd);
        return if origin_matches(url.as_deref().unwrap_or(""), &p.repo) {
            Ok(Ok_ {
                already: true,
                text: wd.display().to_string(),
            })
        } else {
            Err(fail(
                format!(
                    "{} is a checkout of {}, not of {}",
                    wd.display(),
                    url.as_deref()
                        .filter(|u| !u.is_empty())
                        .unwrap_or("no origin"),
                    p.repo
                ),
                "pass another --work-dir".into(),
                4,
            ))
        };
    }
    if std::fs::read_dir(wd).is_ok_and(|mut d| d.next().is_some()) {
        return Err(fail(
            format!(
                "{} exists, is not empty and is not a git checkout",
                wd.display()
            ),
            format!("pass another --work-dir, or move {} away", wd.display()),
            4,
        ));
    }
    let made = match ensure_parent(wd) {
        Ok(m) => m,
        Err(what) => return Err(fail(what, "pass another --work-dir".into(), 4)),
    };
    let helper = format!("credential.helper={}", helper_value(shim));
    let url = format!("https://github.com/{}", p.repo);
    let wd_s = wd.display().to_string();
    let c = git_command(
        &[
            "-c",
            "credential.helper=",
            "-c",
            &helper,
            "clone",
            &url,
            &wd_s,
        ],
        None,
    );
    let cap = run_captured(c, CLONE_LIMIT);
    run.record(&format!("git clone {url} {wd_s}"), &cap.text);
    if cap.code != 0 || cap.timed_out {
        return Err(fail(
            format!("git clone exit {}", cap.code),
            format!(
                "check that {login} can read {} (the owner adds it with the Write role)",
                p.repo
            ),
            exit_of(cap.code),
        ));
    }
    let created = made.map_or(String::new(), |m| format!("created {}; ", m.display()));
    Ok(Ok_ {
        already: false,
        text: format!("{created}cloned {url} to {wd_s}"),
    })
}

/// The parent of the work dir: made when it is missing (like `mkdir -p`, mode 755), and then
/// returned as the top directory that was created. A path that is clearly wrong stops the step:
/// the nearest directory that exists is a file, or it is inside a git checkout (the repo of
/// another agent) other than a checkout of the home directory itself.
fn ensure_parent(wd: &Path) -> Result<Option<PathBuf>, String> {
    let Some(parent) = wd.parent() else {
        return Ok(None);
    };
    if parent.is_dir() {
        return Ok(None);
    }
    let Some(base) = parent.ancestors().find(|a| a.exists()) else {
        return Err(format!("no directory of {} exists", parent.display()));
    };
    if !base.is_dir() {
        return Err(format!(
            "{} is not a directory, so --work-dir cannot be created under it",
            base.display()
        ));
    }
    let top = git_command(&["rev-parse", "--show-toplevel"], Some(base));
    let cap = run_captured(top, STEP_LIMIT);
    let toplevel = cap.text.trim();
    if cap.code == 0 && !toplevel.is_empty() && !is_home(Path::new(toplevel)) {
        return Err(format!(
            "the parent of --work-dir does not exist and {} is inside the git checkout {toplevel}: that is probably the wrong path",
            base.display()
        ));
    }
    let first = parent
        .ancestors()
        .take_while(|a| !a.exists())
        .last()
        .unwrap_or(parent);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    Ok(Some(first.to_path_buf()))
}

/// Is `p` the home directory of the user (a dotfiles checkout there is not "another agent")?
fn is_home(p: &Path) -> bool {
    let canon = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf());
    std::env::var_os("HOME").is_some_and(|h| canon(Path::new(&h)) == canon(p))
}

fn git_origin(wd: &Path) -> Option<String> {
    let c = git_command(&["remote", "get-url", "origin"], Some(wd));
    let cap = run_captured(c, STEP_LIMIT);
    (cap.code == 0).then(|| cap.text.trim().to_string())
}

/// Step 6: the git identity of the crew account in the work dir.
fn step_git(p: &Plan, run: &mut Run, shim: &Path, login: &str, email: &str) -> Result<Ok_, Fail> {
    let id = Identity {
        repo: &p.work_dir,
        shim,
        login,
        email,
    };
    let text = format!("{login} <{email}>");
    if id.is_set() {
        return Ok(Ok_ {
            already: true,
            text,
        });
    }
    match id.set() {
        Ok(out) => {
            run.record("git config", &out);
            Ok(Ok_ {
                already: false,
                text,
            })
        }
        Err(e) => {
            run.record("git config", &e);
            Err(Fail {
                step: 6,
                name: "git config",
                what: "git config failed".into(),
                fix: "read the lines above and run this command again".into(),
                code: 4,
            })
        }
    }
}

pub(super) enum Start {
    Done,
    NotApproved,
    Failed(Fail),
}

/// Is this the refusal of `start` for an agent that the owner has not approved yet? It also
/// covers the moment inside `approve`: the signed approval is posted, and the item State still
/// says `requested` until `approve` writes `approved` a second later.
pub(super) fn is_not_approved(output: &str) -> bool {
    output.contains("rule=agent-approval-pending")
        || (output.contains("rule=sender-not-approved")
            && (output.contains("has not approved it yet")
                || output.contains("item State is 'requested'")))
}

/// Run `start` once as the agent, in the work dir, with the environment that the runtime gets.
pub(super) fn start_once(p: &Plan) -> Result<Captured, Fail> {
    let bin = self_bin(p)?;
    let abs = &p.agent_dir;
    let mut c = Command::new(&bin);
    // a shell sets PWD to the path as typed; the child shows the same path as the caller
    c.arg("start")
        .current_dir(&p.work_dir)
        .env("PWD", &p.work_dir);
    for (k, v) in exec_env_plan(
        abs,
        abs.join("bin").is_dir(),
        abs.join("gh-empty-config").is_dir(),
        std::env::var_os("PATH").as_deref(),
    ) {
        match v {
            Some(v) => c.env(k, v),
            None => c.env_remove(k),
        };
    }
    Ok(run_captured(c, STEP_LIMIT))
}

/// What the output of `start` means: done, not approved yet, or a failed step 7.
pub(super) fn judge_start(p: &Plan, cap: &Captured) -> Start {
    if cap.code == 0 && !cap.timed_out {
        return Start::Done;
    }
    if is_not_approved(&cap.text) {
        return Start::NotApproved;
    }
    Start::Failed(Fail {
        step: 7,
        name: "start",
        what: format!("crewboard start exit {}", cap.code),
        fix: format!(
            "see which setup check fails: cd {} && crewboard agent exec --agent-dir {} -- crewboard doctor --setup",
            sq(&p.work_dir.display().to_string()),
            sq(&p.agent_dir.display().to_string())
        ),
        code: exit_of(cap.code),
    })
}

/// Step 7: `start` as the agent.
fn step_start(p: &Plan, run: &mut Run) -> Start {
    match start_once(p) {
        Ok(cap) => {
            run.record("start", &cap.text);
            judge_start(p, &cap)
        }
        Err(f) => Start::Failed(f),
    }
}

/// The lines for the owner and for the wait. Pure.
pub(super) fn waiting_lines(p: &Plan, initiative: &str, w: &Wait) -> Vec<String> {
    vec![
        format!(
            "{PREFIX} [7/7] start: waiting: the owner has not approved {} yet",
            p.name
        ),
        format!(
            "{PREFIX} owner: run this on the owner machine: {}",
            owner_approve_command(p, initiative)
        ),
        format!(
            "{PREFIX} checking every {} s, for up to {} min; Ctrl-C stops, the same command run again goes on",
            w.interval.as_secs(),
            w.max.as_secs() / 60
        ),
    ]
}

/// Print what the owner must do, then poll `start` until the approval is there. `NotApproved`
/// means the time ran out.
pub(super) fn wait_for_approval(p: &Plan, run: &mut Run, initiative: &str, w: &Wait) -> Start {
    for l in waiting_lines(p, initiative, w) {
        println!("{l}");
    }
    note_interrupt();
    let end = poll_until(
        w,
        &SystemClock,
        |n| {
            let cap = match start_once(p) {
                Ok(c) => c,
                Err(f) => return Some(Start::Failed(f)),
            };
            let verdict = judge_start(p, &cap);
            // a refusal that repeats is kept once in the log; the answer that ends the wait is kept
            if n == 0 || !matches!(verdict, Start::NotApproved) {
                run.record("start", &cap.text);
            }
            (!matches!(verdict, Start::NotApproved)).then_some(verdict)
        },
        |minutes| {
            println!(
                "{PREFIX} still waiting for the approval of {} ({minutes} min)",
                p.name
            );
        },
    );
    end.unwrap_or_else(|| {
        println!(
            "{PREFIX} gave up after {} min: the approval did not come",
            w.max.as_secs() / 60
        );
        Start::NotApproved
    })
}

/// The command that the owner runs to approve this agent.
pub(super) fn owner_approve_command(p: &Plan, initiative: &str) -> String {
    let role = if p.coordinator {
        " --role coordinator"
    } else {
        ""
    };
    format!(
        "crewboard approve {} --initiative {initiative}{role}",
        p.name
    )
}

#[cfg(test)]
#[path = "add_tests.rs"]
mod tests;
