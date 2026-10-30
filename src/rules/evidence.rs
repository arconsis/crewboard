//! Evidence for `done` and `verify` (spec/tickets#evidence,
//! spec/tickets#verification-by-the-coordinator, spec/tickets#branches). Pure: the caller reads the
//! PR and the CI facts from GitHub and asks [`judge_evidence`].
//!
//! `done` needs `pr`, `sha` and `ci`:
//!
//! - the PR is in a repo of the initiative, and it is not closed without a merge;
//! - the PR body closes this ticket (8.5), and the branch is `crew/<agent>/<n>-<slug>` (8.5);
//! - the head of the PR is the SHA that the dev named (if it named one), and did not move
//!   while the evidence was collected;
//! - the CI facts of that SHA are green, judged by conclusion only. `ci: none` is allowed
//!   only if the ticket body declares `CI: none` (an owner decision).

use super::Refusal;
#[cfg(test)]
use super::Verdict;

/// A PR URL `https://github.com/<owner>/<repo>/pull/<n>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrUrl {
    pub repo: String,
    pub number: u64,
}

pub fn parse_pr_url(s: &str) -> Result<PrUrl, Refusal> {
    let bad = || {
        Refusal::new(
            "evidence-pr-url",
            format!("'{s}' is not a PR URL (https://github.com/<owner>/<repo>/pull/<n>)"),
        )
    };
    let rest = s
        .trim()
        .strip_prefix("https://github.com/")
        .ok_or_else(bad)?;
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [owner, repo, "pull", n] if !owner.is_empty() && !repo.is_empty() => {
            let number = n.parse::<u64>().map_err(|_| bad())?;
            if number == 0 {
                return Err(bad());
            }
            Ok(PrUrl {
                repo: format!("{owner}/{repo}"),
                number,
            })
        }
        _ => Err(bad()),
    }
}

/// What GitHub says about the PR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrFacts {
    pub repo: String,
    pub number: u64,
    /// `open` or `closed`.
    pub state: String,
    pub merged: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub title: String,
    pub body: String,
}

/// Which CI sources a judgement saw (for fine-grained tokens): a token
/// without `checks=read` cannot read check runs, but workflow runs (`actions=read`) and commit
/// statuses (`statuses=read`) may still be readable. The evidence names both lists.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiSources {
    /// Sources that were read: `workflow-runs`, `check-runs`, `statuses`.
    pub seen: Vec<String>,
    /// Sources that answered 403 (or 404 under `CI: none`).
    pub unreadable: Vec<String>,
    /// CI runs that counted (spec/ci#which-ci-runs-count): `9002 (push, success)`. Named runs end
    /// with `named`.
    pub counted: Vec<String>,
    /// CI runs that were information only (spec/ci#which-ci-runs-count): `9010 (schedule,
    /// failure)`.
    pub info: Vec<String>,
    /// Check runs that were ignored because they were `skipped` or `neutral` next to a
    /// successful check run (spec/ci#which-ci-runs-count): `desktop-app (skipped)`.
    pub ignored: Vec<String>,
    /// Jobs that the team lists in `ci_ignore` of AUTHORITY.md and the CI judgement did not
    /// count: `lint = failure`.
    pub config_ignored: Vec<String>,
}

impl CiSources {
    /// `counted: 9002 (push, success); information only: 9010 (schedule, failure); ignored:
    /// desktop-app (skipped)` (the part after `ci-runs: `). `None` when no run was seen.
    pub fn runs_text(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if !self.counted.is_empty() {
            parts.push(format!("counted: {}", self.counted.join(", ")));
        }
        if !self.info.is_empty() {
            parts.push(format!("information only: {}", self.info.join(", ")));
        }
        if !self.ignored.is_empty() {
            parts.push(format!("ignored: {}", self.ignored.join(", ")));
        }
        if !self.config_ignored.is_empty() {
            parts.push(format!(
                "ignored (AUTHORITY.md ci_ignore): {}",
                self.config_ignored.join(", ")
            ));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }

    /// `workflow-runs, statuses; unreadable: check-runs` (the part after `ci-sources: `).
    pub fn text(&self) -> String {
        if self.unreadable.is_empty() {
            self.seen.join(", ")
        } else {
            format!(
                "{}; unreadable: {}",
                self.seen.join(", "),
                self.unreadable.join(", ")
            )
        }
    }
}

/// What GitHub says about the CI of one SHA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiFacts {
    /// No workflow run, check run or status for the SHA in the sources that could be read,
    /// and no required check. Holds the names of the sources that could not be read.
    NoCi(Vec<String>),
    /// Runs of the SHA exist, but none is a `push` or `pull_request` run and none was named with
    /// `--run` (spec/ci#which-ci-runs-count): they are information, not CI. Treated like
    /// [`CiFacts::NoCi`], and the text names them.
    OnlyInformation {
        info: Vec<String>,
        unreadable: Vec<String>,
    },
    /// Every run, check and status that was read is completed with conclusion `success`
    /// (statuses: state `success`). `sources` names what was read and what was not.
    Green { run_url: String, sources: CiSources },
    /// At least one run or check is not completed or did not conclude `success`.
    NotGreen(Vec<String>),
    /// NO CI source could be read (HTTP 403, or 404 when the ticket declares `CI: none`, on
    /// workflow runs, check runs and statuses alike). The text names each source and the
    /// permission the token lacks. Refused as `evidence-ci-unreadable` unless the ticket
    /// declares `CI: none`. One readable source is enough to judge: then this is not used.
    NotQueried(String),
}

