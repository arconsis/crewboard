//! `tick` (spec/agents#headless-coordinator): the headless coordinator's work that needs
//! no event.
//!
//! The tick reads the tickets in Review. For each one whose Gate is CI only it runs the
//! existing `verify` and, when that passes, the existing `merge`. It has no checks of its own
//! for the merge: AUTHORITY.md, the evidence, the CI run, the head SHA and the Gate rule all
//! live in `verify` and `merge_ticket`. Every other Review ticket gets one line, "left for
//! the interactive coordinator: ...", and nothing is written for it.
//!
//! A ticket in Review whose issue is closed and whose PR the owner
//! merged by hand is closed (`owner_merge`). That is no judgment, so it needs no Gate rule.
//!
//! The same close for a ticket in In Progress, Blocked or Ready whose issue is
//! closed (`owner_closed_lines`). The owner may merge while the dev works, and the dev then runs
//! `block` to free the slot ([`merged_pr`]). The issue state comes with the board items that the
//! tick reads anyway (`content{... on Issue{state}}` in the item fragment). Read cost per tick:
//! zero for a ticket whose issue is open. A ticket whose issue is closed costs the read of the
//! ticket (issue, comments, item) and the PR lookup of `close`; a closed issue whose PR
//! the owner did not merge repeats that every tick. An owner-route ticket costs the PR (1), its
//! files and, for a merged patch-only PR, the patch and up to 5 files: at most 8 requests plus
//! extra pages (the normal search of PRs is not made for it). No note, no line then.
//!
//! A ticket PR that was merged without a passed `verify` gets one signed `info`
//! (`merged_unverified`). That is a fact, not a judgment.
//!
//! The assignee of an In Progress ticket whose PR was merged gets one signed `info`
//! (`merged_pr`). That is a fact, too.
//!
//! A ticket that the tick loads goes through the note check (`own_notes`): a signed note of this
//! coordinator that is gone gives one `!! SECURITY EVENT: deleted-message !!` block and one
//! report to the owner. It costs no extra read.
//!
//! A lost event cannot hide a finished ticket from the tick. A second run changes nothing: a
//! merged ticket is Done, a verified ticket is not verified again (`verify_result` for the
//! latest `done`), and a refused merge writes nothing.

use super::merge::merge_ticket;
use super::verify::verify;
use super::{Me, me_of, merged_pr, merged_unverified, owner_merge, owner_net};
use crate::error::{CbError, CbResult};
use crate::headless::Gate;
use crate::ops::{AgentCtx, World, load_world};
use crate::rules::Role;
use crate::rules::ids;
use crate::rules::status::Status;
use crate::services::ticket_index::{discover_closed_tickets, discover_tickets};
use crate::services::ticket_loader::{Ticket, load_ticket};
use crate::ticket::{latest_done, verify_result};

/// The note that the log line starts with (spec/agents#headless-coordinator).
const LEFT: &str = "left for the interactive coordinator";

/// The rows of the board items with `status` whose issue the cache knows.
fn rows_of(
    items: &[crate::items::ItemSnapshot],
    cache: &crate::state::StateCache,
    project: u64,
    status: Status,
) -> Vec<crate::ticket::TicketRow> {
    items
        .iter()
        .filter(|it| {
            it.project_number.is_none_or(|n| n == project) && status.is(it.field("Status"))
        })
        .filter_map(|it| {
            let (key, _) = cache.index.tickets.iter().find(|(_, v)| **v == it.id)?;
            // The rows of this view carry no priority and no order.
            Some(crate::ticket::TicketRow {
                order: None,
                ..crate::ticket::TicketRow::from_item(it, key)?
            })
        })
        .collect()
}

/// One line for a ticket that the tick does not handle.
fn left(key: &str, why: &str) -> String {
    format!("{LEFT}: {key}, {why}\n")
}

