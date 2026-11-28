//! The unattended coordinator, the reads (spec/agents#unattended-coordinator): its mode and its
//! budget from AUTHORITY.md, the verdicts of today from the signed messages, and the judgment
//! that one ticket needs. The rules are pure and live in [`crate::unattended`].

use crate::authority::CoordMode;
use crate::base::TicketRef;
use crate::comments::Comment;
use crate::error::CbResult;
use crate::items::ItemSnapshot;
use crate::ops::{AgentCtx, World};
use crate::services::ticket_loader::{Ticket, read_messages};
use crate::state::{StateCache, UnattendedFeed};
use crate::ticket::verified_messages;
use crate::unattended::{
    DayCount, Facts, Need, day_start, floor_on, need_of, used, verdicts_on_day,
};
use std::collections::BTreeMap;

/// What AUTHORITY.md says about the unattended coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Settings {
    pub(crate) mode: CoordMode,
    /// The verdicts per UTC day.
    pub(crate) budget: u32,
}

/// The settings from the text of AUTHORITY.md that the context read when it loaded. This makes
/// no request. Without that text (the read failed, no file, a front matter that does not parse)
/// the mode is `off`: the commands that enforce the file report its fault themselves.
pub(crate) fn settings(ac: &AgentCtx) -> Settings {
    let a = ac
        .ctx
        .authority_text_cached()
        .flatten()
        .and_then(|t| crate::authority::parse(&t).ok().flatten())
        .unwrap_or_default();
    Settings {
        mode: a.unattended_coordinator,
        budget: a.unattended_budget,
    }
}

/// The verdicts that the unattended coordinator used today (UTC). One read of the comment feed
/// per repo of the initiative, from the start of the day; the comments are grouped by issue and
/// only the verified messages count. The editors of edited owner comments are not read: no
/// comment of the owner counts here.
///
/// `cache` remembers each read ([`crate::state::TickState::unattended_feed`]): a feed that did
/// not change costs a conditional request, and the count of a repo never goes below what an
/// earlier read of the same day gave. The caller saves `cache`. The floor of the local state is
/// added ([`crate::unattended::used`]). A read that fails is an error: the caller must not go on
/// as if the count were 0.
pub(crate) fn load_used_today(
    ac: &AgentCtx,
    world: &World,
    now: i64,
    cache: &mut StateCache,
) -> CbResult<u32> {
    let start = day_start(now);
    let cfg = &ac.ctx.cfg;
    let now_text = crate::clock::format_ts(now);
    let today = crate::ops::digest::day_of(&now_text);
    let mut day = DayCount::default();
    for repo in &cfg.repos {
        let seen = cache
            .tick
            .unattended_feed
            .get(repo)
            .filter(|s| s.day == today)
            .cloned();
        let seen_count = seen.as_ref().map_or(DayCount::default(), |s| DayCount {
            verdicts: s.verdicts,
            highest: s.highest,
        });
        let etag = seen.as_ref().and_then(|s| s.etag.as_deref());
        let count = match crate::feed::feed_since_unresolved(&ac.ctx.client, repo, start, etag)? {
            None => seen_count,
            Some(feed) => {
                let mut by_issue: BTreeMap<u64, Vec<Comment>> = BTreeMap::new();
                for c in feed.comments {
                    by_issue.entry(c.issue).or_default().push(c);
                }
                let mut read = DayCount::default();
                for comments in by_issue.values() {
                    let msgs = verified_messages(comments, cfg, &world.trust);
                    read = read.with(verdicts_on_day(&msgs, start));
                }
                let count = read.at_least(seen_count);
                cache.tick.unattended_feed.insert(
                    repo.clone(),
                    UnattendedFeed {
                        day: today.to_string(),
                        etag: feed.etag,
                        verdicts: count.verdicts,
                        highest: count.highest,
                    },
                );
                count
            }
        };
        day = day.with(count);
    }
    let floor = floor_on(cache.tick.unattended_floor.as_ref(), today);
    Ok(used(day, floor))
}

/// The judgment that a loaded ticket needs: the issue, its labels and its messages are read.
pub(crate) fn loaded_need(t: &Ticket) -> Option<Need> {
    need_of(&Facts {
        item: &t.item,
        issue: t.issue,
        msgs: Some(&t.msgs),
        body: Some(&t.info.body),
        closed: !t.info.open,
        needs_owner_label: t
            .info
            .labels
            .iter()
            .any(|l| l == crate::labels::NEEDS_OWNER),
    })
}

/// What one read for the list gave.
pub(crate) struct BoardNeed {
    pub(crate) need: Option<Need>,
    /// The item is a Backlog candidate and the signed log says that no reassign returned the
    /// ticket. That holds until the ticket is assigned, so the caller need not read again while
    /// the board item is unchanged.
    pub(crate) not_returned: bool,
}

/// The judgment that the ticket `t` of the board item `item` needs: one read of its comments,
/// for every ticket whose issue is not closed. The board fields alone put no ticket on the
/// list. The issue is not read, so the label `needs-owner` is not known here: the signed log
/// says whether a question to the owner is open.
pub(crate) fn board_need(
    ac: &AgentCtx,
    world: &World,
    item: &ItemSnapshot,
    t: &TicketRef,
) -> CbResult<BoardNeed> {
    let board_only = Facts {
        item,
        issue: t.issue,
        msgs: None,
        body: None,
        closed: false,
        needs_owner_label: false,
    };
    if crate::unattended::never(&board_only).is_some() || !ac.ctx.cfg.repos.contains(&t.repo) {
        return Ok(BoardNeed {
            need: None,
            not_returned: false,
        });
    }
    let (_, msgs) = read_messages(&ac.ctx.reader(), &world.trust, &t.repo, t.issue)?;
    Ok(BoardNeed {
        need: need_of(&Facts {
            msgs: Some(&msgs),
            ..board_only
        }),
        not_returned: crate::unattended::is_backlog_candidate(item)
            && !crate::unattended::returned(&msgs),
    })
}
