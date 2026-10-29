//! Stale work (spec/events#stale-work) end to end: `poll` against a fake GitHub that
//! answers the reads of the markers (branch list, PR list, CI of the head). The unit tests of
//! `src/late/mod.rs` use an injected probe; these tests prove that the request paths and the
//! answer shapes of the real probe (`src/ops/late.rs`) are right. No test sleeps: the limits
//! in the agent config are seconds, the data are older.

use crate::support::late::{LateBoard, SHA1, SHA2};

fn no_push() -> &'static str {
    "stale-work: no-push"
}

#[test]
fn no_push_comes_once_and_not_with_a_branch() {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "60s", "1h", "1h");
    // no branch: exactly one event
    let (v, o) = s.poll(&s.dev);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(LateBoard::stale_events(&v), [no_push()], "{}", o.all());
    assert_eq!(s.count("/git/matching-refs/heads/crew/hermes@fake-b/5-"), 1);
    // AUTHORITY.md (read once by the context), the branch list, the PR list
    assert_eq!(s.stale_reads(), 3);
    let body = LateBoard::stale_body(&v);
    assert!(body.contains("`crew/hermes@fake-b/5-...`"), "{body}");
    // a second poll (throttle lifted) gives none and does not read the branch list again
    s.lift_throttle(&s.dev);
    let reads = s.stale_reads();
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert_eq!(s.count("/git/matching-refs/"), 1);
    assert!(s.stale_reads() >= reads);
    // the throttle: a poll right after reads nothing for the markers
    let reads = s.stale_reads();
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    // one read of AUTHORITY.md where the context loads, none for the markers
    assert_eq!(s.stale_reads(), reads + 1, "a throttled poll read GitHub");
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn no_push_does_not_come_when_the_branch_exists() {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "60s", "1h", "1h");
    s.scene(|sc| sc.branch = true);
    let (v, o) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    // the read happened (a wrong path would also give no event, but then the count is 0)
    assert_eq!(s.count("/git/matching-refs/heads/crew/hermes@fake-b/5-"), 1);
    assert_eq!(
        s.stale_reads(),
        3,
        "AUTHORITY.md, branch list, PR list: {:?}",
        s.requests()
    );
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn green_not_done_comes_once_per_head_sha() {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "1h", "60s", "1h");
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (v, o) = s.poll(&s.dev);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: green-not-done"],
        "{}",
        o.all()
    );
    let body = LateBoard::stale_body(&v);
    assert!(body.contains("#7") && body.contains("1111111"), "{body}");
    assert_eq!(s.count("/pulls?"), 1);
    assert_eq!(s.count("/actions/runs?head_sha=1111"), 1);
    // same head: nothing, no new CI read (the PR list is read to learn the head)
    s.lift_throttle(&s.dev);
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert_eq!(s.count("/pulls?"), 2);
    assert_eq!(s.count("/actions/runs?"), 1);
    // a new head SHA: one new event
    s.scene(|sc| sc.pr_head = Some(SHA2.into()));
    s.lift_throttle(&s.dev);
    let (v, _) = s.poll(&s.dev);
    assert_eq!(LateBoard::stale_events(&v), ["stale-work: green-not-done"]);
    assert_eq!(s.count("/actions/runs?head_sha=2222"), 1);
    // and once only
    s.lift_throttle(&s.dev);
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert_eq!(s.count("/actions/runs?"), 2);
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn green_not_done_does_not_come_while_the_ci_is_younger_than_the_limit() {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "1h", "2h", "1h");
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (v, o) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    assert_eq!(s.count("/actions/runs?"), 1, "the CI was not read");
}

#[test]
fn the_coordinator_gets_review_not_verified_once() {
    let s = LateBoard::new("Review");
    s.limits(&s.coord, "1h", "1h", "60s");
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"],
        "{}",
        o.all()
    );
    // it costs no read of the markers, only the AUTHORITY.md read where the context loads
    assert_eq!(s.stale_reads(), 1, "{:?}", s.requests());
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // control: under the limit, no event (a new fake, default limit 30 min, item 2 min old)
    let t = LateBoard::new("Review");
    let (v, _) = t.poll(&t.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn a_failing_read_gives_no_marker_and_no_failure() {
    let s = LateBoard::new("In Progress");
    s.limits(&s.dev, "60s", "60s", "60s");
    s.scene(|sc| {
        sc.fail = true;
        sc.pr_head = Some(SHA1.into());
    });
    let (v, o) = s.poll(&s.dev);
    assert!(matches!(o.code, 0 | 1), "{}", o.all());
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    assert!(s.stale_reads() >= 2, "the reads were not tried");
    // control: the same data without the failure gives the events
    s.scene(|sc| sc.fail = false);
    s.lift_throttle(&s.dev);
    let (v, _) = s.poll(&s.dev);
    let ev = LateBoard::stale_events(&v);
    let mut ev = ev;
    ev.sort();
    assert_eq!(ev, ["stale-work: green-not-done", no_push()], "{ev:?}");
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}
