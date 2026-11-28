//! The judgment list and the verdict count of the unattended coordinator in the output of
//! `tick` (spec/agents#unattended-coordinator), against the fake GitHub.
//!
//! With `unattended_coordinator` absent or `off` the tick prints and reads what it does without
//! the key. With `verify` it lists the tickets that need a judgment and counts the verdicts of
//! the day from the signed messages. The tick writes nothing to GitHub for the list.
//!
//! Every message of these tests is created "now", and a test that counts the verdicts of the
//! day first waits when the UTC day is about to end (`settle_day`). So the day of a message and
//! the day of the tick are the same in every run.

use crate::support::board::{Agent, Out, REPO};
use crate::support::headless::{Hub, PR, SHA, Scenario, TICKET};
use crewboard::clock::now_epoch;
use crewboard::config::ConfigDir;
use crewboard::protocol::acts::{reassigned_body, round_limit_question, unattended_line};
use crewboard::protocol::names::{AgentName, Kind};
use crewboard::rules::Role;
use crewboard::state::StateCache;
use std::path::PathBuf;

pub(crate) const BY_HAND: &str = "Goal: fix it\nGate: the demo runs\n";
pub(crate) const GATE: &str = "the demo runs";
const NEEDS: &str = "needs judgment: ";
pub(crate) const VERIFY: &str = "unattended_coordinator: verify\n";
/// The start of the line of the tick with the mode and the verdicts.
const COUNT: &str = "unattended coordinator: mode ";
/// The second dev agent of these tests.
pub(crate) const OTHER: &str = "codex@fake-c";

pub(crate) fn key(n: u64) -> String {
    format!("{REPO}#{n}")
}

pub(crate) fn item_of(n: u64) -> String {
    format!("PVTI_issue_{n}")
}

