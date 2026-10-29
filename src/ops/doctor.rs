//! `doctor` (spec/agents#headless-coordinator): a local report of the headless scope.
//!
//! It says whether the `headless` key of the config dir switches the scope on (a guardrail, not
//! a boundary) or not, and notes a leftover system scope file that crewboard does not read any
//! more. It reads no board and sends no request.
//!
//! `doctor --board` also reads the board and warns about the built-in project workflows that
//! write Status behind crewboard (spec/board#built-in-project-workflows), and about a default
//! branch that the crew account can push to (`domain::branch_guard`). That part never changes
//! the exit.
//!
//! `doctor --setup` checks the setup of one agent, as that agent: token, push access, plain
//! `gh`, git identity, runtime, board, AUTHORITY.md and approval. It reads, it never writes.

use super::{Global, new_client, resolve_board_config};
use crate::board::Board;
use crate::builtin_workflows;
use crate::config::ConfigDir;
use crate::error::{CbError, CbResult};
use crate::fsutil::find_on_path;
use std::path::Path;

/// The headless scope line for each state.
pub const GUARDRAIL: &str = "headless scope: per-dir key only (guardrail)";
pub const NONE: &str = "headless scope: none";

/// The line of a plain `doctor`: the board part runs only with `--board`.
pub const BOARD_NOT_READ: &str =
    "built-in workflows: not checked (a local check; `crewboard doctor --board` reads the board)";

/// The last line of a plain `doctor`: the check of a new agent is another mode.
pub const SETUP_HINT: &str = "new agent: check its setup with `crewboard doctor --setup` (as that agent: crewboard agent exec --agent-dir <agent-dir> -- crewboard doctor --setup)";

/// The board part of `doctor --board` (spec/board#built-in-project-workflows). It never fails:
/// a problem gives a "not checked" line or a "could not check" warning. `scope_ok`: the local
/// check passed.
pub fn board_report(g: &Global, scope_ok: bool) -> String {
    if !scope_ok {
        return "built-in workflows: not checked (fix the headless scope first)\n".into();
    }
    let cfg = match resolve_board_config(g) {
        Ok(c) => c,
        Err(e) => return format!("built-in workflows: not checked ({e})\n"),
    };
    let place = format!(
        "the settings of project {} of {}",
        cfg.project_number, cfg.project_owner
    );
    let client = match new_client() {
        Ok(c) => c,
        Err(e) => return builtin_workflows::could_not_check(&place, &e.to_string()),
    };
    // The default branch of the main repo: can the crew account push to it (read once per run)?
    let guard = crate::services::branch_guard::guard_line(&client, cfg.main_repo());
    let board = match Board::load(&client, &cfg.project_owner, cfg.project_number) {
        Ok(b) => b,
        Err(e) => return builtin_workflows::could_not_check(&place, &e.to_string()) + &guard,
    };
    let w = builtin_workflows::check(&client, &board);
    let w = if w.is_empty() {
        format!(
            "built-in workflows: none that writes Status or adds items is on ({}/workflows)\n",
            board.url
        )
    } else {
        w
    };
    w + &guard
}

/// The report, and the result: `Ok`, or a config error when the config dir cannot be read.
pub fn run(config_dir: Option<&Path>) -> (String, CbResult<()>) {
    let mut out = String::new();
    let mut result = Ok(());
    let key = per_dir_key(config_dir);
    match &key {
        Ok((dir, true)) => out.push_str(&format!(
            "per-dir key: headless = true in {}\n",
            dir.display()
        )),
        Ok((dir, false)) => out.push_str(&format!("per-dir key: none in {}\n", dir.display())),
        Err(e) => {
            out.push_str(&format!("per-dir key: {e}\n"));
            result = Err(CbError::config(e.to_string()));
        }
    }
    let state = if matches!(key, Ok((_, true))) {
        GUARDRAIL
    } else {
        NONE
    };
    out.push_str(state);
    out.push('\n');
    if state == GUARDRAIL {
        out.push_str(
            "note: the key is a guardrail, not a boundary: the headless user can remove it or pass --config-dir to another dir. The boundary is the OS user and its token\n",
        );
    }
    (out, result)
}

fn per_dir_key(config_dir: Option<&Path>) -> CbResult<(std::path::PathBuf, bool)> {
    let cd = ConfigDir::resolve(config_dir)?;
    let on = cd.has_headless_key()?;
    Ok((cd.0, on))
}

// ---- doctor --setup -------------------------------------------------------------------------
//
// `doctor --setup` (docs/guides/agents.md#check-the-setup): run as the agent, read
// only. One line per check: `ok`, `FAIL` with the fix command, or `skip` when an earlier check
// failed. It reuses the checks of `start` and `selftest` (token, plain `gh`, AUTHORITY.md,
// approval) and adds the ones that only a new agent needs (push access, git identity, the
// runtime on the PATH of the agent).

/// The PATH that cron gives a job.
pub const SCHEDULER_PATH: &str = "/usr/bin:/bin";

/// The verdict of one setup check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckVerdict {
    Ok(String),
    Fail {
        what: String,
        fix: String,
    },
    /// Not checked, because an earlier check failed.
    Skip(String),
}

fn fail(what: impl Into<String>, fix: impl Into<String>) -> CheckVerdict {
    CheckVerdict::Fail {
        what: what.into(),
        fix: fix.into(),
    }
}

/// The lines of `doctor --setup`, in order.
#[derive(Debug, Default)]
pub struct SetupReport {
    pub lines: Vec<(String, CheckVerdict)>,
    /// Warning lines after the checks. They never change the result.
    pub warnings: Vec<String>,
}

/// The warning of a headless coordinator whose `Gate: ci` merges no person reviews, when
/// `protected_paths` in AUTHORITY.md is empty (spec/agents#headless-coordinator).
pub const NO_PROTECTED_PATHS: &str = "warning: Gate: ci tickets merge without a diff review; list the files that define your CI and tests in protected_paths (docs/guides/owner.md#authoritymd)";

/// The warning for this authority, if it applies: the agent config is headless (the only signal
/// this host has of a headless coordinator) and the list is empty.
pub fn protected_paths_warning(headless: bool, a: &crate::authority::Authority) -> Option<String> {
    (headless && a.protected_paths.is_empty()).then(|| NO_PROTECTED_PATHS.to_string())
}

