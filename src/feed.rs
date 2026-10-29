//! Comment IO: the per-repo feed, one issue's comments, posting and deleting.
//!
//! The layer between `github.rs` (HTTP) and the callers (`ops/say.rs`, `ops/watch.rs`).
//! It reads with ETags and a cursor, and posts a comment once only.

use crate::comments::{Comment, UNKNOWN_EDITOR};
use crate::error::{CbError, CbResult};
use crate::github::paging::{MAX_PAGES, paged_from};
use crate::github::{
    Client, Conditional, GhError, Rest, TRANSIENT_ATTEMPTS, is_transient, retry_transient,
    with_attempts,
};
use crate::state::CachedComments;
use serde_json::{Value, json};

fn comments_of(rest: &Rest, repo: &str) -> CbResult<Vec<Comment>> {
    let arr = rest.body.as_array().ok_or_else(|| {
        GhError::Shape(format!(
            "comments feed of {repo} is not a list ({})",
            match &rest.body {
                Value::Object(o) => format!(
                    "object with keys {:?}",
                    o.keys().take(5).collect::<Vec<_>>()
                ),
                other => other.to_string().chars().take(80).collect(),
            }
        ))
    })?;
    Ok(arr
        .iter()
        .map(|v| Comment::from_json(v, repo))
        .collect::<Result<Vec<_>, _>>()?)
}

/// Read all pages starting at `first`. Later pages are read without the ETag.
fn read_all(client: &Client, first: Rest, repo: &str) -> CbResult<(Vec<Comment>, Option<String>)> {
    let etag = first.etag.clone();
    let paged = paged_from(client, first, MAX_PAGES)?.complete(&format!("comments of {repo}"))?;
    let mut out = Vec::new();
    for page in &paged.pages {
        out.extend(comments_of(page, repo)?);
    }
    Ok((out, etag))
}

/// GraphQL document that names the editors of issue comments by node id.
const EDITORS_QUERY: &str = "query($ids:[ID!]!){nodes(ids:$ids){... on IssueComment{id editor{login} userContentEdits(first:100){totalCount nodes{editor{login}}}}}}";

/// Comments per editor query (the `nodes` limit of GraphQL).
const EDITORS_BATCH: usize = 100;