/// One more ticket: the issue `n`, its board item and the signed info comment that names it.
pub(crate) fn ticket(h: &Hub, n: u64, status: &str, agent: bool) {
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
pub(crate) fn coord(h: &Hub, n: u64, to: &str, kind: Kind, body: &str) -> u64 {
    h.board
        .signed_now(&h.coord.key, &h.coord.name, to, kind, n, body)
        .0
}

/// A message of the dev agent on the ticket `n`. Returns the comment id.
pub(crate) fn dev(h: &Hub, n: u64, to: &str, kind: Kind, body: &str) -> u64 {
    h.board
        .signed_now(&h.dev.key, &h.dev.name, to, kind, n, body)
        .0
}

pub(crate) fn assign(h: &Hub, n: u64) {
    let body = format!(
        "Assigned to you: Fix it\nticket: {}\nitem={}\ngate: {GATE}\n",
        key(n),
        item_of(n)
    );
    coord(h, n, &h.dev.name, Kind::Request, &body);
}

/// A `done` of the dev agent with the evidence of the fake PR, and `extra` lines after it.
pub(crate) fn done(h: &Hub, n: u64, extra: &str) -> u64 {
    let body = format!(
        "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\n{extra}"
    );
    dev(h, n, &h.coord.name, Kind::Done, &body)
}

/// A message of the agent `a` on the ticket `n`.
pub(crate) fn by(h: &Hub, a: &Agent, n: u64, to: &str, kind: Kind, body: &str) -> u64 {
    h.board.signed_now(&a.key, &a.name, to, kind, n, body).0
}

/// A verify result of the coordinator for the `done` in the comment `done`, with `extra` lines
/// after it, created now. Returns the comment id.
pub(crate) fn verdict(h: &Hub, n: u64, done: u64, extra: &str) -> u64 {
    verdict_ago(h, n, done, extra, 0)
}

/// [`verdict`], created `ago` seconds ago.
pub(crate) fn verdict_ago(h: &Hub, n: u64, done: u64, extra: &str, ago: i64) -> u64 {
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

pub(crate) fn stamp(n: u32) -> String {
    format!("{}\n", unattended_line(n, 10))
}

pub(crate) fn hub() -> Hub {
    Hub::new(Scenario {
        body: BY_HAND,
        may_merge: true,
        ci_green: true,
    })
}

/// The board of these tests. The numbers are the issues.
///
/// - 5: Review, a Gate in prose, CI evidence, no verify (the ticket of the fake): on the list.
/// - 6: Blocked by its agent: on the list.
/// - 7: Blocked, a question to the owner is open: not on the list.
/// - 8: Blocked, stopped at the round limit: not on the list.
/// - 9: Review, the `done` names a protected path: not on the list.
/// - 10: Review, two unattended verdicts and a new `done`: not on the list.
/// - 11: Ready without an agent, never assigned; the owner moved it to Ready: on the list.
/// - 12: Backlog, returned by a reassign: on the list.
/// - 13: Backlog, never assigned: not on the list.
/// - 14: Ready with an agent: not on the list.
/// - 15: stopped at the round limit; then somebody set Ready and cleared the Agent on the
///   board: not on the list.
/// - 16: assigned in the signed log; the board says Ready without an Agent: not on the list.
/// - 17: Blocked, assigned to the dev agent, whose question to the owner is open. Another dev
///   agent wrote its name into the board Agent and posted a `blocked`: not on the list.
/// - 18: Review, assigned to the dev agent, whose `done` names a protected path. The other dev
///   agent wrote its name into the board Agent and posted its own `done`: not on the list.
/// - 19: Review like 5, and the issue has the label `needs-owner`: not on the list.
fn board() -> Hub {
    let h = hub();
    let other = h.board.agent(OTHER, Role::Dev);
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
    verdict(&h, 10, d, &stamp(1));
    let d = done(&h, 10, "");
    verdict(&h, 10, d, &stamp(2));
    done(&h, 10, "");

    ticket(&h, 11, "Ready", false);
    // the owner moved it to Ready: the signed log allows the Status
    h.board.owner_comment(11, "/crew move Ready");

    ticket(&h, 12, "Backlog", false);
    assign(&h, 12);
    let back = reassigned_body("the agent is dead");
    coord(&h, 12, &h.dev.name, Kind::Info, &back);

    ticket(&h, 13, "Backlog", false);
    ticket(&h, 14, "Ready", true);
    assign(&h, 14);

    ticket(&h, 15, "Ready", false);
    assign(&h, 15);
    let d = done(&h, 15, "");
    verdict(&h, 15, d, "case: round limit 1 of 1; the owner decides\n");

    ticket(&h, 16, "Ready", false);
    assign(&h, 16);

    ticket(&h, 17, "Blocked", true);
    assign(&h, 17);
    dev(
        &h,
        17,
        "owner",
        Kind::Question,
        "May I delete the old data?",
    );
    h.board.set_item(&item_of(17), "Agent", OTHER);
    by(&h, &other, 17, "coordinator", Kind::Blocked, "I am stuck");

    ticket(&h, 18, "Review", true);
    assign(&h, 18);
    done(&h, 18, "protected-paths: .github/workflows/ci.yml\n");
    h.board.set_item(&item_of(18), "Agent", OTHER);
    let own = format!(
        "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\n"
    );
    by(&h, &other, 18, &h.coord.name, Kind::Done, &own);

    h.board.issue(19, &["crew:work", "needs-owner"]);
    h.board.item(
        &item_of(19),
        &[
            ("Status", "Review"),
            ("Gate", "from the body"),
            ("Agent", h.dev.name.as_str()),
        ],
    );
    coord(
        &h,
        19,
        "owner",
        Kind::Info,
        &format!(
            "Ticket is on the board.\nitem={}\nproject=15\n",
            item_of(19)
        ),
    );
    assign(&h, 19);
    done(&h, 19, "");
    h
}

/// The Review tickets of [`board`], in the order of the tick.
const REVIEW: [u64; 5] = [TICKET, 9, 10, 18, 19];

/// The tickets of [`board`] that are on the list, with their reasons.
fn listed() -> Vec<String> {
    vec![
        format!("{}, prose Gate", key(TICKET)),
        format!("{}, blocked", key(6)),
        format!("{}, ready, no agent", key(11)),
        format!("{}, returned", key(12)),
    ]
}

pub(crate) fn tick(h: &Hub) -> Out {
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

/// How many requests of `reqs` read the comments of the issue `n`.
fn comment_reads(reqs: &[String], n: u64) -> usize {
    reqs.iter()
        .filter(|r| r.contains(&format!("/issues/{n}/comments")))
        .count()
}

fn needs(o: &Out) -> Vec<&str> {
    o.stdout
        .lines()
        .filter_map(|l| l.strip_prefix(NEEDS))
        .collect()
}

/// The lines of the tick about the unattended coordinator.
pub(crate) fn coord_lines(o: &Out) -> Vec<&str> {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("unattended"))
        .collect()
}

pub(crate) fn count_text(mode: &str, n: u32, b: u32) -> String {
    format!("{COUNT}{mode}; verdicts {n}/{b} today")
}

/// The line that the tick prints for a Review ticket with the Gate of these tests.
fn left(n: u64) -> String {
    format!(
        "left for the interactive coordinator: {}, the Gate is not CI only (signed Gate: '{GATE}'; a CI-only ticket has the Gate `ci`: `crewboard ticket new --gate-ci` writes it)\n",
        key(n)
    )
}

/// The output of a tick on [`board`] without the key, byte for byte.
fn lefts() -> String {
    REVIEW.iter().map(|n| left(*n)).collect()
}

/// Wait when the UTC day ends within the next 30 seconds, so the messages and the ticks of one
/// test are of one day.
pub(crate) fn settle_day() {
    let left = 86_400 - now_epoch().rem_euclid(86_400);
    if left <= 30 {
        std::thread::sleep(std::time::Duration::from_secs(left as u64 + 1));
    }
}

/// The state file of the headless config dir.
pub(crate) fn state_path(h: &Hub) -> PathBuf {
    let name = AgentName::parse(&h.coord.name).unwrap();
    ConfigDir(h.headless_dir.path().to_path_buf()).state_path("fake", &name)
}

#[test]
fn with_the_key_absent_or_off_the_tick_prints_and_reads_what_it_did() {
    let expected = lefts();
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
        for n in [6, 7, 8, 11, 12, 13, 14, 15, 16, 17] {
            assert_eq!(comment_reads(&second_reqs, n), 0, "{extra:?} #{n}");
        }
        assert!(h.writes().is_empty(), "{extra:?}: {:?}", h.writes().len());
        assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
        // the state file holds nothing of the unattended coordinator
        let state = std::fs::read_to_string(state_path(&h)).unwrap();
        assert!(state.contains("\"tickets\""), "{state}");
        assert!(!state.contains("unattended"), "{extra:?}: {state}");
        runs.push((first_reqs, second_reqs));
    }
    // the same requests, in the same order, with the key absent, off, and off with a budget
    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run, &runs[0],
            "run {i} differs from the run without the key"
        );
    }
    // control: the comparison can fail, because the key `verify` adds requests, lines and state
    let h = board();
    h.set_authority_extra(VERIFY);
    let (first, first_reqs) = tick_requests(&h);
    assert_ne!(first.stdout, expected);
    assert!(first.stdout.starts_with(&expected), "{}", first.all());
    assert_ne!(first_reqs, runs[0].0);
    let state = std::fs::read_to_string(state_path(&h)).unwrap();
    assert!(state.contains("unattended_feed"), "{state}");
}

