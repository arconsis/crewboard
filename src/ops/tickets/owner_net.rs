//! The safety net of the owner route (spec/tickets#workflow-files-the-owner-route).
//!
//! The owner route delivers a workflow change as a patch file in the PR. If the owner merges
//! that PR before the patch is applied, the change is not live, yet GitHub (or the board) may
//! show the ticket as Done. This module finds such a ticket and puts it right.
//!
//! A ticket is a finding when all of these hold:
//! - its board Status is Done;
//! - the latest `done` of its assignee names an `owner-route: <patch>` that is the patch of THIS
//!   ticket ([`super::owner_applied::tied`]: the name, and the PR contains the file). A `done`
//!   that names another patch or a PR without it proves nothing: the ticket is not judged (it is
//!   read again next time, nothing is cached);
//! - the PR of that `done` is merged, and the workflow change is not applied: the PR changed no
//!   file under `.github/workflows`, and the patch is not on the base branch
//!   ([`super::owner_applied::patch_applied`], a content check that fails closed: a patch that
//!   cannot be read counts as not applied). A PR that changed a workflow file is applied, so the
//!   net decides that before the patch is read;
//! - no answer follows its `owner-route-merged` question (an owner message or a coordinator
//!   answer with `withdraws: question`): an answered finding is settled for good. ANY owner
//!   message after the question answers it, on purpose: the owner decides what to do next.
//!
//! [`scan`] only reads. [`reopen`] writes, and only the interactive coordinator runs it (the
//! poll or watch duty): reopen the issue, ONE signed question to the owner (once per PR, found
//! again from the signed messages), the label `needs-owner`. The Status stays Done, because Done
//! is final: the owner moves the ticket. The headless `tick` only reads and logs one line per
//! finding: it may not write (spec/agents#headless-coordinator).
//!
//! Reads are bounded. A Done ticket is judged once per change of its board item
//! (`StateCache::owner_route_judged`; a reopened ticket is marked judged too, because its Status
//! stays Done). The headless `tick` marks a finding it logged the same way, with the prefix
//! `tick:` in the value, so it logs the finding once per change of the item and the interactive
//! coordinator still judges it. Requests of one judgement: the comments of the issue (1); for
//! an owner-route `done` of this ticket also the PR (1) and its files (1 per 100); and, only
//! for a merged PR without a workflow file, the patch (1) and one file of the base branch per
//! workflow file of the patch (at most 5). So at most 9 plus extra pages of the lists, and 3
//! when the PR changed a workflow file or is not merged. At most [`MAX_JUDGED_PER_RUN`] tickets
//! are judged per run, so the first run on a long board spreads over several polls. A Done
//! ticket whose owner-route PR is not merged yet, or whose `done` is not tied to it, is not
//! cached: it costs the comments (and the PR, when tied) every run.

use super::owner_applied::{Proof, names_this_ticket, patch_applied, tied};
use super::pr_evidence::{fetch_pr, fetch_pr_files};
use super::{needs_owner_lines, post};
use crate::base::TicketRef;
use crate::error::CbResult;
use crate::ops::{AgentCtx, World};
use crate::protocol::names::{Kind, Re, To};
use crate::rules::evidence::parse_pr_url;
use crate::rules::status::Status;
use crate::services::ticket_loader::{load_ticket, read_messages};
use crate::state::StateCache;
use crate::ticket::TicketRow;
use crate::ticket::latest_done;
use serde_json::json;

/// New judgements per run (a poll or a tick).
pub const MAX_JUDGED_PER_RUN: usize = 10;

/// A Done ticket whose owner-route PR was merged without the workflow change.
#[derive(Debug, Clone)]
pub struct Finding {
    pub repo: String,
    pub issue: u64,
    pub pr_url: String,
    pub patch: String,
    pub base: String,
    /// The board item and its change stamp when the ticket was judged: a reopen marks that
    /// stamp as judged, because the Status stays Done and the finding would repeat.
    pub item_id: String,
    pub stamp: String,
}

impl Finding {
    pub fn key(&self) -> String {
        format!("{}#{}", self.repo, self.issue)
    }
}

/// What a reopen did to one ticket.
#[derive(Debug, Clone)]
pub struct Reopened {
    pub item_id: String,
    pub agent: Option<String>,
    pub text: String,
}

