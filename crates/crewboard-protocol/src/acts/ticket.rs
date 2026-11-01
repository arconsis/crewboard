//! The acts of a ticket log before the work ends: the creation info that puts the ticket on the
//! board, the assignment, the reassign notice, the park, the answer that takes a question back.

use super::{
    ASSIGNED_PREFIX, BOARD_ANNOUNCE_PREFIX, PARK_PREFIX, REASSIGNED_PREFIX, WITHDRAWS_QUESTION_LINE,
};

/// The body of the signed assignment message: the head, the signed `gate:` and `ci:` lines, then
/// `extra` (priority, order, reason and override lines, each with its final newline). `assign`
/// and `reassign` both write it.
pub fn assignment_body(
    title: &str,
    ticket_key: &str,
    item_id: &str,
    gate: Option<&str>,
    ci_none: Option<bool>,
    extra: &str,
) -> String {
    let gate = gate.map(|g| format!("gate: {g}\n")).unwrap_or_default();
    let ci = ci_none.map(ci_line).unwrap_or_default();
    format!("{ASSIGNED_PREFIX}: {title}\nticket: {ticket_key}\nitem={item_id}\n{gate}{ci}{extra}")
}

/// The `ci:` line that `assign` signs (`declared`: the body has `CI: none`) and that `reassign`
/// copies forward: `ci: none` or `ci: required`, with the final newline.
pub fn ci_line(declared: bool) -> &'static str {
    if declared {
        "ci: none\n"
    } else {
        "ci: required\n"
    }
}

/// The first line of the creation info that puts a ticket on the board.
pub fn board_announce_head() -> String {
    format!("{BOARD_ANNOUNCE_PREFIX}.")
}

/// A ticket's item id from the signed `kind=info` comment of the coordinator
/// (spec/board#item-ids-are-the-source-of-truth).
pub fn info_body(item_id: &str, project: u64) -> String {
    format!(
        "{}\nitem={item_id}\nproject={project}\n",
        board_announce_head()
    )
}

/// The line of the creation info that records the PR a new ticket adopts:
/// `adopts-pr: <url> branch: <branch>`.
pub const ADOPTS_PR: &str = "adopts-pr:";

/// [`info_body`] of a ticket that adopts an open PR of an older, Done ticket.
pub fn info_body_adopting(item_id: &str, project: u64, pr_url: &str, branch: &str) -> String {
    format!(
        "{}{ADOPTS_PR} {pr_url} branch: {branch}\n",
        info_body(item_id, project)
    )
}

/// The item id of an `item=PVTI_...` line of `body` (the first such line).
pub fn item_id_of(body: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let v = l.strip_prefix("item=")?.trim();
        v.starts_with("PVTI_").then(|| v.to_string())
    })
}

/// The `(url, branch)` of one `adopts-pr: <url> branch: <branch>` line, untrimmed; `None` for any
/// other line.
pub fn adopted_pr_of(line: &str) -> Option<(&str, &str)> {
    line.strip_prefix(ADOPTS_PR)?.trim().split_once(" branch: ")
}

/// The notice to the old assignee of a reassignment.
pub fn reassigned_body(reason: &str) -> String {
    format!("{REASSIGNED_PREFIX} Reason: {}", reason.trim())
}

/// What a dead, draining or replaced agent left on GitHub for a ticket: its crew branch and, when
/// there is one, the open PR of that branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlierWork {
    /// The URL of the open PR (`https://github.com/<repo>/pull/<n>`).
    pub pr: Option<String>,
    pub branch: String,
}

/// The line of the reassign notice that names the open PR: `open-pr: <url> branch: <branch>`.
const OPEN_PR_LINE: &str = "open-pr:";
/// The line of the reassign notice for a pushed branch without an open PR.
const EARLIER_BRANCH_LINE: &str = "earlier-branch:";

/// [`reassigned_body`] with the work that the old assignee left: the next line is
/// `open-pr: <url> branch: <branch>`, or `earlier-branch: <branch>` when there is no open PR.
pub fn reassigned_body_with(reason: &str, earlier: Option<&EarlierWork>) -> String {
    let mut s = reassigned_body(reason);
    match earlier {
        Some(EarlierWork {
            pr: Some(url),
            branch,
        }) => s.push_str(&format!("\n{OPEN_PR_LINE} {url} branch: {branch}\n")),
        Some(EarlierWork { pr: None, branch }) => {
            s.push_str(&format!("\n{EARLIER_BRANCH_LINE} {branch}\n"));
        }
        None => {}
    }
    s
}

/// The earlier work that a reassign notice names (`None`: it names none).
pub fn earlier_work_of(body: &str) -> Option<EarlierWork> {
    body.lines().find_map(|l| {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix(OPEN_PR_LINE) {
            let (url, branch) = rest.trim().split_once(" branch: ")?;
            let (url, branch) = (url.trim(), branch.trim());
            return (!url.is_empty() && !branch.is_empty()).then(|| EarlierWork {
                pr: Some(url.to_string()),
                branch: branch.to_string(),
            });
        }
        let branch = l.strip_prefix(EARLIER_BRANCH_LINE)?.trim();
        (!branch.is_empty()).then(|| EarlierWork {
            pr: None,
            branch: branch.to_string(),
        })
    })
}

/// The body of the answer that takes a question back.
pub fn withdraw_body(reason: &str) -> String {
    format!("{reason}\n{WITHDRAWS_QUESTION_LINE}\n")
}

/// The body of the park message: `parked for #<n>: <reason>`.
pub fn park_body(new_issue: u64, reason: &str) -> String {
    format!("{PARK_PREFIX}{new_issue}: {}", reason.trim())
}