#[test]
fn an_authority_file_that_cannot_be_read_is_the_mode_off() {
    let h = board();
    h.set_authority_extra(VERIFY);
    // control: the key is on
    assert_eq!(needs(&tick(&h)), listed());
    h.fail_get("/contents/AUTHORITY.md", 500);
    let (o, reqs) = tick_requests(&h);
    assert_eq!(o.stdout, lefts(), "{}", o.all());
    assert!(
        !reqs.iter().any(|r| r.contains("/issues/comments?")),
        "{reqs:#?}"
    );
    assert_eq!(comment_reads(&reqs, 6), 0, "{reqs:#?}");
    // the file is back, and so is the list
    h.fail_nothing();
    assert_eq!(needs(&tick(&h)), listed());
}

#[test]
fn with_the_key_verify_the_tick_lists_the_tickets_that_need_a_judgment() {
    settle_day();
    let h = board();
    h.set_authority_extra(VERIFY);
    let o = tick(&h);
    assert_eq!(needs(&o), listed(), "{}", o.all());
    // the lines of the tick stay, the list comes after them, and the count is the last line:
    // the two stamped verdicts of #10
    let end = format!(
        "{}{}\n",
        listed()
            .iter()
            .map(|l| format!("{NEEDS}{l}\n"))
            .collect::<String>(),
        count_text("verify", 2, 10)
    );
    assert_eq!(o.stdout, format!("{}{end}", lefts()), "{}", o.all());
    // the list is a view: the tick wrote nothing to GitHub
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
    assert_eq!(
        coord_lines(&o),
        [count_text("verify-and-merge", 2, 4)],
        "{}",
        o.all()
    );
    assert!(!h.merged());
    assert!(h.writes().is_empty(), "{:?}", h.writes().len());
}

