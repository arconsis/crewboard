//! `merge <issue>` (coordinator): merge the PR of a verified ticket.
//!
//! Allowed only when AUTHORITY.md lets the coordinator merge (`coordinator_may_merge: true`
//! and no `merge` in `needs_owner`). The evidence checks are the ones of `done` and `verify`
//! ([`judge_evidence`]): the PR is the one that the signed `done` names, its head is the
//! verified SHA (a head that moved is refused), and the CI of that SHA is green. The merge
//! call carries the verified SHA, so GitHub also refuses a head that moves after the check.
//! Then the branch under `dev_branch_prefix` is deleted, the ticket moves to Done and a
//! signed message goes to the dev agent.
//!
//! Order: pure checks, reads, evidence, then the writes. The merge is the one write that
//! cannot be undone, so it is the first write.

use super::local_gate::local_gate_not_enabled_for_evidence;
use super::pr_evidence::{check_pr_in_initiative, fetch_pr, fetch_pr_files};
use super::{
    Me, check_verify_ticket, load_world_cached, me_of, needs_owner_lines, post_with,
    question_open_since, remember, to_agent,
};
use crate::authority::Authority;
use crate::base::ids;
use crate::clock::{Clock, SystemClock};
use crate::domain::ci::evaluate::CiRuns;
use crate::error::{CbError, CbResult};
use crate::github::GhError;
use crate::ops::{AgentCtx, parse_issue_arg};
use crate::protocol::names::{Kind, Re};
use crate::rules::Refusal;
use crate::rules::evidence::{CiFacts, PrFacts, PrUrl, branch_ticket, parse_pr_url};
use crate::rules::evidence::{Evidence, EvidenceInput, judge_evidence};
use crate::rules::status::{DoneReason, Status, Transition, check_status_transition};
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::{
    EvidenceMsg, ci_none_declared, is_local_gate, latest_done, passed_coordinator_local_gate,
    question_open, verify_result,
};
use serde_json::Value;
use serde_json::json;

/// The merge methods of GitHub.
pub fn parse_method(m: &str) -> Result<&'static str, Refusal> {
    match m.trim().to_ascii_lowercase().as_str() {
        "squash" => Ok("squash"),
        "merge" => Ok("merge"),
        "rebase" => Ok("rebase"),
        other => Err(Refusal::new(
            ids::MERGE_METHOD_UNKNOWN,
            format!("merge method '{other}' is not one of squash, merge, rebase"),
        )),
    }
}

/// What [`judge_merge`] needs. All facts, no network.
pub struct MergeInput<'a> {
    pub auth: &'a Authority,
    pub status: Status,
    /// The time of the question to the owner that is open on the ticket, if one is
    /// ([`crate::ticket::question_open`]). A ticket in Review with an open question is not merged.
    pub question_open_since: Option<&'a str>,
    /// The latest verify result for the latest `done`: `Some(true)` passed.
    pub verify: Option<bool>,
    pub pr: &'a PrFacts,
    /// The SHA of the signed `done` evidence.
    pub evidence_sha: &'a str,
    pub ci: &'a CiFacts,
    pub ci_none_declared: bool,
    pub linked_repos: &'a [String],
    pub ticket_repo: &'a str,
    pub ticket_issue: u64,
    pub agent: &'a str,
    /// The judgement for a head without a CI run (spec/ci#files-that-every-workflow-ignores),
    /// as for `done`.
    pub no_run: Option<&'a Result<String, String>>,
    /// The evidence is a local gate (`ci: local-gate`, spec/ci#local-run-ci_mode-local): the
    /// local-gate line of the dev's `done` (empty if it has none). `None`: the evidence has CI.
    pub evidence_local_gate: Option<&'a str>,
    /// The `coordinator-local-gate:` line of the PASSED verify of that `done` on the same head
    /// ([`crate::ticket::passed_coordinator_local_gate`]).
    pub coordinator_local_gate: Option<&'a str>,
    /// The branch that the ticket adopted with a signed `adopts-pr:` line.
    pub adopted_branch: Option<&'a str>,
    /// The protected paths among the files of the PR ([`crate::rules::protected::protected_hits`]).
    pub protected: &'a [String],
    /// The PR has more files than crewboard reads: it cannot prove that none is protected.
    pub files_truncated: bool,
    /// The path of the authority file (`cfg.authority`), for the refusal text.
    pub authority_path: &'a str,
}