/// What the tick reads once: the world, the coordinator and the board items.
struct Scan<'a> {
    world: &'a World,
    me: &'a Me,
    items: &'a [crate::items::ItemSnapshot],
    project: u64,
    lim: crate::late::Expect,
    now: i64,
    /// The `deleted-message` events that the tick reported to the owner. Their notes are
    /// forgotten after the caller printed the output ([`TickOut::settle`]), not before: a tick
    /// that fails later, or whose output is not printed, raises them again.
    shown: std::cell::RefCell<Vec<crate::events::SecurityEvent>>,
}

/// The ids of the board items that the tick acts on, by what it does with them.
struct Buckets {
    /// In Review: the tick verifies and merges them, or leaves them.
    review: Vec<String>,
    /// In Progress and old enough for an escalated stale-work marker.
    old: Vec<String>,
    /// In Progress with a PR in the Evidence field, for the merged-PR note.
    pr: Vec<String>,
    /// In work (In Progress, Blocked, Ready) whose issue is closed (the state comes with the
    /// items).
    closed: Vec<String>,
}

impl Buckets {
    fn of(s: &Scan<'_>) -> Buckets {
        let ids = |keep: &dyn Fn(&crate::items::ItemSnapshot) -> bool| -> Vec<String> {
            s.items
                .iter()
                .filter(|it| it.project_number.is_none_or(|n| n == s.project) && keep(it))
                .map(|it| it.id.clone())
                .collect()
        };
        let in_progress_with_agent = |it: &crate::items::ItemSnapshot| {
            Status::InProgress.is(it.field("Status")) && it.field("Agent").is_some()
        };
        Buckets {
            review: ids(&|it| Status::Review.is(it.field("Status"))),
            old: ids(&|it| {
                in_progress_with_agent(it)
                    && crate::clock::parse_ts(&it.updated_at)
                        .is_some_and(|u| s.now - u > s.lim.push.min(s.lim.done) + s.lim.escalate)
            }),
            pr: ids(&|it| {
                in_progress_with_agent(it)
                    && it
                        .field("Evidence")
                        .and_then(merged_pr::pr_of_evidence)
                        .is_some()
            }),
            closed: ids(&|it| is_in_work(it.field("Status")) && it.issue_closed()),
        }
    }
}

/// The `repo#n` key of the ticket of a board item, from the cache.
fn issue_of(cache: &crate::state::StateCache, id: &str) -> Option<String> {
    cache
        .index
        .tickets
        .iter()
        .find(|(_, v)| v.as_str() == id)
        .map(|(k, _)| k.clone())
}

/// Resolves the items that the cache does not know by a scan of the issues. The Review tickets
/// come from the board itself, on every run: the cache never decides WHICH tickets exist; it
/// only saves a scan when it already knows the issue of an item.
fn discover_missing(
    ac: &AgentCtx,
    world: &World,
    cache: &mut crate::state::StateCache,
    b: &Buckets,
) -> CbResult<()> {
    if b.review
        .iter()
        .chain(&b.old)
        .chain(&b.pr)
        .any(|id| issue_of(cache, id).is_none())
    {
        discover_tickets(&ac.ctx.reader(), &world.trust, &mut cache.index)?;
    }
    // The issue of an owner-merged ticket is closed, so the scan of open issues does not list it.
    if b.review
        .iter()
        .chain(&b.pr)
        .chain(&b.closed)
        .any(|id| issue_of(cache, id).is_none())
    {
        discover_closed_tickets(&ac.ctx.reader(), &world.trust, &mut cache.index)?;
    }
    Ok(())
}