/// The checked evidence of a `done` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub pr_url: String,
    pub sha: String,
    /// A CI run URL, or `none`.
    pub ci: String,
    /// `workflow-runs, statuses; unreadable: check-runs`, when CI was judged green.
    pub ci_sources: Option<String>,
    /// `counted: ...; information only: ...` (spec/ci#which-ci-runs-count), when CI runs were
    /// judged green.
    pub ci_runs: Option<String>,
    /// The local gate of the dev, when the evidence holds one instead of a CI run
    /// (`ci_mode: local`, spec/ci#local-run-ci_mode-local).
    pub local_gate: Option<String>,
}

pub struct EvidenceInput<'a> {
    pub pr: &'a PrFacts,
    /// The SHA the dev says it worked on (`done --sha`), if it named one.
    pub claimed_sha: Option<&'a str>,
    /// The head SHA read from the PR a second time, after the CI facts were collected.
    pub head_after: &'a str,
    pub ci: &'a CiFacts,
    /// The ticket body has the line `CI: none`.
    pub ci_none_declared: bool,
    pub linked_repos: &'a [String],
    /// The ticket: repo and issue number.
    pub ticket_repo: &'a str,
    pub ticket_issue: u64,
    pub agent: &'a str,
    /// `dev_branch_prefix` of AUTHORITY.md, ending with `/`.
    pub branch_prefix: &'a str,
    /// The `done` goes the owner route: the PR holds the workflow change only as a
    /// patch, so it links the ticket with `Refs #n` and must not close it.
    pub owner_route: bool,
    /// For [`CiFacts::NoCi`] without `CI: none`: the judgement of `judge_no_run` (services,
    /// spec/ci#files-that-every-workflow-ignores). `Ok` is the evidence text
    /// `none (all files paths-ignored: ...)`, `Err` the reason that is added to the refusal.
    /// `None`: not judged (the refusal stays as it was).
    pub no_run: Option<&'a Result<String, String>>,
    /// `ci_mode: local` and the evidence is a local gate (spec/ci#local-run-ci_mode-local):
    /// the text of the gate. The CI facts are NOT judged then (`ci` is information only); the
    /// PR, head and branch checks stay as they are.
    pub local_gate: Option<&'a str>,
    /// The head branch that a signed `adopts-pr:` line of a coordinator in the
    /// creation info of this ticket allows for this PR (spec/tickets#a-follow-up-ticket-continues-an-open-pr).
    /// The branch is named for an older ticket, so [`branch_ok`] would refuse it. `None`: the
    /// ticket adopted no PR, or the PR is not the adopted one.
    pub adopted_branch: Option<&'a str>,
}

/// The likely cause of "no CI run for this SHA" (spec/ci#ci-must-run-for-crew-branches). A typical case:
/// CI had no trigger for crew branches, and a branch made before a CI change gets no run until it
/// merges the base branch (for a push, GitHub uses the workflow file of the pushed commit).
pub fn no_ci_cause(branch_prefix: &str) -> String {
    format!(
        "Likely cause: no workflow ran for this SHA. Check that a workflow triggers on a push to {branch_prefix}** or on a pull_request. If the CI file changed after you made the branch, merge the base branch into it and push"
    )
}

/// The branch of a ticket PR (spec/tickets#branches): `<dev_branch_prefix><agent>/<issue>-<slug>`
/// (`prefix` ends with `/`; AUTHORITY.md, default `crew/`). The slug is `[A-Za-z0-9._-]`, at most
/// 100 bytes, no leading `-` or `.` and no `..` ([`crate::rules::text::slug_ok`]): git allows
/// much more, but a branch name goes into a command line and a URL path, so it must be one safe
/// word.
pub fn branch_ok(prefix: &str, head_ref: &str, agent: &str, issue: u64) -> bool {
    let want = format!("{prefix}{agent}/{issue}-");
    head_ref
        .strip_prefix(&want)
        .is_some_and(crate::rules::text::slug_ok)
}

/// The agent and the ticket number that a crew branch names
/// (`<prefix><agent>/<issue>-<slug>`), or `None` when the name has another shape.
pub fn branch_ticket<'a>(prefix: &str, head_ref: &'a str) -> Option<(&'a str, u64)> {
    let rest = head_ref.strip_prefix(prefix)?;
    let (agent, tail) = rest.split_once('/')?;
    let (num, slug) = tail.split_once('-')?;
    if !crate::rules::text::branch_agent_ok(agent)
        || !crate::rules::text::slug_ok(slug)
        || num.is_empty()
        || !num.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let n = num.parse::<u64>().ok().filter(|n| *n > 0)?;
    Some((agent, n))
}

/// The head branch is the one that a coordinator signed for this ticket, and it
/// is still under `dev_branch_prefix`.
pub fn adopted_branch_ok(prefix: &str, head_ref: &str, adopted: Option<&str>) -> bool {
    adopted == Some(head_ref) && head_ref.starts_with(prefix)
}

/// The wire word of `ci: none`.
pub const CI_NONE: &str = "none";

/// What proves CI for the head of a PR: the four kinds of the CI proof (spec/ci). One value,
/// made by [`judge_ci`], so no caller compares evidence strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CiProof {
    /// The counted runs of the head SHA are green. `run_url` is the evidence word, `sources` says
    /// what was read, `runs` names the counted runs (when there are any).
    Runs {
        run_url: String,
        sources: String,
        runs: Option<String>,
    },
    /// The ticket body declares `CI: none`: there is no CI to rely on.
    None,
    /// No run, and every workflow ignores all changed files. The text is the evidence word
    /// `none (all files paths-ignored: ...)`.
    Skipped(String),
    /// `ci_mode: local`: the dev carries a local run, the CI facts are information only.
    Local,
}

