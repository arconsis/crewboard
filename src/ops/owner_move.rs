//! `crewboard move <issue> <status> [--why <text>]`: the owner moves a ticket by hand
//! (spec/tickets#owner-move).
//!
//! The board `Status` is not signed, so a drag of the owner looks like a forged edit: `status`
//! and `audit` report it. This command first posts the owner command `/crew move <status>` on
//! the ticket, which puts the move into the log ([`crate::ticket::log::fold_ticket`]), and then
//! writes the board. It runs with the owner's token only (`owner-command-not-owner`).

use super::{Ctx, Global, load_world, new_owner_client, parse_issue_arg, require_owner};
use crate::base::ids;
use crate::board::FieldValue;
use crate::board::ItemRef;
use crate::error::CbResult;
use crate::feed;
use crate::owner_cmd::{move_target, move_text};
use crate::rules::fields::Field;
use crate::rules::fields::WriteCtx;
use crate::rules::status::Status;
use crate::rules::{Actor, Refusal};
use crate::services::ticket_loader::load_ticket;
use crate::state::StateCache;
use crate::ticket::log::{Expected, fold_ticket};
use serde_json::json;

/// What the move writes after its comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Write the board `Status` (false: the board shows the target already, only the log lacks it).
    pub write_status: bool,
    /// Write the board `Agent` to the assignee of the signed log.
    pub set_agent: Option<String>,
}

/// Decide a move of the owner. Pure.
///
/// - `from`: the board `Status` (`None`: empty or not one of ours); `log`: what the signed log
///   shows; `log_agent`: the assignee of the signed log; `board_agent`: the board `Agent`.
/// - Backlog needs nothing and keeps the `Agent` as it is.
/// - Ready, In Progress, Review and Blocked need the assignee of a signed assignment: the ticket
///   is somebody's work. A board `Agent` that is not that assignee is set to it.
/// - A ticket that the board AND the log show in the target already is `status-no-change`. A
///   board that shows it and a log that does not (a drag before) gets the comment only.
pub fn plan(
    key: &str,
    from: Option<Status>,
    to: Status,
    log: &Expected,
    log_agent: Option<&str>,
    board_agent: Option<&str>,
) -> Result<Plan, Refusal> {
    if from == Some(to) && log.allows(to) == Some(true) {
        return Err(Refusal::new(
            ids::STATUS_NO_CHANGE,
            format!(
                "{key} is {} on the board and in the signed log already",
                to.name()
            ),
        ));
    }
    let set_agent = if to == Status::Backlog {
        None
    } else {
        let Some(a) = log_agent else {
            return Err(Refusal::new(
                ids::MOVE_NEEDS_ASSIGNEE,
                format!(
                    "{key} has no signed assignment, and {} needs an agent: move it to Backlog, and the coordinator assigns it (`crewboard assign`)",
                    to.name()
                ),
            ));
        };
        (board_agent != Some(a)).then(|| a.to_string())
    };
    Ok(Plan {
        write_status: from != Some(to),
        set_agent,
    })
}

