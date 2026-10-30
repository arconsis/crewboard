//! A deleted note of the coordinator (spec/security-details#deleted-notes), against the fake
//! GitHub. The coordinator remembers the comment ids of the signed notes it posts on a ticket. A
//! holder of the crew token deletes the close note and sets the board back: the signed log then
//! agrees with the board, but the next `poll` of the coordinator raises the security event
//! `deleted-message` once, and the owner hears of it once. A ticket without a deletion raises
//! nothing, a failed read raises nothing, and the order "set back, then delete" is seen too.

use crate::support::board::{FakeBoard, REGISTRY, REPO};
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
    // the move out of Dropped is read by the Status check (`log_check`), which passes its read
    // to the note check
    assert!(
        o.stderr.contains(&format!("cannot read {REPO}#{t}")),
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
    let line = format!(
        "!! SECURITY EVENT: deleted-message !!\nwhere: fake-owner/fake-repo#{TICKET} comment {report} by (deleted)"
    );
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

/// The owner report of a `deleted-message` event fails once (an injected 500 on the post to the
/// registry issue).
fn fail_the_report(fake: &FakeBoard) {
    fake.fail_once(
        "POST",
        &format!("issues/{REGISTRY}/comments"),
        "Security event: deleted-message",
    );
}

#[test]
fn a_failed_owner_report_in_watch_keeps_the_note_and_the_next_cycle_raises_it_again() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT failed report watch");
    assert_eq!(deleted(&fake, &c), vec![]);
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.set_item(&item, "Status", "In Progress");
    fail_the_report(&fake);
    // the first cycle cannot report the event and fails; `watch` repeats the cycle with the
    // same memory, which still holds the note
    let w = fake.run(
        &c.dir,
        &["watch", "--json", "--timeout", "20s", "--interval", "1s"],
    );
    assert_eq!(w.code, 0, "{}", w.all());
    assert!(w.stderr.contains("retrying"), "{}", w.all());
    assert!(w.stdout.contains("deleted_message"), "{}", w.all());
    assert!(
        w.stdout.contains(&format!("\"comment_id\":{note}")),
        "{}",
        w.all()
    );
    assert_eq!(owner_reports(&fake), 1);
    // shown and reported: forgotten now, so the next cycle says nothing of it
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(deleted(&fake, &c), vec![]);
    assert_eq!(owner_reports(&fake), 1);
    no_unhandled(&fake);
}

#[test]
fn a_failed_owner_report_in_poll_keeps_the_note_for_the_next_poll() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT failed report poll");
    assert_eq!(deleted(&fake, &c), vec![]);
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.set_item(&item, "Status", "In Progress");
    fail_the_report(&fake);
    let o = fake.run(&c.dir, &["poll", "--json"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("deleted_message"), "{}", o.all());
    assert_eq!(owner_reports(&fake), 0);
    // the next poll raises it again and reports it once
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(deleted(&fake, &c), vec![(note, t)]);
    assert_eq!(owner_reports(&fake), 1);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(deleted(&fake, &c), vec![]);
    no_unhandled(&fake);
}

#[test]
fn a_failed_owner_report_in_tick_keeps_the_note_and_the_next_tick_raises_it_again() {
    use crate::support::headless::{Hub, Scenario, TICKET};
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: false,
    });
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let report = comment_with(&h.board, TICKET, "verify: fail");
    h.board.delete_comment(report);
    h.board.set_item("PVTI_ticket", "Status", "Review");
    fail_the_report(&h.board);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    // the report fails, so the tick fails and shows nothing of the event
    let o = h.headless(&["tick"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("deleted-message"), "{}", o.all());
    assert_eq!(owner_reports(&h.board), 0);
    // the next tick raises it again, reports it and shows it
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line = format!(
        "!! SECURITY EVENT: deleted-message !!\nwhere: fake-owner/fake-repo#{TICKET} comment {report} by (deleted)"
    );
    assert!(o.stdout.contains(&line), "{}", o.all());
    assert!(o.stdout.contains("told the owner"), "{}", o.all());
    assert_eq!(owner_reports(&h.board), 1);
    // settled: the next tick says nothing of it
    h.board.set_item("PVTI_ticket", "Status", "Review");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("deleted-message"), "{}", o.all());
    assert_eq!(owner_reports(&h.board), 1);
}

