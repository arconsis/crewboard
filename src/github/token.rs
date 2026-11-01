//! Which token, from where. An agent command: the env file of the agent, else `GH_TOKEN` /
//! `GITHUB_TOKEN`, else `gh auth token`. An owner command: `--env-file`, else the owner's own
//! `gh auth token`. The caller passes the token of the env file in force as a value; nothing
//! here reads the loaded env. The client in `github/mod.rs` keeps the result.

use super::{GhError, Token};
use std::process::Command;

/// `file_token` (the token of the env file of the agent), else the token from the environment,
/// else from `gh auth token` (spec/overview#build-and-distribution). The caller
/// (`ctx::new_client`) stops first when the env file did not load, so that no variable and no
/// `gh` takes its place, and when an agent dir has no token (`agent_env::check_token_source`).
pub fn resolve_token(file_token: Option<&str>) -> Result<Token, GhError> {
    if let Some(t) = file_token {
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

/// The hidden test variable: what `gh auth token` would print. It is read only in a build with
/// debug assertions (the builds of `cargo test`; a release build ignores it), and only while
/// `CREWBOARD_API_BASE` names a loopback fake ([`super::parse_api_base`]), so a token from it
/// never reaches GitHub. Not documented for users; the component tests use it, because a fake
/// `gh` script does not run on every platform.
pub const TEST_GH_AUTH_TOKEN_ENV: &str = "CREWBOARD_TEST_GH_AUTH_TOKEN";

/// The token of an owner command outside an agent dir (spec/identity#author-and-signature):
/// `env_file_token` (the token of `--env-file`), else the owner's own `gh auth token`.
/// `GH_TOKEN` and `GITHUB_TOKEN` are ignored. The caller (`ctx::new_owner_client`) stops first
/// when the env file did not load, keeps the agent order in an agent dir and prints the note.
pub fn resolve_owner_token(env_file_token: Option<&str>) -> Result<Token, GhError> {
    if let Some(t) = env_file_token {
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

/// The test token of [`TEST_GH_AUTH_TOKEN_ENV`], only in a build with debug assertions and only
/// with a loopback `CREWBOARD_API_BASE`.
fn test_gh_token() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let base = std::env::var(super::API_BASE_ENV).ok()?;
    super::parse_api_base(&base).ok()?;
    std::env::var(TEST_GH_AUTH_TOKEN_ENV)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_of_the_env_file_is_a_value_of_each_call() {
        let a = resolve_token(Some("token-a")).unwrap();
        let b = resolve_token(Some("token-b")).unwrap();
        assert_eq!((a.expose(), b.expose()), ("token-a", "token-b"));
        let o = resolve_owner_token(Some("owner-file-token")).unwrap();
        assert_eq!(o.expose(), "owner-file-token");
    }
}
