//! The round limit `max_rounds` of AUTHORITY.md (spec/tickets#round-limit), against the fake
//! GitHub of `tests/support/board.rs`.
//!
//! The failed verify that spends the last round moves the ticket to Blocked, with a question to
//! the owner and the label `needs-owner`. An owner comment or the coordinator's `unblock`
//! releases the ticket, and the count starts again. The headless `tick` does the same. Without
//! the key nothing changes.
//!
//! A stopped ticket does not go back to its agent without a release: `take` and `done` are
//! refused, also after a park or a split before the stop. A `verify` after a failed write
//! finishes the stop and posts no second result.

use crate::support::authority::{NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Out};
use crate::support::cli::{GATE, crew, item_in, run_ok, run_refused};
use crate::support::headless::{Hub, Scenario, TICKET};

const NOTE: &str = "Gate part 2 (the migration test) is missing";
const QUESTION: &str = "round limit: ";

/// A crew whose AUTHORITY.md has the front matter lines `extra`.
fn crew_with(extra: &str) -> (FakeBoard, Agent, Agent) {
    let (fake, c, b) = crew();
    fake.file(
        "AUTHORITY.md",
        &authority_with("fake-owner", NEEDS_OWNER_ALL, extra),
    );
    (fake, c, b)
}

fn status(fake: &FakeBoard, item: &str) -> Option<String> {
    fake.item_field(item, "Status")
}

/// A ticket in Review: its number, its board item and the arguments of its `done`.
struct Delivered {
    t: u64,
    ts: String,
    item: String,
    pr_url: String,
    head: String,
    pr: u64,
}

/// A ticket that declares `CI: none` (so its evidence holds), assigned to `b` with `order`.
/// Returns its number and its board item.
fn assigned(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str, order: &str) -> (u64, String) {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(title, &body, &["crew:work"]);
    let o = run_ok(
        fake,
        c,
        &["assign", &t.to_string(), "--to", &b.name, "--order", order],
    );
    (t, item_in(&o))
}

/// The PR of a ticket that `b` has In Progress, and its first `done`.
fn open_pr_and_deliver(fake: &FakeBoard, b: &Agent, t: u64, item: String) -> Delivered {
    let pr = fake.pr_for(t, &b.name);
    let d = Delivered {
        t,
        ts: t.to_string(),
        item,
        pr_url: pr.url,
        head: pr.head,
        pr: pr.number,
    };
    deliver(fake, b, &d);
    d
}

/// Assign, take, PR and done.
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Delivered {
    let (t, item) = assigned(fake, c, b, title, "20");
    run_ok(fake, b, &["take", &t.to_string()]);
    open_pr_and_deliver(fake, b, t, item)
}

/// The verify results (`verify: fail`) on the ticket.
fn fail_reports(fake: &FakeBoard, t: u64) -> usize {
    fake.comments_of(t)
        .into_iter()
        .filter(|(_, _, b)| b.contains("verify: fail"))
        .count()
}

/// `take` and `done` of the assignee are refused with the text of the round limit, and nothing
/// was written.
fn assert_no_way_back(fake: &FakeBoard, b: &Agent, d: &Delivered) {
    let _ = fake.poll_json(b);
    let before = crate::support::cli::writes(fake);
    for args in [
        vec!["take", d.ts.as_str()],
        vec![
            "done",
            d.ts.as_str(),
            "--pr",
            d.pr_url.as_str(),
            "--sha",
            d.head.as_str(),
        ],
    ] {
        let o = run_refused(fake, b, &args, "status-answer-missing");
        assert!(
            o.stderr.contains("is stopped at the round limit"),
            "{args:?}: {}",
            o.all()
        );
    }
    assert_eq!(crate::support::cli::writes(fake), before);
    assert_eq!(status(fake, &d.item).as_deref(), Some("Blocked"));
}

/// The dev reads its messages, pushes a new head and delivers it. A new head per `done` is
/// what a repair is; and two `done` messages with the same text in the same second have the
/// same signature, so the second is dropped as a copy.
fn deliver(fake: &FakeBoard, b: &Agent, d: &Delivered) {
    static PUSH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let head = format!(
        "{:040x}",
        0xfeed_0000_u64 + u64::from(PUSH.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    );
    fake.set_pr_head(d.pr, &head);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &d.ts, "--pr", &d.pr_url, "--sha", &head]);
    assert_eq!(status(fake, &d.item).as_deref(), Some("Review"));
}