/// Every check before the merge call, in order: authority, status, verify, the PR is open,
/// then the evidence rules of `done` (head equals the verified SHA, CI green).
pub fn judge_merge(i: &MergeInput<'_>) -> Result<Evidence, Refusal> {
    i.auth.check_merge()?;
    // Rule `protected-path-owner-merge`: whatever AUTHORITY.md says, the owner merges a PR that
    // changes a protected path.
    crate::rules::protected::check_hits(
        &format!("https://github.com/{}/pull/{}", i.pr.repo, i.pr.number),
        &format!("{}#{}", i.ticket_repo, i.ticket_issue),
        i.protected,
        i.files_truncated,
        crate::rules::protected::Protect::new(i.authority_path, &i.auth.protected_paths),
    )?;
    if i.evidence_local_gate.is_some() && !i.auth.ci_mode.is_local() {
        return Err(local_gate_not_enabled_for_evidence());
    }
    if i.status != Status::Review {
        return Err(Refusal::new(
            ids::MERGE_NEEDS_REVIEW,
            format!(
                "merge needs a ticket in Review, this one is {}",
                i.status.name()
            ),
        ));
    }
    crate::rules::status::check_no_open_question(
        &format!("{}#{}", i.ticket_repo, i.ticket_issue),
        i.question_open_since,
    )?;
    if i.verify != Some(true) {
        return Err(Refusal::new(
            ids::MERGE_VERIFY_REQUIRED,
            "run `crewboard verify` first: merge needs a passed verify for the latest done message",
        ));
    }
    if i.pr.merged || i.pr.state != "open" {
        return Err(Refusal::new(
            ids::MERGE_PR_NOT_OPEN,
            format!(
                "PR {}#{} is {}; there is nothing to merge ({})",
                i.pr.repo,
                i.pr.number,
                if i.pr.merged {
                    "merged already"
                } else {
                    "closed"
                },
                if i.pr.merged {
                    "`close --verified` ends the ticket"
                } else {
                    "a new `done` with an open PR, or `close --why` drops the ticket"
                }
            ),
        ));
    }
    // spec/ci#local-run-ci_mode-local: a local gate passes the CI part only if ci_mode is still
    // `local`, the `done` has its gate line and the passed verify has the coordinator's line on
    // the same head.
    let local_gate = match i.evidence_local_gate {
        None => None,
        Some(dev) => {
            if dev.trim().is_empty() {
                return Err(Refusal::new(
                    ids::LOCAL_GATE_MISSING,
                    "the evidence is 'ci: local-gate' but the done message has no local-gate line",
                ));
            }
            if i.coordinator_local_gate.is_none_or(|t| t.trim().is_empty()) {
                return Err(Refusal::new(
                    ids::LOCAL_GATE_MISSING,
                    format!(
                        "merge needs the coordinator's local gate: the passed verify of {} has no `coordinator-local-gate` line on this head. Run `crewboard verify {} --local-run \"<host, command, rc, log>\"` again",
                        i.evidence_sha, i.ticket_issue
                    ),
                ));
            }
            Some(dev)
        }
    };
    judge_evidence(&EvidenceInput {
        pr: i.pr,
        claimed_sha: Some(i.evidence_sha),
        head_after: &i.pr.head_sha,
        ci: i.ci,
        ci_none_declared: i.ci_none_declared,
        linked_repos: i.linked_repos,
        ticket_repo: i.ticket_repo,
        ticket_issue: i.ticket_issue,
        agent: i.agent,
        branch_prefix: &i.auth.branch_prefix(),
        owner_route: false,
        no_run: i.no_run,
        local_gate,
        adopted_branch: i.adopted_branch,
    })
}

/// What the mergeability of a PR allows. See [`judge_mergeable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mergeable {
    /// Go on to the merge call.
    Go,
    /// Refuse with `pr-not-mergeable`; the state is named. `conflict` is true for DIRTY: the
    /// dev gets a request to rebase.
    Refuse { state: String, conflict: bool },
    /// GitHub has not computed the state yet: read again.
    Unknown,
}

/// Map the GitHub `mergeable_state` of a PR (REST; the same values as GraphQL
/// `mergeStateStatus`, in any case) to what `merge` does:
///
/// | state | action |
/// |---|---|
/// | clean, has_hooks | go on |
/// | unstable | go on (the checks of the verified SHA are green already, see [`judge_merge`]; only a non-required check fails) |
/// | blocked | go on: GitHub's own refusal of the merge call is the guard (405 maps to `pr-not-mergeable`) |
/// | draft | refuse, no request (GitHub's draft text is not a "not mergeable" text) |
/// | dirty | refuse, and ask the dev to rebase |
/// | behind | refuse |
/// | unknown, empty, null | read again |
/// | any other value | refuse, naming it |
pub fn judge_mergeable(state: &str) -> Mergeable {
    match state.trim().to_ascii_lowercase().as_str() {
        "clean" | "has_hooks" | "unstable" | "blocked" => Mergeable::Go,
        "draft" => Mergeable::Refuse {
            state: "DRAFT".into(),
            conflict: false,
        },
        "dirty" => Mergeable::Refuse {
            state: "DIRTY".into(),
            conflict: true,
        },
        "behind" => Mergeable::Refuse {
            state: "BEHIND".into(),
            conflict: false,
        },
        "" | "unknown" => Mergeable::Unknown,
        other => Mergeable::Refuse {
            state: other.to_ascii_uppercase(),
            conflict: false,
        },
    }
}

