//! The work that an agent leaves behind when its ticket goes back to the board (spec/tickets#work-that-an-earlier-agent-left).
//!
//! A dead, draining or replaced agent may have pushed a crew branch and opened a PR that says
//! `Closes #n`. Without a record the PR is orphaned: the next agent starts again, or a merge by
//! hand closes the issue by accident. So a reassign (the command, the coordinator duty,
//! `stop --force`) reads GitHub once for the branch and the open PR of the old agent and writes
//! them into the signed reassign notice ([`crate::protocol::acts::reassigned_body_with`]).
//! `assign` and `take` of the new agent print what the notice names. `take --continue-pr <url>`
//! checks and records the adoption of that PR; `done` accepts its branch because the notice of the
//! coordinator names it ([`crate::ticket::adopted_branch`]).
//!
//! The read is a help: a read that fails never blocks the reassign.

use super::pr_evidence::fetch_pr;
use crate::base::ids::{self, RuleId};
use crate::error::{CbError, CbResult};
use crate::github::api::pulls::{self, Order, PullSummary};
use crate::ops::AgentCtx;
use crate::protocol::acts::EarlierWork;
use crate::rules::evidence::{branch_ticket, parse_pr_url};
use crate::rules::text::closes_ticket;
use crate::rules::text::mentions_ticket;
use crate::services::ticket_loader::{Ticket, check_repo};
use serde_json::Value;

/// The PR that `agent` left for ticket `issue`, from the open PR list: an open PR whose head
/// branch is a crew branch of this ticket, `<prefix><any agent>/<issue>-<slug>`. The branch of
/// `agent` itself comes first. Else the PR is one that `agent` adopted from an agent before it
/// (`take --continue-pr`): its branch carries the name of that agent, and the notice must still
/// name it, or the next agent cannot adopt it. Pure.
fn pr_in_list(
    list: &[PullSummary],
    repo: &str,
    prefix: &str,
    agent: &str,
    issue: u64,
) -> Option<EarlierWork> {
    let of_ticket = |p: &&PullSummary| {
        branch_ticket(prefix, &p.head_ref).map(|(a, n)| (a == agent, n == issue))
    };
    let own = list.iter().find(|p| of_ticket(p) == Some((true, true)));
    let any = || list.iter().find(|p| of_ticket(p).is_some_and(|(_, n)| n));
    own.or_else(any).map(|p| EarlierWork {
        pr: Some(format!("https://github.com/{repo}/pull/{}", p.number)),
        branch: p.head_ref.clone(),
    })
}

/// The branch `<prefix><agent>/<issue>-...` from a `matching-refs` answer. Pure.
fn branch_in_refs(refs: &Value) -> Option<EarlierWork> {
    let name = refs
        .as_array()?
        .first()?
        .get("ref")?
        .as_str()?
        .strip_prefix("refs/heads/")?;
    Some(EarlierWork {
        pr: None,
        branch: name.to_string(),
    })
}

/// What `agent` left on GitHub for ticket `repo#issue`: the open PR of its crew branch (or of the
/// crew branch that it adopted for this ticket), else its branch alone, else nothing. A read that fails gives a warning on stderr and `None`.
pub(crate) fn find(ac: &AgentCtx, repo: &str, issue: u64, agent: &str) -> Option<EarlierWork> {
    let prefix = crate::ctx::load_authority_quiet(&ac.ctx)
        .unwrap_or_default()
        .branch_prefix();
    let warn = |e: crate::github::GhError| {
        eprintln!(
            "crewboard: warning: the work of {agent} on {repo}#{issue} was not read ({e}); the notice names no PR"
        );
    };
    let prs = pulls::list_open(&ac.ctx.client, repo, Order::Updated)
        .map_err(warn)
        .ok()?;
    if let Some(found) = pr_in_list(&prs, repo, &prefix, agent, issue) {
        return Some(found);
    }
    let refs = ac
        .ctx
        .client
        .get(&format!(
            "repos/{repo}/git/matching-refs/heads/{prefix}{agent}/{issue}-"
        ))
        .map_err(warn)
        .ok()?;
    branch_in_refs(&refs.body)
}

