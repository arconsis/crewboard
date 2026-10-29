//! One table of expected times (spec/events#stale-work) end to end: `poll`
//! and `status` against a fake GitHub, with the `[late]` table of `.crewboard.toml`. No test
//! sleeps: the times are seconds, the data are older. The event bodies of `[stale]` configs are asserted in
//! `fake_stale_work` and `fake_stale_escalate`, which use the `[stale]` table.

use crate::support::board::{REGISTRY, REPO};
use crate::support::late::{HEAD_REF, LateBoard, SHA1, SHA2, TICKET};
use crewboard::clock::{format_ts, now_epoch};
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use serde_json::Value;

/// The times of the push wait for the dev and the coordinator: the item is 120 s old, so it is
/// late (60 s) and the coordinator hears 30 s later.
fn push_times(s: &LateBoard) {
    for who in [&s.dev, &s.coord] {
        s.late_table(
            who,
            &[
                ("push", "60s"),
                ("done", "1h"),
                ("verify", "1h"),
                ("escalate", "30s"),
                ("repeat", "2h"),
            ],
        );
    }
}

fn status_json(s: &LateBoard) -> Value {
    let o = s.run(&s.coord, &["status", "--json"]);
    assert_eq!(o.code, 0, "{}", o.all());
    serde_json::from_str(&o.stdout)
        .or_else(|_| serde_json::from_str(o.stdout.lines().last().unwrap_or("")))
        .unwrap_or_else(|e| panic!("status --json is not JSON ({e}): {}", o.all()))
}

#[test]
fn a_push_wait_goes_to_the_dev_then_to_the_coordinator_and_again_after_repeat() {
    let s = LateBoard::new("In Progress");
    push_times(&s);
    // the dev hears first, with the `stale-work: no-push` first line, without the escalation line
    let (v, o) = s.poll(&s.dev);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: no-push"],
        "{}",
        o.all()
    );
    let body = LateBoard::stale_body(&v);
    assert!(
        body.contains("`crew/hermes@fake-b/5-...`") && !body.contains("escalated"),
        "{body}"
    );
    // the coordinator hears 30 s later: one event with `escalated: true`
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: no-push"],
        "{}",
        o.all()
    );
    let body = LateBoard::stale_body(&v);
    for want in ["\nescalated: true\n", "agent: hermes@fake-b", "`reassign`"] {
        assert!(body.contains(want), "{want}: {body}");
    }
    assert_eq!(s.seen_entries(&s.coord, "|escalated").len(), 1);
    // a second poll (throttle lifted) tells nobody
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // 1 h later (repeat is 2 h): nothing
    s.age_escalations(&s.coord, 3600);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(
        LateBoard::stale_events(&v).is_empty(),
        "repeat came too early"
    );
    // 2 h in all: the coordinator hears again
    s.age_escalations(&s.coord, 3700);
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), ["stale-work: no-push"]);
    // the dev is not told again by any of this
    s.lift_throttle(&s.dev);
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn the_coordinator_does_not_hear_a_push_wait_before_escalate_has_passed() {
    let s = LateBoard::new("In Progress");
    // push 60 s, escalate 1 h: the item is 120 s old, so only the dev hears
    for who in [&s.dev, &s.coord] {
        s.late_table(who, &[("push", "60s"), ("escalate", "1h")]);
    }
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // only the read of AUTHORITY.md where the context loads
    assert_eq!(s.stale_reads(), 1, "{:?}", s.requests());
    let (v, _) = s.poll(&s.dev);
    assert_eq!(LateBoard::stale_events(&v), ["stale-work: no-push"]);
}

