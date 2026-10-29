//! `init --with-ci`: a minimal CI workflow in the init pull request, for a repo that has none.
//!
//! The owner merges the init pull request anyway, and a workflow file is owner-only, so one merge
//! also brings CI. The test command comes from the files in the root of the repo, for the few
//! toolchains that a GitHub-hosted runner has installed. A repo that none of them fits gets a
//! workflow that FAILS with a message until the owner puts the test command in: a green check
//! that tests nothing would be worse than a red one.

use crate::board::repo_path;
use crate::error::{CbError, CbResult};
use crate::github::{Client, GhError};
use serde_json::Value;

/// The path of the file in the repo.
pub const CI_PATH: &str = ".github/workflows/ci.yml";

/// The files that name a toolchain, and the test command for it. The first match wins. The
/// list has no stack in front: a repo with two of these files gets the first, and the owner
/// checks the step before the merge (the report says so). The Python commands install the
/// project and `pytest` into the runner Python, which a GitHub-hosted runner has.
const DETECT: &[(&str, &str)] = &[
    ("package.json", "npm ci && npm test"),
    (
        "pyproject.toml",
        "python -m pip install . pytest && python -m pytest",
    ),
    (
        "requirements.txt",
        "python -m pip install -r requirements.txt pytest && python -m pytest",
    ),
    ("go.mod", "go test ./..."),
    ("pom.xml", "mvn --batch-mode test"),
    ("gradlew", "./gradlew test"),
    ("Cargo.toml", "cargo test --locked"),
];

/// The step of a repo for which no toolchain was found.
const PLACEHOLDER: &str =
    "echo \"ci.yml says: replace this step with the test command of this repo\"\n          exit 1";

/// The test command for the file names in the root of the repo; `None` when no toolchain is
/// recognised. Pure.
pub fn detect_test_command(root_names: &[String]) -> Option<&'static str> {
    DETECT
        .iter()
        .find(|(file, _)| root_names.iter().any(|n| n == file))
        .map(|(_, cmd)| *cmd)
}

/// The text of the workflow file. `test_cmd` `None` gives the failing placeholder. Pure.
pub fn workflow(default_branch: &str, test_cmd: Option<&str>) -> String {
    let run = test_cmd.unwrap_or(PLACEHOLDER);
    format!(
        "# Minimal CI added by `crewboard init --with-ci`. Check the test step, and edit it if the tests of\n\
# this repo need more. Agents never edit workflow files: a change here is yours.\n\
name: ci\n\
on:\n\
\x20 push:\n\
\x20   branches: [{default_branch}]\n\
\x20   paths-ignore: ['docs/**', '.crewboard.toml', 'AUTHORITY.md']\n\
\x20 pull_request:          # every crew PR (branch crew/...) gets a run for its head\n\
\x20   branches: [{default_branch}]\n\
permissions:\n\
\x20 contents: read\n\
jobs:\n\
\x20 test:\n\
\x20   runs-on: ubuntu-latest\n\
\x20   steps:\n\
\x20     - uses: actions/checkout@v4\n\
\x20     - run: |\n\
\x20         {run}\n"
    )
}

/// What `init --with-ci` will add to the PR, and what to tell the owner about it.
#[derive(Debug, PartialEq, Eq)]
pub struct CiPlan {
    /// The workflow file, or `None` when the repo has a workflow already.
    pub text: Option<String>,
    /// The lines for the report of `init`.
    pub lines: Vec<String>,
}

