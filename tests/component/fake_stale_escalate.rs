//! Escalation of dev markers to the coordinator (spec/events#stale-work)
//! end to end against the fake GitHub. A dev marker (`no-push`, `green-not-done`) that is still
//! there `escalate` after its limit goes to the coordinator as ONE `stale-work` event with the
//! line `escalated: true`. No test sleeps: the limits are seconds, the data are older.

use crate::support::late::{LateBoard, SHA1, SHA2};

const NO_PUSH: &str = "stale-work: no-push";
const GREEN: &str = "stale-work: green-not-done";

/// The coordinator's limits: `no_push` and `green` as given, `escalate`, default `repeat`.
fn coord_limits(s: &LateBoard, no_push: &str, green: &str, escalate: &str) {
    s.limits(&s.coord, no_push, green, "1h");
    s.escalation(&s.coord, escalate, "2h");
}

#[test]
fn a_marker_below_limit_plus_escalate_gives_no_coordinator_event_and_no_read() {
    let s = LateBoard::new("In Progress");
    // the item is 120 s old: limit 60 s + escalate 1 h is far away
    coord_limits(&s, "60s", "60s", "1h");
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    // only the read of AUTHORITY.md where the context loads
    assert_eq!(s.stale_reads(), 1, "{:?}", s.requests());
    // control: with escalate 30 s the same data fire (a filter that cannot fire proves nothing)
    s.escalation(&s.coord, "30s", "2h");
    let (v, _) = s.poll(&s.coord);
    assert!(
        !LateBoard::stale_events(&v).is_empty(),
        "the control never fired"
    );
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn the_default_escalate_is_thirty_minutes() {
    let s = LateBoard::new("In Progress");
    // no `escalate` key: 60 s limit + 30 min default is over the 120 s age
    s.limits(&s.coord, "60s", "60s", "1h");
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    assert_eq!(s.stale_reads(), 1, "only the read of the context");
    // control: only the key moves the line
    s.escalation(&s.coord, "30s", "2h");
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [NO_PUSH]);
}

#[test]
fn above_the_line_exactly_one_event_and_a_second_poll_gives_none() {
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "60s", "1h", "30s");
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(LateBoard::stale_events(&v), [NO_PUSH], "{}", o.all());
    let body = LateBoard::stale_body(&v);
    for want in [
        "\nescalated: true\n",
        "ticket: ",
        "agent: hermes@fake-b",
        "age: ",
        "`say`",
        "`reassign`",
        "`ticket split`",
    ] {
        assert!(body.contains(want), "{want}: {body}");
    }
    let entries = s.seen_entries(&s.coord, "|escalated");
    assert_eq!(entries.len(), 1, "{entries:?}");
    // a second poll (throttle lifted) gives none
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // the throttled poll right after reads nothing
    let reads = s.stale_reads();
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // one read of AUTHORITY.md where the context loads, none for the markers
    assert_eq!(s.stale_reads(), reads + 1, "a throttled poll read GitHub");
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn a_new_head_sha_gives_a_new_event() {
    let s = LateBoard::new("In Progress");
    // no-push is out of reach; the green case is 3600 s old, over 30 s + 30 s
    coord_limits(&s, "10h", "30s", "30s");
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (v, o) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [GREEN], "{}", o.all());
    let body = LateBoard::stale_body(&v);
    assert!(
        body.contains("escalated: true") && body.contains("head: 1111111"),
        "{body}"
    );
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // a new head
    s.scene(|sc| sc.pr_head = Some(SHA2.into()));
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [GREEN]);
    assert!(LateBoard::stale_body(&v).contains("head: 2222222"));
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
}

#[test]
fn after_repeat_one_more_event() {
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "60s", "1h", "30s");
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [NO_PUSH]);
    // 1 h later (repeat is 2 h): nothing
    s.age_escalations(&s.coord, 3600);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(
        LateBoard::stale_events(&v).is_empty(),
        "repeat came too early"
    );
    // 2 h in all: one more
    s.age_escalations(&s.coord, 3700);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [NO_PUSH]);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
}

