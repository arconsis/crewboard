//! `close <n>` without `--verified` (coordinator): end a ticket that has no
//! passed verify to rest on.
//!
//! The owner may merge a crew PR in the browser (CI is down, or an agent may not merge). GitHub
//! then closes the ticket issue through the closing keyword of the PR, but the board item stays
//! where it was. `verify` and `close --verified` judge evidence and CI that the owner's merge
//! made moot, and a ticket without a signed `done` cannot be verified at all.
//!
//! The close after an owner merge is no judgment, so `tick` (the headless coordinator) does it
//! too. It is accepted only when ALL of these hold:
//!
//! - the ticket issue is closed;
//! - a PR in a repo of the initiative is merged and its body closes this issue (the same
//!   closing-keyword check that `done` uses);
//! - the merge actor (`merged_by` of the PR) is the owner of AUTHORITY.md.
//!
//! The owner route is the one exception to the closing keyword and to the closed issue. A PR
//! that holds a workflow change only as a patch has no closing keyword by design
//! (spec/tickets#workflow-files-the-owner-route), so the issue stays open after the owner
//! merged it. When the latest signed `done` of the assignee is on the owner route (it has an
//! `owner-route:` line), ONLY the owner-route proof counts, also when the PR text closes the
//! issue: the PR of that `done` is in a repo of the initiative, merged by the owner, tied to
//! THIS ticket ([`super::owner_applied::tied`]: the patch is `patch_path(issue)` and the PR
//! contains it) and the workflow change is applied ([`super::owner_applied::patch_applied`]: a
//! content check that fails closed). `close` then also closes the issue. Reads: the PR once, its
//! files, then at most 6 for the content check.
//!
//! When a condition fails and the coordinator gave `--why "<text>"`, the coordinator drops the
//! ticket by judgment (the ticket has no PR, or will not be done): Status Dropped, never Done.
//! `tick` never does that. A board whose Status field has no option Dropped is refused with
//! `dropped-option-missing` (rerun `crewboard init`). Without a reason the close is refused with
//! `close-needs-reason`, and the message names the failed condition. Nothing is written then.
//!
//! The PR comes from the evidence of the latest signed `done` of the assignee. If that PR does
//! not meet the conditions (or there is no `done`), the
//! cross-references on the issue timeline name more candidate PRs (one more read). A ticket on
//! the owner route has no such search.
//!
//! Writes, in this order: the `merged without verify` note for an owner merge, ONE signed
//! `info` on the ticket (to the assignee, else to the coordinator) with the line `closed: owner
//! merged <pr-url>` (Status Done) or `dropped: <reason>` (Status Dropped), the label
//! `needs-owner` removed.

use super::owner_applied::{Proof, names_this_ticket, patch_applied, tied};
use super::pr_evidence::fetch_pr_files;
use super::pr_read::{MergedPr, read_pr};
use super::{Me, load_world_cached, me_of, needs_owner_lines, post, remember, to_agent};
use crate::base::ids;
use crate::error::CbResult;
use crate::ops::say::check_body_text;
use crate::ops::{AgentCtx, parse_issue_arg};
use crate::protocol::names::{Kind, Re, To};
use crate::rules::Refusal;
use crate::rules::evidence::parse_pr_url;
use crate::rules::status::{
    DoneReason, Status, Transition, check_status_transition, dropped_final,
};
use crate::rules::text::closes_ticket;
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::latest_done;
use crate::ticket::log::{fold_ticket, unsigned_end, unsigned_end_remedy};
use serde_json::{Value, json};

/// Cap on the PRs that one close reads.
const MAX_CANDIDATES: usize = 8;

/// The refusal for a close that has neither an owner merge nor a reason: names the failed
/// condition and the two ways on.
fn no_merge_found(key: &str, why: &str) -> Refusal {
    Refusal::new(
        ids::CLOSE_NEEDS_REASON,
        format!(
            "crewboard found no owner merge for {key} ({why}); give --why \"<text>\" to drop it by your judgment (Status Dropped), or --verified after a passed verify and a merge"
        ),
    )
}