/// The closes and the notes of the tick that need no Review judgement: the tickets whose issue
/// the owner closed, the Done tickets, the dev of a merged PR and the stale-work markers.
/// Returns their lines.
fn closes_and_notes(
    ac: &mut AgentCtx,
    s: &Scan<'_>,
    b: &Buckets,
    cache: &mut crate::state::StateCache,
) -> CbResult<String> {
    // The Done tickets that the cache knows. The tick writes nothing for them.
    let done_rows = rows_of(s.items, cache, s.project, Status::Done);
    // Close first, so the dev of a closed ticket gets no "PR merged" note after it.
    // `close_loaded` writes the state cache itself, so the cache is saved before and read after.
    ac.save_cache_required(cache)?;
    let (closed_out, closed_done) = owner_closed_lines(ac, s, &b.closed, cache)?;
    *cache = ac.load_cache();
    let mut progress_rows = rows_of(s.items, cache, s.project, Status::InProgress);
    progress_rows.retain(|r| !closed_done.contains(&r.item_id));
    let mut net = owner_net::tick_lines(ac, s.world, &done_rows, cache);
    // The dev of an In Progress ticket hears that its PR was merged.
    net.push_str(&merged_pr::tick_lines(
        ac,
        s.world,
        s.items,
        &progress_rows,
        cache,
    )?);
    net.push_str(&crate::ops::late::tick_lines(
        ac,
        s.world,
        &progress_rows,
        cache,
        s.now,
    ));
    ac.save_cache_required(cache)?;
    let mut out = closed_out;
    out.push_str(&net);
    Ok(out)
}

/// The Review tickets as `(repo, issue)`; an item without a ticket gets a `left` line in `out`.
fn review_tickets(
    ids: &[String],
    cache: &crate::state::StateCache,
    out: &mut String,
) -> Vec<(String, u64)> {
    let mut review: Vec<(String, u64)> = Vec::new();
    for id in ids {
        match issue_of(cache, id).and_then(|k| {
            let (r, n) = k.rsplit_once('#')?;
            Some((r.to_string(), n.parse::<u64>().ok()?))
        }) {
            Some(t) => review.push(t),
            None => out.push_str(&left(
                id,
                "a board item in Review has no signed info comment with its ticket",
            )),
        }
    }
    review.sort();
    review
}

/// Handles each Review ticket: the owner merge, the note for a merge without a verify, the late
/// note to the owner, then verify and merge when the Gate is CI only. The lines go to `out`.
fn handle_review(
    ac: &mut AgentCtx,
    s: &Scan<'_>,
    review: Vec<(String, u64)>,
    cache: &crate::state::StateCache,
    out: &mut String,
) -> CbResult<()> {
    // The time in Review is the age of the board item; the coordinator heartbeat comes
    // from the agent items already read (no extra read). The owner hears through the late path.
    let review_rows = rows_of(s.items, cache, s.project, Status::Review);
    let (verify_late, coordinator_late) =
        crate::ops::late::review_lates(&ac.ctx, s.world, &review_rows, &s.lim, s.now);
    for (repo, issue) in review {
        let key = format!("{repo}#{issue}");
        let t = match load_ticket(&ac.ctx.reader(), &s.world.trust, &repo, issue) {
            Ok(t) => t,
            Err(CbError::Refused(r)) => {
                out.push_str(&left(&key, &format!("not readable as a ticket: {r}")));
                continue;
            }
            Err(e) => return Err(e),
        };
        // A deleted note of this coordinator on the ticket (its comments are loaded anyway).
        out.push_str(&deleted_note_lines(ac, s, &t)?);
        // The owner merged the PR by hand: close the ticket. Other cases of a closed issue go
        // on below.
        if !t.info.open {
            match owner_merge::close_loaded(ac, s.me, &t, None) {
                Ok(l) => {
                    out.push_str(&l);
                    continue;
                }
                Err(CbError::Refused(r)) if r.rule == ids::CLOSE_NEEDS_REASON => {}
                Err(e) => return Err(e),
            }
        }
        // A merged PR without a passed verify gets one note (no refusal, no undo).
        out.push_str(&merged_unverified::check_loaded(ac, &t)?);
        // The tick reads no PR head here, so it judges no verify wait it cannot judge from the
        // comments: an owner route (`verify` refuses it, the owner merges) and a passing verify
        // of the latest `done` (whether its head is still the PR head is for the `status` scan).
        let no_verify_wait = t.agent().is_some_and(|a| {
            latest_done(&t.msgs, a).is_some_and(|(d, _)| {
                crate::ticket::is_owner_route_done(d)
                    || verify_result(&t.msgs, d, t.issue) == Some(true)
            })
        });
        if !no_verify_wait && let Some(l) = verify_late.iter().find(|l| l.subject_name() == key) {
            out.push_str(&tell_owner_review(
                ac,
                &t,
                l,
                &s.lim,
                coordinator_late.as_ref(),
                s.now,
            )?);
        }
        out.push_str(&one(ac, &t, &key)?);
    }
    Ok(())
}

