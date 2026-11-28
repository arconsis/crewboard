//! The work queue, the `agent:` label, `prune` and `report-status` against a fake GitHub
//!.
//!
//! These tests run in plain `cargo test`: no token, no sandbox, no real GitHub. They replace
//! five tests of `it_sandbox.rs` (see `CONTRIBUTING.md`).

use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::{GATE, no_unhandled, run_ok, writes};
use crewboard::rules::Role;
use serde_json::Value;

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

/// The command is refused: exit 2 and the rule named in the text.
fn run_refused(fake: &FakeBoard, who: &Agent, args: &[&str], rule: &str) -> Out {
    let o = fake.run(&who.dir, args);
    assert_eq!(o.code, 2, "{} {args:?}: {}", who.name, o.all());
    assert!(o.stderr.contains(rule), "{rule} missing: {}", o.all());
    o
}

fn field(fake: &FakeBoard, item: &str, name: &str) -> Option<String> {
    fake.item_field(item, name)
}

fn item(fake: &FakeBoard, t: u64) -> String {
    fake.item_of_issue(t).expect("board item of the ticket")
}

fn json_of(o: &Out) -> Value {
    serde_json::from_str(o.stdout.lines().last().unwrap_or(""))
        .unwrap_or_else(|e| panic!("not JSON ({e}): {}", o.all()))
}

fn assignment_body(fake: &FakeBoard, issue: u64) -> String {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("Assigned to you"))
        .unwrap_or_default()
}