/// How often the state is read while GitHub still computes it, and the wait between reads.
pub const MERGEABLE_READS: u32 = 4;
pub const MERGEABLE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Read the state until it is known (at most `reads` reads, `wait` between them). After the
/// last unknown the answer is the refusal "state unknown, try again".
pub fn resolve_mergeable<E>(
    reads: u32,
    mut wait: impl FnMut(),
    mut read: impl FnMut() -> Result<String, E>,
) -> Result<Result<Mergeable, Refusal>, E> {
    for i in 0..reads {
        if i > 0 {
            wait();
        }
        match judge_mergeable(&read()?) {
            Mergeable::Unknown => {}
            Mergeable::Go => return Ok(Ok(Mergeable::Go)),
            r @ Mergeable::Refuse { .. } => return Ok(Ok(r)),
        }
    }
    Ok(Err(Refusal::new(
        ids::PR_NOT_MERGEABLE,
        "the mergeable state of the PR is UNKNOWN (GitHub still computes it): state unknown, try again",
    )))
}

/// The command that delivers the updated branch from Review. `ticket` is the
/// issue argument of the ticket (`<n>` or `<owner>/<repo>#<n>`), `pr` the PR URL.
pub fn redeliver_command(ticket: &str, pr: &str) -> String {
    format!("crewboard done {ticket} --pr {pr} --sha <new head>")
}

/// The refusal for a PR that cannot be merged. `ticket` and `pr` name the re-delivery command
/// ([`redeliver_command`]).
pub fn not_mergeable(ticket: &str, pr: &str, state: &str) -> Refusal {
    if state == "DRAFT" {
        return Refusal::new(
            ids::PR_NOT_MERGEABLE,
            format!(
                "PR {pr} is not mergeable (mergeStateStatus DRAFT): the PR is a draft; mark it ready for review"
            ),
        );
    }
    Refusal::new(
        ids::PR_NOT_MERGEABLE,
        format!(
            "PR {pr} is not mergeable: mergeStateStatus {state}. Nothing was merged; the dev agent rebases or merges the base branch, pushes and runs `{}` (the ticket stays in Review). Then run verify and merge again",
            redeliver_command(ticket, pr)
        ),
    )
}

/// The signed `request` to the dev agent on a conflict. `files` are the conflicting files if
/// known; `from_pr_files` says they are only the files the PR changes (the fallback). The
/// request names the re-delivery command ([`redeliver_command`]).
pub fn rebase_request_body(
    ticket: &str,
    pr: &str,
    base: &str,
    files: &[String],
    from_pr_files: bool,
) -> String {
    let list = if files.is_empty() {
        "files: (could not be listed)".to_string()
    } else {
        let src = if from_pr_files {
            "files the PR changes (the conflicting files could not be listed cheaply; check these first)"
        } else {
            "conflicting files"
        };
        let shown: Vec<&str> = files.iter().take(30).map(String::as_str).collect();
        let more = files.len().saturating_sub(shown.len());
        format!(
            "{src}:\n- {}{}",
            shown.join("\n- "),
            if more > 0 {
                format!("\n- ... and {more} more")
            } else {
                String::new()
            }
        )
    };
    format!(
        "PR {pr} conflicts with {base} (mergeStateStatus DIRTY), so the coordinator did not merge it.\nPlease rebase onto {base} (or merge {base} into the branch), resolve the conflicts and push. Then run `{}`: the ticket stays in Review and the coordinator verifies the new head.\n{list}",
        redeliver_command(ticket, pr)
    )
}

/// GitHub's own refusal of the merge call. 405/409 whose message says the PR is not
/// mergeable (conflict, base moved) is `pr-not-mergeable`; a moved head stays
/// `merge-head-moved`; other 405/422 stay `merge-not-allowed` (for example a method that the
/// repo forbids).
pub fn classify_merge_refusal(status: u16, message: &str, pr: &str, sha: &str) -> Option<Refusal> {
    let m = message.to_ascii_lowercase();
    let not_mergeable_msg = m.contains("not mergeable")
        || m.contains("merge conflict")
        || m.contains("base branch was modified");
    match status {
        405 | 409 if not_mergeable_msg => Some(Refusal::new(
            ids::PR_NOT_MERGEABLE,
            format!("GitHub refused the merge of {pr}: the PR is not mergeable ({message})"),
        )),
        409 => Some(Refusal::new(
            ids::MERGE_HEAD_MOVED,
            format!("GitHub refused the merge: the PR head is not {sha} any more ({message})"),
        )),
        405 | 422 => Some(Refusal::new(
            ids::MERGE_NOT_ALLOWED,
            format!("GitHub refused the merge of {pr}: {message}"),
        )),
        _ => None,
    }
}

