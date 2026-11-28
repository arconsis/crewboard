//! The judgment list and the verdict count of the unattended coordinator in the output of
//! `tick` (spec/agents#unattended-coordinator), against the fake GitHub.
//!
//! With `unattended_coordinator` absent or `off` the tick prints and reads what it does without
//! the key. With `verify` it lists the tickets that need a judgment and counts the verdicts of
//! the day from the signed messages. The tick writes nothing for the list.

use crate::support::board::{Out, REPO};
use crate::support::headless::{Hub, PR, SHA, Scenario, TICKET};
use crewboard::clock::now_epoch;
use crewboard::protocol::acts::{reassigned_body, round_limit_question, unattended_line};
use crewboard::protocol::names::Kind;

const BY_HAND: &str = "Goal: fix it\nGate: the demo runs\n";
const GATE: &str = "the demo runs";
const NEEDS: &str = "needs judgment: ";
const VERIFY: &str = "unattended_coordinator: verify\n";

fn key(n: u64) -> String {
    format!("{REPO}#{n}")
}

fn item_of(n: u64) -> String {
    format!("PVTI_issue_{n}")
}

/// One more ticket: the issue `n`, its board item and the signed info comment that names it.
fn ticket(h: &Hub, n: u64, status: &str, agent: bool) {
    h.board.issue(n, &["crew:work"]);
    let mut fields = vec![("Status", status), ("Gate", "from the body")];
    if agent {
        fields.push(("Agent", h.dev.name.as_str()));
    }
    h.board.item(&item_of(n), &fields);
    coord(
        h,
        n,
        "owner",
        Kind::Info,
        &format!("Ticket is on the board.\nitem={}\nproject=15\n", item_of(n)),
    );
}

/// A message of the coordinator on the ticket `n`. Returns the comment id.
fn coord(h: &Hub, n: u64, to: &str, kind: Kind, body: &str) -> u64 {
    h.board
        .signed_now(&h.coord.key, &h.coord.name, to, kind, n, body)
        .0
}

/// A message of the dev agent on the ticket `n`. Returns the comment id.
fn dev(h: &Hub, n: u64, to: &str, kind: Kind, body: &str) -> u64 {
    h.board
        .signed_now(&h.dev.key, &h.dev.name, to, kind, n, body)
        .0
}

fn assign(h: &Hub, n: u64) {
    let body = format!(
        "Assigned to you: Fix it\nticket: {}\nitem={}\ngate: {GATE}\n",
        key(n),
        item_of(n)
    );
    coord(h, n, &h.dev.name, Kind::Request, &body);
}

/// A `done` of the dev agent with the evidence of the fake PR, and `extra` lines after it.
fn done(h: &Hub, n: u64, extra: &str) -> u64 {
    let body = format!(
        "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\n{extra}"
    );
    dev(h, n, &h.coord.name, Kind::Done, &body)
}

/// A verify result of the coordinator for the `done` in the comment `done`, created `ago`
/// seconds ago, with `extra` lines after it. Returns the comment id.
fn verdict(h: &Hub, n: u64, done: u64, extra: &str, ago: i64) -> u64 {
    h.board.signed_re_ago(
        &h.coord.key,
        &h.coord.name,
        &h.dev.name,
        Kind::Report,
        n,
        done,
        &format!("verify: fail\nsha: {SHA}\nfailed: the demo does not run\n{extra}"),
        ago,
    )
}

fn stamp(n: u32) -> String {
    format!("{}\n", unattended_line(n, 10))
}

