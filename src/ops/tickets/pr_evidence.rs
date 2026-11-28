//! The PR and CI evidence that `done`, `verify` and `merge` read: PR head and files, named CI runs, the judgement of a missing CI run.

use crate::base::ids;
use crate::error::{CbError, CbResult};
use crate::github::api::pulls;
use crate::github::{GhError, optional};
use crate::ops::AgentCtx;
use crate::protocol::acts::EvidenceMsg;
use crate::rules::Refusal;
use crate::rules::evidence::{CiFacts, PrFacts};
use crate::rules::protected::{MAX_FILES, PrFileList};

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
        ids::EVIDENCE_PR_FOREIGN_REPO,
        format!(
            "PR {}#{} is not in a repo of the initiative ({})",
            pr.repo,
            pr.number,
            repos.join(", ")
        ),
    ))
}

pub(super) fn fetch_pr(ac: &AgentCtx, repo: &str, number: u64) -> CbResult<(PrFacts, String)> {
    fetch_pr_based(ac, repo, number).map(|(pr, base)| (pr, base.branch))
}

/// The base of a PR: the branch that it merges into, and the default branch of its repo as the
/// PR answer names it (`None`: the answer has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrBase {
    pub branch: String,
    pub default: Option<String>,
}

/// [`fetch_pr`] with the default branch of the repo (the same one request).
pub(super) fn fetch_pr_based(
    ac: &AgentCtx,
    repo: &str,
    number: u64,
) -> CbResult<(PrFacts, PrBase)> {
    let Some(pr) = optional(pulls::get(&ac.ctx.client, repo, number))? else {
        return Err(CbError::refused(
            ids::EVIDENCE_PR_MISSING,
            format!("PR {repo}#{number} does not exist"),
        ));
    };
    let need = |v: Option<String>, p: &str| {
        v.ok_or_else(|| GhError::Shape(format!("PR answer has no {p}")))
    };
    let base = PrBase {
        branch: need(pr.base_ref, "/base/ref")?,
        default: pr.base_default,
    };
    Ok((
        PrFacts {
            repo: repo.to_string(),
            number,
            state: need(pr.state, "/state")?,
            merged: pr.merged,
            head_sha: need(pr.head_sha, "/head/sha")?,
            head_ref: need(pr.head_ref, "/head/ref")?,
            title: pr.title.unwrap_or_default(),
            body: pr.body.unwrap_or_default(),
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

/// [`fetch_pr_files`] that stops after the page that holds file `max` (100 per page).
/// `truncated` is true when the read stopped with a next page left.
pub(super) fn fetch_pr_files_upto(
    ac: &AgentCtx,
    repo: &str,
    number: u64,
    max: usize,
) -> CbResult<PrFileList> {
    let listed = pulls::files(&ac.ctx.client, repo, number, max.div_ceil(100).max(1))?;
    Ok(PrFileList {
        previous: listed
            .items
            .iter()
            .filter_map(|f| f.previous_filename.clone())
            .collect(),
        names: listed.items.into_iter().map(|f| f.filename).collect(),
        truncated: listed.truncated,
    })
}

/// The refusal for a file list that cannot be read: no proof that no file is protected or
/// guarded, so no pass (fail closed).
fn unreadable_files(repo: &str, number: u64, ticket: &str, why: &str) -> CbError {
    Refusal::new(
        ids::PROTECTED_PATH_OWNER_MERGE,
        format!(
            "the files of {repo}#{number} cannot be read ({why}); crewboard cannot prove that none is protected: try again, or the owner merges it by hand (its body closes the ticket), then run crewboard close {ticket}; without a closing line: crewboard close {ticket} --why \"<text>\""
        ),
    )
    .into()
}

/// The files that the base branch of a PR hides (spec/tickets#the-files-of-a-pr). The file list
/// of a PR is the diff against its base branch, and the author of the PR chooses that branch: a
/// branch that already holds the change of a protected or guarded file hides that file. So for
/// a PR whose base is not the default branch of its repo, this reads the files that differ
/// between the default branch and the commit `sha` (one request, the three-dot comparison), and
/// the caller joins them with the list of the PR. `None`: the base is the default branch, and
/// nothing is read. The list belongs to `sha`, whatever the head of the PR is now. A default
/// branch that the PR answer does not name is read from the repo (one more request). A
/// comparison that cannot be read is an error: the callers refuse. A list with `truncated` set
/// may be cut: the callers refuse it with [`cut_comparison`].
pub(super) fn files_hidden_by_base(
    ac: &AgentCtx,
    pr: &PrFacts,
    base: &PrBase,
    sha: &str,
) -> Result<Option<PrFileList>, GhError> {
    let default = match &base.default {
        Some(d) => d.clone(),
        None => ac
            .ctx
            .client
            .get(&format!("repos/{}", pr.repo))?
            .body
            .get("default_branch")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| GhError::Shape(format!("{} names no default branch", pr.repo)))?,
    };
    if base.branch == default {
        return Ok(None);
    }
    compare_files(&ac.ctx.client, &pr.repo, &default, sha).map(Some)
}

/// The refusal for a comparison with the default branch that may be cut: GitHub lists at most
/// [`COMPARE_MAX_FILES`] files of a comparison. `way_out` is the next step of the caller.
pub(super) fn cut_comparison(pr: &PrFacts, base: &PrBase, way_out: &str) -> Refusal {
    Refusal::new(
        ids::PROTECTED_PATH_OWNER_MERGE,
        format!(
            "PR {}#{} goes onto the branch {}, not onto the default branch, and its head differs from the default branch in {COMPARE_MAX_FILES} files or more. GitHub lists no more files of a comparison, so crewboard cannot prove that none is protected or guarded: {way_out}",
            pr.repo, pr.number, base.branch
        ),
    )
}

/// The most files that GitHub lists for one comparison of two commits.
const COMPARE_MAX_FILES: usize = 300;

/// The files that differ between the merge base of the branch `base` and `head` and the commit
/// `head` (the three-dot comparison `heads/<base>...<head>`), in the repo `repo`. The branch is
/// named as `heads/<base>`: a short name could mean a tag of the same name. `head` is a SHA, so the list
/// belongs to exactly that commit, and a commit of a fork needs no `owner:` form: the repo of a
/// PR holds the commits of its head. One request: GitHub gives the files of the whole
/// comparison with the first page, at most [`COMPARE_MAX_FILES`], and has no further page of
/// files. `truncated`: the list has that many files, so it may be cut.
fn compare_files(
    client: &crate::github::Client,
    repo: &str,
    base: &str,
    head: &str,
) -> Result<PrFileList, GhError> {
    let enc = |s: &str| crate::rules::text::url_encode(s, true);
    let path = format!(
        "repos/{repo}/compare/heads/{}...{}?per_page=1&page=1",
        enc(base),
        enc(head)
    );
    let r = client.get(&path)?;
    let files = r
        .body
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            GhError::Shape(format!(
                "the comparison {base}...{head} of {repo} has no file list"
            ))
        })?;
    let mut list = PrFileList::default();
    for f in files {
        let s = |k: &str| {
            f.get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        // a file without a name is a shape that proves nothing: no silent skip
        let filename = s("filename").ok_or_else(|| {
            GhError::Shape(format!(
                "a file of the comparison {base}...{head} of {repo} has no name"
            ))
        })?;
        list.names.push(filename);
        list.previous.extend(s("previous_filename"));
    }
    list.truncated = list.names.len() >= COMPARE_MAX_FILES;
    Ok(list)
}

/// The files of a PR for the rules `protected-path-owner-merge` and `guarded-path-changed`, as
/// `verify`, `close --verified` and `merge` read them. `pr` and `base` are the PR as the caller
/// read it just before.
///
/// - The list of the PR: up to one file more than [`crate::rules::protected::MAX_FILES`], so
///   that "more than that" is known.
/// - A PR whose base is not the default branch: joined with [`files_hidden_by_base`] for the
///   head `pr.head_sha`. A comparison that may be cut is refused here ([`cut_comparison`]).
/// - An open PR: the PR is read again after the lists (one request; none for a merged PR, whose
///   head and base do not change). The list of the PR is the diff of the head against the base
///   branch at the time of its read, and the choice "no comparison" rests on the base branch.
///   So a head or a base branch that is not the one of `pr` and `base` any more is
///   `evidence-head-moved` ([`check_same_pr`]).
///
/// A list that cannot be read is a refusal of the rule `protected-path-owner-merge`, not a pass
/// (fail closed). A cut list of the PR has `truncated` set, and the rules refuse it.
pub(crate) fn pr_files_for_protection(
    ac: &AgentCtx,
    pr: &PrFacts,
    base: &PrBase,
    ticket: &str,
) -> CbResult<PrFileList> {
    let (repo, number) = (pr.repo.as_str(), pr.number);
    let mut files = match fetch_pr_files_upto(ac, repo, number, MAX_FILES + 1) {
        Ok(f) => PrFileList {
            truncated: f.over(MAX_FILES),
            names: f.names,
            previous: f.previous,
        },
        Err(e) => return Err(unreadable_files(repo, number, ticket, &e.to_string())),
    };
    match files_hidden_by_base(ac, pr, base, &pr.head_sha) {
        Ok(Some(hidden)) if hidden.truncated => {
            return Err(cut_comparison(
                pr,
                base,
                &format!(
                    "the owner merges it by hand (its body closes the ticket), then run crewboard close {ticket}; without a closing line: crewboard close {ticket} --why \"<text>\""
                ),
            )
            .into());
        }
        Ok(Some(hidden)) => files.join(hidden),
        Ok(None) => {}
        Err(e) => {
            return Err(unreadable_files(
                repo,
                number,
                ticket,
                &format!("the comparison with the default branch: {e}"),
            ));
        }
    }
    // the head and the base of a merged PR do not change any more
    if !pr.merged {
        let (now, base_now) = fetch_pr_based(ac, repo, number)
            .map_err(|e| unreadable_files(repo, number, ticket, &e.to_string()))?;
        check_same_pr(pr, &base.branch, &now.head_sha, &base_now.branch)?;
    }
    Ok(files)
}

/// Rule `evidence-head-moved` for the file lists of a PR: after the read of the lists, the head
/// of the PR (`head_now`) and its base branch (`base_now`) are the ones that the command read
/// before (`pr`, `base`). A moved head: the list of the PR may belong to another commit. A
/// changed base branch: the list of the PR may be the diff against another branch, and the
/// choice to read no comparison with the default branch may be wrong. Pure.
pub(super) fn check_same_pr(
    pr: &PrFacts,
    base: &str,
    head_now: &str,
    base_now: &str,
) -> Result<(), Refusal> {
    if !head_now.eq_ignore_ascii_case(&pr.head_sha) {
        return Err(Refusal::new(
            ids::EVIDENCE_HEAD_MOVED,
            format!(
                "the head of PR {}#{} moved from {} to {head_now} while its files were read: the file list may belong to another commit. Nothing was written; run the command again",
                pr.repo, pr.number, pr.head_sha
            ),
        ));
    }
    if base_now != base {
        return Err(Refusal::new(
            ids::EVIDENCE_HEAD_MOVED,
            format!(
                "the base branch of PR {}#{} changed from {base} to {base_now} while its files were read: the file list may be the diff against another branch. Nothing was written; run the command again",
                pr.repo, pr.number
            ),
        ));
    }
    Ok(())
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
    files: Option<&PrFileList>,
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
    fn a_file_list_counts_only_for_the_head_and_the_base_that_the_command_read() {
        let pr = PrFacts {
            repo: MAIN.to_string(),
            number: 7,
            state: "open".into(),
            merged: false,
            head_sha: "abc123".into(),
            head_ref: "crew/a@h/5-x".into(),
            title: String::new(),
            body: String::new(),
        };
        assert!(check_same_pr(&pr, "main", "abc123", "main").is_ok());
        assert!(check_same_pr(&pr, "main", "ABC123", "main").is_ok());
        let r = check_same_pr(&pr, "main", "def456", "main").unwrap_err();
        assert_eq!(r.rule, ids::EVIDENCE_HEAD_MOVED);
        assert!(
            r.message.contains("moved from abc123 to def456"),
            "{}",
            r.message
        );
        // a base branch that changed while the files were read
        let r = check_same_pr(&pr, "main", "abc123", "stage").unwrap_err();
        assert_eq!(r.rule, ids::EVIDENCE_HEAD_MOVED);
        assert!(
            r.message.contains(
                "the base branch of PR example-org/example-repo#7 changed from main to stage"
            ),
            "{}",
            r.message
        );
        // a branch name is compared as it is
        assert!(check_same_pr(&pr, "main", "abc123", "Main").is_err());
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