#[test]
fn a_review_that_is_too_old_goes_to_the_coordinator_only() {
    let s = LateBoard::new("Review");
    for who in [&s.dev, &s.coord] {
        s.late_table(who, &[("verify", "60s")]);
    }
    let (v, o) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"],
        "{}",
        o.all()
    );
    assert!(!LateBoard::stale_body(&v).contains("escalated"));
    // once
    s.lift_throttle(&s.coord);
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn take_is_off_until_the_table_gives_a_time_then_the_dev_and_later_the_coordinator_hear() {
    let s = LateBoard::new("Ready");
    // off by default: neither hears, however old the ticket is
    let (v, _) = s.poll(&s.dev);
    assert!(LateBoard::stale_events(&v).is_empty());
    let (v, _) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty());
    // the table gives a time: the dev hears `late: take` with the ticket line
    for who in [&s.dev, &s.coord] {
        s.late_table(who, &[("take", "60s"), ("escalate", "30s")]);
    }
    let (v, o) = s.poll(&s.dev);
    assert_eq!(LateBoard::stale_events(&v), ["late: take"], "{}", o.all());
    let body = LateBoard::stale_body(&v);
    assert!(
        body.contains("\nticket: ") && body.contains("`take`"),
        "{body}"
    );
    assert!(!body.contains("escalated"));
    // the coordinator hears the escalated form
    let (v, o) = s.poll(&s.coord);
    assert_eq!(LateBoard::stale_events(&v), ["late: take"], "{}", o.all());
    assert!(LateBoard::stale_body(&v).contains("\nescalated: true\n"));
    // and nobody twice
    for who in [&s.dev, &s.coord] {
        let (v, _) = s.poll(who);
        assert!(LateBoard::stale_events(&v).is_empty());
    }
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn status_prints_the_late_section_with_since_age_expected_and_who_is_told() {
    let s = LateBoard::new("In Progress");
    push_times(&s);
    let before = now_epoch();
    let o = s.run(&s.coord, &["status"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let line = o
        .stdout
        .lines()
        .find(|l| l.trim_start().starts_with("push "))
        .unwrap_or_else(|| panic!("no push line: {}", o.all()));
    for want in [
        "#5",
        "since 20",
        "age ",
        "expected 60s",
        "told: dev hermes@fake-b, coordinator",
    ] {
        assert!(line.contains(want), "{want}: {line}");
    }
    assert!(o.stdout.contains("\nlate: 1 ("), "{}", o.stdout);
    assert!(!o.stdout.contains("stale work"), "{}", o.stdout);
    // json: the same fact, with since as a time
    let v = status_json(&s);
    let late = v["late"].as_array().expect("late list");
    assert_eq!(late.len(), 1, "{v}");
    assert_eq!(late[0]["wait"], "push");
    assert_eq!(late[0]["expected_s"], 60);
    assert!(late[0]["age_s"].as_i64().unwrap() >= 120);
    let since = late[0]["since"].as_str().unwrap();
    assert!(since <= format_ts(before - 100).as_str(), "{since}");
    assert_eq!(late[0]["told"][0], "dev hermes@fake-b");
    assert_eq!(late[0]["told"][1], "coordinator");
    // control: with the default 2 h the same ticket is not late
    let t = LateBoard::new("In Progress");
    let v = status_json(&t);
    assert_eq!(v["late"].as_array().map(Vec::len), Some(0), "{v}");
    let o = t.run(&t.coord, &["status"]);
    assert!(o.stdout.contains("\nlate: 0 ("), "{}", o.stdout);
}

#[test]
fn a_cache_with_the_old_stale_names_is_read_and_nothing_is_sent_again() {
    let s = LateBoard::new("In Progress");
    for who in [&s.dev, &s.coord] {
        s.late_table(who, &[("push", "60s"), ("escalate", "30s")]);
    }
    let (v, _) = s.poll(&s.dev);
    assert_eq!(LateBoard::stale_events(&v), ["stale-work: no-push"]);
    // write the file with the `stale_*` key names and the throttle open
    let path = s.state_file(&s.dev);
    let mut j: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let obj = j.as_object_mut().unwrap();
    let seen = obj.remove("late_seen").expect("the poll stored the wait");
    assert_eq!(seen.as_object().unwrap().len(), 1, "{seen}");
    obj.remove("late_checked");
    obj.insert("stale_seen".into(), seen);
    obj.insert("stale_checked".into(), Value::from(0));
    std::fs::write(&path, serde_json::to_string(&j).unwrap()).unwrap();
    let (v, o) = s.poll(&s.dev);
    assert!(
        LateBoard::stale_events(&v).is_empty(),
        "resent: {}",
        o.all()
    );
    // the branch list was read again (the throttle was open), and the file has the new names
    assert_eq!(
        s.count("/git/matching-refs/"),
        1,
        "the dev wait is not read again"
    );
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(after.get("late_seen").is_some() && after.get("stale_seen").is_none());
    // control: without the old entry the same poll sends it again
    let mut j = after;
    j.as_object_mut().unwrap().remove("late_seen");
    j.as_object_mut().unwrap().remove("late_checked");
    std::fs::write(&path, serde_json::to_string(&j).unwrap()).unwrap();
    let (v, _) = s.poll(&s.dev);
    assert_eq!(LateBoard::stale_events(&v), ["stale-work: no-push"]);
}

#[test]
fn poll_and_tick_keep_their_own_read_throttle_in_one_cache() {
    let s = LateBoard::new("In Progress");
    push_times(&s);
    let dir = s.headless_copy(&s.coord);
    // the tick reads first and logs its line
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("stale-work escalated: no-push"),
        "{}",
        o.all()
    );
    // a poll right after must still read GitHub and give the escalated event: the tick must not
    // use up its throttle
    let o = s.run_dir(dir.path(), &["poll", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.lines().last().unwrap_or("")).unwrap();
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: no-push"],
        "{}",
        o.all()
    );
    assert!(LateBoard::stale_body(&v).contains("\nescalated: true\n"));
    // both stored their own time
    let name = crewboard::protocol::names::AgentName::parse(&s.coord.name).unwrap();
    let p = crewboard::config::ConfigDir(dir.path().to_path_buf()).state_path("fake", &name);
    let c = crewboard::state::StateCache::load(&p);
    assert!(c.late.late_checked.is_some() && c.late.late_tick_checked.is_some());
    // and a second tick within the throttle reads nothing for the tick
    let reads = s.stale_reads();
    s.run_dir(dir.path(), &["tick"]);
    // the tick reads AUTHORITY.md once where its context loads, and nothing for the markers
    assert_eq!(s.stale_reads(), reads + 1, "a throttled tick read GitHub");
}

/// The notes to the owner on an issue: the signed `info` comments that carry a `late-key:` line.
fn late_notes(s: &LateBoard, issue: u64) -> Vec<String> {
    s.board
        .comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains("\nlate-key: "))
        .collect()
}

/// The coordinator wrote its last heartbeat a day ago.
fn coordinator_away(s: &LateBoard) {
    s.board.set_item(
        &s.coord.item_id,
        "Heartbeat",
        &format_ts(now_epoch() - 86_400),
    );
}

#[test]
fn a_live_coordinator_hears_an_old_review_and_the_owner_hears_nothing() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"],
        "{}",
        o.all()
    );
    assert!(
        late_notes(&s, TICKET).is_empty(),
        "{:?}",
        late_notes(&s, TICKET)
    );
    assert!(late_notes(&s, REGISTRY).is_empty());
    let o = s.run(&s.coord, &["status"]);
    assert!(
        o.stdout.contains("told: coordinator\n"),
        "the owner is not told yet: {}",
        o.all()
    );
}

