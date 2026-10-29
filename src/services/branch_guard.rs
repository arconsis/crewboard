//! The reads behind the branch protection warning (`domain::branch_guard`): two requests, the
//! default branch of the repo and that branch. The answer is kept for the rest of the run, so
//! `start` and `doctor` ask once. It never fails: a read that fails is `unknown`.

use crate::base::GhError;
use crate::domain::branch_guard::{Guard, ReadError, judge, judge_rules};
use crate::github::Client;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// The default branch and its guard, per repo, for this process.
type Cache = Mutex<BTreeMap<String, (String, Guard)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn to_read_error(e: GhError) -> ReadError {
    match e {
        GhError::Http {
            status, message, ..
        } => (status, message),
        other => (0, other.to_string()),
    }
}

/// Read the guard of the default branch of `repo`; `(branch, guard)`. The branch is `?` when
/// the repo answer has none.
fn read(client: &Client, repo: &str) -> (String, Guard) {
    let name = match client.get(&format!("repos/{repo}")) {
        Ok(r) => r
            .body
            .get("default_branch")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        Err(e) => {
            let (s, m) = to_read_error(e);
            return (
                "?".into(),
                Guard::Unknown(format!("the repo cannot be read (HTTP {s}: {m})")),
            );
        }
    };
    let Some(branch) = name else {
        return (
            "?".into(),
            Guard::Unknown("the repo answer has no default_branch".into()),
        );
    };
    let info = client
        .get(&format!(
            "repos/{repo}/branches/{}",
            crate::rules::text::url_encode(&branch, true)
        ))
        .map(|r| r.body)
        .map_err(to_read_error);
    let g = judge(&branch, info.as_ref());
    // a ruleset can apply while `protected` says false: ask the rules of the branch too
    if g == Guard::Open {
        let rules = client
            .get(&format!(
                "repos/{repo}/rules/branches/{}",
                crate::rules::text::url_encode(&branch, true)
            ))
            .map(|r| r.body)
            .map_err(to_read_error);
        return (branch.clone(), judge_rules(g, &branch, rules.as_ref()));
    }
    (branch, g)
}

/// The guard of the default branch of `repo`, read once per run.
pub fn default_branch_guard(client: &Client, repo: &str) -> (String, Guard) {
    if let Ok(c) = cache().lock()
        && let Some(hit) = c.get(repo)
    {
        return hit.clone();
    }
    let got = read(client, repo);
    if let Ok(mut c) = cache().lock() {
        c.insert(repo.to_string(), got.clone());
    }
    got
}

/// The line that `start` and `doctor --board` print for `repo`.
pub fn guard_line(client: &Client, repo: &str) -> String {
    let (branch, g) = default_branch_guard(client, repo);
    crate::domain::branch_guard::line(repo, &branch, &g).0
}