/// `key`, with the earlier PR or branch when there is one: for the lists of a report.
pub(crate) fn tagged(key: &str, earlier: Option<&EarlierWork>) -> String {
    match earlier {
        Some(EarlierWork {
            pr: Some(url),
            branch: _,
        }) => format!("{key} (earlier PR {url})"),
        Some(EarlierWork { pr: None, branch }) => format!("{key} (earlier branch {branch})"),
        None => key.to_string(),
    }
}

/// Is the PR still open? A read that fails says `false`: a hint is not worth a wrong claim.
fn still_open(ac: &AgentCtx, url: &str) -> bool {
    parse_pr_url(url)
        .ok()
        .and_then(|u| fetch_pr(ac, &u.repo, u.number).ok())
        .is_some_and(|(pr, _)| pr.state == "open" && !pr.merged)
}

/// The lines for the agent that takes over `t` (`agent`): what the latest reassign notice names
/// and GitHub still shows. `assign` names the `take` flag; after a `take` (`after_take`) there is
/// no later step to name. Empty when there is nothing to say (also for a branch of `agent`).
pub(crate) fn hint(ac: &AgentCtx, t: &Ticket, agent: &str, after_take: bool) -> String {
    let Some(e) = crate::ticket::earlier_work(&t.msgs) else {
        return String::new();
    };
    let own = |branch: &str| {
        let prefix = crate::ctx::load_authority_quiet(&ac.ctx)
            .unwrap_or_default()
            .branch_prefix();
        branch_ticket(&prefix, branch).is_some_and(|(a, _)| a == agent)
    };
    if own(&e.branch) {
        return String::new();
    }
    match &e.pr {
        Some(url) if still_open(ac, url) => {
            let how = if after_take {
                format!(
                    "continue on its branch {} (`done` accepts it) or close it",
                    e.branch
                )
            } else {
                format!(
                    "continue on it with `crewboard take {} --continue-pr {url}` or close it",
                    t.key()
                )
            };
            format!("an earlier PR exists: {url}; {how}\n")
        }
        Some(_) => String::new(),
        None => format!(
            "an earlier branch exists: {}; start from it or ignore it\n",
            e.branch
        ),
    }
}

/// An open PR of the same ticket that `take --continue-pr` has checked.
pub(super) struct Adopted {
    pub url: String,
    pub branch: String,
    /// The PR body says `Closes #n` (the ticket is not an owner route ticket).
    pub closes: bool,
}

fn refuse(rule: RuleId, why: String) -> CbError {
    CbError::refused(rule, why)
}

