//! Identity check of the agent's own `gh` (spec/identity#identity-of-the-agents-own-gh).
//!
//! crewboard checks its own token. The model also calls plain `gh` and `git` in its shell.
//! On a host with the owner's `gh` login, those calls can run as the owner without notice
//! (seen in practice: a PR and a comment posted as the owner). `start` and `selftest` run
//! `gh api user --jq .login` the way the agent's shell would run it: the `gh` that is first
//! on the PATH, without a token in the environment. If the login is not the agent's own account
//! (the account that the registry bound to it: the shared crew account or a contributor's own
//! account), they refuse with rule `agent-gh-not-crew`.
//!
//! The token variables are removed for this call, because some harnesses (Hermes) strip them
//! from every command the model runs, while crewboard itself still holds the token through
//! `crewboard agent with-env`. `CREWBOARD_GH_KEEP_TOKEN=1` keeps them, for a runtime whose shell really
//! holds the crew token. `CREWBOARD_AGENT_GH` names another `gh` executable (tests).

use crate::base::ids;
use crate::config::BoardConfig;
use crate::error::{CbError, CbResult};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub const GH_ENV: &str = "CREWBOARD_AGENT_GH";
pub const KEEP_TOKEN_ENV: &str = "CREWBOARD_GH_KEEP_TOKEN";
const TOKEN_VARS: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];
const LIMIT: Duration = Duration::from_secs(30);

/// What `gh api user --jq .login` gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhLogin {
    Login(String),
    /// No `gh` on the PATH.
    NotInstalled,
    /// `gh` ran and failed (not logged in, no network, timeout).
    Failed(String),
}

pub trait GhIdentityProbe {
    fn login(&self) -> GhLogin;
}

/// The real probe: runs the executable `exe` (default `gh`, found through the PATH).
pub struct ShellGh {
    pub exe: String,
    pub keep_token: bool,
}

impl ShellGh {
    pub fn from_env() -> Self {
        ShellGh {
            exe: std::env::var(GH_ENV)
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "gh".to_string()),
            keep_token: std::env::var(KEEP_TOKEN_ENV).as_deref() == Ok("1"),
        }
    }
}

impl GhIdentityProbe for ShellGh {
    fn login(&self) -> GhLogin {
        let mut cmd = Command::new(&self.exe);
        cmd.args(["api", "user", "--jq", ".login"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !self.keep_token {
            for v in TOKEN_VARS {
                cmd.env_remove(v);
            }
        }
        let child = match crate::spawn::spawn_retrying(&mut cmd) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return GhLogin::NotInstalled,
            Err(e) => return GhLogin::Failed(format!("cannot start {}: {e}", self.exe)),
        };
        // wait_with_output has no timeout: run it in a thread and give up after LIMIT
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            tx.send(child.wait_with_output()).ok(); // the receiver may be gone
        });
        match rx.recv_timeout(LIMIT) {
            Ok(Ok(o)) if o.status.success() => {
                let l = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if l.is_empty() {
                    GhLogin::Failed("empty login".into())
                } else {
                    GhLogin::Login(l)
                }
            }
            Ok(Ok(o)) => {
                let e = String::from_utf8_lossy(&o.stderr);
                let line = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                GhLogin::Failed(format!(
                    "exit {}: {}",
                    o.status.code().unwrap_or(-1),
                    line.trim()
                ))
            }
            Ok(Err(e)) => GhLogin::Failed(e.to_string()),
            Err(_) => GhLogin::Failed(format!("no answer within {} s", LIMIT.as_secs())),
        }
    }
}

/// Not checked: the one-person trial (`insecure_one_account`), where the agents use the owner's
/// login (spec/identity#identity-modes).
fn one_account(cfg: &BoardConfig) -> bool {
    cfg.insecure_one_account
}

