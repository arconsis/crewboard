//! A Done without proof is detected, not prevented. The agent token can write the board
//! `Status` field; `status` lists every ticket whose Status the signed log does not support, and
//! the coordinator's watch cycle reports one to the owner once. Both use the judgement of
//! `audit` (`ticket::log::status_contradicts`).

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REGISTRY};
use crate::support::cli::{GATE, crew, item_in, run_ok};
use serde_json::Value;

fn ticket(fake: &FakeBoard) -> u64 {
    fake.ticket_issue(
        "IT log check",
        &format!("Goal: log check.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
}

fn status_json(fake: &FakeBoard, who: &Agent) -> Value {
    let o = run_ok(fake, who, &["status", "--json"]);
    serde_json::from_str(o.stdout.trim())
        .unwrap_or_else(|e| panic!("status --json ({e}): {}", o.all()))
}

fn mismatches(v: &Value) -> Vec<(String, String, String)> {
    v["status_log_mismatches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["ticket"].as_str().unwrap().to_string(),
                m["board_status"].as_str().unwrap().to_string(),
                m["log_status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn status_lists_a_done_that_no_signed_message_supports_and_only_that() {
    let (fake, c, b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    // control: a board that follows its log has no mismatch
    assert!(mismatches(&status_json(&fake, &c)).is_empty());
    let text = run_ok(&fake, &c, &["status"]);
    assert!(
        text.stdout
            .contains("board against signed log: 0 mismatch(es)"),
        "{}",
        text.all()
    );

    // a raw write of Done, no `done`, no `verify`, no `merge`
    fake.set_item(&item, "Status", "Done");
    fake.touch_item(&item);
    let v = status_json(&fake, &c);
    let key = format!("{}#{t}", crate::support::board::REPO);
    assert_eq!(
        mismatches(&v),
        vec![(key.clone(), "Done".to_string(), "In Progress".to_string())]
    );
    let text = run_ok(&fake, &c, &["status"]);
    assert!(
        text.stdout
            .contains("board against signed log: 1 mismatch(es)")
            && text.stdout.contains(&format!(
                "{key}: board Status is Done, the signed log shows In Progress"
            )),
        "{}",
        text.all()
    );
    // `status` reads only: nothing was fixed
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Done"));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

fn status_mismatch_notes(fake: &FakeBoard) -> usize {
    fake.comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, body)| body.contains("Status mismatch: "))
        .count()
}

#[test]
fn the_coordinator_cycle_reports_a_hand_written_done_to_the_owner_once() {
    let (fake, c, b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    // the cycle that records the snapshot
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert_eq!(status_mismatch_notes(&fake), 0);

    fake.set_item(&item, "Status", "Done");
    fake.touch_item_ago(&item, 5);
    // first sight: it may be a write in flight, so nothing is reported yet
    let _ = fake.poll_json(&c);
    assert_eq!(status_mismatch_notes(&fake), 0);
    // still there at the next cycle, and the item is quiet now: one signed note to the owner and
    // one event
    fake.touch_item_ago(&item, 300);
    let (v, _) = fake.poll_json(&c);
    assert_eq!(status_mismatch_notes(&fake), 1, "{v}");
    let kinds: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"status-mismatch"), "{v}");
    let note = fake
        .comments_of(REGISTRY)
        .into_iter()
        .find(|(_, _, body)| body.contains("Status mismatch: "))
        .unwrap()
        .2;
    assert!(
        note.contains("board Status is Done, the log shows In Progress"),
        "{note}"
    );
    assert!(note.contains("detected, not prevented"), "{note}");
    // later cycles say nothing more
    let (v, _) = fake.poll_json(&c);
    let _ = fake.poll_json(&c);
    assert_eq!(status_mismatch_notes(&fake), 1, "{v}");
    // the audit names the same ticket (one judgement)
    let o = run_ok(&fake, &c, &["audit", "--json", "--initiative", INITIATIVE]);
    assert!(
        o.stdout
            .contains("board Status is Done, the log shows In Progress"),
        "{}",
        o.all()
    );
}

#[test]
fn a_ticket_that_follows_its_log_is_never_reported() {
    let (fake, c, b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let _ = fake.poll_json(&c);
    run_ok(&fake, &b, &["take", &ts]);
    for _ in 0..3 {
        let _ = fake.poll_json(&c);
    }
    assert_eq!(status_mismatch_notes(&fake), 0);
}

#[test]
fn the_coordinator_cycle_reports_a_ticket_set_back_behind_its_close_once() {
    let (fake, c, b) = crew();
    let t = ticket(&fake);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &c, &["close", &ts, "--why", "no PR needed"]);
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Dropped"));
    // the cycle that records the snapshot
    let _ = fake.poll_json(&c);
    assert_eq!(status_mismatch_notes(&fake), 0);

    // a raw write sets the board back behind the signed drop
    fake.set_item(&item, "Status", "In Progress");
    fake.touch_item_ago(&item, 5);
    let _ = fake.poll_json(&c);
    assert_eq!(
        status_mismatch_notes(&fake),
        0,
        "first sight: a write in flight"
    );
    fake.touch_item_ago(&item, 300);
    let (v, _) = fake.poll_json(&c);
    assert_eq!(status_mismatch_notes(&fake), 1, "{v}");
    let ev: Vec<&Value> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "status-mismatch")
        .collect();
    assert_eq!(ev.len(), 1, "{v}");
    assert!(
        ev[0]["body"]
            .as_str()
            .unwrap_or("")
            .contains("board Status is In Progress, the log shows Dropped"),
        "{v}"
    );
    // a board/log mismatch, not a security event: the close note is still there
    assert!(v["security"].as_array().is_none_or(|s| s.is_empty()), "{v}");
    // later cycles say nothing more
    for _ in 0..2 {
        let (v, _) = fake.poll_json(&c);
        assert!(
            !v["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "status-mismatch"),
            "{v}"
        );
    }
    assert_eq!(status_mismatch_notes(&fake), 1);
}
