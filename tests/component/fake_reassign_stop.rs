//! `reassign` (typed reason; Ready only with an agent), `stop --force` and the warning for a
//! paused agent with Ready tickets (spec/tickets#status-flow-and-writers, spec/board#lifecycle),
//! against the fake GitHub.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, OWNER, REGISTRY, REPO};
use crate::support::cli::{GATE, no_unhandled, run_ok, run_refused, writes};
use crewboard::rules::Role;

/// The fake, a coordinator and the dev agents A and B.
fn crew() -> (FakeBoard, Agent, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let a = fake.agent("claude@fake-a", Role::Dev);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    (fake, c, a, b)
}

fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    fake.ticket_issue(
        title,
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
}

fn status(fake: &FakeBoard, t: u64) -> Option<String> {
    fake.item_field(&fake.item_of_issue(t).unwrap(), "Status")
}

fn agent_of(fake: &FakeBoard, t: u64) -> Option<String> {
    fake.item_field(&fake.item_of_issue(t).unwrap(), "Agent")
}

/// A ticket that `who` works on (assigned and taken).
fn in_progress(fake: &FakeBoard, c: &Agent, who: &Agent, title: &str) -> u64 {
    let t = ticket(fake, title);
    run_ok(fake, c, &["assign", &t.to_string(), "--to", &who.name]);
    run_ok(fake, who, &["take", &t.to_string()]);
    t
}

