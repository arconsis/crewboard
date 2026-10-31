//! The token checks of the commands: who the token is, and whether this agent or owner command
//! may run with it (spec/identity#identity-modes).

use crate::agent_env::OwnerSource;
use crate::base::ids;
use crate::config::BoardConfig;
use crate::error::{CbError, CbResult};
use crate::github::Client;

/// Owner commands (init, approve, audit, move, `pause|resume|stop --agent`, `stop --force` of the
/// owner, `agent profile set --agent`) need the token of the board owner `owner`. `source` says
/// where the token came from, so the fix names the right place. `action` completes "only the
/// board owner X ACTION". Pure.
pub fn judge_owner(viewer: &str, owner: &str, source: OwnerSource, action: &str) -> CbResult<()> {
    if viewer.eq_ignore_ascii_case(owner) {
        return Ok(());
    }
    let why = match source {
        OwnerSource::EnvFile => format!(
            "the token of --env-file is {viewer}, the board owner is {owner}. Pass the env file of {owner}, or leave out --env-file to use your own gh login"
        ),
        OwnerSource::GhLogin => format!(
            "it runs with your own gh login; `gh auth status` shows {viewer}, the board owner is {owner}. Log in as {owner} (`gh auth login`), or pass --env-file with a token of {owner}"
        ),
        OwnerSource::AgentDir => {
            let dir = crate::agent_env::config_dir()
                .map(|d| format!(" {}", d.display()))
                .unwrap_or_default();
            format!(
                "the config dir{dir} is the dir of an agent, so it runs with the token of the agent, {viewer}; the board owner is {owner}. Run it in your own terminal with your own config dir (without the CREWBOARD_CONFIG_DIR of an agent): there it uses your own gh login"
            )
        }
    };
    Err(CbError::refused(
        ids::OWNER_COMMAND_NOT_OWNER,
        format!("only the board owner {owner} {action}. This is an owner command: {why}"),
    ))
}

/// The identity mode of a board (spec/identity#identity-modes): who the agents act as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityMode {
    /// `insecure_one_account = true`: the one-person trial. The agents use the owner's token.
    Trial,
    /// `crew_login`: a crew with a shared agent account; a contributor may use an own account.
    SharedAccount,
    /// Neither key: a crew where each agent acts as its own account.
    OwnAccounts,
}

impl IdentityMode {
    pub fn of(cfg: &BoardConfig) -> Self {
        if cfg.insecure_one_account {
            IdentityMode::Trial
        } else if cfg.crew_login.is_some() {
            IdentityMode::SharedAccount
        } else {
            IdentityMode::OwnAccounts
        }
    }

    /// The one-person trial: nothing separates owner from agent.
    pub fn is_trial(self) -> bool {
        self == IdentityMode::Trial
    }
}

/// Agent commands. In the one-person trial (`insecure_one_account`) any token passes. On a crew
/// board (with or without `crew_login`) the owner's token is refused (`agent-uses-owner-token`);
/// any other account passes here. Which account an agent must use, the registry decides: the
/// owner's approval binds the account that wrote the join, and `start` checks the token against it
/// ([`judge_agent_account`]).
pub fn judge_agent(viewer: &str, cfg: &BoardConfig) -> CbResult<()> {
    if cfg.insecure_one_account || !viewer.eq_ignore_ascii_case(&cfg.owner_login) {
        return Ok(());
    }
    let who = match &cfg.crew_login {
        Some(crew) => format!("the shared agent account {crew} or their own account"),
        None => "their own account (the board names no crew_login)".to_string(),
    };
    Err(CbError::refused(
        ids::AGENT_USES_OWNER_TOKEN,
        format!(
            "this token is the owner {viewer}; agents of initiative {} act as {who}. Load the agent's token (`crewboard agent with-env --env-file ...`), never the owner's. Only the owner can make a one-person trial (`insecure_one_account = true`, `crewboard init --insecure-one-account`)",
            cfg.initiative
        ),
    ))
}

/// The token of an agent against the account that the registry bound to it: the login of its
/// join (the owner's approval binds it). `bound` is that login. A token of another account
/// would post messages that the crew drops, so `start` refuses it (`agent-token-not-crew`).
pub fn judge_agent_account(viewer: &str, name: &str, bound: &str) -> CbResult<()> {
    if viewer.eq_ignore_ascii_case(bound) {
        return Ok(());
    }
    Err(CbError::refused(
        ids::AGENT_TOKEN_NOT_CREW,
        format!(
            "this token is {viewer}; the registry binds {name} to the account {bound} (the account that wrote its join). Load the token of {bound}, or join again with this account under another agent name"
        ),
    ))
}

/// [`judge_owner`] with the login read from the token (cached per client) and the owner of
/// the board config.
pub fn require_owner(client: &Client, cfg: &BoardConfig, action: &str) -> CbResult<()> {
    judge_owner(
        &client.viewer_login()?,
        &cfg.owner_login,
        crate::agent_env::owner_source(),
        action,
    )
}

