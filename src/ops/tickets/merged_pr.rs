//! A PR that was merged while the dev still works on it.
//!
//! GitHub accepts a push to the branch of a merged PR, but the push goes nowhere: the change is
//! not on the default branch. The PR still shows the old head, so a `done` with that head looked
//! fine. Two checks close the gap:
//!
//! - `done` refuses a merged PR with `pr-already-merged` before any write ([`refusal`]).
//! - `tick` tells the assignee of an In Progress ticket, by one signed `info`, that the PR of the
//!   ticket was merged ([`tick_lines`]). It posts once per (ticket, merge commit).
//!
//! The PR of an In Progress ticket is the PR in the `Evidence` field of its board item (written
//! by the latest `done`; no read). A ticket without that field costs no read. A ticket with it
//! costs one PR read per tick, until the dev was told about a merged PR: then the state cache
//! keeps the PR URL and the ticket costs nothing. A merged PR that is not told yet costs the read
//! of the ticket (issue, comments, item) and, once, the post.

use super::pr_read::{MergedPr, read_pr};
use super::{post, to_agent};
use crate::error::{CbError, CbResult};
use crate::ops::{AgentCtx, World};
use crate::protocol::names::{Kind, Re};
use crate::rules::Refusal;
use crate::rules::evidence::{PrFacts, parse_pr_url};
use crate::rules::status::Status;
use crate::services::ticket_loader::load_ticket;
use crate::state::StateCache;
use crate::ticket::TicketRow;
use crate::ticket::VMsg;

/// The refusal of `done` for a merged PR.
pub fn refusal(pr: &MergedPr) -> Refusal {
    Refusal::new(
        "pr-already-merged",
        format!(
            "PR {} was merged at {} by {}; push nothing more to its branch. For a remaining change open a new PR from the default branch and run done with it. If the owner merged it, the coordinator runs `crewboard close <n>` (the PR body must close the issue, for example `Closes #<n>`)",
            pr.url(),
            pr.merge_commit.as_deref().unwrap_or("unknown"),
            pr.merged_by.as_deref().unwrap_or("unknown"),
        ),
    )
}

/// `done` on a merged PR: refuse with the merge commit and the merge actor, which cost one more
/// read of the PR (only on this refusal path).
pub(super) fn check_not_merged(ac: &AgentCtx, pr: &PrFacts) -> CbResult<()> {
    if !pr.merged {
        return Ok(());
    }
    let m = read_pr(&ac.ctx, &pr.repo, pr.number)?.unwrap_or(MergedPr {
        repo: pr.repo.clone(),
        number: pr.number,
        merged: true,
        body: String::new(),
        merged_by: None,
        merge_commit: None,
        head_sha: Some(pr.head_sha.clone()),
        base: None,
    });
    Err(refusal(&m).into())
}

/// The text of the `info` to the assignee.
pub fn told_body(pr: &MergedPr) -> String {
    format!(
        "PR {} was merged by {} at {}; do not push to its branch; a remaining change needs a new PR from the default branch.\n",
        pr.url(),
        pr.merged_by.as_deref().unwrap_or("unknown"),
        pr.merge_commit.as_deref().unwrap_or("unknown"),
    )
}

/// Did a coordinator tell the assignee about this PR and merge commit already? The text names
/// both, so a second merge commit (another PR) is told again.
pub fn told(msgs: &[VMsg], pr: &MergedPr) -> bool {
    let want = told_body(pr);
    msgs.iter().any(|m| {
        m.sender.actor().is_coordinator()
            && m.kind == Some(Kind::Info)
            && m.body.trim() == want.trim()
    })
}

/// The PR URL in the `Evidence` field (`pr=<url> sha=...`).
pub fn pr_of_evidence(field: &str) -> Option<&str> {
    field
        .split_whitespace()
        .find_map(|w| w.strip_prefix("pr="))
        .filter(|u| !u.is_empty())
}

