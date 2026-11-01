//! The notices that end a ticket: a close after a passed verify, a merge by the coordinator, a
//! close after an owner merge, a drop by judgment.

use super::{CLOSED_VERIFIED, DROPPED_PREFIX, MERGED_PREFIX, OWNER_MERGED_HEAD};

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