impl CiProof {
    /// The value of the `ci:` evidence line (wire v1, unchanged).
    pub fn text(&self) -> String {
        match self {
            CiProof::Runs { run_url, .. } => run_url.clone(),
            CiProof::None => CI_NONE.to_string(),
            CiProof::Skipped(text) => text.clone(),
            CiProof::Local => super::text::LOCAL_GATE_CI.to_string(),
        }
    }
}

/// The one judgement of "what proves CI for this head": a local run first (`ci_mode: local`),
/// then green runs, then the declared `CI: none`, then the paths-ignored case. A red or
/// unreadable CI is refused; `CI: none` never hides a red run.
pub fn judge_ci(i: &EvidenceInput<'_>) -> Result<CiProof, Refusal> {
    let head = &i.pr.head_sha;
    // spec/ci#files-that-every-workflow-ignores: no run, and every workflow ignores the files.
    let paths_ignored = match i.no_run {
        Some(Ok(text)) => Some(text.clone()),
        _ => None,
    };
    match i.ci {
        _ if i.local_gate.is_some() => Ok(CiProof::Local),
        CiFacts::Green { run_url, sources } => Ok(CiProof::Runs {
            run_url: run_url.clone(),
            sources: sources.text(),
            runs: sources.runs_text(),
        }),
        CiFacts::NotGreen(reasons) => Err(Refusal::new(
            "evidence-ci-not-green",
            format!("CI of {head} is not green: {}", reasons.join("; ")),
        )),
        CiFacts::NoCi(_) | CiFacts::OnlyInformation { .. } if i.ci_none_declared => {
            Ok(CiProof::None)
        }
        CiFacts::OnlyInformation { info, unreadable } => Err(Refusal::new(
            "evidence-ci-missing",
            format!(
                "no counted CI run for {head}: only runs that are information (not push or pull_request): {}{}. Name a run that is part of the Gate with `--run <id>`. {}",
                info.join(", "),
                if unreadable.is_empty() {
                    String::new()
                } else {
                    format!(" (unreadable: {})", unreadable.join(", "))
                },
                no_ci_cause(i.branch_prefix)
            ),
        )),
        CiFacts::NoCi(_) if paths_ignored.is_some() => {
            Ok(CiProof::Skipped(paths_ignored.unwrap_or_default()))
        }
        CiFacts::NoCi(unread) => Err(Refusal::new(
            "evidence-ci-missing",
            format!(
                "no CI run for {head}, and the ticket body does not declare 'CI: none'{}.{} {}",
                if unread.is_empty() {
                    String::new()
                } else {
                    format!(" (unreadable: {})", unread.join(", "))
                },
                match i.no_run {
                    Some(Err(why)) => format!(" Not accepted as paths-ignored: {why}."),
                    _ => String::new(),
                },
                no_ci_cause(i.branch_prefix)
            ),
        )),
        CiFacts::NotQueried(_) if i.ci_none_declared => Ok(CiProof::None),
        CiFacts::NotQueried(why) => Err(Refusal::new(
            "evidence-ci-unreadable",
            format!("CI cannot be read ({why}) and the ticket does not declare 'CI: none'"),
        )),
    }
}

pub fn judge_evidence(i: &EvidenceInput<'_>) -> Result<Evidence, Refusal> {
    let pr = i.pr;
    if !i
        .linked_repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&pr.repo))
    {
        return Err(Refusal::new(
            "evidence-pr-foreign-repo",
            format!(
                "PR {}#{} is not in a repo of the initiative ({})",
                pr.repo,
                pr.number,
                i.linked_repos.join(", ")
            ),
        ));
    }
    if pr.state == "closed" && !pr.merged {
        return Err(Refusal::new(
            "evidence-pr-closed",
            format!("PR {}#{} is closed without a merge", pr.repo, pr.number),
        ));
    }
    if i.owner_route {
        // The PR links the ticket and does not close it (the check for a closing
        // keyword ran before, in `done`).
        let text = format!("{}\n{}", pr.title, pr.body);
        if !super::text::mentions_ticket(&text, &pr.repo, i.ticket_repo, i.ticket_issue) {
            return Err(Refusal::new(
                "evidence-pr-not-linked",
                format!(
                    "the PR must link the ticket with 'Refs #{}' (or 'Refs {}#{}')",
                    i.ticket_issue, i.ticket_repo, i.ticket_issue
                ),
            ));
        }
    } else if !super::text::closes_ticket(&pr.body, &pr.repo, i.ticket_repo, i.ticket_issue) {
        return Err(Refusal::new(
            "evidence-pr-not-linked",
            format!(
                "the PR body must say 'Closes #{}' (or 'Closes {}#{}'){}",
                i.ticket_issue,
                i.ticket_repo,
                i.ticket_issue,
                if i.adopted_branch.is_some() {
                    ". This PR was adopted by the ticket from an older one: the dev agent edits the PR body so that it closes this ticket"
                } else {
                    ""
                }
            ),
        ));
    }
    if !branch_ok(i.branch_prefix, &pr.head_ref, i.agent, i.ticket_issue)
        && !adopted_branch_ok(i.branch_prefix, &pr.head_ref, i.adopted_branch)
    {
        return Err(Refusal::new(
            "evidence-branch-name",
            format!(
                "branch '{}' is not {}{}/{}-<slug>",
                pr.head_ref, i.branch_prefix, i.agent, i.ticket_issue
            ),
        ));
    }
    if let Some(c) = i.claimed_sha
        && !c.eq_ignore_ascii_case(&pr.head_sha)
    {
        return Err(Refusal::new(
            "evidence-head-moved",
            format!(
                "the PR head is {}, not {c}: it moved after the work that you name",
                pr.head_sha
            ),
        ));
    }
    if !i.head_after.eq_ignore_ascii_case(&pr.head_sha) {
        return Err(Refusal::new(
            "evidence-head-moved",
            format!(
                "the PR head moved from {} to {} while the evidence was collected",
                pr.head_sha, i.head_after
            ),
        ));
    }
    let local_gate = i.local_gate.map(str::to_string);
    let proof = judge_ci(i)?;
    let (ci_sources, ci_runs) = match &proof {
        CiProof::Runs { sources, runs, .. } => (Some(sources.clone()), runs.clone()),
        CiProof::None | CiProof::Skipped(_) | CiProof::Local => (None, None),
    };
    let ci = proof.text();
    Ok(Evidence {
        pr_url: format!("https://github.com/{}/pull/{}", pr.repo, pr.number),
        sha: pr.head_sha.clone(),
        ci,
        ci_sources,
        ci_runs,
        local_gate,
    })
}