/// `verify --fail` of the coordinator.
fn fail(fake: &FakeBoard, c: &Agent, d: &Delivered) -> Out {
    let o = run_ok(fake, c, &["verify", &d.ts, "--fail", "--body", NOTE]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    o
}

/// The bodies of the round-limit questions on the ticket.
fn questions(fake: &FakeBoard, t: u64) -> Vec<String> {
    fake.comments_of(t)
        .into_iter()
        .map(|(_, _, body)| body)
        .filter(|b| b.contains("kind=question") && b.contains(QUESTION))
        .collect()
}

fn labelled(fake: &FakeBoard, t: u64) -> bool {
    fake.labels_of(t).contains(&"needs-owner".to_string())
}

/// The checks of a ticket that the round limit stopped with `rounds` failed verifies.
fn assert_stopped(fake: &FakeBoard, d: &Delivered, o: &Out, rounds: u32, limit: u32) {
    assert_eq!(
        status(fake, &d.item).as_deref(),
        Some("Blocked"),
        "{}",
        o.all()
    );
    assert!(
        o.stdout.contains(&format!(
            "Status moved to Blocked: the round limit stopped the ticket ({rounds} failed verifies, max_rounds {limit})"
        )),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains("The owner decides"), "{}", o.all());
    assert!(
        o.stdout.contains("round limit: question to the owner"),
        "{}",
        o.all()
    );
    assert!(labelled(fake, d.t), "{:?}", fake.labels_of(d.t));
    let q = questions(fake, d.t);
    let latest = q.last().expect("a round-limit question");
    assert!(latest.contains("to=owner"), "{latest}");
    assert!(
        latest.contains(&format!("round limit: {rounds} failed verifies\n")),
        "{latest}"
    );
    assert!(
        latest.contains(&format!("limit: max_rounds {limit} (AUTHORITY.md)")),
        "{latest}"
    );
    for option in ["- redirect:", "- split:", "- drop:"] {
        assert!(latest.contains(option), "{option} missing: {latest}");
    }
    // the question names the comment of the verify result
    let report = fake
        .comments_of(d.t)
        .into_iter()
        .rev()
        .find(|(_, _, b)| b.contains("verify: fail"))
        .expect("a verify report");
    assert!(
        report.2.contains(&format!(
            "case: round limit {rounds} of {limit}; the owner decides"
        )),
        "{}",
        report.2
    );
    assert!(
        latest.contains(&format!("issuecomment-{}", report.0)),
        "the question does not name report {}: {latest}",
        report.0
    );
}

#[test]
fn the_last_round_blocks_the_ticket_and_an_owner_comment_releases_it() {
    let (fake, c, b) = crew_with("max_rounds: 2\n");
    let d = delivered(&fake, &c, &b, "IT rounds owner");

    // round 1 of 2: back to the dev, as without a limit
    let o = fail(&fake, &c, &d);
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert!(questions(&fake, d.t).is_empty());
    assert!(!labelled(&fake, d.t));

    // round 2 of 2: Blocked, a question to the owner, the label
    deliver(&fake, &b, &d);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 2, 2);
    assert_eq!(questions(&fake, d.t).len(), 1);
    // the agent stays the assignee, and no Order was written
    assert_eq!(
        fake.item_field(&d.item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    // the board and the signed log agree: `status` reports no drift
    let s = run_ok(&fake, &c, &["status"]);
    assert!(!s.stdout.contains("the log shows"), "{}", s.all());

    // the dev cannot take the ticket back before the owner answers
    assert_no_way_back(&fake, &b, &d);
    // a second verify is refused: the ticket is not in Review
    run_refused(
        &fake,
        &c,
        &["verify", &d.ts, "--fail", "--body", NOTE],
        "verify-needs-review",
    );

    // the owner answers: the dev takes the ticket, and the count starts again
    fake.owner_comment_now(d.t, "Redirect: keep the old schema and add the test.");
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &d.ts]);
    assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));
    deliver(&fake, &b, &d);
    let o = fail(&fake, &c, &d);
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));
    assert_eq!(questions(&fake, d.t).len(), 1);

    // the release is no end of the limit: the second fail after it stops the ticket again
    deliver(&fake, &b, &d);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 2, 2);
    assert_eq!(questions(&fake, d.t).len(), 2);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_limit_of_one_stops_the_first_fail_and_unblock_releases_the_ticket() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let d = delivered(&fake, &c, &b, "IT rounds unblock");
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 1, 1);

    // the coordinator's signed `unblock` withdraws the question
    let _ = fake.poll_json(&c);
    let o = run_ok(&fake, &c, &["unblock", &d.ts, "--body", "Try once more."]);
    assert!(
        o.stdout
            .contains("the open question to the owner is withdrawn"),
        "{}",
        o.all()
    );
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &d.ts]);
    assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));

    // the count started again; with a limit of 1 the next fail stops the ticket again
    deliver(&fake, &b, &d);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 1, 1);
    assert_eq!(questions(&fake, d.t).len(), 2);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_head_moved_only_fail_at_the_limit_moves_as_without_a_limit() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let d = delivered(&fake, &c, &b, "IT rounds head");
    // somebody updates the branch: the head is not the commit of the evidence any more
    fake.set_pr_head(d.pr, "fedcba9876543210fedcba9876543210fedcba98");
    let o = run_ok(&fake, &c, &["verify", &d.ts]);
    assert!(
        o.stdout.contains("verify FAIL") && o.stdout.contains("PR head moved"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));
    assert!(questions(&fake, d.t).is_empty());
    assert!(!labelled(&fake, d.t));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn without_the_key_every_fail_goes_back_to_the_dev() {
    let (fake, c, b) = crew_with("");
    let d = delivered(&fake, &c, &b, "IT rounds off");
    for round in 0..3 {
        if round > 0 {
            deliver(&fake, &b, &d);
        }
        let o = fail(&fake, &c, &d);
        assert!(
            o.stdout.contains("Status moved back to In Progress"),
            "{}",
            o.all()
        );
        assert!(!o.stdout.contains("round limit"), "{}", o.all());
        assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));
    }
    assert!(questions(&fake, d.t).is_empty());
    assert!(!labelled(&fake, d.t));
    let reports: Vec<String> = fake
        .comments_of(d.t)
        .into_iter()
        .map(|(_, _, body)| body)
        .filter(|b| b.contains("verify: fail"))
        .collect();
    assert_eq!(reports.len(), 3);
    assert!(reports.iter().all(|r| !r.contains("case:")), "{reports:?}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn the_headless_tick_stops_a_ticket_at_the_limit() {
    // the CI of the head is red: the tick's verify fails, and that fail is a round
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: false,
    });
    h.set_authority_extra("max_rounds: 1\n");
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains(
            "Status moved to Blocked: the round limit stopped the ticket (1 failed verifies, max_rounds 1)"
        ),
        "{}",
        o.all()
    );
    assert_eq!(h.status_of_ticket().as_deref(), Some("Blocked"));
    assert!(!h.merged());
    assert!(
        labelled(&h.board, TICKET),
        "{:?}",
        h.board.labels_of(TICKET)
    );
    let q = questions(&h.board, TICKET);
    assert_eq!(q.len(), 1, "{q:?}");
    assert!(q[0].contains("to=owner"), "{}", q[0]);
    assert!(
        q[0].contains("round limit: 1 failed verifies\n"),
        "{}",
        q[0]
    );
    // the next tick has no ticket in Review and writes nothing for this one
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("tick: no ticket in Review"),
        "{}",
        o.all()
    );
    assert_eq!(questions(&h.board, TICKET).len(), 1);
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());

    // control: without the key the same tick sends the ticket back to its agent
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: false,
    });
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(h.status_of_ticket().as_deref(), Some("In Progress"));
    assert!(questions(&h.board, TICKET).is_empty());
    assert!(!labelled(&h.board, TICKET));
}