#[test]
fn tick_tells_the_owner_once_of_an_old_review_while_the_coordinator_is_away() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    coordinator_away(&s);
    let dir = s.headless_copy(&s.coord);
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    // the log line says what was done
    assert!(
        o.stdout
            .contains("late: verify; the interactive coordinator is late; told the owner"),
        "{}",
        o.all()
    );
    // ONE signed info to the owner on the ticket, with the key, the time and the next step
    let n = late_notes(&s, TICKET);
    assert_eq!(n.len(), 1, "{n:?}");
    let note = &n[0];
    assert!(
        note.contains("from=claude@fake-a to=owner kind=info"),
        "{note}"
    );
    for part in [
        "late: verify ",
        "#5\nlate-key: verify:",
        "\nsince: 20",
        "\nnext: the interactive coordinator verifies, or the owner decides\n",
        "coordinator heartbeat: ",
        " (late)\n",
    ] {
        assert!(note.contains(part), "{part:?} in {note}");
    }
    // no Status change
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("Review")
    );
    // a second cycle posts nothing, from the tick or from a dev agent
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    s.poll(&s.dev);
    assert_eq!(late_notes(&s, TICKET).len(), 1, "{}", o.all());
    // `status` names the owner as told
    let o = s.run(&s.coord, &["status"]);
    let line = o
        .stdout
        .lines()
        .find(|l| l.trim_start().starts_with("verify "))
        .unwrap_or_else(|| panic!("no verify line: {}", o.all()));
    assert!(line.contains("told: coordinator, owner"), "{line}");
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn a_fresh_coordinator_or_a_young_review_posts_no_owner_note() {
    // the coordinator is alive: no note, however old the ticket
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    let dir = s.headless_copy(&s.coord);
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(late_notes(&s, TICKET).is_empty(), "{}", o.all());
    // the coordinator is away but the ticket is young (limit 1 h, age 120 s): no note
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "1h")]);
    coordinator_away(&s);
    let dir = s.headless_copy(&s.coord);
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(late_notes(&s, TICKET).is_empty(), "{}", o.all());
}

#[test]
fn a_dev_agent_tells_the_owner_once_that_the_coordinator_is_away() {
    let s = LateBoard::new("In Progress");
    coordinator_away(&s);
    let (_, o) = s.poll(&s.dev);
    assert_eq!(o.code, 0, "{}", o.all());
    let n = late_notes(&s, REGISTRY);
    assert_eq!(n.len(), 1, "{n:?}\n{}", o.all());
    let note = &n[0];
    assert!(
        note.contains("from=hermes@fake-b to=owner kind=info"),
        "{note}"
    );
    for part in [
        "late: heartbeat claude@fake-a\nlate-key: heartbeat:claude@fake-a:",
        "\nsince: 20",
        "\nnext: restart the coordinator session, or approve another agent",
        "shows no sign of life",
    ] {
        assert!(note.contains(part), "{part:?} in {note}");
    }
    // the model is not woken: no event, and a second poll posts nothing
    assert!(
        o.stderr.contains("told the owner"),
        "one log line: {}",
        o.all()
    );
    let (v, _) = s.poll(&s.dev);
    assert!(v["events"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(late_notes(&s, REGISTRY).len(), 1);
    // `status` lists the heartbeat wait and names the owner
    let o = s.run(&s.dev, &["status"]);
    let line = o
        .stdout
        .lines()
        .find(|l| l.trim_start().starts_with("heartbeat "))
        .unwrap_or_else(|| panic!("no heartbeat line: {}", o.all()));
    assert!(
        line.contains("claude@fake-a")
            && line.contains("expected 30m")
            && line.contains("told: owner"),
        "{line}"
    );
    // the coordinator itself never posts about itself
    s.poll(&s.coord);
    assert_eq!(late_notes(&s, REGISTRY).len(), 1);
    // control: a live coordinator gets no note
    let t = LateBoard::new("In Progress");
    t.poll(&t.dev);
    assert!(late_notes(&t, REGISTRY).is_empty());
}

#[test]
fn a_dead_agent_gets_its_tickets_back_in_ready_and_the_owner_hears_once() {
    let s = LateBoard::new("In Progress");
    // the dev wrote its last heartbeat a day ago; nobody pinged it: the heartbeat age alone
    // makes it dead
    let hb = now_epoch() - 86_400;
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(hb));
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    // one `agent-dead` event for the model
    let dead: Vec<&Value> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "agent-dead")
        .collect();
    assert_eq!(dead.len(), 1, "{v}");
    assert!(!dead[0]["body"].as_str().unwrap().contains("Needs Human"));
    // the ticket is back in Backlog, without an agent (a ticket in Ready has an agent)
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("Backlog")
    );
    // the registry holds ONE report to all and ONE late note to the owner, keyed by the heartbeat
    let reg: Vec<String> = s
        .board
        .comments_of(REGISTRY)
        .into_iter()
        .map(|(_, _, b)| b)
        .collect();
    let reports = reg
        .iter()
        .filter(|b| b.contains("to=all kind=report") && b.contains("agent-dead: hermes@fake-b"))
        .count();
    assert_eq!(reports, 1, "{reg:?}");
    let n = late_notes(&s, REGISTRY);
    assert_eq!(n.len(), 1, "{n:?}");
    for part in [
        "from=claude@fake-a to=owner kind=info",
        "late: dead hermes@fake-b\n",
        &format!("late-key: dead:hermes@fake-b:{hb}\n"),
        "\nnext: restart the agent session",
        "agent-dead: hermes@fake-b\n",
        "tickets returned to Backlog (assign them again): ",
    ] {
        assert!(n[0].contains(part), "{part:?} in {}", n[0]);
    }
    // a second poll posts nothing and says nothing
    let (v, _) = s.poll(&s.coord);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["kind"] != "agent-dead"),
        "{v}"
    );
    assert_eq!(late_notes(&s, REGISTRY).len(), 1);
    // no ping was ever sent
    assert!(
        s.board
            .comments_of(REGISTRY)
            .iter()
            .all(|(_, _, b)| !b.contains("kind=ping")),
        "the coordinator sent a ping"
    );
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