/// Check that a recorded evidence is complete (all three parts present and non-empty). Only the
/// tests of this file call it.
#[cfg(test)]
pub fn check_complete(pr: &str, sha: &str, ci: &str) -> Verdict {
    if pr.is_empty() || sha.is_empty() || ci.is_empty() {
        return Err(Refusal::new(
            "status-evidence-missing",
            "evidence needs pr, sha and ci",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn pr() -> PrFacts {
        PrFacts {
            repo: "example-org/app".into(),
            number: 7,
            state: "open".into(),
            merged: false,
            head_sha: SHA.into(),
            head_ref: "crew/hermes@b/12-fix-it".into(),
            title: "Work".into(),
            body: "Does the work.\n\nCloses #12\n".into(),
        }
    }

    fn input<'a>(
        pr: &'a PrFacts,
        ci: &'a CiFacts,
        declared: bool,
        repos: &'a [String],
    ) -> EvidenceInput<'a> {
        EvidenceInput {
            pr,
            claimed_sha: None,
            head_after: SHA,
            ci,
            ci_none_declared: declared,
            linked_repos: repos,
            ticket_repo: "example-org/app",
            ticket_issue: 12,
            agent: "hermes@b",
            branch_prefix: "crew/",
            owner_route: false,
            no_run: None,
            local_gate: None,
            adopted_branch: None,
        }
    }

    fn repos() -> Vec<String> {
        vec!["example-org/app".into(), "example-org/app-2".into()]
    }

    fn green() -> CiFacts {
        CiFacts::Green {
            run_url: "https://github.com/example-org/app/actions/runs/1".into(),
            sources: CiSources {
                seen: vec!["workflow-runs".into(), "check-runs".into()],
                unreadable: vec![],
                ..CiSources::default()
            },
        }
    }

    #[test]
    fn judge_ci_names_the_four_kinds_and_their_evidence_words() {
        let (p, r) = (pr(), repos());
        let g = green();
        let runs = judge_ci(&input(&p, &g, false, &r)).unwrap();
        assert!(
            matches!(&runs, CiProof::Runs { sources, .. } if sources == "workflow-runs, check-runs")
        );
        assert_eq!(
            runs.text(),
            "https://github.com/example-org/app/actions/runs/1"
        );
        let none = CiFacts::NoCi(vec![]);
        let declared = judge_ci(&input(&p, &none, true, &r)).unwrap();
        assert_eq!(declared, CiProof::None);
        assert_eq!(declared.text(), CI_NONE);
        let ignored: Result<String, String> = Ok("none (all files paths-ignored: docs/**)".into());
        let mut i = input(&p, &none, false, &r);
        i.no_run = Some(&ignored);
        let skipped = judge_ci(&i).unwrap();
        assert_eq!(skipped.text(), "none (all files paths-ignored: docs/**)");
        assert!(matches!(skipped, CiProof::Skipped(_)));
        let mut i = input(&p, &g, false, &r);
        i.local_gate = Some("3 runs green");
        let local = judge_ci(&i).unwrap();
        assert_eq!(local, CiProof::Local);
        assert_eq!(local.text(), crate::rules::text::LOCAL_GATE_CI);
    }

    #[test]
    fn judge_ci_refuses_red_and_unread_ci_and_never_hides_red_behind_ci_none() {
        let (p, r) = (pr(), repos());
        let red = CiFacts::NotGreen(vec!["build failed".into()]);
        for declared in [false, true] {
            let e = judge_ci(&input(&p, &red, declared, &r)).unwrap_err();
            assert_eq!(e.rule, "evidence-ci-not-green");
        }
        let none = CiFacts::NoCi(vec![]);
        let e = judge_ci(&input(&p, &none, false, &r)).unwrap_err();
        assert_eq!(e.rule, "evidence-ci-missing");
        let unread = CiFacts::NotQueried("403".into());
        let e = judge_ci(&input(&p, &unread, false, &r)).unwrap_err();
        assert_eq!(e.rule, "evidence-ci-unreadable");
        assert_eq!(
            judge_ci(&input(&p, &unread, true, &r)).unwrap(),
            CiProof::None
        );
    }

    fn rule(r: Result<Evidence, Refusal>) -> &'static str {
        r.expect_err("must be refused").rule
    }

    #[test]
    fn pr_urls_parse_and_bad_ones_are_refused() {
        assert_eq!(
            parse_pr_url("https://github.com/example-org/app/pull/7").unwrap(),
            PrUrl {
                repo: "example-org/app".into(),
                number: 7
            }
        );
        assert!(parse_pr_url("https://github.com/a/b/pull/7/").is_ok());
        for bad in [
            "",
            "https://github.com/a/b/issues/7",
            "https://github.com/a/b/pull/x",
            "https://github.com/a/b/pull/0",
            "http://github.com/a/b/pull/7",
            "https://evil.example/a/b/pull/7",
            "https://github.com/a/pull/7",
            "https://github.com/a/b/pull/7/files",
        ] {
            assert_eq!(
                parse_pr_url(bad).unwrap_err().rule,
                "evidence-pr-url",
                "{bad}"
            );
        }
    }

    #[test]
    fn green_ci_is_accepted_and_names_the_run() {
        let (p, c, r) = (pr(), green(), repos());
        let e = judge_evidence(&input(&p, &c, false, &r)).unwrap();
        assert_eq!(e.sha, SHA);
        assert_eq!(e.ci, "https://github.com/example-org/app/actions/runs/1");
        assert_eq!(e.pr_url, "https://github.com/example-org/app/pull/7");
    }

    #[test]
    fn the_missing_ci_refusal_names_the_sources_that_could_not_be_read_and_only_those() {
        let (p, r) = (pr(), repos());
        let msg = |c: CiFacts| {
            judge_evidence(&input(&p, &c, false, &r))
                .unwrap_err()
                .to_string()
        };
        let with = msg(CiFacts::NoCi(vec!["check-runs".into()]));
        assert!(with.contains("(unreadable: check-runs)"), "{with}");
        let without = msg(CiFacts::NoCi(vec![]));
        assert!(!without.contains("unreadable"), "{without}");
    }

    #[test]
    fn no_run_with_all_files_paths_ignored_is_accepted_and_a_reason_is_added_to_the_refusal() {
        // spec/ci#files-that-every-workflow-ignores
        let (p, r) = (pr(), repos());
        let nothing = CiFacts::NoCi(vec![]);
        let ok: Result<String, String> = Ok("none (all files paths-ignored: docs/**)".into());
        let mut i = input(&p, &nothing, false, &r);
        i.no_run = Some(&ok);
        let e = judge_evidence(&i).unwrap();
        assert_eq!(e.ci, "none (all files paths-ignored: docs/**)");
        // refusal direction: the reason is in the old refusal
        let no: Result<String, String> =
            Err("file src/a.rs is not ignored by workflow ci.yml".into());
        i.no_run = Some(&no);
        let e = judge_evidence(&i).unwrap_err();
        assert_eq!(e.rule, "evidence-ci-missing");
        assert!(
            e.message.contains(
                "Not accepted as paths-ignored: file src/a.rs is not ignored by workflow ci.yml."
            ) && e.message.contains("no workflow ran for this SHA"),
            "{}",
            e.message
        );
        // a red CI is never turned green by the judgement
        let red = CiFacts::NotGreen(vec!["run 1 (ci) concluded failure".into()]);
        let mut j = input(&p, &red, false, &r);
        j.no_run = Some(&ok);
        assert_eq!(rule(judge_evidence(&j)), "evidence-ci-not-green");
        // `CI: none` still records plain `none`
        let mut d = input(&p, &nothing, true, &r);
        d.no_run = Some(&ok);
        assert_eq!(judge_evidence(&d).unwrap().ci, "none");
    }

    #[test]
    fn ci_none_is_accepted_only_when_the_ticket_declares_it() {
        let (p, r) = (pr(), repos());
        let e = judge_evidence(&input(&p, &CiFacts::NoCi(vec![]), true, &r)).unwrap();
        assert_eq!(e.ci, "none");
        assert_eq!(
            rule(judge_evidence(&input(
                &p,
                &CiFacts::NoCi(vec![]),
                false,
                &r
            ))),
            "evidence-ci-missing"
        );
    }

    #[test]
    fn the_missing_ci_refusal_names_the_likely_cause_and_the_prefix() {
        let (p, r) = (pr(), repos());
        let e = judge_evidence(&input(&p, &CiFacts::NoCi(vec![]), false, &r)).unwrap_err();
        assert_eq!(e.rule, "evidence-ci-missing");
        assert!(
            e.message.contains("no workflow ran for this SHA")
                && e.message.contains("merge the base branch"),
            "{}",
            e.message
        );
        assert!(
            e.message.contains("**"),
            "names the prefix glob: {}",
            e.message
        );
        // control: a green CI has no such text
        assert!(judge_evidence(&input(&p, &green(), false, &r)).is_ok());
    }

    #[test]
    fn a_ci_that_cannot_be_read_counts_only_with_the_declaration() {
        let (p, r) = (pr(), repos());
        let c = CiFacts::NotQueried("HTTP 403".into());
        assert_eq!(judge_evidence(&input(&p, &c, true, &r)).unwrap().ci, "none");
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-ci-unreadable"
        );
        // the refusal names the permission and is a refusal (exit 2), not an error (exit 3)
        let c = CiFacts::NotQueried("check runs: HTTP 403, token lacks checks=read".into());
        let e = judge_evidence(&input(&p, &c, false, &r)).unwrap_err();
        assert!(
            e.message.contains("token lacks checks=read"),
            "{}",
            e.message
        );
        let cb: crate::error::CbError = e.into();
        assert_eq!(cb.exit() as i32, 2);
        let net: crate::error::CbError = crate::github::GhError::Http {
            status: 500,
            message: "boom".into(),
            accepted_permissions: None,
        }
        .into();
        assert_eq!(net.exit() as i32, 3);
    }

    #[test]
    fn red_ci_is_refused_even_when_the_ticket_declares_ci_none() {
        let (p, r) = (pr(), repos());
        let c = CiFacts::NotGreen(vec!["run 5 concluded failure".into()]);
        for declared in [true, false] {
            let e = judge_evidence(&input(&p, &c, declared, &r)).unwrap_err();
            assert_eq!(e.rule, "evidence-ci-not-green");
            assert!(e.message.contains("run 5 concluded failure"));
        }
    }

    #[test]
    fn a_pr_in_a_foreign_repo_is_refused() {
        let (mut p, c, r) = (pr(), green(), repos());
        p.repo = "someone/else".into();
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-pr-foreign-repo"
        );
        // control: the second repo of the initiative is fine (cross-repo closing form)
        let mut p2 = pr();
        p2.repo = "example-org/app-2".into();
        p2.body = "Closes example-org/app#12".into();
        assert!(judge_evidence(&input(&p2, &c, false, &r)).is_ok());
    }

    #[test]
    fn a_closed_unmerged_pr_is_refused_and_a_merged_one_passes() {
        let (mut p, c, r) = (pr(), green(), repos());
        p.state = "closed".into();
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-pr-closed"
        );
        p.merged = true;
        assert!(judge_evidence(&input(&p, &c, false, &r)).is_ok());
    }

    #[test]
    fn the_pr_head_that_moved_is_refused_in_both_ways_of_moving() {
        let (p, c, r) = (pr(), green(), repos());
        // the dev names an older SHA
        let mut i = input(&p, &c, false, &r);
        i.claimed_sha = Some("ffffffffffffffffffffffffffffffffffffffff");
        assert_eq!(rule(judge_evidence(&i)), "evidence-head-moved");
        // control: naming the head itself passes (any letter case)
        let upper = SHA.to_uppercase();
        i.claimed_sha = Some(&upper);
        assert!(judge_evidence(&i).is_ok());
        // the head moves while the CI facts are collected
        let mut j = input(&p, &c, false, &r);
        j.head_after = "ffffffffffffffffffffffffffffffffffffffff";
        assert_eq!(rule(judge_evidence(&j)), "evidence-head-moved");
    }

    #[test]
    fn the_pr_body_must_close_this_ticket() {
        use super::super::text::closes_ticket;
        assert!(closes_ticket("Closes #12", "a/b", "a/b", 12));
        assert!(closes_ticket("fixes #12.", "a/b", "a/b", 12));
        assert!(closes_ticket("Resolves: #12", "a/b", "a/b", 12));
        assert!(closes_ticket("closes A/B#12", "x/y", "a/b", 12));
        for (body, repo) in [
            ("Closes #13", "a/b"),
            ("Closes #120", "a/b"),
            ("See #12", "a/b"),
            ("closes #12 in a roundabout way", "other/repo"),
            ("Closes #12", "other/repo"), // short form in another repo means another issue
            ("unclosed #12", "a/b"),
            ("", "a/b"),
        ] {
            assert!(!closes_ticket(body, repo, "a/b", 12), "{body}");
        }
        let (mut p, c, r) = (pr(), green(), repos());
        p.body = "no link".into();
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-pr-not-linked"
        );
    }

    #[test]
    fn the_normal_route_reads_every_closing_form_of_the_owner_route() {
        // the same parser as the owner route: the URL, a colon, owner/repo#n
        let (mut p, c, r) = (pr(), green(), repos());
        for body in [
            "Closes https://github.com/example-org/app/issues/12",
            "Fixes: #12",
            "closes example-org/app#12",
        ] {
            p.body = body.into();
            assert!(judge_evidence(&input(&p, &c, false, &r)).is_ok(), "{body}");
        }
        for body in [
            "Closes #13",
            "Fixes: https://github.com/example-org/app/issues/13",
        ] {
            p.body = body.into();
            assert_eq!(
                rule(judge_evidence(&input(&p, &c, false, &r))),
                "evidence-pr-not-linked",
                "{body}"
            );
        }
    }

    #[test]
    fn the_branch_must_be_crew_agent_issue_slug() {
        assert!(branch_ok(
            "crew/",
            "crew/hermes@b/12-fix-it",
            "hermes@b",
            12
        ));
        for bad in [
            "main",
            "crew/hermes@b/12-",
            "crew/hermes@b/13-fix",
            "crew/other@c/12-fix",
            "feature/12-fix",
            "crew/hermes@b/120-fix",
        ] {
            assert!(!branch_ok("crew/", bad, "hermes@b", 12), "{bad}");
        }
        // the prefix comes from AUTHORITY.md (dev_branch_prefix), it is not hard-coded
        assert!(branch_ok("team/", "team/hermes@b/12-fix", "hermes@b", 12));
        assert!(!branch_ok("team/", "crew/hermes@b/12-fix", "hermes@b", 12));
        assert!(!branch_ok("crew/", "team/hermes@b/12-fix", "hermes@b", 12));
        let (mut p, c, r) = (pr(), green(), repos());
        p.head_ref = "my-branch".into();
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-branch-name"
        );
    }

    #[test]
    fn a_branch_prefix_from_authority_md_is_enforced_and_named() {
        let (p, c, r) = (pr(), green(), repos());
        let mut i = input(&p, &c, false, &r);
        i.branch_prefix = "team/";
        let e = judge_evidence(&i).unwrap_err();
        assert_eq!(e.rule, "evidence-branch-name");
        assert!(
            e.message.contains("team/hermes@b/12-<slug>"),
            "{}",
            e.message
        );
        // control: the matching branch passes under that prefix
        let mut p2 = pr();
        p2.head_ref = "team/hermes@b/12-fix-it".into();
        let mut i2 = input(&p2, &c, false, &r);
        i2.branch_prefix = "team/";
        assert!(judge_evidence(&i2).is_ok());
    }

    #[test]
    fn green_evidence_records_which_ci_sources_were_seen_and_which_were_not() {
        let (p, r) = (pr(), repos());
        let c = CiFacts::Green {
            run_url: "https://x/1".into(),
            sources: CiSources {
                seen: vec!["workflow-runs".into(), "statuses".into()],
                unreadable: vec!["check-runs".into()],
                ..CiSources::default()
            },
        };
        let e = judge_evidence(&input(&p, &c, false, &r)).unwrap();
        assert_eq!(
            e.ci_sources.as_deref(),
            Some("workflow-runs, statuses; unreadable: check-runs")
        );
        // nothing unreadable: no "unreadable" part; no green CI: no sources line
        let all = green();
        let e = judge_evidence(&input(&p, &all, false, &r)).unwrap();
        assert_eq!(e.ci_sources.as_deref(), Some("workflow-runs, check-runs"));
        let none = judge_evidence(&input(&p, &CiFacts::NoCi(vec![]), true, &r)).unwrap();
        assert_eq!(none.ci_sources, None);
    }

    #[test]
    fn only_information_runs_are_the_missing_ci_refusal_and_name_the_runs() {
        let (p, r) = (pr(), repos());
        let c = CiFacts::OnlyInformation {
            info: vec!["5 (workflow_dispatch, success)".into()],
            unreadable: vec![],
        };
        let e = judge_evidence(&input(&p, &c, false, &r)).unwrap_err();
        assert_eq!(e.rule, "evidence-ci-missing");
        assert!(
            e.message.contains("5 (workflow_dispatch, success)"),
            "{}",
            e.message
        );
        assert!(e.message.contains("--run"), "{}", e.message);
        assert!(e.message.contains(&no_ci_cause("crew/")), "{}", e.message);
        // the ticket declares `CI: none`: the same as no CI
        let ok = judge_evidence(&input(&p, &c, true, &r)).unwrap();
        assert_eq!(ok.ci, "none");
    }

    #[test]
    fn green_evidence_records_which_runs_counted_and_which_were_information() {
        let (p, r) = (pr(), repos());
        let c = CiFacts::Green {
            run_url: "https://x/1".into(),
            sources: CiSources {
                counted: vec!["1 (push, success)".into()],
                info: vec!["2 (schedule, failure)".into()],
                ..CiSources::default()
            },
        };
        let e = judge_evidence(&input(&p, &c, false, &r)).unwrap();
        assert_eq!(
            e.ci_runs.as_deref(),
            Some("counted: 1 (push, success); information only: 2 (schedule, failure)")
        );
        assert_eq!(
            judge_evidence(&input(&p, &green(), false, &r))
                .unwrap()
                .ci_runs,
            None
        );
    }

    #[test]
    fn a_local_gate_replaces_the_ci_judgement_and_every_ci_state_passes() {
        let (p, r) = (pr(), repos());
        let gate = "host-a: make test rc=0, log /tmp/gate.log";
        for ci in [
            CiFacts::NotGreen(vec!["run 1 failed".into()]),
            CiFacts::NoCi(vec![]),
            CiFacts::NoCi(vec!["check-runs".into()]),
            CiFacts::OnlyInformation {
                info: vec!["2 (schedule, failure)".into()],
                unreadable: vec![],
            },
            CiFacts::NotQueried("HTTP 403".into()),
            green(),
        ] {
            let mut i = input(&p, &ci, false, &r);
            // without the local gate the same facts are judged as before
            let plain = judge_evidence(&i);
            i.local_gate = Some(gate);
            let e = judge_evidence(&i).expect("local gate passes");
            assert_eq!(e.ci, "local-gate");
            assert_eq!(e.local_gate.as_deref(), Some(gate));
            assert_eq!((e.ci_sources, e.ci_runs), (None, None));
            if matches!(ci, CiFacts::NotGreen(_) | CiFacts::NoCi(_)) {
                assert!(plain.is_err(), "{ci:?}");
            }
        }
    }

    #[test]
    fn a_local_gate_keeps_the_pr_head_and_branch_checks() {
        let r = repos();
        let ci = CiFacts::NotGreen(vec!["x".into()]);
        let gate = Some("host-a: make test rc=0");
        let mut p = pr();
        p.body = "no keyword".into();
        let mut i = input(&p, &ci, false, &r);
        i.local_gate = gate;
        assert_eq!(rule(judge_evidence(&i)), "evidence-pr-not-linked");
        let mut p = pr();
        p.head_ref = "other/branch".into();
        let mut i = input(&p, &ci, false, &r);
        i.local_gate = gate;
        assert_eq!(rule(judge_evidence(&i)), "evidence-branch-name");
        let p = pr();
        let mut i = input(&p, &ci, false, &r);
        i.local_gate = gate;
        i.claimed_sha = Some("ffffffffffffffffffffffffffffffffffffffff");
        assert_eq!(rule(judge_evidence(&i)), "evidence-head-moved");
        let mut i = input(&p, &ci, false, &r);
        i.local_gate = gate;
        i.head_after = "ffffffffffffffffffffffffffffffffffffffff";
        assert_eq!(rule(judge_evidence(&i)), "evidence-head-moved");
        let mut p = pr();
        p.state = "closed".into();
        let mut i = input(&p, &ci, false, &r);
        i.local_gate = gate;
        assert_eq!(rule(judge_evidence(&i)), "evidence-pr-closed");
    }

    #[test]
    fn complete_evidence_needs_all_three_parts() {
        assert!(check_complete("u", "s", "none").is_ok());
        for (a, b, c) in [("", "s", "c"), ("p", "", "c"), ("p", "s", "")] {
            assert_eq!(
                check_complete(a, b, c).unwrap_err().rule,
                "status-evidence-missing"
            );
        }
    }

    #[test]
    fn a_hostile_branch_slug_is_refused() {
        for bad in [
            "crew/a@h/1-x;touch${IFS}p",
            "crew/a@h/1-x/%2e%2e/main",
            "crew/a@h/1-x/../main",
            "crew/a@h/1-x y",
            "crew/a@h/1-$(id)",
            "crew/a@h/1-`id`",
            "crew/a@h/1-x|y",
            "crew/a@h/1-x&y",
            "crew/a@h/1-'x'",
            "crew/a@h/1--x",
            "crew/a@h/1-.x",
            "crew/a@h/1-x..y",
            "crew/a@h/1-x%2Fy",
            "crew/a@h/1-x#y",
            "crew/a@h/1-x\ny",
        ] {
            assert!(!branch_ok("crew/", bad, "a@h", 1), "branch_ok {bad}");
            assert_eq!(branch_ticket("crew/", bad), None, "branch_ticket {bad}");
        }
        let long = format!("crew/a@h/1-{}", "x".repeat(101));
        assert!(!branch_ok("crew/", &long, "a@h", 1));
        assert_eq!(branch_ticket("crew/", &long), None);
        let fine = format!("crew/a@h/1-{}", "x".repeat(100));
        assert!(branch_ok("crew/", &fine, "a@h", 1));
        assert!(branch_ok("crew/", "crew/a@h/1-v1.2_fix-it", "a@h", 1));
    }

    #[test]
    fn the_judgement_refuses_a_hostile_branch_with_the_branch_rule() {
        let (mut p, c, r) = (pr(), green(), repos());
        p.head_ref = "crew/hermes@b/12-x;touch${IFS}p".into();
        assert_eq!(
            rule(judge_evidence(&input(&p, &c, false, &r))),
            "evidence-branch-name"
        );
    }

    #[test]
    fn a_crew_branch_names_its_agent_and_ticket() {
        assert_eq!(
            branch_ticket("crew/", "crew/hermes@b/12-fix-it"),
            Some(("hermes@b", 12))
        );
        assert_eq!(
            branch_ticket("team/x/", "team/x/claude@a/7-s-t"),
            Some(("claude@a", 7))
        );
        // every other shape is refused
        for bad in [
            "main",
            "crew/hermes@b/12",      // no slug
            "crew/hermes@b/12-",     // empty slug
            "crew/hermes@b/x12-fix", // not a number
            "crew/hermes@b/0-fix",   // issue 0
            "crew//12-fix",          // no agent
            "other/hermes@b/12-fix", // another prefix
            "crew/hermes@b-12-fix",  // no slash
        ] {
            assert_eq!(branch_ticket("crew/", bad), None, "{bad}");
        }
    }

    #[test]
    fn an_adopted_branch_passes_only_when_it_is_the_signed_one_under_the_prefix() {
        assert!(adopted_branch_ok(
            "crew/",
            "crew/a@b/5-x",
            Some("crew/a@b/5-x")
        ));
        assert!(!adopted_branch_ok(
            "crew/",
            "crew/a@b/5-x",
            Some("crew/a@b/6-x")
        ));
        assert!(!adopted_branch_ok("crew/", "crew/a@b/5-x", None));
        // a signed branch outside the prefix does not count
        assert!(!adopted_branch_ok("crew/", "main", Some("main")));
    }

    #[test]
    fn the_branch_of_an_older_ticket_is_refused_unless_adopted() {
        let repos = repos();
        let ci = green();
        let p = PrFacts {
            head_ref: "crew/hermes@b/5-old".into(),
            body: "Closes #12".into(),
            ..pr()
        };
        let i = input(&p, &ci, false, &repos);
        // refusal: the plain rule
        assert_eq!(judge_evidence(&i).unwrap_err().rule, "evidence-branch-name");
        // pass: the same PR with the signed branch
        let adopted = EvidenceInput {
            adopted_branch: Some("crew/hermes@b/5-old"),
            ..input(&p, &ci, false, &repos)
        };
        assert!(judge_evidence(&adopted).is_ok());
        // refusal: another branch signed
        let other = EvidenceInput {
            adopted_branch: Some("crew/hermes@b/6-old"),
            ..input(&p, &ci, false, &repos)
        };
        assert_eq!(
            judge_evidence(&other).unwrap_err().rule,
            "evidence-branch-name"
        );
        // the body must still close THIS ticket, and the refusal says how to fix it
        let wrong_body = PrFacts {
            body: "Closes #5".into(),
            ..p.clone()
        };
        let e = judge_evidence(&EvidenceInput {
            adopted_branch: Some("crew/hermes@b/5-old"),
            ..input(&wrong_body, &ci, false, &repos)
        })
        .unwrap_err();
        assert_eq!(e.rule, "evidence-pr-not-linked");
        assert!(e.message.contains("edits the PR body"), "{}", e.message);
        let plain = judge_evidence(&input(&wrong_body, &ci, false, &repos)).unwrap_err();
        assert!(!plain.message.contains("edits the PR body"));
    }
}
