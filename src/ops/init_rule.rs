//! The branch rule of `init` (spec/board#authority, docs/guides/owner.md#branch-rule).
//!
//! The agent account has Write on the main repo, and AUTHORITY.md and `.crewboard.toml` are read
//! from the default branch. `init` runs with the owner token, which is admin on the repo, so it
//! can close the gap: a repository ruleset on the default branch that requires a pull request
//! and forbids force push and deletion. It asks for no approval (GitHub does not let an author
//! approve their own pull request) and for no status check (crewboard does not know the CI
//! job names). Repository admins bypass it through a pull request, so the owner can still merge.
//!
//! A ruleset, not classic branch protection: `~DEFAULT_BRANCH` follows a renamed default
//! branch, and one request sets all rules. Both kinds need the same plan on a private repo.
//!
//! The step never fails `init`. An existing rule is left alone. A refused creation gives one
//! line with the reason and the manual fix; the warning of `start` and `doctor --board` stays.

use crate::github::{Client, GhError};
use serde_json::{Value, json};

/// The name of the ruleset. A rerun finds the ruleset by this name.
pub const RULE_NAME: &str = "crewboard: default branch";

/// The manual fix, as a sentence.
const MANUAL: &str = "in the repo settings, add a ruleset (or branch protection) for the default branch that requires a pull request (docs/guides/owner.md#branch-rule)";

/// The request body of `POST repos/{repo}/rulesets`.
pub fn ruleset_body() -> Value {
    json!({
        "name": RULE_NAME,
        "target": "branch",
        "enforcement": "active",
        "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
        // actor 5 is the repository role Admin; `pull_request`: the bypass works on a pull
        // request only, never for a direct push
        "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request"}],
        "rules": [
            {"type": "deletion"},
            {"type": "non_fast_forward"},
            {"type": "pull_request", "parameters": {
                "required_approving_review_count": 0,
                "dismiss_stale_reviews_on_push": false,
                "require_code_owner_review": false,
                "require_last_push_approval": false,
                "required_review_thread_resolution": false
            }}
        ]
    })
}

fn reason(e: &GhError) -> String {
    match e {
        GhError::Http {
            status, message, ..
        } => format!("HTTP {status}: {message}"),
        other => other.to_string(),
    }
}

/// Does a rule protect `branch` of `repo` already? `Some(why)` for yes. A read that fails is
/// not a yes: the creation then decides.
fn protected_already(client: &Client, repo: &str, branch: &str) -> Option<String> {
    let classic = client
        .get(&format!("repos/{repo}/branches/{branch}"))
        .ok()
        .and_then(|r| r.body.get("protected").and_then(Value::as_bool));
    if classic == Some(true) {
        return Some(format!("{branch} of {repo} is protected already"));
    }
    let rules = client
        .get(&format!("repos/{repo}/rules/branches/{branch}"))
        .ok()
        .and_then(|r| r.body.as_array().map(|a| !a.is_empty()));
    if rules == Some(true) {
        return Some(format!("a ruleset applies to {branch} of {repo} already"));
    }
    let named = client
        .get(&format!("repos/{repo}/rulesets"))
        .ok()
        .and_then(|r| {
            r.body.as_array().and_then(|a| {
                a.iter()
                    .find(|x| x.get("name").and_then(Value::as_str) == Some(RULE_NAME))
                    .cloned()
            })
        });
    named.map(|n| {
        let state = n
            .get("enforcement")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        format!("the ruleset \"{RULE_NAME}\" of {repo} exists (enforcement: {state})")
    })
}

/// Make sure a rule protects the default branch `branch` of `repo`; the report line.
pub fn ensure(client: &Client, repo: &str, branch: &str) -> String {
    if let Some(why) = protected_already(client, repo, branch) {
        return format!("branch rule: {why}, left as it is");
    }
    match client.post(&format!("repos/{repo}/rulesets"), &ruleset_body()) {
        Ok(_) => format!(
            "branch rule: created ruleset \"{RULE_NAME}\" on the default branch of {repo}: pull request required, no force push, no deletion, 0 approvals; repo admins can bypass through a pull request"
        ),
        Err(e) => format!(
            "branch rule: not created for {repo} ({}). Fix: {MANUAL}, or run `crewboard init` again as a repo admin",
            reason(&e)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_asks_for_a_pull_request_and_no_approval_and_no_status_check() {
        let b = ruleset_body();
        assert_eq!(
            b["conditions"]["ref_name"]["include"],
            json!(["~DEFAULT_BRANCH"])
        );
        let types: Vec<&str> = b["rules"]
            .as_array()
            .map(|a| a.iter().filter_map(|r| r["type"].as_str()).collect())
            .unwrap_or_default();
        assert_eq!(types, ["deletion", "non_fast_forward", "pull_request"]);
        assert_eq!(
            b["rules"][2]["parameters"]["required_approving_review_count"],
            0
        );
        assert_eq!(b["bypass_actors"][0]["bypass_mode"], "pull_request");
    }

    #[test]
    fn a_failure_reads_as_one_line_with_status_and_message() {
        let e = GhError::Http {
            status: 403,
            message: "Upgrade to GitHub Pro".into(),
            accepted_permissions: None,
        };
        assert_eq!(reason(&e), "HTTP 403: Upgrade to GitHub Pro");
    }
}
