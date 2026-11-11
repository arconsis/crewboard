//! `retro` (spec/retro): reads the signed log of the tickets and prints a retrospective report.
//! See [`crate::retro`] for what counts. It writes nothing: no comment, no board field, no
//! cursor and no state file.
//!
//! Requests: the board, the authority file, the registry issue and the agent items (the reads of
//! every command that loads the registry), then one request per 100 comments of each repo.
//! With `--since`, the feed holds only the comments that changed since that time; each issue
//! that has a crew message among them is then read in full (one request per 100 comments of the
//! issue), because the report needs the whole log of a ticket.

use super::audit::parse_since;
use super::{AgentCtx, Ctx, Global, load_world, new_client, resolve_board_config};
use crate::base::TicketRef;
use crate::clock::{format_ts, now_epoch};
use crate::comments::Comment;
use crate::error::CbResult;
use crate::feed;
use crate::registry::{CREW_PREFIX, Trust};
use crate::retro::{Event, Report, Spread, TicketLog, retro};
use crate::state::StateCache;
use crate::ticket::{Sender, out_of_window_messages, verified_messages};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// What `retro` prints, and whether the report has no ticket (exit 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Out {
    pub text: String,
    pub empty: bool,
}

/// The context of a reader of the board, as `status` opens it: a joined agent reads with its own
/// config and token, anyone else (the owner) with the board config of the config dir and the
/// token of the caller. No role is asked for: the report holds only what the comments of the
/// repos show to every reader.
fn open(g: &Global) -> CbResult<Ctx> {
    match AgentCtx::load(g) {
        Ok(ac) => Ok(ac.ctx),
        Err(_) => Ctx::load(new_client()?, resolve_board_config(g)?, g.dir()?),
    }
}

/// The comments of the tickets, by issue. Without `since`: the whole feed of every repo. With
/// `since`: every issue with a crew message that changed since then, read in full.
fn read_comments(ctx: &Ctx, since: Option<i64>) -> CbResult<BTreeMap<TicketRef, Vec<Comment>>> {
    let owner = &ctx.cfg.owner_login;
    let mut by: BTreeMap<TicketRef, Vec<Comment>> = BTreeMap::new();
    for repo in &ctx.cfg.repos {
        let feed = feed::feed_since(&ctx.client, repo, since.unwrap_or(0), None, owner)?;
        if since.is_none() {
            for c in feed.comments {
                by.entry(TicketRef::new(repo.as_str(), c.issue))
                    .or_default()
                    .push(c);
            }
            continue;
        }
        let mut touched: Vec<u64> = feed
            .comments
            .iter()
            .filter(|c| c.body.starts_with(CREW_PREFIX))
            .map(|c| c.issue)
            .collect();
        touched.sort_unstable();
        touched.dedup();
        for issue in touched {
            let (comments, _) = feed::issue_comments(&ctx.client, repo, issue, None, owner)?;
            by.insert(TicketRef::new(repo.as_str(), issue), comments);
        }
    }
    // the registry issue holds joins and owner reports, no ticket
    by.remove(&TicketRef::new(ctx.main_repo(), ctx.cfg.registry_issue));
    Ok(by)
}

/// The counted messages of one issue: the verified messages and the messages that fail only
/// the replay window, in comment order. `None` with `since` when no agent message of the issue
/// was created at or after that time.
fn ticket_log(
    ctx: &Ctx,
    trust: &Trust,
    ticket: TicketRef,
    comments: &[Comment],
    since: Option<i64>,
) -> Option<TicketLog> {
    let mut msgs = verified_messages(comments, &ctx.cfg, trust);
    let late = out_of_window_messages(comments, &ctx.cfg, trust);
    let outside_window = late.len();
    msgs.extend(late);
    msgs.sort_by_key(|m| m.comment_id);
    if let Some(t) = since
        && !msgs
            .iter()
            .any(|m| m.sender != Sender::Owner && m.created >= t)
    {
        return None;
    }
    Some(TicketLog {
        ticket,
        urls: comments.iter().map(|c| (c.id, c.url.clone())).collect(),
        msgs,
        outside_window,
    })
}

