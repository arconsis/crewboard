//! `audit` compares board fields with the signed log (spec/overview#where-things-live), end to
//! end against the fake GitHub. A clean board gives no `field-mismatch`. A field written by hand
//! that the log does not support gives one.

use crate::support::board::{FakeBoard, INITIATIVE};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok};
use crewboard::rules::Role;
use serde_json::Value;

/// The `field-mismatch` details of `audit --json`.
fn mismatches(fake: &FakeBoard, who: &crate::support::board::Agent) -> Vec<String> {
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

#[test]
fn a_clean_board_has_no_mismatch_and_a_status_written_by_hand_has_one() {
    let (fake, c, b) = crew();
    let t = fake.ticket_issue(
        "IT audit fields",
        &format!("Goal: audit test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(mismatches(&fake, &c), Vec::<String>::new());

    // a raw field write that no signed message supports
    fake.set_item(&item, "Status", "Done");
    let got = mismatches(&fake, &c);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains("board Status is Done, the log shows In Progress")
            && got[0].ends_with("an owner edit is not in the log"),
        "{got:?}"
    );
    // the audit fixes nothing
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Done"));
    let o = run_ok(&fake, &c, &["audit", "--initiative", INITIATIVE]);
    assert!(o.stdout.contains("[field-mismatch]"), "{}", o.stdout);
    no_unhandled(&fake);
}

#[test]
fn a_left_agent_whose_state_is_set_back_to_active_has_one_mismatch() {
    let (fake, c, _b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    assert_eq!(mismatches(&fake, &c), Vec::<String>::new());
    run_ok(&fake, &d, &["stop"]);
    assert_eq!(
        fake.item_field(&d.item_id, "State").as_deref(),
        Some("left")
    );
    assert_eq!(mismatches(&fake, &c), Vec::<String>::new());

    fake.set_item(&d.item_id, "State", "active");
    let got = mismatches(&fake, &c);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains("claude@fake-d") && got[0].contains("left marker"),
        "{got:?}"
    );
    no_unhandled(&fake);
}