#[test]
fn a_board_edit_does_not_put_a_stopped_or_an_assigned_ticket_on_the_list() {
    let h = board();
    h.set_authority_extra(VERIFY);
    // the first tick fills the ticket index of the state
    tick(&h);
    let (o, reqs) = tick_requests(&h);
    let got = needs(&o).join("\n");
    // 15: stopped at the round limit, set to Ready without an Agent by hand.
    // 16: Ready without an Agent on the board, assigned in the signed log.
    // 17: the board Agent is a dev agent that the signed log does not name; the question of the
    //     signed assignee to the owner is open.
    // 18: the same in Review, with a `done` of that dev agent.
    // 19: the label `needs-owner`.
    for n in [15, 16, 17, 18, 19] {
        assert!(!got.contains(&key(n)), "#{n} is on the list: {}", o.all());
    }
    // the tick read the signed log of each of them to know that
    for n in [15, 16, 17] {
        assert_eq!(comment_reads(&reqs, n), 1, "#{n}: {reqs:#?}");
    }
    // control: each of them is on the list when the board agrees with the signed log and the
    // owner has answered
    h.board.owner_comment_now(15, "Go on, with another agent.");
    coord(&h, 16, &h.dev.name, Kind::Info, &reassigned_body("late"));
    h.board.set_item(&item_of(17), "Agent", &h.dev.name);
    h.board.owner_comment_now(17, "Yes, delete it.");
    dev(
        &h,
        17,
        "coordinator",
        Kind::Blocked,
        "the API key is missing",
    );
    let o = tick(&h);
    let got = needs(&o).join("\n");
    // 15 still has its signed assignee, so it is no free Ready ticket
    assert!(!got.contains(&key(15)), "{}", o.all());
    assert!(
        got.contains(&format!("{}, ready, no agent", key(16))),
        "{}",
        o.all()
    );
    assert!(
        got.contains(&format!("{}, blocked", key(17))),
        "{}",
        o.all()
    );
}