/// The merge call: path and body. `sha` is the verified head: GitHub refuses (409) if the
/// head moved.
pub fn merge_request(repo: &str, pr: u64, sha: &str, method: &str) -> (String, Value) {
    (
        format!("repos/{repo}/pulls/{pr}/merge"),
        json!({"sha": sha, "merge_method": method}),
    )
}

/// Delete the branch only if it is under `dev_branch_prefix` and not protected, and the PR
/// comes from the same repo (a fork branch of the same name is not ours to delete). The branch
/// must have the shape of a ticket branch with a safe slug ([`branch_ticket`]), and the path
/// holds it percent-encoded per segment: the URL parser folds a dot segment, also `%2e%2e`, so
/// a raw name could make the request reach another ref.
pub fn branch_to_delete(
    auth: &Authority,
    repo: &str,
    head_repo: Option<&str>,
    head_ref: &str,
) -> Option<String> {
    let same_repo = head_repo.is_some_and(|h| h.eq_ignore_ascii_case(repo));
    (same_repo
        && head_ref.starts_with(&auth.branch_prefix())
        && branch_ticket(&auth.branch_prefix(), head_ref).is_some()
        && auth.check_head_branch(head_ref).is_ok())
    .then(|| {
        format!(
            "repos/{repo}/git/refs/heads/{}",
            crate::rules::text::url_encode(head_ref, true)
        )
    })
}

/// What `merge` has loaded before it decides: the agent and the ticket.
struct MergeLoad {
    repo: String,
    n: u64,
    me: Me,
    t: Ticket,
    from: Status,
}

/// The reads of `merge` that come before any decision about the PR.
fn load_merge(ac: &AgentCtx, issue_arg: &str) -> CbResult<MergeLoad> {
    let (repo, n) = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    t.check_agent_signed()?;
    let from = t.status()?;
    Ok(MergeLoad {
        repo,
        n,
        me,
        t,
        from,
    })
}

/// The checks of `merge` that need no PR: authority, headless limits, the evidence of the `done`.
struct MergePre {
    auth: Authority,
    headless: bool,
    assignee: String,
    ev: EvidenceMsg,
    ev_local: bool,
    /// The latest verify result for the latest `done`.
    verify: Option<bool>,
    coordinator_gate: Option<String>,
    pr_url: PrUrl,
    named: CiRuns,
}

fn precheck_merge(ac: &AgentCtx, l: &MergeLoad, runs: &[String]) -> CbResult<MergePre> {
    let t = &l.t;
    // The headless boundary for an open question to the owner, then the verify checks.
    check_verify_ticket(ac, t, l.from, &l.me, false)?;
    // spec/board#authority: AUTHORITY.md, read now, before any call about the PR.
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    auth.check_merge()?;
    // spec/agents#headless-coordinator: a headless coordinator merges only a ticket whose Gate is
    // CI only. The check is here, in the operation, and not only in `dispatch`, so that no other
    // caller skips it.
    let headless = ac.ctx.cd.is_headless()?;
    if headless {
        crate::headless::check_merge_ticket(
            &t.key(),
            crate::ticket::signed_gate(&t.msgs).as_deref(),
            ci_none_declared(&t.msgs, &t.info.body),
        )?;
    }
    let assignee = t.agent().map(str::to_string).ok_or_else(|| {
        CbError::refused(
            ids::VERIFY_NO_EVIDENCE,
            format!("{} has no assignee", t.key()),
        )
    })?;
    let (done_msg, ev) = latest_done(&t.msgs, &assignee).ok_or_else(|| {
        CbError::refused(
            ids::VERIFY_NO_EVIDENCE,
            format!(
                "{} has no signed done message of {assignee} with pr, sha and ci",
                t.key()
            ),
        )
    })?;
    let ev_local = is_local_gate(&ev);
    check_evidence_for_merge(headless, &ev, ev_local, auth.ci_mode.is_local())?;
    let verify = verify_result(&t.msgs, done_msg, l.n);
    let coordinator_gate = passed_coordinator_local_gate(&t.msgs, done_msg, &ev, l.n);
    let mut pr_url = parse_pr_url(&ev.pr)?;
    super::pr_evidence::follow_former_name(
        &mut pr_url,
        &done_msg.ts,
        ac.ctx.main_repo(),
        &auth.former_repos,
    );
    check_pr_in_initiative(&ac.ctx.cfg.repos, &pr_url)?;
    let named = if ev_local {
        CiRuns::new(Vec::new(), Vec::new())
    } else {
        super::pr_evidence::evidence_runs(ac, &pr_url.repo, &ev, runs)?
    };
    Ok(MergePre {
        auth,
        headless,
        assignee,
        ev,
        ev_local,
        verify,
        coordinator_gate,
        pr_url,
        named,
    })
}

