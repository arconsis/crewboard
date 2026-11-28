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
use crate::ctx::{Stamp, StampClass, StampTicket};
use crate::error::{CbError, CbResult};
use crate::headless::UAct;
use crate::items::ItemSnapshot;
use crate::ops::{AgentCtx, World};
use crate::protocol::acts::{Act, has_unattended_line, unattended_line};
use crate::protocol::names::{Kind, To};
use crate::rules::Refusal;
use crate::services::ticket_loader::{Ticket, read_messages};
use crate::state::{NOTE_KEEP_SECS, StateCache, TickState, TicketFloor, UnattendedFeed};
use crate::ticket::verified_messages;
use crate::unattended::{
    Block, DayCount, Facts, MAX_ASKS_PER_DAY, MAX_MESSAGES_PER_DAY, MAX_VERDICTS_PER_TICKET, Need,
    check_act, day_is_ahead, day_start, floor_on, floor_with_release, last_release, messages_used,
    need_of, ticket_verdicts, used, verdicts_on_day,
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
    /// The stamped questions to the owner.
    pub(crate) asks: u32,
}

/// [`load_used_today`] with the stamped messages of the day.
fn load_day(ac: &AgentCtx, world: &World, now: i64, cache: &mut StateCache) -> CbResult<Used> {
    let start = day_start(now);
    let cfg = &ac.ctx.cfg;
    let now_text = crate::clock::format_ts(now);
    let today = crate::ops::digest::day_of(&now_text);
    check_clock(&cache.tick, today)?;
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
            asks: s.asks,
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
                        asks: count.asks,
                    },
                );
                count
            }
        };
        day = day.with(count);
    }
    let floor = floor_on(cache.tick.unattended_floor.as_ref(), today);
    let messages_floor = floor_on(cache.tick.unattended_messages_floor.as_ref(), today);
    let asks_floor = floor_on(cache.tick.unattended_asks_floor.as_ref(), today);
    Ok(Used {
        verdicts: used(day, floor),
        messages: messages_used(day, messages_floor),
        asks: day.asks.max(asks_floor),
    })
}

/// What the stamped messages of the UTC day `[start, end)` say, for the daily digest: one read
/// of the comment feed per repo of the initiative, from `start`. The read has no memo: the
/// digest asks once per day, for a day that is over. Only verified messages of a coordinator
/// count.
pub(crate) fn load_day_report(
    ac: &AgentCtx,
    world: &World,
    start: i64,
    end: i64,
) -> CbResult<crate::unattended::DayReport> {
    let cfg = &ac.ctx.cfg;
    let mut report = crate::unattended::DayReport::default();
    for repo in &cfg.repos {
        let Some(feed) = crate::feed::feed_since_unresolved(&ac.ctx.client, repo, start, None)?
        else {
            continue;
        };
        let mut by_issue: BTreeMap<u64, Vec<Comment>> = BTreeMap::new();
        for c in feed.comments {
            by_issue.entry(c.issue).or_default().push(c);
        }
        for (issue, comments) in &by_issue {
            let msgs = verified_messages(comments, cfg, &world.trust);
            report.add(&TicketRef::new(repo, *issue).to_string(), &msgs, start, end);
        }
    }
    Ok(report)
}

/// The key (`event=` line) of the note to the owner that the verdicts of the UTC day `day` are
/// used. It has no `#`, so no reader takes a part of it for a repo.
pub(crate) fn budget_spent_key(day: &str) -> String {
    format!("unattended-budget:{day}")
}

/// The text of that note. No line of it starts with the key of the protocol line
/// (`unattended:`): the note is a fact that the tick posts, no message of the unattended
/// coordinator, and it counts for no budget.
pub(crate) fn budget_spent_body(day: &str, used: u32, budget: u32, authority: &str) -> String {
    format!(
        "The unattended coordinator used its verdicts of {day} (UTC): {used} of {budget}.\nevent={}\nIt gives no verify result before 00:00 UTC. The tickets that still need a judgment are in the output of `crewboard tick`; the interactive coordinator can take them now. For more verdicts per day, raise `unattended_budget` in {authority} on the default branch.\n",
        budget_spent_key(day)
    )
}