#[test]
fn a_ticket_that_was_parked_before_the_stop_is_not_resumed_without_a_release() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let (t, item) = assigned(&fake, &c, &b, "IT rounds parked", "20");
    let ts = t.to_string();
    run_ok(&fake, &b, &["take", &ts]);
    // an urgent ticket interrupts: the dev parks this one, then comes back to it
    let (urgent, urgent_item) = assigned(&fake, &c, &b, "IT rounds urgent", "5");
    let us = urgent.to_string();
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &us, "--interrupt", "--why", "urgent"]);
    assert_eq!(status(&fake, &item).as_deref(), Some("Blocked"));
    run_ok(&fake, &b, &["block", &us, "--why", "waits for a key"]);
    // a park needs no answer: the dev resumes its own park
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    let d = open_pr_and_deliver(&fake, &b, t, item);

    let _ = fake.poll_json(&c);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 1, 1);
    // the park message is still the latest `blocked` message of the ticket: it opens no way back
    assert_no_way_back(&fake, &b, &d);
    // `mine` shows the ticket as waiting for the owner, not as parked
    let m = fake.run(&b.dir, &["mine"]);
    assert!(
        m.stdout.contains("Blocked, waits for the owner"),
        "{}",
        m.all()
    );
    let line = m
        .stdout
        .lines()
        .find(|l| l.contains(&format!("#{t}")) && l.contains("wait"))
        .unwrap_or_else(|| panic!("no verdict line for #{t}: {}", m.all()));
    assert!(!line.contains("parked"), "{line}");

    // the owner answers: now the take passes
    fake.owner_comment_now(d.t, "Go on, but keep the old schema.");
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(status(&fake, &d.item).as_deref(), Some("In Progress"));
    assert_eq!(status(&fake, &urgent_item).as_deref(), Some("Blocked"));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_ticket_that_was_split_before_the_stop_takes_no_done_without_a_release() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let (t, item) = assigned(&fake, &c, &b, "IT rounds split", "20");
    let ts = t.to_string();
    run_ok(&fake, &b, &["take", &ts]);
    // the dev blocks, the coordinator splits the open part off
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["block", &ts, "--why", "part A is done, part B is open"],
    );
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("open-part.md");
    std::fs::write(&file, format!("Part B is open.\n\n{GATE}CI: none\n")).unwrap();
    let _ = fake.poll_json(&c);
    run_ok(
        &fake,
        &c,
        &[
            "ticket",
            "split",
            &ts,
            "--title",
            "IT rounds split, part B",
            "--body-file",
            file.to_str().unwrap(),
        ],
    );
    // the finished part goes from Blocked to Review without a take
    assert_eq!(status(&fake, &item).as_deref(), Some("Blocked"));
    let d = open_pr_and_deliver(&fake, &b, t, item);

    let _ = fake.poll_json(&c);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 1, 1);
    assert!(
        o.stdout
            .contains("For a split, release the ticket first, then run `ticket split`"),
        "{}",
        o.all()
    );
    // the split proof still holds, but the stop comes first
    assert_no_way_back(&fake, &b, &d);
    // a second split of the stopped ticket is refused: release first, then split
    let _ = fake.poll_json(&c);
    let before = crate::support::cli::writes(&fake);
    let o = run_refused(
        &fake,
        &c,
        &[
            "ticket",
            "split",
            &ts,
            "--title",
            "IT rounds split, part C",
            "--body-file",
            file.to_str().unwrap(),
        ],
        "status-answer-missing",
    );
    assert!(
        o.stderr.contains("release it first, then split"),
        "{}",
        o.all()
    );
    assert_eq!(crate::support::cli::writes(&fake), before);

    // after the release the `done` of the split ticket passes again
    let _ = fake.poll_json(&c);
    run_ok(&fake, &c, &["unblock", &d.ts, "--body", "Deliver part A."]);
    deliver(&fake, &b, &d);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// A stop whose Status write failed: the result, the question and the label are there, the