impl SetupReport {
    fn push(&mut self, check: impl Into<String>, v: CheckVerdict) {
        self.lines.push((check.into(), v));
    }

    pub fn failed(&self) -> usize {
        self.lines
            .iter()
            .filter(|(_, v)| matches!(v, CheckVerdict::Fail { .. }))
            .count()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (check, v) in &self.lines {
            match v {
                CheckVerdict::Ok(t) => out.push_str(&format!("ok    {check}: {t}\n")),
                CheckVerdict::Fail { what, fix } => {
                    out.push_str(&format!("FAIL  {check}: {what}. Fix: {fix}\n"))
                }
                CheckVerdict::Skip(t) => out.push_str(&format!("skip  {check}: {t}\n")),
            }
        }
        for w in &self.warnings {
            out.push_str(w);
            out.push('\n');
        }
        out
    }

    /// The text and the result: a refusal `setup-incomplete` (exit 2) if a check failed.
    pub fn finish(self) -> (String, CbResult<()>) {
        let n = self.failed();
        let text = self.render();
        if n == 0 {
            (text, Ok(()))
        } else {
            (
                text,
                Err(CbError::refused(
                    "setup-incomplete",
                    format!(
                        "{n} setup check(s) failed; run the fix of each FAIL line, then `crewboard doctor --setup` again"
                    ),
                )),
            )
        }
    }
}

/// The scopes of a classic token (`x-oauth-scopes`). A fine-grained token has none to show.
#[cfg(test)]
pub fn judge_scopes(header: Option<&str>) -> CheckVerdict {
    judge_scopes_in(header, false)
}

/// [`judge_scopes`] for a token of a given mode. In trial mode (`trial`: one account for the
/// owner and the agents, `--insecure-one-account`) the token is the owner's, so the `workflow`
/// scope is expected and is a note, not a failure. Every other too-wide scope still fails.
pub fn judge_scopes_in(header: Option<&str>, trial: bool) -> CheckVerdict {
    let Some(h) = header else {
        return CheckVerdict::Ok(
            "GitHub shows no scopes (a fine-grained token); compare its permissions on its GitHub page with the token table of docs/guides/owner.md#the-crew-account-and-its-token".into(),
        );
    };
    let have: Vec<&str> = h
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let missing: Vec<&str> = ["repo", "project"]
        .into_iter()
        .filter(|s| !have.contains(s))
        .collect();
    let too_much: Vec<&str> = have
        .iter()
        .copied()
        .filter(|s| *s == "workflow" || *s == "delete_repo" || s.starts_with("admin:"))
        .filter(|s| !(trial && *s == "workflow"))
        .collect();
    let fix = "make a new classic token as the crew account with the scopes repo and project only, and write it to the env file";
    if !missing.is_empty() {
        return fail(
            format!(
                "the classic token lacks the scope(s) {} (it has: {h})",
                missing.join(", ")
            ),
            fix,
        );
    }
    if !too_much.is_empty() {
        return fail(
            format!(
                "the classic token has the scope(s) {}, which the owner keeps",
                too_much.join(", ")
            ),
            fix,
        );
    }
    if trial && have.contains(&"workflow") {
        return CheckVerdict::Ok(format!(
            "trial mode: one account; the workflow scope is expected (classic token, scopes {h})"
        ));
    }
    CheckVerdict::Ok(format!("classic token, scopes {h}"))
}

/// Write access of the crew account to one repo: the `permissions.push` of `GET repos/{r}`.
pub fn judge_push(
    repo: &str,
    crew: &str,
    got: &Result<serde_json::Value, crate::github::GhError>,
) -> CheckVerdict {
    let collab = format!(
        "the owner adds {crew} to {repo} with the Write role (Settings, Collaborators) and {crew} accepts the invitation"
    );
    match got {
        Ok(v) => match v
            .pointer("/permissions/push")
            .and_then(serde_json::Value::as_bool)
        {
            Some(true) => CheckVerdict::Ok(format!("{repo}: write access")),
            Some(false) => fail(format!("{repo}: the token has no write access"), collab),
            None => fail(
                format!("{repo}: the answer has no permissions"),
                "run the check again with the crew token loaded",
            ),
        },
        Err(crate::github::GhError::Http { status: 404, .. }) => fail(
            format!("{repo}: the token cannot see the repo (HTTP 404)"),
            format!(
                "a fine-grained token: add {repo} under Repository access. A classic token: {collab}"
            ),
        ),
        Err(e) => fail(
            format!("{repo}: {e}"),
            "check the network and the token, then run again",
        ),
    }
}

/// The env file behind `<agent-dir>/agent.env` (the shim and `agent with-env` need the file itself,
/// not the link), else the usual name.
fn env_file_of(agent_dir: &Path, initiative: &str) -> String {
    std::fs::canonicalize(agent_dir.join("agent.env"))
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| format!("~/.config/crewboard/{initiative}.env"))
}

