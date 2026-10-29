//! Which token, from where. An agent command: the env file of the agent, else `GH_TOKEN` /
//! `GITHUB_TOKEN`, else `gh auth token`. An owner command: `--env-file`, else the owner's own
//! `gh auth token`. The transport in `github/mod.rs` only uses the result.

use super::{GhError, Token};
use std::process::Command;

/// Token from the env file of the agent, else from the environment, else from `gh auth token`
/// (spec/overview#build-and-distribution). The refusal of an agent dir without a token
/// is `agent_env::check_token_source`; the caller runs it first.
pub fn resolve_token() -> Result<Token, GhError> {
    // a file that did not load is final: no variable and no `gh` takes its place
    if let Some(e) = crate::agent_env::failure() {
        return Err(GhError::Auth(e.to_string()));
    }
    if let Some(t) = crate::agent_env::token() {
        return Ok(Token::new(t));
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim();
            if !v.is_empty() {
                return Ok(Token::new(v));
            }
        }
    }
    let out = Command::new("gh")
        .args(["auth", "token"])
        .output()
        .map_err(|e| GhError::Auth(format!("no GH_TOKEN/GITHUB_TOKEN and cannot run `gh`: {e}")))?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || t.is_empty() {
        return Err(GhError::Auth(
            "no GH_TOKEN/GITHUB_TOKEN and `gh auth token` gave no token".into(),
        ));
    }
    Ok(Token::new(t))
}

/// The hidden test variable: what `gh auth token` would print. It is read only while
/// `CREWBOARD_API_BASE` names a loopback fake ([`super::parse_api_base`]), so a token from it
/// never reaches GitHub. Not documented for users; the component tests use it, because a fake
/// `gh` script does not run on every platform.
pub const TEST_GH_AUTH_TOKEN_ENV: &str = "CREWBOARD_TEST_GH_AUTH_TOKEN";

/// The token of an owner command outside an agent dir (spec/identity#author-and-signature): the
/// file of `--env-file`, else the owner's own `gh auth token`. `GH_TOKEN` and `GITHUB_TOKEN` are
/// ignored. The caller (`ctx::new_owner_client`) keeps the agent order in an agent dir
/// and prints the note.
pub fn resolve_owner_token() -> Result<Token, GhError> {
    if let Some(e) = crate::agent_env::failure() {
        return Err(GhError::Auth(e.to_string()));
    }
    if let Some(t) = crate::agent_env::explicit_token() {
        return Ok(Token::new(t));
    }
    own_gh_token().map(Token::new)
}

/// `gh auth token` without the four token variables of the caller: `gh` prints the token of a
/// variable when one is set, and an owner command wants the login of `gh` itself.
fn own_gh_token() -> Result<String, GhError> {
    if let Some(t) = test_gh_token() {
        return Ok(t);
    }
    let mut cmd = Command::new("gh");
    cmd.args(["auth", "token"]);
    for v in crate::agent_env::TOKEN_VARS {
        cmd.env_remove(v);
    }
    let no_login = |why: String| {
        GhError::Auth(format!(
            "this is an owner command: it runs with your own gh login, and {why}. Log in with `gh auth login`, or pass --env-file with a token of the owner"
        ))
    };
    let out = cmd
        .output()
        .map_err(|e| no_login(format!("`gh` cannot run: {e}")))?;
    let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || t.is_empty() {
        return Err(no_login("`gh auth token` gave no token".into()));
    }
    Ok(t)
}

/// The test token of [`TEST_GH_AUTH_TOKEN_ENV`], only with a loopback `CREWBOARD_API_BASE`.
fn test_gh_token() -> Option<String> {
    let base = std::env::var(super::API_BASE_ENV).ok()?;
    super::parse_api_base(&base).ok()?;
    std::env::var(TEST_GH_AUTH_TOKEN_ENV)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}