/// What the evidence of the `done` allows, before any read of the PR. Pure.
///
/// spec/agents#headless-coordinator: evidence without a CI run (`ci: none`, also `none (all
/// files paths-ignored: ...)`) has no CI to rely on, so the Gate is not CI only. Evidence that names a CI run by hand (`done --run`) is part of a Gate that is not CI only
/// either. spec/ci#local-run-ci_mode-local: a headless coordinator leaves a local gate (a
/// judgment); in `ci_mode: ci` it does not count any more.
fn check_evidence_for_merge(
    headless: bool,
    ev: &EvidenceMsg,
    ev_local: bool,
    ci_mode_local: bool,
) -> Result<(), Refusal> {
    if headless && crate::domain::ci::triggers::evidence_has_no_run(&ev.ci) {
        return Err(Refusal::new(
            ids::HEADLESS_SCOPE,
            format!(
                "this config is headless: the evidence is 'ci: {}', so no CI run proves the change and the Gate is not CI only. Leave the merge for the interactive coordinator and note it",
                ev.ci
            ),
        ));
    }
    if headless
        && ev
            .ci_runs
            .as_deref()
            .is_some_and(crate::domain::ci::evaluate::names_runs)
    {
        return Err(Refusal::new(
            ids::HEADLESS_SCOPE,
            "this config is headless: the evidence names a CI run by hand (`named` in its ci-runs line), so the Gate is not CI only. Leave the merge for the interactive coordinator and note it",
        ));
    }
    if ev_local {
        if headless {
            return Err(Refusal::new(
                ids::HEADLESS_SCOPE,
                "this config is headless: the evidence is a local gate (`ci: local-gate`), so the Gate is not CI only. Leave the merge for the interactive coordinator and note it",
            ));
        }
        if !ci_mode_local {
            return Err(local_gate_not_enabled_for_evidence());
        }
    }
    Ok(())
}

/// A headless coordinator merges only on a counted CI run that is green for the verified SHA.
fn check_headless_ci(headless: bool, ci: &CiFacts) -> Result<(), Refusal> {
    if headless && !matches!(ci, CiFacts::Green { .. }) {
        return Err(Refusal::new(
            ids::HEADLESS_SCOPE,
            "this config is headless: merge needs a counted CI run that is green for the verified SHA. Leave the merge for the interactive coordinator and note it",
        ));
    }
    Ok(())
}

/// What `merge` reads about the PR and its CI.
struct MergeRead {
    pr: PrFacts,
    base: String,
    files_truncated: bool,
    protected: Vec<String>,
    adopted: Option<String>,
    declared: bool,
    ci: CiFacts,
    no_run: Option<Result<String, String>>,
}

fn read_merge(ac: &AgentCtx, l: &MergeLoad, pre: &MergePre) -> CbResult<MergeRead> {
    let (t, ev) = (&l.t, &pre.ev);
    let (pr, base) = fetch_pr(ac, &pre.pr_url.repo, pre.pr_url.number)?;
    // Rule `protected-path-owner-merge`: the current file list of the PR, before any CI read. A
    // refusal writes nothing (`judge_merge` checks the same facts again).
    let files = super::pr_evidence::pr_files_for_protection(ac, &pr.repo, pr.number, &t.key())?;
    let guard =
        crate::rules::protected::Protect::new(&ac.ctx.cfg.authority, &pre.auth.protected_paths);
    let protected = crate::rules::protected::protected_hits(&files.all(), guard);
    crate::rules::protected::check_hits(&ev.pr, &t.key(), &protected, files.truncated, guard)?;
    // A PR that the ticket adopted from an older ticket keeps its branch name.
    let adopted = crate::ticket::adopted_branch(&t.msgs, &pre.pr_url.repo, pre.pr_url.number);
    let declared = ci_none_declared(&t.msgs, &t.info.body);
    let ci_none = ev.ci == "none";
    // The named runs of the evidence count again, like in `done` and `verify`. A
    // named run of the evidence that does not hold any more refuses the merge. A local gate has no CI
    // to judge.
    let ci = if pre.ev_local {
        CiFacts::NoCi(Vec::new())
    } else {
        let q = CiQuery {
            repo: &pr.repo,
            sha: &ev.sha,
            base: &base,
            declared: ci_none && declared,
            named: &pre.named.extra,
            evidence: &pre.named.evidence,
            ignore: &pre.auth.ci_ignore,
        };
        let (ci, run_findings) = fetch_ci(&|p| ac.ctx.client.get(p), &q)?;
        if let Some(r) = run_findings.into_iter().next() {
            return Err(r.into());
        }
        ci
    };
    // spec/ci#files-that-every-workflow-ignores: the `done` judgement again, for the verified SHA.
    let no_run = if pre.headless || pre.ev_local {
        None
    } else {
        super::pr_evidence::judge_no_run_for(
            ac,
            &ci,
            ci_none && declared,
            &pr,
            &ev.sha,
            &base,
            None,
        )?
    };
    check_headless_ci(pre.headless, &ci)?;
    Ok(MergeRead {
        pr,
        base,
        files_truncated: files.truncated,
        protected,
        adopted,
        declared,
        ci,
        no_run,
    })
}

