//! `close`: the coordinator closes a ticket (after a passed verify and a merge, or after an
//! owner merge), or drops it with a stated reason.

use super::owner_merge;
use super::pr_evidence::fetch_pr;
use super::pr_evidence::pr_files_for_protection;
use super::{
    check_owner_route, load_world_cached, me_of, post, question_open_since, remember, set_status,
    sync_needs_owner, to_agent,
};
use crate::error::CbResult;
use crate::ops::{AgentCtx, parse_issue_arg};
use crate::protocol::names::{Kind, Re};
use crate::rules::Refusal;
use crate::rules::evidence::PrFacts;
use crate::rules::evidence::parse_pr_url;
use crate::rules::status::{
    DoneReason, Status, Transition, check_no_open_question, check_status_transition,
};
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::{kv, latest_done, question_open, verify_report, verify_result};

/// A ticket in Review is verified, merged or closed after a passed verify only without an owner
/// route `done` and without an open question to the owner.
fn check_review_deliverable(t: &Ticket, open_since: Option<&str>) -> Result<(), Refusal> {
    check_owner_route(t)?;
    check_no_open_question(&t.key(), open_since)
}

/// How `close` ends a ticket.
#[derive(Debug, Clone, Copy)]
pub struct CloseHow<'a> {
    /// `--verified`: Review -> Done after a passed verify, for a PR merged at the verified head.
    pub verified: bool,
    /// `--why <text>`.
    pub reason: Option<&'a str>,
}

/// `close <issue> [--verified | --why <text>]` (coordinator).
pub fn close(ac: &AgentCtx, issue_arg: &str, how: CloseHow<'_>) -> CbResult<String> {
    if how.verified {
        close_verified(ac, issue_arg)
    } else {
        owner_merge::close_ticket(ac, issue_arg, how.reason)
    }
}

/// The rule of a `close --verified` whose PR is still open: Done means a merged PR, so the
/// coordinator merges it with `merge`.
pub const CLOSE_VERIFIED_NOT_MERGED: &str = "close-verified-not-merged";

/// The refusal of `close --verified` without a merged PR.
fn not_merged(key: &str, what: &str) -> Refusal {
    let n = key.rsplit('#').next().unwrap_or(key);
    Refusal::new(
        CLOSE_VERIFIED_NOT_MERGED,
        format!(
            "{key}: {what}; `close --verified` needs the PR merged at the head of the passing verify. Merge it with `crewboard merge {n}`. Nothing was written"
        ),
    )
}

/// Rules `close-verified-not-merged` and `close-verified-head-moved`: Done after
/// `close --verified` means "merged at the head that the passing verify saw" (its `head_now:`
/// line). An open PR is not merged yet; a PR closed without a merge, or merged at another head,
/// is not that head.
fn check_verified_head(key: &str, pr: &PrFacts, verified: Option<&str>) -> Result<(), Refusal> {
    if pr.state != "closed" && !pr.merged {
        return Err(not_merged(
            key,
            &format!("PR {}#{} is open", pr.repo, pr.number),
        ));
    }
    if !pr.merged {
        return Err(Refusal::new(
            "close-verified-head-moved",
            format!(
                "{key}: PR {}#{} is closed without a merge; `close --verified` needs a PR that is merged at the verified head. Nothing was written",
                pr.repo, pr.number
            ),
        ));
    }
    match verified {
        Some(h) if h.trim().eq_ignore_ascii_case(pr.head_sha.trim()) => Ok(()),
        seen => Err(Refusal::new(
            "close-verified-head-moved",
            format!(
                "{key}: the head of PR {}#{} is {}, but the passing verify saw {}. Run `verify` again for the new head. Nothing was written",
                pr.repo,
                pr.number,
                pr.head_sha,
                seen.unwrap_or("no head")
            ),
        )),
    }
}