/// A count of the state that is of a later day than `today` refuses the command: the clock of
/// this host went back, or it was ahead when the count was noted. With such a clock a count of
/// "today" would start at 0 again, so nothing is counted and nothing is posted until the day of
/// the state is reached or the owner removes the state.
fn check_clock(tick: &TickState, today: &str) -> CbResult<()> {
    let days = [
        &tick.unattended_floor,
        &tick.unattended_messages_floor,
        &tick.unattended_asks_floor,
    ]
    .into_iter()
    .filter_map(|f| f.as_ref().map(|(d, _)| d.as_str()))
    .chain(tick.unattended_feed.values().map(|f| f.day.as_str()));
    match days.filter(|d| day_is_ahead(Some(d), today)).max() {
        None => Ok(()),
        Some(ahead) => Err(budget_refusal(format!(
            "the state of this config dir has a count of the unattended coordinator for {ahead}, and the clock of this host says {today} (UTC): the clock went back, or it was ahead. No count of today can be trusted"
        ))),
    }
}

/// The key of a ticket in the floors of the state ([`TickState::unattended_tickets`]): the one
/// function for the write, the compare and every read. It is [`TicketRef::key`] (the repo in
/// lower case), so the case of a repo name in a config changes nothing.
pub(crate) fn floor_key(t: &TicketRef) -> String {
    t.key()
}

/// The floor of the unattended verdicts of the ticket `t` in the local state.
pub(crate) fn floor_of(tick: &TickState, t: &TicketRef) -> Option<(u32, u64)> {
    tick.unattended_tickets
        .get(&floor_key(t))
        .map(|f| (f.count, f.after))
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
    need_of(&loaded_facts(t, floor_of(tick, &t.tref())))
}

fn asks_refusal(asks: u32) -> CbError {
    budget_refusal(format!(
        "the unattended coordinator asked the owner {asks} questions today (UTC); its limit is {MAX_ASKS_PER_DAY} per day"
    ))
}

fn messages_refusal(messages: u32) -> CbError {
    budget_refusal(format!(
        "the unattended coordinator posted {messages} messages today (UTC); its limit is {MAX_MESSAGES_PER_DAY} per day"
    ))
}

fn verdicts_refusal(ac: &AgentCtx, verdicts: u32, budget: u32) -> CbError {
    budget_refusal(format!(
        "the unattended coordinator used {verdicts} of its {budget} verdicts of today (UTC; `unattended_budget` of {})",
        ac.ctx.cfg.authority
    ))
}