/// The branch of the PR after the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BranchStep {
    /// Delete it: the path of the ref.
    Delete(String),
    /// Keep it; the line of the answer.
    Keep(String),
}

/// The facts [`merge_steps`] needs. All read already; no network.
struct StepFacts<'a> {
    auth: &'a Authority,
    ticket_key: String,
    assignee: &'a str,
    method: &'static str,
    ev: &'a EvidenceMsg,
    ev_local: bool,
    coordinator_gate: Option<&'a str>,
    pr_repo: &'a str,
    pr_number: u64,
    head_ref: &'a str,
    head_repo: Option<&'a str>,
    /// The ticket has the label `needs-owner`.
    labelled_needs_owner: bool,
    /// A question to the owner is still open on the ticket.
    question_open: bool,
}

/// The writes after the merge is decided. Plain data; [`act_merge`] performs them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeSteps {
    ticket_key: String,
    assignee: String,
    pr_repo: String,
    pr_number: u64,
    pr_url: String,
    sha: String,
    method: &'static str,
    head_ref: String,
    branch: BranchStep,
    /// The label `needs-owner` goes with the close.
    clear_needs_owner: bool,
    /// The notice to the dev agent, up to the merge commit and from it on.
    notice: (String, String),
}

impl MergeSteps {
    /// The text of the notice to the dev agent, once the merge commit is known.
    fn notice_body(&self, merge_sha: &str) -> String {
        format!("{}{merge_sha}{}", self.notice.0, self.notice.1)
    }
}

/// Decide the steps after a merge: the branch, the label, the notice. Pure.
fn merge_steps(f: &StepFacts<'_>) -> MergeSteps {
    let prefix = f.auth.branch_prefix();
    let branch = match branch_to_delete(f.auth, f.pr_repo, f.head_repo, f.head_ref) {
        Some(path) => BranchStep::Delete(path),
        None => BranchStep::Keep(format!(
            "branch {} not deleted (not under {prefix} in {}, or protected)\n",
            f.head_ref, f.pr_repo
        )),
    };
    // spec/ci#local-run-ci_mode-local: the notice names the gates that stood in for CI.
    let local_lines = if f.ev_local {
        format!(
            "\nci-mode: local\nlocal-gate: {}\ncoordinator-local-gate: {}",
            f.ev.local_gate.as_deref().unwrap_or(""),
            f.coordinator_gate.unwrap_or("")
        )
    } else {
        String::new()
    };
    MergeSteps {
        ticket_key: f.ticket_key.clone(),
        assignee: f.assignee.to_string(),
        pr_repo: f.pr_repo.to_string(),
        pr_number: f.pr_number,
        pr_url: f.ev.pr.clone(),
        sha: f.ev.sha.clone(),
        method: f.method,
        head_ref: f.head_ref.to_string(),
        branch,
        clear_needs_owner: f.labelled_needs_owner && !f.question_open,
        notice: (
            crate::protocol::acts::merged_head(&f.ev.pr, &f.ev.sha, f.method),
            format!("\nbranch: {}{local_lines}", f.head_ref),
        ),
    }
}

/// What `merge` does once the checks passed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum MergeAction {
    /// The PR conflicts: post the request to rebase to the dev agent, then refuse.
    AskRebase {
        refusal: Refusal,
        assignee: String,
        request: String,
    },
    /// Merge the PR, delete the branch, close the ticket, tell the dev agent.
    Merge(Box<MergeSteps>),
}

