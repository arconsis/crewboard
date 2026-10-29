//! The PR column of `status` (part 2): for each ticket in Review, and each ticket
//! In Progress that has a signed `done`, the PR it names with the head SHA, the CI word of
//! that head and the merge state. Green CI on the head does not mean that the PR can be
//! merged once the base has moved, so DIRTY is marked.
//!
//! Cost: one PR read and one CI read per listed ticket. The `done` messages come from the
//! feed that `status` reads anyway. The PR read, the state mapping and the CI word are the
//! ones of `merge` ([`judge_mergeable`]) and `resume` ([`ci_word`]).
//!
//! Unreadable data prints `unreadable`; a missing value is never guessed.

use super::restart::ci_word;
use super::tickets::merge::{Mergeable, judge_mergeable};
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::github::Client;
use crate::registry::Trust;
use crate::rules::evidence::parse_pr_url;
use crate::rules::status::Status;
use crate::services::ci_evidence::{CiQuery, fetch_ci};
use crate::ticket::{latest_done, verified_messages};
use serde_json::{Value, json};

/// The word for a value that could not be read.
pub const UNREADABLE: &str = "unreadable";

/// True for the statuses that get a PR line (still only with a signed `done`).
pub fn wants_pr(status: Option<&str>) -> bool {
    Status::Review.is(status) || Status::InProgress.is(status)
}

/// What one PR read gave. `None` is a value that could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrView {
    /// `owner/repo#n`.
    pub pr: String,
    pub head: Option<String>,
    /// The [`ci_word`] of the head.
    pub ci: Option<String>,
    /// The GitHub `mergeable_state` as read.
    pub merge: Option<String>,
}

/// The merge column: the state, with DIRTY marked.
pub fn merge_word(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return UNREADABLE.into();
    };
    match judge_mergeable(raw) {
        Mergeable::Refuse {
            state,
            conflict: true,
        } => format!("{state} (rebase needed)"),
        Mergeable::Refuse { state, .. } => state,
        Mergeable::Unknown => "UNKNOWN (GitHub still computes it)".into(),
        Mergeable::Go => raw.trim().to_ascii_uppercase(),
    }
}

pub fn is_dirty(raw: Option<&str>) -> bool {
    raw.is_some_and(|r| matches!(judge_mergeable(r), Mergeable::Refuse { conflict: true, .. }))
}

/// One text line. `key` and `status` are the ticket's.
pub fn render_line(key: &str, status: &str, v: &PrView) -> String {
    let head: String = v
        .head
        .as_deref()
        .map_or(UNREADABLE.into(), |h| h.chars().take(7).collect());
    format!(
        "  {key:<34} {status:<12} PR {} head {head}  CI {}  merge {}",
        v.pr,
        v.ci.as_deref().unwrap_or(UNREADABLE),
        merge_word(v.merge.as_deref()),
    )
}

/// The block for the text output: one line per ticket that has a PR. `None` entries (a ticket
/// without `done`) give no line. Empty when no ticket has one.
pub fn render_block(entries: &[(String, String, Option<PrView>)]) -> String {
    let lines: Vec<String> = entries
        .iter()
        .filter_map(|(k, s, v)| v.as_ref().map(|v| render_line(k, s, v)))
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "\npull requests ({}; one PR read and one CI read each):\n{}\n",
        lines.len(),
        lines.join("\n")
    )
}

/// The same fields for `--json`; `null` is unreadable.
pub fn to_json(ticket: &str, status: &str, v: &PrView) -> Value {
    json!({"ticket": ticket, "status": status, "pr": v.pr,
           "head_sha": v.head, "ci": v.ci, "merge_state": v.merge,
           "merge_dirty": is_dirty(v.merge.as_deref())})
}

/// The PR that the latest signed `done` of the assignee names, as `owner/repo` and number.
/// `None`: no `done` (nothing is guessed from branch names) or a PR URL that does not parse.
pub fn done_pr(
    comments: &[Comment],
    cfg: &BoardConfig,
    trust: &Trust,
    repo: &str,
    issue: u64,
    assignee: &str,
) -> Option<(String, u64)> {
    let mine: Vec<Comment> = comments
        .iter()
        .filter(|c| c.repo == repo && c.issue == issue)
        .cloned()
        .collect();
    let msgs = verified_messages(&mine, cfg, trust);
    let (_, ev) = latest_done(&msgs, assignee)?;
    parse_pr_url(&ev.pr).ok().map(|p| (p.repo, p.number))
}

