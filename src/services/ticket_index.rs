//! The one place that finds the tickets of a board (spec/board#item-ids-are-the-source-of-truth).
//!
//! A ticket is an issue with the label `crew:work`; its board item id is named by the signed info
//! comment of the coordinator. This module lists those issues ([`each_ticket_issue`]), learns the
//! item ids into the local index, and reads the items by id with the retry for ids that GitHub
//! cannot resolve any more ([`read_items_retrying`]). It never lists the board items to find
//! tickets: that listing lags by minutes.

use super::Reader;
use crate::config::BoardConfig;
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::github::GhError;
use crate::github::paging::{MAX_PAGES, paged};
use crate::items::ItemSnapshot;
use crate::registry::{Registry, Trust};
use crate::state::TicketIndex;
use crate::ticket::{TicketRow, WORK_LABEL, find_item_id, verified_messages};
use std::collections::{BTreeMap, BTreeSet};

/// How many times a read is repeated after GitHub named ids that it cannot resolve. The texts
/// of the two errors that name it ("... could not be read after N attempts") are a contract:
/// callers and tests match on them.
const ATTEMPTS: usize = 6;

/// One ticket as its issue shows it: the key `owner/repo#n`, the item id from the signed info
/// comment.
pub(crate) struct TicketLog {
    pub(crate) key: String,
    pub(crate) item_id: String,
}

/// Which issues [`each_ticket_issue`] lists.
#[derive(Clone, Copy)]
enum Listing {
    /// The open issues, every page.
    Open,
    /// The newest closed issues (one page of 30, newest change first).
    Closed,
    /// Open and closed issues, every page.
    All,
}

impl Listing {
    fn query(self) -> &'static str {
        match self {
            Listing::Open => "state=open&per_page=100",
            Listing::Closed => "state=closed&sort=updated&direction=desc&per_page=30",
            Listing::All => "state=all&per_page=100",
        }
    }

    fn all_pages(self) -> bool {
        !matches!(self, Listing::Closed)
    }
}

/// Call `f` for every issue of the listing, in every repo of the initiative, that is not a pull
/// request, is not skipped by `skip(key)` (decided before any comment is read) and has a signed
/// info comment that names its board item. An issue without one is left out.
fn each_ticket_issue(
    r: &Reader,
    trust: &Trust,
    listing: Listing,
    skip: impl Fn(&str) -> bool,
    mut f: impl FnMut(TicketLog),
) -> CbResult<()> {
    for repo in &r.cfg.repos {
        let path = format!(
            "repos/{repo}/issues?labels={WORK_LABEL}&{}",
            listing.query()
        );
        let pages = paged(
            r.client,
            &path,
            if listing.all_pages() { MAX_PAGES } else { 1 },
        )?;
        // The closed listing is a sample of one page; every other listing must be whole.
        let pages = if listing.all_pages() {
            pages.complete(&format!("issues of {repo}"))?
        } else {
            pages
        };
        for i in pages
            .items("issue list")?
            .into_iter()
            .filter(|i| i.get("pull_request").is_none())
        {
            let Some(n) = i.get("number").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            let key = format!("{repo}#{n}");
            if skip(&key) {
                continue;
            }
            let (comments, _) = feed::issue_comments(r.client, repo, n, None, &r.cfg.owner_login)?;
            let msgs = verified_messages(&comments, r.cfg, trust);
            if let Some(item_id) = find_item_id(&msgs) {
                f(TicketLog { key, item_id });
            }
        }
    }
    Ok(())
}

/// One-time full scan (an empty cache): the open `crew:work` issues of every repo, and for
/// each the signed info comment of the coordinator. The issue list can lag by seconds; later
/// tickets arrive through the comment feed.
pub(crate) fn discover_tickets(r: &Reader, trust: &Trust, index: &mut TicketIndex) -> CbResult<()> {
    each_ticket_issue(
        r,
        trust,
        Listing::Open,
        |_| false,
        |t| {
            index.tickets.insert(t.key, t.item_id);
        },
    )?;
    index.tickets_scanned = true;
    Ok(())
}

/// The newest closed tickets (one page of 30 per repo, newest change first) that the index does
/// not know. [`discover_tickets`] lists open issues only, but the owner's merge closes the
/// ticket issue at once, and the tick must still find the board item of that ticket. A ticket
/// that is indexed already costs no read.
pub(crate) fn discover_closed_tickets(
    r: &Reader,
    trust: &Trust,
    index: &mut TicketIndex,
) -> CbResult<()> {
    let known: BTreeSet<String> = index.tickets.keys().cloned().collect();
    each_ticket_issue(
        r,
        trust,
        Listing::Closed,
        |key| known.contains(key),
        |t| {
            index.tickets.insert(t.key, t.item_id);
        },
    )
}

/// Every issue with the `crew:work` label in every repo, closed ones too, with its signed info
/// comment. An issue without an info comment is left out.
pub(crate) fn all_ticket_logs(r: &Reader, trust: &Trust) -> CbResult<Vec<TicketLog>> {
    let mut logs = Vec::new();
    each_ticket_issue(r, trust, Listing::All, |_| false, |t| logs.push(t))?;
    Ok(logs)
}