/// ticket is still in Review.
fn half_stopped(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Delivered {
    let d = delivered(fake, c, b, title);
    fake.fail_select_write(Some("Blocked"));
    let o = fake.run(&c.dir, &["verify", &d.ts, "--fail", "--body", NOTE]);
    assert_ne!(
        o.code,
        0,
        "verify must report the failed write: {}",
        o.all()
    );
    fake.fail_select_write(None);
    assert_eq!(status(fake, &d.item).as_deref(), Some("Review"));
    assert_eq!(fail_reports(fake, d.t), 1);
    assert_eq!(questions(fake, d.t).len(), 1);
    assert!(labelled(fake, d.t));
    d
}

#[test]
fn a_verify_after_a_failed_status_write_finishes_the_stop_without_a_second_result() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let d = half_stopped(&fake, &c, &b, "IT rounds rerun");
    // the dev cannot deliver a new head into the half-stopped ticket
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["done", &d.ts, "--pr", &d.pr_url, "--sha", &d.head],
        "status-answer-missing",
    );
    // a plain verify: the open question of the stop does not refuse it
    let o = run_ok(&fake, &c, &["verify", &d.ts]);
    assert!(
        o.stdout.contains("this run finishes the stop"),
        "{}",
        o.all()
    );
    assert!(
        o.stdout
            .contains("round limit: the question to the owner is open already"),
        "{}",
        o.all()
    );
    assert!(
        o.stdout.contains(
            "Status moved to Blocked: the round limit stopped the ticket (1 failed verifies, max_rounds 1)"
        ),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &d.item).as_deref(), Some("Blocked"));
    assert_eq!(fail_reports(&fake, d.t), 1);
    assert_eq!(questions(&fake, d.t).len(), 1);
    // nothing is left to finish: the next verify is refused as for any Blocked ticket
    run_refused(&fake, &c, &["verify", &d.ts], "verify-needs-review");

    // `verify --fail` finishes a stop too, and its note opens no second round
    let d = half_stopped(&fake, &c, &b, "IT rounds rerun fail");
    let o = run_ok(
        &fake,
        &c,
        &["verify", &d.ts, "--fail", "--body", "another finding"],
    );
    assert!(
        o.stdout.contains("this run finishes the stop"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &d.item).as_deref(), Some("Blocked"));
    assert_eq!(fail_reports(&fake, d.t), 1);
    assert_eq!(questions(&fake, d.t).len(), 1);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn the_headless_tick_finishes_a_stop_that_a_failed_write_left_open() {
    let (fake, c, b) = crew_with("max_rounds: 1\n");
    let d = half_stopped(&fake, &c, &b, "IT rounds rerun tick");
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    // the Gate of this ticket is prose and a question is open: the tick judges nothing here,
    // but it finishes the stop that is in the log
    let o = fake.run(headless.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("this run finishes the stop"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &d.item).as_deref(), Some("Blocked"));
    assert_eq!(fail_reports(&fake, d.t), 1);
    assert_eq!(questions(&fake, d.t).len(), 1);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_reassign_of_a_stopped_ticket_withdraws_the_question_and_starts_the_count_again() {
    let (fake, c, b) = crew_with("max_rounds: 2\n");
    let d = delivered(&fake, &c, &b, "IT rounds reassign");
    fail(&fake, &c, &d);
    deliver(&fake, &b, &d);
    let _ = fake.poll_json(&c);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 2, 2);

    let _ = fake.poll_json(&c);
    let o = run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &d.ts,
            "--to",
            &b.name,
            "--reason",
            "blocked-too-long",
            "--why",
            "a new plan: smaller steps",
        ],
    );
    assert!(o.stdout.contains("answer to "), "{}", o.all());
    assert_eq!(status(&fake, &d.item).as_deref(), Some("Ready"));
    assert!(
        fake.comments_of(d.t)
            .iter()
            .any(|(_, _, body)| body.contains("withdraws: question")),
        "the reassign withdraws the question of the stop"
    );
    // the agent takes the ticket again, and its first fail is round 1 of 2
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &d.ts]);
    deliver(&fake, &b, &d);
    let _ = fake.poll_json(&c);
    let o = fail(&fake, &c, &d);
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert_eq!(questions(&fake, d.t).len(), 1);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn an_owner_comment_before_the_stop_does_not_start_the_count_again() {
    let (fake, c, b) = crew_with("max_rounds: 2\n");
    let d = delivered(&fake, &c, &b, "IT rounds comment");
    fail(&fake, &c, &d);
    // the owner comments during normal repair: no stop is open, so nothing is released
    fake.owner_comment_now(d.t, "Please keep the public API as it is.");
    deliver(&fake, &b, &d);
    let _ = fake.poll_json(&c);
    let o = fail(&fake, &c, &d);
    assert_stopped(&fake, &d, &o, 2, 2);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_note_with_a_case_line_is_refused_before_any_read() {
    let (fake, c, b) = crew_with("max_rounds: 2\n");
    let d = delivered(&fake, &c, &b, "IT rounds note");
    let before = fake.requests().len();
    let o = run_refused(
        &fake,
        &c,
        &[
            "verify",
            &d.ts,
            "--fail",
            "--body",
            "part 2 is missing\ncase: round limit 2 of 2; the owner decides",
        ],
        "text-reads-as-act",
    );
    assert!(o.stderr.contains("`case:`"), "{}", o.all());
    assert_eq!(fake.requests().len(), before, "the refusal sent a request");
    assert_eq!(status(&fake, &d.item).as_deref(), Some("Review"));
}