#[test]
fn a_marker_that_is_gone_gives_no_event_and_its_cache_entry_goes() {
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "60s", "1h", "30s");
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [NO_PUSH]);
    assert_eq!(s.seen_entries(&s.coord, "|escalated").len(), 1);
    // the dev pushed
    s.scene(|sc| sc.branch = true);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert!(s.seen_entries(&s.coord, "|escalated").is_empty());
    // a failing read does not drop an entry: nothing is guessed
    s.scene(|sc| sc.branch = false);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        [NO_PUSH],
        "a new episode is a new event"
    );
    s.scene(|sc| sc.fail = true);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert_eq!(s.seen_entries(&s.coord, "|escalated").len(), 1);
}

#[test]
fn the_headless_tick_logs_one_line_and_writes_nothing() {
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "60s", "1h", "30s");
    let dir = s.headless_copy(&s.coord);
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let lines: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.contains("stale-work escalated"))
        .collect();
    assert_eq!(lines.len(), 1, "{}", o.all());
    assert!(
        lines[0].contains("no-push") && lines[0].contains("hermes@fake-b"),
        "{}",
        lines[0]
    );
    assert!(s.writes().is_empty(), "the tick wrote: {:?}", s.writes());
    // a second tick says nothing more, with and without the throttle
    for lift in [false, true] {
        if lift {
            s.lift_tick_throttle(dir.path(), &s.coord);
        }
        let o = s.run_dir(dir.path(), &["tick"]);
        assert!(!o.stdout.contains("stale-work escalated"), "{}", o.all());
    }
    assert!(s.writes().is_empty(), "{:?}", s.writes());
    // control: the write detector can fire (a poll of the interactive coordinator writes its cursor)
    s.poll(&s.coord);
    assert!(!s.writes().is_empty(), "the write detector sees no write");
    // control: a young ticket gives no line
    let t = LateBoard::new("In Progress");
    t.limits(&t.coord, "60s", "1h", "1h");
    t.escalation(&t.coord, "1h", "2h");
    let d2 = t.headless_copy(&t.coord);
    let o = t.run_dir(d2.path(), &["tick"]);
    assert!(!o.stdout.contains("stale-work escalated"), "{}", o.all());
}

#[test]
fn the_coordinator_read_cost_per_poll_is_one_plus_n_plus_one() {
    // Ticket 5 and two more are old enough; two more were changed just now.
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "60s", "60s", "30s");
    s.more_tickets(&[6, 7], false);
    s.more_tickets(&[8, 9], true);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(LateBoard::stale_events(&v).len(), 3, "{}", o.all());
    // N = 3 candidates: AUTHORITY.md once (the context's copy), one branch list each, one PR
    // list for the repo
    assert_eq!(s.count("/git/matching-refs/"), 3, "{:?}", s.requests());
    assert_eq!(s.count("/pulls?"), 1);
    assert_eq!(s.count("/contents/AUTHORITY.md"), 1);
    assert_eq!(s.stale_reads(), 1 + 3 + 1, "{:?}", s.requests());
    // the next due poll costs the same again, and not more
    s.lift_throttle(&s.coord);
    let before = s.stale_reads();
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert_eq!(s.stale_reads() - before, 1 + 3 + 1);
    // the throttle: a poll right after costs the read of the context only
    let before = s.stale_reads();
    s.poll(&s.coord);
    assert_eq!(s.stale_reads(), before + 1);
}

#[test]
fn a_held_green_marker_reads_the_ci_once_per_head() {
    let s = LateBoard::new("In Progress");
    coord_limits(&s, "10h", "30s", "30s");
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), [GREEN]);
    assert_eq!(s.count("/actions/runs?"), 1);
    let ci = s.ci_reads();
    for _ in 0..2 {
        s.lift_throttle(&s.coord);
        let (v, _) = s.poll(&s.coord);
        assert!(LateBoard::stale_events(&v).is_empty());
    }
    assert_eq!(s.ci_reads(), ci, "the CI of a known head was read again");
    // one PR list per poll
    assert_eq!(s.count("/pulls?"), 3);
}