/// Run the move: check, post the owner command on the ticket, then write the board.
pub fn run(g: &Global, issue: &str, status: &str, why: Option<&str>) -> CbResult<String> {
    let to = move_target(status)?;
    if let Some(w) = why {
        super::say::check_body_text(w)?;
    }
    let cfg = super::resolve_board_config(g)?;
    let client = new_owner_client()?;
    require_owner(&client, &cfg, "moves a ticket by hand")?;
    let ctx = Ctx::load(client, cfg, g.dir()?)?;
    let world = load_world(&ctx, &[], &mut StateCache::default())?;
    let (repo, n) = parse_issue_arg(issue, ctx.main_repo())?;
    let t = load_ticket(&ctx.reader(), &world.trust, &repo, n)?;
    let key = t.key();
    let raw = t.item.get(Field::Status);
    let from = raw.and_then(Status::parse);
    let log = fold_ticket(&t.msgs);
    let p = plan(&key, from, to, &log.status, log.agent.as_deref(), t.agent())?;
    let text = move_text(to, why);
    let (id, url) = feed::post_comment(&ctx.client, &repo, n, &text)?;
    let mut out = format!(
        "owner command posted: /crew move {} (comment {id}, {url})\n",
        to.name()
    );
    if !t.info.open {
        ctx.client.send(
            "PATCH",
            &format!("repos/{repo}/issues/{n}"),
            Some(&json!({"state": "open"})),
        )?;
        out.push_str(&format!("{key}: issue reopened\n"));
    }
    if let Some(a) = &p.set_agent {
        ctx.board.write(
            &ctx.client,
            ItemRef::work(&t.item_id),
            Field::Agent,
            FieldValue::Option(a),
            &WriteCtx::by(Actor::Owner),
        )?;
        out.push_str(&format!("{key}: Agent {a} (the signed assignment)\n"));
    }
    if p.write_status {
        // a failed write leaves the move in the log and the board behind it: `status` shows the
        // mismatch, and the same command again writes the field (only the comment repeats)
        ctx.board.write_status(&ctx.client, &t.item_id, to)?;
        out.push_str(&format!(
            "{key}: Status {} -> {}\n",
            raw.unwrap_or("empty"),
            to.name()
        ));
    } else {
        out.push_str(&format!(
            "{key}: Status {} on the board already; the log shows it now\n",
            to.name()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "o/r#7";

    fn is(s: Status) -> Expected {
        Expected::Is(s)
    }

    #[test]
    fn backlog_needs_no_agent_and_keeps_the_board_agent() {
        let p = plan(
            KEY,
            Some(Status::Done),
            Status::Backlog,
            &is(Status::Done),
            None,
            Some("a@x"),
        )
        .unwrap();
        assert_eq!(
            p,
            Plan {
                write_status: true,
                set_agent: None
            }
        );
    }

    #[test]
    fn work_statuses_need_the_signed_assignee_and_set_a_board_agent_that_differs() {
        for to in [
            Status::Ready,
            Status::InProgress,
            Status::Review,
            Status::Blocked,
        ] {
            let r = plan(KEY, Some(Status::Done), to, &is(Status::Done), None, None);
            assert_eq!(r.unwrap_err().rule, ids::MOVE_NEEDS_ASSIGNEE, "{to:?}");
            // the board Agent alone is no assignment
            let r = plan(
                KEY,
                Some(Status::Done),
                to,
                &is(Status::Done),
                None,
                Some("a@x"),
            );
            assert_eq!(r.unwrap_err().rule, ids::MOVE_NEEDS_ASSIGNEE, "{to:?}");
            let same = plan(
                KEY,
                Some(Status::Done),
                to,
                &is(Status::Done),
                Some("a@x"),
                Some("a@x"),
            )
            .unwrap();
            assert_eq!(same.set_agent, None, "{to:?}");
            for board in [None, Some("b@y")] {
                let p = plan(
                    KEY,
                    Some(Status::Done),
                    to,
                    &is(Status::Done),
                    Some("a@x"),
                    board,
                )
                .unwrap();
                assert_eq!(p.set_agent.as_deref(), Some("a@x"), "{to:?} {board:?}");
            }
        }
    }

    #[test]
    fn a_status_that_board_and_log_show_is_no_change_and_a_drag_is_recorded() {
        let r = plan(
            KEY,
            Some(Status::Backlog),
            Status::Backlog,
            &is(Status::Backlog),
            None,
            None,
        );
        assert_eq!(r.unwrap_err().rule, "status-no-change");
        // the owner dragged it before: the board shows it, the log does not; only the comment
        let p = plan(
            KEY,
            Some(Status::Backlog),
            Status::Backlog,
            &is(Status::InProgress),
            Some("a@x"),
            Some("a@x"),
        )
        .unwrap();
        assert_eq!(
            p,
            Plan {
                write_status: false,
                set_agent: None
            }
        );
        // an empty or unknown board Status is written
        let p = plan(KEY, None, Status::Backlog, &Expected::Unknown, None, None).unwrap();
        assert!(p.write_status);
    }
}
