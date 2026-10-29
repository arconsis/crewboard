//! The coordinator duty that keeps the label `needs-owner` true (spec/tickets#agent-labels).
//!
//! The commands remove the label when a question is answered, taken back or its ticket ends. A
//! change outside crewboard (the owner moves the ticket, GitHub closes the issue, a lost run)
//! leaves a stale label. Once per [`CHECK_EVERY_S`] this duty lists the issues that carry the
//! label (one REST list per repo) and removes it from each ticket whose issue is closed or that
//! has no open question, whatever its Status. A Done ticket with an open issue and an open
//! question keeps it: the safety net of the owner route reopens such a ticket and asks the
//! owner. The comments are read only for the few labelled tickets. Only the real poll or watch cycle of the interactive coordinator runs it.

use super::{AgentCtx, World};
use crate::late::CHECK_EVERY_S;
use crate::services::ticket_loader::read_messages;
use crate::state::StateCache;
use crate::ticket::{TicketRow, question_open};

/// The warning lines of one run (empty when all went well or the run was not due).
pub fn run(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    cache: &mut StateCache,
    now: i64,
) -> Vec<String> {
    let due = cache
        .owner
        .needs_owner_checked
        .is_none_or(|t| now - t >= CHECK_EVERY_S || t > now);
    if !due {
        return Vec::new();
    }
    cache.owner.needs_owner_checked = Some(now);
    let mut warnings = Vec::new();
    for repo in &ac.ctx.cfg.repos {
        if let Err(e) = sync_repo(ac, world, rows, repo, &mut warnings) {
            warnings.push(format!(
                "crewboard: warning: the label needs-owner of {repo} was not checked: {e}"
            ));
        }
    }
    warnings
}

fn sync_repo(
    ac: &AgentCtx,
    world: &World,
    rows: &[TicketRow],
    repo: &str,
    warnings: &mut Vec<String>,
) -> Result<(), crate::error::CbError> {
    let path = format!(
        "repos/{repo}/issues?labels={}&state=all&per_page=100",
        crate::labels::NEEDS_OWNER
    );
    let page = ac.ctx.client.get(&path)?;
    let list = page
        .body
        .as_array()
        .ok_or_else(|| crate::github::GhError::Shape("issue list is not an array".into()))?;
    for i in list.iter().filter(|i| i.get("pull_request").is_none()) {
        let Some(n) = i.get("number").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let closed = i.get("state").and_then(serde_json::Value::as_str) == Some("closed");
        let row = rows.iter().find(|r| r.repo == repo && r.issue == n);
        let stale = if closed {
            true
        } else {
            let (_, msgs) = read_messages(&ac.ctx.reader(), &world.trust, repo, n)?;
            question_open(&msgs, row.and_then(|r| r.agent.as_deref())).is_none()
        };
        if stale && let Err(e) = crate::labels::set_needs_owner(&ac.ctx.client, repo, n, false) {
            warnings.push(format!(
                "crewboard: warning: the label needs-owner of {repo}#{n} was not removed: {e}"
            ));
        }
    }
    Ok(())
}
