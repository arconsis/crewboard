//! `doctor --repo-move` (spec/messages#repository-moves): a read-only dry run for the
//! owner, before and after a repo transfer or rename.
//!
//! For the registry issue, the open tickets and a sample of the newest closed tickets it counts
//! the signed messages that verify under the current repo name and under each former name of
//! AUTHORITY.md, how many the crew accepts (the rule of `former_repos`, cutoff included), and
//! lists the messages that verify under no name. It posts and changes nothing.

use super::resolve_board_config;
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::ctx::{Global, fetch_authority_text, new_client};
use crate::error::CbResult;
use crate::feed;
use crate::github::paging::{MAX_PAGES, Paged, paged};
use crate::github::{Client, GhError};
use crate::protocol::former::{FormerRepo, under_name, verify_with_former};
use crate::protocol::header::Message;
use crate::protocol::signature::verify_message;
use crate::registry::{CREW_PREFIX, parse_join};
use crate::ticket::WORK_LABEL;
use ssh_key::PublicKey;
use std::collections::HashMap;
use std::fmt::Write as _;

/// How many of the newest closed tickets are sampled per repo.
pub const CLOSED_SAMPLE: usize = 20;

/// One signed message that verifies under no name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub issue: String,
    pub comment_id: u64,
    pub why: String,
}

/// The counts of one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueCount {
    pub key: String,
    /// Comments with a crew header from a crew author.
    pub messages: usize,
    /// Valid under the current name, then under each former name in list order. A former name
    /// counts here without its cutoff: the column shows what the old name would verify.
    pub under: Vec<usize>,
    /// What the crew accepts: current name, or a former name whose cutoff is after the `ts`.
    pub accepted: usize,
    pub orphans: Vec<Orphan>,
}

/// The public keys of the join messages of the registry issue, by agent name. The first join of
/// a name wins, as in the registry.
pub fn join_keys(registry: &[Comment], authors: &[String]) -> HashMap<String, PublicKey> {
    let mut sorted: Vec<&Comment> = registry.iter().collect();
    sorted.sort_by_key(|c| c.id);
    let mut keys = HashMap::new();
    for c in sorted {
        if !c.body.starts_with(CREW_PREFIX) || !authors.iter().any(|a| c.by(a)) {
            continue;
        }
        let Ok(msg) = Message::parse(&c.body) else {
            continue;
        };
        if let Some(k) = parse_join(&msg, c.id).and_then(|j| j.key()) {
            keys.entry(msg.header.from.to_string()).or_insert(k);
        }
    }
    keys
}

/// Count the messages of one issue. `current` is the current `owner/repo` of the issue;
/// `former` applies to it only if it is the main repo (`former_apply`).
pub fn count_issue(
    current: &str,
    number: u64,
    comments: &[Comment],
    authors: &[String],
    keys: &HashMap<String, PublicKey>,
    former: &[FormerRepo],
) -> IssueCount {
    let key = format!("{current}#{number}");
    let mut out = IssueCount {
        key: key.clone(),
        messages: 0,
        under: vec![0; 1 + former.len()],
        accepted: 0,
        orphans: Vec::new(),
    };
    let Ok(cur) = under_name(current, number) else {
        return out;
    };
    let mut sorted: Vec<&Comment> = comments.iter().collect();
    sorted.sort_by_key(|c| c.id);
    for c in sorted {
        if !c.body.starts_with(CREW_PREFIX) || !authors.iter().any(|a| c.by(a)) {
            continue;
        }
        out.messages += 1;
        let orphan = |why: &str| Orphan {
            issue: key.clone(),
            comment_id: c.id,
            why: why.to_string(),
        };
        let Ok(msg) = Message::parse(&c.body) else {
            out.orphans.push(orphan("malformed header"));
            continue;
        };
        let Some(pk) = keys.get(msg.header.from.as_str()) else {
            out.orphans.push(orphan(&format!(
                "no join key for {}",
                msg.header.from.as_str()
            )));
            continue;
        };
        if c.edited() {
            out.orphans.push(orphan("edited after posting"));
            continue;
        }
        if verify_message(&msg, &cur, pk).is_ok() {
            out.under[0] += 1;
        }
        for (i, f) in former.iter().enumerate() {
            if under_name(&f.name, number).is_ok_and(|r| verify_message(&msg, &r, pk).is_ok()) {
                out.under[i + 1] += 1;
            }
        }
        if verify_with_former(&msg, &cur, former, pk).is_ok() {
            out.accepted += 1;
        } else {
            let after = former.iter().any(|f| {
                !f.covers(&msg.header.ts)
                    && under_name(&f.name, number)
                        .is_ok_and(|r| verify_message(&msg, &r, pk).is_ok())
            });
            out.orphans.push(orphan(if after {
                "signed under a former name, but at or after its cutoff"
            } else {
                "signature fails under every name"
            }));
        }
    }
    out
}