/// Read one PR: the PR (head, base, merge state) and the CI of the head.
pub fn read_pr(client: &Client, repo: &str, number: u64) -> PrView {
    let pr = format!("{repo}#{number}");
    let Ok(r) = client.get(&format!("repos/{repo}/pulls/{number}")) else {
        return PrView {
            pr,
            head: None,
            ci: None,
            merge: None,
        };
    };
    let s = |p: &str| r.body.pointer(p).and_then(Value::as_str);
    let head = s("/head/sha").map(str::to_string);
    let merge = s("/mergeable_state").map(str::to_string);
    let ci = match (&head, s("/base/ref")) {
        (Some(h), Some(base)) => {
            let q = CiQuery {
                repo,
                sha: h,
                base,
                ..CiQuery::default()
            };
            fetch_ci(&|p| client.get(p), &q)
                .ok()
                .map(|(f, _)| ci_word(&f))
        }
        _ => None,
    };
    PrView {
        pr,
        head,
        ci,
        merge,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(head: Option<&str>, ci: Option<&str>, merge: Option<&str>) -> PrView {
        PrView {
            pr: "a/b#45".into(),
            head: head.map(str::to_string),
            ci: ci.map(str::to_string),
            merge: merge.map(str::to_string),
        }
    }

    #[test]
    fn dirty_is_marked_and_clean_is_not() {
        let dirty = render_line(
            "a/b#12",
            "Review",
            &view(Some("1a2b3c4d5e"), Some("green"), Some("dirty")),
        );
        assert!(dirty.contains("merge DIRTY (rebase needed)"), "{dirty}");
        let clean = render_line(
            "a/b#12",
            "Review",
            &view(Some("1a2b3c4d5e"), Some("green"), Some("clean")),
        );
        assert!(clean.contains("merge CLEAN"), "{clean}");
        assert!(!clean.contains("rebase"), "{clean}");
        assert!(clean.contains("head 1a2b3c4 "), "short head: {clean}");
        assert!(is_dirty(Some("DIRTY")) && !is_dirty(Some("clean")) && !is_dirty(None));
        // behind is named but is not a conflict
        assert_eq!(merge_word(Some("behind")), "BEHIND");
    }

    #[test]
    fn green_and_not_green_ci_words_differ() {
        let g = render_line(
            "a/b#1",
            "Review",
            &view(Some("abc1234"), Some("green"), Some("clean")),
        );
        let r = render_line(
            "a/b#1",
            "Review",
            &view(Some("abc1234"), Some("not green: build"), Some("clean")),
        );
        assert!(g.contains("CI green"), "{g}");
        assert!(r.contains("CI not green: build"), "{r}");
        assert!(!r.contains("CI green"), "{r}");
    }

    #[test]
    fn a_ticket_without_done_gets_no_pr_line() {
        let entries = vec![
            ("a/b#1".to_string(), "In Progress".to_string(), None),
            (
                "a/b#2".to_string(),
                "Review".to_string(),
                Some(view(Some("abc1234"), Some("green"), Some("clean"))),
            ),
        ];
        let out = render_block(&entries);
        assert!(out.contains("a/b#2"), "{out}");
        assert!(!out.contains("a/b#1"), "{out}");
        assert_eq!(render_block(&entries[..1]), "", "no PR at all: no block");
        assert!(wants_pr(Some("Review")) && wants_pr(Some("In Progress")));
        assert!(!wants_pr(Some("Ready")) && !wants_pr(Some("Done")) && !wants_pr(None));
    }

    #[test]
    fn unreadable_data_prints_unreadable_never_a_guess() {
        let l = render_line("a/b#1", "Review", &view(None, None, None));
        assert_eq!(l.matches(UNREADABLE).count(), 3, "{l}");
        assert!(!l.contains("green") && !l.contains("CLEAN"), "{l}");
        let j = to_json("a/b#1", "Review", &view(None, None, None));
        assert!(j["head_sha"].is_null() && j["ci"].is_null() && j["merge_state"].is_null());
        assert_eq!(j["merge_dirty"], false);
        let d = to_json(
            "a/b#1",
            "Review",
            &view(Some("x"), Some("green"), Some("dirty")),
        );
        assert_eq!(d["merge_dirty"], true);
    }
}