/// The reason of a close: one line, not empty, passing the body check. `None`: no reason given.
pub fn reason_of(reason: Option<&str>) -> CbResult<Option<String>> {
    let Some(raw) = reason else { return Ok(None) };
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return Err(Refusal::new(
            ids::CLOSE_NEEDS_REASON,
            "the --why text is empty; give one non-empty line, or --verified after a passed verify",
        )
        .into());
    }
    check_body_text(&one_line)?;
    Ok(Some(one_line))
}

/// The status check: the owner's merge ends the ticket in any status but Done and Dropped. The
/// PR checks (merged by the owner, closes the issue) are the proof; a ticket that went back to
/// Ready or Backlog after its work was delivered (a `reassign`) ends the same way. A Done ticket
/// is `status-no-change` (idempotent), and so is a Dropped ticket for a repeat `--why`. A
/// Dropped ticket is final for a close without `--why`: only the owner moves it.
pub fn judge_status(key: &str, status: Status, why: bool) -> Result<(), Refusal> {
    match status {
        Status::Done => Err(Refusal::new(
            ids::STATUS_NO_CHANGE,
            format!("{key} is already Done"),
        )),
        Status::Dropped if why => Err(Refusal::new(
            ids::STATUS_NO_CHANGE,
            format!("{key} is already Dropped"),
        )),
        Status::Dropped => Err(dropped_final()),
        _ => Ok(()),
    }
}

/// A drop needs the option Dropped on the board (an older board lacks it until `init` runs
/// again). Pure.
pub fn check_dropped_option(key: &str, has_option: bool) -> Result<(), Refusal> {
    if has_option {
        return Ok(());
    }
    Err(Refusal::new(
        ids::DROPPED_OPTION_MISSING,
        format!(
            "the Status field of the board has no option Dropped, so {key} cannot be dropped: the owner runs `crewboard init` once more (it adds the option and keeps every other one). Nothing was written"
        ),
    ))
}

/// The conditions on one candidate PR. `Err` holds the failed condition as text.
pub fn judge_pr(
    pr: &MergedPr,
    repos: &[String],
    ticket_repo: &str,
    issue: u64,
    owner: &str,
) -> Result<(), String> {
    judge_pr_with(pr, repos, Some((ticket_repo, issue)), owner)
}

/// [`judge_pr`] with the closing keyword as an option: `None` (the owner route) does not ask
/// for it.
fn judge_pr_with(
    pr: &MergedPr,
    repos: &[String],
    closing: Option<(&str, u64)>,
    owner: &str,
) -> Result<(), String> {
    if !repos.iter().any(|r| r.eq_ignore_ascii_case(&pr.repo)) {
        return Err(format!(
            "PR {}#{} is not in a repo of the initiative ({})",
            pr.repo,
            pr.number,
            repos.join(", ")
        ));
    }
    if !pr.merged {
        return Err(format!("PR {}#{} is not merged", pr.repo, pr.number));
    }
    if let Some((ticket_repo, issue)) = closing
        && !closes_ticket(&pr.body, &pr.repo, ticket_repo, issue)
    {
        return Err(format!(
            "the body of PR {}#{} does not close {ticket_repo}#{issue}",
            pr.repo, pr.number
        ));
    }
    match pr.merged_by.as_deref() {
        Some(by) if by.eq_ignore_ascii_case(owner) => Ok(()),
        Some(by) => Err(format!(
            "PR {}#{} was merged by {by}, not by the owner ({owner})",
            pr.repo, pr.number
        )),
        None => Err(format!(
            "GitHub names no merge actor for PR {}#{}",
            pr.repo, pr.number
        )),
    }
}

