//! How far the head of a PR is behind its base branch (the "merge first" practice,
//! docs/concepts.md). One read of the GitHub compare API; read-only. Used by `mine` (a hint) and
//! `done` (a warning). An error or a shape that does not parse gives `None`: the hint is advice,
//! never a reason to fail.

use crate::github::Client;
use serde_json::Value;

/// The number of commits that `head` is behind `base`; `None` when it cannot be read.
/// `head` is a SHA, so a branch of a fork or a branch name with odd characters is no problem.
/// `per_page=1` keeps the answer small: only `behind_by` is read.
pub fn behind_by(client: &Client, repo: &str, base: &str, head: &str) -> Option<u64> {
    let r = client
        .get(&format!(
            "repos/{repo}/compare/{}...{}?per_page=1",
            crate::rules::text::url_encode(base, true),
            crate::rules::text::url_encode(head, true)
        ))
        .ok()?;
    r.body.get("behind_by").and_then(Value::as_u64)
}

/// `1 commit`, `3 commits`.
fn commits(n: u64) -> String {
    format!("{n} commit{}", if n == 1 { "" } else { "s" })
}

/// The hint line of `mine`; `None` when the head is not behind.
pub fn mine_hint(base: &str, n: u64) -> Option<String> {
    (n > 0).then(|| format!("behind {base} by {}; merge {base} before done", commits(n)))
}

/// The warning line of `done` (with its newline); empty when the head is not behind.
pub fn done_warning(base: &str, n: u64) -> String {
    if n == 0 {
        return String::new();
    }
    format!(
        "warning: your branch is {} behind {base}; merge {base} and run done again if CI should test the latest code\n",
        commits(n)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_and_the_warning_exist_only_when_the_head_is_behind() {
        assert_eq!(mine_hint("main", 0), None);
        assert_eq!(
            mine_hint("main", 3).as_deref(),
            Some("behind main by 3 commits; merge main before done")
        );
        assert_eq!(
            mine_hint("main", 1).as_deref(),
            Some("behind main by 1 commit; merge main before done")
        );
        assert_eq!(done_warning("main", 0), "");
        assert_eq!(
            done_warning("release/1", 2),
            "warning: your branch is 2 commits behind release/1; merge release/1 and run done again if CI should test the latest code\n"
        );
    }
}