/// The issue numbers of the board items `moved` (`PVTI_ticket` is the issue 5).
fn backlog_numbers(moved: &[&str]) -> Vec<u64> {
    moved
        .iter()
        .map(|i| match *i {
            "PVTI_ticket" => TICKET,
            other => other.trim_start_matches("PVTI_t").parse().unwrap(),
        })
        .collect()
}

#[test]
fn a_failed_write_for_one_ticket_of_a_dead_agent_keeps_the_report_of_the_others() {
    let s = LateBoard::new("In Progress");
    s.more_tickets(&[6, 7], false);
    let hb = now_epoch() - 86_400;
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(hb));
    // the first return to Backlog works, the next ones fail
    s.board.fail_select_write_after(Some("Backlog"), 1);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    let backlog = |item: &str| s.board.item_field(item, "Status").as_deref() == Some("Backlog");
    let items = ["PVTI_ticket", "PVTI_t6", "PVTI_t7"];
    let moved: Vec<&str> = items.iter().copied().filter(|i| backlog(i)).collect();
    assert_eq!(moved.len(), 1, "{moved:?}");
    // the report and the event name the ticket that moved and the ones that did not
    let dead: Vec<&Value> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "agent-dead")
        .collect();
    assert_eq!(dead.len(), 1, "{v}");
    let body = dead[0]["body"].as_str().unwrap();
    assert!(
        body.contains("tickets returned to Backlog (assign them again): ")
            && !body.contains("returned to Backlog (assign them again): none")
            && body.contains("tickets that could not be returned (the next cycle tries again): "),
        "{body}"
    );
    // every ticket is in exactly one of the two lists
    let moved_n = backlog_numbers(&moved);
    for n in [5, 6, 7] {
        let (ok, bad) = body.split_once("could not be returned").unwrap();
        assert_eq!(
            (
                ok.contains(&format!("#{n}")),
                bad.contains(&format!("#{n}"))
            ),
            (moved_n.contains(&n), !moved_n.contains(&n)),
            "#{n} in {body}"
        );
    }
    let reports = s
        .board
        .comments_of(REGISTRY)
        .into_iter()
        .filter(|(_, _, b)| b.contains("to=all kind=report") && b.contains("agent-dead:"))
        .count();
    assert_eq!(reports, 1);
}

/// The dead dev agent of `LateBoard`, its ticket in `status` as `(Status, Agent)` of the board.
fn ticket_now(s: &LateBoard) -> (Option<String>, Option<String>) {
    (
        s.board.item_field("PVTI_ticket", "Status"),
        s.board.item_field("PVTI_ticket", "Agent"),
    )
}

fn dead_dev(s: &LateBoard) {
    let hb = now_epoch() - 86_400;
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(hb));
}

#[test]
fn a_failed_status_write_leaves_the_ticket_with_its_agent_and_the_next_cycle_returns_it() {
    let s = LateBoard::new("In Progress");
    dead_dev(&s);
    s.board.fail_select_write(Some("Backlog"));
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    // the Status did not move, so the Agent must still be there: the ticket is not orphaned
    assert_eq!(
        ticket_now(&s),
        (Some("In Progress".into()), Some("hermes@fake-b".into()))
    );
    let body = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "agent-dead");
    assert!(
        body.is_some_and(|e| e["body"]
            .as_str()
            .unwrap()
            .contains("could not be returned")),
        "{v}"
    );
    s.board.fail_select_write(None);
    let (_, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(ticket_now(&s), (Some("Backlog".into()), None));
}

#[test]
fn a_failed_agent_clear_leaves_backlog_with_the_agent_and_the_next_cycle_clears_it() {
    let s = LateBoard::new("In Progress");
    dead_dev(&s);
    s.board.fail_clear_write(true);
    let (_, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        ticket_now(&s),
        (Some("Backlog".into()), Some("hermes@fake-b".into()))
    );
    let notices = |s: &LateBoard| {
        s.board
            .comments_of(TICKET)
            .iter()
            .filter(|(_, _, b)| b.contains("Reassigned."))
            .count()
    };
    assert_eq!(notices(&s), 1);
    s.board.fail_clear_write(false);
    let (_, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(ticket_now(&s), (Some("Backlog".into()), None));
    // the second cycle only cleared the Agent: no second notice
    assert_eq!(notices(&s), 1);
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn a_dead_agents_open_pr_is_named_in_the_notice_and_the_report_and_a_failed_read_blocks_nothing() {
    let dead = |fail: bool| {
        let s = LateBoard::new("In Progress");
        s.scene(|sc| {
            sc.pr_head = Some(SHA1.to_string());
            sc.fail = fail;
        });
        s.board.set_item(
            &s.dev.item_id,
            "Heartbeat",
            &format_ts(now_epoch() - 86_400),
        );
        let (v, o) = s.poll(&s.coord);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(
            s.board.item_field("PVTI_ticket", "Status").as_deref(),
            Some("Backlog")
        );
        (s, v, o)
    };
    let (s, v, _) = dead(false);
    let url = format!("https://github.com/{REPO}/pull/7");
    let notice = s
        .board
        .comments_of(TICKET)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("Reassigned. Reason:"))
        .expect("notice");
    assert!(
        notice.contains(&format!("open-pr: {url} branch: {HEAD_REF}")),
        "{notice}"
    );
    let ev = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "agent-dead")
        .expect("agent-dead");
    assert!(
        ev["body"]
            .as_str()
            .unwrap()
            .contains(&format!("(earlier PR {url})")),
        "{ev}"
    );
    // the read of the PR list fails: the ticket is returned all the same, without a PR line
    let (s, _, o) = dead(true);
    assert!(o.stderr.contains("was not read"), "{}", o.all());
    let notice = s
        .board
        .comments_of(TICKET)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("Reassigned. Reason:"))
        .expect("notice");
    assert!(!notice.contains("open-pr:"), "{notice}");
}