/// The names of the entries of a directory listing. A missing directory is empty.
fn names_in(client: &Client, repo: &str, dir: &str, git_ref: &str) -> CbResult<Vec<String>> {
    let path = if dir.is_empty() {
        format!("{}/contents?ref={git_ref}", repo_path(repo)?)
    } else {
        format!("{}/contents/{dir}?ref={git_ref}", repo_path(repo)?)
    };
    match client.get(&path) {
        Ok(r) => Ok(r
            .body
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| e.get("name").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()),
        Err(GhError::Http { status: 404, .. }) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// The refusal for an owner token that cannot write a workflow file. Nothing was written yet.
pub fn workflow_scope_error(granted: &str) -> CbError {
    CbError::config(format!(
        "--with-ci writes {CI_PATH}, and the token of the owner has no `workflow` scope (it has: {granted}); nothing was written. Run: gh auth refresh -h github.com -s workflow, then run init again"
    ))
}

/// Decide what `--with-ci` adds: nothing when the repo has a workflow already, else the file.
/// The scope of the owner token is checked here, before `init` writes anything: a classic token
/// shows its scopes, a fine-grained one shows none (and then the PUT decides).
pub fn plan(client: &Client, repo: &str, default_branch: &str) -> CbResult<CiPlan> {
    let existing = names_in(client, repo, ".github/workflows", default_branch)?;
    if existing
        .iter()
        .any(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
    {
        return Ok(CiPlan {
            text: None,
            lines: vec![format!(
                "ci: {repo} has a workflow already; --with-ci adds none"
            )],
        });
    }
    if let Some(scopes) = client.oauth_scopes()
        && !scopes.split(',').any(|s| s.trim() == "workflow")
    {
        return Err(workflow_scope_error(&scopes));
    }
    let root = names_in(client, repo, "", default_branch)?;
    let cmd = detect_test_command(&root);
    let line = match cmd {
        Some(c) => format!(
            "ci: {repo} has no workflow: {CI_PATH} goes into the init PR, with the test command `{c}` (check it before you merge)"
        ),
        None => format!(
            "WARNING: ci: {repo} has no workflow, and no toolchain was recognised: {CI_PATH} goes into the init PR with a step that FAILS until you put the test command of the repo there (edit the file on the PR branch, then merge)"
        ),
    };
    Ok(CiPlan {
        text: Some(workflow(default_branch, cmd)),
        lines: vec![line],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_toolchain_comes_from_the_root_files() {
        assert_eq!(
            detect_test_command(&names(&["README.md", "Cargo.toml"])),
            Some("cargo test --locked")
        );
        assert_eq!(
            detect_test_command(&names(&["package.json"])),
            Some("npm ci && npm test")
        );
        assert_eq!(
            detect_test_command(&names(&["pyproject.toml", "README.md"])),
            Some("python -m pip install . pytest && python -m pytest")
        );
        assert_eq!(
            detect_test_command(&names(&["requirements.txt"])),
            Some("python -m pip install -r requirements.txt pytest && python -m pytest")
        );
        assert_eq!(
            detect_test_command(&names(&["pom.xml"])),
            Some("mvn --batch-mode test")
        );
        assert_eq!(
            detect_test_command(&names(&["gradlew"])),
            Some("./gradlew test")
        );
        // no stack wins by default: a mixed repo gets the first file in the list
        assert_eq!(
            detect_test_command(&names(&["Cargo.toml", "package.json"])),
            Some("npm ci && npm test")
        );
        // a docs-only repo, or one of a stack without a rule, gets the failing placeholder
        assert_eq!(
            detect_test_command(&names(&["Package.swift", "mkdocs.yml"])),
            None
        );
        assert_eq!(
            detect_test_command(&names(&["go.mod"])),
            Some("go test ./...")
        );
        assert_eq!(detect_test_command(&names(&["README.md"])), None);
        assert_eq!(detect_test_command(&[]), None);
    }

    #[test]
    fn the_workflow_runs_for_the_pull_requests_of_the_crew_and_names_the_branch() {
        let t = workflow("trunk", Some("make test"));
        assert!(t.contains("branches: [trunk]"), "{t}");
        assert!(t.contains("pull_request:"), "{t}");
        assert!(t.contains("- run: |\n          make test\n"), "{t}");
        // the pull_request trigger has no paths-ignore: a docs-only crew PR needs a run too
        let pr = t.split("pull_request:").nth(1).unwrap();
        assert!(!pr.contains("paths-ignore"), "{t}");
        assert!(t.contains("permissions:\n  contents: read"), "{t}");
    }

    #[test]
    fn without_a_toolchain_the_step_fails_loudly() {
        let t = workflow("main", None);
        assert!(t.contains("\n          exit 1\n"), "{t}");
        assert!(t.contains("replace this step with the test command"), "{t}");
    }

    #[test]
    fn a_missing_workflow_scope_names_the_exact_fix() {
        let e = workflow_scope_error("repo, read:org").to_string();
        assert!(
            e.contains("gh auth refresh -h github.com -s workflow"),
            "{e}"
        );
        assert!(e.contains("nothing was written"), "{e}");
    }
}