#[test]
fn assign_queues_for_a_busy_agent_and_refuses_the_others() {
    let (fake, c, _a, b) = crew();
    let (t1, t2, t3) = (
        ticket(&fake, "IT queue first").to_string(),
        ticket(&fake, "IT queue second"),
        ticket(&fake, "IT queue third").to_string(),
    );
    let t2s = t2.to_string();
    run_ok(&fake, &c, &["assign", &t1, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &t1]);
    assert_eq!(field(&fake, &b.item_id, "State").as_deref(), Some("active"));

    let w = writes(&fake);
    // refusal: a non-dev agent (the coordinator) is refused
    run_refused(
        &fake,
        &c,
        &["assign", &t2s, "--to", &c.name],
        "not a dev agent",
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert!(fake.item_of_issue(t2).is_none());

    // pass: the busy agent gets the second ticket as a queue entry
    let o = run_ok(&fake, &c, &["assign", &t2s, "--to", &b.name]);
    assert!(o.stdout.contains("Order set to 2"), "{}", o.stdout);
    let item2 = item(&fake, t2);
    assert_eq!(field(&fake, &item2, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        field(&fake, &item2, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(field(&fake, &item2, "Order").as_deref(), Some("2"));
    assert!(assignment_body(&fake, t2).contains("order: 2"));
    // the agent still works on the first ticket
    assert_eq!(field(&fake, &b.item_id, "State").as_deref(), Some("active"));
    // the board has no Priority field and the message no priority line
    assert_eq!(field(&fake, &item2, "Priority"), None);
    assert!(!assignment_body(&fake, t2).contains("priority"));
    // status shows the Order
    let sv = json_of(&run_ok(&fake, &c, &["status", "--json"]));
    assert!(
        sv["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["ticket"] == format!("{REPO}#{t2}") && x["order"] == 2),
        "{sv}"
    );

    // refusal: a paused agent takes nothing new; after resume it does
    run_ok(&fake, &b, &["pause"]);
    run_refused(
        &fake,
        &c,
        &["assign", &t3, "--to", &b.name],
        "status-agent-not-active-dev",
    );
    run_ok(&fake, &b, &["resume"]);
    run_ok(&fake, &c, &["assign", &t3, "--to", &b.name]);
    no_unhandled(&fake);
}

#[test]
fn reassign_swaps_the_agent_label_and_clearing_the_agent_removes_it() {
    let (fake, c, a, b) = crew();
    let t = ticket(&fake, "IT agent label swap");
    let ts = t.to_string();
    let (la, lb) = (format!("agent: {}", a.name), format!("agent: {}", b.name));
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let l = fake.labels_of(t);
    assert!(l.contains(&lb) && !l.contains(&la), "{l:?}");
    // reassign to A: B's label goes, A's label comes, the other labels stay
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--reason",
            "late",
            "--why",
            "swap test",
            "--to",
            &a.name,
        ],
    );
    let l = fake.labels_of(t);
    assert!(l.contains(&la) && !l.contains(&lb), "{l:?}");
    assert!(l.contains(&"crew:work".to_string()), "{l:?}");
    assert_eq!(
        field(&fake, &item(&fake, t), "Agent").as_deref(),
        Some(a.name.as_str())
    );
    // reassign without --to: the Agent is cleared and so is the label
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "late", "--why", "park it"],
    );
    let l = fake.labels_of(t);
    assert!(
        !l.iter().any(|x| x.starts_with("agent: ")),
        "no agent label: {l:?}"
    );
    assert!(l.contains(&"crew:work".to_string()), "{l:?}");
    assert_eq!(field(&fake, &item(&fake, t), "Agent"), None);
    no_unhandled(&fake);
}

/// The ticket keys (`repo#n`) in the order `mine` prints the Ready tickets.
fn ready_order(mine_stdout: &str) -> Vec<String> {
    mine_stdout
        .lines()
        .filter(|l| l.contains(" Ready "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .collect()
}

#[test]
fn queue_order_moves_and_status_queue() {
    let (fake, c, a, b) = crew();
    let key = |t: u64| format!("{REPO}#{t}");
    // three tickets for B with orders 30, 10, 20
    let mut ts = Vec::new();
    for (title, order) in [
        ("IT queue 1", "30"),
        ("IT queue 2", "10"),
        ("IT queue 3", "20"),
    ] {
        let t = ticket(&fake, title);
        let o = run_ok(
            &fake,
            &c,
            &["assign", &t.to_string(), "--to", &b.name, "--order", order],
        );
        assert!(
            o.stdout.contains(&format!("Order set to {order}")),
            "{}",
            o.stdout
        );
        ts.push(t);
    }
    let items: Vec<String> = ts.iter().map(|t| item(&fake, *t)).collect();
    for (i, want) in items.iter().zip(["30", "10", "20"]) {
        assert_eq!(field(&fake, i, "Order").as_deref(), Some(want));
    }
    // the lowest Order is the next ticket: #2 (10), #3 (20), #1 (30)
    let o = run_ok(&fake, &b, &["mine"]);
    assert_eq!(
        ready_order(&o.stdout),
        [key(ts[1]), key(ts[2]), key(ts[0])],
        "{}",
        o.stdout
    );
    // reorder: #1 to the front
    run_ok(&fake, &c, &["assign", &ts[0].to_string(), "--order", "5"]);
    assert_eq!(field(&fake, &items[0], "Order").as_deref(), Some("5"));
    let o = run_ok(&fake, &b, &["mine"]);
    assert_eq!(
        ready_order(&o.stdout),
        [key(ts[0]), key(ts[1]), key(ts[2])],
        "{}",
        o.stdout
    );
    // refusals: a dev agent may not reorder; a bad order is refused before any write
    let w = writes(&fake);
    run_refused(
        &fake,
        &b,
        &["assign", &ts[0].to_string(), "--order", "1"],
        "field-wrong-writer",
    );
    run_refused(
        &fake,
        &c,
        &["assign", &ts[0].to_string(), "--order", "-3"],
        "order-invalid",
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert_eq!(field(&fake, &items[0], "Order").as_deref(), Some("5"));

    // status --queue (text and json): B's tickets in order, with the counts
    let o = run_ok(&fake, &c, &["status", "--queue"]);
    let at = |t: u64| {
        o.stdout
            .find(&key(t))
            .unwrap_or_else(|| panic!("{}", o.stdout))
    };
    assert!(
        at(ts[0]) < at(ts[1]) && at(ts[1]) < at(ts[2]),
        "{}",
        o.stdout
    );
    let v = json_of(&run_ok(&fake, &c, &["status", "--queue", "--json"]));
    let b_q = v["queue"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["agent"] == b.name.as_str())
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(b_q["ready_count"], 3, "{b_q}");

    // rebalance: #3 moves to A (Agent, label, Order, one message)
    let before = fake.comments_of(ts[2]).len();
    run_ok(&fake, &c, &["assign", &ts[2].to_string(), "--to", &a.name]);
    assert_eq!(
        field(&fake, &items[2], "Agent").as_deref(),
        Some(a.name.as_str())
    );
    assert!(field(&fake, &items[2], "Order").is_some());
    assert_eq!(field(&fake, &items[2], "Status").as_deref(), Some("Ready"));
    let labels = fake.labels_of(ts[2]);
    assert!(labels.contains(&format!("agent: {}", a.name)), "{labels:?}");
    assert!(
        !labels.contains(&format!("agent: {}", b.name)),
        "{labels:?}"
    );
    assert_eq!(fake.comments_of(ts[2]).len(), before + 1, "one message");
    let o = run_ok(&fake, &a, &["mine"]);
    assert!(o.stdout.contains(&key(ts[2])), "{}", o.stdout);
    // rebalance back with an explicit order
    run_ok(
        &fake,
        &c,
        &[
            "assign",
            &ts[2].to_string(),
            "--to",
            &b.name,
            "--order",
            "99",
        ],
    );
    assert_eq!(field(&fake, &items[2], "Order").as_deref(), Some("99"));
    assert_eq!(
        field(&fake, &items[2], "Agent").as_deref(),
        Some(b.name.as_str())
    );

    assert_eq!(
        field(&fake, &items[0], "Agent").as_deref(),
        Some(b.name.as_str())
    );
    no_unhandled(&fake);
}

/// The signed message that a move of a queued ticket posted to the new agent: the body that names the move.
fn moved_message(fake: &FakeBoard, issue: u64) -> String {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("moved-from: "))
        .unwrap_or_default()
}

#[test]
fn assign_moves_a_queued_ticket_and_judges_the_ticket_and_the_writer_before_any_write() {
    let (fake, c, a, b) = crew();
    let t1 = ticket(&fake, "IT assign moves queued");
    let s2 = ticket(&fake, "IT assign moves backlog").to_string();
    let t3 = ticket(&fake, "IT assign moves running");
    let (s1, s3) = (t1.to_string(), t3.to_string());
    run_ok(&fake, &c, &["assign", &s1, "--to", &b.name, "--order", "3"]);
    run_ok(&fake, &c, &["ticket", "adopt", &s2]);
    run_ok(&fake, &c, &["assign", &s3, "--to", &b.name, "--order", "1"]);
    run_ok(&fake, &b, &["take", &s3]);
    let w = writes(&fake);
    // refusals: nothing to do, the same agent without an order, a dev agent, an agent that is
    // no dev, a ticket that runs
    run_refused(&fake, &c, &["assign", &s1], "assign-needs-to");
    run_refused(
        &fake,
        &c,
        &["assign", &s1, "--to", &b.name],
        "ticket-already-assigned",
    );
    run_refused(
        &fake,
        &b,
        &["assign", &s1, "--order", "9"],
        "field-wrong-writer",
    );
    run_refused(
        &fake,
        &c,
        &["assign", &s1, "--to", &c.name],
        "status-agent-not-active-dev",
    );
    run_refused(
        &fake,
        &c,
        &["assign", &s3, "--order", "9"],
        "reorder-not-queued",
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    // pass: --order alone, and --to its own agent with --order, set the position; no message
    let before = fake.comments_of(t1).len();
    let o = run_ok(&fake, &c, &["assign", &s1, "--order", "7"]);
    assert!(
        o.stdout.contains(&format!(
            "{REPO}#{t1} queue position of {}: Order 7",
            b.name
        )),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item(&fake, t1), "Order").as_deref(),
        Some("7")
    );
    run_ok(&fake, &c, &["assign", &s1, "--to", &b.name, "--order", "5"]);
    assert_eq!(
        field(&fake, &item(&fake, t1), "Order").as_deref(),
        Some("5")
    );
    assert_eq!(
        fake.comments_of(t1).len(),
        before,
        "a new position posts nothing"
    );
    // pass: --to another agent moves the ticket: Agent, label, Order, ONE message
    let o = run_ok(&fake, &c, &["assign", &s1, "--to", &a.name]);
    assert!(
        o.stdout.contains(&format!(
            "moved from {} to {}; Status Ready; Order 1",
            b.name, a.name
        )),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item(&fake, t1), "Agent").as_deref(),
        Some(a.name.as_str())
    );
    assert!(fake.labels_of(t1).contains(&format!("agent: {}", a.name)));
    assert!(!fake.labels_of(t1).contains(&format!("agent: {}", b.name)));
    assert_eq!(fake.comments_of(t1).len(), before + 1, "one message");
    let m = moved_message(&fake, t1);
    assert!(
        m.contains(&format!("moved-from: {}", b.name)) && m.contains("order: 1"),
        "{m}"
    );
    // pass: the move with an explicit order
    run_ok(
        &fake,
        &c,
        &["assign", &s1, "--to", &b.name, "--order", "99"],
    );
    assert_eq!(
        field(&fake, &item(&fake, t1), "Order").as_deref(),
        Some("99")
    );
    assert_eq!(
        field(&fake, &item(&fake, t1), "Agent").as_deref(),
        Some(b.name.as_str())
    );
    no_unhandled(&fake);
}

/// The `order:` line of the signed assignment message of a ticket.
fn assigned_order(fake: &FakeBoard, issue: u64) -> Option<String> {
    assignment_body(fake, issue)
        .lines()
        .find_map(|l| l.strip_prefix("order: ").map(str::to_string))
}

#[test]
fn assign_without_an_order_appends_to_the_queue() {
    let (fake, c, a, b) = crew();
    let key = |t: u64| format!("{REPO}#{t}");
    // an empty queue starts at 1, then each ticket goes to the end
    let mut ts = Vec::new();
    for (i, want) in ["1", "2", "3"].into_iter().enumerate() {
        let t = ticket(&fake, &format!("IT append {i}"));
        let o = run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
        assert!(
            o.stdout
                .contains(&format!("Order set to {want} (the end of the queue)")),
            "{}",
            o.stdout
        );
        assert_eq!(
            field(&fake, &item(&fake, t), "Order").as_deref(),
            Some(want)
        );
        assert_eq!(
            assigned_order(&fake, t).as_deref(),
            Some(want),
            "the signed message"
        );
        ts.push(t);
    }
    // the running ticket stays in the queue: a new ticket does not jump ahead of it
    run_ok(&fake, &b, &["take", &ts[2].to_string()]);
    let t4 = ticket(&fake, "IT append 4");
    run_ok(&fake, &c, &["assign", &t4.to_string(), "--to", &b.name]);
    assert_eq!(
        field(&fake, &item(&fake, t4), "Order").as_deref(),
        Some("4")
    );
    // another agent has its own queue
    let a_queue = ticket(&fake, "IT append other agent");
    run_ok(
        &fake,
        &c,
        &["assign", &a_queue.to_string(), "--to", &a.name],
    );
    assert_eq!(
        field(&fake, &item(&fake, a_queue), "Order").as_deref(),
        Some("1")
    );
    // an explicit order wins, and the default does not look at it
    let t5 = ticket(&fake, "IT append 5");
    let o = run_ok(
        &fake,
        &c,
        &["assign", &t5.to_string(), "--to", &b.name, "--order", "2"],
    );
    assert!(o.stdout.contains("Order set to 2\n"), "{}", o.stdout);
    let o = run_ok(&fake, &b, &["mine"]);
    assert!(o.stdout.contains(&key(t5)), "{}", o.stdout);

    // a ticket without an Order (an old board) does not stop the default: the end of the queue
    fake.clear_item(&item(&fake, ts[1]), "Order");
    let t6 = ticket(&fake, "IT append 6");
    run_ok(&fake, &c, &["assign", &t6.to_string(), "--to", &b.name]);
    assert_eq!(
        field(&fake, &item(&fake, t6), "Order").as_deref(),
        Some("5")
    );
    assert_eq!(assigned_order(&fake, t6).as_deref(), Some("5"));
    no_unhandled(&fake);
}

#[test]
fn prune_removes_only_unused_options_of_agents_that_left() {
    let (fake, c, a, b) = crew();
    let p = fake.agent("kimi@fake-p", Role::Dev);
    let q = fake.agent("qwen@fake-q", Role::Dev);
    // q holds a ticket (Ready), then both stop: p has nothing open and leaves at once; q is
    // drained by the coordinator and leaves
    let t = ticket(&fake, "IT prune");
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &q.name]);
    let it = item(&fake, t);
    run_ok(&fake, &p, &["stop"]);
    run_ok(&fake, &q, &["stop"]);
    let _ = fake.poll_json(&c);
    let _ = fake.poll_json(&q);
    assert_eq!(field(&fake, &p.item_id, "State").as_deref(), Some("left"));
    assert_eq!(field(&fake, &q.item_id, "State").as_deref(), Some("left"));
    assert_eq!(
        field(&fake, &it, "Agent"),
        None,
        "the coordinator returned the ticket of q"
    );
    // the owner sets the Agent of the ticket by hand: the option of q is used now
    fake.set_item(&it, "Agent", &q.name);
    let opts = fake.agent_options();
    assert!(opts.contains(&p.name) && opts.contains(&q.name), "{opts:?}");

    // a dev cannot prune
    let w = writes(&fake);
    run_refused(&fake, &b, &["prune"], "prune-coordinator-only");
    assert_eq!(writes(&fake), w);

    let o = run_ok(&fake, &c, &["prune"]);
    assert!(
        o.stdout.contains(&format!("removed option: {}", p.name)),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!("kept: {}", q.name))
            && o.stdout.contains("1 ticket(s) still use this option"),
        "{}",
        o.stdout
    );
    let after = fake.agent_options();
    assert!(!after.contains(&p.name), "the unused option is gone");
    assert!(after.contains(&q.name), "the used option stays");
    for standing in [&a.name, &b.name] {
        assert!(after.contains(standing), "{standing} is not left: it stays");
    }
    assert_eq!(
        field(&fake, &it, "Agent").as_deref(),
        Some(q.name.as_str()),
        "the ticket value survives the prune"
    );
    // control: when nothing uses the option any more, the next prune removes it
    fake.clear_item(&it, "Agent");
    let o = run_ok(&fake, &c, &["prune"]);
    assert!(
        o.stdout.contains(&format!("removed option: {}", q.name)),
        "{}",
        o.stdout
    );
    assert!(!fake.agent_options().contains(&q.name));
    // nothing left to prune: no write
    let w = writes(&fake);
    let o = run_ok(&fake, &c, &["prune"]);
    assert!(o.stdout.contains("nothing to prune"), "{}", o.stdout);
    assert_eq!(writes(&fake), w);
    no_unhandled(&fake);
}

