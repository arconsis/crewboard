//! The notices that end a ticket: a close after a passed verify, a merge by the coordinator, a
//! close after an owner merge, a drop by judgment.

use super::{CLOSED_VERIFIED, DROPPED_PREFIX, MERGED_PREFIX, OWNER_MERGED_HEAD, kv};

/// The notice of `close --verified`.
pub fn closed_verified_body() -> String {
    format!("{CLOSED_VERIFIED}.")
}

/// The notice of `merge` to the dev agent. `local` holds the local gates of the dev and of the
/// coordinator when they stood in for CI (spec/ci#local-run-ci_mode-local); the notice then
/// names them after the line `ci-mode: local`.
pub fn merged_body(
    pr: &str,
    sha: &str,
    method: &str,
    merge_commit: &str,
    branch: &str,
    local: Option<(&str, &str)>,
) -> String {
    let local = local.map_or(String::new(), |(dev, coordinator)| {
        format!("\nci-mode: local\nlocal-gate: {dev}\ncoordinator-local-gate: {coordinator}")
    });
    format!(
        "{MERGED_PREFIX}\npr: {pr}\nsha: {sha}\nmethod: {method}\nmerge-commit: {merge_commit}\nbranch: {branch}{local}"
    )
}

/// The text of the `info` of a close after an owner merge of the PR `pr_url`. A merge commit that
/// GitHub did not name is `unknown`; a reason is added as a line.
pub fn owner_merged_body(pr_url: &str, merge_commit: Option<&str>, reason: Option<&str>) -> String {
    let mut body = format!(
        "{OWNER_MERGED_HEAD}\npr: {pr_url}\nmerge-commit: {}\nmerged by the owner\nclosed: owner merged {pr_url}\n",
        merge_commit.unwrap_or("unknown"),
    );
    if let Some(r) = reason {
        body.push_str(&format!("reason: {r}\n"));
    }
    body
}

/// The notice of a drop by judgment (`close --why`): `dropped: <reason>`.
pub fn dropped_body(reason: &str) -> String {
    format!("{DROPPED_PREFIX} {reason}\n")
}

/// The notice of a drop of a ticket whose PR is merged, but not by the owner: the drop line of
/// [`dropped_body`], then the PR, its merge commit and the merge actor, with the keys of the
/// other notes about a merged PR. The ticket is Dropped all the same; the lines only record
/// that work was delivered. A merge commit that GitHub did not name is `unknown`.
pub fn dropped_merged_body(
    reason: &str,
    pr_url: &str,
    merge_commit: Option<&str>,
    merged_by: &str,
) -> String {
    format!(
        "{}pr: {pr_url}\nmerge-commit: {}\nmerged-by: {merged_by}\n",
        dropped_body(reason),
        merge_commit.unwrap_or("unknown"),
    )
}

/// The merged PR that a drop notice records ([`dropped_merged_body`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedPr {
    pub pr: String,
    pub merge_commit: String,
    pub merged_by: Option<String>,
}

/// The merged PR of a drop notice: the lines `pr:` and `merge-commit:` after the drop line.
/// `None` for a plain drop (the notice of every older build) and for any other text. The reason
/// is one line, the first, so its text is never read as a key.
pub fn dropped_merged_pr(body: &str) -> Option<DroppedPr> {
    let (first, rest) = body.split_once('\n')?;
    if !first.trim().starts_with(DROPPED_PREFIX) {
        return None;
    }
    Some(DroppedPr {
        pr: kv(rest, "pr")?,
        merge_commit: kv(rest, "merge-commit")?,
        merged_by: kv(rest, "merged-by"),
    })
}
