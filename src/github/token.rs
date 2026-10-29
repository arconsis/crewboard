//! Which token, from where: the env file of the agent, else `GH_TOKEN` / `GITHUB_TOKEN`,
//! else `gh auth token`. The transport in `github/mod.rs` only uses the result.

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