#[test]
fn report_status_reads_back_and_status_lists_tickets_with_liveness() {
    let (fake, c, _a, b) = crew();
    let o = run_ok(
        &fake,
        &c,
        &[
            "report-status",
            "--health",
            "on-track",
            "--body",
            "IT status update",
        ],
    );
    assert!(
        o.stdout.contains(&format!("first line from={}", c.name)),
        "{}",
        o.stdout
    );
    let id = o
        .stdout
        .split("status update ")
        .nth(1)
        .and_then(|r| r.split(':').next())
        .unwrap()
        .to_string();
    let ups = fake.status_updates();
    assert_eq!(ups.len(), 1, "{ups:?}");
    let (uid, status, body) = &ups[0];
    assert_eq!(uid, &id);
    assert_eq!(status, "ON_TRACK");
    assert_eq!(body, &format!("from={}\n\nIT status update", c.name));

    // refusals: a dev, an unknown state; neither writes an update
    run_refused(
        &fake,
        &b,
        &["report-status", "--health", "on-track"],
        "report-status-coordinator-only",
    );
    let o = fake.run(&c.dir, &["report-status", "--health", "green"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert_eq!(fake.status_updates().len(), 1);
    // refusal: the newest update does not read back as ours
    fake.foreign_status_update_next();
    let o = run_refused(
        &fake,
        &c,
        &["report-status", "--health", "at-risk"],
        "report-status-readback",
    );
    assert!(o.stderr.contains("was created"), "{}", o.all());

    // status: the ticket with status, agent and age, and the agents with derived liveness
    let t = ticket(&fake, "IT status list");
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    let v = json_of(&run_ok(&fake, &c, &["status", "--json"]));
    let row = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["ticket"] == format!("{REPO}#{t}"))
        .unwrap_or_else(|| panic!("ticket not listed: {v}"));
    assert_eq!(row["status"], "Ready");
    assert_eq!(row["agent"], b.name.as_str());
    assert!(
        row["age_s"].as_i64().is_some_and(|a| (0..600).contains(&a)),
        "{row}"
    );
    let live = |v: &Value, n: &str| -> String {
        v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == n)
            .unwrap_or_else(|| panic!("{n}: {v}"))["liveness"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(live(&v, &b.name), "alive", "{v}");
    assert_eq!(v["heartbeat_interval_s"], 600);
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains("tickets (") && o.stdout.contains(&format!("{REPO}#{t}")),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("liveness=alive"), "{}", o.stdout);
    // control: a heartbeat far older than the interval is not alive
    fake.set_item(
        &b.item_id,
        "Heartbeat",
        &crewboard::clock::format_ts(crewboard::clock::now_epoch() - 6 * 3600),
    );
    let v = json_of(&run_ok(&fake, &c, &["status", "--json"]));
    assert_ne!(live(&v, &b.name), "alive", "{v}");
    no_unhandled(&fake);
}

/// A move posts the signed assignment: the signed log names the new agent, so its `take` works
/// (it was refused with `ticket-agent-not-signed` while the move posted a plain request).
#[test]
fn a_queue_move_posts_the_signed_assignment_so_the_new_agent_can_take() {
    let (fake, c, a, b) = crew();
    let t = ticket(&fake, "IT move signs");
    let s = t.to_string();
    run_ok(&fake, &c, &["assign", &s, "--to", &b.name, "--order", "3"]);
    run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
    let m = moved_message(&fake, t);
    assert!(m.contains("Assigned to you: "), "{m}");
    assert!(m.contains(&format!("ticket: {REPO}#{t}")), "{m}");
    assert!(m.contains("gate: "), "{m}");
    assert!(m.contains("ci: none"), "{m}");
    assert!(m.contains("order: 1"), "{m}");
    assert!(m.contains(&format!("moved-from: {}", b.name)), "{m}");
    // the move still names its new agent in the output, with the assignment message line
    let t2 = ticket(&fake, "IT move signs, second");
    let s2 = t2.to_string();
    run_ok(&fake, &c, &["assign", &s2, "--to", &b.name, "--order", "3"]);
    let o = run_ok(&fake, &c, &["assign", &s2, "--to", &a.name]);
    assert!(o.stdout.contains("assignment message "), "{}", o.stdout);
    // the new agent takes the first; the old agent is refused the second
    run_ok(&fake, &a, &["take", &s]);
    run_refused(&fake, &b, &["take", &s2], "status-wrong-writer");
    no_unhandled(&fake);
}

/// The repair of an unsigned Agent keeps the Order that the ticket has; a ticket without one
/// goes to the end of the queue.
#[test]
fn assign_repairing_an_unsigned_agent_keeps_the_existing_order() {
    let (fake, c, _a, b) = crew();
    let t0 = ticket(&fake, "IT repair queue");
    run_ok(
        &fake,
        &c,
        &["assign", &t0.to_string(), "--to", &b.name, "--order", "5"],
    );
    let t1 = ticket(&fake, "IT repair keeps order");
    let t2 = ticket(&fake, "IT repair without order");
    for (t, order) in [(t1, Some("1")), (t2, None)] {
        let s = t.to_string();
        run_ok(&fake, &c, &["ticket", "adopt", &s]);
        let id = item(&fake, t);
        fake.set_item(&id, "Status", "Ready");
        fake.set_item(&id, "Agent", &b.name);
        if let Some(o) = order {
            fake.set_item(&id, "Order", o);
        }
        let o = run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
        assert!(
            o.stdout.contains("repaired an unsigned assignment"),
            "{}",
            o.stdout
        );
    }
    assert_eq!(
        field(&fake, &item(&fake, t1), "Order").as_deref(),
        Some("1")
    );
    assert_eq!(assigned_order(&fake, t1).as_deref(), Some("1"));
    // control: a ticket without an Order goes to the end of the queue (after Order 5)
    assert_eq!(
        field(&fake, &item(&fake, t2), "Order").as_deref(),
        Some("6")
    );
    assert_eq!(assigned_order(&fake, t2).as_deref(), Some("6"));
    no_unhandled(&fake);
}

/// The assignment messages of a ticket, oldest first.
fn assignments(fake: &FakeBoard, issue: u64) -> Vec<String> {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains("Assigned to you"))
        .collect()
}

