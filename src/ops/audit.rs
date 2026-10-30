//! `audit` (owner, spec/cli#commands): reads every comment of the initiative's repos and lists what
//! the signed comments show. See [`crate::audit`] for what counts.

use super::{Ctx, Global, World, load_world, new_owner_client, resolve_board_config};
use crate::audit::{
    BoardAgent, BoardTicket, BoardView, FindingKind, Report, audit, reported_events,
};
use crate::clock::{format_ts, now_epoch, parse_ts};
use crate::error::{CbError, CbResult};
use crate::feed;
use crate::late::liveness::LATE_INTERVALS;
use crate::rules::fields::Field;
use crate::services::ticket_index::{discover_tickets, read_ticket_rows};
use crate::ticket::verified_messages;
use serde_json::json;
use std::collections::BTreeMap;

/// `--since` is an ISO time (`2026-10-16T08:00:00Z`) or a duration back from now (`90m`, `2h`).
pub fn parse_since(s: &str, now: i64) -> CbResult<i64> {
    if let Some(t) = parse_ts(s) {
        return Ok(t);
    }
    crate::clock::parse_duration(s)
        .map(|d| now - d.as_secs() as i64)
        .map_err(|_| {
            CbError::config(format!(
                "bad --since '{s}' (use 2026-10-16T08:00:00Z or 90m or 2h)"
            ))
        })
}

pub fn run(g: &Global, json_out: bool, since: Option<&str>) -> CbResult<String> {
    let cfg = resolve_board_config(g)?;
    let client = new_owner_client()?;
    super::require_owner(&client, &cfg, "may run the audit")?;
    let now = now_epoch();
    let since = since.map(|s| parse_since(s, now)).transpose()?;
    let ctx = Ctx::load(client, cfg, g.dir()?)?;
    let world = load_world(&ctx, &[], &mut crate::state::StateCache::default())?;
    let mut comments = Vec::new();
    for repo in &ctx.cfg.repos {
        comments
            .extend(feed::feed_since(&ctx.client, repo, 0, None, &ctx.cfg.owner_login)?.comments);
    }
    let mut heartbeats: BTreeMap<String, i64> = BTreeMap::new();
    for e in world.reg.agents.values() {
        let item = e.join.as_ref().and_then(|j| world.items.get(&j.item_id));
        let running = item
            .and_then(|i| i.get(Field::State))
            .is_some_and(crate::late::liveness::heartbeat_state);
        if let (true, Some(hb)) = (
            running,
            item.and_then(|i| i.get(Field::Heartbeat))
                .and_then(parse_ts),
        ) {
            heartbeats.insert(e.name.clone(), hb);
        }
    }
    let board = board_view(&ctx, &world)?;
    let reg_msgs = verified_messages(&world.registry_comments, &ctx.cfg, &world.trust);
    let reported = reported_events(&reg_msgs);
    let grace = LATE_INTERVALS * ctx.hb_interval;
    let mut report = audit(
        &ctx.cfg,
        &world.reg,
        &comments,
        &heartbeats,
        grace,
        &reported,
        &board,
    );
    if let Some(t) = since {
        report.findings.retain(|f| f.created_epoch >= t);
    }
    Ok(if json_out {
        json!({"initiative": ctx.cfg.initiative, "since": since.map(format_ts), "report": report})
            .to_string()
            + "\n"
    } else {
        render(&ctx.cfg.initiative, &report, since, grace)
    })
}

/// The ticket and agent fields as the board shows them now. The tickets are found as `prune`
/// finds them and read by id; the agent items are in the world already.
fn board_view(ctx: &Ctx, world: &World) -> CbResult<BoardView> {
    let mut cache = crate::state::StateCache::default();
    discover_tickets(&ctx.reader(), &world.trust, &mut cache.index)?;
    let tickets = read_ticket_rows(&ctx.reader(), &mut cache.index.tickets)?
        .into_iter()
        .map(|r| BoardTicket {
            key: format!("{}#{}", r.repo, r.issue),
            item_id: r.item_id,
            status: r.status.map(|s| s.as_str().to_string()),
            agent: r.agent,
        })
        .collect();
    let agents = world
        .reg
        .agents
        .values()
        .filter_map(|e| {
            let item = world.items.get(&e.join.as_ref()?.item_id)?;
            Some(BoardAgent {
                name: e.name.clone(),
                role: item.get(Field::Role).map(str::to_string),
                key: item.get(Field::Key).map(str::to_string),
                state: item.get(Field::State).map(str::to_string),
            })
        })
        .collect();
    Ok(BoardView { tickets, agents })
}

