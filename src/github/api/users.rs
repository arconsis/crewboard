//! GitHub accounts: `GET users/<login>`.

use crate::github::{Client, GhError};
use serde_json::Value;

/// One account as `GET users/<login>` gives it. A field that the answer does not have is `None`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct User {
    /// The numeric id.
    pub id: Option<u64>,
    pub node_id: Option<String>,
    /// `type`: `User` or `Organization`.
    pub kind: Option<String>,
}

/// The account `login`. An account that is not there is the HTTP 404 of the client: wrap the
/// call in [`crate::github::optional`] for `None`.
pub fn get(client: &Client, login: &str) -> Result<User, GhError> {
    let r = client.get(&format!("users/{login}"))?;
    let s = |k: &str| r.body.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(User {
        id: r.body.get("id").and_then(Value::as_u64),
        node_id: s("node_id"),
        kind: s("type"),
    })
}

/// The e-mail that GitHub accepts for an account: `<id>+<login>@users.noreply.github.com`.
pub fn noreply_email(login: &str, id: &str) -> String {
    format!("{id}+{login}@users.noreply.github.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_noreply_email_has_the_id_first() {
        assert_eq!(
            noreply_email("crew-bot", "4242"),
            "4242+crew-bot@users.noreply.github.com"
        );
    }
}
