//! Reads the workflow files of a repo and judges them for crew branches
//! (spec/ci#ci-must-run-for-crew-branches). The reader and the judgement are in
//! `domain::ci::triggers`; this module does the GET calls.

use crate::base::GhError;
use crate::domain::ci::triggers::{
    MAX_JUDGED_FILES, MAX_SHOWN_PATTERNS, check_no_required_checks, judge, judge_ignored,
    judge_paths, paths_warning, warning,
};
use crate::github::{Client, Rest};
use serde_json::Value;

/// `done` and `verify` without a CI run for the head (spec/ci#files-that-every-workflow-ignores):
/// is that because every workflow ignores the files of the PR? `Ok`: the evidence text
/// `none (all files paths-ignored: <patterns>)`. `Err`: the reason, for the old refusal.
///
/// `changed` holds the files of the PR (the caller read them), `unreadable` the CI sources
/// that could not be read. Reads, in order, each only if the step before passed: the workflow
/// directory at the head SHA and each workflow file (1 + W, W at most 40), then the base branch
/// and its rules (2). An unreadable CI source or more than [`MAX_JUDGED_FILES`] files refuse
/// before any read.
pub fn judge_no_run(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    repo: &str,
    head_sha: &str,
    head_ref: &str,
    base: &str,
    changed: &[String],
    unreadable: &[String],
) -> Result<String, String> {
    if !unreadable.is_empty() {
        return Err(format!(
            "the CI source(s) {} cannot be read, so a run there cannot be ruled out",
            unreadable.join(", ")
        ));
    }
    if changed.len() > MAX_JUDGED_FILES {
        return Err(format!(
            "the PR has {} or more files; GitHub judges paths filters on at most {MAX_JUDGED_FILES}",
            changed.len()
        ));
    }
    let workflows = read_workflows_with(get, repo, Some(head_sha), true)
        .map_err(|e| format!("the workflows of {repo} cannot be read ({e})"))?;
    let patterns = judge_ignored(&workflows, changed, head_ref, base)?;
    let base_path = crate::rules::text::url_encode(base, true);
    let branch = get(&format!("repos/{repo}/branches/{base_path}"))
        .map_err(|e| format!("branch {base} cannot be read ({e})"))?;
    let rules = match get(&format!(
        "repos/{repo}/rules/branches/{base_path}?per_page=100"
    )) {
        Ok(r) => Ok(r.body),
        Err(GhError::Http {
            status, message, ..
        }) => Err((status, message)),
        Err(e) => return Err(format!("the rules of branch {base} cannot be read ({e})")),
    };
    check_no_required_checks(
        base,
        &branch.body,
        rules.as_ref().map_err(|(s, m)| (*s, m.clone())),
    )?;
    let shown: Vec<&str> = patterns
        .iter()
        .take(MAX_SHOWN_PATTERNS)
        .map(String::as_str)
        .collect();
    let more = patterns.len().saturating_sub(shown.len());
    Ok(format!(
        "none (all files paths-ignored: {}{})",
        shown.join(", "),
        if more > 0 {
            format!(", and {more} more")
        } else {
            String::new()
        }
    ))
}

/// Read the workflow files of a repo through the contents API. A repo without
/// `.github/workflows` gives an empty list.
fn read_workflows(client: &Client, repo: &str) -> Result<Vec<(String, String)>, GhError> {
    read_workflows_with(&|p| client.get(p), repo, None, false)
}

/// At most this many workflow files are read.
const MAX_WORKFLOWS: usize = 40;