#[test]
fn reassign_without_an_agent_goes_to_backlog_and_assign_makes_it_ready_again() {
    let (fake, c, a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT reassign backlog");
    let ts = t.to_string();
    let o = run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead", "--why", "host crashed"],
    );
    assert!(
        o.stdout
            .contains(&format!("{REPO}#{t} back to Backlog; Agent cleared")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("reason: agent is dead: host crashed"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains(&format!("crewboard assign {REPO}#{t} --to <agent>")),
        "{}",
        o.stdout
    );
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    assert_eq!(agent_of(&fake, t), None);
    // the notice to the old agent carries the typed reason with its detail
    assert!(
        fake.comments_of(t)
            .iter()
            .any(|(_, _, body)| body.contains("Reassigned. Reason: agent is dead: host crashed")),
        "no notice"
    );
    // a Backlog ticket has no agent to take from: it is not reassigned again
    run_refused(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead"],
        "status-no-change",
    );
    // `assign` puts it into Ready for the new agent
    run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert_eq!(status(&fake, t).as_deref(), Some("Ready"));
    assert_eq!(agent_of(&fake, t).as_deref(), Some(a.name.as_str()));
    no_unhandled(&fake);
}

#[test]
fn reassign_with_an_agent_goes_to_ready_for_that_agent() {
    let (fake, c, a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT reassign ready");
    let ts = t.to_string();
    let o = run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--to",
            &a.name,
            "--reason",
            "blocked-too-long",
        ],
    );
    assert!(
        o.stdout.contains("back to Ready; Agent claude@fake-a"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("reason: ticket was blocked too long\n"),
        "{}",
        o.stdout
    );
    assert_eq!(status(&fake, t).as_deref(), Some("Ready"));
    assert_eq!(agent_of(&fake, t).as_deref(), Some(a.name.as_str()));
    // within Ready only the agent changes
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--to", &b.name, "--reason", "late"],
    );
    assert_eq!(status(&fake, t).as_deref(), Some("Ready"));
    assert_eq!(agent_of(&fake, t).as_deref(), Some(b.name.as_str()));
    // a Ready ticket without --to goes to Backlog too
    run_ok(&fake, &c, &["reassign", &ts, "--reason", "stopping"]);
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    assert_eq!(agent_of(&fake, t), None);
}

#[test]
fn a_reason_that_is_not_one_of_the_defined_words_is_refused_before_any_write() {
    let (fake, c, _a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT bad reason");
    let ts = t.to_string();
    let before = writes(&fake);
    for args in [
        &["reassign", &ts, "--reason", "agent stopped"][..],
        &["reassign", &ts, "--reason", "Dead"][..],
        &["reassign", &ts, "--why", "no typed reason"][..],
        &["reassign", &ts][..],
    ] {
        let o = fake.run(&c.dir, args);
        assert_ne!(o.code, 0, "{args:?}: {}", o.all());
        assert!(
            o.stderr.contains("--reason") || o.stderr.contains("--why"),
            "{args:?}: {}",
            o.all()
        );
    }
    let o = fake.run(&c.dir, &["reassign", &ts, "--reason", "agent stopped"]);
    assert!(
        o.stderr.contains("dead") && o.stderr.contains("blocked-too-long"),
        "the error names the words: {}",
        o.stderr
    );
    assert_eq!(writes(&fake), before);
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
}

#[test]
fn stop_force_by_the_coordinator_returns_the_tickets_and_the_agent_has_left() {
    let (fake, c, a, b) = crew();
    let busy = in_progress(&fake, &c, &b, "IT forced busy");
    let queued = ticket(&fake, "IT forced queued");
    run_ok(&fake, &c, &["assign", &queued.to_string(), "--to", &b.name]);
    let other = in_progress(&fake, &c, &a, "IT other agent");
    let o = run_ok(&fake, &c, &["stop", "--agent", &b.name, "--force"]);
    assert!(
        o.stdout
            .contains("hermes@fake-b stopped by force (was active): State left"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!(
            "tickets returned to Backlog (assign them again): {REPO}#{busy}, {REPO}#{queued}"
        )),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("the owner revokes it with `/crew reject hermes@fake-b`"),
        "{}",
        o.stdout
    );
    for t in [busy, queued] {
        assert_eq!(status(&fake, t).as_deref(), Some("Backlog"), "#{t}");
        assert_eq!(agent_of(&fake, t), None);
    }
    // another agent's ticket stays
    assert_eq!(status(&fake, other).as_deref(), Some("In Progress"));
    assert_eq!(
        fake.item_field(&b.item_id, "State").as_deref(),
        Some("left")
    );
    // one signed notice to the owner in the registry issue
    assert!(
        fake.comments_of(REGISTRY).iter().any(|(_, _, body)| body
            .contains("hermes@fake-b was stopped by force by codex@fake-c: State left")),
        "no notice"
    );
    // control: a repeat is refused, nothing more is written
    let before = writes(&fake);
    run_refused(
        &fake,
        &c,
        &["stop", "--agent", &b.name, "--force"],
        "stop-force-already-left",
    );
    assert_eq!(writes(&fake), before);
    // the coordinator cannot end the span of an agent: the log has no stop of b, and the notice
    // above is the one report of that (its `event=` key), so the cycle adds no second note
    let _ = fake.poll_json(&c);
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 0);
    assert!(
        fake.comments_of(REGISTRY)
            .iter()
            .any(|(_, _, body)| body.contains("event=hermes@fake-b/state:left")),
        "the notice carries the key"
    );
    // b keeps its span: it may still act (it reads the reassign notice first)
    let _ = fake.run(&b.dir, &["watch", "--timeout", "1s"]);
    run_ok(
        &fake,
        &b,
        &[
            "say",
            &busy.to_string(),
            "--to",
            "coordinator",
            "--kind",
            "info",
            "--body",
            "still here",
        ],
    );
    // the owner records the stop in the log: the State is left already, so only the command
    let owner = fake.owner_dir();
    let o = fake.run(
        owner.path(),
        &[
            "stop",
            "--agent",
            &b.name,
            "--force",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("owner command posted: /crew stop hermes@fake-b"),
        "{}",
        o.stdout
    );
    no_unhandled(&fake);
}

/// The signed notes to the owner on a board State that the log does not support.
fn unlogged_notes(fake: &FakeBoard) -> usize {
    fake.comments_of(REGISTRY)
        .iter()
        .filter(|(_, _, body)| body.contains("Board State without a stop in the log: "))
        .count()
}

#[test]
fn a_hand_written_state_left_neither_mutes_the_agent_nor_returns_its_tickets_and_is_reported() {
    let (fake, c, _a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT hand-written left");
    // a crew-token holder writes the field: one write, no log entry
    fake.set_item(&b.item_id, "State", "left");
    fake.touch_item_ago(&b.item_id, 300);
    // the agent is not muted: it still speaks, and `status` lists it as trusted with the problem
    run_ok(
        &fake,
        &b,
        &[
            "say",
            &t.to_string(),
            "--to",
            "coordinator",
            "--kind",
            "info",
            "--body",
            "still working",
        ],
    );
    let o = run_ok(&fake, &c, &["status", "--json"]);
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).unwrap();
    let problems = v["trust_problems"].as_array().unwrap();
    assert!(
        problems.iter().any(|p| p
            .as_str()
            .unwrap()
            .contains("hermes@fake-b: item State is 'left' but the registry log has not stopped or ended this agent")),
        "{v}"
    );
    // its own loop goes on (the log has not ended it)
    let o = fake.run(&b.dir, &["poll"]);
    assert_ne!(o.code, 2, "{}", o.all());
    assert!(!o.stderr.contains("agent-left"), "{}", o.all());
    // the coordinator returns nothing and tells the owner once: the first sight may be a write
    // in flight, the next cycle that still sees it reports it
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 0);
    let (v, _) = fake.poll_json(&c);
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"), "{v}");
    assert_eq!(agent_of(&fake, t).as_deref(), Some(b.name.as_str()));
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent-state-unlogged"),
        "{v}"
    );
    assert!(
        !v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent-stopped"),
        "{v}"
    );
    assert_eq!(unlogged_notes(&fake), 1);
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 1, "reported once");
    // a derived reassign is not justified by the field either
    run_refused(
        &fake,
        &c,
        &["reassign", &t.to_string(), "--derived"],
        "status-reassign-not-justified",
    );
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
    no_unhandled(&fake);
}