/// Every item id of the board that crewboard can find, sorted and without duplicates. The item
/// listing of GitHub lags by minutes, so it only adds to the ids that are found without it: the
/// tickets of the local index and of the signed info comments of all `crew:work` issues, and the
/// agent items of the registry joins. `prune` needs every user of a value.
pub(crate) fn all_item_ids(
    r: &Reader,
    reg: &Registry,
    trust: &Trust,
    index: &TicketIndex,
) -> CbResult<Vec<String>> {
    // `all_ticket_logs` reads every `crew:work` issue, open or closed, so it covers what a scan of
    // the open issues finds; the local index adds tickets that it knew before
    let logs = all_ticket_logs(r, trust)?;
    let mut ids: Vec<String> = index.tickets.values().cloned().collect();
    ids.extend(logs.iter().map(|l| l.item_id.clone()));
    ids.extend(r.board.list_item_ids(r.client)?);
    ids.extend(
        reg.agents
            .values()
            .filter_map(|e| e.join.as_ref().map(|j| j.item_id.clone())),
    );
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// The ids that a "Could not resolve to a node" GraphQL error names.
pub(crate) fn unresolvable_ids(message: &str) -> Vec<String> {
    const MARK: &str = "global id of '";
    let mut out = Vec::new();
    let mut rest = message;
    while let Some(i) = rest.find(MARK) {
        rest = &rest[i + MARK.len()..];
        if let Some(j) = rest.find('\'') {
            out.push(rest[..j].to_string());
            rest = &rest[j..];
        }
    }
    out
}

/// The outcome of [`read_items_retrying`].
enum Read {
    /// No id was left to read.
    Empty,
    /// The items of the ids, in the same order (`None`: GitHub returned no item for the id).
    Items(Vec<Option<ItemSnapshot>>),
    /// Every attempt ended in a node error; `ids` are the ids that were left after the last one.
    Exhausted { ids: Vec<String> },
}

/// Read items by id. An id that GitHub cannot resolve (the item was deleted from the board) makes
/// `nodes(ids:)` answer with an error and no data: such ids are reported to `forget`, dropped,
/// and the read is repeated, so one deleted ticket never blinds the caller. Any other error is
/// returned as it is.
fn read_items_retrying(
    r: &Reader,
    ids: &[String],
    mut forget: impl FnMut(&[String]),
) -> CbResult<Read> {
    let mut ids: Vec<String> = ids.to_vec();
    for _ in 0..ATTEMPTS {
        if ids.is_empty() {
            return Ok(Read::Empty);
        }
        match r.board.read_items(r.client, &ids) {
            Ok(items) => return Ok(Read::Items(items)),
            Err(CbError::Network(GhError::GraphQl(m))) => {
                let gone = unresolvable_ids(&m);
                if gone.is_empty() {
                    return Err(CbError::Network(GhError::GraphQl(m)));
                }
                forget(&gone);
                ids.retain(|i| !gone.contains(i));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Read::Exhausted { ids })
}

/// Whether an item belongs to the project of the board (an item without a number does).
fn of_project(it: &ItemSnapshot, cfg: &BoardConfig) -> bool {
    it.project_number.is_none_or(|n| n == cfg.project_number)
}

/// Read the known tickets by id (spec/board#item-ids-are-the-source-of-truth: never from
/// `items()`). An id of another project is left out. An id that GitHub cannot resolve is
/// forgotten (removed from `tickets`) by [`read_items_retrying`].
pub(crate) fn read_ticket_rows(
    r: &Reader,
    tickets: &mut BTreeMap<String, String>,
) -> CbResult<Vec<TicketRow>> {
    // keys of a former repo name (after a transfer or rename) are no tickets of this board
    tickets.retain(|key, _| {
        key.rsplit_once('#')
            .is_some_and(|(repo, _)| r.cfg.repos.iter().any(|x| x.eq_ignore_ascii_case(repo)))
    });
    let ids: Vec<String> = tickets.values().cloned().collect();
    let read = read_items_retrying(r, &ids, |gone| tickets.retain(|_, id| !gone.contains(id)))?;
    let items = match read {
        Read::Empty => return Ok(Vec::new()),
        Read::Exhausted { ids } if !ids.is_empty() => {
            return Err(GhError::Shape(format!(
                "ticket items could not be read after {ATTEMPTS} attempts"
            ))
            .into());
        }
        Read::Exhausted { .. } => return Ok(Vec::new()),
        Read::Items(items) => items,
    };
    // the keys that were read: `tickets` after the forgetting, in the order of the ids
    Ok(tickets
        .keys()
        .zip(items)
        .filter_map(|(key, it)| TicketRow::from_item(&it.filter(|i| of_project(i, r.cfg))?, key))
        .collect())
}

/// Read items by id; an id that GitHub cannot resolve (the item was deleted) is left out and
/// the read repeats. Items of another project are left out.
pub(crate) fn read_items_tolerant(r: &Reader, ids: &[String]) -> CbResult<Vec<ItemSnapshot>> {
    match read_items_retrying(r, ids, |_| {})? {
        Read::Empty => Ok(Vec::new()),
        Read::Items(items) => Ok(items
            .into_iter()
            .flatten()
            .filter(|i| of_project(i, r.cfg))
            .collect()),
        Read::Exhausted { .. } => {
            Err(GhError::Shape(format!("items could not be read after {ATTEMPTS} attempts")).into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ids_of_a_node_error_are_read_from_its_message() {
        let m = "Could not resolve to a node with the global id of 'PVTI_a'.; Could not resolve to a node with the global id of 'PVTI_b-c_1'.";
        assert_eq!(
            unresolvable_ids(m),
            vec!["PVTI_a".to_string(), "PVTI_b-c_1".to_string()]
        );
        // other errors name no id (control: the filter can say none)
        assert!(unresolvable_ids("Something else failed").is_empty());
        assert!(unresolvable_ids("").is_empty());
    }
}