#[test]
fn the_cost_of_the_key_verify_per_tick() {
    // 3 Blocked tickets, 5 Backlog tickets without an agent that have a Gate, 1 Ready ticket
    // without an agent, and the Review ticket of the fake
    let h = hub();
    for n in [6, 7, 8] {
        ticket(&h, n, "Blocked", true);
        assign(&h, n);
        dev(
            &h,
            n,
            "coordinator",
            Kind::Blocked,
            "the API key is missing",
        );
    }
    for n in [9, 10, 11, 12, 13] {
        ticket(&h, n, "Backlog", false);
    }
    ticket(&h, 14, "Ready", false);
    // the owner moved it to Ready: the signed log allows the Status
    h.board.owner_comment(14, "/crew move Ready");
    tick(&h);
    let (_, off) = tick_requests(&h);
    h.set_authority_extra(VERIFY);

    // the first tick with the key: the feed of the day, and the comments of every candidate
    let from = h.requests().len();
    let (o, first) = tick_requests(&h);
    assert_eq!(needs(&o).len(), 5, "{}", o.all());
    let extra: Vec<&String> = first.iter().filter(|r| !off.contains(r)).collect();
    assert_eq!(first.len(), off.len() + 10, "{extra:#?}");
    assert_eq!(extra.len(), 10, "{extra:#?}");
    assert!(extra[0].contains("/issues/comments?since="), "{extra:#?}");
    for n in 6..=14 {
        assert_eq!(comment_reads(&first, n), 1, "#{n}: {extra:#?}");
    }
    let feed = |reqs: &[crate::support::Req]| -> Vec<Option<String>> {
        reqs.iter()
            .filter(|r| r.path.contains("/issues/comments?since="))
            .map(|r| r.if_none_match.clone())
            .collect()
    };
    assert_eq!(feed(&h.requests()[from..]), [None]);

    // the second tick, nothing changed: the feed is a conditional request, and the Backlog
    // tickets that no reassign returned are not read again
    let from = h.requests().len();
    let (o, second) = tick_requests(&h);
    assert_eq!(needs(&o).len(), 5, "{}", o.all());
    assert_eq!(second.len(), off.len() + 5, "{second:#?}");
    let asked = feed(&h.requests()[from..]);
    assert_eq!(asked.len(), 1);
    assert!(asked[0].is_some(), "the feed read names no ETag");
    for n in [6, 7, 8, 14] {
        assert_eq!(comment_reads(&second, n), 1, "#{n}");
    }
    for n in [9, 10, 11, 12, 13] {
        assert_eq!(comment_reads(&second, n), 0, "#{n}");
    }

    // a Backlog item that changed is read again, once
    h.board.touch_item(&item_of(9));
    let (_, third) = tick_requests(&h);
    assert_eq!(comment_reads(&third, 9), 1);
    assert_eq!(comment_reads(&third, 10), 0);
    let (_, fourth) = tick_requests(&h);
    assert_eq!(comment_reads(&fourth, 9), 0);

    // a ticket that a reassign returned stays on the list, so it is read at every tick
    assign(&h, 10);
    coord(&h, 10, &h.dev.name, Kind::Info, &reassigned_body("late"));
    h.board.touch_item(&item_of(10));
    for _ in 0..2 {
        let (o, reqs) = tick_requests(&h);
        assert_eq!(comment_reads(&reqs, 10), 1);
        assert!(
            needs(&o).contains(&format!("{}, returned", key(10)).as_str()),
            "{}",
            o.all()
        );
    }
    assert!(h.writes().is_empty());
}

