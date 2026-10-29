//! A command that reads Status, decides and writes re-reads Status by id before its first write
//! (spec/board#concurrent-writers): a ticket that somebody moved meanwhile is refused with
//! `status-changed-meanwhile`, and no field, option or message was written before the refusal.

use crate::support::board::{Agent, FakeBoard};
use crate::support::cli::{GATE, no_unhandled, run_ok, run_refused, writes};
use crewboard::rules::Role;

fn status(fake: &FakeBoard, t: u64) -> Option<String> {
    fake.item_field(&fake.item_of_issue(t).unwrap(), "Status")
}

fn crew() -> (FakeBoard, Agent, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let a = fake.agent("claude@fake-a", Role::Dev);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    (fake, c, a, b)
}

fn ticket(fake: &FakeBoard) -> u64 {
    fake.ticket_issue(
        "IT status guard",
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
}

#[test]
fn assign_over_a_ticket_that_moved_meanwhile_writes_no_field() {
    let (fake, c, a, _b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    run_ok(&fake, &c, &["ticket", "adopt", &ts]);
    let item = fake.item_of_issue(t).unwrap();
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    let gate = fake.item_field(&item, "Gate");
    // moved behind the command after its first read of the item, before the guard
    fake.flip_after_item_reads(&item, "Status", "In Progress", 1);
    let before = writes(&fake);
    run_refused(
        &fake,
        &c,
        &["assign", &ts, "--to", &a.name],
        "status-changed-meanwhile",
    );
    assert_eq!(writes(&fake), before, "the refused assign wrote");
    for field in ["Agent", "Order"] {
        assert_eq!(fake.item_field(&item, field), None, "{field}");
    }
    assert_eq!(fake.item_field(&item, "Gate"), gate, "Gate");
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
    // control: nobody moves it, the same command works
    fake.set_item(&item, "Status", "Backlog");
    fake.touch_item(&item);
    run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert_eq!(status(&fake, t).as_deref(), Some("Ready"));
    assert_eq!(
        fake.item_field(&item, "Agent").as_deref(),
        Some(a.name.as_str())
    );
    no_unhandled(&fake);
}

#[test]
fn reassign_over_a_ticket_that_moved_meanwhile_keeps_its_agent_and_posts_no_notice() {
    let (fake, c, _a, b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let item = fake.item_of_issue(t).unwrap();
    let notices = |f: &FakeBoard| {
        f.comments_of(t)
            .iter()
            .filter(|(_, _, body)| body.contains("Reassigned."))
            .count()
    };
    fake.flip_after_item_reads(&item, "Status", "Review", 1);
    let before = writes(&fake);
    run_refused(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead"],
        "status-changed-meanwhile",
    );
    assert_eq!(writes(&fake), before, "the refused reassign wrote");
    assert_eq!(
        fake.item_field(&item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(notices(&fake), 0);
    assert_eq!(status(&fake, t).as_deref(), Some("Review"));
}

#[test]
fn the_derived_reassign_judges_the_fresh_status_and_not_the_one_of_the_listing() {
    use crate::support::late::{LateBoard, TICKET};
    // the listing shows In Progress; by id the item is Blocked already (both can be returned)
    let s = LateBoard::new("In Progress");
    s.board.set_item(
        &s.dev.item_id,
        "Heartbeat",
        &crewboard::clock::format_ts(crewboard::clock::now_epoch() - 86_400),
    );
    s.board
        .flip_after_item_reads("PVTI_ticket", "Status", "Blocked", 1);
    let (_v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.all().contains("status-changed-meanwhile"), "{}", o.all());
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("Backlog")
    );
    assert_eq!(s.board.item_field("PVTI_ticket", "Agent"), None);
    assert!(
        s.board
            .comments_of(TICKET)
            .iter()
            .any(|(_, _, b)| b.contains("Reassigned.")),
        "no notice"
    );
}

#[test]
fn the_derived_reassign_does_not_touch_a_ticket_that_another_agent_holds_now() {
    use crate::support::late::LateBoard;
    let s = LateBoard::new("In Progress");
    s.board.set_item(
        &s.dev.item_id,
        "Heartbeat",
        &crewboard::clock::format_ts(crewboard::clock::now_epoch() - 86_400),
    );
    s.board
        .flip_after_item_reads("PVTI_ticket", "Agent", "claude@fake-a", 1);
    let (_v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Agent").as_deref(),
        Some("claude@fake-a")
    );
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("In Progress")
    );
}
