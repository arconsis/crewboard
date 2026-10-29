//! The 100-option limit of the `Agent` field (spec/board#option-limit), against the
//! fake GitHub. GitHub allows at most 100 options per single-select field. `approve` adds one
//! option per agent, so a board that has seen 100 agent names could approve no one more.
//!
//! The fake enforces the limit with GitHub's own error text (the first test proves that it
//! fires). The other tests prove the prune at the limit, what it never touches, the named
//! refusal when nothing can go, the cost (no extra read below the limit) and the `status`
//! warning.

use crate::support::board::{
    Agent, FakeBoard, INITIATIVE, OPTION_LIMIT, OPTION_LIMIT_ERROR, Out, REGISTRY,
};
use crate::support::cli::{no_unhandled, writes};
use crewboard::clock::{format_ts, now_epoch};
use crewboard::rules::Role;
use serde_json::{Value, json};

const DAY: i64 = 86_400;

/// The item listings that read the `Agent` value of every item (the extra read at the limit).
fn value_listings(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.path == "/graphql" && r.body.contains("fieldValueByName"))
        .count()
}

/// A dev agent that was approved, with its `Agent` option, now in `state`, last seen
/// `age` seconds ago.
fn dev(fake: &FakeBoard, name: &str, state: &str, age: i64) -> Agent {
    let a = fake.agent(name, Role::Dev);
    fake.set_item(&a.item_id, "State", state);
    fake.set_item(&a.item_id, "Heartbeat", &format_ts(now_epoch() - age));
    a
}

/// Fill the `Agent` field with hand-made options (no agent behind them) up to `total`.
fn fill_to(fake: &FakeBoard, total: usize) {
    let have = fake.agent_options().len();
    let names: Vec<String> = (have..total).map(|i| format!("hand-made-{i}")).collect();
    fake.seed_agent_options(&names);
    assert_eq!(fake.agent_options().len(), total);
}

fn approve(fake: &FakeBoard, owner: &tempfile::TempDir, name: &str) -> Out {
    fake.run(owner.path(), &["approve", name, "--initiative", INITIATIVE])
}

/// The ids of `before` minus `gone` must all be in `after`, in the same order.
fn kept_in_order(before: &[String], after: &[String], gone: &[&str]) {
    let want: Vec<&String> = before
        .iter()
        .filter(|i| !gone.contains(&i.as_str()))
        .collect();
    let got: Vec<&String> = after.iter().filter(|i| want.contains(i)).collect();
    assert_eq!(got, want, "an option id was lost or moved");
}

fn id_of(fake: &FakeBoard, name: &str) -> String {
    let (names, ids) = (fake.agent_options(), fake.agent_option_ids());
    let k = names
        .iter()
        .position(|n| n == name)
        .unwrap_or_else(|| panic!("no option {name}"));
    ids[k].clone()
}

/// Proof that the fake can fire: it answers more than 100 options with GitHub's error and
/// keeps the list, and it takes exactly 100.
#[test]
fn the_fake_enforces_githubs_option_limit() {
    let fake = FakeBoard::new();
    let send = |n: usize| -> Value {
        let o: Vec<Value> = (0..n)
            .map(|i| json!({"name": format!("o{i}"), "color": "GRAY", "description": ""}))
            .collect();
        let body = json!({
            "query": "mutation($f:ID!,$o:[ProjectV2SingleSelectFieldOptionInput!]!){updateProjectV2Field(input:{fieldId:$f,singleSelectOptions:$o}){projectV2Field{... on ProjectV2SingleSelectField{id}}}}",
            "variables": {"f": "PVTF_Agent", "o": o},
        });
        reqwest::blocking::Client::new()
            .post(format!("{}/graphql", fake.base()))
            .json(&body)
            .send()
            .unwrap()
            .json()
            .unwrap()
    };
    let v = send(OPTION_LIMIT + 1);
    assert_eq!(v["errors"][0]["message"], OPTION_LIMIT_ERROR, "{v}");
    assert!(
        fake.agent_options().is_empty(),
        "the refused write changed the list"
    );
    let v = send(OPTION_LIMIT);
    assert!(v.get("errors").is_none(), "{v}");
    assert_eq!(fake.agent_options().len(), OPTION_LIMIT);
    no_unhandled(&fake);
}