/// The checks of `take --continue-pr <url>`, before anything is written: the PR is in the repo of
/// the ticket and is the one that the latest reassign notice of the coordinator names; it is open;
/// its head branch is a crew branch `<prefix><agent>/<n>-<slug>` of THIS ticket; and it links the
/// ticket. The safety rules are those of `ticket new --continue-pr`; the ticket is the same one, so
/// the parent is not Done.
pub(super) fn check_adopt(ac: &AgentCtx, t: &Ticket, url: &str) -> CbResult<Adopted> {
    let u = parse_pr_url(url)?;
    check_repo(&ac.ctx.cfg, &u.repo)?;
    if !u.repo.eq_ignore_ascii_case(&t.repo) {
        return Err(refuse(
            ids::ADOPT_PR_OTHER_REPO,
            format!(
                "PR {}#{} is in another repo than {}: take adopts a PR of the ticket's own repo",
                u.repo,
                u.number,
                t.key()
            ),
        ));
    }
    let named = crate::ticket::earlier_work(&t.msgs).and_then(|e| e.pr);
    let recorded = named
        .as_deref()
        .and_then(|n| parse_pr_url(n).ok())
        .is_some_and(|n| n.repo.eq_ignore_ascii_case(&u.repo) && n.number == u.number);
    if !recorded {
        return Err(refuse(
            ids::ADOPT_PR_NOT_RECORDED,
            format!(
                "the latest reassign notice on {} does not name PR {}#{} ({}): take adopts the open PR that an earlier agent left and that the coordinator's notice records; for any other PR start a branch of your own",
                t.key(),
                u.repo,
                u.number,
                named.as_deref().unwrap_or("it names no PR")
            ),
        ));
    }
    let (pr, _) = fetch_pr(ac, &u.repo, u.number)?;
    if pr.state != "open" || pr.merged {
        return Err(refuse(
            ids::ADOPT_PR_NOT_OPEN,
            format!(
                "PR {}#{} is {}: take adopts an OPEN PR only",
                pr.repo,
                pr.number,
                if pr.merged { "merged" } else { "closed" }
            ),
        ));
    }
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    let prefix = auth.branch_prefix();
    match branch_ticket(&prefix, &pr.head_ref) {
        Some((_, n)) if n == t.issue => {}
        _ => {
            return Err(refuse(
                ids::ADOPT_PR_BAD_BRANCH,
                format!(
                    "the head branch '{}' of PR {}#{} is not {prefix}<agent>/{}-<slug>: take adopts the crew branch of this ticket",
                    pr.head_ref, pr.repo, pr.number, t.issue
                ),
            ));
        }
    }
    auth.check_head_branch(&pr.head_ref)?;
    let text = format!("{}\n{}", pr.title, pr.body);
    if !mentions_ticket(&text, &pr.repo, &t.repo, t.issue) {
        return Err(refuse(
            ids::ADOPT_PR_NOT_LINKED,
            format!(
                "PR {}#{} does not mention {}: it must say 'Closes #{}' (or 'Refs #{}' on the owner route)",
                pr.repo,
                pr.number,
                t.key(),
                t.issue,
                t.issue
            ),
        ));
    }
    Ok(Adopted {
        url: format!("https://github.com/{}/pull/{}", pr.repo, pr.number),
        branch: pr.head_ref.clone(),
        closes: closes_ticket(&pr.body, &pr.repo, &t.repo, t.issue),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_pr_of_the_old_agent_is_found_by_its_branch_name_only() {
        let pr = |number: u64, head: &str| PullSummary {
            number,
            head_ref: head.into(),
            head_sha: "abc".into(),
            base_ref: "main".into(),
        };
        let list = [
            pr(3, "crew/other@h/5-x"),
            pr(4, "crew/old@h/6-x"),
            pr(7, "crew/old@h/5-fix-it"),
        ];
        let e = pr_in_list(&list, "o/r", "crew/", "old@h", 5).unwrap();
        assert_eq!(e.pr.as_deref(), Some("https://github.com/o/r/pull/7"));
        assert_eq!(e.branch, "crew/old@h/5-fix-it");
        assert!(pr_in_list(&list, "o/r", "crew/", "old@h", 9).is_none());
        assert!(pr_in_list(&[], "o/r", "crew/", "old@h", 5).is_none());
    }

    #[test]
    fn a_pr_that_the_old_agent_adopted_is_found_under_the_branch_of_the_agent_before_it() {
        let pr = |number: u64, head: &str| PullSummary {
            number,
            head_ref: head.into(),
            head_sha: "abc".into(),
            base_ref: "main".into(),
        };
        // `old@h` has no branch of its own for ticket 5: it adopted the PR of `first@h`
        let list = [
            pr(4, "crew/first@h/6-x"),
            pr(3, "crew/first@h/5-x"),
            pr(8, "feature/5-x"),
        ];
        let e = pr_in_list(&list, "o/r", "crew/", "old@h", 5).unwrap();
        assert_eq!(e.pr.as_deref(), Some("https://github.com/o/r/pull/3"));
        assert_eq!(e.branch, "crew/first@h/5-x");
        // the own branch of the old agent wins over the branch of another agent
        let list = [pr(3, "crew/first@h/5-x"), pr(7, "crew/old@h/5-y")];
        let e = pr_in_list(&list, "o/r", "crew/", "old@h", 5).unwrap();
        assert_eq!(e.pr.as_deref(), Some("https://github.com/o/r/pull/7"));
    }

    #[test]
    fn a_branch_without_a_pr_comes_from_the_refs_answer() {
        let e = branch_in_refs(&json!([{"ref": "refs/heads/crew/old@h/5-x"}])).unwrap();
        assert_eq!((e.pr, e.branch.as_str()), (None, "crew/old@h/5-x"));
        assert!(branch_in_refs(&json!([])).is_none());
    }

    #[test]
    fn a_report_list_names_the_earlier_work() {
        let pr = EarlierWork {
            pr: Some("https://github.com/o/r/pull/7".into()),
            branch: "b".into(),
        };
        assert_eq!(
            tagged("o/r#5", Some(&pr)),
            "o/r#5 (earlier PR https://github.com/o/r/pull/7)"
        );
        assert_eq!(tagged("o/r#5", None), "o/r#5");
    }
}