/// The four checks of the env file of the agent: the file, where the token comes from, the keys
/// that crewboard does not load, and the mode of the agent dir. They read the result of the
/// loader that `cli::run` ran before the command.
fn judge_agent_env(dir: &Path) -> Vec<(&'static str, CheckVerdict)> {
    use crate::agent_env::{FileKind, TokenSource};
    let dir_text = dir.display().to_string();
    let relink = format!(
        "ln -sfn <file> {dir_text}/agent.env; the file needs mode 600 and one line GH_TOKEN=<token of the crew account>"
    );
    let loaded = crate::agent_env::state();
    let mut out = Vec::new();
    // 1. the file
    let env = match loaded {
        Some(Ok(env)) => Some(env),
        _ => None,
    };
    out.push((
        "agent env",
        match (loaded, env) {
            (Some(Err(e)), _) => fail(
                e.to_string()
                    .trim_start_matches("configuration error: ")
                    .to_string(),
                relink.clone(),
            ),
            (_, Some(env)) if env.kind() != FileKind::None => {
                let file = env
                    .file()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let checked = if cfg!(unix) {
                    "mode is private, the owner is you or root"
                } else {
                    "the mode and the owner are not checked on this build"
                };
                CheckVerdict::Ok(format!("{file}: {checked}, it parses and has a token key"))
            }
            (_, Some(_)) => {
                CheckVerdict::Skip(format!("no agent.env in {dir_text}; see the token source"))
            }
            _ => CheckVerdict::Skip("the env file was not loaded".into()),
        },
    ));
    // 2. the token source
    out.push((
        "token source",
        match env {
            None => CheckVerdict::Skip("needs a loadable env file".into()),
            Some(env) => match env.source() {
                TokenSource::AgentEnv => CheckVerdict::Ok("agent.env".into()),
                TokenSource::EnvFile => CheckVerdict::Ok("--env-file".into()),
                TokenSource::CallerVar(v) => CheckVerdict::Ok(format!("env {v}")),
                TokenSource::GhAuth => {
                    CheckVerdict::Ok("gh auth token (this config dir is not an agent dir)".into())
                }
                TokenSource::Refused => fail(
                    "gh auth token in an agent dir: crewboard would use the login of gh, which is the owner",
                    format!("put the crew token in agent.env: {relink}"),
                ),
            },
        },
    ));
    // 3. the keys that crewboard does not load
    out.push((
        "agent env keys",
        match env {
            Some(env) if env.kind() != FileKind::None => {
                if env.ignored_keys().is_empty() {
                    CheckVerdict::Ok(format!(
                        "{} keys, all crewboard settings",
                        env.loaded_keys().len()
                    ))
                } else {
                    CheckVerdict::Ok(format!(
                        "warning: {} (`crewboard wake` hands such a key to the wake command only; remove a key that no wake needs)",
                        env.ignored_warnings().join("; ")
                    ))
                }
            }
            _ => CheckVerdict::Skip("needs a loadable env file".into()),
        },
    ));
    // 4. the mode of the agent dir
    out.push(("agent dir mode", judge_dir_mode(dir)));
    out
}

/// The agent dir holds the env file and the keys: group and others must have no access.
fn judge_dir_mode(dir: &Path) -> CheckVerdict {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(dir) {
            Ok(m) if crate::fsutil::mode_is_private(m.permissions().mode()) => CheckVerdict::Ok(
                format!("{} is {:o}", dir.display(), m.permissions().mode() & 0o777),
            ),
            Ok(m) => fail(
                format!(
                    "{} is {:o}; group or others can enter it",
                    dir.display(),
                    m.permissions().mode() & 0o777
                ),
                format!("chmod 700 {}", dir.display()),
            ),
            Err(e) => fail(
                format!("cannot read {}: {e}", dir.display()),
                "pass --config-dir <agent-dir>",
            ),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        CheckVerdict::Skip("the mode is not checked on this build".into())
    }
}

/// What plain `gh` of this shell is (spec/identity#identity-of-the-agents-own-gh), as a setup
/// check. `account` is the agent's own account (the login of its token on a board with a crew
/// login: the shared crew account or the contributor's own account). Unlike `start`, a `gh` that
/// fails or is missing is a FAIL here: the agent cannot open its pull requests.
pub fn judge_agent_gh(
    cfg: &crate::config::BoardConfig,
    account: Option<&str>,
    got: &crate::agent_gh::GhLogin,
    agent_dir: &Path,
    shim_first: bool,
) -> CheckVerdict {
    use crate::agent_gh::GhLogin;
    if cfg.insecure_one_account {
        return CheckVerdict::Skip(
            "a one-person trial (insecure_one_account); the agents use the owner's login".into(),
        );
    }
    let Some(crew) = account else {
        return CheckVerdict::Skip("needs a working token".into());
    };
    let what = match got {
        GhLogin::Login(l) if l.eq_ignore_ascii_case(crew) => {
            return CheckVerdict::Ok(format!("plain `gh` acts as {crew}"));
        }
        GhLogin::Login(l) if l.eq_ignore_ascii_case(&cfg.owner_login) => {
            format!("plain `gh` acts as the owner {l}, not as {crew}")
        }
        GhLogin::Login(l) => format!("plain `gh` acts as {l}, not as {crew}"),
        GhLogin::Failed(why) => {
            format!("plain `gh` fails ({why}); the agent cannot open pull requests")
        }
        GhLogin::NotInstalled => "no `gh` on the PATH".into(),
    };
    let shim = agent_dir.join("bin").join("gh");
    let env = env_file_of(agent_dir, &cfg.initiative);
    let dir = agent_dir.display();
    let fix = if !shim.exists() {
        if matches!(got, GhLogin::NotInstalled) {
            format!(
                "install gh (https://cli.github.com), then make the shim: crewboard agent shim --agent-dir {dir} --env-file {env}"
            )
        } else {
            format!("make the shim: crewboard agent shim --agent-dir {dir} --env-file {env}")
        }
    } else if !shim_first {
        format!(
            "this shell lacks the agent exports; start the shell or the runtime with them: crewboard agent exec --agent-dir {dir} -- <command>"
        )
    } else {
        format!("the shim loads another token: {env} must hold one line GH_TOKEN=<token of {crew}>")
    };
    fail(what, fix)
}

/// The executable of a runtime label (that `join` wrote): `claude-code/...` is `claude`,
/// `codex/...` is `codex`, `hermes/...` is `hermes`; any other runtime is its first part.
pub fn runtime_binary(runtime: &str) -> String {
    let first = runtime.split('/').next().unwrap_or(runtime).trim();
    match first {
        "claude-code" | "claude" => "claude".into(),
        "hermes-agent" => "hermes".into(),
        other => other.to_string(),
    }
}

/// The PATH that a scheduler gives the wake scripts: [`SCHEDULER_PATH`], then the install dirs
/// of the `crewboard` binary, `~/.cargo/bin` and `~/.local/bin` (the loop scripts add them to find
/// crewboard itself; they say nothing about the stack of the project).
pub fn scheduler_path(home: Option<&str>) -> String {
    match home {
        Some(h) if !h.is_empty() => format!("{SCHEDULER_PATH}:{h}/.cargo/bin:{h}/.local/bin"),
        _ => SCHEDULER_PATH.to_string(),
    }
}

