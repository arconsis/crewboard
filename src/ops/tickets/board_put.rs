//! Put an issue on the board: the part of `new`, `adopt` and `assign` that adds the item, signs
//! the info comment that records the item id and sets `Kind`.

use super::{Me, work_write_ctx};
use crate::clock::now_ts;
use crate::ctx::AgentCtx;
use crate::error::CbResult;
use crate::ops::say::send_signed;
use crate::protocol::names::{Kind, Re, To};
use crate::rules::fields::{Field, Item, check_field_write};
use crate::services::ticket_loader::IssueInfo;
use crate::ticket::{info_body, info_body_adopting};

/// An open PR that a new ticket adopts, after the checks of [`check_adoption`].
pub(super) struct Adoption {
    pub url: String,
    pub branch: String,
    /// The Done ticket that the branch names.
    pub parent: String,
    /// The PR body closes which ticket? Used for the hint after the creation.
    pub body: String,
    pub repo: String,
}

/// Put an issue on the board: item, signed info comment (the item id), `Kind`. Returns the
/// item id. The caller has checked writer, `crew:work` label, open state and Gate.
pub(super) fn put_on_board(
    ac: &AgentCtx,
    me: &Me,
    info: &IssueInfo,
    repo: &str,
    adoption: Option<&Adoption>,
    out: &mut String,
) -> CbResult<String> {
    // spec/board#authority: putting a ticket on the board is "creating" it; the owner may reserve
    // it.
    crate::ctx::load_authority(&ac.ctx)?.check_create_ticket()?;
    let id = ac.ctx.board.add_issue_item(&ac.ctx.client, &info.node_id)?;
    // spec/board#item-ids-are-the-source-of-truth: the signed info comment records the id before
    // anything else is written.
    let posted = send_signed(
        ac,
        repo,
        info.number,
        To::Owner,
        Kind::Info,
        Re::None,
        &match adoption {
            Some(a) => info_body_adopting(&id, ac.ctx.cfg.project_number, &a.url, &a.branch),
            None => info_body(&id, ac.ctx.cfg.project_number),
        },
        &now_ts(),
    )?;
    out.push_str(&format!(
        "ticket added to the board as {id}; info comment {} records the item id\n",
        posted.id
    ));
    check_field_write(Item::Work, Field::Kind, &work_write_ctx(me, false))?;
    ac.ctx
        .board
        .set_select(&ac.ctx.client, &id, "Kind", "Work")?;
    Ok(id)
}
