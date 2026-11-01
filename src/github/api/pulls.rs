//! Pull requests: the open list and the files of one PR.

use super::{Listed, repo_path};
use crate::github::paging::{MAX_PAGES, paged};
use crate::github::{Client, GhError, Rest};
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

fn open_path(repo: &str, order: Order) -> Result<String, GhError> {
    let sort = match order {
        Order::Created => "",
        Order::Updated => "&sort=updated&direction=desc",
    };
    Ok(format!(
        "{}/pulls?state=open&per_page=100{sort}",
        repo_path(repo)?
    ))
}

/// Every open PR of `repo`, all pages. A list cut at `MAX_PAGES` is an error. An entry without
/// the fields of [`PullSummary`] is left out.
pub fn list_open(client: &Client, repo: &str, order: Order) -> Result<Vec<PullSummary>, GhError> {
    let what = format!("open PRs of {repo}");
    let pages = paged(client, &open_path(repo, order)?, MAX_PAGES)?.complete(&what)?;
    Ok(pages
        .items(&what)?
        .into_iter()
        .filter_map(PullSummary::from_json)
        .collect())
}

/// The open PR list of one repo, read a page at a time and kept. A search reads the next page
/// only when the pages read so far hold no hit, so a search for the PR of a ticket costs one
/// read while that PR is on the first page, however many PRs the repo has.
#[derive(Debug, Clone)]
pub struct OpenPrPages {
    prs: Vec<PullSummary>,
    next: Option<String>,
    pages: usize,
}

impl OpenPrPages {
    /// The first page of the open PRs of `repo`.
    pub fn first(client: &Client, repo: &str, order: Order) -> Result<OpenPrPages, GhError> {
        let mut p = OpenPrPages {
            prs: Vec::new(),
            next: None,
            pages: 0,
        };
        p.add(client.get(&open_path(repo, order)?)?, repo)?;
        Ok(p)
    }

    /// The pages read so far.
    pub fn pages(&self) -> usize {
        self.pages
    }

    fn add(&mut self, page: Rest, repo: &str) -> Result<(), GhError> {
        let list = page
            .body
            .as_array()
            .ok_or_else(|| GhError::Shape(format!("open PRs of {repo} is not an array")))?;
        self.prs
            .extend(list.iter().filter_map(PullSummary::from_json));
        self.next = page.link_next;
        self.pages += 1;
        Ok(())
    }

    /// The first PR that `hit` accepts, in list order. Reads the next pages until one holds a
    /// hit or the list ends. A list cut at `MAX_PAGES` without a hit is an error.
    pub fn find(
        &mut self,
        client: &Client,
        repo: &str,
        hit: impl Fn(&PullSummary) -> bool,
    ) -> Result<Option<PullSummary>, GhError> {
        let mut from = 0;
        loop {
            if let Some(p) = self.prs[from..].iter().find(|p| hit(p)) {
                return Ok(Some(p.clone()));
            }
            from = self.prs.len();
            let Some(url) = self.next.clone() else {
                return Ok(None);
            };
            if self.pages >= MAX_PAGES {
                return Err(GhError::PageLimit(format!(
                    "open PRs of {repo} have more than {} pages",
                    self.pages
                )));
            }
            self.add(client.get(&url)?, repo)?;
        }
    }
}

/// The first open PR of `repo` that `hit` accepts, read a page at a time ([`OpenPrPages`]).
pub fn find_open(
    client: &Client,
    repo: &str,
    order: Order,
    hit: impl Fn(&PullSummary) -> bool,
) -> Result<Option<PullSummary>, GhError> {
    OpenPrPages::first(client, repo, order)?.find(client, repo, hit)
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