/// The runtime of the agent. `anywhere`: where the PATH of this shell (the agent's own PATH)
/// finds it; `minimal`: where the minimal PATH of a scheduler finds it. Only a runtime that
/// neither finds is a FAIL. A runtime outside the scheduler PATH (a Homebrew or npm `claude`,
/// the normal case on macOS) is ok: `crewboard wake install` stores its absolute path.
pub fn judge_runtime(bin: &str, minimal: Option<&Path>, anywhere: Option<&Path>) -> CheckVerdict {
    match (anywhere, minimal) {
        (Some(p), Some(_)) => CheckVerdict::Ok(format!(
            "{bin} at {} (also on the PATH of a scheduler)",
            p.display()
        )),
        (Some(p), None) => CheckVerdict::Ok(format!(
            "{bin} at {}; note: a scheduler PATH ({SCHEDULER_PATH}, ~/.cargo/bin, ~/.local/bin) does not find it, so start the wake loop with `crewboard wake install`, which stores this absolute path",
            p.display()
        )),
        (None, Some(m)) => CheckVerdict::Ok(format!(
            "{bin} at {} (on the PATH of a scheduler, not on the PATH of this shell)",
            m.display()
        )),
        (None, None) => fail(
            format!("{bin} is not on the PATH of this agent"),
            format!(
                "install {bin} or put its directory on the PATH, or join again with the --runtime of the runtime you use"
            ),
        ),
    }
}

/// The git identity that the crew account needs in the work dir.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
    /// Every `credential.helper` entry, in order (the last one wins).
    pub helpers: Vec<String>,
}

/// Judge the git identity of the work dir (`crewboard agent shim --work-dir` sets it).
pub fn judge_git(
    work_dir: &Path,
    id: Option<&GitIdentity>,
    crew: &str,
    email: &str,
    shim: &Path,
    repo: &str,
) -> CheckVerdict {
    let wd = work_dir.display();
    let Some(id) = id else {
        return fail(
            format!("{wd} is not a git checkout"),
            format!("git clone https://github.com/{repo} {wd}, then run this check again"),
        );
    };
    let mut wrong = Vec::new();
    if !id
        .name
        .as_deref()
        .is_some_and(|n| n.eq_ignore_ascii_case(crew))
    {
        wrong.push(format!(
            "user.name is {}",
            id.name.as_deref().unwrap_or("not set")
        ));
    }
    if id.email.as_deref() != Some(email) {
        wrong.push(format!(
            "user.email is {}",
            id.email.as_deref().unwrap_or("not set")
        ));
    }
    if !id
        .helpers
        .last()
        .is_some_and(|h| h.starts_with('!') && h.ends_with(" auth git-credential"))
    {
        wrong.push("the last credential.helper is not the gh shim".into());
    }
    if wrong.is_empty() {
        return CheckVerdict::Ok(format!(
            "{wd}: {crew} <{email}>, credentials through the gh shim"
        ));
    }
    fail(
        format!("{wd}: {}", wrong.join(", ")),
        format!(
            "crewboard agent shim --agent-dir {} --work-dir {wd} --crew-login {crew} --email {email}",
            shim.parent()
                .and_then(Path::parent)
                .unwrap_or(shim)
                .display()
        ),
    )
}

/// Read the git identity of a work dir. `None`: not a git checkout.
pub fn read_git_identity(work_dir: &Path) -> Option<GitIdentity> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(work_dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
    };
    let inside = git(&["rev-parse", "--git-dir"])?;
    if !inside.status.success() {
        return None;
    }
    let value = |args: &[&str]| {
        git(args)
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
    };
    let one = |key: &str| {
        value(&["config", "--get", key])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Some(GitIdentity {
        name: one("user.name"),
        email: one("user.email"),
        helpers: value(&["config", "--get-all", "credential.helper"])
            .map(|s| s.lines().map(str::to_string).collect())
            .unwrap_or_default(),
    })
}

/// The registry state of this agent: approved (and trusted), or what is missing.
/// The registry line: approved or not, and the account that the registry binds to the agent.
/// `token` is the login of the agent's token; a token of another account than the bound one
/// fails (its messages would be dropped).
pub fn judge_registry(
    world: &super::World,
    me: &str,
    item_id: &str,
    cfg: &crate::config::BoardConfig,
    join_cmd: &str,
    token: Option<&str>,
) -> CheckVerdict {
    let bound = world.reg.agents.get(me).and_then(|e| e.login());
    if let (Some(b), Some(t)) = (bound, token)
        && !b.eq_ignore_ascii_case(t)
    {
        return fail(
            format!("the registry binds {me} to the account {b}, but the token is {t}"),
            format!(
                "load the token of {b} into the env file of this agent, or join again with a new agent name"
            ),
        );
    }
    match super::say::require_approved(world, me) {
        Ok(t) => {
            let state = world
                .items
                .get(item_id)
                .and_then(|i| i.field("State"))
                .unwrap_or("unknown");
            let next = if state == "approved" {
                "; the next step is `crewboard start`"
            } else {
                ""
            };
            CheckVerdict::Ok(format!(
                "{me} is approved as {} for the account {}; state {state}{next}",
                crate::owner_cmd::role_name(t.role),
                bound.unwrap_or("?")
            ))
        }
        Err(e) => {
            let what = match &e {
                CbError::Refused(r) => r.message.to_string(),
                other => other.to_string(),
            };
            let fix = match world.reg.agents.get(me) {
                None => format!("join: {join_cmd}"),
                Some(a) if a.rejected => format!(
                    "the owner rejected {me}; join again with another name and another agent dir"
                ),
                Some(a) if a.approval.is_none() && !a.earlier_joins.is_empty() => format!(
                    "{me} has joins with more than one key; the owner approves with `/crew approve {me} key=<fingerprint>` (the key that `join` printed on the agent host); `crewboard status` lists the registry notes"
                ),
                Some(a) if a.approval.is_none() => format!(
                    "ask the owner to run: crewboard approve {me} --initiative {} (add --role coordinator for the coordinator); the join is a comment of the account {}",
                    cfg.initiative,
                    a.login().unwrap_or("?")
                ),
                Some(_) => "the owner runs `crewboard status` and reads the registry notes".into(),
            };
            fail(what, fix)
        }
    }
}