/// The PRs that the cross-references of an issue timeline name, in order, without repeats.
/// A cross-reference from an issue (not a PR) is skipped.
pub fn timeline_prs(timeline: &Value) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    let Some(events) = timeline.as_array() else {
        return out;
    };
    for e in events {
        if e.get("event").and_then(Value::as_str) != Some("cross-referenced") {
            continue;
        }
        let Some(issue) = e.pointer("/source/issue") else {
            continue;
        };
        if issue.get("pull_request").is_none() {
            continue;
        }
        let repo = issue
            .pointer("/repository/full_name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                // `repository_url` is `https://api.github.com/repos/<owner>/<repo>`
                let u = issue.get("repository_url")?.as_str()?;
                let (_, rest) = u.split_once("/repos/")?;
                Some(rest.trim_end_matches('/').to_string())
            });
        let (Some(repo), Some(n)) = (repo, issue.get("number").and_then(Value::as_u64)) else {
            continue;
        };
        if !out
            .iter()
            .any(|(r, m)| r.eq_ignore_ascii_case(&repo) && *m == n)
        {
            out.push((repo, n));
        }
    }
    out
}

/// The text of the `info` of a close after an owner merge. A reason is added as a line.
pub fn notice_body(pr: &MergedPr, reason: Option<&str>) -> String {
    let mut body = format!(
        "{}\npr: {}\nmerge-commit: {}\nmerged by the owner\nclosed: owner merged {}\n",
        crate::protocol::acts::OWNER_MERGED_HEAD,
        pr.url(),
        pr.merge_commit.as_deref().unwrap_or("unknown"),
        pr.url()
    );
    if let Some(r) = reason {
        body.push_str(&format!("reason: {r}\n"));
    }
    body
}

/// The login that counts as the owner: `owner_login` of the board config, the one source
/// (`owner:` of AUTHORITY.md only ever agreed with it, because `start` refuses a mismatch).
fn owner_of(ac: &AgentCtx) -> String {
    ac.ctx.cfg.owner_login.clone()
}

/// Find the PR that the owner merged. `Ok(Err(why))`: no candidate meets the conditions.
fn find_merged_pr(ac: &AgentCtx, t: &Ticket, owner: &str) -> CbResult<Result<MergedPr, String>> {
    let repos = &ac.ctx.cfg.repos;
    let mut tried: Vec<(String, u64)> = Vec::new();
    let mut why: Vec<String> = Vec::new();
    // 1. the PR of the latest signed `done` of the assignee
    let mut candidates: Vec<(String, u64)> = Vec::new();
    if let Some((_, ev)) = t.agent().and_then(|a| latest_done(&t.msgs, a))
        && let Ok(u) = parse_pr_url(&ev.pr)
    {
        candidates.push((u.repo, u.number));
    }
    for round in 0..2 {
        if round == 1 {
            // 2. the cross-references on the issue timeline
            candidates = match ac.ctx.client.get(&format!(
                "repos/{}/issues/{}/timeline?per_page=100",
                t.repo, t.issue
            )) {
                Ok(r) => timeline_prs(&r.body),
                Err(e) => {
                    why.push(format!("the timeline of the issue cannot be read ({e})"));
                    Vec::new()
                }
            };
        }
        for (repo, number) in candidates.drain(..) {
            if tried.len() >= MAX_CANDIDATES
                || tried
                    .iter()
                    .any(|(r, m)| r.eq_ignore_ascii_case(&repo) && *m == number)
            {
                continue;
            }
            tried.push((repo.clone(), number));
            if !repos.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
                why.push(format!(
                    "PR {repo}#{number} is not in a repo of the initiative ({})",
                    repos.join(", ")
                ));
                continue;
            }
            let Some(pr) = read_pr(&ac.ctx, &repo, number)? else {
                why.push(format!("PR {repo}#{number} does not exist"));
                continue;
            };
            match judge_pr(&pr, repos, &t.repo, t.issue, owner) {
                Ok(()) => return Ok(Ok(pr)),
                Err(w) => why.push(w),
            }
        }
    }
    Ok(Err(if why.is_empty() {
        "no PR is named by the evidence or by the cross-references of the issue".to_string()
    } else {
        why.join("; ")
    }))
}