/// The board of these tests. The numbers are the issues.
///
/// - 5: Review, a Gate in prose, CI evidence, no verify (the ticket of the fake): on the list.
/// - 6: Blocked by its agent: on the list.
/// - 7: Blocked, a question to the owner is open: not on the list.
/// - 8: Blocked, stopped at the round limit: not on the list.
/// - 9: Review, the `done` names a protected path: not on the list.
/// - 10: Review, two unattended verdicts and a new `done`: not on the list.
/// - 11: Ready without an agent: on the list.
/// - 12: Backlog, returned by a reassign: on the list.
/// - 13: Backlog, never assigned: not on the list.
/// - 14: Ready with an agent: not on the list.
fn board() -> Hub {
    let h = Hub::new(Scenario {
        body: BY_HAND,
        may_merge: true,
        ci_green: true,
    });
    ticket(&h, 6, "Blocked", true);
    assign(&h, 6);
    dev(
        &h,
        6,
        "coordinator",
        Kind::Blocked,
        "the API key is missing",
    );

    ticket(&h, 7, "Blocked", true);
    assign(&h, 7);
    dev(
        &h,
        7,
        "coordinator",
        Kind::Blocked,
        "the API key is missing",
    );
    dev(&h, 7, "owner", Kind::Question, "May I delete the old data?");

    ticket(&h, 8, "Blocked", true);
    assign(&h, 8);
    dev(
        &h,
        8,
        "coordinator",
        Kind::Blocked,
        "the API key is missing",
    );
    let stop = round_limit_question(1, 1, "https://example.invalid/report");
    coord(&h, 8, "owner", Kind::Question, &stop);

    ticket(&h, 9, "Review", true);
    assign(&h, 9);
    done(&h, 9, "protected-paths: .github/workflows/ci.yml\n");

    ticket(&h, 10, "Review", true);
    assign(&h, 10);
    let d = done(&h, 10, "");
    verdict(&h, 10, d, &stamp(1), 20);
    let d = done(&h, 10, "");
    verdict(&h, 10, d, &stamp(2), 10);
    done(&h, 10, "");

    ticket(&h, 11, "Ready", false);

    ticket(&h, 12, "Backlog", false);
    assign(&h, 12);
    let back = reassigned_body("the agent is dead");
    coord(&h, 12, &h.dev.name, Kind::Info, &back);

    ticket(&h, 13, "Backlog", false);
    ticket(&h, 14, "Ready", true);
    assign(&h, 14);
    h
}

fn tick(h: &Hub) -> Out {
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    o
}

/// The requests of one more tick, as `METHOD path`.
fn tick_requests(h: &Hub) -> (Out, Vec<String>) {
    let from = h.requests().len();
    let o = tick(h);
    let reqs = h.requests()[from..]
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    (o, reqs)
}

fn needs(o: &Out) -> Vec<&str> {
    o.stdout
        .lines()
        .filter_map(|l| l.strip_prefix(NEEDS))
        .collect()
}

fn count_line(o: &Out) -> Vec<&str> {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("unattended: "))
        .collect()
}

/// The line that the tick prints for a Review ticket with the Gate of these tests.
fn left(n: u64) -> String {
    format!(
        "left for the interactive coordinator: {}, the Gate is not CI only (signed Gate: '{GATE}'; a CI-only ticket has the Gate `ci`: `crewboard ticket new --gate-ci` writes it)\n",
        key(n)
    )
}

/// The UTC day of now, as a number.
fn day() -> i64 {
    now_epoch().div_euclid(86_400)
}

/// Is the start of the UTC day less than two minutes back? Then a message that these tests
/// create "some seconds ago" can be of yesterday, and a test of the count of today proves
/// nothing.
fn day_just_started() -> bool {
    now_epoch().rem_euclid(86_400) < 120
}