/// Acceptance 1: an add at 100 options with retired, unreferenced agents prunes exactly enough
/// (one option, the oldest) and succeeds. Every other option keeps its id and its place.
#[test]
fn approve_at_100_options_prunes_exactly_the_oldest_retired_unused_option() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    dev(&fake, "dev@fake-old", "left", 30 * DAY);
    dev(&fake, "dev@fake-mid", "left", 10 * DAY);
    dev(&fake, "dev@fake-live", "active", 60);
    fill_to(&fake, OPTION_LIMIT);
    fake.joined("dev@fake-new");
    let before = fake.agent_option_ids();
    let old_id = id_of(&fake, "dev@fake-old");

    let o = approve(&fake, &owner, "dev@fake-new");
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains(&format!("pruned Agent option dev@fake-old (id {old_id}")),
        "{}",
        o.all()
    );
    assert_eq!(
        o.stdout.matches("pruned Agent option").count(),
        1,
        "{}",
        o.stdout
    );
    let names = fake.agent_options();
    assert_eq!(names.len(), OPTION_LIMIT);
    assert!(!names.contains(&"dev@fake-old".to_string()));
    assert!(
        names.contains(&"dev@fake-mid".to_string()),
        "only as many as needed"
    );
    assert_eq!(names.last().map(String::as_str), Some("dev@fake-new"));
    kept_in_order(&before, &fake.agent_option_ids(), &[&old_id]);
    // the reads added at the limit: one listing of the item values (one page here)
    assert_eq!(value_listings(&fake), 1);
    assert!(o.stdout.contains("1 listing read(s)"), "{}", o.stdout);
    no_unhandled(&fake);
}

/// Acceptance 2: the option of a referenced agent or of an agent that has not left is never
/// pruned, even when it is older than the one that goes.
#[test]
fn a_referenced_or_active_agents_option_is_never_pruned() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    // oldest of all, left, but a Done ticket still carries it
    dev(&fake, "dev@fake-used", "left", 90 * DAY);
    fake.item(
        "PVTI_ticket_done",
        &[("Status", "Done"), ("Agent", "dev@fake-used")],
    );
    // older than the young one, but not left
    for (n, st) in [
        ("dev@fake-active", "active"),
        ("dev@fake-working", "working"),
        ("dev@fake-paused", "paused"),
        ("dev@fake-draining", "draining"),
        ("dev@fake-approved", "approved"),
    ] {
        dev(&fake, n, st, 60 * DAY);
    }
    // rejected by the owner, but its State was never set to left: not retired
    dev(&fake, "dev@fake-rejected", "active", 60 * DAY);
    fake.owner_comment(REGISTRY, "/crew reject dev@fake-rejected");
    // the only option that may go: left an hour ago, no item uses it
    dev(&fake, "dev@fake-young", "left", 3600);
    fill_to(&fake, OPTION_LIMIT);
    fake.joined("dev@fake-new");
    let before = fake.agent_option_ids();
    let young = id_of(&fake, "dev@fake-young");

    let o = approve(&fake, &owner, "dev@fake-new");
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("pruned Agent option dev@fake-young"),
        "{}",
        o.stdout
    );
    let names = fake.agent_options();
    for n in [
        "dev@fake-used",
        "dev@fake-active",
        "dev@fake-working",
        "dev@fake-paused",
        "dev@fake-draining",
        "dev@fake-approved",
        "dev@fake-rejected",
    ] {
        assert!(
            names.contains(&n.to_string()),
            "{n} was pruned: {}",
            o.stdout
        );
    }
    kept_in_order(&before, &fake.agent_option_ids(), &[&young]);
    assert_eq!(
        fake.item_field("PVTI_ticket_done", "Agent").as_deref(),
        Some("dev@fake-used"),
        "the ticket value must survive"
    );
    no_unhandled(&fake);
}