/// The owner route: the PR and the patch of the latest `done`, when that `done` has the line
/// `owner-route:`. `None`: the ticket is not on the owner route.
fn owner_route_done(t: &Ticket) -> Option<(String, String)> {
    let (done, ev) = latest_done(&t.msgs, t.agent()?)?;
    let patch = crate::ticket::owner_route_patch(&done.body)?;
    Some((ev.pr, patch))
}

/// The owner merge of an owner-route ticket: the PR of the latest `done`, in a repo of the
/// initiative, merged by the owner, tied to THIS ticket and with the workflow change applied
/// ([`super::owner_applied`]). No closing keyword and no closed issue needed. `Ok(Err(why))`: a
/// condition failed. Reads: the PR once, its files, then the reads of `patch_applied`.
fn find_owner_route_merge(
    ac: &AgentCtx,
    t: &Ticket,
    pr_url: &str,
    patch: &str,
    owner: &str,
) -> CbResult<Result<MergedPr, String>> {
    let repos = &ac.ctx.cfg.repos;
    if let Err(w) = names_this_ticket(t.issue, patch) {
        return Ok(Err(w));
    }
    let Ok(u) = parse_pr_url(pr_url) else {
        return Ok(Err(format!(
            "the PR of the owner-route done is no PR URL ({pr_url})"
        )));
    };
    if !repos.iter().any(|r| r.eq_ignore_ascii_case(&u.repo)) {
        return Ok(Err(format!(
            "PR {}#{} is not in a repo of the initiative ({})",
            u.repo,
            u.number,
            repos.join(", ")
        )));
    }
    let Some(pr) = read_pr(&ac.ctx, &u.repo, u.number)? else {
        return Ok(Err(format!("PR {}#{} does not exist", u.repo, u.number)));
    };
    if let Err(w) = judge_pr_with(&pr, repos, None, owner) {
        return Ok(Err(w));
    }
    let files = fetch_pr_files(ac, &pr.repo, pr.number)?;
    if let Err(w) = tied(t.issue, patch, &files) {
        return Ok(Err(w));
    }
    let (Some(base), Some(head_sha)) = (pr.base.as_deref(), pr.head_sha.as_deref()) else {
        return Ok(Err(format!(
            "GitHub names no base branch or head for PR {}#{}",
            pr.repo, pr.number
        )));
    };
    let proof = Proof {
        repo: &pr.repo,
        base,
        head_sha,
        patch,
        pr_files: &files,
    };
    Ok(patch_applied(ac, &proof)?.map(|_| pr))
}

/// `close <issue> [--why <text>]` (coordinator).
pub fn close_ticket(ac: &AgentCtx, issue_arg: &str, reason: Option<&str>) -> CbResult<String> {
    let tref = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    require_writer(&me)?;
    let reason = reason_of(reason)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &tref)?;
    close_loaded(ac, &me, &t, reason.as_deref())
}

fn require_writer(me: &Me) -> Result<(), Refusal> {
    if me.actor.is_coordinator() {
        Ok(())
    } else {
        Err(Refusal::new(
            ids::STATUS_WRONG_WRITER,
            "close is a coordinator action",
        ))
    }
}