/// The toolchain line of `doctor --setup`: runs the `required_tools` that apply to the agent (by
/// its declared tags) and names the ones that its tags skip. `None` tags: the registry could not
/// be read, so the tags are unknown and only the untagged tools are judged.
fn judge_toolchain(
    cfg: &crate::config::BoardConfig,
    tags: Option<&std::collections::BTreeSet<String>>,
) -> CheckVerdict {
    let none = std::collections::BTreeSet::new();
    let rep = crate::toolchain::check(
        &cfg.required_tools,
        tags.unwrap_or(&none),
        None,
        &crate::toolchain::ShellRunner,
        crate::toolchain::TOOL_TIMEOUT,
        crate::toolchain::SMOKE_TIMEOUT,
    );
    let list = |v: Vec<String>| {
        if v.is_empty() {
            "none".to_string()
        } else {
            v.join(", ")
        }
    };
    let checked = list(rep.passed.iter().map(|c| format!("`{c}`")).collect());
    let skipped = list(
        rep.skipped
            .iter()
            .map(|k| format!("`{}` (needs tag {})", k.cmd, k.tags.join(" or ")))
            .collect(),
    );
    if rep.ok() {
        CheckVerdict::Ok(format!("checked: {checked}; skipped by tag: {skipped}"))
    } else {
        fail(
            format!("{} (skipped by tag: {skipped})", rep.failure_text()),
            "install the tool, fix PATH or the sandbox of the agent, or ask the owner to name the agent tags that need it in required_tools of .crewboard.toml",
        )
    }
}

/// The agent config of the setup check. Without one the checks that need it are skipped.
fn load_setup_agent(
    g: &Global,
    rep: &mut SetupReport,
) -> Option<(ConfigDir, crate::config::AgentConfig)> {
    let cd = match g.dir() {
        Ok(cd) => cd,
        Err(e) => {
            rep.push(
                "agent config",
                fail(e.to_string(), "pass --config-dir <agent-dir>"),
            );
            return None;
        }
    };
    let dir = cd.0.display().to_string();
    match cd.load_agent_auto(g.initiative.as_deref()) {
        Ok(a) => {
            rep.push(
                "agent config",
                CheckVerdict::Ok(format!(
                    "{} of initiative {} in {dir}",
                    a.name, a.board.initiative
                )),
            );
            Some((cd, a))
        }
        Err(e) => {
            rep.push(
                "agent config",
                fail(
                    e.to_string(),
                    format!(
                        "crewboard agent add <owner>/<repo> --env-file <initiative>.env --agent-dir {dir} --agent <agent> --runtime <claude|codex|hermes> --work-dir <work-dir>"
                    ),
                ),
            );
            for c in [
                "agent env",
                "token source",
                "agent env keys",
                "agent dir mode",
                "token",
                "push access",
                "gh shim",
                "git identity",
                "runtime",
                "board",
                "AUTHORITY.md",
                "registry",
            ] {
                rep.push(c, CheckVerdict::Skip("needs the agent config".into()));
            }
            None
        }
    }
}

/// The token: its login and, for a classic token, its scopes. Returns the client and the login if
/// the token is one the board allows.
fn check_token(
    rep: &mut SetupReport,
    cd: &ConfigDir,
    agent: &crate::config::AgentConfig,
) -> Option<(crate::github::Client, String)> {
    let cfg = &agent.board;
    let dir = cd.0.display().to_string();
    let token_fix = format!(
        "run the check with the agent's environment: crewboard agent exec --agent-dir {dir} -- crewboard doctor --setup"
    );
    let token = new_client().and_then(|c| -> CbResult<_> {
        let r = c.get("user")?;
        let login = crate::github::parse_viewer_login(&r.body)?;
        Ok((c, login))
    });
    match token {
        Ok((c, login)) => match super::judge_agent(&login, cfg) {
            Ok(()) => {
                let who = match &cfg.crew_login {
                    _ if cfg.insecure_one_account => format!(
                        "acts as {login}; identity: ONE ACCOUNT (insecure), a one-person trial, so any token passes"
                    ),
                    Some(crew) if crew.eq_ignore_ascii_case(&login) => {
                        format!("acts as {login}, the shared crew account")
                    }
                    _ => format!(
                        "acts as {login}, an own account (the owner's approval binds the account that writes the join)"
                    ),
                };
                rep.push("token", CheckVerdict::Ok(who));
                // trial mode: the board key
                let trial = cfg.insecure_one_account;
                rep.push(
                    "token scopes",
                    judge_scopes_in(c.oauth_scopes().as_deref(), trial),
                );
                Some((c, login))
            }
            Err(e) => {
                let what = match &e {
                    CbError::Refused(r) => r.message.to_string(),
                    other => other.to_string(),
                };
                let env = env_file_of(&cd.0, &cfg.initiative);
                let fix = format!(
                    "{env} must hold the token of the agent's account (the crew account or its own); {token_fix}"
                );
                rep.push("token", fail(what, fix));
                None
            }
        },
        Err(e) => {
            rep.push("token", fail(e.to_string(), token_fix));
            None
        }
    }
}

/// Write access to each repo of the initiative, for the account of the token.
fn check_push_access(
    rep: &mut SetupReport,
    client: Option<&crate::github::Client>,
    account: Option<&str>,
    cfg: &crate::config::BoardConfig,
) {
    let crew = account.unwrap_or("the agent's account");
    match client {
        Some(c) => {
            for r in &cfg.repos {
                let got = c.get(&format!("repos/{r}")).map(|x| x.body);
                rep.push("push access", judge_push(r, crew, &got));
            }
        }
        None => rep.push(
            "push access",
            CheckVerdict::Skip("needs a working token".into()),
        ),
    }
}