/// Acceptance 3: at 100 options, all in use, the add is refused with `agent-field-full`, the
/// text names the count and what the owner can do, and nothing is written (no approval
/// comment, no field write). Never the raw GraphQL error.
#[test]
fn at_100_options_all_in_use_approve_refuses_agent_field_full_and_writes_nothing() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    dev(&fake, "dev@fake-used", "left", 90 * DAY);
    fake.item(
        "PVTI_ticket_done",
        &[("Status", "Done"), ("Agent", "dev@fake-used")],
    );
    dev(&fake, "dev@fake-active", "active", 60);
    fill_to(&fake, OPTION_LIMIT);
    let new = fake.joined("dev@fake-new");
    let before = fake.agent_option_ids();
    let comments = fake.comments_of(REGISTRY).len();

    let o = approve(&fake, &owner, "dev@fake-new");
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("agent-field-full"), "{}", o.all());
    assert!(
        o.stderr.contains("100 options")
            && o.stderr.contains("1 carried by board items")
            && o.stderr.contains("1 of agents that have not left")
            && o.stderr.contains("98 that name no agent")
            && o.stderr.contains("/crew reject")
            && o.stderr.contains("Done or Dropped items"),
        "{}",
        o.all()
    );
    assert!(!o.all().contains(OPTION_LIMIT_ERROR), "{}", o.all());
    assert_eq!(writes(&fake), 0, "the refusal wrote something");
    assert_eq!(fake.comments_of(REGISTRY).len(), comments);
    assert_eq!(fake.agent_option_ids(), before);
    assert_eq!(
        fake.item_field(&new.item_id, "State").as_deref(),
        Some("requested")
    );
    no_unhandled(&fake);
}

/// The cost: below the limit `approve` reads nothing extra (no value listing), at 94 and at
/// 99 options. The control at 100 reads the listing once (see the prune test).
#[test]
fn below_the_limit_approve_adds_without_an_extra_read() {
    for total in [94, OPTION_LIMIT - 1] {
        let fake = FakeBoard::new();
        let owner = fake.owner_dir();
        dev(&fake, "dev@fake-old", "left", 30 * DAY);
        fill_to(&fake, total);
        fake.joined("dev@fake-new");
        let o = approve(&fake, &owner, "dev@fake-new");
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(!o.stdout.contains("pruned"), "{}", o.stdout);
        assert_eq!(fake.agent_options().len(), total + 1);
        assert_eq!(value_listings(&fake), 0, "extra read at {total} options");
        no_unhandled(&fake);
    }
}

/// Acceptance 4: `status` warns at 90 or more `Agent` options, in the text and in `--json`,
/// and not at 89.
#[test]
fn status_warns_at_90_agent_options() {
    let fake = FakeBoard::new();
    let a = fake.agent("dev@fake-a", Role::Dev);
    fill_to(&fake, 89);
    let o = fake.run(&a.dir, &["status", "--json"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["agent_options"]["count"], 89);
    assert_eq!(v["agent_options"]["limit"], 100);
    assert!(v["agent_options"]["warning"].is_null(), "{v}");
    let o = fake.run(&a.dir, &["status"]);
    assert!(!o.stdout.contains("of 100 options"), "{}", o.stdout);

    fill_to(&fake, 90);
    let o = fake.run(&a.dir, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["agent_options"]["count"], 90);
    let w = v["agent_options"]["warning"].as_str().unwrap_or("");
    assert!(w.contains("90 of 100 options"), "{v}");
    let o = fake.run(&a.dir, &["status"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("! the Agent field has 90 of 100 options"),
        "{}",
        o.stdout
    );
    no_unhandled(&fake);
}