const SIGNED_GATE: &str = "`cargo test` passes and the demo runs";

/// A body edit while the ticket waits in a queue is not signed by a later assignment message:
/// the move to another agent, and the `assign` after a `reassign`, keep the signed Gate and CI
/// line and warn about the body.
#[test]
fn a_later_assignment_keeps_the_signed_gate_and_ci_line_when_the_body_was_edited() {
    let (fake, c, a, b) = crew();
    let t = fake.ticket_issue(
        "IT move keeps the signed gate",
        &format!("Goal: x.\n\nGate: {SIGNED_GATE}\n"),
        &["crew:work"],
    );
    let s = t.to_string();
    run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
    let first = assignments(&fake, t).pop().expect("the first assignment");
    assert!(first.contains(&format!("gate: {SIGNED_GATE}\n")), "{first}");
    assert!(first.contains("ci: required\n"), "{first}");
    // a holder of an agent token edits the body: a weaker Gate and `CI: none`
    fake.set_issue_body(t, "Goal: x.\n\nGate: ci\nCI: none\n");
    // somebody wrote the field too: the move puts the signed Gate back
    fake.set_item(&item(&fake, t), "Gate", "ci");
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
    assert!(
        o.stdout.contains("Gate field set from the signed Gate"),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item(&fake, t), "Gate").as_deref(),
        Some(SIGNED_GATE)
    );
    let m = moved_message(&fake, t);
    assert!(m.contains(&format!("moved-from: {}", b.name)), "{m}");
    assert!(m.contains(&format!("gate: {SIGNED_GATE}\n")), "{m}");
    assert!(m.contains("ci: required\n"), "{m}");
    assert!(!m.contains("gate: ci") && !m.contains("ci: none"), "{m}");
    // the answer says that the body differs and that the signed lines stay
    let warnings: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.starts_with("warning: "))
        .collect();
    assert_eq!(warnings.len(), 2, "{}", o.stdout);
    assert!(
        warnings[0].contains("the Gate line in the body")
            && warnings[0].contains("the body was edited after the assignment")
            && warnings[0].ends_with(&format!("The signed Gate stays: '{SIGNED_GATE}'")),
        "{}",
        o.stdout
    );
    assert!(
        warnings[1].contains("the CI line in the body") && warnings[1].ends_with("'ci: required'"),
        "{}",
        o.stdout
    );
    // the same for the `assign` after a `reassign` to Backlog: the ticket has a signed Gate
    run_ok(&fake, &c, &["reassign", &s, "--reason", "dead"]);
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
    let last = assignments(&fake, t).pop().expect("the last assignment");
    assert!(!last.contains("moved-from: "), "{last}");
    assert!(last.contains(&format!("gate: {SIGNED_GATE}\n")), "{last}");
    assert!(last.contains("ci: required\n"), "{last}");
    assert!(
        o.stdout.contains("Gate field set from the signed Gate"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("signed from the body"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains("warning: the Gate line in the body of fake-owner/fake-repo#"),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item(&fake, t), "Gate").as_deref(),
        Some(SIGNED_GATE)
    );
    // control: a move with an unchanged body gives no warning and the same lines
    let t2 = fake.ticket_issue(
        "IT move without an edit",
        &format!("Goal: x.\n\nGate: {SIGNED_GATE}\nCI: none\n"),
        &["crew:work"],
    );
    let s2 = t2.to_string();
    run_ok(&fake, &c, &["assign", &s2, "--to", &b.name]);
    let o = run_ok(&fake, &c, &["assign", &s2, "--to", &a.name]);
    assert!(!o.stdout.contains("warning: "), "{}", o.stdout);
    let m = moved_message(&fake, t2);
    assert!(m.contains(&format!("gate: {SIGNED_GATE}\n")), "{m}");
    assert!(m.contains("ci: none\n"), "{m}");
    no_unhandled(&fake);
}