/// The plain `gh` of this shell (the check of `start` and `selftest`). Returns the path of the
/// `gh` shim of the agent.
fn check_gh_shim(
    rep: &mut SetupReport,
    cd: &ConfigDir,
    cfg: &crate::config::BoardConfig,
    account: Option<&str>,
) -> std::path::PathBuf {
    let shim = cd.0.join("bin").join("gh");
    let shim_first = find_on_path("gh", std::env::var("PATH").unwrap_or_default())
        .and_then(|p| std::fs::canonicalize(p).ok())
        .is_some_and(|p| std::fs::canonicalize(&shim).is_ok_and(|s| s == p));
    let got = if !cfg.insecure_one_account && account.is_some() {
        use crate::agent_gh::GhIdentityProbe;
        crate::agent_gh::ShellGh::from_env().login()
    } else {
        crate::agent_gh::GhLogin::NotInstalled
    };
    rep.push(
        "gh shim",
        judge_agent_gh(cfg, account, &got, &cd.0, shim_first),
    );
    shim
}

/// The git identity of the work dir: the agent's account (the login of its token).
fn check_git_identity(
    rep: &mut SetupReport,
    client: Option<&crate::github::Client>,
    account: Option<&str>,
    cfg: &crate::config::BoardConfig,
    work_dir: &Path,
    shim: &Path,
) {
    let crew = account.filter(|_| !cfg.insecure_one_account);
    match (crew, client) {
        (None, _) if !cfg.insecure_one_account => rep.push(
            "git identity",
            CheckVerdict::Skip("needs a working token".into()),
        ),
        (None, _) => rep.push(
            "git identity",
            CheckVerdict::Skip("a one-person trial (insecure_one_account)".into()),
        ),
        (Some(_), None) => rep.push(
            "git identity",
            CheckVerdict::Skip("needs a working token".into()),
        ),
        (Some(crew), Some(c)) => {
            match c
                .get(&format!("users/{crew}"))
                .ok()
                .and_then(|r| r.body.get("id").and_then(serde_json::Value::as_u64))
            {
                Some(id) => {
                    let email = format!("{id}+{crew}@users.noreply.github.com");
                    let found = read_git_identity(work_dir);
                    rep.push(
                        "git identity",
                        judge_git(
                            work_dir,
                            found.as_ref(),
                            crew,
                            &email,
                            shim,
                            cfg.main_repo(),
                        ),
                    );
                }
                None => rep.push(
                    "git identity",
                    fail(
                        format!("cannot read the id of {crew} (GET users/{crew})"),
                        "check the crew login of the board and the token, then run again",
                    ),
                ),
            }
        }
    }
}

/// The runtime on the PATH of the agent (and a note for the scheduler PATH).
fn check_runtime(rep: &mut SetupReport, runtime: &str) {
    let bin = runtime_binary(runtime);
    let home = std::env::var("HOME").ok();
    let minimal = find_on_path(&bin, scheduler_path(home.as_deref()));
    let anywhere = find_on_path(&bin, std::env::var("PATH").unwrap_or_default());
    rep.push(
        "runtime",
        judge_runtime(&bin, minimal.as_deref(), anywhere.as_deref()),
    );
}

/// The checks that read the board: the board, AUTHORITY.md, the registry and the toolchain.
fn check_board_side(
    rep: &mut SetupReport,
    client: Option<crate::github::Client>,
    cd: &ConfigDir,
    agent: &crate::config::AgentConfig,
    account: Option<&str>,
) {
    let cfg = &agent.board;
    let Some(client) = client else {
        for c in ["board", "AUTHORITY.md", "registry"] {
            rep.push(c, CheckVerdict::Skip("needs a working token".into()));
        }
        return;
    };
    let ctx = match super::Ctx::load(client, cfg.clone(), cd.clone()) {
        Ok(ctx) => {
            rep.push(
                "board",
                CheckVerdict::Ok(format!(
                    "project {} of {} is readable",
                    cfg.project_number, cfg.project_owner
                )),
            );
            ctx
        }
        Err(e) => {
            rep.push(
                "board",
                fail(
                    format!(
                        "cannot read project {} of {}: {e}",
                        cfg.project_number, cfg.project_owner
                    ),
                    format!(
                        "the owner gives the agent's account the Write role on the board (`crewboard init --crew-login` does it for the shared crew account {}); a classic token needs the project scope",
                        cfg.crew_login.as_deref().unwrap_or("-")
                    ),
                ),
            );
            for c in ["AUTHORITY.md", "registry"] {
                rep.push(c, CheckVerdict::Skip("needs the board".into()));
            }
            return;
        }
    };
    let file = cfg.authority.clone();
    let v = match super::start::check_authority(&ctx) {
        Ok(super::start::AuthorityCheck::Present(t)) => CheckVerdict::Ok(format!(
            "on the default branch of {}; {}",
            cfg.main_repo(),
            t.lines().collect::<Vec<_>>().join("; ")
        )),
        Ok(super::start::AuthorityCheck::Missing) => fail(
            format!("{file} is not on the default branch of {}", cfg.main_repo()),
            format!(
                "the owner merges the init pull request (branch crewboard/init-{})",
                cfg.initiative
            ),
        ),
        Err(e) => fail(
            e.to_string(),
            format!("the owner fixes the front matter of {file} on the default branch"),
        ),
    };
    rep.push(file, v);
    if cd.has_headless_key().unwrap_or(false)
        && let Some(a) = crate::ctx::load_authority_quiet(&ctx)
        && let Some(w) = protected_paths_warning(true, &a)
    {
        rep.warnings.push(w);
    }
    let join_cmd = format!(
        "crewboard --config-dir {} join --agent {} {} --repo {}",
        cd.0.display(),
        agent.name,
        crate::ops::agent::join_options(&agent.runtime),
        cfg.main_repo()
    );
    let mut cache = crate::state::StateCache::default();
    let mut tags = None;
    let v = match super::load_world(&ctx, std::slice::from_ref(&agent.item_id), &mut cache) {
        Ok(w) => {
            tags = Some(super::profile::caps_of(w.items.get(&agent.item_id)).declared);
            judge_registry(&w, &agent.name, &agent.item_id, cfg, &join_cmd, account)
        }
        Err(e) => fail(
            format!("cannot read the registry: {e}"),
            "check the network and the token, then run again",
        ),
    };
    rep.push("registry", v);
    // the tools of the repo that this agent needs, by its tags; the same check as
    // `start`, with the copy of the board config of the agent
    if !cfg.required_tools.is_empty() {
        rep.push("toolchain", judge_toolchain(cfg, tags.as_ref()));
    }
}