#[test]
fn a_ticket_whose_board_item_was_deleted_is_read_once_by_its_issue_number() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT item gone");
    let (_, kept_item) = dropped_ticket(&fake, &c, &b, "IT item gone control");
    assert_eq!(deleted(&fake, &c), vec![]);
    // somebody deletes the close note and the board item; the control loses only its item
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.delete_item(&item);
    fake.delete_item(&kept_item);
    // a ticket without a row is judged only when its newest note is older than 10 minutes (the
    // index of a long `watch` may not know a brand-new ticket yet)
    let (v, _) = fake.poll_json(&c);
    assert!(!v.to_string().contains("notes-not-checked"), "{v}");
    age_notes(&c, 3600);
    let not_checked = |v: &serde_json::Value| -> usize {
        v["events"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter(|e| e["kind"] == "notes-not-checked")
            .count()
    };
    let (v, _) = fake.poll_json(&c);
    let gone: Vec<u64> = v["security"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|s| s["reason"] == "deleted_message")
        .map(|s| s["comment_id"].as_u64().unwrap())
        .collect();
    assert_eq!(gone, vec![note], "{v}");
    assert_eq!(not_checked(&v), 2, "one note per ticket: {v}");
    assert_eq!(owner_reports(&fake), 1);
    assert_eq!(
        not_checked_reports(&fake),
        2,
        "the owner hears of each ticket once"
    );
    // once: the next cycle reads neither ticket again
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (v, _) = fake.poll_json(&c);
    assert_eq!(not_checked(&v), 0, "{v}");
    assert!(!v.to_string().contains("deleted_message"), "{v}");
    assert_eq!(owner_reports(&fake), 1);
    assert_eq!(not_checked_reports(&fake), 2);
    no_unhandled(&fake);
}

/// Make every remembered note of the coordinator older by `secs`.
fn age_notes(who: &crate::support::board::Agent, secs: i64) {
    for e in std::fs::read_dir(who.dir.join("state")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let Some(map) = v["posted_notes"].as_object_mut() else {
            continue;
        };
        for list in map.values_mut() {
            for n in list.as_array_mut().unwrap() {
                n["at"] = serde_json::json!(n["at"].as_i64().unwrap() - secs);
            }
        }
        std::fs::write(&p, v.to_string()).unwrap();
    }
}

/// The owner reports of tickets whose notes are not checked as before.
fn not_checked_reports(fake: &FakeBoard) -> usize {
    fake.comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, b)| b.contains("Notes not checked: "))
        .count()
}

/// The `notes-not-checked` events of one `poll --json` answer.
fn not_checked_events(v: &serde_json::Value) -> usize {
    v["events"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|e| e["kind"] == "notes-not-checked")
        .count()
}

/// The reads of the comment list of `issue` so far (every page counts).
fn list_reads(fake: &FakeBoard, issue: u64) -> usize {
    let part = format!("/issues/{issue}/comments");
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&part))
        .count()
}

/// Make the too-large memory of the coordinator older by `secs` (the slow read is due then).
fn age_too_large(who: &crate::support::board::Agent, secs: i64) {
    for e in std::fs::read_dir(who.dir.join("state")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let map = v["notes_too_large"]
            .as_object_mut()
            .expect("notes_too_large");
        for at in map.values_mut() {
            *at = serde_json::json!(at.as_i64().unwrap() - secs);
        }
        std::fs::write(&p, v.to_string()).unwrap();
    }
}