/// The table and the list of orphans. `sections` are `(title, counts)`.
pub fn render(
    current: &str,
    former: &[FormerRepo],
    sections: &[(String, Vec<IssueCount>)],
) -> String {
    let mut head = vec![
        "issue".to_string(),
        "messages".into(),
        format!("current {current}"),
    ];
    for f in former {
        head.push(format!("former {} (until {})", f.name, f.until));
    }
    head.push("accepted".into());
    head.push("none".into());
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut orphans: Vec<&Orphan> = Vec::new();
    let mut totals = vec![0usize; head.len() - 1];
    let mut section_rows: Vec<(usize, &str)> = Vec::new();
    for (title, counts) in sections {
        section_rows.push((rows.len(), title));
        for c in counts {
            let mut r = vec![c.key.clone(), c.messages.to_string()];
            r.extend(c.under.iter().map(ToString::to_string));
            r.push(c.accepted.to_string());
            r.push(c.orphans.len().to_string());
            for (t, v) in totals.iter_mut().zip(r.iter().skip(1)) {
                *t += v.parse::<usize>().unwrap_or(0);
            }
            rows.push(r);
            orphans.extend(c.orphans.iter());
        }
    }
    let mut total = vec!["total".to_string()];
    total.extend(totals.iter().map(ToString::to_string));
    let widths: Vec<usize> = (0..head.len())
        .map(|i| {
            std::iter::once(&head)
                .chain(rows.iter())
                .chain(std::iter::once(&total))
                .map(|r| r[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |r: &[String]| {
        let cells: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    format!("{c:<w$}", w = widths[i])
                } else {
                    format!("{c:>w$}", w = widths[i])
                }
            })
            .collect();
        format!("{}\n", cells.join("  ").trim_end())
    };
    let mut out = format!("repo migration dry run (read only); current name {current}\n");
    if former.is_empty() {
        out.push_str("former_repos: none in AUTHORITY.md\n");
    }
    out.push('\n');
    out.push_str(&line(&head));
    for (i, r) in rows.iter().enumerate() {
        for (at, title) in &section_rows {
            if *at == i {
                writeln!(out, "-- {title}").ok(); // writing to a String cannot fail
            }
        }
        out.push_str(&line(r));
    }
    out.push_str(&line(&total));
    writeln!(
        out,
        "\nmessages that verify under no name: {}",
        orphans.len()
    )
    .ok(); // writing to a String cannot fail
    for o in orphans {
        writeln!(out, "  {} comment {}: {}", o.issue, o.comment_id, o.why).ok(); // writing to a String cannot fail
    }
    out
}

/// The numbers of the ticket issues of one repo: all open ones, and the newest closed ones.
fn ticket_issues(client: &Client, repo: &str) -> Result<(Vec<u64>, Vec<u64>), GhError> {
    let numbers = |paged: &Paged| -> Result<Vec<u64>, GhError> {
        Ok(paged
            .items("issue list")?
            .into_iter()
            .filter(|i| i.get("pull_request").is_none())
            .filter_map(|i| i.get("number").and_then(serde_json::Value::as_u64))
            .collect())
    };
    let open = paged(
        client,
        &format!("repos/{repo}/issues?labels={WORK_LABEL}&state=open&per_page=100"),
        MAX_PAGES,
    )?
    .complete(&format!("open ticket issues of {repo}"))?;
    // The closed list is a sample: one page, newest change first.
    let closed = paged(
        client,
        &format!(
            "repos/{repo}/issues?labels={WORK_LABEL}&state=closed&sort=updated&direction=desc&per_page={CLOSED_SAMPLE}"
        ),
        1,
    )?;
    let mut closed = numbers(&closed)?;
    closed.truncate(CLOSED_SAMPLE);
    Ok((numbers(&open)?, closed))
}

/// Read the board config, AUTHORITY.md and the issues, and print the table.
pub fn run(g: &Global) -> CbResult<String> {
    let cfg = resolve_board_config(g)?;
    let client = new_client()?;
    dry_run(&client, &cfg)
}

pub fn dry_run(client: &Client, cfg: &BoardConfig) -> CbResult<String> {
    let main = cfg.main_repo().to_string();
    let former = match fetch_authority_text(client, &main, &cfg.authority)? {
        Some(t) => crate::authority::parse(&t)?
            .map(|a| a.former_repos)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let authors = cfg.crew_authors();
    let (registry, _) =
        feed::issue_comments(client, &main, cfg.registry_issue, None, &cfg.owner_login)?;
    let keys = join_keys(&registry, &authors);
    let count = |repo: &str, n: u64, comments: &[Comment]| {
        // former names belong to the main repo only
        let f: &[FormerRepo] = if repo.eq_ignore_ascii_case(&main) {
            &former
        } else {
            &[]
        };
        let mut c = count_issue(repo, n, comments, &authors, &keys, f);
        c.under.resize(1 + former.len(), 0);
        c
    };
    let mut open_counts = Vec::new();
    let mut closed_counts = Vec::new();
    for repo in &cfg.repos {
        let (open, closed) = ticket_issues(client, repo)?;
        for (list, into) in [(open, &mut open_counts), (closed, &mut closed_counts)] {
            for n in list {
                let (comments, _) = feed::issue_comments(client, repo, n, None, &cfg.owner_login)?;
                into.push(count(repo, n, &comments));
            }
        }
    }
    let sections = vec![
        (
            "registry issue".to_string(),
            vec![count(&main, cfg.registry_issue, &registry)],
        ),
        ("open tickets".to_string(), open_counts),
        (
            format!("closed tickets (newest {CLOSED_SAMPLE} per repo)"),
            closed_counts,
        ),
    ];
    Ok(render(&main, &former, &sections))
}