/// A ticket with the Gate [`SIGNED_GATE`], no `CI: none`.
fn gated(fake: &FakeBoard, title: &str) -> u64 {
    fake.ticket_issue(
        title,
        &format!("Goal: x.\n\nGate: {SIGNED_GATE}\n"),
        &["crew:work"],
    )
}

const EDITED_BODY: &str = "Goal: x.\n\nGate: ci\nCI: none\n";

/// `reassign --to` copies the signed lines and warns about an edited body, as the move does.
#[test]
fn reassign_to_keeps_the_signed_lines_and_warns_when_the_body_was_edited() {
    let (fake, c, a, b) = crew();
    let t = gated(&fake, "IT reassign keeps the signed gate");
    let s = t.to_string();
    run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
    fake.set_issue_body(t, EDITED_BODY);
    // somebody wrote the field too
    fake.set_item(&item(&fake, t), "Gate", "ci");
    let o = run_ok(
        &fake,
        &c,
        &["reassign", &s, "--reason", "dead", "--to", &a.name],
    );
    assert!(
        o.stdout.contains("Gate field set from the signed Gate"),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item(&fake, t), "Gate").as_deref(),
        Some(SIGNED_GATE)
    );
    let last = assignments(&fake, t).pop().expect("the last assignment");
    assert!(last.contains("reason: "), "{last}");
    assert!(last.contains(&format!("gate: {SIGNED_GATE}\n")), "{last}");
    assert!(last.contains("ci: required\n"), "{last}");
    let warnings: Vec<&str> = o
        .stdout
        .lines()
        .filter(|l| l.starts_with("warning: the "))
        .collect();
    assert_eq!(warnings.len(), 2, "{}", o.stdout);
    assert!(
        warnings[0].ends_with(&format!("The signed Gate stays: '{SIGNED_GATE}'")),
        "{}",
        o.stdout
    );
    assert!(warnings[1].ends_with("'ci: required'"), "{}", o.stdout);
    no_unhandled(&fake);
}

