//! The PR and CI evidence that `done`, `verify` and `merge` read: PR head and files, named CI runs, the judgement of a missing CI run.

use crate::error::{CbError, CbResult};
use crate::github::GhError;
use crate::ops::AgentCtx;
use crate::rules::Refusal;
use crate::rules::evidence::{CiFacts, PrFacts};
use crate::rules::protected::MAX_FILES;
use crate::ticket::EvidenceMsg;
use serde_json::Value;

/// Read the `--run` values (spec/ci#which-ci-runs-count) against the repos of the initiative. A run
/// in a foreign repo is refused here, before any call to GitHub.
pub(crate) fn named_runs(
    ac: &AgentCtx,
    pr_repo: &str,
    runs: &[String],
) -> Result<Vec<crate::domain::ci::evaluate::NamedRun>, Refusal> {
    runs.iter()
        .map(|r| crate::domain::ci::evaluate::parse_run_ref(r, pr_repo, &ac.ctx.cfg.repos))
        .collect()
}

/// The named runs that a judgement of the evidence `ev` counts again (spec/ci#which-ci-runs-count): the runs that the `done` message names as `named` in its
/// `ci-runs:` line, plus the `--run` values of the command. ONE helper for `verify` and `merge`
/// (and so for `tick`, which runs both). A `--run` value of a foreign repo is refused here,
/// before any call to GitHub.
pub(crate) fn evidence_runs(
    ac: &AgentCtx,
    pr_repo: &str,
    ev: &EvidenceMsg,
    runs: &[String],
) -> Result<crate::domain::ci::evaluate::CiRuns, Refusal> {
    let extra = named_runs(ac, pr_repo, runs)?;
    let evidence = ev
        .ci_runs
        .as_deref()
        .map(|t| crate::domain::ci::evaluate::named_in_ci_runs(t, pr_repo, &ac.ctx.cfg.repos))
        .unwrap_or_default();
    Ok(crate::domain::ci::evaluate::CiRuns::new(evidence, extra))
}

/// The PR URL of a signed `done` may name the main repo under a former name (a transfer or a
/// rename, AUTHORITY.md `former_repos`). If the signed `ts` of the `done` is before the cutoff of
/// that name, the PR is the one of the main repo under its current name; GitHub redirects the old
/// name, but the repo check and the calls use the current one. Pure.
pub(super) fn follow_former_name(
    pr: &mut crate::rules::evidence::PrUrl,
    done_ts: &str,
    main_repo: &str,
    former: &[crate::protocol::former::FormerRepo],
) {
    if let Some(now) =
        crate::protocol::former::current_name_of(&pr.repo, done_ts, main_repo, former)
    {
        pr.repo = now.to_string();
    }
}

/// Rule `evidence-pr-foreign-repo`: the PR is in a repo of the initiative. Pure.
pub(super) fn check_pr_in_initiative(
    repos: &[String],
    pr: &crate::rules::evidence::PrUrl,
) -> Result<(), Refusal> {
    if repos.iter().any(|r| r.eq_ignore_ascii_case(&pr.repo)) {
        return Ok(());
    }
    Err(Refusal::new(
        "evidence-pr-foreign-repo",
        format!(
            "PR {}#{} is not in a repo of the initiative ({})",
            pr.repo,
            pr.number,
            repos.join(", ")
        ),
    ))
}

pub(super) fn fetch_pr(ac: &AgentCtx, repo: &str, number: u64) -> CbResult<(PrFacts, String)> {
    let r = match ac.ctx.client.get(&format!("repos/{repo}/pulls/{number}")) {
        Ok(r) => r,
        Err(GhError::Http { status: 404, .. }) => {
            return Err(CbError::refused(
                "evidence-pr-missing",
                format!("PR {repo}#{number} does not exist"),
            ));
        }
        Err(e) => return Err(e.into()),
    };
    let b = &r.body;
    let s = |p: &str| {
        b.pointer(p)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| GhError::Shape(format!("PR answer has no {p}")))
    };
    let base = s("/base/ref")?;
    Ok((
        PrFacts {
            repo: repo.to_string(),
            number,
            state: s("/state")?,
            merged: b.get("merged").and_then(Value::as_bool).unwrap_or(false)
                || b.get("merged_at").is_some_and(|m| !m.is_null()),
            head_sha: s("/head/sha")?,
            head_ref: s("/head/ref")?,
            title: b
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            body: b
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        base,
    ))
}