/// Judge the login. `account` is the agent's own account: the login that the registry bound to
/// it (the author of its join). Returns a line for the user (may be empty) or the refusal.
///
/// - a trial board (`insecure_one_account`): not checked (the agents use the owner's login);
/// - login is the agent's account: pass;
/// - any other login (the owner, or a third account): refused, `agent-gh-not-crew`;
/// - `gh` not installed: skipped with a note;
/// - `gh` fails (not logged in): a warning only, because a bare `gh` that fails is the safe
///   state; the agent then cannot use `gh` at all.
pub fn judge(cfg: &BoardConfig, account: &str, got: &GhLogin) -> CbResult<String> {
    if one_account(cfg) {
        return Ok(String::new());
    }
    if account.eq_ignore_ascii_case(&cfg.owner_login) {
        return Err(CbError::refused(
            ids::AGENT_GH_NOT_CREW,
            format!(
                "the registry binds this agent to the owner account {account}; on a crew board an agent never acts as the owner. Join again with the token of the crew account or of your own account"
            ),
        ));
    }
    let crew = account;
    match got {
        GhLogin::NotInstalled => Ok(
            "note: no `gh` on the PATH of this agent; the identity of its gh calls is not checked\n"
                .into(),
        ),
        GhLogin::Failed(why) => Ok(format!(
            "warning: `gh api user` fails in the shell of this agent ({why}); its plain gh calls will fail, none will run as the owner. If the agent needs gh, put the crew shim first on its PATH (`crewboard agent shim`)\n"
        )),
        GhLogin::Login(l) if l.eq_ignore_ascii_case(crew) => {
            Ok(format!("agent gh: plain `gh` acts as {crew}\n"))
        }
        GhLogin::Login(l) => {
            let who = if l.eq_ignore_ascii_case(&cfg.owner_login) {
                format!("the owner {l}")
            } else {
                l.clone()
            };
            Err(CbError::refused(
                ids::AGENT_GH_NOT_CREW,
                format!(
                    "plain `gh` in the shell of this agent acts as {who}; this agent of initiative {init} acts as {crew} (the account that the registry bound to it). Its PRs and comments would carry the wrong identity. Make the crew gh shim for this agent, then run again. Replace <agent-dir> with the config directory of this agent and <work-dir> with its checkout:\n  crewboard agent shim --agent-dir ~/.config/crewboard/agents/<agent-dir> --env-file ~/.config/crewboard/{init}.env\n  crewboard agent shim --agent-dir ~/.config/crewboard/agents/<agent-dir> --work-dir <work-dir> --crew-login {crew}\nThe second command sets the git identity of the checkout; it reads the id of {crew} from <agent-dir>/crew-id, which `crewboard agent add` writes.\nThen load the exports (PATH with the shim first, empty GH_CONFIG_DIR) in the terminal of the agent, in every new terminal, also when the shim exists already:\n  eval \"$(crewboard agent env --agent-dir ~/.config/crewboard/agents/<agent-dir>)\"\nFrom any shell, start the runtime with the exports instead:\n  crewboard agent exec --agent-dir ~/.config/crewboard/agents/<agent-dir> -- <runtime>\nAn unattended wake uses `crewboard wake install --shim-dir` instead. `crewboard agent add` does all of this for a new agent, and `crewboard doctor --setup` checks it. Details: docs/guides/runtimes/gh-shim.md",
                    init = cfg.initiative
                ),
            ))
        }
    }
}

