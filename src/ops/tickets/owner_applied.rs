//! Is the workflow change of an owner-route ticket applied? The one judgement for the safety
//! net (`owner_net`) and for `close` on the owner route.
//!
//! The owner route delivers a workflow change as a patch file in the PR. Two steps, both
//! fail-closed (a doubt is "not applied", never "applied"):
//!
//! 1. [`tied`]: the proof belongs to THIS ticket. The patch that the `done` names must be
//!    `owner_route::patch_path(issue)`, and the PR must contain that file. A `done` that names
//!    another patch, or a PR without it (an old PR of another ticket), proves nothing.
//! 2. [`patch_applied`]: the change is live. EITHER the merged PR changed a file under
//!    `.github/workflows/` (the owner pushed the patch to the PR branch), OR the content of the
//!    patch is on the base branch (the owner applied it there with a commit of their own, then
//!    merged the patch-only PR). The content check: the patch is read at the head of the PR;
//!    for each workflow file that it names (at most [`MAX_FILES`]; more is "not applied") the
//!    file is read at the head of the base branch, and every hunk of the patch for that file
//!    must be in it ([`hunks_applied`]: the new side of the hunk is one block of the file, and
//!    the old side of a hunk that removes lines is not). All named files must pass. Not applied: a patch that
//!    cannot be read, is over [`MAX_BYTES`] or is malformed, that names no workflow file, that
//!    has no hunk for a file, or a path that is not a plain workflow path
//!    ([`valid_workflow_path`]: the path is text of the agent). The check never widens to the
//!    whole directory and never looks at commit messages or times.
//!
//! Reads: none when the PR changed a workflow file; else one read of the patch and one read per
//! named file (at most [`MAX_FILES`]), so at most 6. The caller reads the PR and its files.

use super::AgentCtx;
use crate::ctx::decode_contents;
use crate::error::CbResult;
use crate::github::GhError;
use crate::owner_route::{
    hunks_applied, is_workflow_path, patch_hunks, patch_path, patch_workflow_files,
    valid_workflow_path,
};
use crate::rules::text::url_encode;

/// Workflow files of one patch that are looked up on the base branch. A patch that names more
/// is not applied.
const MAX_FILES: usize = 5;

/// The largest patch or workflow file that is checked (the contents API gives at most 1 MB).
const MAX_BYTES: usize = 1_000_000;

/// What the judgement reads about the merged PR.
pub(crate) struct Proof<'a> {
    /// The repo of the PR, and the base branch of the PR.
    pub repo: &'a str,
    pub base: &'a str,
    /// The head of the PR, where the patch file is read.
    pub head_sha: &'a str,
    /// The patch that the `done` names, and the files of the PR.
    pub patch: &'a str,
    pub pr_files: &'a [String],
}

/// Step 1, first half (no read): the patch that the `done` names is the patch of this ticket.
pub(crate) fn names_this_ticket(issue: u64, patch: &str) -> Result<(), String> {
    let want = patch_path(issue);
    if patch == want {
        return Ok(());
    }
    Err(format!(
        "the owner-route line of the done names {patch}, not the patch of this ticket ({want})"
    ))
}

/// Step 1: the patch of the `done` is the patch of this ticket, and the PR contains it.
pub(crate) fn tied(issue: u64, patch: &str, pr_files: &[String]) -> Result<(), String> {
    names_this_ticket(issue, patch)?;
    let want = patch_path(issue);
    if !pr_files.iter().any(|f| f == &want) {
        return Err(format!(
            "the PR does not contain {want}, so it is no delivery of this ticket"
        ));
    }
    Ok(())
}

/// Step 2. `Ok(Ok(how))`: applied, and how. `Ok(Err(why))`: not applied, and why (one line).
/// `Err`: a read failed for a reason that says nothing about the patch (a network or server
/// error); nothing is judged then.
pub(crate) fn patch_applied(ac: &AgentCtx, p: &Proof<'_>) -> CbResult<Result<String, String>> {
    if let Some(f) = p.pr_files.iter().find(|f| is_workflow_path(f)) {
        return Ok(Ok(format!("the PR changed {f}")));
    }
    Ok(content_applied(ac, p)?
        .map_err(|why| format!("the PR changed no file under .github/workflows/, and {why}")))
}

