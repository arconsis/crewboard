//! GitHub's built-in project workflows against a fake GitHub.
//!
//! `doctor --board` warns once per enabled workflow that writes Status or adds items, gives no
//! warning when all of them are off, and says "could not check" when the query fails, with the
//! exit of the local check. The watch marks a status change that GitHub automation made, does
//! not mark one by a crew account, reads no timeline for a change that a signed crewboard
//! message explains, and reads no timeline in a quiet poll or a poll without a change.
//! `selftest` calls the same check (`builtin_workflows::check`). `init` and `migrate` delete the
//! four that write Status (`fake_init.rs`, `fake_migrate.rs`).

use crate::support::board::{Agent, FakeBoard, Out};
use crewboard::rules::Role;
use serde_json::Value;

const GATE: &str = "Goal: a ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\n";
const AUTOMATION: &str = "github-project-automation[bot]";
const MARK: &str = "(by GitHub automation)";

fn doctor_board(fake: &FakeBoard) -> Out {
    let dir = fake.owner_dir();
    fake.run(dir.path(), &["doctor", "--board"])
}

fn warning_lines(o: &Out) -> Vec<String> {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("WARNING: built-in project workflow"))
        .map(str::to_string)
        .collect()
}

#[test]
fn doctor_board_warns_once_per_enabled_workflow_that_writes_status() {
    let fake = FakeBoard::new();
    fake.set_workflows(&[
        ("Item closed", true),
        ("Pull request linked to issue", true),
        ("Pull request merged", false),
        ("Auto-close issue", true),
    ]);
    let o = doctor_board(&fake);
    assert_eq!(o.code, 0, "{}", o.all());
    let w = warning_lines(&o);
    assert_eq!(w.len(), 2, "{}", o.all());
    assert!(w[0].contains("\"Item closed\" is on"), "{}", o.all());
    assert!(
        w[1].contains("\"Pull request linked to issue\" is on"),
        "{}",
        o.all()
    );
    // the fix of the four is the command, not the web UI
    assert!(
        w.iter().all(|l| l.contains("crewboard init")),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("Pull request merged"), "{}", o.all());
    assert!(!o.stdout.contains("Auto-close"), "{}", o.all());
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn doctor_board_names_the_ui_page_for_a_writer_that_migrate_does_not_delete() {
    let fake = FakeBoard::new();
    fake.set_workflows(&[
        ("Item reopened", true),
        ("Auto-close issue", true),
        ("Auto-add sub-issues to project", true),
    ]);
    let o = doctor_board(&fake);
    let w = warning_lines(&o);
    assert_eq!(w.len(), 1, "{}", o.all());
    assert!(w[0].contains("\"Item reopened\" is on"), "{}", o.all());
    assert!(w[0].contains("/workflows"), "{}", o.all());
    assert!(!w[0].contains("migrate"), "{}", o.all());
}

#[test]
fn doctor_board_warns_for_each_of_the_four_on_a_new_board_and_only_for_those() {
    let fake = FakeBoard::new();
    fake.set_default_workflows();
    let o = doctor_board(&fake);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(warning_lines(&o).len(), 4, "{}", o.all());
    assert!(!o.stdout.contains("Auto-close"), "{}", o.all());
    assert!(!o.stdout.contains("sub-issues"), "{}", o.all());
    // doctor only reads: it deletes nothing
    assert!(fake.workflows_deleted().is_empty());
    assert_eq!(fake.workflow_names().len(), 6);
}

#[test]
fn doctor_board_gives_no_warning_when_every_status_writer_is_off() {
    let fake = FakeBoard::new();
    fake.set_workflows(&[
        ("Item added to project", false),
        ("Item closed", false),
        ("Pull request linked to issue", false),
        ("Pull request merged", false),
        ("Auto-close issue", true),
        ("Auto-add sub-issues to project", false),
    ]);
    let o = doctor_board(&fake);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(warning_lines(&o).is_empty(), "{}", o.all());
    assert!(
        o.stdout
            .contains("built-in workflows: none that writes Status or adds items is on"),
        "{}",
        o.all()
    );
}

#[test]
fn doctor_board_says_could_not_check_when_the_query_fails_and_keeps_the_exit() {
    let fake = FakeBoard::new();
    fake.fail_workflows("Your token has not been granted the required scopes");
    let o = doctor_board(&fake);
    let plain = fake.run(fake.owner_dir().path(), &["doctor"]);
    assert_eq!(o.code, plain.code, "{}\n{}", o.all(), plain.all());
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("WARNING: crewboard could not check the built-in project workflows"),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains("required scopes"), "{}", o.all());
    assert!(warning_lines(&o).is_empty(), "{}", o.all());
}