pub fn render(initiative: &str, r: &Report, since: Option<i64>, grace: i64) -> String {
    let mut s = format!(
        "audit of initiative {initiative}: {} comment(s) scanned{}\nSource: signed comments (GitHub keeps no history of field values, docs/design/spec/cli.md#commands), and the board fields for field-mismatch.\nSession windows (first to last signed message of an agent, plus {grace} s): {}\n",
        r.scanned,
        since.map_or(String::new(), |t| format!(
            ", findings since {}",
            format_ts(t)
        )),
        if r.windows.is_empty() {
            "none".to_string()
        } else {
            r.windows
                .iter()
                .map(|w| format!("{} {}..{}", w.agent, format_ts(w.from), format_ts(w.to)))
                .collect::<Vec<_>>()
                .join("; ")
        }
    );
    let sections = [
        (
            FindingKind::HeaderlessOwner,
            "headerless comments by the owner's account while an agent session ran (in one-account mode an agent with the owner's token may have posted them; confirm each)",
        ),
        (FindingKind::BadSignature, "bad signatures"),
        (FindingKind::Edited, "edited messages"),
        (FindingKind::UnknownSender, "messages of unknown senders"),
        (
            FindingKind::ForeignAuthor,
            "crew headers by a foreign author",
        ),
        (FindingKind::Malformed, "malformed crew headers"),
        (
            FindingKind::WrongWriter,
            "transitions by the wrong writer (derived from signed messages)",
        ),
        (
            FindingKind::FieldMismatch,
            "board fields that the log does not support (the log is the truth, the board is a view)",
        ),
    ];
    for (kind, title) in sections {
        let list: Vec<_> = r.findings.iter().filter(|f| f.kind == kind).collect();
        s.push_str(&format!("\n{title}: {}\n", list.len()));
        for f in list {
            if kind == FindingKind::FieldMismatch {
                s.push_str(&format!(
                    "  [{}] {}\n    {}\n",
                    kind.word(),
                    f.detail,
                    f.url
                ));
                continue;
            }
            s.push_str(&format!(
                "  [{}] {}#{} comment {} by {} at {}\n    {}\n    {}{}\n",
                kind.word(),
                f.repo,
                f.issue,
                f.comment_id,
                f.author,
                f.created,
                f.url,
                f.detail,
                match f.reported_to_owner {
                    Some(true) =>
                        "\n    reported to the owner: yes (a signed kind=info exists)".to_string(),
                    Some(false) =>
                        "\n    reported to the owner: NO signed kind=info exists yet".to_string(),
                    None => String::new(),
                }
            ));
        }
    }
    s.push_str(&format!(
        "\nheaderless owner-account comments outside every agent session: {} (not listed)\nforeign comments without a crew header: {}\nfindings: {}\n",
        r.owner_comments_outside_sessions,
        r.foreign_comments,
        r.findings.len()
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_is_a_time_or_a_duration_back_from_now() {
        let now = parse_ts("2026-10-16T12:00:00Z").unwrap();
        assert_eq!(
            parse_since("2026-10-16T08:00:00Z", now).unwrap(),
            now - 4 * 3600
        );
        assert_eq!(parse_since("90m", now).unwrap(), now - 5400);
        assert_eq!(parse_since("2h", now).unwrap(), now - 7200);
        for bad in ["", "yesterday", "0m", "2026-10-16"] {
            assert!(parse_since(bad, now).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_text_lists_each_finding_with_its_url_and_says_when_nothing_was_found() {
        let empty = Report {
            scanned: 3,
            windows: vec![],
            findings: vec![],
            owner_comments_outside_sessions: 1,
            foreign_comments: 0,
        };
        let t = render("t", &empty, None, 60);
        assert!(
            t.contains("findings: 0") && t.contains("bad signatures: 0"),
            "{t}"
        );
        assert!(t.contains("outside every agent session: 1"));
        let r = Report {
            findings: vec![crate::audit::Finding {
                kind: FindingKind::BadSignature,
                comment_id: 9,
                repo: "a/b".into(),
                issue: 2,
                url: "https://github.com/a/b/issues/2#issuecomment-9".into(),
                author: "o".into(),
                created: "2026-10-16T08:00:00Z".into(),
                created_epoch: 0,
                detail: "claims from=x".into(),
                reported_to_owner: Some(false),
            }],
            ..empty
        };
        let t = render("t", &r, None, 60);
        assert!(
            t.contains("bad signatures: 1") && t.contains("comment 9 by o"),
            "{t}"
        );
        assert!(
            t.contains("issuecomment-9") && t.contains("NO signed kind=info exists yet"),
            "{t}"
        );
        assert!(t.contains("findings: 1"));
    }
}