/// The content check on the base branch; `Err(why)` completes "... and why".
fn content_applied(ac: &AgentCtx, p: &Proof<'_>) -> CbResult<Result<String, String>> {
    let patch = match read_text(ac, p.repo, p.patch, p.head_sha)? {
        Ok(t) => t,
        Err(why) => return Ok(Err(format!("the patch cannot be read ({why})"))),
    };
    let files = patch_workflow_files(&patch);
    if files.is_empty() {
        return Ok(Err(format!("{} names no workflow file", p.patch)));
    }
    if files.len() > MAX_FILES {
        return Ok(Err(format!(
            "{} names {} workflow files, more than the {MAX_FILES} that are checked",
            p.patch,
            files.len()
        )));
    }
    if let Some(bad) = files.iter().find(|f| !valid_workflow_path(f)) {
        return Ok(Err(format!(
            "{} names the path {bad:?}, which is no plain workflow file path",
            p.patch
        )));
    }
    for f in &files {
        let hunks = match patch_hunks(&patch, f) {
            Ok(h) => h,
            Err(why) => return Ok(Err(format!("{} is no valid patch ({why})", p.patch))),
        };
        if hunks.is_empty() {
            return Ok(Err(format!("{} has no hunk for {f}", p.patch)));
        }
        let text = match read_text(ac, p.repo, f, p.base)? {
            Ok(t) => t,
            Err(why) => return Ok(Err(format!("{f} cannot be read ({why})"))),
        };
        if let Err(why) = hunks_applied(&text, &hunks) {
            return Ok(Err(format!(
                "{f} on {} lacks the change of {}: {why}",
                p.base, p.patch
            )));
        }
    }
    Ok(Ok(format!(
        "{} has the change that {} makes to {}",
        p.base,
        p.patch,
        files.join(", ")
    )))
}

/// A repo file at a ref. `Ok(Err(why))`: it cannot be read as text (missing, no content, over
/// the size limit, not UTF-8). `Err`: any other failure of the request.
fn read_text(
    ac: &AgentCtx,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> CbResult<Result<String, String>> {
    let url = format!(
        "repos/{repo}/contents/{}?ref={}",
        url_encode(path, true),
        url_encode(git_ref, false)
    );
    match ac.ctx.client.get(&url) {
        Ok(r) => Ok(decode_contents(&r.body)
            .map_err(|why| format!("the file {why}"))
            .and_then(|t| {
                if t.len() > MAX_BYTES {
                    Err(format!("the file is over {MAX_BYTES} bytes"))
                } else {
                    Ok(t)
                }
            })),
        Err(GhError::Http { status: 404, .. }) => Ok(Err(format!("it is not at {git_ref}"))),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_proof_is_tied_to_the_patch_of_this_ticket_in_the_pr() {
        let ok = files(&["src/a.rs", "docs/patches/7-workflow.patch"]);
        assert!(tied(7, "docs/patches/7-workflow.patch", &ok).is_ok());
        // the patch name of another ticket
        let e = tied(7, "docs/patches/8-workflow.patch", &ok).unwrap_err();
        assert!(e.contains("not the patch of this ticket"), "{e}");
        // any other file name
        assert!(tied(7, "x.patch", &ok).is_err());
        // the PR does not contain the patch
        let e = tied(7, "docs/patches/7-workflow.patch", &files(&["src/a.rs"])).unwrap_err();
        assert!(e.contains("does not contain"), "{e}");
    }

    #[test]
    fn a_path_is_percent_encoded_for_a_url() {
        assert_eq!(url_encode("feature/x", false), "feature%2Fx");
        assert_eq!(
            url_encode(".github/workflows/ci.yml", true),
            ".github/workflows/ci.yml"
        );
        assert_eq!(url_encode("a#b?c&d e", true), "a%23b%3Fc%26d%20e");
    }
}
