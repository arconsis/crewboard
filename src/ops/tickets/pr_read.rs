//! What GitHub says about a candidate PR, for the commands that judge a merged PR
//! (`owner_merge`, `merged_pr`, `merged_unverified`).

use crate::ctx::Ctx;
use crate::error::CbResult;
use crate::github::api::pulls;
use crate::github::optional;

/// What GitHub says about a candidate PR, as far as the judgement needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedPr {
    pub repo: String,
    pub number: u64,
    pub merged: bool,
    pub body: String,
    /// `merged_by.login`.
    pub merged_by: Option<String>,
    pub merge_commit: Option<String>,
    /// `head.sha`: the head of the PR (for a merged PR, the head that was merged).
    pub head_sha: Option<String>,
    /// `base.ref`: the base branch of the PR.
    pub base: Option<String>,
}

impl MergedPr {
    pub fn url(&self) -> String {
        format!("https://github.com/{}/pull/{}", self.repo, self.number)
    }

    fn from_pull(repo: &str, number: u64, p: pulls::Pull) -> MergedPr {
        MergedPr {
            repo: repo.to_string(),
            number,
            merged: p.merged,
            body: p.body.unwrap_or_default(),
            merged_by: p.merged_by,
            merge_commit: p.merge_commit_sha,
            head_sha: p.head_sha,
            base: p.base_ref,
        }
    }
}

pub(super) fn read_pr(ctx: &Ctx, repo: &str, number: u64) -> CbResult<Option<MergedPr>> {
    let got = optional(pulls::get(&ctx.client, repo, number))?;
    Ok(got.map(|p| MergedPr::from_pull(repo, number, p)))
}
