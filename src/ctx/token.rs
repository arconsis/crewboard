//! The token checks of the commands: who the token is, and whether this agent or owner command
//! may run with it (spec/agents#one-account-mode and the crew account rule).

use crate::config::BoardConfig;
use crate::error::{CbError, CbResult};
use crate::github::Client;

/// Owner commands (init, approve, audit, `pause|resume|stop --agent`) need the owner's
/// token. `action` completes "only the owner X ACTION; this token is Y".
pub fn judge_owner(viewer: &str, cfg: &BoardConfig, action: &str) -> CbResult<()> {
    if viewer.eq_ignore_ascii_case(&cfg.owner_login) {
        return Ok(());
    }
    Err(CbError::refused(
        "not-owner",
        format!(
            "only the owner {} {action}; this token is {viewer}",
            cfg.owner_login
        ),
    ))
}

/// The rule of a board without `crew_login` and without consent.
pub const ONE_ACCOUNT_RULE: &str = "one-account-needs-flag";

/// The text of the refusal for an agent command: the next step is the owner's.
pub const ONE_ACCOUNT_REFUSAL: &str = "this board has no crew account; only the owner may allow a one-account trial (the owner runs join or start with --insecure-one-account, or passes --crew-login <account> to init). Stop and tell the owner.";

/// The text of the refusal of `init`: the owner's next step first.
pub const ONE_ACCOUNT_INIT_REFUSAL: &str = "this board has no crew_login, so the agents would use the owner's token and nothing separates owner from agent; pass --crew-login <account> to init (recommended), or --insecure-one-account for a one-person trial";

/// Agent commands: when the board names a crew login, the token must be that account. The
/// owner's token is refused with its own rule, so the log says which mistake it was.
/// Without a crew login (one-account mode) the agents use the owner's token and any token
/// passes, but only with consent: the board key `insecure_one_account`, the agent config key
/// or `start --insecure-one-account` (`consent`). Without it the rule is
/// `one-account-needs-flag`. A board with a crew login ignores the consent.
pub fn judge_agent(viewer: &str, cfg: &BoardConfig, consent: bool) -> CbResult<()> {
    let Some(crew) = &cfg.crew_login else {
        return one_account_consent(cfg, consent);
    };
    if viewer.eq_ignore_ascii_case(&cfg.owner_login) {
        return Err(CbError::refused(
            "agent-uses-owner-token",
            format!(
                "this token is the owner {viewer}; agents of initiative {} act as {crew}. Load the crew token (`crewboard agent with-env --env-file ...`), never the owner's",
                cfg.initiative
            ),
        ));
    }
    if !viewer.eq_ignore_ascii_case(crew) {
        return Err(CbError::refused(
            "agent-token-not-crew",
            format!(
                "this token is {viewer}; agents of initiative {} act as {crew}",
                cfg.initiative
            ),
        ));
    }
    Ok(())
}

/// A board without `crew_login` needs the consent of the owner or of the agent.
fn one_account_consent(cfg: &BoardConfig, consent: bool) -> CbResult<()> {
    if cfg.insecure_one_account || consent {
        return Ok(());
    }
    Err(CbError::refused(ONE_ACCOUNT_RULE, ONE_ACCOUNT_REFUSAL))
}

/// [`judge_owner`] with the login read from the token (cached per client).
pub fn require_owner(client: &Client, cfg: &BoardConfig, action: &str) -> CbResult<()> {
    judge_owner(&client.viewer_login()?, cfg, action)
}

/// [`judge_agent`] with the login read from the token (cached per client). It reads
/// `GET /user` only when the board names a crew login.
pub fn require_agent(client: &Client, cfg: &BoardConfig, consent: bool) -> CbResult<()> {
    if cfg.crew_login.is_none() {
        return one_account_consent(cfg, consent);
    }
    judge_agent(&client.viewer_login()?, cfg, consent)
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

    fn rule_of(r: CbResult<()>) -> Option<&'static str> {
        match r {
            Err(CbError::Refused(r)) => Some(r.rule),
            _ => None,
        }
    }

    #[test]
    fn an_agent_with_the_owner_token_is_refused_when_a_crew_login_is_set() {
        let b = board(Some("crew-bot"));
        assert_eq!(
            rule_of(judge_agent("owner-login", &b, false)),
            Some("agent-uses-owner-token")
        );
        // case does not matter: GitHub logins are case-insensitive
        assert_eq!(
            rule_of(judge_agent("Owner-Login", &b, false)),
            Some("agent-uses-owner-token")
        );
    }

    #[test]
    fn an_agent_with_a_foreign_token_is_refused_and_the_crew_token_passes() {
        let b = board(Some("crew-bot"));
        assert_eq!(
            rule_of(judge_agent("mallory", &b, false)),
            Some("agent-token-not-crew")
        );
        assert!(judge_agent("crew-bot", &b, false).is_ok());
        assert!(judge_agent("Crew-bot", &b, false).is_ok());
    }

    #[test]
    fn without_a_crew_login_every_token_passes_for_agents_that_consent() {
        let mut b = board(None);
        for v in ["owner-login", "mallory"] {
            assert!(judge_agent(v, &b, true).is_ok(), "{v}");
        }
        b.insecure_one_account = true;
        for v in ["owner-login", "mallory"] {
            assert!(judge_agent(v, &b, false).is_ok(), "{v}");
        }
    }

    #[test]
    fn the_one_account_matrix_crew_login_board_key_and_agent_consent() {
        // (crew_login, board key, consent) -> the rule that refuses, or None
        let cases: [(Option<&str>, bool, bool, Option<&str>); 8] = [
            (None, false, false, Some(ONE_ACCOUNT_RULE)),
            (None, true, false, None),
            (None, false, true, None),
            (None, true, true, None),
            // a crew board ignores the consent: the token decides
            (Some("crew-bot"), false, false, None),
            (Some("crew-bot"), false, true, None),
            // the board key beside a crew login is refused by the config, not here
            (Some("crew-bot"), true, false, None),
            (Some("crew-bot"), true, true, None),
        ];
        for (crew, key, consent, want) in cases {
            let mut b = board(crew);
            b.insecure_one_account = key;
            let viewer = crew.unwrap_or("owner-login");
            assert_eq!(
                rule_of(judge_agent(viewer, &b, consent)),
                want,
                "crew {crew:?} key {key} consent {consent}"
            );
        }
        // a crew board still refuses the owner's token, with or without consent
        let b = board(Some("crew-bot"));
        assert_eq!(
            rule_of(judge_agent("owner-login", &b, true)),
            Some("agent-uses-owner-token")
        );
    }

    #[test]
    fn the_refusal_names_the_next_command() {
        let e = judge_agent("owner-login", &board(None), false).unwrap_err();
        let CbError::Refused(r) = e else { panic!() };
        assert!(r.message.contains("--crew-login") && r.message.contains("--insecure-one-account"));
    }

    #[test]
    fn owner_commands_need_the_owner_login_also_when_a_crew_login_is_set() {
        for crew in [None, Some("crew-bot")] {
            let b = board(crew);
            assert!(judge_owner("owner-login", &b, "may approve").is_ok());
            assert_eq!(
                rule_of(judge_owner("crew-bot", &b, "may approve")),
                Some("not-owner")
            );
            assert_eq!(
                rule_of(judge_owner("mallory", &b, "may approve")),
                Some("not-owner")
            );
        }
    }
}