fn stamp(row: &TicketRow) -> String {
    row.updated.map_or("-".to_string(), |u| u.to_string())
}

/// The value that the headless `tick` stores for a finding it logged.
fn tick_stamp(row: &TicketRow) -> String {
    format!("tick:{}", stamp(row))
}

/// The result of one scan: the findings, and the failure that stopped it, if any. The findings
/// that were found before a failure are kept.
pub struct Scanned {
    pub findings: Vec<Finding>,
    pub failure: Option<String>,
}

/// Judge the Done tickets that are not judged yet. Reads only. `ticking`: the headless `tick`,
/// which skips a ticket that it logged already for this change of the item.
pub fn scan(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    cache: &mut StateCache,
    ticking: bool,
) -> Scanned {
    let mut todo: Vec<&TicketRow> = rows
        .iter()
        .filter(|r| r.in_status(Status::Done))
        .filter(|r| {
            let seen = cache.owner.owner_route_judged.get(&r.item_id);
            seen != Some(&stamp(r)) && !(ticking && seen == Some(&tick_stamp(r)))
        })
        .collect();
    // The newest change first: a ticket that has just become Done is judged in the next poll.
    todo.sort_by_key(|r| std::cmp::Reverse(r.updated));
    todo.truncate(MAX_JUDGED_PER_RUN);
    let mut out = Scanned {
        findings: Vec::new(),
        failure: None,
    };
    for row in todo {
        match judge(ac, world, row) {
            Err(e) => {
                out.failure = Some(format!("{}#{}: {e}", row.repo, row.issue));
                break;
            }
            Ok(None) => {}
            Ok(Some(Some(mut f))) => {
                if ticking {
                    f.stamp = tick_stamp(row);
                }
                out.findings.push(f);
            }
            Ok(Some(None)) => {
                cache
                    .owner
                    .owner_route_judged
                    .insert(row.item_id.clone(), stamp(row));
            }
        }
    }
    out
}

/// `None`: not final, read again next time. `Some(None)`: judged, no finding. `Some(Some(f))`:
/// a finding.
fn judge(ac: &AgentCtx, world: &World, row: &TicketRow) -> CbResult<Option<Option<Finding>>> {
    let Some(agent) = row.agent.as_deref() else {
        return Ok(Some(None));
    };
    let (_, msgs) = read_messages(&ac.ctx.reader(), &world.trust, &row.repo, row.issue)?;
    let Some((done, ev)) = latest_done(&msgs, agent) else {
        return Ok(Some(None));
    };
    let Some(patch) = crate::ticket::owner_route_patch(&done.body) else {
        return Ok(Some(None));
    };
    // The proof must name this ticket's patch (no read yet). Else the ticket is not judged.
    if names_this_ticket(row.issue, &patch).is_err() {
        return Ok(None);
    }
    // An answer after the question of the net settles the finding for good (no read of the PR).
    if crate::owner_route::finding_settled(&msgs, &ev.pr) {
        return Ok(Some(None));
    }
    let Ok(pr_url) = parse_pr_url(&ev.pr) else {
        return Ok(Some(None));
    };
    if !ac
        .ctx
        .cfg
        .repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&pr_url.repo))
    {
        return Ok(Some(None));
    }
    let (pr, base) = fetch_pr(ac, &pr_url.repo, pr_url.number)?;
    if !pr.merged {
        return Ok(None);
    }
    let files = fetch_pr_files(ac, &pr.repo, pr.number)?;
    // The PR must contain the patch of this ticket; else it is no delivery of it (not judged).
    if tied(row.issue, &patch, &files).is_err() {
        return Ok(None);
    }
    // A PR that changed a workflow file is applied: no more reads. (`patch_applied` knows this
    // case too, for `close`.)
    if !crate::owner_route::merged_without_workflow(pr.merged, &files) {
        return Ok(Some(None));
    }
    // The owner may have applied the patch on the base branch and merged the patch-only PR.
    let proof = Proof {
        repo: &pr.repo,
        base: &base,
        head_sha: &pr.head_sha,
        patch: &patch,
        pr_files: &files,
    };
    if patch_applied(ac, &proof)?.is_ok() {
        return Ok(Some(None));
    }
    Ok(Some(Some(Finding {
        repo: row.repo.clone(),
        issue: row.issue,
        pr_url: ev.pr.clone(),
        patch,
        base,
        item_id: row.item_id.clone(),
        stamp: stamp(row),
    })))
}