#[test]
fn a_state_write_that_the_log_follows_at_once_is_not_reported() {
    let (fake, c, _a, b) = crew();
    let _ = fake.poll_json(&c);
    fake.set_item(&b.item_id, "State", "draining");
    // a write in flight: the agent signs its notice a moment after the field
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 0);
    run_ok(&fake, &b, &["stop"]);
    let _ = fake.poll_json(&c);
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 0);
    // control: a field that the log never follows is reported at the second sight
    let x = fake.agent("claude@fake-x", Role::Dev);
    fake.set_item(&x.item_id, "State", "left");
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 0);
    let _ = fake.poll_json(&c);
    assert_eq!(unlogged_notes(&fake), 1);
}

#[test]
fn stop_force_is_refused_for_a_dev_agent_the_coordinator_itself_and_unknown_agents() {
    let (fake, c, a, b) = crew();
    run_refused(
        &fake,
        &a,
        &["stop", "--agent", &b.name, "--force"],
        "stop-force-needs-coordinator",
    );
    run_refused(
        &fake,
        &c,
        &["stop", "--agent", &c.name, "--force"],
        "stop-force-self",
    );
    run_refused(
        &fake,
        &c,
        &["stop", "--agent", "nobody@fake-z", "--force"],
        "no-join-request",
    );
    // --force without --agent is a usage error, and the plain stop is unchanged
    let o = fake.run(&c.dir, &["stop", "--force"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert_eq!(
        fake.item_field(&b.item_id, "State").as_deref(),
        Some("active")
    );
}

#[test]
fn stop_force_by_the_owner_writes_the_state_and_the_coordinator_returns_the_tickets() {
    let (fake, c, _a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT owner force");
    let owner = fake.owner_dir();
    let o = fake.run(
        owner.path(),
        &[
            "stop",
            "--agent",
            &b.name,
            "--force",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());

    assert!(
        o.stdout
            .contains("hermes@fake-b stopped by force (was active): State left"),
        "{}",
        o.stdout
    );
    assert_eq!(
        fake.item_field(&b.item_id, "State").as_deref(),
        Some("left")
    );
    // the stop is in the registry log: the owner command, as the owner account
    assert!(
        o.stdout
            .contains("owner command posted: /crew stop hermes@fake-b"),
        "{}",
        o.stdout
    );
    let stops: Vec<_> = fake
        .comments_of(REGISTRY)
        .into_iter()
        .filter(|(_, login, body)| login == OWNER && body == "/crew stop hermes@fake-b")
        .collect();
    assert_eq!(stops.len(), 1, "{:?}", fake.comments_of(REGISTRY));
    // the owner has no signing identity: the ticket waits for the coordinator's cycle
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
    let (v, _) = fake.poll_json(&c);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "agent-stopped"
                && e["body"].as_str().unwrap().contains("to Backlog")
                && e["body"]
                    .as_str()
                    .unwrap()
                    .contains("the owner's /crew stop")),
        "{v}"
    );
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    // a State left with the stop in the log: no report, and the loop of the agent stops
    assert_eq!(unlogged_notes(&fake), 0);
    let o = fake.run(&b.dir, &["poll"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("agent-left"), "{}", o.all());
    // a repeat has nothing to do
    let o = fake.run(
        owner.path(),
        &[
            "stop",
            "--agent",
            &b.name,
            "--force",
            "--initiative",
            INITIATIVE,
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("stop-force-already-left"), "{}", o.all());
}

#[test]
fn a_paused_agent_with_ready_tickets_is_warned_once_by_the_cycle_and_always_by_status() {
    let (fake, c, _a, b) = crew();
    let t = ticket(&fake, "IT paused ready");
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    run_ok(&fake, &b, &["pause"]);
    let kinds = |v: &serde_json::Value| -> Vec<String> {
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "agent-paused-ready")
            .map(|e| e["body"].as_str().unwrap().to_string())
            .collect()
    };
    let (v, _) = fake.poll_json(&c);
    let first = kinds(&v);
    assert_eq!(first.len(), 1, "{v}");
    assert!(
        first[0].contains(&format!(
            "warning: hermes@fake-b is paused and holds 1 Ready ticket(s): {REPO}#{t}."
        )),
        "{}",
        first[0]
    );
    assert!(first[0].contains("crewboard assign"), "{}", first[0]);
    // the same finding is not repeated
    let (v, _) = fake.poll_json(&c);
    assert!(kinds(&v).is_empty(), "{v}");
    // status shows it every time
    let s = run_ok(&fake, &c, &["status"]);
    assert!(
        s.stdout.contains("is paused and holds 1 Ready ticket(s)"),
        "{}",
        s.stdout
    );
    // after the resume there is nothing to say; a new pause warns again
    run_ok(&fake, &b, &["resume"]);
    let (v, _) = fake.poll_json(&c);
    assert!(kinds(&v).is_empty(), "{v}");
    assert!(
        !run_ok(&fake, &c, &["status"])
            .stdout
            .contains("is paused and holds")
    );
    run_ok(&fake, &b, &["pause"]);
    let (v, _) = fake.poll_json(&c);
    assert_eq!(kinds(&v).len(), 1, "{v}");
}

#[test]
fn a_ready_ticket_without_an_agent_on_an_old_board_is_a_warning_not_a_crash() {
    let (fake, c, _a, b) = crew();
    let t = ticket(&fake, "IT old ready");
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    // an older reassign left the ticket in Ready with no Agent
    let item = fake.item_of_issue(t).unwrap();
    fake.clear_item(&item, "Agent");
    let s = run_ok(&fake, &c, &["status"]);
    assert!(
        s.stdout
            .contains(&format!("{REPO}#{t} is Ready without an Agent")),
        "{}",
        s.stdout
    );
    let j = run_ok(&fake, &c, &["status", "--json"]);
    assert!(
        j.stdout.contains("is Ready without an Agent"),
        "{}",
        j.stdout
    );
    // `assign` repairs it
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    assert_eq!(agent_of(&fake, t).as_deref(), Some(b.name.as_str()));
    assert!(
        !run_ok(&fake, &c, &["status"])
            .stdout
            .contains("Ready without an Agent")
    );
}

fn refused_or_failed(fake: &FakeBoard, who: &Agent, args: &[&str]) {
    let o = fake.run(&who.dir, args);
    assert_ne!(o.code, 0, "{args:?} should fail: {}", o.all());
}

#[test]
fn a_failed_status_write_of_reassign_leaves_the_ticket_with_its_agent_and_a_second_run_finishes() {
    for to in [None, Some("claude@fake-a")] {
        let (fake, c, _a, b) = crew();
        let t = in_progress(&fake, &c, &b, "IT reassign status fails");
        let ts = t.to_string();
        let mut args = vec!["reassign", ts.as_str(), "--reason", "dead"];
        if let Some(n) = to {
            args.extend(["--to", n]);
        }
        let target = if to.is_some() { "Ready" } else { "Backlog" };
        fake.fail_select_write(Some(target));
        refused_or_failed(&fake, &c, &args);
        // no agent was cleared or changed before the Status write: the ticket is still b's
        assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
        assert_eq!(agent_of(&fake, t).as_deref(), Some(b.name.as_str()));
        fake.fail_select_write(None);
        run_ok(&fake, &c, &args);
        assert_eq!(status(&fake, t).as_deref(), Some(target));
        assert_eq!(agent_of(&fake, t).as_deref(), to);
    }
}

#[test]
fn a_failed_agent_write_of_reassign_leaves_a_state_that_status_names_and_assign_repairs() {
    // without --to: Backlog with the old agent
    let (fake, c, a, b) = crew();
    let t = in_progress(&fake, &c, &b, "IT reassign clear fails");
    let ts = t.to_string();
    fake.fail_clear_write(true);
    refused_or_failed(&fake, &c, &["reassign", &ts, "--reason", "dead"]);
    fake.fail_clear_write(false);
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    assert_eq!(agent_of(&fake, t).as_deref(), Some(b.name.as_str()));
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout
            .contains("tickets in Backlog that still name an agent")
            && o.stdout.contains(&format!("{REPO}#{t} ({})", b.name)),
        "{}",
        o.stdout
    );
    // `assign` overwrites the old agent
    run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert_eq!(status(&fake, t).as_deref(), Some("Ready"));
    assert_eq!(agent_of(&fake, t).as_deref(), Some(a.name.as_str()));
    let o = run_ok(&fake, &c, &["status"]);
    assert!(!o.stdout.contains("still name an agent"), "{}", o.stdout);
    // with --to: Ready with the old agent, and the same command again finishes it
    let t2 = in_progress(&fake, &c, &b, "IT reassign set fails");
    let ts2 = t2.to_string();
    let args = ["reassign", &ts2, "--reason", "dead", "--to", &a.name];
    fake.fail_select_write_after(Some("Ready"), 0);
    refused_or_failed(&fake, &c, &args);
    fake.fail_select_write_after(None, 0);
    assert_eq!(status(&fake, t2).as_deref(), Some("In Progress"));
    run_ok(&fake, &c, &args);
    assert_eq!(agent_of(&fake, t2).as_deref(), Some(a.name.as_str()));
    no_unhandled(&fake);
}