#[test]
fn a_second_stale_period_of_the_coordinator_gives_a_second_review_note() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    coordinator_away(&s);
    let dir = s.headless_copy(&s.coord);
    s.run_dir(dir.path(), &["tick"]);
    assert_eq!(late_notes(&s, TICKET).len(), 1);
    // the same period again: nothing more
    s.run_dir(dir.path(), &["tick"]);
    assert_eq!(late_notes(&s, TICKET).len(), 1);
    // the coordinator came back, wrote a heartbeat, and went away again; the Review start is
    // the same: a new note with another key
    s.board.set_item(
        &s.coord.item_id,
        "Heartbeat",
        &format_ts(now_epoch() - 43_200),
    );
    s.run_dir(dir.path(), &["tick"]);
    let n = late_notes(&s, TICKET);
    assert_eq!(n.len(), 2, "{n:?}");
    let key = |b: &str| {
        b.lines()
            .find(|l| l.starts_with("late-key: "))
            .unwrap()
            .to_string()
    };
    assert_ne!(key(&n[0]), key(&n[1]));
    s.run_dir(dir.path(), &["tick"]);
    assert_eq!(late_notes(&s, TICKET).len(), 2);
}

#[test]
fn a_coordinator_poll_never_counts_itself_as_late_and_reads_nothing_for_it() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    coordinator_away(&s);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        late_notes(&s, TICKET).is_empty(),
        "{:?}",
        late_notes(&s, TICKET)
    );
    assert!(late_notes(&s, REGISTRY).is_empty());
    // the same board with a live coordinator costs the same reads: the owner check adds none
    let t = LateBoard::new("Review");
    t.late_table(&t.coord, &[("verify", "60s")]);
    t.poll(&t.coord);
    let reads = |s: &LateBoard| s.count(&format!("/issues/{TICKET}/comments"));
    assert_eq!(reads(&s), reads(&t));
    // it still hears the wait as an event
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"]
    );
}

/// A board with one ticket in `status` and a signed message of the dev on it, and the `[late]`
/// table `pairs` for the coordinator.
fn waiting(status: &str, kind: Kind, to: &str, body: &str, pairs: &[(&str, &str)]) -> LateBoard {
    let s = LateBoard::new(status);
    s.board
        .signed(&s.dev.key, &s.dev.name, to, kind, TICKET, body);
    s.late_table(&s.coord, pairs);
    s
}

/// Like [`waiting`], with no `[late]` table, a message from two days ago and a ticket item that
/// is two days old: only the defaults decide.
fn waiting_old(
    status: &str,
    kind: Kind,
    to: &str,
    body: &str,
    pairs: &[(&str, &str)],
) -> LateBoard {
    let s = LateBoard::new(status);
    let two_days = 2 * 86_400;
    s.board
        .signed_ago(&s.dev.key, &s.dev.name, to, kind, TICKET, body, two_days);
    s.board.age_item("PVTI_ticket", two_days);
    if !pairs.is_empty() {
        s.late_table(&s.coord, pairs);
    }
    s
}