/// The interactive coordinator puts one finding right. Every step can repeat: the question is
/// found again from the signed messages, and the reopen and the label are idempotent. The
/// board Status is not written (Done is final), so the ticket stays Done until the owner moves
/// it, and a failed run is repeated by the next poll.
pub fn reopen(ac: &AgentCtx, world: &World, f: &Finding) -> CbResult<Reopened> {
    let t = load_ticket(
        &ac.ctx.reader(),
        &world.trust,
        &TicketRef::new(&f.repo, f.issue),
    )?;
    let marker = crate::owner_route::merged_marker(&f.pr_url);
    let question = if crate::owner_route::question_sent(&t.msgs, &marker) {
        "question sent already".to_string()
    } else {
        let q = crate::owner_route::merged_message(&t.repo, t.issue, &f.base, &f.pr_url, &f.patch);
        let sent = post(ac, &t, To::Owner, Kind::Question, Re::None, &q)?;
        format!("question {}", sent.url)
    };
    // Every write is skipped when it is in place: a write changes the board item, and the item
    // is judged again when it changes.
    if !t.info.open {
        ac.ctx.client.send(
            "PATCH",
            &format!("repos/{}/issues/{}", t.repo, t.issue),
            Some(&json!({"state": "open"})),
        )?;
    }
    let label = if t
        .info
        .labels
        .iter()
        .any(|l| l == crate::labels::NEEDS_OWNER)
    {
        "label 'needs-owner' is set\n".to_string()
    } else {
        needs_owner_lines(ac, &t.repo, t.issue, true)
    };
    Ok(Reopened {
        item_id: t.item_id.clone(),
        agent: t.agent().map(str::to_string),
        text: format!(
            "{}: the owner-route PR {} was merged without the workflow files ({}); issue reopened, {question}; {}",
            t.key(),
            f.pr_url,
            f.patch,
            label.trim_end().replace('\n', "; ")
        ),
    })
}

/// What one run of the interactive duty did.
pub struct Run {
    pub reopened: Vec<Reopened>,
    /// The failure that stopped the run, with the ticket; the work done before it stays.
    pub failure: Option<String>,
}

/// The duty of the poll or watch of the interactive coordinator: scan, then reopen. A failure
/// of a later judgement or reopen keeps what was done before it.
pub fn run(ac: &AgentCtx, world: &World, rows: &[TicketRow], cache: &mut StateCache) -> Run {
    let scanned = scan(ac, world, rows, cache, false);
    let mut out = Run {
        reopened: Vec::new(),
        failure: scanned.failure,
    };
    for f in scanned.findings {
        match reopen(ac, world, &f) {
            Ok(r) => {
                out.reopened.push(r);
                // the ticket stays Done: judged until its board item changes
                cache
                    .owner
                    .owner_route_judged
                    .insert(f.item_id.clone(), f.stamp.clone());
            }
            Err(e) => {
                out.failure = Some(format!("{}: {e}", f.key()));
                break;
            }
        }
    }
    out
}

/// The lines of the headless `tick`: one per finding, nothing written on the board. A finding is
/// logged once per change of its board item. A failure of a judgement is one more line; the
/// findings before it stay.
pub fn tick_lines(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    cache: &mut StateCache,
) -> String {
    let scanned = scan(ac, world, rows, cache, true);
    let mut out = String::new();
    for f in &scanned.findings {
        out.push_str(&format!(
            "left for the interactive coordinator: {}, the owner-route PR {} was merged without the workflow files; the interactive coordinator reopens the ticket and asks the owner\n",
            f.key(),
            f.pr_url
        ));
        cache
            .owner
            .owner_route_judged
            .insert(f.item_id.clone(), f.stamp.clone());
    }
    if let Some(e) = scanned.failure {
        out.push_str(&format!(
            "the owner-route check of the Done tickets stopped: {e}\n"
        ));
    }
    out
}