/// The checks of the PR and the ticket move before the merge call ([`judge_merge`], then the
/// status table, so a refusal there leaves the PR alone).
fn judge_before_merge(
    ac: &AgentCtx,
    l: &MergeLoad,
    pre: &MergePre,
    read: &MergeRead,
) -> CbResult<()> {
    let t = &l.t;
    let question_since = question_open_since(t);
    judge_merge(&MergeInput {
        auth: &pre.auth,
        status: l.from,
        question_open_since: question_since.as_deref(),
        verify: pre.verify,
        pr: &read.pr,
        evidence_sha: &pre.ev.sha,
        ci: &read.ci,
        ci_none_declared: read.declared,
        linked_repos: &ac.ctx.cfg.repos,
        ticket_repo: &l.repo,
        ticket_issue: l.n,
        agent: &pre.assignee,
        no_run: read.no_run.as_ref(),
        evidence_local_gate: pre
            .ev_local
            .then(|| pre.ev.local_gate.as_deref().unwrap_or("")),
        coordinator_local_gate: pre.coordinator_gate.as_deref(),
        adopted_branch: read.adopted.as_deref(),
        protected: &read.protected,
        files_truncated: read.files_truncated,
        authority_path: &ac.ctx.cfg.authority,
    })?;
    check_status_transition(Some(l.from), Status::Done, l.me.actor, &close_transition())?;
    Ok(())
}

/// The move to Done after a passed verify.
fn close_transition() -> Transition {
    Transition::Close {
        reason: Some(DoneReason::VerifiedMerge),
        verify: Some(true),
    }
}