/// The lines of the headless `tick`: for each In Progress ticket with an assignee whose PR was
/// merged, ONE signed `info` to the assignee (once per ticket and merge commit).
pub(super) fn tick_lines(
    ac: &AgentCtx,
    world: &World,
    items: &[crate::items::ItemSnapshot],
    rows: &[TicketRow],
    cache: &mut StateCache,
) -> CbResult<String> {
    let mut out = String::new();
    for row in rows {
        if !row.in_status(Status::InProgress) || row.agent.is_none() {
            continue;
        }
        let Some(item) = items.iter().find(|i| i.id == row.item_id) else {
            continue;
        };
        let Some(url) = item.field("Evidence").and_then(pr_of_evidence) else {
            continue;
        };
        if cache.tick.merged_told.get(&row.item_id).map(String::as_str) == Some(url) {
            continue;
        }
        let Ok(u) = parse_pr_url(url) else {
            continue;
        };
        if !ac
            .ctx
            .cfg
            .repos
            .iter()
            .any(|r| r.eq_ignore_ascii_case(&u.repo))
        {
            continue;
        }
        let Some(pr) = read_pr(&ac.ctx, &u.repo, u.number)? else {
            continue;
        };
        if !pr.merged {
            continue;
        }
        let t = match load_ticket(&ac.ctx.reader(), &world.trust, &row.repo, row.issue) {
            Ok(t) => t,
            Err(CbError::Refused(_)) => continue,
            Err(e) => return Err(e),
        };
        // the ticket moved since the board listing: nothing to tell
        if !matches!(t.status(), Ok(Status::InProgress)) {
            continue;
        }
        let Some(agent) = t.agent().map(str::to_string) else {
            continue;
        };
        if !told(&t.msgs, &pr) {
            let p = post(
                ac,
                &t,
                to_agent(&agent)?,
                Kind::Info,
                Re::None,
                &told_body(&pr),
            )?;
            out.push_str(&format!(
                "{}: PR {} was merged by {} at {}; told {agent} (note {}, {})\n",
                t.key(),
                pr.url(),
                pr.merged_by.as_deref().unwrap_or("unknown"),
                pr.merge_commit.as_deref().unwrap_or("unknown"),
                p.id,
                p.url
            ));
        }
        cache
            .tick
            .merged_told
            .insert(row.item_id.clone(), url.to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::names::Re;
    use crate::rules::Role;
    use crate::ticket::Sender;

    fn pr() -> MergedPr {
        MergedPr {
            repo: "o/r".into(),
            number: 9,
            merged: true,
            body: String::new(),
            merged_by: Some("the-owner".into()),
            merge_commit: Some("mc1".into()),
            head_sha: Some("h1".into()),
            base: None,
        }
    }

    fn msg(sender: Sender, body: &str) -> VMsg {
        VMsg {
            ts: String::new(),
            comment_id: 1,
            created: 0,
            sender,
            to: "d@h".into(),
            kind: Some(Kind::Info),
            re: Re::None,
            body: body.into(),
        }
    }

    fn coord() -> Sender {
        Sender::Agent {
            name: "c@h".into(),
            role: Role::Coordinator,
        }
    }

    fn dev() -> Sender {
        Sender::Agent {
            name: "d@h".into(),
            role: Role::Dev,
        }
    }

    #[test]
    fn the_refusal_names_the_merge_and_what_to_do() {
        let r = refusal(&pr());
        assert_eq!(r.rule, "pr-already-merged");
        assert!(r.message.contains("crewboard close <n>"), "{}", r.message);
        assert_eq!(
            r.message,
            "PR https://github.com/o/r/pull/9 was merged at mc1 by the-owner; push nothing more to its branch. For a remaining change open a new PR from the default branch and run done with it. If the owner merged it, the coordinator runs `crewboard close <n>` (the PR body must close the issue, for example `Closes #<n>`)"
        );
    }

    #[test]
    fn the_note_is_told_once_per_pr_and_merge_commit() {
        let p = pr();
        let body = told_body(&p);
        assert_eq!(
            body.trim(),
            "PR https://github.com/o/r/pull/9 was merged by the-owner at mc1; do not push to its branch; a remaining change needs a new PR from the default branch."
        );
        assert!(!told(&[], &p));
        let posted = msg(coord(), &body);
        assert!(told(std::slice::from_ref(&posted), &p));
        // another merge commit is told again
        let mut p2 = p.clone();
        p2.merge_commit = Some("mc2".into());
        assert!(!told(std::slice::from_ref(&posted), &p2));
        // the same text from a dev agent is no note
        assert!(!told(&[msg(dev(), &body)], &p));
    }

    #[test]
    fn the_pr_comes_from_the_evidence_field() {
        assert_eq!(
            pr_of_evidence("pr=https://github.com/o/r/pull/9 sha=abc ci=none"),
            Some("https://github.com/o/r/pull/9")
        );
        assert_eq!(pr_of_evidence("sha=abc ci=none"), None);
        assert_eq!(pr_of_evidence(""), None);
        assert_eq!(pr_of_evidence("pr= sha=abc"), None);
    }
}
