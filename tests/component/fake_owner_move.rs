//! `crewboard move <issue> <status>` (spec/tickets#owner-move), against the fake GitHub. The
//! owner moves a ticket by hand: the command posts `/crew move <status>` on the ticket, then
//! writes the board. `status` and `audit` then show no mismatch. A plain board drag without the
//! command is still reported. Done, Dropped, an unknown status and a work status without a
//! signed assignment are refused, and a refusal writes nothing.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REPO};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, writes};
use serde_json::Value;

/// The `status_log_mismatches` of `status --json` as `(ticket, board, log)`.
fn status_mismatches(fake: &FakeBoard, who: &Agent) -> Vec<(String, String, String)> {
    let o = run_ok(fake, who, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim())
        .unwrap_or_else(|e| panic!("status --json ({e}): {}", o.all()));
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

/// The `field-mismatch` details of `audit --json`.
fn audit_mismatches(fake: &FakeBoard, who: &Agent) -> Vec<String> {
    let o = run_ok(fake, who, &["audit", "--json", "--initiative", INITIATIVE]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    v["report"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "field_mismatch")
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .collect()
}

fn new_ticket(fake: &FakeBoard) -> u64 {
    fake.ticket_issue(
        "IT owner move",
        &format!("Goal: owner move.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
}

/// `crewboard move <args> --initiative fake` with the owner's own login.
fn owner_move(
    fake: &FakeBoard,
    dir: &std::path::Path,
    args: &[&str],
) -> crate::support::board::Out {
    let mut all = vec!["move"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--initiative", INITIATIVE]);
    fake.run(dir, &all)
}

#[test]
fn the_owner_moves_a_dropped_ticket_back_to_ready_and_status_and_audit_show_no_mismatch() {
    let (fake, c, b) = crew();
    let owner = fake.owner_dir();
    let t = new_ticket(&fake);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    // the coordinator drops the ticket: Dropped, and the log shows Dropped too
    run_ok(&fake, &c, &["close", &ts, "--why", "no PR needed"]);
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Dropped"));
    assert!(status_mismatches(&fake, &c).is_empty());
    assert_eq!(audit_mismatches(&fake, &c), Vec::<String>::new());
    // a later act of the assignee changes nothing: Dropped is final
    let o = fake.run(&b.dir, &["take", &ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("status-dropped-final"), "{}", o.all());
    // the owner closed the issue by hand
    fake.close_issue(t);

    let o = owner_move(
        &fake,
        owner.path(),
        &[&ts, "Ready", "--why", "the fix is incomplete"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("owner command posted: /crew move Ready")
            && o.stdout.contains("issue reopened")
            && o.stdout.contains("Status Dropped -> Ready"),
        "{}",
        o.all()
    );
    assert!(fake.issue_is_open(t));
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        fake.item_field(&item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    let (_, login, body) = fake.comments_of(t).last().cloned().unwrap();
    assert_eq!(login, crate::support::board::OWNER);
    assert_eq!(body, "/crew move Ready\nthe fix is incomplete\n");
    // the move is in the log: no mismatch in `status` or `audit`
    assert!(status_mismatches(&fake, &c).is_empty());
    assert_eq!(audit_mismatches(&fake, &c), Vec::<String>::new());

    // the assignee's acts count again after the move. The second ack has the body of the first;
    // in the same second its signed form is the same too, and the replay rule drops a copy. On a
    // real board the owner move comes minutes later, so the test waits into the next second.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(status_mismatches(&fake, &c), vec![]);
    assert_eq!(audit_mismatches(&fake, &c), Vec::<String>::new());

    // control: a plain drag without the command is still reported by both
    fake.set_item(&item, "Status", "Backlog");
    let key = format!("{REPO}#{t}");
    assert_eq!(
        status_mismatches(&fake, &c),
        vec![(key, "Backlog".to_string(), "In Progress".to_string())]
    );
    let got = audit_mismatches(&fake, &c);
    assert!(
        got.len() == 1 && got[0].contains("board Status is Backlog, the log shows In Progress"),
        "{got:?}"
    );
    // the owner records the drag with the command: the comment only, no Status write
    let o = owner_move(&fake, owner.path(), &[&ts, "backlog"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("on the board already"), "{}", o.all());
    assert!(status_mismatches(&fake, &c).is_empty());
    // the same move again changes nothing
    let before = writes(&fake);
    let o = owner_move(&fake, owner.path(), &[&ts, "Backlog"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("status-no-change"), "{}", o.all());
    assert_eq!(writes(&fake), before);
    no_unhandled(&fake);
}

#[test]
fn a_forbidden_target_is_refused_and_writes_nothing() {
    let (fake, c, b) = crew();
    let owner = fake.owner_dir();
    let t = new_ticket(&fake);
    let ts = t.to_string();
    run_ok(&fake, &c, &["ticket", "adopt", &ts]);
    let before = writes(&fake);
    for (status, rule) in [
        ("Done", "move-done-needs-close"),
        ("Dropped", "move-dropped-needs-close"),
        ("dropped", "move-dropped-needs-close"),
        ("Needs Human", "move-bad-status"),
        ("Elsewhere", "move-bad-status"),
        // a Backlog ticket without a signed assignment is nobody's work yet
        ("Ready", "move-needs-assignee"),
        ("In Progress", "move-needs-assignee"),
    ] {
        let o = owner_move(&fake, owner.path(), &[&ts, status]);
        assert_eq!(o.code, 2, "{status}: {}", o.all());
        assert!(o.stderr.contains(rule), "{status}: {}", o.all());
    }
    assert_eq!(writes(&fake), before);
    // control: after the signed assignment the same move passes
    let s2 = new_ticket(&fake);
    let item = item_in(&run_ok(
        &fake,
        &c,
        &["assign", &s2.to_string(), "--to", &b.name],
    ));
    let o = owner_move(&fake, owner.path(), &[&s2.to_string(), "Blocked"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Blocked"));
    assert!(status_mismatches(&fake, &c).is_empty());
    no_unhandled(&fake);
}

#[test]
fn a_move_line_of_another_account_is_not_the_owners_move() {
    let (fake, c, b) = crew();
    let t = new_ticket(&fake);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    // somebody writes the owner's line and drags the card
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - 20;
    fake.comment_at(t, "crew-bot", "/crew move Backlog", now, now);
    fake.set_item(&item, "Status", "Backlog");
    assert_eq!(status_mismatches(&fake, &c).len(), 1);
    // an owner move that another account edited does not count either
    let id = fake.owner_comment(t, "/crew move Ready");
    fake.edit_comment_by(id, "/crew move Backlog", "crew-bot");
    assert_eq!(status_mismatches(&fake, &c).len(), 1);
    assert_eq!(audit_mismatches(&fake, &c).len(), 1);
    no_unhandled(&fake);
}