/// Run the probe and judge it. `account` is the agent's own account (see [`judge`]).
pub fn check(cfg: &BoardConfig, account: &str, probe: &dyn GhIdentityProbe) -> CbResult<String> {
    if one_account(cfg) {
        return Ok(String::new());
    }
    judge(cfg, account, &probe.login())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// A crew board: a shared crew account, or (`None`) one own account per agent.
    fn cfg(crew: Option<&str>) -> BoardConfig {
        crate::registry::tests::crew_cfg(crew)
    }

    fn refusal(r: CbResult<String>) -> Option<&'static str> {
        match r {
            Err(CbError::Refused(r)) => Some(r.rule.as_str()),
            _ => None,
        }
    }

    #[test]
    fn the_owner_or_a_stranger_is_refused_and_the_shared_crew_login_passes() {
        let c = cfg(Some("crew-bot"));
        assert_eq!(
            refusal(judge(&c, "crew-bot", &GhLogin::Login("owner-login".into()))),
            Some("agent-gh-not-crew")
        );
        assert_eq!(
            refusal(judge(&c, "crew-bot", &GhLogin::Login("someone".into()))),
            Some("agent-gh-not-crew")
        );
        let ok = judge(&c, "crew-bot", &GhLogin::Login("Crew-Bot".into())).unwrap();
        assert!(ok.contains("acts as crew-bot"), "{ok}");
    }

    #[test]
    fn the_refusal_prints_the_exact_shim_commands() {
        let c = cfg(Some("crew-bot"));
        let Err(CbError::Refused(r)) = judge(&c, "crew-bot", &GhLogin::Login("owner-login".into()))
        else {
            panic!("expected a refusal");
        };
        let m = r.message.to_string();
        assert!(
            m.contains(
                "crewboard agent shim --agent-dir ~/.config/crewboard/agents/<agent-dir> --env-file"
            ),
            "{m}"
        );
        assert!(
            m.contains(&format!(
                "--env-file ~/.config/crewboard/{}.env",
                c.initiative
            )),
            "{m}"
        );
        assert!(
            m.contains("--work-dir <work-dir> --crew-login crew-bot"),
            "{m}"
        );
        assert!(m.contains("--crew-login crew-bot"), "{m}");
        // a new terminal lacks the exports even when the shim exists
        assert!(
            m.contains(
                "eval \"$(crewboard agent env --agent-dir ~/.config/crewboard/agents/<agent-dir>)\""
            ),
            "{m}"
        );
        // any shell: the --exec form (no eval needed)
        assert!(
            m.contains(
                "crewboard agent exec --agent-dir ~/.config/crewboard/agents/<agent-dir> -- <runtime>"
            ),
            "{m}"
        );
        // pass direction: the crew login prints no commands
        let ok = judge(&c, "crew-bot", &GhLogin::Login("crew-bot".into())).unwrap();
        assert!(!ok.contains("agent shim") && !ok.contains("--env"), "{ok}");
    }

    #[test]
    fn a_missing_or_failing_gh_is_a_note_not_a_refusal() {
        let c = cfg(Some("crew-bot"));
        let n = judge(&c, "crew-bot", &GhLogin::NotInstalled).unwrap();
        assert!(n.contains("no `gh` on the PATH"), "{n}");
        let f = judge(
            &c,
            "crew-bot",
            &GhLogin::Failed("exit 4: please run gh auth login".into()),
        )
        .unwrap();
        assert!(f.starts_with("warning:") && f.contains("exit 4"), "{f}");
    }

    #[test]
    fn the_one_person_trial_is_not_checked_and_an_own_account_is() {
        // the one-person trial (insecure_one_account): the agent acts as the owner
        let c = crate::registry::tests::cfg();
        assert_eq!(
            judge(&c, "owner-login", &GhLogin::Login("owner-login".into())).unwrap(),
            ""
        );
        // a contributor's own account is checked on any board
        for board in [cfg(None), cfg(Some("crew-bot"))] {
            let ok = judge(
                &board,
                "contributor-a",
                &GhLogin::Login("Contributor-A".into()),
            );
            assert!(ok.unwrap().contains("acts as contributor-a"));
            let Err(CbError::Refused(r)) =
                judge(&board, "contributor-a", &GhLogin::Login("crew-bot".into()))
            else {
                panic!("another account must be refused");
            };
            assert_eq!(r.rule, "agent-gh-not-crew");
            assert!(
                r.message.contains("acts as contributor-a (the account"),
                "{}",
                r.message
            );
        }
        // on both crew modes the owner's account is never the agent's, whatever gh says
        for c in [cfg(Some("crew-bot")), cfg(None)] {
            assert_eq!(
                refusal(judge(
                    &c,
                    "owner-login",
                    &GhLogin::Login("owner-login".into())
                )),
                Some("agent-gh-not-crew")
            );
        }
    }

    #[cfg(unix)]
    fn fake_gh(tag: &str, script: &str) -> String {
        let d = std::env::temp_dir().join(format!("cb-fake-gh-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("gh");
        std::fs::write(&p, script).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    #[test]
    fn a_gh_that_is_not_installed_is_skipped_with_a_note() {
        let c = cfg(Some("crew-bot"));
        let probe = ShellGh {
            exe: "/nonexistent/dir/gh".into(),
            keep_token: false,
        };
        let none = check(&c, "crew-bot", &probe).unwrap();
        assert!(none.contains("no `gh` on the PATH"), "{none}");
    }

    #[cfg(unix)]
    #[test]
    fn a_real_fake_gh_script_gives_each_verdict() {
        let c = cfg(Some("crew-bot"));
        let probe = |exe: String| ShellGh {
            exe,
            keep_token: false,
        };
        // wrong login: refused
        let owner = fake_gh("owner", "#!/bin/sh\necho owner-login\n");
        assert_eq!(
            refusal(check(&c, "crew-bot", &probe(owner))),
            Some("agent-gh-not-crew")
        );
        // crew login: passes
        let crew = fake_gh("crew", "#!/bin/sh\necho crew-bot\n");
        assert!(
            check(&c, "crew-bot", &probe(crew))
                .unwrap()
                .contains("acts as crew-bot")
        );
        // a gh that fails: a warning
        let fail = fake_gh(
            "fail",
            "#!/bin/sh\necho 'please run gh auth login' >&2\nexit 4\n",
        );
        let w = check(&c, "crew-bot", &probe(fail)).unwrap();
        assert!(w.starts_with("warning:") && w.contains("exit 4"), "{w}");
    }

    #[cfg(unix)]
    #[test]
    fn the_token_variables_are_removed_unless_kept() {
        // the fake gh answers with the crew login only if GH_TOKEN is set
        let script =
            "#!/bin/sh\nif [ -n \"$GH_TOKEN\" ]; then echo crew-bot; else echo owner-login; fi\n";
        let exe = fake_gh("tok", script);
        // SAFETY of the test: a variable set for the whole test process, removed after
        unsafe { std::env::set_var("GH_TOKEN", "x") };
        let stripped = ShellGh {
            exe: exe.clone(),
            keep_token: false,
        }
        .login();
        let kept = ShellGh {
            exe,
            keep_token: true,
        }
        .login();
        unsafe { std::env::remove_var("GH_TOKEN") };
        assert_eq!(stripped, GhLogin::Login("owner-login".into()));
        assert_eq!(kept, GhLogin::Login("crew-bot".into()));
    }
}