/// The paths of the files of a PR (up to [`MAX_FILES`]; GitHub lists at most 3000). A longer
/// list is an error, not a cut list: a caller that reads a patch or a workflow file from it
/// must not take "not in the list" for "not in the PR" (fail closed).
pub(super) fn fetch_pr_files(ac: &AgentCtx, repo: &str, number: u64) -> CbResult<Vec<String>> {
    let f = fetch_pr_files_upto(ac, repo, number, MAX_FILES + 1)?;
    if f.over(MAX_FILES) {
        return Err(GhError::Shape(format!(
            "{repo}#{number} has more than {MAX_FILES} files: the list is cut, so crewboard cannot prove what the PR holds"
        ))
        .into());
    }
    Ok(f.names)
}

/// The files of a PR: the paths, and the old paths of renamed files.
pub(crate) struct PrFiles {
    pub names: Vec<String>,
    pub previous: Vec<String>,
    /// True when the read stopped at a full page: the PR may have more files than it lists.
    pub truncated: bool,
}

impl PrFiles {
    /// Can the list not prove the absence of a path? True when the read was cut, or the list is
    /// longer than `limit` files.
    pub fn over(&self, limit: usize) -> bool {
        self.truncated || self.names.len() > limit
    }

    /// Every path that the PR touches: the paths, then the old paths of renamed files
    /// (spec/ci#files-that-every-workflow-ignores).
    pub fn all(&self) -> Vec<String> {
        let mut v = self.names.clone();
        for p in &self.previous {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
        v
    }
}

/// [`fetch_pr_files`] that stops after the page that holds file `max` (100 per page).
pub(super) fn fetch_pr_files_upto(
    ac: &AgentCtx,
    repo: &str,
    number: u64,
    max: usize,
) -> CbResult<PrFiles> {
    let mut out = PrFiles {
        names: Vec::new(),
        previous: Vec::new(),
        truncated: false,
    };
    for page in 1..=max.div_ceil(100).max(1) {
        let r = ac.ctx.client.get(&format!(
            "repos/{repo}/pulls/{number}/files?per_page=100&page={page}"
        ))?;
        let arr = r.body.as_array().ok_or_else(|| {
            GhError::Shape(format!(
                "the files answer of {repo}#{number} is not a list: {}",
                r.body.to_string().chars().take(80).collect::<String>()
            ))
        })?;
        for f in arr {
            if let Some(p) = f.get("filename").and_then(Value::as_str) {
                out.names.push(p.to_string());
            }
            if let Some(p) = f.get("previous_filename").and_then(Value::as_str) {
                out.previous.push(p.to_string());
            }
        }
        if arr.len() < 100 {
            break;
        }
        // a full last page that the cap allows no more of: there may be more files
        out.truncated = page == max.div_ceil(100).max(1);
    }
    Ok(out)
}

/// The files of a PR for the rule `protected-path-owner-merge`: up to one file more than
/// [`crate::rules::protected::MAX_FILES`], so that "more than that" is known. A list that cannot
/// be read is a refusal of the same rule, not a pass (fail closed).
pub(crate) fn pr_files_for_protection(
    ac: &AgentCtx,
    repo: &str,
    number: u64,
    ticket: &str,
) -> CbResult<crate::rules::protected::PrFileList> {
    use crate::rules::protected::{PrFileList, RULE};
    match fetch_pr_files_upto(ac, repo, number, MAX_FILES + 1) {
        Ok(f) => Ok(PrFileList {
            truncated: f.over(MAX_FILES),
            names: f.names,
            previous: f.previous,
        }),
        Err(e) => Err(Refusal::new(
            RULE,
            format!(
                "the files of {repo}#{number} cannot be read ({e}); crewboard cannot prove that none is protected: try again, or the owner merges it by hand (its body closes the ticket), then run crewboard close {ticket}; without a closing line: crewboard close {ticket} --why \"<text>\""
            ),
        )
        .into()),
    }
}

/// spec/ci#files-that-every-workflow-ignores: the judgement for a head without any CI run, for
/// `done`, `verify` and `merge`. Asked only for [`CiFacts::NoCi`] when the ticket does not
/// declare `CI: none`; any other case gives `None` and no read. `files`: the files of the PR if
/// the caller read them already (`done`); otherwise up to 4 pages are read here (301 files are
/// enough to see "more than 300").
#[allow(clippy::too_many_arguments)]
pub(crate) fn judge_no_run_for(
    ac: &AgentCtx,
    ci: &CiFacts,
    declared: bool,
    pr: &PrFacts,
    sha: &str,
    base: &str,
    files: Option<&PrFiles>,
) -> CbResult<Option<Result<String, String>>> {
    let CiFacts::NoCi(unreadable) = ci else {
        return Ok(None);
    };
    if declared {
        return Ok(None);
    }
    let read;
    let files = match files {
        Some(f) => f,
        None => {
            read = match fetch_pr_files_upto(
                ac,
                &pr.repo,
                pr.number,
                crate::domain::ci::triggers::MAX_JUDGED_FILES + 1,
            ) {
                Ok(f) => f,
                Err(e) => {
                    return Ok(Some(Err(format!(
                        "the files of the PR cannot be read ({e})"
                    ))));
                }
            };
            &read
        }
    };
    Ok(Some(crate::services::ci_triggers::judge_no_run(
        &|p| ac.ctx.client.get(p),
        &pr.repo,
        sha,
        &pr.head_ref,
        base,
        &files.all(),
        unreadable,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::former::FormerRepo;
    use crate::rules::evidence::parse_pr_url;

    const MAIN: &str = "example-org/example-repo";
    const OLD: &str = "old-owner/old-repo";

    /// The PR of a `done` after `follow_former_name`, judged by the repo rule.
    fn judged(url: &str, ts: &str, former: &[FormerRepo]) -> Result<String, Refusal> {
        let mut pr = parse_pr_url(url).unwrap();
        follow_former_name(&mut pr, ts, MAIN, former);
        check_pr_in_initiative(&[MAIN.to_string()], &pr)?;
        Ok(pr.repo)
    }

    fn former() -> Vec<FormerRepo> {
        vec![FormerRepo::parse(OLD, "2026-11-01T12:00:00Z").unwrap()]
    }

    #[test]
    fn a_done_before_the_cutoff_may_name_its_pr_under_the_former_name() {
        let url = format!("https://github.com/{OLD}/pull/7");
        assert_eq!(
            judged(&url, "2026-11-01T11:59:59Z", &former()).unwrap(),
            MAIN
        );
    }

    #[test]
    fn at_or_after_the_cutoff_without_a_list_or_for_another_repo_it_is_refused() {
        let url = format!("https://github.com/{OLD}/pull/7");
        for (ts, f) in [
            ("2026-11-01T12:00:00Z", former()),
            ("2026-11-02T00:00:00Z", former()),
            ("2026-10-01T00:00:00Z", Vec::new()),
        ] {
            let e = judged(&url, ts, &f).unwrap_err();
            assert_eq!(e.rule, "evidence-pr-foreign-repo", "{ts}");
        }
        let other = "https://github.com/third/repo/pull/7";
        let e = judged(other, "2026-10-01T00:00:00Z", &former()).unwrap_err();
        assert_eq!(e.rule, "evidence-pr-foreign-repo");
        // control: the current name always passes
        let now = format!("https://github.com/{MAIN}/pull/7");
        assert_eq!(judged(&now, "2030-01-01T00:00:00Z", &[]).unwrap(), MAIN);
    }
}
