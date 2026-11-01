//! Issues: the lists by label, and the timeline of one issue. A pull request in an issue list
//! (GitHub lists both) is left out.

use super::{Listed, repo_path};
use crate::github::paging::{MAX_PAGES, Paged, paged};
use crate::github::{Client, GhError};
use serde_json::Value;

/// One issue as a list or a read of the issue shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSummary {
    pub number: u64,
    pub node_id: String,
    pub title: String,
    pub body: String,
    pub open: bool,
    /// The conversation is locked.
    pub locked: bool,
}

impl IssueSummary {
    /// `None` for an entry without a number. A missing text is empty.
    fn from_json(i: &Value) -> Option<IssueSummary> {
        let s = |k: &str| i.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Some(IssueSummary {
            number: i.get("number")?.as_u64()?,
            node_id: s("node_id"),
            title: s("title"),
            body: s("body"),
            open: s("state") != "closed",
            locked: i.get("locked").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}

/// Which issues a whole list holds. The closed ones come as a sample ([`newest_closed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    All,
}

impl State {
    fn query(self) -> &'static str {
        match self {
            State::Open => "open",
            State::All => "all",
        }
    }
}

/// The issues of the pages, without pull requests and entries without a number.
fn summaries(pages: &Paged, what: &str) -> Result<Vec<IssueSummary>, GhError> {
    Ok(pages
        .items(what)?
        .into_iter()
        .filter(|i| i.get("pull_request").is_none())
        .filter_map(IssueSummary::from_json)
        .collect())
}

/// Issue `number` of `repo`.
pub fn get(client: &Client, repo: &str, number: u64) -> Result<IssueSummary, GhError> {
    let r = client.get(&format!("{}/issues/{number}", repo_path(repo)?))?;
    IssueSummary::from_json(&r.body)
        .ok_or_else(|| GhError::Shape(format!("issue {repo}#{number} has no number")))
}

/// Every issue of `repo` with `label` in `state`, all pages. A list cut at `MAX_PAGES` is an
/// error. `label` goes into the query as it is.
pub fn list_labelled(
    client: &Client,
    repo: &str,
    label: &str,
    state: State,
) -> Result<Vec<IssueSummary>, GhError> {
    let path = format!(
        "{}/issues?labels={label}&state={}&per_page=100",
        repo_path(repo)?,
        state.query()
    );
    let what = format!("issues of {repo}");
    summaries(&paged(client, &path, MAX_PAGES)?.complete(&what)?, &what)
}

/// The closed issues of `repo` with `label`, newest change first: one page of at most `n`, on
/// purpose. A sample of the recent ones, not the list.
pub fn newest_closed(
    client: &Client,
    repo: &str,
    label: &str,
    n: usize,
) -> Result<Vec<IssueSummary>, GhError> {
    let path = format!(
        "{}/issues?labels={label}&state=closed&sort=updated&direction=desc&per_page={n}",
        repo_path(repo)?
    );
    let mut list = summaries(&paged(client, &path, 1)?, "issue list")?;
    list.truncate(n);
    Ok(list)
}

/// An open issue of `repo` with `label`: the first of one page of 10, on purpose, as any
/// one of them will do.
pub fn any_open(client: &Client, repo: &str, label: &str) -> Result<Option<u64>, GhError> {
    let path = format!(
        "{}/issues?labels={label}&state=open&per_page=10",
        repo_path(repo)?
    );
    Ok(summaries(&paged(client, &path, 1)?, "issue list")?
        .first()
        .map(|i| i.number))
}

/// The events of the timeline of `issue`, oldest first, at most `MAX_PAGES` pages. GitHub adds
/// new events at the end, so a cut list (`truncated`) lacks the newest ones.
pub fn timeline(client: &Client, repo: &str, issue: u64) -> Result<Listed<Value>, GhError> {
    let path = format!("{}/issues/{issue}/timeline?per_page=100", repo_path(repo)?);
    let pages = paged(client, &path, MAX_PAGES)?;
    Ok(Listed {
        items: pages
            .items("issue timeline")?
            .into_iter()
            .cloned()
            .collect(),
        truncated: pages.truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Rest;
    use serde_json::json;

    fn one(body: Value) -> Paged {
        Paged {
            pages: vec![Rest {
                etag: None,
                link_next: None,
                body,
            }],
            truncated: false,
        }
    }

    #[test]
    fn an_issue_list_leaves_out_pull_requests_and_reads_the_state() {
        let p = one(json!([
            {"number": 1, "node_id": "I_1", "title": "a", "body": "x", "state": "open",
             "locked": true},
            {"number": 2, "title": "pr", "state": "open", "pull_request": {}},
            {"number": 3, "title": "c", "body": null, "state": "closed"},
            {"title": "no number"},
        ]));
        let got = summaries(&p, "list").unwrap();
        assert_eq!(
            got,
            [
                IssueSummary {
                    number: 1,
                    node_id: "I_1".into(),
                    title: "a".into(),
                    body: "x".into(),
                    open: true,
                    locked: true,
                },
                IssueSummary {
                    number: 3,
                    node_id: String::new(),
                    title: "c".into(),
                    body: String::new(),
                    open: false,
                    locked: false,
                },
            ]
        );
        let e = summaries(&one(json!({"message": "x"})), "issue list").unwrap_err();
        assert!(e.to_string().contains("issue list is not an array"), "{e}");
    }
}