#[test]
fn with_the_key_absent_or_off_the_tick_prints_and_reads_what_it_did() {
    // the output of a tick without the key, byte for byte: one line per Review ticket
    let expected = format!("{}{}{}", left(TICKET), left(9), left(10));
    let mut runs = Vec::new();
    for extra in [
        "",
        "unattended_coordinator: off\n",
        "unattended_coordinator: off\nunattended_budget: 3\n",
        "unattended_budget: 3\n",
    ] {
        let h = board();
        h.set_authority_extra(extra);
        // the first tick fills the ticket index of the state; the second one is the steady state
        let (first, first_reqs) = tick_requests(&h);
        let (second, second_reqs) = tick_requests(&h);
        for o in [&first, &second] {
            assert_eq!(o.stdout, expected, "{extra:?}: {}", o.all());
        }
        for reqs in [&first_reqs, &second_reqs] {
            assert!(
                !reqs.iter().any(|r| r.contains("/issues/comments?")),
                "{extra:?}: the tick read the comment feed: {reqs:#?}"
            );
        }
        // the steady state reads no comments of a ticket that is not in Review
        for n in [6, 7, 8, 11, 12, 13, 14] {
            assert!(
                !second_reqs
                    .iter()
                    .any(|r| r.contains(&format!("/issues/{n}/comments"))),
                "{extra:?}: the tick read the comments of #{n}: {second_reqs:#?}"
            );
        }
        assert!(h.writes().is_empty(), "{extra:?}: {:?}", h.writes().len());
        assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
        runs.push((first_reqs, second_reqs));
    }
    // the same requests, in the same order, with the key absent, off, and off with a budget
    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run, &runs[0],
            "run {i} differs from the run without the key"
        );
    }
    // control: the comparison can fail, because the key `verify` adds requests and lines
    let h = board();
    h.set_authority_extra(VERIFY);
    let (first, first_reqs) = tick_requests(&h);
    assert_ne!(first.stdout, expected);
    assert!(first.stdout.starts_with(&expected), "{}", first.all());
    assert_ne!(first_reqs, runs[0].0);
}

#[test]
fn with_the_key_verify_the_tick_lists_the_tickets_that_need_a_judgment() {
    let today = day();
    let counted = !day_just_started();
    let h = board();
    h.set_authority_extra(VERIFY);
    let o = tick(&h);
    assert_eq!(
        needs(&o),
        [
            format!("{}, prose Gate", key(TICKET)),
            format!("{}, blocked", key(6)),
            format!("{}, ready, no agent", key(11)),
            format!("{}, returned", key(12)),
        ],
        "{}",
        o.all()
    );
    // the lines of the tick stay, and the list comes after them
    let lefts = format!("{}{}{}", left(TICKET), left(9), left(10));
    assert!(o.stdout.starts_with(&lefts), "{}", o.all());
    if counted && day() == today {
        // the two stamped verdicts of #10
        assert_eq!(
            count_line(&o),
            ["unattended: mode verify; verdicts 2/10 today"],
            "{}",
            o.all()
        );
        assert!(
            o.stdout
                .ends_with("unattended: mode verify; verdicts 2/10 today\n"),
            "{}",
            o.all()
        );
    }
    // the list is a view: the tick wrote nothing
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());

    // an answer takes a ticket off the list, and a question puts one on it
    coord(&h, 6, &h.dev.name, Kind::Answer, "Use the test key.");
    dev(&h, 14, "coordinator", Kind::Question, "Which port?");
    h.board.set_item(&item_of(14), "Status", "Blocked");
    // the owner answered the question of #7: the ticket has its answer and stays off the list
    h.board.owner_comment_now(7, "Yes, delete it.");
    let o = tick(&h);
    assert_eq!(
        needs(&o),
        [
            format!("{}, prose Gate", key(TICKET)),
            format!("{}, ready, no agent", key(11)),
            format!("{}, returned", key(12)),
            format!("{}, question", key(14)),
        ],
        "{}",
        o.all()
    );

    // the other mode lists the same tickets and names itself
    h.set_authority_extra("unattended_coordinator: verify-and-merge\nunattended_budget: 4\n");
    let o = tick(&h);
    assert_eq!(needs(&o).len(), 4, "{}", o.all());
    if counted && day() == today {
        assert_eq!(
            count_line(&o),
            ["unattended: mode verify-and-merge; verdicts 2/4 today"],
            "{}",
            o.all()
        );
    }
    assert!(!h.merged());
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
}