/// Two Gate lines of the body are one signed line (` | `): the body agrees with it, so a later
/// assignment gives no warning.
#[test]
fn a_body_with_two_gate_lines_agrees_with_its_signed_line() {
    let (fake, c, a, b) = crew();
    let t = fake.ticket_issue(
        "IT two gate lines",
        "Goal: x.\n\nGate: the build passes\n- **Gate:** the demo runs\n",
        &["crew:work"],
    );
    let s = t.to_string();
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
    assert!(
        o.stdout.contains(
            "signed from the body: gate 'the build passes | the demo runs'; guard none; ci: required"
        ),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("warning: "), "{}", o.stdout);
    // the field holds what is signed: both lines, also after `ticket adopt` and the move
    let joined = Some("the build passes | the demo runs");
    assert_eq!(field(&fake, &item(&fake, t), "Gate").as_deref(), joined);
    let o = run_ok(&fake, &c, &["ticket", "adopt", &s]);
    assert!(!o.stdout.contains("Gate field"), "{}", o.stdout);
    assert_eq!(field(&fake, &item(&fake, t), "Gate").as_deref(), joined);
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
    assert!(!o.stdout.contains("warning: "), "{}", o.stdout);
    assert!(!o.stdout.contains("signed from the body"), "{}", o.stdout);
    assert!(!o.stdout.contains("Gate field"), "{}", o.stdout);
    assert_eq!(field(&fake, &item(&fake, t), "Gate").as_deref(), joined);
    let m = moved_message(&fake, t);
    assert!(
        m.contains("gate: the build passes | the demo runs\n"),
        "{m}"
    );
    no_unhandled(&fake);
}