/// The coordinator's `poll` tells the owner once of `wait`; a second cycle posts nothing.
fn assert_polled_note_once(s: &LateBoard, wait: &str) {
    s.poll(&s.coord);
    let n = late_notes(s, TICKET);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(
        n[0].contains("from=claude@fake-a to=owner kind=info")
            && n[0].contains(&format!("late: {wait} "))
            && n[0].contains(&format!("\nlate-key: {wait}:"))
            && n[0].contains("\nnext: "),
        "{}",
        n[0]
    );
    s.lift_throttle(&s.coord);
    s.poll(&s.coord);
    assert_eq!(late_notes(s, TICKET).len(), 1);
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn the_coordinator_poll_tells_the_owner_once_of_a_question_that_waits_for_the_owner() {
    let s = waiting(
        "Needs Human",
        Kind::Question,
        "owner",
        "Which way?",
        &[("owner", "10s")],
    );
    assert_polled_note_once(&s, "owner");
    // no key: the default of 1 day is on, so an old question reaches the owner once
    let d = waiting_old("Needs Human", Kind::Question, "owner", "Which way?", &[]);
    assert_polled_note_once(&d, "owner");
    // the key is off: nothing
    let t = waiting_old(
        "Needs Human",
        Kind::Question,
        "owner",
        "Which way?",
        &[("owner", "off"), ("answer", "off")],
    );
    t.poll(&t.coord);
    assert!(late_notes(&t, TICKET).is_empty());
    // the wait and the note share one read of the ticket comments per due cycle
    let reads = |s: &LateBoard| {
        s.requests()
            .iter()
            .filter(|r| r.method == "GET" && r.path.contains(&format!("/issues/{TICKET}/comments")))
            .count()
    };
    let u = waiting_old(
        "Needs Human",
        Kind::Question,
        "owner",
        "Which way?",
        &[("owner", "10s"), ("answer", "off")],
    );
    u.poll(&u.coord);
    assert_eq!(reads(&u), reads(&t) + 1);
}

/// A `done` body with complete evidence (the tick and the late scan read the `done` of the
/// assignee, and only one with complete evidence counts) and `extra` lines.
fn done_with(extra: &str) -> String {
    format!("done\npr: https://x/pull/7\nsha: {SHA1}\nci: none\n{extra}")
}

#[test]
fn the_coordinator_poll_tells_the_owner_once_of_a_review_that_waits_for_an_owner_merge() {
    let s = waiting(
        "Review",
        Kind::Done,
        "coordinator",
        &done_with("protected-paths: AUTHORITY.md\n"),
        &[("owner", "10s")],
    );
    assert_polled_note_once(&s, "owner-merge");
    // no key: the default of 1 day is on
    let d = waiting_old(
        "Review",
        Kind::Done,
        "coordinator",
        &done_with("protected-paths: AUTHORITY.md\n"),
        &[],
    );
    assert_polled_note_once(&d, "owner-merge");
    // the key is off: nothing
    let t = waiting_old(
        "Review",
        Kind::Done,
        "coordinator",
        &done_with("protected-paths: AUTHORITY.md\n"),
        &[("owner", "off")],
    );
    t.poll(&t.coord);
    assert!(late_notes(&t, TICKET).is_empty());
    // a done without such a line is no owner merge
    let u = waiting(
        "Review",
        Kind::Done,
        "coordinator",
        &done_with(""),
        &[("owner", "10s")],
    );
    u.poll(&u.coord);
    assert!(late_notes(&u, TICKET).is_empty());
}

#[test]
fn the_coordinator_poll_tells_the_owner_once_of_a_question_without_an_answer() {
    let s = waiting(
        "Blocked",
        Kind::Blocked,
        "coordinator",
        "Stuck.",
        &[("answer", "10s"), ("escalate", "10s")],
    );
    assert_polled_note_once(&s, "answer");
    // before `answer + escalate` only the coordinator hears
    let t = waiting(
        "Blocked",
        Kind::Blocked,
        "coordinator",
        "Stuck.",
        &[("answer", "10s"), ("escalate", "1h")],
    );
    t.poll(&t.coord);
    assert!(late_notes(&t, TICKET).is_empty());
    // no key: the default of 4 hours is on
    let d = waiting_old("Blocked", Kind::Blocked, "coordinator", "Stuck.", &[]);
    assert_polled_note_once(&d, "answer");
    // the key is off: nothing
    let u = waiting_old(
        "Blocked",
        Kind::Blocked,
        "coordinator",
        "Stuck.",
        &[("answer", "off")],
    );
    u.poll(&u.coord);
    assert!(late_notes(&u, TICKET).is_empty());
}

#[test]
fn a_plain_comment_with_the_same_key_text_does_not_stop_the_note() {
    let s = LateBoard::new("In Progress");
    let t = now_epoch() - 86_400;
    s.board
        .set_item(&s.coord.item_id, "Heartbeat", &format_ts(t));
    s.board.owner_comment(
        REGISTRY,
        &format!("late: heartbeat claude@fake-a\nlate-key: heartbeat:claude@fake-a:{t}\n"),
    );
    s.poll(&s.dev);
    let mine = |s: &LateBoard| {
        late_notes(s, REGISTRY)
            .iter()
            .filter(|b| b.contains("from=hermes@fake-b to=owner kind=info"))
            .count()
    };
    assert_eq!(mine(&s), 1, "{:?}", late_notes(&s, REGISTRY));
    s.poll(&s.dev);
    assert_eq!(mine(&s), 1);
}

#[test]
fn a_dev_agent_does_not_tell_the_owner_of_another_dev_agent() {
    let s = LateBoard::new("In Progress");
    let other = s.board.agent("kimi@fake-c", Role::Dev);
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 2_400));
    // kimi sees that hermes is late (40 min, over 3 intervals, under 5) and the coordinator is alive: no note
    s.poll(&other);
    assert!(
        late_notes(&s, REGISTRY).is_empty(),
        "{:?}",
        late_notes(&s, REGISTRY)
    );
    // `status` lists the late dev and tells nobody
    let o = s.run(&other, &["status"]);
    let line = o
        .stdout
        .lines()
        .find(|l| l.trim_start().starts_with("heartbeat "))
        .unwrap_or_else(|| panic!("no heartbeat line: {}", o.all()));
    assert!(
        line.contains("hermes@fake-b") && line.contains("nobody yet"),
        "{line}"
    );
}

/// Every `kind=ping` or `kind=pong` comment in the registry issue.
fn liveness_traffic(s: &LateBoard) -> usize {
    s.board
        .comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, b)| b.contains("kind=ping") || b.contains("kind=pong"))
        .count()
}

fn events_of(v: &Value, kind: &str) -> usize {
    v["events"]
        .as_array()
        .map_or(0, |a| a.iter().filter(|e| e["kind"] == kind).count())
}

#[test]
fn a_silent_agent_goes_late_then_dead_and_its_tickets_return_without_any_ping() {
    let s = LateBoard::new("In Progress");
    let status = |s: &LateBoard| s.board.item_field("PVTI_ticket", "Status");
    // alive: a heartbeat 20 min old (interval 10 min: late after 30 min, dead after 50 min)
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 1_200));
    let (v, _) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v}");
    assert_eq!(status(&s).as_deref(), Some("In Progress"));
    // late: 40 min. The coordinator only shows it: nothing is sent, nothing moves
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 2_400));
    let (v, o) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v} {}", o.all());
    assert_eq!(status(&s).as_deref(), Some("In Progress"), "{}", o.all());
    assert_eq!(liveness_traffic(&s), 0, "a late agent is not pinged");
    assert!(late_notes(&s, REGISTRY).is_empty());
    // dead: 60 min. The ticket returns to Backlog, with no ping before
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 3_600));
    let (v, o) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 1, "{v} {}", o.all());
    assert_eq!(status(&s).as_deref(), Some("Backlog"));
    assert_eq!(late_notes(&s, REGISTRY).len(), 1);
    assert_eq!(liveness_traffic(&s), 0, "no ping on the way to dead");
}

const OVERRIDE_ENV: &str = "CREWBOARD_HEARTBEAT_INTERVAL";
const TEST_ENV: &str = "CREWBOARD_TEST_HEARTBEAT_INTERVAL";