#[test]
fn a_comment_flood_keeps_the_notes_and_the_set_back_after_it_shows_the_deleted_close_note() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT flood");
    assert_eq!(deleted(&fake, &c), vec![]);
    // the attacker floods the ticket (more than 5000 comments) and moves the board once, so the
    // coordinator reads the list and hits the page limit
    fake.flood_comments(t);
    fake.set_item(&item, "Status", "Blocked");
    let (v, _) = fake.poll_json(&c);
    assert_eq!(not_checked_events(&v), 1, "{v}");
    assert!(v.to_string().contains("more than 50 pages"), "{v}");
    assert_eq!(not_checked_reports(&fake), 1);
    // told once: the next cycle does not read the list again and says nothing more
    let reads = list_reads(&fake, t);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (v, _) = fake.poll_json(&c);
    assert_eq!(not_checked_events(&v), 0, "{v}");
    assert_eq!(list_reads(&fake, t), reads, "no read in every cycle");
    assert_eq!(not_checked_reports(&fake), 1);
    // the attacker deletes the close note, deletes the flood and sets the board back: the row
    // changed, the list is complete again, and the deletion is seen (it was never found before
    // the fix: the flood made the coordinator forget the notes)
    let note = comment_with(&fake, t, "dropped: no PR needed");
    fake.delete_comment(note);
    fake.end_flood(t);
    fake.set_item(&item, "Status", "In Progress");
    assert_eq!(deleted(&fake, &c), vec![(note, t)]);
    assert_eq!(owner_reports(&fake), 1);
    no_unhandled(&fake);
}

#[test]
fn a_false_done_on_a_flooded_ticket_stays_under_the_status_check() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT flood done",
        &format!("Goal: flood.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let _ = fake.poll_json(&c);
    // a flood, then a Done that no signed message supports
    fake.flood_comments(t);
    fake.set_item(&item, "Status", "Done");
    fake.touch_item_ago(&item, 300);
    let (v, _) = fake.poll_json(&c);
    assert_eq!(not_checked_events(&v), 1, "{v}");
    let mismatch = |fake: &FakeBoard| {
        fake.comments_of(REGISTRY)
            .iter()
            .filter(|(_, _, b)| b.contains("Status mismatch: "))
            .count()
    };
    assert_eq!(mismatch(&fake), 0);
    // the flood goes and the board stays: the slow read judges the Done and reports it
    fake.end_flood(t);
    age_too_large(&c, 7 * 3600);
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (v, _) = fake.poll_json(&c);
    assert_eq!(mismatch(&fake), 1, "the false Done is reported: {v}");
    let kinds: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"status-mismatch"), "{v}");
}

#[test]
fn a_failed_owner_report_of_a_flood_in_watch_raises_notes_not_checked_again() {
    let (fake, c, b) = crew();
    let (t, item) = dropped_ticket(&fake, &c, &b, "IT flood watch");
    assert_eq!(deleted(&fake, &c), vec![]);
    fake.flood_comments(t);
    fake.set_item(&item, "Status", "Blocked");
    // the first cycle cannot post the owner report and fails; `watch` repeats the cycle with the
    // memory in place, so the event is raised again, reported and shown
    fake.fail_once(
        "POST",
        &format!("issues/{REGISTRY}/comments"),
        "Notes not checked",
    );
    let w = fake.run(
        &c.dir,
        &["watch", "--json", "--timeout", "20s", "--interval", "1s"],
    );
    assert_eq!(w.code, 0, "{}", w.all());
    assert!(w.stderr.contains("retrying"), "{}", w.all());
    assert!(w.stdout.contains("notes-not-checked"), "{}", w.all());
    assert_eq!(not_checked_reports(&fake), 1);
    // shown and reported: settled, so the next cycle says nothing of it
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let (v, _) = fake.poll_json(&c);
    assert_eq!(not_checked_events(&v), 0, "{v}");
    assert_eq!(not_checked_reports(&fake), 1);
    no_unhandled(&fake);
}