/// [`read_workflows`] over any GET function, at `git_ref` (a SHA; `None`: the default branch).
/// With `strict`, more than [`MAX_WORKFLOWS`] workflow files, a listing entry without a path,
/// or a file whose content is not base64 text is an error instead of being skipped or read as
/// empty.
fn read_workflows_with(
    get: &dyn Fn(&str) -> Result<Rest, GhError>,
    repo: &str,
    git_ref: Option<&str>,
    strict: bool,
) -> Result<Vec<(String, String)>, GhError> {
    use base64::Engine;
    let at = git_ref
        .map(|r| format!("?ref={}", crate::rules::text::url_encode(r, false)))
        .unwrap_or_default();
    let list = match get(&format!("repos/{repo}/contents/.github/workflows{at}")) {
        Ok(r) => r.body,
        Err(GhError::Http { status: 404, .. }) => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let Some(arr) = list.as_array() else {
        return Err(GhError::Shape("workflow directory is not an array".into()));
    };
    let yml: Vec<&Value> = arr
        .iter()
        .filter(|f| {
            let name = f.get("name").and_then(Value::as_str).unwrap_or("");
            name.ends_with(".yml") || name.ends_with(".yaml")
        })
        .collect();
    if strict && yml.len() > MAX_WORKFLOWS {
        return Err(GhError::Shape(format!(
            "{} workflow files; crewboard reads at most {MAX_WORKFLOWS}",
            yml.len()
        )));
    }
    let mut out = Vec::new();
    for f in yml.into_iter().take(MAX_WORKFLOWS) {
        let name = f.get("name").and_then(Value::as_str).unwrap_or("");
        let path = f.get("path").and_then(Value::as_str).unwrap_or("");
        if strict && path.is_empty() {
            return Err(GhError::Shape(format!("workflow {name} has no path")));
        }
        let r = get(&format!("repos/{repo}/contents/{path}{at}"))?;
        let b64 = r.body.get("content").and_then(Value::as_str).unwrap_or("");
        let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
        let text = base64::engine::general_purpose::STANDARD
            .decode(clean)
            .ok()
            .and_then(|b| String::from_utf8(b).ok());
        let text = match text {
            Some(t) => t,
            None if strict => {
                return Err(GhError::Shape(format!(
                    "the content of workflow {name} is not base64 text"
                )));
            }
            None => String::new(),
        };
        out.push((name.to_string(), text));
    }
    Ok(out)
}

/// The warning lines for every repo of the initiative (empty if all are covered). A read
/// error is a warning, never a failure: the check must not stop `init` or `start`.
pub fn check_repos(client: &Client, repos: &[String], prefix: &str) -> String {
    let mut out = String::new();
    for repo in repos {
        let base = client
            .get(&format!("repos/{repo}"))
            .ok()
            .and_then(|r| {
                r.body
                    .get("default_branch")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "main".to_string());
        match read_workflows(client, repo) {
            Ok(files) => {
                if let Some(w) = warning(repo, &judge(&files, prefix, &base), prefix) {
                    out.push_str(&w);
                }
                if let Some(w) = paths_warning(repo, &judge_paths(&files, &base)) {
                    out.push_str(&w);
                }
            }
            Err(e) => out.push_str(&format!(
                "warning: cannot read the workflows of {repo} ({e}); check by hand that CI runs for {prefix}** and pull_request\n"
            )),
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;

    const CREW_PUSH: &str =
        "on:\n  push:\n    branches: [main, 'crew/**']\n    paths-ignore: ['docs/**', README.md]\n";

    /// `judge_no_run` over recorded answers: the reads, their order, and the text.
    #[test]
    fn judge_no_run_reads_only_what_it_needs() {
        use base64::Engine;
        use std::cell::RefCell;
        let log: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let ok = |v: Value| -> Result<Rest, GhError> {
            Ok(Rest {
                etag: None,
                link_next: None,
                body: v,
            })
        };
        let get = |p: &str| -> Result<Rest, GhError> {
            log.borrow_mut().push(p.to_string());
            let path = p.split('?').next().unwrap_or("");
            match path {
                "repos/o/r/contents/.github/workflows" => ok(serde_json::json!([
                    {"name": "ci.yml", "path": ".github/workflows/ci.yml", "type": "file"},
                    {"name": "README.md", "path": ".github/workflows/README.md", "type": "file"}
                ])),
                "repos/o/r/contents/.github/workflows/ci.yml" => ok(serde_json::json!({
                    "content": base64::engine::general_purpose::STANDARD.encode(CREW_PUSH)
                })),
                "repos/o/r/branches/main" => ok(serde_json::json!({"protected": false})),
                "repos/o/r/rules/branches/main" => ok(serde_json::json!([])),
                _ => Err(GhError::Http {
                    status: 404,
                    message: "Not Found".into(),
                    accepted_permissions: None,
                }),
            }
        };
        let files = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };
        let r = judge_no_run(
            &get,
            "o/r",
            "abc",
            "crew/a/1-x",
            "main",
            &files(&["docs/a.md"]),
            &[],
        );
        assert_eq!(r, Ok("none (all files paths-ignored: docs/**)".to_string()));
        assert_eq!(
            *log.borrow(),
            vec![
                "repos/o/r/contents/.github/workflows?ref=abc",
                "repos/o/r/contents/.github/workflows/ci.yml?ref=abc",
                "repos/o/r/branches/main",
                "repos/o/r/rules/branches/main?per_page=100",
            ]
        );
        // a refusal of the workflows stops before the branch reads
        log.borrow_mut().clear();
        let r = judge_no_run(
            &get,
            "o/r",
            "abc",
            "crew/a/1-x",
            "main",
            &files(&["src/a.rs"]),
            &[],
        );
        assert!(r.unwrap_err().contains("src/a.rs"));
        assert_eq!(log.borrow().len(), 2);
        // an unreadable CI source and too many files refuse before any read
        log.borrow_mut().clear();
        let r = judge_no_run(
            &get,
            "o/r",
            "abc",
            "crew/a/1-x",
            "main",
            &files(&["docs/a.md"]),
            &["check-runs".into()],
        );
        assert!(r.unwrap_err().contains("check-runs cannot be read"));
        let many: Vec<String> = (0..301).map(|i| format!("docs/{i}.md")).collect();
        let r = judge_no_run(&get, "o/r", "abc", "crew/a/1-x", "main", &many, &[]);
        assert!(r.unwrap_err().contains("301 or more files"));
        assert!(log.borrow().is_empty(), "{:?}", log.borrow());
        // control: 300 files are judged
        let three_hundred: Vec<String> = (0..300).map(|i| format!("docs/{i}.md")).collect();
        assert!(
            judge_no_run(
                &get,
                "o/r",
                "abc",
                "crew/a/1-x",
                "main",
                &three_hundred,
                &[]
            )
            .is_ok()
        );
    }
}