/// The assignment comment is deleted or edited, so the log has no assignment, and the body is
/// edited. The next `assign` signs the body: it says so, and it warns because the board shows
/// an assigned ticket.
#[test]
fn assign_of_a_ticket_whose_assignment_message_is_gone_warns_and_names_what_it_signs() {
    let (fake, c, a, b) = crew();
    for deleted in [true, false] {
        let t = gated(&fake, "IT assignment message gone");
        let s = t.to_string();
        // control: the first assignment names what it signs and has no warning
        let o = run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
        assert!(
            o.stdout.contains(&format!(
                "signed from the body: gate '{SIGNED_GATE}'; guard none; ci: required"
            )),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("warning: "), "{}", o.stdout);
        let (id, _, text) = fake
            .comments_of(t)
            .into_iter()
            .find(|(_, _, b)| b.contains("Assigned to you"))
            .expect("the assignment comment");
        if deleted {
            fake.delete_comment(id);
        } else {
            fake.edit_comment(id, &text.replace(SIGNED_GATE, "ci"));
        }
        fake.set_issue_body(t, EDITED_BODY);
        let o = run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
        let key = format!("{REPO}#{t}");
        assert!(
            o.stdout.contains(&format!(
                "warning: {key} has an Agent or a Status past Backlog, but its log has no valid signed assignment (never posted, deleted, edited or not valid)"
            )) && o
                .stdout
                .contains("The Gate and the guard now come from the unsigned body: check both (guard none)"),
            "deleted={deleted}: {}",
            o.stdout
        );
        assert!(
            o.stdout
                .contains("signed from the body: gate 'ci'; guard none; ci: none"),
            "deleted={deleted}: {}",
            o.stdout
        );
        // the new message holds what the answer names
        let last = assignments(&fake, t).pop().expect("the new assignment");
        assert!(
            last.contains("gate: ci\n") && last.contains("ci: none\n"),
            "deleted={deleted}: {last}"
        );
        assert_eq!(field(&fake, &item(&fake, t), "Gate").as_deref(), Some("ci"));
    }
    no_unhandled(&fake);
}

