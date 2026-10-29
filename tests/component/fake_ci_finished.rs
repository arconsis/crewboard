//! `ci-finished` (spec/events#stale-work) end to end: `poll` against a fake GitHub whose
//! CI of the PR head changes between polls. The tests count the GitHub reads of each poll. No test
//! sleeps: the throttle of the markers is lifted in the state cache.

use crate::support::late::{LateBoard, SHA1, SHA2};

/// A fake with a dev ticket In Progress, an open PR on `SHA1` and limits that keep the markers
/// quiet, so only `ci-finished` can come.
fn ticket() -> LateBoard {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "10h", "10h", "10h");
    s.scene(|sc| {
        sc.branch = true;
        sc.pr_head = Some(SHA1.into());
    });
    s
}

fn again(s: &LateBoard) -> (Vec<String>, serde_json::Value) {
    s.lift_throttle(&s.dev);
    let (v, o) = s.poll(&s.dev);
    assert!(matches!(o.code, 0 | 1), "{}", o.all());
    (LateBoard::ci_events(&v), v)
}

#[test]
fn queued_then_running_then_green_gives_one_green_event_after_the_last_check() {
    let s = ticket();
    // queued
    s.scene(|sc| sc.runs(&[("completed", Some("success")), ("queued", None)]));
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    let first = s.ci_reads();
    // runs, check runs, combined status, status list, branch protection
    assert_eq!(first, 5, "{:?}", s.requests());
    assert_eq!(s.count("/pulls?"), 1);
    // one check completed, the other runs: still nothing, and the head is read again
    s.scene(|sc| sc.runs(&[("completed", Some("success")), ("in_progress", None)]));
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(s.ci_reads(), 2 * first, "one CI read set per poll");
    // the last check completes: exactly one green event
    s.scene(|sc| {
        sc.runs(&[
            ("completed", Some("success")),
            ("completed", Some("success")),
        ])
    });
    let (ev, v) = again(&s);
    assert_eq!(ev, ["ci-finished: green"], "{v}");
    assert_eq!(s.ci_reads(), 3 * first);
    assert_eq!(s.count("/pulls?"), 3);
    let body = &LateBoard::ci_bodies(&v)[0];
    assert!(body.contains(&format!("head: {SHA1}")), "{body}");
    assert!(body.contains("pr: ") && body.contains("#7"), "{body}");
    assert!(
        body.contains(&format!(
            "next: crewboard done 5 --pr https://github.com/fake-owner/fake-repo/pull/7 --sha {SHA1}\n"
        )),
        "{body}"
    );
    // a poll after that says nothing and reads no CI (the PR list is read to learn the head)
    let reads = s.ci_reads();
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(s.ci_reads(), reads, "a head with its event was read again");
    assert_eq!(
        s.count("/pulls?"),
        4,
        "the PR list is read to learn the head"
    );
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn the_throttle_stops_all_reads_between_two_polls() {
    let s = ticket();
    s.scene(|sc| sc.runs(&[("in_progress", None)]));
    let (ev, _) = again(&s);
    assert!(ev.is_empty());
    let (ci, prs, all) = (s.ci_reads(), s.count("/pulls?"), s.requests().len());
    assert!(
        ci >= 3 && prs == 1,
        "the first poll did not read: {ci} {prs}"
    );
    // no lift: the next poll is inside the 60 seconds and reads no marker and no CI
    s.scene(|sc| sc.runs(&[("completed", Some("success"))]));
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::ci_events(&v).is_empty(), "{v}");
    assert_eq!((s.ci_reads(), s.count("/pulls?")), (ci, prs));
    let _ = all;
    // control: after the throttle the same data gives the event
    assert_eq!(again(&s).0, ["ci-finished: green"]);
}

#[test]
fn a_short_ci_is_reported_after_60_seconds_not_after_the_5_minutes_of_the_waits() {
    // Found by a newcomer run: the CI of the PR finished in 10 s, and the event came 4 min later,
    // because the throttle of the late waits (5 min) held it back.
    let s = ticket();
    s.scene(|sc| sc.runs(&[("in_progress", None)]));
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    // the CI finishes; the waits were read a moment ago, so they are not due. The 60 s of
    // `ci-finished` are over:
    s.scene(|sc| sc.runs(&[("completed", Some("success"))]));
    s.lift_ci_throttle(&s.dev);
    let (v, o) = s.poll(&s.dev);
    assert!(matches!(o.code, 0 | 1), "{}", o.all());
    assert_eq!(LateBoard::ci_events(&v), ["ci-finished: green"], "{v}");
    // control: with both throttles in force nothing is read at all
    let t = ticket();
    t.scene(|sc| sc.runs(&[("in_progress", None)]));
    assert!(again(&t).0.is_empty());
    let reads = (t.ci_reads(), t.count("/pulls?"));
    t.scene(|sc| sc.runs(&[("completed", Some("success"))]));
    let (v, _) = t.poll(&t.dev);
    assert!(LateBoard::ci_events(&v).is_empty(), "{v}");
    assert_eq!(
        (t.ci_reads(), t.count("/pulls?")),
        reads,
        "a read inside the 60 seconds"
    );
}