/// The editors of one `IssueComment` node of the editors query: the last editor plus the editor
/// of every recorded edit. A missing editor (deleted account) or a history that is longer than
/// the page counts as [`UNKNOWN_EDITOR`], so it never passes as the owner.
pub fn editors_of_node(node: &Value) -> Vec<String> {
    let login = |v: &Value| {
        v.pointer("/editor/login")
            .and_then(Value::as_str)
            .map_or_else(|| UNKNOWN_EDITOR.to_string(), str::to_string)
    };
    let mut out = Vec::new();
    if !node["editor"].is_null() {
        out.push(login(node));
    }
    let edits = node.pointer("/userContentEdits");
    let listed = edits
        .and_then(|e| e.get("nodes"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    out.extend(listed.iter().map(login));
    let total = edits
        .and_then(|e| e.get("totalCount"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if total > listed.len() as u64 {
        out.push(UNKNOWN_EDITOR.to_string());
    }
    out
}

/// Read the editors of every edited owner comment that does not know them yet. One GraphQL
/// call per 100 such comments; no call when there is none (the normal case). A comment that
/// GitHub does not return stays unresolved, which the readers treat as untrusted. A failed
/// call is an error: the caller keeps its cursor and reads again, so no owner message is lost
/// to a transient failure.
pub fn resolve_owner_edits(client: &Client, comments: &mut [Comment], owner: &str) -> CbResult<()> {
    let ids: Vec<String> = comments
        .iter()
        .filter(|c| c.needs_editor_read(owner))
        .map(|c| c.node_id.clone())
        .collect();
    for chunk in ids.chunks(EDITORS_BATCH) {
        let data = client.graphql(EDITORS_QUERY, json!({ "ids": chunk }))?;
        let nodes = data
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| GhError::Shape("editors query: `nodes` is not a list".into()))?;
        for node in nodes.iter().filter(|n| !n.is_null()) {
            let Some(id) = node.get("id").and_then(Value::as_str) else {
                continue;
            };
            let editors = editors_of_node(node);
            for c in comments.iter_mut().filter(|c| c.node_id == id) {
                c.editors = Some(editors.clone());
            }
        }
    }
    Ok(())
}

/// The result of reading a feed.
#[derive(Debug, Clone)]
pub struct Feed {
    /// Ascending by (`updated_at`, id).
    pub comments: Vec<Comment>,
    pub etag: Option<String>,
}

/// `GET /repos/{r}/issues/comments?since=&sort=updated` for one repo
/// (spec/events#polling-and-rate-limits).
///
/// The list is read newest first, so a new comment always lands on page 1 and the ETag of
/// page 1 changes (an ascending list would put it on the last page, behind an unchanged
/// page 1). The result is sorted ascending here.
pub fn feed_since(
    client: &Client,
    repo: &str,
    since_epoch: i64,
    etag: Option<&str>,
    owner: &str,
) -> CbResult<Feed> {
    let since = crate::clock::format_ts(since_epoch);
    let path = format!(
        "repos/{repo}/issues/comments?since={since}&sort=updated&direction=desc&per_page=100"
    );
    match client.get_conditional(&path, etag)? {
        Conditional::NotModified => Ok(Feed {
            comments: Vec::new(),
            etag: etag.map(str::to_string),
        }),
        Conditional::Modified(first) => {
            let (mut comments, etag) = read_all(client, first, repo)?;
            resolve_owner_edits(client, &mut comments, owner)?;
            comments.sort_by_key(|c| (c.updated_epoch, c.id));
            Ok(Feed { comments, etag })
        }
    }
}

/// The ETag of page 1 covers the whole list only while the list fits on one page: the issue
/// comments come oldest first, so a new comment lands on the LAST page and page 1 stays
/// unchanged (a registry past 100 comments then hid new agents behind a 304). A full first page means
/// "do not trust the ETag".
pub fn etag_covers_all(c: &CachedComments) -> bool {
    c.comments.len() < 100
}

/// All comments of one issue, ascending by id. Uses the cache when GitHub answers 304.
pub fn issue_comments(
    client: &Client,
    repo: &str,
    issue: u64,
    cache: Option<&CachedComments>,
    owner: &str,
) -> CbResult<(Vec<Comment>, Option<String>)> {
    let path = format!("repos/{repo}/issues/{issue}/comments?per_page=100");
    match client.get_conditional(
        &path,
        cache
            .filter(|c| etag_covers_all(c))
            .map(|c| c.etag.as_str()),
    )? {
        Conditional::NotModified => match cache {
            Some(c) => {
                // A cache from an older run may hold an edited owner comment without editors.
                let mut comments = c.comments.clone();
                resolve_owner_edits(client, &mut comments, owner)?;
                Ok((comments, Some(c.etag.clone())))
            }
            None => Err(GhError::Shape("304 without a cached body".into()).into()),
        },
        Conditional::Modified(first) => {
            let (mut comments, etag) = read_all(client, first, repo)?;
            resolve_owner_edits(client, &mut comments, owner)?;
            comments.sort_by_key(|c| c.id);
            Ok((comments, etag))
        }
    }
}

/// Post a comment. Returns `(comment id, html url)`.
pub fn post_comment(
    client: &Client,
    repo: &str,
    issue: u64,
    body: &str,
) -> CbResult<(u64, String)> {
    let me = client.viewer_login()?;
    let r = post_once_safely(
        crate::clock::pause,
        || {
            client.post(
                &format!("repos/{repo}/issues/{issue}/comments"),
                &json!({"body": body}),
            )
        },
        || {
            let (all, _) = issue_comments(client, repo, issue, None, "").map_err(|e| match e {
                CbError::Network(g) => g,
                other => GhError::Shape(other.to_string()),
            })?;
            Ok(all
                .into_iter()
                .find(|c| c.body == body && c.by(&me))
                .map(|c| Rest {
                    etag: None,
                    link_next: None,
                    body: json!({"id": c.id, "html_url": c.url}),
                }))
        },
    )?;
    let id = r
        .body
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| CbError::from(GhError::Shape("posted comment has no id".into())))?;
    let url = r
        .body
        .get("html_url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok((id, url))
}

/// Post a comment at most once. A comment is not idempotent: a request that failed with a
/// transient error may have reached GitHub. So after such a failure, look for a comment with
/// the same body by this account first (the signed header carries ts, kind and re, so the
/// body is unique); post again only if it is not there. After `TRANSIENT_ATTEMPTS` the
/// transient error is returned (exit 3) with the number of attempts.
pub fn post_once_safely(
    mut sleep: impl FnMut(std::time::Duration),
    mut post: impl FnMut() -> Result<Rest, GhError>,
    mut find_existing: impl FnMut() -> Result<Option<Rest>, GhError>,
) -> Result<Rest, GhError> {
    let mut n = 0;
    loop {
        n += 1;
        match post() {
            Err(e) if is_transient(&e) => {
                if n >= TRANSIENT_ATTEMPTS {
                    return Err(with_attempts(e, n));
                }
                eprintln!(
                    "crewboard: transient failure posting a comment, attempt {n} of {TRANSIENT_ATTEMPTS}: {e}"
                );
                sleep(std::time::Duration::from_secs(if n == 1 { 2 } else { 5 }));
                // The lookup is a read and is repeated on its own. If it cannot be read, we do
                // NOT post again (we cannot tell whether the first one arrived): exit 3.
                let found = retry_transient(TRANSIENT_ATTEMPTS, &mut sleep, &mut find_existing)?;
                if let Some(found) = found {
                    return Ok(found);
                }
            }
            other => return other,
        }
    }
}

/// Delete one of our own comments (selftest cleanup, test cleanup).
pub fn delete_comment(client: &Client, repo: &str, id: u64) -> CbResult<()> {
    client.send(
        "DELETE",
        &format!("repos/{repo}/issues/comments/{id}"),
        None,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{editors_of_node, post_once_safely};
    use crate::github::{GhError, Rest};
    use serde_json::json;
    use std::cell::Cell;

    fn ok_rest(id: u64) -> Rest {
        Rest {
            etag: None,
            link_next: None,
            body: json!({"id": id, "html_url": "u"}),
        }
    }

    fn net() -> GhError {
        GhError::Network("error sending request".into())
    }

    #[test]
    fn a_transient_failure_before_it_reached_github_posts_once_after_the_retry() {
        let (posts, landed) = (Cell::new(0), Cell::new(0));
        let r = post_once_safely(
            |_| {},
            || {
                posts.set(posts.get() + 1);
                if posts.get() == 1 {
                    Err(net()) // never arrived
                } else {
                    landed.set(landed.get() + 1);
                    Ok(ok_rest(7))
                }
            },
            || {
                Ok(if landed.get() > 0 {
                    Some(ok_rest(7))
                } else {
                    None
                })
            },
        );
        assert_eq!(r.unwrap().body["id"], 7);
        assert_eq!(
            (posts.get(), landed.get()),
            (2, 1),
            "exactly one comment landed"
        );
    }

    #[test]
    fn a_post_that_failed_after_reaching_github_is_not_posted_twice() {
        let (posts, landed) = (Cell::new(0), Cell::new(0));
        let r = post_once_safely(
            |_| {},
            || {
                posts.set(posts.get() + 1);
                landed.set(landed.get() + 1); // GitHub stored it, the answer was lost
                Err(net())
            },
            || {
                Ok(if landed.get() > 0 {
                    Some(ok_rest(9))
                } else {
                    None
                })
            },
        );
        assert_eq!(
            r.unwrap().body["id"],
            9,
            "the comment that arrived is returned"
        );
        assert_eq!((posts.get(), landed.get()), (1, 1), "no second post");
    }

    #[test]
    fn a_persistent_failure_is_exit_3_with_the_attempts_and_posts_nothing_extra() {
        let posts = Cell::new(0);
        let e = post_once_safely(
            |_| {},
            || {
                posts.set(posts.get() + 1);
                Err(net())
            },
            || Ok(None),
        )
        .unwrap_err();
        assert!(
            matches!(&e, GhError::Network(m) if m.contains("after 3 attempts")),
            "{e}"
        );
        assert_eq!(posts.get(), 3);
    }

    #[test]
    fn if_the_lookup_cannot_be_read_nothing_is_posted_again() {
        let posts = Cell::new(0);
        let e = post_once_safely(
            |_| {},
            || {
                posts.set(posts.get() + 1);
                Err(net())
            },
            || Err(net()),
        )
        .unwrap_err();
        assert!(matches!(e, GhError::Network(_)));
        assert_eq!(
            posts.get(),
            1,
            "unknown whether it arrived: do not post again"
        );
    }

    #[test]
    fn a_non_transient_error_is_returned_at_once() {
        let posts = Cell::new(0);
        let e = post_once_safely(
            |_| {},
            || {
                posts.set(posts.get() + 1);
                Err(GhError::Http {
                    status: 422,
                    message: "x".into(),
                    accepted_permissions: None,
                })
            },
            || Ok(None),
        )
        .unwrap_err();
        assert!(matches!(e, GhError::Http { status: 422, .. }));
        assert_eq!(posts.get(), 1);
    }

    use super::*;

    fn cached(n: usize) -> CachedComments {
        CachedComments {
            etag: "\"x\"".into(),
            comments: (0..n)
                .map(|i| {
                    Comment::from_json(
                        &json!({"id": i as u64 + 1, "body": "b", "created_at": "2026-10-16T08:00:00Z",
                        "updated_at": "2026-10-16T08:00:00Z", "user": {"login": "u"},
                        "issue_url": "https://api.github.com/repos/o/r/issues/2",
                        "html_url": "https://github.com/o/r/issues/2#c"}),
                        "o/r",
                    )
                    .unwrap()
                })
                .collect(),
        }
    }

    #[test]
    fn an_etag_is_trusted_on_one_page_and_never_on_a_full_first_page() {
        assert!(etag_covers_all(&cached(99)));
        assert!(!etag_covers_all(&cached(100)));
        assert!(!etag_covers_all(&cached(162)));
    }

    #[test]
    fn a_feed_that_is_an_object_is_a_shape_error_not_an_empty_list() {
        let rest = Rest {
            etag: None,
            link_next: None,
            body: json!({"message": "Not Found", "documentation_url": "x"}),
        };
        let e = comments_of(&rest, "a/b").unwrap_err();
        assert!(e.to_string().contains("not a list"), "{e}");
        // control: a list parses
        let ok = Rest {
            body: json!([]),
            ..rest
        };
        assert!(comments_of(&ok, "a/b").unwrap().is_empty());
    }

    #[test]
    fn the_editors_of_a_node_are_the_last_editor_and_every_listed_edit() {
        let node = json!({
            "id": "IC_1",
            "editor": {"login": "owner-login"},
            "userContentEdits": {"totalCount": 2, "nodes": [
                {"editor": {"login": "owner-login"}},
                {"editor": {"login": "crew-bot"}}
            ]}
        });
        assert_eq!(
            editors_of_node(&node),
            vec!["owner-login", "owner-login", "crew-bot"]
        );
    }

    #[test]
    fn a_missing_editor_or_a_cut_off_history_never_passes_as_the_owner() {
        let deleted = json!({"editor": null, "userContentEdits": {"totalCount": 1,
            "nodes": [{"editor": null}]}});
        assert_eq!(editors_of_node(&deleted), vec!["(unknown)"]);
        let long = json!({"editor": {"login": "o"}, "userContentEdits": {"totalCount": 150,
            "nodes": [{"editor": {"login": "o"}}]}});
        assert_eq!(editors_of_node(&long), vec!["o", "o", "(unknown)"]);
        // no edit recorded at all: nothing to prove the owner did it
        let none = json!({"editor": null, "userContentEdits": {"totalCount": 0, "nodes": []}});
        assert!(editors_of_node(&none).is_empty());
    }
}