/// What `tick` gives its caller: the output, and the deleted notes that it showed in it and
/// reported to the owner.
pub struct TickOut {
    pub text: String,
    shown: Vec<crate::events::SecurityEvent>,
}

impl TickOut {
    /// Forget the reported deleted notes (`own_notes::tick_settle`). The caller calls it after
    /// the output is printed: if the print does not happen, the next tick raises them again.
    pub fn settle(&self, ac: &AgentCtx) -> CbResult<()> {
        crate::ops::own_notes::tick_settle(ac, &self.shown)
    }
}

/// `tick`: see the module text. Returns one line per Review ticket; the caller prints it, then
/// calls [`TickOut::settle`].
pub fn tick(ac: &mut AgentCtx) -> CbResult<TickOut> {
    if !ac.ctx.cd.is_headless()? {
        return Err(CbError::refused(
            "tick-headless-only",
            "tick is for a headless config dir (`headless = true`): it merges without a model that reads the Gate. The interactive coordinator uses verify and merge",
        ));
    }
    let mut cache = ac.load_cache();
    let world = load_world(&ac.ctx, std::slice::from_ref(&ac.agent.item_id), &mut cache)?;
    let me = me_of(ac, &world)?;
    if me.role != Role::Coordinator {
        return Err(CbError::refused(
            "verify-coordinator-only",
            "tick is a coordinator action",
        ));
    }
    // One item listing, then the items by id.
    let ids = ac.ctx.board.list_item_ids(&ac.ctx.client)?;
    let items: Vec<_> = ac
        .ctx
        .board
        .read_items(&ac.ctx.client, &ids)?
        .into_iter()
        .flatten()
        .collect();
    let scan = Scan {
        world: &world,
        me: &me,
        items: &items,
        project: ac.ctx.cfg.project_number,
        lim: crate::late::expect(&ac.ctx.cfg),
        now: crate::clock::now_epoch(),
        shown: std::cell::RefCell::default(),
    };
    let buckets = Buckets::of(&scan);
    discover_missing(ac, &world, &mut cache, &buckets)?;
    let mut out = closes_and_notes(ac, &scan, &buckets, &mut cache)?;
    if buckets.review.is_empty() {
        out = format!("tick: no ticket in Review\n{out}");
    } else {
        let review = review_tickets(&buckets.review, &cache, &mut out);
        handle_review(ac, &scan, review, &cache, &mut out)?;
    }
    // The output is complete and goes to the caller, which forgets the reported deleted notes
    // after its print (`TickOut::settle`). A failure before keeps them, and the next tick raises
    // them again.
    Ok(TickOut {
        text: out,
        shown: scan.shown.take(),
    })
}

/// A Review ticket that waits for a verify while the coordinator is late. The tick
/// posts ONE signed `info` to the owner (`late_owner_duty`) and logs it. It changes no Status and
/// refuses nothing.
fn tell_owner_review(
    ac: &AgentCtx,
    t: &Ticket,
    l: &crate::late::Late,
    lim: &crate::late::Expect,
    coordinator: Option<&crate::late::Late>,
    now: i64,
) -> CbResult<String> {
    let detail = coordinator.map_or(String::new(), |c| {
        format!(
            "coordinator heartbeat: {} (late)\n",
            crate::clock::format_ts(c.since)
        )
    });
    let note = crate::ops::late::Note {
        expect: lim,
        coordinator,
        now,
        detail: &detail,
    };
    let Some(p) = crate::ops::late::late_owner_duty(ac, l, &note, &t.msgs)? else {
        return Ok(String::new());
    };
    Ok(format!(
        "{}: late: verify; the interactive coordinator is late; told the owner (note {}, {})\n",
        t.key(),
        p.id,
        p.url
    ))
}

