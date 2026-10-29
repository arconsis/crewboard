//! The headless coordinator has its own event cursor and a tick that needs no event
//! (spec/agents#headless-coordinator), against the fake GitHub of
//! `tests/support/headless.rs`.

use crate::support::board::{OWNER, THREAD, sign_ts};
use crate::support::headless::{Hub, Scenario, TICKET};
use crewboard::config::ConfigDir;
use crewboard::protocol::names::{AgentName, Kind};
use serde_json::Value;
use std::path::Path;

const CI_ONLY: &str = "Goal: fix it\nGate: ci\n";
const BY_HAND: &str = "Goal: fix it\nGate: `cargo test` passes and the demo runs\n";
const LEFT: &str = "left for the interactive coordinator";

fn hub(body: &'static str, may_merge: bool) -> Hub {
    Hub::new(Scenario {
        body,
        may_merge,
        ci_green: true,
    })
}

fn board_cursor(h: &Hub) -> String {
    h.board
        .item_field(&h.coord.item_id, "Cursor")
        .expect("the agent item has a Cursor")
}

/// The `local_cursor` of the state cache of a config dir.
fn local_cursor(dir: &Path) -> Option<String> {
    let name = AgentName::parse("claude@fake-a").unwrap();
    let text = std::fs::read_to_string(ConfigDir(dir.to_path_buf()).state_path("fake", &name))
        .unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    v.get("local_cursor")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `poll --json` in `dir`: the parsed last line and the exit code.
fn poll(h: &Hub, dir: &Path) -> (Value, i32) {
    let o = h.run(dir, &["poll", "--json"]);
    let last = o.stdout.lines().last().unwrap_or("").to_string();
    let v: Value = serde_json::from_str(&last)
        .unwrap_or_else(|e| panic!("poll --json is not JSON ({e}): {}", o.all()));
    (v, o.code)
}

/// The kinds of the events of one poll that the dev sent (the `done` of the fixture too).
fn kinds(v: &Value) -> Vec<String> {
    v["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["from"] == "hermes@fake-b")
                .map(|e| e["kind"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A message of the dev to the coordinator, in the thread issue.
fn dev_says(h: &Hub, kind: Kind, body: &str) {
    h.board
        .signed_now(&h.dev.key, &h.dev.name, &h.coord.name, kind, THREAD, body);
}

#[test]
fn a_headless_poll_writes_no_board_cursor_and_moves_only_its_own() {
    let h = hub(CI_ONLY, true);
    // an old message (older than the board cursor) and a new one
    let t = crewboard::clock::now_epoch() - 1200;
    let old = sign_ts(
        &h.dev.key,
        THREAD,
        &h.dev.name,
        &h.coord.name,
        Kind::Info,
        "very old news",
        t,
    );
    h.board.comment_at(THREAD, OWNER, &old, t, t);
    dev_says(&h, Kind::Info, "fresh news");
    let before = board_cursor(&h);
    assert_eq!(local_cursor(h.headless_dir.path()), None);

    let (v, code) = poll(&h, h.headless_dir.path());
    assert_eq!(code, 0, "{v}");
    // the first headless poll starts from the board cursor: no old history
    assert_eq!(kinds(&v), ["done", "info"], "{v}");
    assert!(!v.to_string().contains("very old news"), "{v}");
    // zero writes of any kind (so none to the Cursor field), the board field is unchanged
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    assert_eq!(board_cursor(&h), before);
    // only the local cursor moved
    let local = local_cursor(h.headless_dir.path()).expect("local cursor");
    assert_ne!(local, before);
    // the headless dir does not get the event twice ...
    let (v2, code2) = poll(&h, h.headless_dir.path());
    assert_eq!(code2, 1, "{v2}");
    assert!(h.writes().is_empty());
    // ... and the interactive poll afterwards still gets the same event (and moves the board)
    let (vi, codei) = poll(&h, &h.coord.dir);
    assert_eq!(codei, 0, "{vi}");
    assert_eq!(kinds(&vi), ["done", "info"], "{vi}");
    assert_ne!(board_cursor(&h), before);
    assert_eq!(
        local_cursor(&h.coord.dir),
        None,
        "an interactive dir keeps no local cursor"
    );
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
}

#[test]
fn an_interactive_poll_still_moves_the_board_cursor() {
    // control for the test above: the write exists, so "zero writes" there can fire
    let h = hub(CI_ONLY, true);
    dev_says(&h, Kind::Info, "news");
    let before = board_cursor(&h);
    let (v, code) = poll(&h, &h.coord.dir);
    assert_eq!(code, 0, "{v}");
    assert_ne!(board_cursor(&h), before);
    assert!(!h.writes().is_empty());
}

#[test]
fn a_question_is_never_consumed_by_the_headless_dir() {
    let h = hub(CI_ONLY, true);
    dev_says(&h, Kind::Question, "which of the two?");
    let before = board_cursor(&h);
    let (v, code) = poll(&h, h.headless_dir.path());
    assert_eq!(code, 0, "{v}");
    assert_eq!(
        kinds(&v),
        ["done", "question"],
        "the headless dir sees it: {v}"
    );
    assert_eq!(board_cursor(&h), before);
    // the interactive session still gets the question
    let (vi, codei) = poll(&h, &h.coord.dir);
    assert_eq!(codei, 0, "{vi}");
    assert_eq!(kinds(&vi), ["done", "question"], "{vi}");
    // and a `watch` of the headless dir does not move the board cursor either
    dev_says(&h, Kind::Question, "and a second one?");
    let mid = board_cursor(&h);
    let w = h.headless(&["watch", "--timeout", "1m", "--interval", "5s"]);
    assert_eq!(w.code, 0, "{}", w.all());
    assert_eq!(board_cursor(&h), mid);
}

#[test]
fn the_tick_verifies_and_merges_a_ci_only_review_ticket_without_any_event() {
    let h = hub(CI_ONLY, true);
    // no poll, no watch: the tick does not read events
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert!(o.stdout.contains("merged"), "{}", o.all());
    assert!(h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
    assert!(!o.stdout.contains(LEFT), "{}", o.all());
    assert!(h.board.unhandled().is_empty(), "{:?}", h.board.unhandled());
}

#[test]
fn the_tick_leaves_a_ticket_that_is_not_ci_only_alone() {
    let h = hub(BY_HAND, true);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let lines: Vec<&str> = o.stdout.lines().filter(|l| l.contains(LEFT)).collect();
    assert_eq!(lines.len(), 1, "{}", o.all());
    assert!(lines[0].contains("fake-owner/fake-repo#5"), "{}", lines[0]);
    assert!(h.writes().is_empty(), "{:?}", h.writes());
    assert!(!h.merged());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Review"));
}

#[test]
fn the_tick_is_idempotent() {
    // merged: the second run finds nothing in Review
    let h = hub(CI_ONLY, true);
    assert_eq!(h.headless(&["tick"]).code, 0);
    let writes = h.writes().len();
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(h.writes().len(), writes, "the second tick wrote");

    // verified, but AUTHORITY.md does not allow the merge: the verify is written once
    let h = hub(CI_ONLY, false);
    let first = h.headless(&["tick"]);
    assert_eq!(first.code, 0, "{}", first.all());
    assert!(first.stdout.contains(LEFT), "{}", first.all());
    assert!(first.stdout.contains("authority-merge-not-allowed"));
    let writes = h.writes().len();
    assert!(writes > 0, "the first tick posts the verify result");
    let second = h.headless(&["tick"]);
    assert_eq!(second.code, 0, "{}", second.all());
    assert!(second.stdout.contains(LEFT), "{}", second.all());
    assert_eq!(h.writes().len(), writes, "the second tick wrote");
    assert!(!h.merged());

    // not CI only: zero writes, every run
    let h = hub(BY_HAND, true);
    h.headless(&["tick"]);
    h.headless(&["tick"]);
    assert!(h.writes().is_empty());
}

#[test]
fn the_tick_is_refused_in_an_interactive_config_dir() {
    let h = hub(CI_ONLY, true);
    let o = h.interactive(&["tick"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=tick-headless-only]"),
        "{}",
        o.all()
    );
    assert!(h.writes().is_empty());
    assert!(!h.merged());
    assert_eq!(TICKET, 5);
}

#[test]
fn the_tick_finds_a_review_ticket_that_the_state_cache_does_not_know() {
    let h = hub(CI_ONLY, true);
    // a state cache that says "scanned" and knows no ticket: the index must not decide
    let name = AgentName::parse("claude@fake-a").unwrap();
    let path = ConfigDir(h.headless_dir.path().to_path_buf()).state_path("fake", &name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"tickets_scanned": true, "tickets": {}}"#).unwrap();
    let listings = |h: &Hub| {
        h.requests()
            .iter()
            .filter(|r| r.path.starts_with("/graphql") && r.body.contains("items(first:"))
            .count()
    };
    let before = listings(&h);
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(h.merged(), "{}", o.all());
    assert_eq!(h.status_of_ticket().as_deref(), Some("Done"));
    assert_eq!(listings(&h) - before, 1, "one board listing per tick");
    // a tick with nothing in Review also reads the listing exactly once
    let before = listings(&h);
    let o = h.headless(&["tick"]);
    assert!(o.stdout.contains("no ticket in Review"), "{}", o.all());
    assert_eq!(listings(&h) - before, 1);
}