#[test]
fn a_red_ci_gives_a_red_event_that_names_the_failure() {
    let s = ticket();
    s.scene(|sc| {
        sc.runs(&[
            ("completed", Some("failure")),
            ("completed", Some("success")),
        ])
    });
    let (ev, v) = again(&s);
    assert_eq!(ev, ["ci-finished: red"], "{v}");
    let body = &LateBoard::ci_bodies(&v)[0];
    assert!(body.contains("concluded failure"), "{body}");
    assert!(body.contains("Do not run `done`"), "{body}");
    // once
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
}

#[test]
fn a_red_run_next_to_a_running_one_is_not_finished() {
    let s = ticket();
    s.scene(|sc| sc.runs(&[("completed", Some("failure")), ("in_progress", None)]));
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    s.scene(|sc| {
        sc.runs(&[
            ("completed", Some("failure")),
            ("completed", Some("success")),
        ])
    });
    let (ev, _) = again(&s);
    assert_eq!(ev, ["ci-finished: red"]);
}

#[test]
fn a_new_push_gives_a_new_event_for_the_new_head() {
    let s = ticket();
    let (ev, _) = again(&s);
    assert_eq!(ev, ["ci-finished: green"]);
    s.scene(|sc| {
        sc.pr_head = Some(SHA2.into());
        sc.runs(&[("queued", None)]);
    });
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(s.count("/actions/runs?head_sha=2222"), 1);
    s.scene(|sc| sc.runs(&[("completed", Some("failure"))]));
    let (ev, v) = again(&s);
    assert_eq!(ev, ["ci-finished: red"]);
    assert!(LateBoard::ci_bodies(&v)[0].contains(&format!("head: {SHA2}")));
}

#[test]
fn no_ci_gives_none_only_on_the_second_answer() {
    let s = ticket();
    s.scene(|sc| sc.runs.clear());
    // a head that GitHub has not registered yet looks the same: wait one period
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    let (ev, v) = again(&s);
    assert_eq!(ev, ["ci-finished: none"], "{v}");
    assert!(LateBoard::ci_bodies(&v)[0].contains("`CI: none`"));
    let (ev, _) = again(&s);
    assert!(ev.is_empty(), "{ev:?}");
    // control: a run that shows up after the first answer gives its result, never `none`
    let t = ticket();
    t.scene(|sc| sc.runs.clear());
    assert!(again(&t).0.is_empty());
    t.scene(|sc| sc.runs(&[("completed", Some("success"))]));
    assert_eq!(again(&t).0, ["ci-finished: green"]);
}

#[test]
fn no_event_for_a_ticket_that_is_not_in_progress_or_not_owned() {
    // In Review with the agent: no event
    let r = LateBoard::new("Review");
    r.limits(&r.dev, "10h", "10h", "10h");
    r.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (ev, _) = again(&r);
    assert!(ev.is_empty(), "{ev:?}");
    assert_eq!(r.ci_reads(), 0, "{:?}", r.requests());
    // the coordinator gets none for a dev ticket
    let s = ticket();
    s.lift_throttle(&s.coord);
    let (v, o) = s.poll(&s.coord);
    assert!(matches!(o.code, 0 | 1), "{}", o.all());
    assert!(LateBoard::ci_events(&v).is_empty());
    assert_eq!(s.ci_reads(), 0);
    // control: the owner of the same ticket gets the event
    assert_eq!(again(&s).0, ["ci-finished: green"]);
}

#[test]
fn a_failing_read_gives_no_event_and_the_next_poll_does() {
    let s = ticket();
    s.scene(|sc| sc.fail = true);
    let (ev, _) = {
        s.lift_throttle(&s.dev);
        let (v, _) = s.poll(&s.dev);
        (LateBoard::ci_events(&v), v)
    };
    assert!(ev.is_empty(), "{ev:?}");
    s.scene(|sc| sc.fail = false);
    assert_eq!(again(&s).0, ["ci-finished: green"]);
}

#[test]
fn the_entry_goes_when_the_ticket_leaves_the_status() {
    use crewboard::config::ConfigDir;
    use crewboard::protocol::names::AgentName;
    use crewboard::state::StateCache;
    let s = ticket();
    assert_eq!(again(&s).0, ["ci-finished: green"]);
    let cd = ConfigDir(s.dev.dir.clone());
    let p = cd.state_path("fake", &AgentName::parse(&s.dev.name).unwrap());
    let key = "PVTI_ticket|ci-finished";
    assert_eq!(
        StateCache::load(&p).late.late_seen.get(key),
        Some(&SHA1.to_string())
    );
    s.board.set_item("PVTI_ticket", "Status", "Review");
    let _ = again(&s);
    assert!(!StateCache::load(&p).late.late_seen.contains_key(key));
    // back In Progress with the same head: a new event
    s.board.set_item("PVTI_ticket", "Status", "In Progress");
    assert_eq!(again(&s).0, ["ci-finished: green"]);
}