#[test]
fn the_key_verify_costs_the_feed_of_the_day_and_one_comment_read_per_candidate() {
    let h = board();
    tick(&h);
    let (_, off) = tick_requests(&h);
    h.set_authority_extra(VERIFY);
    let (_, on) = tick_requests(&h);
    let extra: Vec<&String> = on.iter().filter(|r| !off.contains(r)).collect();
    // one feed read, then the comments of the Blocked tickets (6, 7, 8) and of the Backlog
    // tickets without an agent that have a Gate (12, 13)
    assert_eq!(extra.len(), 6, "{extra:#?}");
    assert!(extra[0].contains("/issues/comments?since="), "{extra:#?}");
    for n in [6, 7, 8, 12, 13] {
        assert_eq!(
            extra
                .iter()
                .filter(|r| r.contains(&format!("/issues/{n}/comments")))
                .count(),
            1,
            "#{n}: {extra:#?}"
        );
    }
    assert_eq!(on.len(), off.len() + 6, "on: {on:#?}\noff: {off:#?}");
    // Ready tickets cost no read: the board fields decide
    for n in [11, 14] {
        assert!(
            !on.iter()
                .any(|r| r.contains(&format!("/issues/{n}/comments"))),
            "{on:#?}"
        );
    }
}

#[test]
fn the_count_comes_from_the_signed_messages_of_the_day_and_a_deleted_one_does_not_lower_it() {
    if day_just_started() {
        return;
    }
    let today = day();
    let h = Hub::new(Scenario {
        body: BY_HAND,
        may_merge: true,
        ci_green: true,
    });
    h.set_authority_extra(VERIFY);
    let count = |h: &Hub| -> String {
        let o = tick(h);
        let lines = count_line(&o);
        assert_eq!(lines.len(), 1, "{}", o.all());
        lines[0].to_string()
    };
    let line = |n: u32, b: u32| format!("unattended: mode verify; verdicts {n}/{b} today");
    assert_eq!(count(&h), line(0, 10));

    ticket(&h, 6, "In Progress", true);
    assign(&h, 6);
    let d = done(&h, 6, "");
    // not counted: an attended verify result, a stamped verify result of yesterday, a stamped
    // report of the dev agent, and a comment of the owner with the line
    verdict(&h, 6, d, "", 9);
    verdict(&h, 6, d, &stamp(9), 2 * 86_400);
    dev(
        &h,
        6,
        &h.coord.name,
        Kind::Report,
        &format!("verify: pass\n{}", stamp(8)),
    );
    h.board.owner_comment_now(6, "unattended: 7/10");
    assert_eq!(count(&h), line(0, 10));
    // counted: two stamped verify results of the coordinator
    let first = verdict(&h, 6, d, &stamp(1), 8);
    verdict(&h, 6, d, &stamp(2), 7);
    assert_eq!(count(&h), line(2, 10));
    // the budget is the key of AUTHORITY.md
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 5\n");
    assert_eq!(count(&h), line(2, 5));
    // a deleted verdict does not lower the count: the `n` of the one that stayed is a floor
    h.board.delete_comment(first);
    assert_eq!(count(&h), line(2, 5));
    // a stamped message that is no verdict raises the floor, not the list of verdicts
    coord(
        &h,
        6,
        &h.dev.name,
        Kind::Answer,
        &format!("Use the test key.\n{}", unattended_line(4, 5)),
    );
    assert_eq!(count(&h), line(4, 5));
    assert!(day() == today, "the UTC day changed while the test ran");
    assert!(h.writes().is_empty());
}

#[test]
fn the_floor_of_the_state_cache_holds_the_count_up_for_its_day_only() {
    use crewboard::clock::format_ts;
    use crewboard::config::ConfigDir;
    use crewboard::protocol::names::AgentName;
    use crewboard::state::StateCache;
    if day_just_started() {
        return;
    }
    let h = Hub::new(Scenario {
        body: BY_HAND,
        may_merge: true,
        ci_green: true,
    });
    h.set_authority_extra(VERIFY);
    let name = AgentName::parse(&h.coord.name).unwrap();
    let path = ConfigDir(h.headless_dir.path().to_path_buf()).state_path("fake", &name);
    let set_floor = |day: i64, n: u32| {
        let mut c = StateCache::load(&path);
        let text = format_ts(day * 86_400);
        c.tick.unattended_floor = Some((text.split('T').next().unwrap().to_string(), n));
        c.save(&path).unwrap();
    };
    let count = |h: &Hub| count_line(&tick(h)).join("\n");
    assert_eq!(count(&h), "unattended: mode verify; verdicts 0/10 today");
    // no stamped message is on the board (all were deleted): the floor of today holds
    set_floor(day(), 3);
    assert_eq!(count(&h), "unattended: mode verify; verdicts 3/10 today");
    // the tick keeps the floor in the state
    assert_eq!(
        StateCache::load(&path).tick.unattended_floor.map(|f| f.1),
        Some(3)
    );
    // a floor of another day is none
    set_floor(day() - 1, 9);
    assert_eq!(count(&h), "unattended: mode verify; verdicts 0/10 today");
}