#[test]
fn the_old_override_variable_is_ignored() {
    let s = LateBoard::new("In Progress");
    let status = |s: &LateBoard| s.board.item_field("PVTI_ticket", "Status");
    // the dev wrote its last heartbeat 40 min ago: late (not dead) at the board value of 10 min
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 2_400));
    // the coordinator has a test value of 20 s in its env file: with it the dev would be long
    // dead and its ticket would return to Backlog (F14). Now the board value judges.
    let (v, o) = s.poll_env(&s.coord, &[(OVERRIDE_ENV, "20s")]);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v} {}", o.all());
    assert_eq!(status(&s).as_deref(), Some("In Progress"), "{}", o.all());
    assert!(
        !o.stderr.contains("differs from the board value"),
        "{}",
        o.all()
    );
    // control: the same data is dead for the hidden test variable (it sets the judging value)
    let (v, o) = s.poll_env(&s.coord, &[(TEST_ENV, "20s")]);
    assert_eq!(events_of(&v, "agent-dead"), 1, "{v} {}", o.all());
    assert_eq!(status(&s).as_deref(), Some("Backlog"));
    assert!(
        !o.stderr.contains("differs from the board value"),
        "{}",
        o.all()
    );
}

#[test]
fn status_shows_the_board_interval_and_the_override_variable_changes_nothing() {
    let s = LateBoard::new("In Progress");
    let cwd = std::env::temp_dir();
    let interval = |env: &[(&str, &str)]| {
        let o = s
            .board
            .run_env(&s.coord.dir, &cwd, &["status", "--json"], env);
        assert_eq!(o.code, 0, "{}", o.all());
        let v: Value = serde_json::from_str(o.stdout.lines().last().unwrap_or(""))
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", o.all()));
        (v["heartbeat_interval_s"].as_i64().unwrap(), o)
    };
    // the old variable changes nothing and warns about nothing
    let (n, o) = interval(&[(OVERRIDE_ENV, "20s")]);
    assert_eq!(n, 600, "{}", o.all());
    assert!(!o.stderr.contains("differs"), "{}", o.all());
    let (n, o) = interval(&[]);
    assert_eq!(n, 600);
    assert!(!o.stderr.contains("differs"), "{}", o.all());
    // the flag is gone
    let o = s.board.run_env(
        &s.coord.dir,
        &cwd,
        &["--heartbeat-interval", "20s", "status", "--json"],
        &[],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("unexpected argument"), "{}", o.all());
    // the hidden test variable sets the value and warns about nothing
    let (n, o) = interval(&[(TEST_ENV, "20s")]);
    assert_eq!(n, 20);
    assert!(!o.stderr.contains("differs"), "{}", o.all());
}

#[test]
fn an_old_ping_on_the_board_is_ignored_and_not_answered() {
    let s = LateBoard::new("In Progress");
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 60));
    // a ping of an old coordinator, 5 min old
    s.board.signed_ago(
        &s.coord.key,
        &s.coord.name,
        &s.dev.name,
        Kind::Ping,
        REGISTRY,
        "ping: hermes@fake-b has written no heartbeat. Answer with a pong.",
        300,
    );
    let before = s.board.comments_of(REGISTRY).len();
    let (v, o) = s.poll(&s.dev);
    // no event for the model about the ping (the one event is the assignment), no pong, no post
    let kinds: Vec<&str> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["assignment"], "{v} {}", o.all());
    assert!(!o.stderr.contains("pong"), "{}", o.stderr);
    assert_eq!(s.board.comments_of(REGISTRY).len(), before);
    assert_eq!(liveness_traffic(&s), 1, "only the old ping is there");
    // the coordinator takes no action on it either: no report, no reassign
    let (v, _) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v}");
    assert_eq!(s.board.comments_of(REGISTRY).len(), before);
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn a_mixed_crew_with_an_old_ping_and_a_late_agent_keeps_watch_and_poll_working() {
    let s = LateBoard::new("In Progress");
    // the dev is late (40 min), and an old coordinator pinged it 5 min ago
    s.board
        .set_item(&s.dev.item_id, "Heartbeat", &format_ts(now_epoch() - 2_400));
    s.board.signed_ago(
        &s.coord.key,
        &s.coord.name,
        &s.dev.name,
        Kind::Ping,
        REGISTRY,
        "ping: hermes@fake-b has written no heartbeat. Answer with a pong.",
        300,
    );
    // the new coordinator polls: no error, no second ping
    let (v, o) = s.poll(&s.coord);
    assert!(o.code == 0 || o.code == 1, "{}", o.all());
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v}");
    assert_eq!(liveness_traffic(&s), 1, "the old ping only");
    // the late dev watches: it writes a heartbeat, does not answer, and ends without an error
    let w = s.run(&s.dev, &["watch", "--timeout", "1s", "--interval", "1s"]);
    assert!(w.code == 0 || w.code == 1, "{}", w.all());
    assert!(!w.stderr.contains("error"), "{}", w.all());
    assert_eq!(liveness_traffic(&s), 1, "no pong");
    let hb = s
        .board
        .item_field(&s.dev.item_id, "Heartbeat")
        .and_then(|h| crewboard::clock::parse_ts(&h))
        .expect("heartbeat");
    assert!(now_epoch() - hb < 60, "the watch wrote a heartbeat");
    // the dev is alive again: the next coordinator cycle does not move its ticket
    let (v, _) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v}");
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn a_heartbeat_from_a_clock_that_runs_ahead_does_not_hide_a_silent_agent() {
    let s = LateBoard::new("In Progress");
    // the Heartbeat text says "tomorrow", but GitHub saw the item change an hour ago
    s.board.set_item(
        &s.dev.item_id,
        "Heartbeat",
        &format_ts(now_epoch() + 86_400),
    );
    s.board.touch_item_ago(&s.dev.item_id, 3_600);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(events_of(&v, "agent-dead"), 1, "{v} {}", o.all());
    assert_eq!(
        s.board.item_field("PVTI_ticket", "Status").as_deref(),
        Some("Backlog")
    );
    // control: the same text with a fresh `updatedAt` is alive
    let t = LateBoard::new("In Progress");
    t.board.set_item(
        &t.dev.item_id,
        "Heartbeat",
        &format_ts(now_epoch() + 86_400),
    );
    t.board.touch_item(&t.dev.item_id);
    let (v, _) = t.poll(&t.coord);
    assert_eq!(events_of(&v, "agent-dead"), 0, "{v}");
}