pub fn run(g: &Global, json_out: bool, since: Option<&str>) -> CbResult<Out> {
    // a bad `--since` is a usage error, before any read
    let since = since.map(|s| parse_since(s, now_epoch())).transpose()?;
    let ctx = open(g)?;
    let world = load_world(&ctx, &[], &mut StateCache::default())?;
    let logs: Vec<TicketLog> = read_comments(&ctx, since)?
        .into_iter()
        .filter_map(|(ticket, comments)| ticket_log(&ctx, &world.trust, ticket, &comments, since))
        .collect();
    let report = retro(&logs);
    let view = View {
        initiative: &ctx.cfg.initiative,
        main_repo: ctx.main_repo(),
        since,
    };
    Ok(Out {
        empty: report.tickets == 0,
        text: if json_out {
            render_json(&view, &report).to_string() + "\n"
        } else {
            render(&view, &report)
        },
    })
}

/// What the output needs besides the report.
pub struct View<'a> {
    pub initiative: &'a str,
    /// A ticket of this repo prints as `#n`, any other as `owner/repo#n`.
    pub main_repo: &'a str,
    pub since: Option<i64>,
}

impl View<'_> {
    fn ticket(&self, t: &TicketRef) -> String {
        if t.repo.eq_ignore_ascii_case(self.main_repo) {
            format!("#{}", t.issue)
        } else {
            t.to_string()
        }
    }
}

/// Seconds as hours with one decimal.
fn hours(s: i64) -> f64 {
    (s as f64 / 360.0).round() / 10.0
}

fn percent(part: usize, of: usize) -> String {
    (part * 100 + of / 2)
        .checked_div(of)
        .map_or(String::new(), |p| format!(" ({p}%)"))
}

/// One line per event: the ticket and the URL of the comment. Never the agent.
fn event_lines(v: &View, events: &[Event]) -> String {
    events
        .iter()
        .map(|e| format!("    {} {}\n", v.ticket(&e.ticket), e.url))
        .collect()
}

fn spread_text(s: Option<Spread>) -> String {
    match s {
        Some(s) => format!(
            "median {:.1} h, p90 {:.1} h ({} ticket(s))",
            hours(s.median_s),
            hours(s.p90_s),
            s.tickets
        ),
        None => "no ticket".to_string(),
    }
}

fn since_text(v: &View) -> String {
    v.since.map_or(String::new(), |t| {
        format!(
            "Since {}: the tickets with a signed message since then, each with its whole log.\n",
            format_ts(t)
        )
    })
}

pub fn render(v: &View, r: &Report) -> String {
    if r.tickets == 0 {
        return format!(
            "retro of initiative {}: no ticket with a signed assignment\n{}",
            v.initiative,
            since_text(v)
        );
    }
    let mut s = format!(
        "retro of initiative {}: {} ticket(s), {} signed message(s), {} to {}\nSource: verified signed messages only. The report counts classes and ranks no agent.\n{}",
        v.initiative,
        r.tickets,
        r.messages,
        r.first.map_or("?".to_string(), format_ts),
        r.last.map_or("?".to_string(), format_ts),
        since_text(v)
    );
    if r.outside_window > 0 {
        s.push_str(&format!(
            "{} message(s) with a signed time outside the replay window count too (docs/design/spec/retro.md#what-counts).\n",
            r.outside_window
        ));
    }
    s.push_str(&format!(
        "\nVolume\n  tickets with an assignment: {}\n  delivered tickets (at least one done): {}\n",
        r.tickets, r.delivered
    ));
    let y = &r.verify;
    s.push_str(&format!(
        "\nVerify\n  tickets verified: {}\n  first verify passed: {} of {}{}\n  tickets with a verify fail: {} ({} without {a})\n  verify fails: {} ({} without {a})\n",
        y.verified,
        y.first_pass,
        y.verified,
        percent(y.first_pass, y.verified),
        y.tickets_with_fail,
        y.rework_tickets,
        y.fails,
        y.rework_fails,
        a = crate::retro::ACCEPTED_AFTER_HEAD_MOVED,
    ));
    s.push_str("\nVerify fails by class (a fail with failures of two classes is in both)\n");
    if r.classes.is_empty() {
        s.push_str("  none\n");
    }
    for c in &r.classes {
        s.push_str(&format!("  {}: {}\n", c.class, c.events.len()));
        s.push_str(&event_lines(v, &c.events));
    }
    s.push_str("\nRounds (the tickets with the most verify fails)\n");
    if r.rounds.is_empty() {
        s.push_str("  none\n");
    }
    for x in &r.rounds {
        s.push_str(&format!(
            "  {}: {} fail(s) {}\n",
            v.ticket(&x.ticket),
            x.fails,
            x.latest.url
        ));
    }
    render_andon(v, r, &mut s);
    s.push_str(
        "\nRefusals mentioned (rule ids in question, blocked and report messages of dev agents)\n",
    );
    if r.refusals.is_empty() {
        s.push_str("  none\n");
    }
    for x in &r.refusals {
        s.push_str(&format!("  {}: {}\n", x.rule, x.events.len()));
        s.push_str(&event_lines(v, &x.events));
    }
    render_flow(r, &mut s);
    s
}