#[test]
fn the_count_comes_from_the_signed_messages_and_two_floors_hold_it_after_a_deletion() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let count = |h: &Hub| -> String {
        let o = tick(h);
        let lines = coord_lines(&o);
        assert_eq!(lines.len(), 1, "{}", o.all());
        lines[0].to_string()
    };
    assert_eq!(count(&h), count_text("verify", 0, 10));

    ticket(&h, 6, "In Progress", true);
    assign(&h, 6);
    let d = done(&h, 6, "");
    // not counted: an attended verify result, a stamped verify result of two days ago, a stamped
    // report of the dev agent, and a comment of the owner with the line
    verdict(&h, 6, d, "");
    verdict_ago(&h, 6, d, &stamp(9), 2 * 86_400);
    dev(
        &h,
        6,
        &h.coord.name,
        Kind::Report,
        &format!("verify: pass\n{}", stamp(8)),
    );
    h.board.owner_comment_now(6, "unattended: 7/10");
    assert_eq!(count(&h), count_text("verify", 0, 10));
    // counted: two stamped verify results of the coordinator
    let first = verdict(&h, 6, d, &stamp(1));
    let second = verdict(&h, 6, d, &stamp(2));
    assert_eq!(count(&h), count_text("verify", 2, 10));
    // the budget is the key of AUTHORITY.md
    h.set_authority_extra("unattended_coordinator: verify\nunattended_budget: 5\n");
    assert_eq!(count(&h), count_text("verify", 2, 5));

    // The first verdict is deleted. On a host without the state, the signed log alone gives
    // one verdict, and the `n` of the verdict that stayed holds the count at 2.
    h.board.delete_comment(first);
    std::fs::remove_file(state_path(&h)).unwrap();
    assert_eq!(count(&h), count_text("verify", 2, 5));
    // The latest verdict is deleted too. No message holds the count now: only the state of this
    // host does, which saw 2.
    h.board.delete_comment(second);
    assert_eq!(count(&h), count_text("verify", 2, 5));
    // A host without that state reads what the log has left: nothing. The signed log does not
    // protect the count against the deletion of the latest stamped message.
    std::fs::remove_file(state_path(&h)).unwrap();
    assert_eq!(count(&h), count_text("verify", 0, 5));

    // a stamped message that is no verdict raises the count through its `n`
    coord(
        &h,
        6,
        &h.dev.name,
        Kind::Answer,
        &format!("Use the test key.\n{}", unattended_line(4, 5)),
    );
    assert_eq!(count(&h), count_text("verify", 4, 5));
    assert!(h.writes().is_empty());
}

#[test]
fn an_edited_stamped_verdict_does_not_lower_the_count_of_a_host_that_saw_it() {
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    ticket(&h, 6, "In Progress", true);
    assign(&h, 6);
    let d = done(&h, 6, "");
    verdict(&h, 6, d, &stamp(1));
    let latest = verdict(&h, 6, d, &stamp(2));
    assert_eq!(coord_lines(&tick(&h)), [count_text("verify", 2, 10)]);
    // an edit breaks the signature: the message is no verified message any more
    let body = h.board.comment_body(latest);
    h.board
        .edit_comment(latest, &body.replace("unattended: 2/10", "note: none"));
    assert_eq!(coord_lines(&tick(&h)), [count_text("verify", 2, 10)]);
    // a host without the state counts the one verdict that is left
    std::fs::remove_file(state_path(&h)).unwrap();
    assert_eq!(coord_lines(&tick(&h)), [count_text("verify", 1, 10)]);
}

#[test]
fn the_floor_of_the_state_cache_holds_the_count_up_for_its_day_only() {
    use crewboard::clock::format_ts;
    settle_day();
    let h = hub();
    h.set_authority_extra(VERIFY);
    let path = state_path(&h);
    let day = now_epoch().div_euclid(86_400);
    let set_floor = |day: i64, n: u32| {
        let mut c = StateCache::load(&path);
        let text = format_ts(day * 86_400);
        c.tick.unattended_floor = Some((text.split('T').next().unwrap().to_string(), n));
        c.save(&path).unwrap();
    };
    let count = |h: &Hub| coord_lines(&tick(h)).join("\n");
    assert_eq!(count(&h), count_text("verify", 0, 10));
    // no stamped message is on the board: the floor of today holds
    set_floor(day, 3);
    assert_eq!(count(&h), count_text("verify", 3, 10));
    // the tick keeps the floor in the state
    assert_eq!(
        StateCache::load(&path).tick.unattended_floor.map(|f| f.1),
        Some(3)
    );
    // a floor of another day is none
    set_floor(day - 1, 9);
    assert_eq!(count(&h), count_text("verify", 0, 10));
}