/// The Review ticket of the board has a `done` on `SHA1` and a passing verify of it on `head`.
fn verified_review(s: &LateBoard, head: &str) {
    let body = format!("Done. Evidence follows.\npr: https://x/pull/7\nsha: {SHA1}\nci: none\n");
    let done = s.board.signed_ago(
        &s.dev.key,
        &s.dev.name,
        "coordinator",
        Kind::Done,
        TICKET,
        &body,
        7200,
    );
    let pass =
        format!("verify: pass\npr: https://x/pull/7\nsha: {head}\nhead_now: {head}\nci: none\n");
    s.board.signed_re_ago(
        &s.coord.key,
        &s.coord.name,
        &s.dev.name,
        Kind::Report,
        TICKET,
        done,
        &pass,
        3600,
    );
}

#[test]
fn a_verified_ticket_with_a_held_merge_is_not_late_until_a_new_head_is_pushed() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    verified_review(&s, SHA1);
    // the merge is held: the item is old, the PR head is the verified one
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    let j = status_json(&s);
    let row = &j["tickets"][0];
    assert_eq!(row["waiting_for_verify"], false, "{j}");
    assert!(
        row["verified_waiting_since"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z')),
        "{j}"
    );
    assert!(
        j["late"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["wait"] != "verify"),
        "{j}"
    );
    let t = s.run(&s.coord, &["status"]);
    assert!(
        t.stdout.contains("verified, waiting for merge since 20"),
        "{}",
        t.all()
    );
    assert!(!t.stdout.contains("waiting for verify"), "{}", t.all());
    // a push after the verify: the verify judged an older head, the ticket is late again
    s.scene(|sc| sc.pr_head = Some(SHA2.into()));
    s.lift_throttle(&s.coord);
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"],
        "{}",
        o.all()
    );
    let j = status_json(&s);
    assert!(j["tickets"][0]["verified_waiting_since"].is_null(), "{j}");
    assert_eq!(j["tickets"][0]["waiting_for_verify"], true, "{j}");
    assert!(s.board.unhandled().is_empty(), "{:?}", s.board.unhandled());
}

#[test]
fn a_cycle_without_reads_does_not_report_a_verified_ticket() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    verified_review(&s, SHA1);
    // the reading cycle finds the ticket verified
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
    // the next cycle is inside the read throttle: it cannot read, so it must not report
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
}

#[test]
fn an_owner_route_ticket_in_review_gets_no_review_not_verified() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let body = format!(
        "Done. Evidence follows.\npr: https://x/pull/7\nsha: {SHA1}\nci: none\n\
         owner-route: docs/patches/7-workflow.patch\n"
    );
    s.board.signed_ago(
        &s.dev.key,
        &s.dev.name,
        "coordinator",
        Kind::Done,
        TICKET,
        &body,
        7200,
    );
    let (v, o) = s.poll(&s.coord);
    assert!(LateBoard::stale_events(&v).is_empty(), "{}", o.all());
}

#[test]
fn the_done_of_another_sender_does_not_hide_a_late_review() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    s.scene(|sc| sc.pr_head = Some(SHA1.into()));
    let plain = format!("Done. Evidence follows.\npr: https://x/pull/7\nsha: {SHA1}\nci: none\n");
    s.board.signed_ago(
        &s.dev.key,
        &s.dev.name,
        "coordinator",
        Kind::Done,
        TICKET,
        &plain,
        7200,
    );
    // a later `done` of another sender names the owner route: it is not the assignee's
    let stray = format!("{plain}owner-route: docs/patches/5-workflow.patch\n");
    s.board.signed_ago(
        &s.coord.key,
        &s.coord.name,
        "coordinator",
        Kind::Done,
        TICKET,
        &stray,
        3600,
    );
    let (v, o) = s.poll(&s.coord);
    assert_eq!(
        LateBoard::stale_events(&v),
        ["stale-work: review-not-verified"],
        "{}",
        o.all()
    );
}

/// The tick on a headless copy while the coordinator is away: its output.
fn tick_while_away(s: &LateBoard) -> String {
    coordinator_away(s);
    let dir = s.headless_copy(&s.coord);
    let o = s.run_dir(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    o.stdout
}

#[test]
fn tick_tells_the_owner_nothing_of_an_owner_route_review() {
    for line in [
        "owner-route: docs/patches/7-workflow.patch\n",
        "protected-paths: AUTHORITY.md\n",
    ] {
        let s = LateBoard::new("Review");
        s.late_table(&s.coord, &[("verify", "60s")]);
        let body =
            format!("Done. Evidence follows.\npr: https://x/pull/7\nsha: {SHA1}\nci: none\n{line}");
        s.board.signed_ago(
            &s.dev.key,
            &s.dev.name,
            "coordinator",
            Kind::Done,
            TICKET,
            &body,
            7200,
        );
        let out = tick_while_away(&s);
        assert!(!out.contains("late: verify"), "{line}: {out}");
        assert!(
            late_notes(&s, TICKET).is_empty(),
            "{line}: {:?}",
            late_notes(&s, TICKET)
        );
    }
}

#[test]
fn tick_tells_the_owner_nothing_of_a_passing_verify_it_cannot_compare_with_the_head() {
    let s = LateBoard::new("Review");
    s.late_table(&s.coord, &[("verify", "60s")]);
    // the verify passed on another head than the done names: the tick reads no PR head, so it
    // cannot say that the verify is stale
    verified_review(&s, SHA2);
    let out = tick_while_away(&s);
    assert!(!out.contains("late: verify"), "{out}");
    assert!(
        late_notes(&s, TICKET).is_empty(),
        "{:?}",
        late_notes(&s, TICKET)
    );
}
