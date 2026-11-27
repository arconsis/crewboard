//! The round limit `max_rounds` of AUTHORITY.md (spec/tickets#round-limit), against the fake
//! GitHub of `tests/support/board.rs`.
//!
//! The failed verify that spends the last round moves the ticket to Blocked, with a question to
//! the owner and the label `needs-owner`. An owner comment or the coordinator's `unblock`
//! releases the ticket, and the count starts again. The headless `tick` does the same. Without
//! the key nothing changes.

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

/// Assign, take, PR and done. The ticket declares `CI: none`, so its evidence holds.
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Delivered {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(title, &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let d = Delivered {
        t,
        ts,
        item,
        pr_url: pr.url,
        head: pr.head,
        pr: pr.number,
    };
    deliver(fake, b, &d);
    d
}

/// The dev reads its messages and delivers again.
fn deliver(fake: &FakeBoard, b: &Agent, d: &Delivered) {
    let _ = fake.poll_json(b);
    run_ok(
        fake,
        b,
        &["done", &d.ts, "--pr", &d.pr_url, "--sha", &d.head],
    );
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
    let _ = fake.poll_json(&b);
    let o = run_refused(&fake, &b, &["take", &d.ts], "status-answer-missing");
    assert!(
        o.stderr.contains("Blocked by a question to the owner"),
        "{}",
        o.all()
    );
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
