//! The unattended coordinator, the reads (spec/agents#unattended-coordinator): its mode and its
//! budget from AUTHORITY.md, the verdicts of today from the signed messages, and the judgment
//! that one ticket needs. The rules are pure and live in [`crate::unattended`].
//!
//! A command of the unattended coordinator passes two checks here. [`admit`] asks the mode and
//! the budget of the day, and puts the stamp into the context. [`check_ticket`] asks the signed
//! log of the ticket that the command just read. Neither takes the list of a tick as a right.
//! `say::send_signed` then writes the line `unattended: <n>/<budget>` and notes the counts in
//! the local state before the post ([`stamp_body`]).

use crate::authority::CoordMode;
use crate::base::TicketRef;
use crate::base::ids;
use crate::comments::Comment;
use crate::ctx::Stamp;
use crate::error::{CbError, CbResult};
use crate::headless::UAct;
use crate::items::ItemSnapshot;
use crate::ops::{AgentCtx, World};
use crate::protocol::acts::{Act, has_unattended_line, unattended_line};
use crate::protocol::names::Kind;
use crate::rules::Refusal;
use crate::services::ticket_loader::{Ticket, read_messages};
use crate::state::{NOTE_KEEP_SECS, StateCache, TickState, TicketFloor, UnattendedFeed};
use crate::ticket::verified_messages;
use crate::unattended::{
    Block, DayCount, Facts, MAX_MESSAGES_PER_DAY, MAX_VERDICTS_PER_TICKET, Need, check_act,
    day_start, floor_on, messages_used, need_of, ticket_verdicts_at_least, used, verdicts_on_day,
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
    load_day(ac, world, now, cache).map(|d| d.verdicts)
}

/// What the unattended coordinator used today (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Used {
    /// The verdicts ([`crate::unattended::used`]).
    pub(crate) verdicts: u32,
    /// The stamped messages, the verdicts among them ([`crate::unattended::messages_used`]).
    pub(crate) messages: u32,
}

/// [`load_used_today`] with the stamped messages of the day.
fn load_day(ac: &AgentCtx, world: &World, now: i64, cache: &mut StateCache) -> CbResult<Used> {
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
            messages: s.messages,
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
                        messages: count.messages,
                    },
                );
                count
            }
        };
        day = day.with(count);
    }
    let floor = floor_on(cache.tick.unattended_floor.as_ref(), today);
    let messages_floor = floor_on(cache.tick.unattended_messages_floor.as_ref(), today);
    Ok(Used {
        verdicts: used(day, floor),
        messages: messages_used(day, messages_floor),
    })
}

/// The floor of the unattended verdicts of the ticket `key` in the local state.
pub(crate) fn floor_of(tick: &TickState, key: &str) -> Option<(u32, u64)> {
    tick.unattended_tickets.get(key).map(|f| (f.count, f.after))
}

/// The facts of a loaded ticket: the issue, its labels and its messages are read.
fn loaded_facts(t: &Ticket, verdict_floor: Option<(u32, u64)>) -> Facts<'_> {
    Facts {
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
        verdict_floor,
    }
}

/// The judgment that a loaded ticket needs. `tick` is the local state (the floor of the
/// verdicts of the ticket).
pub(crate) fn loaded_need(t: &Ticket, tick: &TickState) -> Option<Need> {
    need_of(&loaded_facts(t, floor_of(tick, &t.key())))
}

fn budget_refusal(text: String) -> CbError {
    Refusal::new(
        ids::UNATTENDED_BUDGET,
        format!(
            "{text}. Leave this for the interactive coordinator and note it; nothing was written"
        ),
    )
    .into()
}

/// Let a command of the unattended coordinator in. Call it after the context and the world are
/// loaded and before any write.
///
/// `Ok(false)`: the config dir is not headless. The caller is an attended agent; nothing is
/// checked and nothing is stamped. `Ok(true)`: the mode of AUTHORITY.md allows the act and the
/// budget of the day has room; the stamp is in the context, so every signed message of this
/// command gets the line. The marker `headless = true` of the config dir decides, as for the
/// headless scope: no flag and no environment variable does.
///
/// Refusals: `unattended-off` for the mode `off` (also when AUTHORITY.md was not read), before
/// any read of the count. `unattended-budget` when the count of the day cannot be read (the
/// command must not go on as if it were 0), when the day has its
/// [`MAX_MESSAGES_PER_DAY`] stamped messages, and for a verdict when the day has its
/// `unattended_budget` verdicts.
pub(crate) fn admit(ac: &AgentCtx, world: &World, act: UAct) -> CbResult<bool> {
    if !ac.ctx.cd.is_headless()? {
        return Ok(false);
    }
    let s = settings(ac);
    if s.mode.is_off() {
        return Err(Refusal::new(
            ids::UNATTENDED_OFF,
            format!(
                "this config is headless and the unattended coordinator is off: {} is a judgment, and `unattended_coordinator` of {} on the default branch is `off`, absent or not readable. Leave this for the interactive coordinator and note it; nothing was written",
                act.words(),
                ac.ctx.cfg.authority
            ),
        )
        .into());
    }
    let now = crate::clock::now_epoch();
    let mut cache = ac.load_cache();
    let before = cache.tick.clone();
    let day = load_day(ac, world, now, &mut cache).map_err(|e| {
        budget_refusal(format!(
            "the unattended coordinator could not read its verdicts of today ({}), so it posts nothing",
            e.to_string().lines().next().unwrap_or("").trim()
        ))
    })?;
    // A lost save costs one read of the feed at the next command.
    if cache.tick != before {
        ac.save_cache(&mut cache);
    }
    if day.messages >= MAX_MESSAGES_PER_DAY {
        return Err(budget_refusal(format!(
            "the unattended coordinator posted {} messages today (UTC); its limit is {MAX_MESSAGES_PER_DAY} per day",
            day.messages
        )));
    }
    if act == UAct::Verdict && day.verdicts >= s.budget {
        return Err(budget_refusal(format!(
            "the unattended coordinator used {} of its {} verdicts of today (UTC; `unattended_budget` of {})",
            day.verdicts, s.budget, ac.ctx.cfg.authority
        )));
    }
    let now_text = crate::clock::format_ts(now);
    ac.stamp.set(Stamp {
        day: crate::ops::digest::day_of(&now_text).to_string(),
        used: day.verdicts,
        budget: s.budget,
        messages: day.messages,
        ticket: None,
    });
    Ok(true)
}

