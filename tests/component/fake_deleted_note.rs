//! A deleted note of the coordinator (spec/security-details#deleted-notes), against the fake
//! GitHub. The coordinator remembers the comment ids of the signed notes it posts on a ticket. A
//! holder of the crew token deletes the close note and sets the board back: the signed log then
//! agrees with the board, but the next `poll` of the coordinator raises the security event
//! `deleted-message` once, and the owner hears of it once. A ticket without a deletion raises
//! nothing, a failed read raises nothing, and the order "set back, then delete" is seen too.

use crate::support::board::{FakeBoard, REGISTRY};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok};

/// `(comment id, issue)` of each `deleted-message` event of one `poll --json` of `who`.
fn deleted(fake: &FakeBoard, who: &crate::support::board::Agent) -> Vec<(u64, u64)> {
    let (v, _) = fake.poll_json(who);
    v["security"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|s| s["reason"] == "deleted_message")
        .map(|s| {
            (
                s["comment_id"].as_u64().unwrap(),
                s["issue"].as_u64().unwrap(),
            )
        })
        .collect()
}

/// The id of the comment of `issue` whose body contains `text`.
fn comment_with(fake: &FakeBoard, issue: u64, text: &str) -> u64 {
    fake.comments_of(issue)
        .iter()
        .find(|(_, _, b)| b.contains(text))
        .unwrap_or_else(|| panic!("no comment with {text:?} on #{issue}"))
        .0
}

/// The owner reports of deleted notes in the registry issue.
fn owner_reports(fake: &FakeBoard) -> usize {
    fake.comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, b)| b.contains("Security event: deleted-message"))
        .count()
}

/// A ticket that the coordinator assigned and then dropped (`close --why`): Dropped on the board,
/// with the signed `dropped:` note. Returns the issue and the item.
fn dropped_ticket(
    fake: &FakeBoard,
    c: &crate::support::board::Agent,
    b: &crate::support::board::Agent,
    title: &str,
) -> (u64, String) {
    let t = fake.ticket_issue(
        title,
        &format!("Goal: deleted note.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    run_ok(fake, c, &["close", &ts, "--why", "no PR needed"]);
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Dropped"));
    (t, item)
}

#[test]
fn a_deleted_close_note_raises_deleted_message_once_and_a_ticket_without_a_deletion_raises_nothing()
{
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT deleted note");
    let (control, control_item) = dropped_ticket(&fake, &c, &b, "IT kept note");
    // control: nothing is deleted, nothing is raised
    assert_eq!(deleted(&fake, &c), vec![]);

    // somebody with the crew token deletes the close note and sets the board back; the signed
    // log now agrees with the board
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.set_item(&item, "Status", "In Progress");
    assert_eq!(deleted(&fake, &c), vec![(note, t)]);
    assert_eq!(owner_reports(&fake), 1);
    // the second cycle does not raise it again, and the owner hears of it once
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(deleted(&fake, &c), vec![]);
    assert_eq!(owner_reports(&fake), 1);

    // control: the board moves, but every note is still there
    fake.set_item(&control_item, "Status", "In Progress");
    assert_eq!(deleted(&fake, &c), vec![]);
    // the order "set back, then delete": the board contradicts the log, so the coordinator
    // reads the ticket again at the next cycle and sees the deletion then
    let kept = comment_with(&fake, control, "dropped: no PR needed");
    fake.delete_comment(kept);
    assert_eq!(deleted(&fake, &c), vec![(kept, control)]);
    assert_eq!(owner_reports(&fake), 2);
    no_unhandled(&fake);
}

#[test]
fn a_failed_read_raises_nothing_and_the_next_cycle_reads_again() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT failed read");
    assert_eq!(deleted(&fake, &c), vec![]);
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.set_item(&item, "Status", "Review");
    // the comment list of the ticket cannot be read in this cycle: no event, no false alarm
    fake.fail_once("GET", &format!("issues/{t}/comments"), "");
    let o = fake.run(&c.dir, &["poll", "--json"]);
    assert!(
        o.stderr.contains("to check the notes of this coordinator"),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("deleted_message"), "{}", o.all());
    // the next cycle reads it, although the board did not change again
    assert_eq!(deleted(&fake, &c), vec![(note, t)]);
    no_unhandled(&fake);
}

#[test]
fn a_fresh_cache_remembers_nothing_and_raises_nothing() {
    // a coordinator without the memory (a fresh host, a lost state file) sees nothing: a
    // documented limit, not an error
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT fresh cache");
    let state = c.dir.join("state");
    for e in std::fs::read_dir(&state).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "json") {
            std::fs::remove_file(p).unwrap();
        }
    }
    assert_eq!(deleted(&fake, &c), vec![]);
    fake.delete_comment(comment_with(&fake, t, "dropped: no PR needed"));
    fake.set_item(&item, "Status", "In Progress");
    assert_eq!(deleted(&fake, &c), vec![]);
    no_unhandled(&fake);
}

#[test]
fn the_headless_tick_sees_a_deleted_verify_report_of_its_own_on_a_ticket_it_loads() {
    use crate::support::headless::{Hub, Scenario, TICKET};
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: false,
    });
    // the tick verifies: CI failed, so `verify: fail` and the ticket goes back to the dev
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        h.status_of_ticket().as_deref(),
        Some("In Progress"),
        "{}",
        o.all()
    );
    let report = comment_with(&h.board, TICKET, "verify: fail");
    // somebody deletes the report and sets the board back to Review
    h.board.delete_comment(report);
    h.board.set_item("PVTI_ticket", "Status", "Review");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line =
        format!("SECURITY EVENT deleted-message: fake-owner/fake-repo#{TICKET} comment {report}:");
    assert!(o.stdout.contains(&line), "{}", o.all());
    assert!(o.stdout.contains("told the owner"), "{}", o.all());
    assert_eq!(owner_reports(&h.board), 1);
    // once: the next tick on the same ticket in Review says nothing of it
    h.board.set_item("PVTI_ticket", "Status", "Review");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("deleted-message"), "{}", o.all());
    assert_eq!(owner_reports(&h.board), 1);
}