/// The tickets in In Progress, Blocked or Ready whose issue is closed. The close is
/// the one of Review (`owner_merge::close_loaded`). A refusal writes nothing and gives no line,
/// except `close-needs-reason` on a Blocked ticket: one `left` line. Returns the lines and the item ids that moved to Done.
fn owner_closed_lines(
    ac: &AgentCtx,
    s: &Scan<'_>,
    ids: &[String],
    cache: &crate::state::StateCache,
) -> CbResult<(String, std::collections::BTreeSet<String>)> {
    let (world, me) = (s.world, s.me);
    let mut keys: Vec<(String, u64, String)> = ids
        .iter()
        .filter_map(|id| {
            let (k, _) = cache.index.tickets.iter().find(|(_, v)| v.as_str() == id)?;
            let (r, n) = k.rsplit_once('#')?;
            Some((r.to_string(), n.parse::<u64>().ok()?, id.clone()))
        })
        .collect();
    keys.sort();
    let mut out = String::new();
    let mut done = std::collections::BTreeSet::new();
    for (repo, issue, id) in keys {
        let t = match load_ticket(&ac.ctx.reader(), &world.trust, &repo, issue) {
            Ok(t) => t,
            Err(CbError::Refused(_)) => continue,
            Err(e) => return Err(e),
        };
        out.push_str(&deleted_note_lines(ac, s, &t)?);
        match owner_merge::close_loaded(ac, me, &t, None) {
            Ok(l) => {
                out.push_str(&l);
                done.insert(id);
            }
            // A Blocked ticket whose issue the owner closed without a merge is for the interactive
            // coordinator: `close --why` is its judgment.
            Err(CbError::Refused(r))
                if r.rule == ids::CLOSE_NEEDS_REASON
                    && t.status().is_ok_and(|s| s == Status::Blocked) =>
            {
                out.push_str(&merged_unverified::check_loaded(ac, &t)?);
                out.push_str(&left(
                    &format!("{repo}#{issue}"),
                    &format!("Blocked, issue closed: {r}"),
                ));
            }
            Err(CbError::Refused(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok((out, done))
}

/// A deleted note of this coordinator on a ticket that the tick loaded (`own_notes`): one line
/// per gone note, and one report to the owner per event (the `event=` key). A failed report
/// fails the tick; a reported event goes to `s.shown`, to be forgotten when the tick returns.
fn deleted_note_lines(ac: &AgentCtx, s: &Scan<'_>, t: &Ticket) -> CbResult<String> {
    let world = s.world;
    let gone = crate::ops::own_notes::tick_check(ac, world, t)?;
    if gone.is_empty() {
        return Ok(String::new());
    }
    let mut reported = crate::audit::reported_events(&crate::ctx::registry_msgs(ac, world));
    let mut out = String::new();
    for e in &gone {
        let told = crate::ops::registry_post::report_security(ac, e, &mut reported)?;
        out.push_str(&e.render());
        if let Some(url) = told {
            out.push_str(&format!("told the owner: {url}\n"));
        }
        s.shown.borrow_mut().push(e.clone());
    }
    Ok(out)
}

/// Is the board Status one of a ticket in work (a legacy `Needs Human` reads as Blocked)?
fn is_in_work(status: Option<&str>) -> bool {
    matches!(
        status.and_then(Status::parse_board),
        Some(Status::InProgress | Status::Blocked | Status::Ready)
    )
}

/// One Review ticket: its line or lines.
fn one(ac: &mut AgentCtx, t: &Ticket, key: &str) -> CbResult<String> {
    // The Gate is the signed `gate:` line of the assignment, not the body (spec/agents#headless-coordinator).
    let gate = match crate::ticket::signed_gate(&t.msgs) {
        None => return Ok(left(key, "no signed gate line: the Gate is not CI only")),
        Some(g) => Gate::parse(&g),
    };
    if let Gate::Text(g) = &gate {
        let shown: String = g.chars().take(80).collect();
        return Ok(left(
            key,
            &format!(
                "the Gate is not CI only (signed Gate: '{shown}'; a CI-only ticket has the Gate `{}`: `crewboard ticket new --gate-ci` writes it)",
                crate::headless::GATE_CI
            ),
        ));
    }
    if crate::ticket::ci_none_declared(&t.msgs, &t.info.body) {
        return Ok(left(
            key,
            "the Gate is not CI only (the ticket declares `CI: none`)",
        ));
    }
    one_ci_only(ac, t, key)
}

/// One Review ticket whose Gate is CI only: the checks of the evidence, then verify and merge.
fn one_ci_only(ac: &mut AgentCtx, t: &Ticket, key: &str) -> CbResult<String> {
    let Some(assignee) = t.agent() else {
        return Ok(left(key, "no assignee"));
    };
    let Some((done, ev)) = latest_done(&t.msgs, assignee) else {
        return Ok(left(key, "no signed done message with evidence"));
    };
    // Rule `protected-path-owner-merge`: the `done` of a PR that changes a protected path has a
    // `protected-paths:` line. The owner merges such a PR; no read is needed. `verify` and
    // `merge` still check the live file list.
    if crate::ticket::kv(&done.body, "protected-paths").is_some() {
        return Ok(left(key, "protected path: the owner merges this PR"));
    }
    // A local gate (`ci_mode: local`) is a judgment of the interactive coordinator.
    if crate::ticket::is_local_gate(&ev) {
        return Ok(left(
            key,
            "the evidence is a local gate ('ci: local-gate'): a local gate is a judgment, so the Gate is not CI only",
        ));
    }
    // spec/agents#headless-coordinator: evidence without a CI run is not CI only.
    if crate::domain::ci::triggers::evidence_has_no_run(&ev.ci) {
        return Ok(left(
            key,
            &format!(
                "the evidence is 'ci: {}': no CI run proves the change, so the Gate is not CI only",
                ev.ci
            ),
        ));
    }
    // A run named by hand (`done --run`) is not CI only either.
    if ev
        .ci_runs
        .as_deref()
        .is_some_and(crate::domain::ci::evaluate::names_runs)
    {
        return Ok(left(
            key,
            "the evidence names a CI run by hand (`named` in its ci-runs line), so the Gate is not CI only",
        ));
    }
    let mut cache = ac.load_cache();
    if cache.tick.tick_requested.get(key) == Some(&done.comment_id) {
        return Ok(left(
            key,
            "a merge attempt asked the dev for a change; waiting for a new done",
        ));
    }
    let mut out = String::new();
    match verify_result(&t.msgs, done, t.issue) {
        Some(true) => {}
        Some(false) => return Ok(left(key, "the verify of the latest done failed")),
        None => {
            let v = match verify(
                ac,
                key,
                "tick: Gate is CI only; evidence and CI checked by verify",
                &[],
                false,
                None,
            ) {
                Ok(v) => v,
                Err(CbError::Refused(r)) => return Ok(left(key, &format!("verify refused: {r}"))),
                Err(e) => return Err(e),
            };
            out.push_str(&v);
            if !v.starts_with("verify PASS") {
                return Ok(out);
            }
        }
    }
    match merge_ticket(ac, key, "squash", &[]) {
        Ok(m) => out.push_str(&m),
        Err(CbError::Refused(r)) => {
            if r.message.contains(". Request ") {
                // The merge sent the dev a request (a conflict). Once per `done` is enough.
                cache
                    .tick
                    .tick_requested
                    .insert(key.to_string(), done.comment_id);
                ac.save_cache_required(&mut cache)?;
            }
            out.push_str(&left(key, &format!("merge refused: {r}")));
        }
        Err(e) => return Err(e),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::is_in_work;

    #[test]
    fn a_ticket_in_work_is_in_progress_blocked_ready_or_the_legacy_option() {
        for s in ["In Progress", "Blocked", "Ready", "Needs Human"] {
            assert!(is_in_work(Some(s)), "{s}");
        }
        for s in ["Review", "Done", "Backlog", "?"] {
            assert!(!is_in_work(Some(s)), "{s}");
        }
        assert!(!is_in_work(None));
    }
}