/// The close of a loaded ticket; `reason` is already one line (`reason_of`). The checks come
/// first (writer, status, issue closed, PR), the writes after. Without an owner merge a reason
/// drops the ticket by judgment, and no reason is `close-needs-reason`.
pub(super) fn close_loaded(
    ac: &AgentCtx,
    me: &Me,
    t: &Ticket,
    reason: Option<&str>,
) -> CbResult<String> {
    require_writer(me)?;
    let key = t.key();
    let from = t.status()?;
    // A board end that no signed message supports (set by hand): name the way out.
    if unsigned_end(from, &fold_ticket(&t.msgs).status) {
        judge_status(&key, from, reason.is_some()).map_err(|r| {
            Refusal::new(
                r.rule,
                format!(
                    "{key} is {} on the board, but {}. Nothing was written",
                    from.name(),
                    unsigned_end_remedy(t.issue)
                ),
            )
        })?;
    }
    judge_status(&key, from, reason.is_some())?;
    let route = owner_route_done(t);
    let owner = if !t.info.open || route.is_some() {
        // Read the file for its refusals and warnings; the owner comes from the board config.
        crate::ctx::load_authority(&ac.ctx)?;
        Some(owner_of(ac))
    } else {
        None
    };
    // true: the issue is still open and `close` ends it (the owner route)
    let mut end_issue = false;
    let found = match (&route, &owner) {
        // The owner route: only its proof counts, also when the PR text closes the issue (a
        // keyword added after the `done` is no proof that the change is live).
        (Some((pr, patch)), Some(owner)) => {
            let r = find_owner_route_merge(ac, t, pr, patch, owner)?;
            end_issue = t.info.open && r.is_ok();
            r.map_err(|w| format!("on the owner route: {w}"))
        }
        (None, Some(owner)) if !t.info.open => find_merged_pr(ac, t, owner)?,
        _ => Err("the issue is still open".to_string()),
    };
    match (found, reason) {
        (Ok(pr), _) => close_after_merge(ac, me, t, from, &pr, reason, end_issue),
        (Err(_), Some(r)) => close_by_judgment(ac, me, t, from, r),
        (Err(why), None) => Err(no_merge_found(&key, &why).into()),
    }
}

/// The signed `info` goes to the assignee, else to the coordinator.
fn close_recipient(t: &Ticket) -> CbResult<(Option<String>, To)> {
    let assignee = t.agent().map(str::to_string);
    let to = match &assignee {
        Some(a) => to_agent(a)?,
        None => To::Coordinator,
    };
    Ok((assignee, to))
}

fn close_after_merge(
    ac: &AgentCtx,
    me: &Me,
    t: &Ticket,
    from: Status,
    pr: &MergedPr,
    reason: Option<&str>,
    end_issue: bool,
) -> CbResult<String> {
    let key = t.key();
    let note = notice_body(pr, reason);
    check_body_text(&note)?;
    let close = Transition::Close {
        reason: Some(DoneReason::OwnerMerged),
        verify: None,
    };
    check_status_transition(Some(from), Status::Done, me.actor, &close)?;
    // The owner's merge ends the ticket, but a merge without a passed verify is noted.
    let gap_line = super::merged_unverified::note_loaded(ac, t, pr)?;
    let headline = format!(
        "{key} closed: Status Done (the owner merged {} at {})",
        pr.url(),
        pr.merge_commit.as_deref().unwrap_or("unknown")
    );
    let end = End {
        to: Status::Done,
        note: &note,
        headline: &headline,
        extra: &gap_line,
        end_issue,
    };
    finish_close(ac, t, &end)
}

fn close_by_judgment(
    ac: &AgentCtx,
    me: &Me,
    t: &Ticket,
    from: Status,
    reason: &str,
) -> CbResult<String> {
    let key = t.key();
    let note = crate::protocol::acts::dropped_body(reason);
    check_body_text(&note)?;
    check_status_transition(Some(from), Status::Dropped, me.actor, &Transition::Drop)?;
    check_dropped_option(&key, ac.ctx.board.has_status_option(Status::Dropped))?;
    let headline = format!("{key} dropped: Status Dropped (dropped: {reason})");
    let end = End {
        to: Status::Dropped,
        note: &note,
        headline: &headline,
        extra: "",
        end_issue: false,
    };
    finish_close(ac, t, &end)
}

/// How a close ends a ticket.
struct End<'a> {
    /// Done, or Dropped for a drop.
    to: Status,
    /// The signed `info`.
    note: &'a str,
    headline: &'a str,
    /// Output that comes after the notice line.
    extra: &'a str,
    /// Close the open issue first.
    end_issue: bool,
}