#[test]
fn a_feed_that_cannot_be_read_lists_no_ticket() {
    let h = board();
    tick(&h);
    h.set_authority_extra(VERIFY);
    // control: the list is there
    assert_eq!(needs(&tick(&h)).len(), 4);
    h.fail_feed(404);
    let (o, reqs) = tick_requests(&h);
    assert!(needs(&o).is_empty(), "{}", o.all());
    let lines = count_line(&o);
    assert_eq!(lines.len(), 1, "{}", o.all());
    assert!(
        lines[0].starts_with(
            "unattended: mode verify; the verdicts of today could not be read, so no ticket is listed: "
        ),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("verdicts 0/"), "{}", o.all());
    // the tick did not read the tickets of the list either
    assert!(
        !reqs.iter().any(|r| r.contains("/issues/6/comments")),
        "{reqs:#?}"
    );
    // the rest of the tick is as it was
    assert!(
        o.stdout
            .starts_with(&format!("{}{}{}", left(TICKET), left(9), left(10))),
        "{}",
        o.all()
    );
}

#[test]
fn a_bad_value_of_the_key_lists_nothing_and_the_commands_refuse_the_file() {
    let h = board();
    h.set_authority_extra("unattended_coordinator: yes\n");
    let o = tick(&h);
    assert!(needs(&o).is_empty(), "{}", o.all());
    assert!(count_line(&o).is_empty(), "{}", o.all());
    // a command that enforces AUTHORITY.md refuses the file
    let v = h.headless(&["verify", &TICKET.to_string()]);
    assert_eq!(v.code, 2, "{}", v.all());
    assert!(
        v.stderr.contains("[rule=authority-bad-front-matter]"),
        "{}",
        v.all()
    );
    assert!(v.stderr.contains("unattended_coordinator"), "{}", v.all());
}

#[test]
fn a_board_item_without_a_ticket_gets_a_line_and_one_scan() {
    let h = board();
    h.set_authority_extra(VERIFY);
    tick(&h);
    // a person put an issue on the board by hand: no signed info comment names the item
    h.board
        .item("PVTI_by_hand", &[("Status", "Ready"), ("Gate", "x")]);
    h.board.item(
        "PVTI_backlog_by_hand",
        &[("Status", "Backlog"), ("Gate", "x")],
    );
    let line = "left for the interactive coordinator: PVTI_by_hand, a board item in Ready has no signed info comment with its ticket\n";
    let scans = |reqs: &[String]| reqs.iter().filter(|r| r.contains("/issues?")).count();
    let (o, reqs) = tick_requests(&h);
    assert!(o.stdout.contains(line), "{}", o.all());
    assert!(!o.stdout.contains("PVTI_backlog_by_hand"), "{}", o.all());
    assert_eq!(needs(&o).len(), 4, "{}", o.all());
    assert_eq!(scans(&reqs), 1, "{reqs:#?}");
    // the same unknown items: the line stays, and the tick does not scan again
    let (o, reqs) = tick_requests(&h);
    assert!(o.stdout.contains(line), "{}", o.all());
    assert_eq!(scans(&reqs), 0, "{reqs:#?}");
    // with the key off the items are nothing to the tick
    h.set_authority_extra("");
    let (o, reqs) = tick_requests(&h);
    assert!(!o.stdout.contains("PVTI_by_hand"), "{}", o.all());
    assert_eq!(scans(&reqs), 0, "{reqs:#?}");
}