/// An assignment without a `gate:` line (a ticket of an earlier version): a move copies what the
/// assignment has, so the new message has no Gate of the body either.
#[test]
fn a_move_of_a_ticket_whose_assignment_has_no_gate_line_does_not_sign_the_body() {
    let (fake, c, a, b) = crew();
    let t = gated(&fake, "IT assignment without a gate line");
    let s = t.to_string();
    run_ok(&fake, &c, &["ticket", "adopt", &s]);
    let id = item(&fake, t);
    fake.signed(
        &c.key,
        &c.name,
        &b.name,
        crewboard::protocol::names::Kind::Request,
        t,
        &format!("Assigned to you: T\nticket: {REPO}#{t}\nitem={id}\n"),
    );
    fake.set_item(&id, "Status", "Ready");
    fake.set_item(&id, "Agent", &b.name);
    fake.set_issue_body(t, EDITED_BODY);
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
    let m = moved_message(&fake, t);
    assert!(m.contains(&format!("moved-from: {}", b.name)), "{m}");
    assert!(!m.contains("gate: ") && !m.contains("ci: "), "{m}");
    assert!(!o.stdout.contains("signed from the body"), "{}", o.stdout);
    no_unhandled(&fake);
}

/// `ticket adopt` writes the signed Gate into the `Gate` field, not the Gate of an edited body.
#[test]
fn adopt_writes_the_signed_gate_into_the_field_when_the_body_was_edited() {
    let (fake, c, _a, b) = crew();
    let t = gated(&fake, "IT adopt keeps the signed gate");
    let s = t.to_string();
    run_ok(&fake, &c, &["assign", &s, "--to", &b.name]);
    let id = item(&fake, t);
    fake.set_issue_body(t, EDITED_BODY);
    // the field agrees with the signed Gate: nothing is written
    let o = run_ok(&fake, &c, &["ticket", "adopt", &s]);
    assert!(!o.stdout.contains("Gate field set"), "{}", o.stdout);
    assert_eq!(field(&fake, &id, "Gate").as_deref(), Some(SIGNED_GATE));
    // somebody wrote the field: the signed Gate comes back
    fake.set_item(&id, "Gate", "ci");
    let o = run_ok(&fake, &c, &["ticket", "adopt", &s]);
    assert!(
        o.stdout.contains("Gate field set from the signed Gate"),
        "{}",
        o.stdout
    );
    assert_eq!(field(&fake, &id, "Gate").as_deref(), Some(SIGNED_GATE));
    no_unhandled(&fake);
}

/// The same for `assign` after a `reassign`: the log has an assignment, so `assign` copies what
/// it has and does not sign the body, also when that assignment has no `gate:` line.
#[test]
fn assign_after_a_reassign_of_a_ticket_without_a_signed_gate_does_not_sign_the_body() {
    let (fake, c, a, b) = crew();
    let t = gated(&fake, "IT assign without a signed gate");
    let s = t.to_string();
    run_ok(&fake, &c, &["ticket", "adopt", &s]);
    let id = item(&fake, t);
    fake.signed(
        &c.key,
        &c.name,
        &b.name,
        crewboard::protocol::names::Kind::Request,
        t,
        &format!("Assigned to you: T\nticket: {REPO}#{t}\nitem={id}\n"),
    );
    fake.set_item(&id, "Status", "Ready");
    fake.set_item(&id, "Agent", &b.name);
    run_ok(&fake, &c, &["reassign", &s, "--reason", "dead"]);
    fake.set_issue_body(t, EDITED_BODY);
    let before = assignments(&fake, t).len();
    let o = run_ok(&fake, &c, &["assign", &s, "--to", &a.name]);
    let all = assignments(&fake, t);
    assert_eq!(all.len(), before + 1, "{all:?}");
    let last = all.last().expect("the new assignment");
    assert!(last.contains("order: "), "{last}");
    assert!(!last.contains("gate: ") && !last.contains("ci: "), "{last}");
    assert!(!o.stdout.contains("signed from the body"), "{}", o.stdout);
    assert!(
        o.stdout.contains(&format!(
            "note: {REPO}#{t} has no signed Gate in its log, so the new assignment message has no `gate:` line either"
        )),
        "{}",
        o.stdout
    );
    no_unhandled(&fake);
}