#[test]
fn a_feed_that_cannot_be_read_lists_no_ticket() {
    let h = board();
    tick(&h);
    h.set_authority_extra(VERIFY);
    // control: the list is there
    assert_eq!(needs(&tick(&h)), listed());
    h.fail_get("/issues/comments?", 404);
    let (o, reqs) = tick_requests(&h);
    assert_eq!(o.code, 0);
    assert!(needs(&o).is_empty(), "{}", o.all());
    let lines = coord_lines(&o);
    assert_eq!(lines.len(), 1, "{}", o.all());
    assert!(
        lines[0].starts_with(
            "unattended coordinator: mode verify; the verdicts of today could not be read, so no ticket is listed: "
        ),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("verdicts 0/"), "{}", o.all());
    // the tick did not read the tickets of the list either
    assert_eq!(comment_reads(&reqs, 6), 0, "{reqs:#?}");
    // the rest of the tick is as it was
    assert!(o.stdout.starts_with(&lefts()), "{}", o.all());
}

#[test]
fn a_ticket_that_cannot_be_read_for_the_list_is_one_line_and_the_tick_goes_on() {
    settle_day();
    let h = board();
    h.set_authority_extra(VERIFY);
    tick(&h);
    h.fail_get("/issues/6/comments", 404);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line = format!(
        "unattended coordinator: {} could not be read for the list: ",
        key(6)
    );
    assert_eq!(
        o.stdout.lines().filter(|l| l.starts_with(&line)).count(),
        1,
        "{}",
        o.all()
    );
    // the other tickets are on the list, and the tick prints its other lines and the count
    assert_eq!(
        needs(&o),
        [
            format!("{}, prose Gate", key(TICKET)),
            format!("{}, ready, no agent", key(11)),
            format!("{}, returned", key(12)),
        ],
        "{}",
        o.all()
    );
    assert!(o.stdout.starts_with(&lefts()), "{}", o.all());
    assert!(
        o.stdout
            .ends_with(&format!("{}\n", count_text("verify", 2, 10))),
        "{}",
        o.all()
    );
    // the read works again: the ticket is back on the list
    h.fail_nothing();
    assert_eq!(needs(&tick(&h)), listed());
}

#[test]
fn a_bad_value_of_the_key_lists_nothing_and_the_commands_refuse_the_file() {
    let h = board();
    h.set_authority_extra("unattended_coordinator: yes\n");
    let o = tick(&h);
    assert_eq!(o.stdout, lefts(), "{}", o.all());
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
    // the scan fails: one line, the tick goes on, and the next tick scans again
    h.fail_get("/issues?", 404);
    let (o, reqs) = tick_requests(&h);
    assert_eq!(scans(&reqs), 1, "{reqs:#?}");
    assert!(
        o.stdout.contains(
            "unattended coordinator: the tickets of new board items could not be read for the list: "
        ),
        "{}",
        o.all()
    );
    assert_eq!(needs(&o), listed(), "{}", o.all());
    h.fail_nothing();
    let (o, reqs) = tick_requests(&h);
    assert!(o.stdout.contains(line), "{}", o.all());
    assert!(!o.stdout.contains("PVTI_backlog_by_hand"), "{}", o.all());
    assert!(!o.stdout.contains("could not be read"), "{}", o.all());
    assert_eq!(needs(&o), listed(), "{}", o.all());
    assert_eq!(scans(&reqs), 1, "{reqs:#?}");
    // the same unknown items: the line stays, and the tick does not scan again
    let (o, reqs) = tick_requests(&h);
    assert!(o.stdout.contains(line), "{}", o.all());
    assert_eq!(scans(&reqs), 0, "{reqs:#?}");
    // one unknown item less is no reason for a scan
    h.board.delete_item("PVTI_backlog_by_hand");
    let (_, reqs) = tick_requests(&h);
    assert_eq!(scans(&reqs), 0, "{reqs:#?}");
    // a new unknown item is one
    h.board
        .item("PVTI_by_hand_2", &[("Status", "Ready"), ("Gate", "x")]);
    let (_, reqs) = tick_requests(&h);
    assert_eq!(scans(&reqs), 1, "{reqs:#?}");
    // with the key off the items are nothing to the tick
    h.set_authority_extra("");
    let (o, reqs) = tick_requests(&h);
    assert!(!o.stdout.contains("PVTI_by_hand"), "{}", o.all());
    assert_eq!(scans(&reqs), 0, "{reqs:#?}");
}