fn render_andon(v: &View, r: &Report, s: &mut String) {
    let a = &r.andon;
    s.push_str(&format!(
        "\nAndon\n  questions to the owner: {}\n  questions to the coordinator: {}\n  questions to another agent: {}\n  blocked: {}\n  parked: {}\n  questions to the owner still open: {}\n",
        a.questions_owner.len(),
        a.questions_coordinator.len(),
        a.questions_other.len(),
        a.blocked.len(),
        a.parked.len(),
        a.open_questions.len()
    ));
    s.push_str(&event_lines(v, &a.open_questions));
}

fn render_flow(r: &Report, s: &mut String) {
    let f = &r.flow;
    let closes = if f.closes.is_empty() {
        "none".to_string()
    } else {
        f.closes
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    s.push_str(&format!(
        "\nFlow\n  reassigned tickets: {}\n  dropped tickets: {}\n  closes: {closes}\n  late notes: {}\n  assignment to first done: {}\n  first done to close: {}\n",
        f.reassigned_tickets,
        f.dropped.len(),
        f.late_notes.len(),
        spread_text(f.assign_to_done),
        spread_text(f.done_to_close)
    ));
}

fn event_json(e: &Event) -> Value {
    json!({"ticket": e.ticket.to_string(), "issue": e.ticket.issue, "url": e.url,
           "at": format_ts(e.at), "from": e.from, "to": e.to})
}

/// A list of events with its count.
fn events_json(events: &[Event]) -> Value {
    json!({"count": events.len(), "events": events.iter().map(event_json).collect::<Vec<_>>()})
}

fn spread_json(s: Option<Spread>) -> Value {
    s.map_or(
        Value::Null,
        |s| json!({"tickets": s.tickets, "median_h": hours(s.median_s), "p90_h": hours(s.p90_s)}),
    )
}

/// The same data as [`render`]. Every listed event carries the URL of its comment and, only
/// here, the sender and the addressee.
pub fn render_json(v: &View, r: &Report) -> Value {
    let (y, a, f) = (&r.verify, &r.andon, &r.flow);
    let rows = |class: &str, events: &[Event]| {
        let mut row = events_json(events);
        row["class"] = json!(class);
        row
    };
    json!({
        "initiative": v.initiative,
        "since": v.since.map(format_ts),
        "report": {
            "period": {"first": r.first.map(format_ts), "last": r.last.map(format_ts),
                       "messages": r.messages, "outside_window": r.outside_window},
            "tickets": r.tickets,
            "delivered": r.delivered,
            "verify": {"verified": y.verified, "first_pass": y.first_pass,
                       "tickets_with_fail": y.tickets_with_fail, "fails": y.fails,
                       "rework_tickets": y.rework_tickets, "rework_fails": y.rework_fails},
            "classes": r.classes.iter().map(|c| rows(c.class, &c.events)).collect::<Vec<_>>(),
            "rounds": r.rounds.iter().map(|x| json!({
                "ticket": x.ticket.to_string(), "issue": x.ticket.issue, "fails": x.fails,
                "url": x.latest.url})).collect::<Vec<_>>(),
            "andon": {"questions_owner": events_json(&a.questions_owner),
                      "questions_coordinator": events_json(&a.questions_coordinator),
                      "questions_other": events_json(&a.questions_other),
                      "blocked": events_json(&a.blocked),
                      "parked": events_json(&a.parked),
                      "open_questions": events_json(&a.open_questions)},
            "refusals": r.refusals.iter().map(|x| {
                let mut row = events_json(&x.events);
                row["rule"] = json!(x.rule);
                row
            }).collect::<Vec<_>>(),
            "flow": {"reassigned_tickets": f.reassigned_tickets,
                     "reassigns": events_json(&f.reassigns),
                     "dropped": events_json(&f.dropped),
                     "closes": f.closes,
                     "late_notes": events_json(&f.late_notes),
                     "assign_to_done": spread_json(f.assign_to_done),
                     "done_to_close": spread_json(f.done_to_close)},
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retro::{Andon, ClassRow, Flow, RefusalRow, Round, VerifyStats};

    const VIEW: View = View {
        initiative: "t",
        main_repo: "a/b",
        since: None,
    };

    fn event(repo: &str, issue: u64, comment: u64) -> Event {
        Event {
            ticket: TicketRef::new(repo, issue),
            comment_id: comment,
            url: format!("https://github.com/{repo}/issues/{issue}#issuecomment-{comment}"),
            at: 3600,
            from: "agent-a@host-a".into(),
            to: "agent-b@host-b".into(),
        }
    }

    fn report() -> Report {
        Report {
            tickets: 4,
            delivered: 3,
            messages: 40,
            outside_window: 0,
            first: Some(0),
            last: Some(86400),
            verify: VerifyStats {
                verified: 3,
                first_pass: 2,
                tickets_with_fail: 2,
                fails: 3,
                rework_tickets: 1,
                rework_fails: 2,
            },
            classes: vec![
                ClassRow {
                    class: "review",
                    events: vec![event("a/b", 7, 71), event("a/b", 7, 72)],
                },
                ClassRow {
                    class: crate::retro::ACCEPTED_AFTER_HEAD_MOVED,
                    events: vec![event("a/c", 2, 21)],
                },
            ],
            rounds: vec![Round {
                ticket: TicketRef::new("a/b", 7),
                fails: 2,
                latest: event("a/b", 7, 72),
            }],
            andon: Andon {
                questions_owner: vec![event("a/b", 8, 81)],
                open_questions: vec![event("a/b", 8, 81)],
                ..Andon::default()
            },
            refusals: vec![RefusalRow {
                rule: "done-needs-pr",
                events: vec![event("a/b", 7, 73)],
            }],
            flow: Flow {
                reassigned_tickets: 1,
                reassigns: vec![event("a/b", 9, 91)],
                closes: BTreeMap::from([("merged", 2), ("dropped", 1)]),
                dropped: vec![event("a/b", 9, 92)],
                assign_to_done: Some(Spread {
                    tickets: 3,
                    median_s: 7560,
                    p90_s: 28440,
                }),
                ..Flow::default()
            },
        }
    }

    #[test]
    fn the_text_has_every_section_and_names_no_agent() {
        let t = render(&VIEW, &report());
        for want in [
            "retro of initiative t: 4 ticket(s), 40 signed message(s), 1970-01-01T00:00:00Z to 1970-01-02T00:00:00Z\n",
            "  tickets with an assignment: 4\n  delivered tickets (at least one done): 3\n",
            "  first verify passed: 2 of 3 (67%)\n",
            "  tickets with a verify fail: 2 (1 without accepted-after-head-moved)\n",
            "  verify fails: 3 (2 without accepted-after-head-moved)\n",
            "  review: 2\n    #7 https://github.com/a/b/issues/7#issuecomment-71\n",
            // a ticket of another repo prints with its repo
            "  accepted-after-head-moved: 1\n    a/c#2 https://github.com/a/c/issues/2#issuecomment-21\n",
            "  #7: 2 fail(s) https://github.com/a/b/issues/7#issuecomment-72\n",
            "  questions to the owner: 1\n",
            "  questions to the owner still open: 1\n    #8 https://github.com/a/b/issues/8#issuecomment-81\n",
            "  done-needs-pr: 1\n    #7 https://github.com/a/b/issues/7#issuecomment-73\n",
            "  reassigned tickets: 1\n  dropped tickets: 1\n  closes: dropped 1, merged 2\n  late notes: 0\n",
            "  assignment to first done: median 2.1 h, p90 7.9 h (3 ticket(s))\n",
            "  first done to close: no ticket\n",
        ] {
            assert!(t.contains(want), "missing {want:?} in:\n{t}");
        }
        // blameless: classes, not agents
        assert!(
            !t.contains("agent-a@host-a") && !t.contains("agent-b@host-b"),
            "{t}"
        );
        assert!(!t.contains("outside the replay window"), "{t}");
        let late = Report {
            outside_window: 2,
            ..report()
        };
        assert!(
            render(&VIEW, &late)
                .contains("2 message(s) with a signed time outside the replay window"),
        );
    }

    #[test]
    fn an_empty_report_says_so_and_names_the_since_time() {
        let v = View {
            since: Some(3600),
            ..VIEW
        };
        let t = render(&v, &Report::default());
        assert_eq!(
            t,
            "retro of initiative t: no ticket with a signed assignment\nSince 1970-01-01T01:00:00Z: the tickets with a signed message since then, each with its whole log.\n"
        );
        let j = render_json(&v, &Report::default());
        assert_eq!(j["since"], "1970-01-01T01:00:00Z");
        assert_eq!(j["report"]["tickets"], 0);
        assert_eq!(j["report"]["period"]["first"], Value::Null);
        assert_eq!(j["report"]["flow"]["assign_to_done"], Value::Null);
    }

    #[test]
    fn the_json_has_the_same_numbers_and_the_url_and_the_agents_of_every_event() {
        let j = render_json(&VIEW, &report());
        let r = &j["report"];
        assert_eq!(j["initiative"], "t");
        assert_eq!(j["since"], Value::Null);
        assert_eq!(r["period"]["messages"], 40);
        assert_eq!(r["period"]["last"], "1970-01-02T00:00:00Z");
        assert_eq!((&r["tickets"], &r["delivered"]), (&json!(4), &json!(3)));
        assert_eq!(
            r["verify"],
            json!({"verified": 3, "first_pass": 2, "tickets_with_fail": 2, "fails": 3,
                   "rework_tickets": 1, "rework_fails": 2})
        );
        assert_eq!(r["classes"][0]["class"], "review");
        assert_eq!(r["classes"][0]["count"], 2);
        assert_eq!(
            r["classes"][0]["events"][0],
            json!({"ticket": "a/b#7", "issue": 7,
                   "url": "https://github.com/a/b/issues/7#issuecomment-71",
                   "at": "1970-01-01T01:00:00Z", "from": "agent-a@host-a",
                   "to": "agent-b@host-b"})
        );
        assert_eq!(r["classes"][1]["class"], "accepted-after-head-moved");
        assert_eq!(
            r["rounds"][0],
            json!({"ticket": "a/b#7", "issue": 7, "fails": 2,
                   "url": "https://github.com/a/b/issues/7#issuecomment-72"})
        );
        assert_eq!(r["andon"]["questions_owner"]["count"], 1);
        assert_eq!(r["andon"]["open_questions"]["events"][0]["issue"], 8);
        assert_eq!(r["andon"]["blocked"]["count"], 0);
        assert_eq!(r["refusals"][0]["rule"], "done-needs-pr");
        assert_eq!(r["refusals"][0]["events"][0]["issue"], 7);
        assert_eq!(r["flow"]["closes"], json!({"merged": 2, "dropped": 1}));
        assert_eq!(r["flow"]["dropped"]["events"][0]["issue"], 9);
        assert_eq!(
            r["flow"]["assign_to_done"],
            json!({"tickets": 3, "median_h": 2.1, "p90_h": 7.9})
        );
    }

    #[test]
    fn hours_round_to_one_decimal_and_a_share_of_nothing_prints_nothing() {
        assert_eq!(hours(0), 0.0);
        assert_eq!(hours(3600), 1.0);
        assert_eq!(hours(5400), 1.5);
        assert_eq!(hours(180), 0.1);
        assert_eq!(percent(1, 3), " (33%)");
        assert_eq!(percent(2, 3), " (67%)");
        assert_eq!(percent(0, 0), "");
    }
}