/// [`judge_agent`] with the login read from the token (cached per client). It reads
/// `GET /user` on every crew board; the trial reads nothing.
pub fn require_agent(client: &Client, cfg: &BoardConfig) -> CbResult<()> {
    if cfg.insecure_one_account {
        return Ok(());
    }
    judge_agent(&client.viewer_login()?, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(crew: Option<&str>) -> BoardConfig {
        let mut c = BoardConfig::parse(
            "initiative=\"demo\"\nproject_owner=\"owner-login\"\nproject_number=1\nrepos=[\"owner-login/app\"]\nowner_login=\"owner-login\"\nregistry_issue=2\n",
        )
        .unwrap();
        c.crew_login = crew.map(str::to_string);
        c
    }

    fn trial() -> BoardConfig {
        let mut c = board(None);
        c.insecure_one_account = true;
        c
    }

    fn rule_of(r: CbResult<()>) -> Option<&'static str> {
        match r {
            Err(CbError::Refused(r)) => Some(r.rule.as_str()),
            _ => None,
        }
    }

    #[test]
    fn the_three_identity_modes() {
        assert_eq!(IdentityMode::of(&trial()), IdentityMode::Trial);
        assert_eq!(
            IdentityMode::of(&board(Some("crew-bot"))),
            IdentityMode::SharedAccount
        );
        assert_eq!(IdentityMode::of(&board(None)), IdentityMode::OwnAccounts);
        assert!(IdentityMode::of(&trial()).is_trial());
        assert!(!IdentityMode::of(&board(None)).is_trial());
    }

    #[test]
    fn the_owner_token_is_refused_on_both_crew_modes_and_passes_in_the_trial() {
        for b in [board(Some("crew-bot")), board(None)] {
            for viewer in ["owner-login", "Owner-Login"] {
                assert_eq!(
                    rule_of(judge_agent(viewer, &b)),
                    Some("agent-uses-owner-token"),
                    "{viewer} on {:?}",
                    b.crew_login
                );
            }
        }
        let CbError::Refused(r) = judge_agent("owner-login", &board(None)).unwrap_err() else {
            panic!()
        };
        assert!(
            r.message.contains("their own account") && r.message.contains("--insecure-one-account"),
            "{}",
            r.message
        );
        // control: the trial lets every token pass, the owner's too
        for v in ["owner-login", "someone"] {
            assert!(judge_agent(v, &trial()).is_ok(), "{v}");
        }
    }

    #[test]
    fn on_a_crew_board_the_crew_token_and_a_contributor_token_pass() {
        for b in [board(Some("crew-bot")), board(None)] {
            for v in ["crew-bot", "Crew-bot", "contributor-a"] {
                assert!(judge_agent(v, &b).is_ok(), "{v}");
            }
        }
    }

    #[test]
    fn the_token_must_be_the_account_that_the_registry_bound() {
        assert!(judge_agent_account("contributor-a", "claude@a", "contributor-a").is_ok());
        assert!(judge_agent_account("Contributor-A", "claude@a", "contributor-a").is_ok());
        let e = judge_agent_account("crew-bot", "claude@a", "contributor-a");
        assert_eq!(rule_of(e), Some("agent-token-not-crew"));
        let CbError::Refused(r) =
            judge_agent_account("x", "claude@a", "contributor-a").unwrap_err()
        else {
            panic!()
        };
        assert!(
            r.message.contains("the account contributor-a"),
            "{}",
            r.message
        );
    }

    #[test]
    fn owner_commands_need_the_owner_login_in_every_mode() {
        let gh = OwnerSource::GhLogin;
        for b in [board(None), board(Some("crew-bot")), trial()] {
            let o = &b.owner_login;
            assert!(judge_owner("owner-login", o, gh, "may approve").is_ok());
            assert!(judge_owner("Owner-Login", o, gh, "may approve").is_ok());
            for v in ["crew-bot", "mallory"] {
                assert_eq!(
                    rule_of(judge_owner(v, o, gh, "may approve")),
                    Some(ids::OWNER_COMMAND_NOT_OWNER.as_str())
                );
            }
        }
    }

    #[test]
    fn the_owner_refusal_names_the_gh_login_or_the_env_file() {
        let CbError::Refused(r) = judge_owner(
            "crew-bot",
            "owner-login",
            OwnerSource::GhLogin,
            "may approve",
        )
        .unwrap_err() else {
            panic!()
        };
        assert!(
            r.message
                .contains("only the board owner owner-login may approve")
                && r.message.contains("This is an owner command")
                && r.message.contains("`gh auth status` shows crew-bot")
                && r.message.contains("--env-file"),
            "{}",
            r.message
        );
        let CbError::Refused(r) = judge_owner(
            "crew-bot",
            "owner-login",
            OwnerSource::EnvFile,
            "may approve",
        )
        .unwrap_err() else {
            panic!()
        };
        assert!(
            r.message.contains("the token of --env-file is crew-bot")
                && !r.message.contains("gh auth status"),
            "{}",
            r.message
        );
        let CbError::Refused(r) = judge_owner(
            "crew-bot",
            "owner-login",
            OwnerSource::AgentDir,
            "may approve",
        )
        .unwrap_err() else {
            panic!()
        };
        assert!(
            r.message.contains("is the dir of an agent")
                && r.message.contains("the token of the agent, crew-bot")
                && r.message.contains("own config dir"),
            "{}",
            r.message
        );
    }
}