/// `doctor --setup`: every check of an agent's setup. `work_dir` is the checkout of the agent.
pub fn setup(g: &Global, work_dir: &Path) -> (String, CbResult<()>) {
    let mut rep = SetupReport::default();
    let Some((cd, agent)) = load_setup_agent(g, &mut rep) else {
        return rep.finish();
    };
    let cfg = &agent.board;
    for (check, v) in judge_agent_env(&cd.0) {
        rep.push(check, v);
    }
    let token = check_token(&mut rep, &cd, &agent);
    let account = token.as_ref().map(|(_, l)| l.clone());
    let client = token.map(|(c, _)| c);
    check_push_access(&mut rep, client.as_ref(), account.as_deref(), cfg);
    let shim = check_gh_shim(&mut rep, &cd, cfg, account.as_deref());
    check_git_identity(
        &mut rep,
        client.as_ref(),
        account.as_deref(),
        cfg,
        work_dir,
        &shim,
    );
    check_runtime(&mut rep, &agent.runtime);
    check_board_side(&mut rep, client, &cd, &agent, account.as_deref());
    rep.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_gh::GhLogin;

    fn is_fail(v: &CheckVerdict) -> bool {
        matches!(v, CheckVerdict::Fail { .. })
    }

    fn agent_dir(headless: bool) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let extra = if headless { "headless = true\n" } else { "" };
        std::fs::write(
            d.path().join("demo.toml"),
            format!(
                "{extra}name = \"agent-a@host-a\"\nruntime = \"x\"\nhost = \"h\"\nitem_id = \"PVTI_x\"\n\n[board]\ninitiative = \"demo\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n"
            ),
        )
        .unwrap();
        d
    }

    #[test]
    fn the_toolchain_line_skips_tagged_tools_and_fails_for_a_tagged_agent() {
        use crate::config::RequiredTool;
        let mut cfg = crate::registry::tests::cfg();
        cfg.required_tools = vec![
            RequiredTool::all("exit 0"),
            RequiredTool::tagged("exit 1", &["rust"]),
        ];
        let none = std::collections::BTreeSet::new();
        match judge_toolchain(&cfg, Some(&none)) {
            CheckVerdict::Ok(t) => assert!(
                t.contains("checked: `exit 0`") && t.contains("`exit 1` (needs tag rust)"),
                "{t}"
            ),
            v => panic!("{v:?}"),
        }
        // control: the agent with the tag runs the failing tool
        let rust = std::collections::BTreeSet::from(["rust".to_string()]);
        assert!(is_fail(&judge_toolchain(&cfg, Some(&rust))));
    }

    #[test]
    fn run_reports_the_per_dir_key_as_a_guardrail() {
        let d = agent_dir(true);
        let (text, r) = run(Some(d.path()));
        assert!(r.is_ok(), "{text}");
        assert!(text.contains("per-dir key: headless = true in"), "{text}");
        assert!(text.contains(GUARDRAIL), "{text}");
        assert!(text.contains("not a boundary"), "{text}");
    }

    #[test]
    fn run_reports_no_scope_without_the_key() {
        let d = agent_dir(false);
        let (text, r) = run(Some(d.path()));
        assert!(r.is_ok(), "{text}");
        assert!(text.contains("per-dir key: none in"), "{text}");
        assert!(text.contains(NONE) && !text.contains(GUARDRAIL), "{text}");
        assert!(!text.contains("not a boundary"), "{text}");
    }

    #[test]
    fn scopes_need_repo_and_project_and_refuse_workflow() {
        assert!(matches!(
            judge_scopes(Some("project, repo")),
            CheckVerdict::Ok(_)
        ));
        assert!(matches!(
            judge_scopes(Some("repo, project, read:org")),
            CheckVerdict::Ok(_)
        ));
        // a fine-grained token: no header, nothing to judge
        assert!(matches!(judge_scopes(None), CheckVerdict::Ok(t) if t.contains("fine-grained")));
        for bad in [
            "repo",
            "project",
            "",
            "repo, project, workflow",
            "repo, project, admin:org",
        ] {
            assert!(is_fail(&judge_scopes(Some(bad))), "{bad}");
        }
    }

    #[test]
    fn in_trial_mode_the_workflow_scope_is_a_note_and_other_wide_scopes_still_fail() {
        let v = judge_scopes_in(Some("repo, project, workflow"), true);
        assert!(
            matches!(&v, CheckVerdict::Ok(t) if t.contains("trial mode: one account; the workflow scope is expected")),
            "{v:?}"
        );
        // the same token fails for a real crew
        assert!(is_fail(&judge_scopes_in(
            Some("repo, project, workflow"),
            false
        )));
        // trial mode forgives only workflow, and still needs repo and project
        assert!(is_fail(&judge_scopes_in(
            Some("repo, project, workflow, delete_repo"),
            true
        )));
        assert!(is_fail(&judge_scopes_in(Some("repo, workflow"), true)));
        // no workflow scope: the plain line
        assert!(matches!(
            judge_scopes_in(Some("repo, project"), true),
            CheckVerdict::Ok(t) if t.starts_with("classic token")
        ));
    }

    #[test]
    fn push_access_is_read_from_the_permissions_of_the_repo() {
        let yes = Ok(serde_json::json!({"permissions": {"push": true}}));
        let no = Ok(serde_json::json!({"permissions": {"push": false}}));
        let none = Ok(serde_json::json!({}));
        let hidden = Err(crate::github::GhError::Http {
            status: 404,
            message: "Not Found".into(),
            accepted_permissions: None,
        });
        assert!(matches!(
            judge_push("o/r", "crew", &yes),
            CheckVerdict::Ok(_)
        ));
        assert!(is_fail(&judge_push("o/r", "crew", &no)));
        assert!(is_fail(&judge_push("o/r", "crew", &none)));
        let CheckVerdict::Fail { fix, .. } = judge_push("o/r", "crew", &hidden) else {
            panic!("404 must fail");
        };
        assert!(fix.contains("Repository access"), "{fix}");
    }

    #[test]
    fn plain_gh_must_be_the_crew_account_and_the_fix_fits_the_cause() {
        let mut cfg = crate::registry::tests::crew_cfg(Some("crew-bot"));
        let dir = tempfile::tempdir().unwrap();
        let ok = judge_agent_gh(
            &cfg,
            Some("crew-bot"),
            &GhLogin::Login("Crew-Bot".into()),
            dir.path(),
            false,
        );
        assert!(matches!(ok, CheckVerdict::Ok(_)), "{ok:?}");
        let fix = |got: GhLogin, first: bool| match judge_agent_gh(
            &cfg,
            Some("crew-bot"),
            &got,
            dir.path(),
            first,
        ) {
            CheckVerdict::Fail { fix, .. } => fix,
            other => panic!("{other:?}"),
        };
        // no shim yet: make it
        assert!(fix(GhLogin::Login(cfg.owner_login.clone()), false).contains("agent shim"));
        assert!(fix(GhLogin::Failed("exit 4".into()), false).contains("agent shim"));
        assert!(fix(GhLogin::NotInstalled, false).contains("install gh"));
        // a shim that is not first on the PATH: the exports are missing
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        std::fs::write(dir.path().join("bin/gh"), "#!/bin/sh\n").unwrap();
        assert!(fix(GhLogin::Login("someone".into()), false).contains("crewboard agent exec"));
        // the shim is first, and still not the crew: the env file holds another token
        assert!(
            fix(GhLogin::Login("someone".into()), true).contains("GH_TOKEN=<token of crew-bot>")
        );
        // a contributor's own account: plain gh must be that account, not the shared one
        let own = |got: &str| {
            judge_agent_gh(
                &cfg,
                Some("contributor-a"),
                &GhLogin::Login(got.into()),
                dir.path(),
                true,
            )
        };
        assert!(matches!(own("contributor-a"), CheckVerdict::Ok(_)));
        assert!(is_fail(&own("crew-bot")));
        // a one-person trial: not checked
        cfg = crate::registry::tests::cfg();
        assert!(matches!(
            judge_agent_gh(&cfg, None, &GhLogin::NotInstalled, dir.path(), false),
            CheckVerdict::Skip(_)
        ));
    }

    #[test]
    fn the_runtime_binary_and_the_scheduler_path() {
        assert_eq!(runtime_binary("claude-code/some-model"), "claude");
        assert_eq!(runtime_binary("codex/x"), "codex");
        assert_eq!(runtime_binary("hermes"), "hermes");
        assert_eq!(runtime_binary("other/x"), "other");
        assert_eq!(
            scheduler_path(Some("/h")),
            "/usr/bin:/bin:/h/.cargo/bin:/h/.local/bin"
        );
        let p = Path::new("/x/claude");
        assert!(matches!(
            judge_runtime("claude", Some(p), Some(p)),
            CheckVerdict::Ok(_)
        ));
        // a Homebrew or npm runtime: on the agent's PATH, not on the scheduler PATH -> ok, note
        let CheckVerdict::Ok(t) = judge_runtime("claude", None, Some(p)) else {
            panic!("a runtime on the agent's PATH must pass");
        };
        assert!(
            t.contains("/x/claude") && t.contains("crewboard wake install"),
            "{t}"
        );
        assert!(matches!(
            judge_runtime("claude", Some(p), None),
            CheckVerdict::Ok(_)
        ));
        // the other direction: on no PATH at all -> FAIL
        let CheckVerdict::Fail { fix, .. } = judge_runtime("claude", None, None) else {
            panic!("a missing runtime must fail");
        };
        assert!(fix.contains("install claude"), "{fix}");
    }

    #[test]
    fn git_identity_needs_name_email_and_the_shim_helper() {
        let wd = Path::new("/w");
        let shim = Path::new("/a/bin/gh");
        let good = GitIdentity {
            name: Some("crew-bot".into()),
            email: Some("7+crew-bot@users.noreply.github.com".into()),
            helpers: vec![
                "osxkeychain".into(),
                "".into(),
                "!/a/bin/gh auth git-credential".into(),
            ],
        };
        let email = "7+crew-bot@users.noreply.github.com";
        assert!(matches!(
            judge_git(wd, Some(&good), "crew-bot", email, shim, "o/r"),
            CheckVerdict::Ok(_)
        ));
        let mut wrong_mail = good.clone();
        wrong_mail.email = Some("owner@example.org".into());
        let mut no_helper = good.clone();
        no_helper.helpers = vec!["osxkeychain".into()];
        let mut wrong_name = good.clone();
        wrong_name.name = None;
        for id in [wrong_mail, no_helper, wrong_name] {
            let CheckVerdict::Fail { fix, .. } =
                judge_git(wd, Some(&id), "crew-bot", email, shim, "o/r")
            else {
                panic!("{id:?} must fail");
            };
            assert!(
                fix.contains("crewboard agent shim --agent-dir /a --work-dir /w --crew-login"),
                "{fix}"
            );
        }
        assert!(is_fail(&judge_git(
            wd, None, "crew-bot", email, shim, "o/r"
        )));
    }

    #[test]
    fn the_protected_paths_warning_needs_headless_and_an_empty_list() {
        let mut a = crate::authority::Authority::default();
        assert_eq!(
            protected_paths_warning(true, &a).as_deref(),
            Some(NO_PROTECTED_PATHS)
        );
        assert_eq!(protected_paths_warning(false, &a), None);
        a.protected_paths = vec!["tests/**".into()];
        assert_eq!(protected_paths_warning(true, &a), None);
    }

    #[test]
    fn the_report_counts_failures_and_refuses() {
        let mut r = SetupReport::default();
        r.push("a", CheckVerdict::Ok("fine".into()));
        r.push("b", CheckVerdict::Skip("later".into()));
        let (t, res) = SetupReport {
            lines: r.lines.clone(),
            ..SetupReport::default()
        }
        .finish();
        assert!(
            res.is_ok() && t.contains("ok    a: fine") && t.contains("skip  b: later"),
            "{t}"
        );
        r.push("c", fail("broken", "do x"));
        let (t, res) = r.finish();
        assert!(t.contains("FAIL  c: broken. Fix: do x"), "{t}");
        assert!(matches!(res, Err(CbError::Refused(ref x)) if x.rule == "setup-incomplete"));
    }
}