/// `close <issue> --verified` (coordinator): Review -> Done after a passed verify, for a PR that
/// is merged at the verified head.
fn close_verified(ac: &AgentCtx, issue_arg: &str) -> CbResult<String> {
    let (repo, n) = parse_issue_arg(issue_arg, ac.ctx.main_repo())?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    let t = load_ticket(&ac.ctx.reader(), &world.trust, &repo, n)?;
    let from = t.status()?;
    let assignee = t.agent().map(str::to_string);
    let verify = assignee
        .as_deref()
        .and_then(|a| latest_done(&t.msgs, a))
        .and_then(|(m, _)| verify_result(&t.msgs, m, n));
    let close = Transition::Close {
        reason: Some(DoneReason::VerifiedClose),
        verify,
    };
    // A ticket in Review needs no open question and no owner-route `done`.
    if from == Status::Review && me.actor.is_coordinator() {
        check_review_deliverable(&t, question_open_since(&t).as_deref())?;
    }
    // true: the PR of the latest `done` is merged at the head of the passing verify
    let mut merged_at_head = false;
    if let Some((done, ev)) = assignee.as_deref().and_then(|a| latest_done(&t.msgs, a))
        && let Ok(pu) = parse_pr_url(&ev.pr)
    {
        // Rules `close-verified-not-merged` and `close-verified-head-moved`: Done after
        // `close --verified` means "merged at the head that the passing verify saw". Read the PR
        // before any write.
        if let Some((report, true)) = verify_report(&t.msgs, done, n) {
            let (pr, _) = fetch_pr(ac, &pu.repo, pu.number)?;
            check_verified_head(&t.key(), &pr, kv(&report.body, "head_now").as_deref())?;
            merged_at_head = true;
        }
        // Rule `protected-path-owner-merge`: `close --verified` is refused for a PR that changes
        // a protected path now (a verify may have passed before the PR changed). The owner's
        // merge is closed with `close`.
        let files = pr_files_for_protection(ac, &pu.repo, pu.number, &t.key())?;
        let auth = crate::ctx::load_authority(&ac.ctx)?;
        crate::rules::protected::check_unprotected(
            &ev.pr,
            &t.key(),
            &files,
            crate::rules::protected::Protect::new(&ac.ctx.cfg.authority, &auth.protected_paths),
        )?;
    }
    check_status_transition(Some(from), Status::Done, me.actor, &close)?;
    // Fail closed: a passed verify whose PR was not read is no proof of the merge.
    if !merged_at_head {
        return Err(not_merged(&t.key(), "no merged PR of the latest done was read").into());
    }
    // Post the note first, then write the Status, as `close` does: a failed post leaves a
    // Review ticket that a rerun closes. The other order left Done without proof, which a
    // rerun refused (`status-no-change`).
    let mut notice = String::new();
    if let Some(a) = &assignee {
        let p = post(
            ac,
            &t,
            to_agent(a)?,
            Kind::Info,
            Re::None,
            &crate::protocol::acts::closed_verified_body(),
        )?;
        notice = format!("notice {} ({})\n", p.id, p.url);
    }
    set_status(ac, &t, Status::Done)?;
    let mut out = format!("{} closed: Status Done\n", t.key());
    out.push_str(&sync_needs_owner(
        ac,
        &t,
        question_open(&t.msgs, t.agent()).is_some(),
    ));
    out.push_str(&notice);
    remember(ac, &t, Status::Done, assignee.as_deref());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(state: &str, merged: bool, head: &str) -> PrFacts {
        PrFacts {
            repo: "o/r".into(),
            number: 9,
            state: state.into(),
            merged,
            head_sha: head.into(),
            head_ref: "crew/a@h/7-x".into(),
            title: String::new(),
            body: String::new(),
        }
    }

    fn rule(p: &PrFacts, seen: Option<&str>) -> &'static str {
        check_verified_head("o/r#7", p, seen).unwrap_err().rule
    }

    #[test]
    fn only_a_pr_merged_at_the_verified_head_passes() {
        assert!(check_verified_head("o/r#7", &pr("closed", true, "abc"), Some("ABC")).is_ok());
        // open: merge it first, at any head
        for seen in [Some("abc"), Some("def"), None] {
            assert_eq!(
                rule(&pr("open", false, "abc"), seen),
                CLOSE_VERIFIED_NOT_MERGED
            );
        }
        let m = check_verified_head("o/r#7", &pr("open", false, "abc"), Some("abc"))
            .unwrap_err()
            .message;
        assert!(m.contains("crewboard merge 7"), "{m}");
        // closed without a merge, or merged at another head
        assert_eq!(
            rule(&pr("closed", false, "abc"), Some("abc")),
            "close-verified-head-moved"
        );
        assert_eq!(
            rule(&pr("closed", true, "def"), Some("abc")),
            "close-verified-head-moved"
        );
        assert_eq!(
            rule(&pr("closed", true, "abc"), None),
            "close-verified-head-moved"
        );
    }
}
