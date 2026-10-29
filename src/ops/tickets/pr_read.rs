//! What GitHub says about a candidate PR, for the commands that judge a merged PR
//! (`owner_merge`, `merged_pr`, `merged_unverified`).

use crate::ctx::Ctx;
use crate::error::CbResult;
use crate::github::GhError;
use serde_json::Value;

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

    fn parse(repo: &str, number: u64, v: &Value) -> MergedPr {
        MergedPr {
            repo: repo.to_string(),
            number,
            merged: v.get("merged").and_then(Value::as_bool).unwrap_or(false)
                || v.get("merged_at").is_some_and(|m| !m.is_null()),
            body: v
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            merged_by: v
                .pointer("/merged_by/login")
                .and_then(Value::as_str)
                .map(str::to_string),
            merge_commit: v
                .get("merge_commit_sha")
                .and_then(Value::as_str)
                .map(str::to_string),
            head_sha: v
                .pointer("/head/sha")
                .and_then(Value::as_str)
                .map(str::to_string),
            base: v
                .pointer("/base/ref")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }
}

pub(super) fn read_pr(ctx: &Ctx, repo: &str, number: u64) -> CbResult<Option<MergedPr>> {
    match ctx.client.get(&format!("repos/{repo}/pulls/{number}")) {
        Ok(r) => Ok(Some(MergedPr::parse(repo, number, &r.body))),
        Err(GhError::Http { status: 404, .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