fn ticket_cap_refusal(key: &str) -> CbError {
    budget_refusal(format!(
        "{key} has its {MAX_VERDICTS_PER_TICKET} unattended verdicts: a person looks at it first"
    ))
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
    let day = load_day(ac, world, now, &mut cache).map_err(|e| match e {
        // the refusal of the clock check names its cause itself
        CbError::Refused(r) if r.rule == ids::UNATTENDED_BUDGET => CbError::Refused(r),
        e => budget_refusal(format!(
            "the unattended coordinator could not read its verdicts of today ({}), so it posts nothing",
            e.to_string().lines().next().unwrap_or("").trim()
        )),
    })?;
    // A lost save costs one read of the feed at the next command.
    if cache.tick != before {
        ac.save_cache(&mut cache);
    }
    if day.messages >= MAX_MESSAGES_PER_DAY {
        return Err(messages_refusal(day.messages));
    }
    if act == UAct::Ask && day.asks >= MAX_ASKS_PER_DAY {
        return Err(asks_refusal(day.asks));
    }
    if act == UAct::Verdict && day.verdicts >= s.budget {
        return Err(verdicts_refusal(ac, day.verdicts, s.budget));
    }
    let now_text = crate::clock::format_ts(now);
    ac.stamp.set(Stamp {
        day: crate::ops::digest::day_of(&now_text).to_string(),
        used: day.verdicts,
        budget: s.budget,
        messages: day.messages,
        asks: day.asks,
        pending: Vec::new(),
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
    let floor = floor_of(&ac.load_cache().tick, &t.tref());
    match check_act(act, &key, &loaded_facts(t, floor), to) {
        Ok(()) => {}
        Err(Block::Cap) => {
            return Err(ticket_cap_refusal(&key));
        }
        Err(Block::Scope(why)) => {
            return Err(crate::headless::unattended_refusal(&why).into());
        }
    }
    if act == UAct::Verdict
        && let Some(mut stamp) = ac.stamp.get()
    {
        stamp.ticket = Some(StampTicket {
            key: floor_key(&t.tref()),
            count: u32::try_from(ticket_verdicts(&t.msgs)).unwrap_or(u32::MAX),
            after: t.comments.iter().map(|c| c.id).max().unwrap_or(0),
            release: last_release(&t.msgs),
        });
        ac.stamp.set(stamp);
    }
    Ok(())
}

/// A text that a person or a model passes in must not have a line with the key `unattended`:
/// only crewboard writes the stamp. In a headless config dir such a text is refused with
/// `headless-scope`; an attended agent is not restricted.
pub(crate) fn check_no_stamp_line(ac: &AgentCtx, text: &str) -> CbResult<()> {
    let Some(key) = written_by_crewboard_only(text) else {
        return Ok(());
    };
    if ac.ctx.cd.is_headless()? {
        return Err(crate::headless::unattended_refusal(&format!(
            "the text has a line that starts with `{key}:`; only crewboard writes that line"
        ))
        .into());
    }
    Err(Refusal::new(
        ids::TEXT_READS_AS_ACT,
        format!(
            "the text has a line that starts with `{key}:`; only crewboard writes that line (it marks a message of the unattended coordinator, or a merge after its verdict). Write the text without that line; nothing was written"
        ),
    )
    .into())
}

/// The key of a line of `text` that only crewboard writes: `unattended` (the stamp) or
/// `verified-by` (the line of a merge notice). Blank space before the key does not hide it.
fn written_by_crewboard_only(text: &str) -> Option<&'static str> {
    use crate::protocol::acts::{UNATTENDED_KEY, VERIFIED_BY_KEY};
    if has_unattended_line(text) {
        return Some(UNATTENDED_KEY);
    }
    text.lines()
        .any(|l| {
            l.trim_start()
                .split_once(':')
                .is_some_and(|(k, _)| k == VERIFIED_BY_KEY)
        })
        .then_some(VERIFIED_BY_KEY)
}

/// Take the messages `classes` of the budget, under the lock of the state file: read the counts
/// of the state again, refuse at a limit, write the counts with these messages, and only then
/// let the caller post. [`admit`] decided from a read that another command can be past by now;
/// this step is the one that two commands cannot both pass. The state file has a lock that
/// every writer of the file takes (`StateCache::update_locked`), so the read and the write are
/// one step between the processes of this config dir.
///
/// All messages of `classes` are taken together or none is: a command that will post a verify
/// result and a question to the owner (a stop at the round limit) reserves both before its
/// first write. The limits: the verdicts, the stamped messages and the questions to the owner
/// of the day, and for a verdict the unattended verdicts of the ticket.
///
/// The count is in the state before the post. So a comment that somebody deletes or edits
/// later does not lower it on this host, and a post that fails after this step keeps the count
/// raised (the command fails closed). A lock or a write that fails is an error, and nothing is
/// posted. A count of a later day in the state refuses the command ([`check_clock`]).
fn reserve(
    ac: &AgentCtx,
    stamp: &mut Stamp,
    t: &TicketRef,
    classes: &[StampClass],
) -> CbResult<()> {
    let shown = t.to_string();
    let count_of =
        |c: StampClass| u32::try_from(classes.iter().filter(|x| **x == c).count()).unwrap_or(0);
    let (new_verdicts, new_asks) = (count_of(StampClass::Verdict), count_of(StampClass::Ask));
    let new_messages = u32::try_from(classes.len()).unwrap_or(u32::MAX);
    let now = crate::clock::now_epoch();
    StateCache::update_locked(&ac.state_path(), |cache| {
        let tick = &mut cache.tick;
        check_clock(tick, &stamp.day)?;
        let day = stamp.day.clone();
        let at_least =
            |seen: u32, slot: &Option<(String, u32)>| seen.max(floor_on(slot.as_ref(), &day));
        let verdicts = at_least(stamp.used, &tick.unattended_floor);
        let messages = at_least(stamp.messages, &tick.unattended_messages_floor);
        let asks = at_least(stamp.asks, &tick.unattended_asks_floor);
        // The verdicts of the ticket: what the log had when the command read it, and the count
        // of the state as it is now (another command can have noted one since), unless the log
        // shows a release after that count was noted.
        let on_ticket = stamp
            .ticket
            .as_ref()
            .filter(|_| new_verdicts > 0)
            .map(|st| {
                let stored = tick
                    .unattended_tickets
                    .get(&st.key)
                    .map(|f| (f.count, f.after));
                (
                    floor_with_release(st.count, st.release, stored),
                    st.after.max(stored.map_or(0, |(_, after)| after)),
                )
            });
        if messages.saturating_add(new_messages) > MAX_MESSAGES_PER_DAY {
            return Err(messages_refusal(messages));
        }
        if new_verdicts > 0 && verdicts.saturating_add(new_verdicts) > stamp.budget {
            return Err(verdicts_refusal(ac, verdicts, stamp.budget));
        }
        if new_asks > 0 && asks.saturating_add(new_asks) > MAX_ASKS_PER_DAY {
            return Err(asks_refusal(asks));
        }
        if on_ticket.is_some_and(|(count, _)| count as usize >= MAX_VERDICTS_PER_TICKET) {
            return Err(ticket_cap_refusal(&shown));
        }
        stamp.used = verdicts.saturating_add(new_verdicts);
        stamp.messages = messages.saturating_add(new_messages);
        stamp.asks = asks.saturating_add(new_asks);
        tick.unattended_floor = Some((day.clone(), stamp.used));
        tick.unattended_messages_floor = Some((day.clone(), stamp.messages));
        if stamp.asks > 0 {
            tick.unattended_asks_floor = Some((day.clone(), stamp.asks));
        }
        if let (Some((count, after)), Some(st)) = (on_ticket, &stamp.ticket) {
            tick.unattended_tickets
                .retain(|_, f| now - f.at < NOTE_KEEP_SECS);
            tick.unattended_tickets.insert(
                st.key.clone(),
                TicketFloor {
                    count: count.saturating_add(new_verdicts),
                    after,
                    at: now,
                },
            );
        }
        Ok(())
    })?;
    if new_verdicts > 0 {
        stamp.ticket = None;
    }
    stamp.pending.extend_from_slice(classes);
    Ok(())
}

/// Reserve the messages of a command before it writes anything else, so that a refusal leaves
/// the board and the ticket as they were. Each message of the command then takes its
/// reservation. Nothing happens for a command that is no judgment of the unattended coordinator.
fn reserve_ahead(ac: &AgentCtx, t: &TicketRef, classes: &[StampClass]) -> CbResult<()> {
    let Some(mut stamp) = ac.stamp.get() else {
        return Ok(());
    };
    reserve(ac, &mut stamp, t, classes)?;
    ac.stamp.set(stamp);
    Ok(())
}

/// Reserve one message that is no verdict and no question (`assign` writes board fields before
/// its message).
pub(crate) fn reserve_message(ac: &AgentCtx, t: &TicketRef) -> CbResult<()> {
    reserve_ahead(ac, t, &[StampClass::Other])
}

/// Reserve the verify result of `verify`, and with `question` also the question to the owner
/// that a failed verify at the round limit posts after it. Both or none: a day that has no
/// room for the question refuses the verdict too, so no stop is half written and the limit of
/// the questions holds.
pub(crate) fn reserve_verdict(ac: &AgentCtx, t: &TicketRef, question: bool) -> CbResult<()> {
    if question {
        reserve_ahead(ac, t, &[StampClass::Verdict, StampClass::Ask])
    } else {
        reserve_ahead(ac, t, &[StampClass::Verdict])
    }
}

/// The body that `send_signed` posts. Without a stamp in the context it is `body`. With one it
/// gets the last line `unattended: <n>/<budget>`: `n` is the number of verdicts of the day, with
/// this message when it is a verify result. The message is reserved first ([`reserve`]), unless
/// the command reserved it already.
pub(crate) fn stamp_body(
    ac: &AgentCtx,
    t: &TicketRef,
    to: &To,
    kind: Kind,
    body: &str,
) -> CbResult<String> {
    // every signed message of a headless config dir; an attended command checks the text that
    // a person passed in, and a message that crewboard builds (a merge notice) is not a text
    // of a person
    if has_unattended_line(body) && ac.ctx.cd.is_headless()? {
        return Err(crate::headless::unattended_refusal(
            "the text has a line that starts with `unattended:`; only crewboard writes that line",
        )
        .into());
    }
    let Some(mut stamp) = ac.stamp.get() else {
        return Ok(body.to_string());
    };
    let class = if matches!(
        crate::protocol::acts::parse(Some(kind), body),
        Act::Verify { .. }
    ) {
        StampClass::Verdict
    } else if kind == Kind::Question && *to == To::Owner {
        StampClass::Ask
    } else {
        StampClass::Other
    };
    match stamp.pending.iter().position(|c| *c == class) {
        Some(at) => {
            stamp.pending.remove(at);
        }
        None => {
            reserve(ac, &mut stamp, t, &[class])?;
            if let Some(at) = stamp.pending.iter().position(|c| *c == class) {
                stamp.pending.remove(at);
            }
        }
    }
    // The line is strict: `n` is never above the budget.
    let line = unattended_line(stamp.used.min(stamp.budget), stamp.budget);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_floor_of_a_ticket_has_one_key_for_every_letter_case_of_the_repo() {
        let typed = TicketRef::new("Acme/Widgets", 7);
        let lower = TicketRef::new("acme/widgets", 7);
        assert_eq!(floor_key(&typed), "acme/widgets#7");
        assert_eq!(floor_key(&typed), floor_key(&lower));
        // the write and the read use that key
        let mut tick = TickState::default();
        tick.unattended_tickets.insert(
            floor_key(&typed),
            TicketFloor {
                count: 2,
                after: 40,
                at: 1,
            },
        );
        assert_eq!(floor_of(&tick, &typed), Some((2, 40)));
        assert_eq!(floor_of(&tick, &lower), Some((2, 40)));
        assert_eq!(floor_of(&tick, &TicketRef::new("acme/widgets", 8)), None);
    }

    #[test]
    fn the_keys_that_only_crewboard_writes_are_found_in_a_text() {
        for (text, key) in [
            ("fine\nunattended: 1/10", Some("unattended")),
            ("  unattended: x", Some("unattended")),
            ("fine\nverified-by: unattended", Some("verified-by")),
            (" verified-by: me", Some("verified-by")),
            ("verified-by:", Some("verified-by")),
            ("it was verified-by: me", None),
            ("unattended coordinator: mode verify", None),
            ("", None),
        ] {
            assert_eq!(written_by_crewboard_only(text), key, "{text:?}");
        }
    }

    #[test]
    fn the_note_of_a_spent_budget_has_its_key_and_no_line_of_the_protocol_key() {
        let body = budget_spent_body("2026-11-28", 10, 10, "AUTHORITY.md");
        assert!(
            body.lines()
                .any(|l| l == "event=unattended-budget:2026-11-28"),
            "{body}"
        );
        assert!(body.starts_with(
            "The unattended coordinator used its verdicts of 2026-11-28 (UTC): 10 of 10.\n"
        ));
        assert!(!has_unattended_line(&body), "{body}");
        assert_ne!(
            budget_spent_key("2026-11-28"),
            budget_spent_key("2026-11-29")
        );
    }

    #[test]
    fn a_count_of_a_later_day_is_refused_and_one_of_today_or_before_is_not() {
        let floor = |day: &str| TickState {
            unattended_messages_floor: Some((day.to_string(), 1)),
            ..TickState::default()
        };
        assert!(check_clock(&TickState::default(), "2026-11-28").is_ok());
        assert!(check_clock(&floor("2026-11-28"), "2026-11-28").is_ok());
        assert!(check_clock(&floor("2026-11-27"), "2026-11-28").is_ok());
        let e = check_clock(&floor("2026-11-29"), "2026-11-28").unwrap_err();
        let text = e.to_string();
        assert!(text.contains("[rule=unattended-budget]"), "{text}");
        assert!(
            text.contains("2026-11-29") && text.contains("the clock"),
            "{text}"
        );
        // the memo of a feed counts too
        let mut memo = TickState::default();
        memo.unattended_feed.insert(
            "a/b".into(),
            UnattendedFeed {
                day: "2026-11-29".into(),
                ..UnattendedFeed::default()
            },
        );
        assert!(check_clock(&memo, "2026-11-28").is_err());
        assert!(check_clock(&memo, "2026-11-29").is_ok());
    }
}