#[test]
fn plain_doctor_reads_no_board() {
    let fake = FakeBoard::new();
    fake.set_workflows(&[("Item closed", true)]);
    let o = fake.run(fake.owner_dir().path(), &["doctor"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("`crewboard doctor --board` reads the board"),
        "{}",
        o.all()
    );
    assert!(warning_lines(&o).is_empty(), "{}", o.all());
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

/// The coordinator's `status-changed` events of ticket `t`, with their bodies.
fn status_changes(v: &Value, t: u64) -> Vec<String> {
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "status-changed" && e["issue"] == t)
        .map(|e| e["body"].as_str().unwrap().to_string())
        .collect()
}

/// A coordinator, a dev agent and a ticket that the coordinator assigned to the dev. The
/// coordinator has polled once, so its snapshot holds the ticket. Returns the item id too.
fn assigned() -> (FakeBoard, Agent, Agent, u64, String) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let t = fake.ticket_issue("IT workflow writes", GATE, &["crew:work"]);
    let o = fake.run(&c.dir, &["assign", &t.to_string(), "--to", &b.name]);
    assert_eq!(o.code, 0, "{}", o.all());
    let item = fake.item_of_issue(t).expect("ticket item");
    let _ = fake.poll_json(&c);
    (fake, c, b, t, item)
}

#[test]
fn a_status_change_by_github_automation_is_marked_and_one_by_a_crew_account_is_not() {
    let (fake, c, _b, t, item) = assigned();
    // a poll without a change reads no timeline
    let (v, _) = fake.poll_json(&c);
    assert!(status_changes(&v, t).is_empty(), "{v}");
    assert_eq!(fake.timeline_reads(), 0);

    // "Pull request linked to issue" moves the ticket: no crewboard message explains it
    fake.set_item(&item, "Status", "In Progress");
    fake.touch_item(&item);
    fake.status_changed_by(t, AUTOMATION);
    // a quiet poll (a peek) reads no timeline
    let q = fake.run(&c.dir, &["poll", "--quiet"]);
    assert!(q.stdout.contains("events pending"), "{}", q.all());
    assert_eq!(fake.timeline_reads(), 0);
    // the real poll: one timeline read, and the event says who made the change
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        status_changes(&v, t),
        vec![format!("status Ready -> In Progress {MARK}")],
        "{v}"
    );
    assert_eq!(fake.timeline_reads(), 1);

    // a person with the crew account moves it in the web UI: one read, no mark
    fake.set_item(&item, "Status", "Ready");
    fake.touch_item(&item);
    fake.status_changed_by(t, "fake-crew");
    let (v, _) = fake.poll_json(&c);
    assert_eq!(
        status_changes(&v, t),
        vec!["status In Progress -> Ready".to_string()],
        "{v}"
    );
    assert_eq!(fake.timeline_reads(), 2);
    // the next poll repeats nothing and reads nothing
    let (v, _) = fake.poll_json(&c);
    assert!(status_changes(&v, t).is_empty(), "{v}");
    assert_eq!(fake.timeline_reads(), 2);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_status_change_that_a_crewboard_message_explains_reads_no_timeline() {
    let (fake, c, b, t, _item) = assigned();
    // the timeline would name the automation, so a read would mark the event
    fake.status_changed_by(t, AUTOMATION);
    let o = fake.run(&b.dir, &["take", &t.to_string()]);
    assert_eq!(o.code, 0, "{}", o.all());
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        status_changes(&v, t),
        vec!["status Ready -> In Progress".to_string()],
        "{v}"
    );
    assert_eq!(fake.timeline_reads(), 0);
}
