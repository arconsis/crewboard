//! Pull requests: the open list and the files of one PR.

use super::{Listed, repo_path};
use crate::github::paging::{MAX_PAGES, paged};
use crate::github::{Client, GhError};
use serde_json::Value;

/// One PR as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullSummary {
    pub number: u64,
    /// The branch of the head.
    pub head_ref: String,
    pub head_sha: String,
    /// The branch that the PR merges into.
    pub base_ref: String,
}

impl PullSummary {
    /// `None` for an entry without a number, head branch, head SHA or base branch.
    fn from_json(p: &Value) -> Option<PullSummary> {
        let s = |ptr: &str| p.pointer(ptr).and_then(Value::as_str).map(str::to_string);
        Some(PullSummary {
            number: p.get("number")?.as_u64()?,
            head_ref: s("/head/ref")?,
            head_sha: s("/head/sha")?,
            base_ref: s("/base/ref")?,
        })
    }
}

/// The order of the open PR list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// The order of GitHub: the newest PR first.
    Created,
    /// The PR with the newest change first.
    Updated,
}

/// Every open PR of `repo`, all pages. A list cut at `MAX_PAGES` is an error. An entry without
/// the fields of [`PullSummary`] is left out.
pub fn list_open(client: &Client, repo: &str, order: Order) -> Result<Vec<PullSummary>, GhError> {
    let sort = match order {
        Order::Created => "",
        Order::Updated => "&sort=updated&direction=desc",
    };
    let path = format!("{}/pulls?state=open&per_page=100{sort}", repo_path(repo)?);
    let what = format!("open PRs of {repo}");
    let pages = paged(client, &path, MAX_PAGES)?.complete(&what)?;
    Ok(pages
        .items(&what)?
        .into_iter()
        .filter_map(PullSummary::from_json)
        .collect())
}

/// One file of a PR: its path, and the old path of a renamed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullFile {
    pub filename: String,
    pub previous_filename: Option<String>,
}

/// The files of PR `number`, at most `max_pages` pages of 100. `truncated`: more files were
/// left after the last page read.
pub fn files(
    client: &Client,
    repo: &str,
    number: u64,
    max_pages: usize,
) -> Result<Listed<PullFile>, GhError> {
    let path = format!("{}/pulls/{number}/files?per_page=100", repo_path(repo)?);
    let pages = paged(client, &path, max_pages)?;
    let items = pages
        .items(&format!("the file list of {repo}#{number}"))?
        .into_iter()
        .filter_map(|f| {
            let s = |k: &str| f.get(k).and_then(Value::as_str).map(str::to_string);
            Some(PullFile {
                filename: s("filename")?,
                previous_filename: s("previous_filename"),
            })
        })
        .collect();
    Ok(Listed {
        items,
        truncated: pages.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pr_entry_needs_number_head_and_base() {
        let p = json!({"number": 7, "head": {"ref": "crew/a/5-x", "sha": "abc"},
                       "base": {"ref": "main"}});
        assert_eq!(
            PullSummary::from_json(&p),
            Some(PullSummary {
                number: 7,
                head_ref: "crew/a/5-x".into(),
                head_sha: "abc".into(),
                base_ref: "main".into(),
            })
        );
        for broken in [
            json!({"head": {"ref": "b", "sha": "s"}, "base": {"ref": "main"}}),
            json!({"number": 7, "head": {"sha": "s"}, "base": {"ref": "main"}}),
            json!({"number": 7, "head": {"ref": "b", "sha": "s"}}),
        ] {
            assert_eq!(PullSummary::from_json(&broken), None, "{broken}");
        }
    }
}