/// The writes of every close, in order: the signed `info`, the Status, the label off.
fn finish_close(ac: &AgentCtx, t: &Ticket, end: &End<'_>) -> CbResult<String> {
    let (assignee, to) = close_recipient(t)?;
    // The guard runs before the first write: a ticket that moved gets no notice.
    if let Ok(from) = t.status() {
        super::check_status_unchanged(ac, &t.item_id, &t.key(), from, Some(end.to))?;
    }
    if end.end_issue {
        // the owner route: GitHub did not close the issue, because the PR has no closing keyword
        ac.ctx.client.send(
            "PATCH",
            &format!("repos/{}/issues/{}", t.repo, t.issue),
            Some(&json!({"state": "closed", "state_reason": "completed"})),
        )?;
    }
    let p = post(ac, t, to, Kind::Info, Re::None, end.note)?;
    super::set_select_status(ac, &t.item_id, end.to)?;
    let mut out = format!("{}\nnotice {} ({})\n", end.headline, p.id, p.url);
    out.push_str(end.extra);
    out.push_str(&needs_owner_lines(ac, &t.repo, t.issue, false));
    remember(ac, t, end.to, assignee.as_deref());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CbError;
    use serde_json::json;

    const REPOS: [&str; 2] = ["o/r", "o/r2"];

    fn repos() -> Vec<String> {
        REPOS.iter().map(|s| s.to_string()).collect()
    }

    fn pr() -> MergedPr {
        MergedPr {
            repo: "o/r".into(),
            number: 9,
            merged: true,
            body: "Work.\n\nCloses #12\n".into(),
            merged_by: Some("the-owner".into()),
            merge_commit: Some("abc".into()),
            head_sha: Some("def".into()),
            base: None,
        }
    }

    fn judge(p: &MergedPr) -> Result<(), String> {
        judge_pr(p, &repos(), "o/r", 12, "the-owner")
    }

    #[test]
    fn a_merged_pr_that_closes_the_issue_and_was_merged_by_the_owner_passes() {
        assert!(judge(&pr()).is_ok());
        // the login compares without letter case
        let p = MergedPr {
            merged_by: Some("The-Owner".into()),
            ..pr()
        };
        assert!(judge(&p).is_ok());
        // a PR of another repo of the initiative, with the full form of the keyword
        let p = MergedPr {
            repo: "o/r2".into(),
            body: "Fixes o/r#12".into(),
            ..pr()
        };
        assert!(judge(&p).is_ok());
    }

    #[test]
    fn each_failed_condition_is_refused_and_named() {
        let cases: Vec<(MergedPr, &str)> = vec![
            (
                MergedPr {
                    merged: false,
                    ..pr()
                },
                "is not merged",
            ),
            (
                MergedPr {
                    body: "Refs #12".into(),
                    ..pr()
                },
                "does not close o/r#12",
            ),
            (
                MergedPr {
                    body: "Closes #13".into(),
                    ..pr()
                },
                "does not close o/r#12",
            ),
            (
                MergedPr {
                    merged_by: Some("crew-account".into()),
                    ..pr()
                },
                "merged by crew-account, not by the owner",
            ),
            (
                MergedPr {
                    merged_by: None,
                    ..pr()
                },
                "no merge actor",
            ),
            (
                MergedPr {
                    repo: "x/y".into(),
                    body: "Closes o/r#12".into(),
                    ..pr()
                },
                "not in a repo of the initiative",
            ),
            // a same-repo keyword in a PR of another repo closes another issue
            (
                MergedPr {
                    repo: "o/r2".into(),
                    body: "Closes #12".into(),
                    ..pr()
                },
                "does not close o/r#12",
            ),
        ];
        for (p, want) in cases {
            let e = judge(&p).unwrap_err();
            assert!(e.contains(want), "{want}: {e}");
        }
    }

    #[test]
    fn the_status_check_names_the_unfinished_statuses_done_and_the_rest() {
        for why in [false, true] {
            for s in [Status::Review, Status::InProgress, Status::Blocked] {
                assert!(judge_status("o/r#12", s, why).is_ok(), "{s:?}");
            }
            assert_eq!(
                judge_status("o/r#12", Status::Done, why).unwrap_err().rule,
                "status-no-change"
            );
            // a ticket that went back to the queue after its work was delivered
            for s in [Status::Backlog, Status::Ready] {
                assert!(judge_status("o/r#12", s, why).is_ok(), "{s:?}");
            }
        }
        // Dropped: a repeat drop is no change; an owner merge does not end it
        assert_eq!(
            judge_status("o/r#12", Status::Dropped, true)
                .unwrap_err()
                .rule,
            "status-no-change"
        );
        assert_eq!(
            judge_status("o/r#12", Status::Dropped, false)
                .unwrap_err()
                .rule,
            "status-dropped-final"
        );
    }

    #[test]
    fn a_drop_needs_the_option_and_the_refusal_names_init() {
        assert!(check_dropped_option("o/r#12", true).is_ok());
        let r = check_dropped_option("o/r#12", false).unwrap_err();
        assert_eq!(r.rule, ids::DROPPED_OPTION_MISSING);
        assert!(r.message.contains("crewboard init"), "{}", r.message);
    }

    #[test]
    fn the_refusal_names_the_rule_and_the_way_on() {
        let e = no_merge_found("o/r#12", "the issue is still open");
        assert_eq!(e.rule, "close-needs-reason");
        assert!(e.message.contains("the issue is still open"));
        assert!(e.message.contains("--why"));
        assert!(e.message.contains("--verified"));
    }

    #[test]
    fn the_reason_is_one_line_and_not_empty() {
        assert_eq!(reason_of(None).unwrap(), None);
        assert_eq!(
            reason_of(Some("  too\nbig  now ")).unwrap().as_deref(),
            Some("too big now")
        );
        for r in ["", "  \n\t "] {
            let CbError::Refused(e) = reason_of(Some(r)).unwrap_err() else {
                panic!("not a refusal");
            };
            assert_eq!(e.rule, "close-needs-reason");
        }
    }

    #[test]
    fn the_timeline_names_the_prs_that_refer_to_the_issue() {
        let t = json!([
            {"event": "labeled"},
            // a PR, with the repo in the answer
            {"event": "cross-referenced", "source": {"type": "issue", "issue": {
                "number": 9, "pull_request": {}, "repository": {"full_name": "o/r"}}}},
            // the same PR again: once
            {"event": "cross-referenced", "source": {"type": "issue", "issue": {
                "number": 9, "pull_request": {}, "repository": {"full_name": "O/R"}}}},
            // a plain issue: skipped
            {"event": "cross-referenced", "source": {"type": "issue", "issue": {
                "number": 10, "repository": {"full_name": "o/r"}}}},
            // a PR with only the repository URL
            {"event": "cross-referenced", "source": {"type": "issue", "issue": {
                "number": 11, "pull_request": {},
                "repository_url": "https://api.github.com/repos/o/r2"}}},
            // no repo at all: skipped
            {"event": "cross-referenced", "source": {"type": "issue", "issue": {
                "number": 12, "pull_request": {}}}},
        ]);
        assert_eq!(
            timeline_prs(&t),
            vec![("o/r".to_string(), 9), ("o/r2".to_string(), 11)]
        );
        assert!(timeline_prs(&json!({"message": "x"})).is_empty());
    }

    #[test]
    fn the_notice_has_the_lines_that_the_assignee_reads() {
        let b = notice_body(&pr(), None);
        assert!(b.contains("pr: https://github.com/o/r/pull/9\n"), "{b}");
        assert!(b.contains("merge-commit: abc\n"), "{b}");
        assert!(b.contains("merged by the owner\n"), "{b}");
        assert!(
            b.contains("closed: owner merged https://github.com/o/r/pull/9\n"),
            "{b}"
        );
        assert!(!b.contains("reason:"), "{b}");
        assert!(notice_body(&pr(), Some("x y")).ends_with("reason: x y\n"));
    }
}