/// May the unattended coordinator run `act` on the ticket that the command just loaded? Call it
/// only after [`admit`] returned `true`, and before any write. `to`: the agent that an answer
/// names. The decision is [`check_act`] on the signed log; the refusal is `unattended-budget`
/// for a ticket that has its unattended verdicts and `headless-scope` for any other reason.
///
/// For a verdict it also puts the floor of the ticket into the stamp: `send_signed` notes it in
/// the local state before the post.
pub(crate) fn check_ticket(ac: &AgentCtx, act: UAct, t: &Ticket, to: Option<&str>) -> CbResult<()> {
    let key = t.key();
    let floor = floor_of(&ac.load_cache().tick, &key);
    match check_act(act, &key, &loaded_facts(t, floor), to) {
        Ok(()) => {}
        Err(Block::Cap) => {
            return Err(budget_refusal(format!(
                "{key} has its {MAX_VERDICTS_PER_TICKET} unattended verdicts: a person looks at it first. An owner comment on the ticket, or an assignment by the interactive coordinator, starts the count again"
            )));
        }
        Err(Block::Scope(why)) => {
            return Err(crate::headless::unattended_refusal(&why).into());
        }
    }
    if act == UAct::Verdict
        && let Some(mut stamp) = ac.stamp.get()
    {
        let count = ticket_verdicts_at_least(&t.msgs, floor);
        let after = t.comments.iter().map(|c| c.id).max().unwrap_or(0);
        stamp.ticket = Some((key, u32::try_from(count).unwrap_or(u32::MAX), after));
        ac.stamp.set(stamp);
    }
    Ok(())
}

/// A text that a person or a model passes in must not have a line with the key `unattended`:
/// only crewboard writes the stamp. In a headless config dir such a text is refused with
/// `headless-scope`; an attended agent is not restricted.
pub(crate) fn check_no_stamp_line(ac: &AgentCtx, text: &str) -> CbResult<()> {
    if has_unattended_line(text) && ac.ctx.cd.is_headless()? {
        return Err(crate::headless::unattended_refusal(
            "the text has a line that starts with `unattended:`; only crewboard writes that line",
        )
        .into());
    }
    Ok(())
}

/// The body that `send_signed` posts. Without a stamp in the context it is `body`. With one it
/// gets the last line `unattended: <n>/<budget>`: `n` is the number of verdicts of the day, with
/// this message when it is a verify result.
///
/// Before the post, the counts go into the local state as floors: the verdicts and the stamped
/// messages of the day, and for a verify result the verdicts of the ticket. A comment that
/// somebody deletes or edits later then does not lower a count on this host. A state that cannot
/// be saved refuses the post (the error names the path).
pub(crate) fn stamp_body(ac: &AgentCtx, t: &TicketRef, kind: Kind, body: &str) -> CbResult<String> {
    check_no_stamp_line(ac, body)?;
    let Some(mut stamp) = ac.stamp.get() else {
        return Ok(body.to_string());
    };
    let verdict = matches!(
        crate::protocol::acts::parse(Some(kind), body),
        Act::Verify { .. }
    );
    stamp.messages = stamp.messages.saturating_add(1);
    if verdict {
        stamp.used = stamp.used.saturating_add(1);
    }
    // The line is strict (`n` is never above the budget); [`admit`] refused a verdict that
    // would be above it.
    let n = stamp.used.min(stamp.budget);
    let now = crate::clock::now_epoch();
    let mut cache = ac.load_cache();
    let tick = &mut cache.tick;
    let day = stamp.day.clone();
    let raise = |slot: &mut Option<(String, u32)>, to: u32| {
        let kept = floor_on(slot.as_ref(), &day);
        *slot = Some((day.clone(), kept.max(to)));
    };
    raise(&mut tick.unattended_floor, stamp.used);
    raise(&mut tick.unattended_messages_floor, stamp.messages);
    if verdict
        && let Some((key, count, after)) = stamp.ticket.take()
        && key == t.key()
    {
        tick.unattended_tickets
            .retain(|_, f| now - f.at < NOTE_KEEP_SECS);
        tick.unattended_tickets.insert(
            key,
            TicketFloor {
                count: count.saturating_add(1),
                after,
                at: now,
            },
        );
    }
    ac.save_cache_required(&mut cache)?;
    let line = unattended_line(n, stamp.budget);
    ac.stamp.set(stamp);
    Ok(format!("{}\n{line}", body.trim_end()))
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
    verdict_floor: Option<(u32, u64)>,
) -> CbResult<BoardNeed> {
    let board_only = Facts {
        item,
        issue: t.issue,
        msgs: None,
        body: None,
        closed: false,
        needs_owner_label: false,
        verdict_floor,
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