/// Mergeability: one read (REST mergeable_state), before the merge call. A conflict is
/// refused here, with a request to the dev; no message says "merged". Reads again while GitHub
/// still computes the state.
fn plan_mergeable(
    ac: &AgentCtx,
    l: &MergeLoad,
    pre: &MergePre,
    read: &MergeRead,
    clock: &dyn Clock,
) -> CbResult<Option<MergeAction>> {
    let pr = &read.pr;
    let state = resolve_mergeable(
        MERGEABLE_READS,
        || clock.sleep(MERGEABLE_WAIT),
        || -> CbResult<String> {
            let r = ac
                .ctx
                .client
                .get(&format!("repos/{}/pulls/{}", pr.repo, pr.number))?;
            Ok(r.body
                .get("mergeable_state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string())
        },
    )?;
    let (state, conflict) = match state {
        Ok(Mergeable::Refuse { state, conflict }) => (state, conflict),
        Ok(_) => return Ok(None),
        Err(r) => return Err(r.into()),
    };
    // The issue argument of the ticket: the bare number in the main repo.
    let ticket = if l.repo.eq_ignore_ascii_case(ac.ctx.main_repo()) {
        l.n.to_string()
    } else {
        l.t.key()
    };
    let refusal = not_mergeable(&ticket, &pre.ev.pr, &state);
    if !conflict {
        return Err(refusal.into());
    }
    // The PR files list is the cheap fallback: the conflicting files themselves
    // need a local merge-tree, which the coordinator does not have.
    let files = fetch_pr_files(ac, &pr.repo, pr.number).unwrap_or_default();
    Ok(Some(MergeAction::AskRebase {
        refusal,
        assignee: pre.assignee.clone(),
        request: rebase_request_body(&ticket, &pre.ev.pr, &read.base, &files, true),
    }))
}

/// Read everything `merge` needs and decide: no write.
fn plan_merge(
    ac: &AgentCtx,
    l: &MergeLoad,
    method: &'static str,
    runs: &[String],
    clock: &dyn Clock,
) -> CbResult<MergeAction> {
    let t = &l.t;
    let pre = precheck_merge(ac, l, runs)?;
    let read = read_merge(ac, l, &pre)?;
    judge_before_merge(ac, l, &pre, &read)?;
    if let Some(action) = plan_mergeable(ac, l, &pre, &read, clock)? {
        return Ok(action);
    }
    let head_repo = ac
        .ctx
        .client
        .get(&format!("repos/{}/pulls/{}", read.pr.repo, read.pr.number))?
        .body
        .pointer("/head/repo/full_name")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(MergeAction::Merge(Box::new(merge_steps(&StepFacts {
        auth: &pre.auth,
        ticket_key: t.key(),
        assignee: &pre.assignee,
        method,
        ev: &pre.ev,
        ev_local: pre.ev_local,
        coordinator_gate: pre.coordinator_gate.as_deref(),
        pr_repo: &read.pr.repo,
        pr_number: read.pr.number,
        head_ref: &read.pr.head_ref,
        head_repo: head_repo.as_deref(),
        labelled_needs_owner: t
            .info
            .labels
            .iter()
            .any(|x| x == crate::labels::NEEDS_OWNER),
        question_open: question_open(&t.msgs, t.agent()).is_some(),
    }))))
}

/// Perform a [`MergeAction`], in order, and return the answer.
fn act_merge(
    ac: &AgentCtx,
    l: &MergeLoad,
    action: &MergeAction,
    clock: &dyn Clock,
) -> CbResult<String> {
    match action {
        MergeAction::AskRebase {
            refusal,
            assignee,
            request,
        } => {
            let p = post_with(
                ac,
                &l.t,
                clock,
                to_agent(assignee)?,
                Kind::Request,
                Re::None,
                request,
            )?;
            let mut why = refusal.clone();
            why.message
                .push_str(&format!(". Request {} sent to {assignee}", p.id));
            Err(why.into())
        }
        MergeAction::Merge(steps) => act_merge_steps(ac, l, steps, clock),
    }
}

/// The merge call: the one write that cannot be undone, so it is the first write.
fn put_merge(ac: &AgentCtx, s: &MergeSteps) -> CbResult<String> {
    let (path, body) = merge_request(&s.pr_repo, s.pr_number, &s.sha, s.method);
    let merged = match ac.ctx.client.send("PUT", &path, Some(&body)) {
        Ok(r) => r.body,
        Err(e) => {
            // 405, 409 and 422 are refusals of the merge with their own names
            if let GhError::Http {
                status: status @ (405 | 409 | 422),
                message,
                ..
            } = &e
                && let Some(r) = classify_merge_refusal(*status, message, &s.pr_url, &s.sha)
            {
                return Err(r.into());
            }
            return Err(e.into());
        }
    };
    Ok(merged
        .get("sha")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string())
}

/// The line of the answer about the branch, after the delete call if there is one.
fn act_branch(ac: &AgentCtx, s: &MergeSteps) -> String {
    match &s.branch {
        BranchStep::Delete(p) => match ac.ctx.client.send("DELETE", p, None) {
            Ok(_) => format!("branch {} deleted\n", s.head_ref),
            // 404 and 422: already gone (GitHub can delete the head branch itself)
            Err(GhError::Http {
                status: 404 | 422, ..
            }) => format!("branch {} was already gone\n", s.head_ref),
            Err(e) => format!("warning: branch {} not deleted: {e}\n", s.head_ref),
        },
        BranchStep::Keep(line) => line.clone(),
    }
}

fn act_merge_steps(
    ac: &AgentCtx,
    l: &MergeLoad,
    s: &MergeSteps,
    clock: &dyn Clock,
) -> CbResult<String> {
    let t = &l.t;
    // The guard runs before the first write, the merge of the PR (it cannot be undone).
    if let Ok(from) = t.status() {
        super::check_status_unchanged(ac, &t.item_id, &t.key(), from, Some(Status::Done))?;
    }
    let merge_sha = put_merge(ac, s)?;
    let mut out = format!(
        "PR {} merged at head {} ({}); merge commit {merge_sha}\n",
        s.pr_url, s.sha, s.method
    );
    // From here on the merge is done. A later failure says so, and how to finish.
    let finish = |e: CbError| {
        CbError::refused(
            ids::MERGE_DONE_TICKET_OPEN,
            format!(
                "PR {} is merged (commit {merge_sha}), but a later step failed: {e}. Finish with `crewboard close {} --verified`",
                s.pr_url, l.n
            ),
        )
    };
    out.push_str(&act_branch(ac, s));
    super::set_select_status(ac, &t.item_id, Status::Done).map_err(finish)?;
    out.push_str(&format!("{} closed: Status Done\n", s.ticket_key));
    if s.clear_needs_owner {
        out.push_str(&needs_owner_lines(ac, &t.repo, t.issue, false));
    }
    let to = to_agent(&s.assignee)?;
    let p = post_with(
        ac,
        t,
        clock,
        to,
        Kind::Info,
        Re::None,
        &s.notice_body(&merge_sha),
    )
    .map_err(finish)?;
    out.push_str(&format!("notice {} ({})\n", p.id, p.url));
    remember(ac, t, Status::Done, Some(&s.assignee));
    Ok(out)
}

/// `merge <issue> [--method squash|merge|rebase]` (coordinator): load, plan, act.
pub fn merge_ticket(
    ac: &AgentCtx,
    issue_arg: &str,
    method: &str,
    runs: &[String],
) -> CbResult<String> {
    let method = parse_method(method)?;
    let l = load_merge(ac, issue_arg)?;
    let plan = plan_merge(ac, &l, method, runs, &SystemClock)?;
    act_merge(ac, &l, &plan, &SystemClock)
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
