//! A question to the owner (spec/tickets#status-flow-and-writers), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! The case: the dev agent finished a ticket (`done`), then asked the owner for a release go
//! (`ask`). The ticket stays in Review and carries the label `needs-owner`. The coordinator
//! verifies, merges or closes it only after the owner answered ON THE TICKET. Each refusal
//! writes nothing. An `ask` of the assignee on an In Progress ticket also moves it to Blocked.
//! A ticket that an old binary put in Needs Human reads as a Blocked ticket with an open
//! question.

use crate::support::authority::AUTHORITY;
use crate::support::board::{Agent, FakeBoard, Out};
use crate::support::cli::{GATE, crew, item_in, run_ok, run_refused, writes};
use crewboard::rules::Role;

/// A ticket that declares `CI: none`, so the fake needs no CI run.
fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    fake.ticket_issue(title, &body, &["crew:work"])
}

/// Refused with `rule` (exit 2), and nothing was written.
fn refused_no_write(fake: &FakeBoard, who: &Agent, args: &[&str], rule: &str) -> Out {
    let before = writes(fake);
    let o = fake.run(&who.dir, args);
    assert_eq!(o.code, 2, "{} {args:?}: {}", who.name, o.all());
    assert!(
        o.stderr.contains(rule),
        "{rule} missing for {args:?}: {}",
        o.all()
    );
    assert_eq!(writes(fake), before, "the refusal wrote: {}", o.all());
    o
}

fn status(fake: &FakeBoard, item: &str) -> Option<String> {
    fake.item_field(item, "Status")
}

fn needs_owner(fake: &FakeBoard, t: u64) -> bool {
    fake.labels_of(t).contains(&"needs-owner".to_string())
}

/// The number of verify reports on ticket `t`.
fn reports(fake: &FakeBoard, t: u64) -> usize {
    fake.comments_of(t)
        .iter()
        .filter(|(_, _, b)| b.contains("verify: pass") || b.contains("verify: fail"))
        .count()
}

struct Case {
    t: u64,
    ts: String,
    item: String,
    pr: crate::support::board::Pull,
}

fn key(k: &Case) -> String {
    format!("{}#{}", crate::support::board::REPO, k.t)
}

/// Assign, take, PR and done: the ticket is in Review.
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Case {
    let t = ticket(fake, title);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    assert_eq!(status(fake, &item).as_deref(), Some("Review"));
    Case { t, ts, item, pr }
}

/// `delivered`, then the dev asks the owner for the release go: Review stays, the label is set.
fn asked(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Case {
    let k = delivered(fake, c, b, title);
    let o = run_ok(
        fake,
        b,
        &["ask", &k.ts, "--body", "Release go for this change?"],
    );
    assert!(
        o.stdout.contains(&format!("{} question ", key(&k)))
            && o.stdout.contains("; label needs-owner"),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("Status Blocked"), "{}", o.all());
    assert_eq!(status(fake, &k.item).as_deref(), Some("Review"));
    assert!(needs_owner(fake, k.t));
    k
}

/// `asked`, then the owner answers on the ticket.
fn answered(fake: &FakeBoard, c: &Agent, b: &Agent, title: &str) -> Case {
    let k = asked(fake, c, b, title);
    fake.owner_comment_now(k.t, "Go: release it.");
    k
}

#[test]
fn after_the_owners_answer_verify_passes_and_close_moves_the_ticket_to_done() {
    let (fake, c, b) = crew();
    let k = answered(&fake, &c, &b, "IT question close");
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    // a pass writes no status
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Review"));
    // `close --verified` closes a PR that somebody else merged at the verified head
    fake.merge_pr_by(k.pr.number, "a-maintainer");
    let o = run_ok(&fake, &c, &["close", &k.ts, "--verified"]);
    assert!(o.stdout.contains("closed: Status Done"), "{}", o.all());
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Done"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
    assert!(
        fake.comments_of(k.t)
            .iter()
            .any(|(_, _, body)| body.contains("Closed after a passed verify."))
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn after_the_owners_answer_merge_merges_at_the_verified_head_and_closes() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = answered(&fake, &c, &b, "IT question merge");
    // refusal first: merge needs a passed verify, also after the answer
    refused_no_write(&fake, &c, &["merge", &k.ts], "merge-verify-required");
    run_ok(&fake, &c, &["verify", &k.ts]);
    let o = run_ok(&fake, &c, &["merge", &k.ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
    assert!(o.stdout.contains(&k.pr.head), "{}", o.all());
    assert!(fake.pr_now(k.pr.number).0);
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Done"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn with_an_open_question_verify_close_and_merge_are_refused_and_a_chat_answer_does_not_count() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = asked(&fake, &c, &b, "IT question open");
    // an answer of the coordinator is not the owner's (a chat answer cannot be checked either)
    let _ = fake.poll_json(&c);
    run_ok(
        &fake,
        &c,
        &[
            "say",
            &k.ts,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "The owner said yes in the chat",
        ],
    );
    let o = refused_no_write(&fake, &c, &["verify", &k.ts], "verify-owner-question-open");
    assert!(
        o.stderr
            .contains(&format!("{} has a question to the owner since", key(&k)))
            && o.stderr
                .contains(&format!("crewboard reassign {} --to <agent> --reason", k.t))
            && o.stderr.contains("an answer in a chat is not signed"),
        "{}",
        o.all()
    );
    refused_no_write(
        &fake,
        &c,
        &["close", &k.ts, "--verified"],
        "verify-owner-question-open",
    );
    refused_no_write(&fake, &c, &["merge", &k.ts], "verify-owner-question-open");
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Review"));
    assert!(needs_owner(&fake, k.t));
    assert_eq!(reports(&fake, k.t), 0);
    // control: the owner answers on the ticket, and verify passes
    fake.owner_comment_now(k.t, "Yes, release.");
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn only_the_assignee_or_the_coordinator_asks_and_a_coordinator_ask_moves_no_status() {
    let (fake, c, b) = crew();
    let d = fake.agent("other@fake-d", Role::Dev);
    let k = delivered(&fake, &c, &b, "IT question writer");
    let o = refused_no_write(
        &fake,
        &d,
        &["ask", &k.ts, "--body", "May I?"],
        "ask-not-assignee",
    );
    assert!(
        o.stderr.contains(&format!(
            "only the assignee or the coordinator asks the owner about {}",
            key(&k)
        )),
        "{}",
        o.all()
    );
    assert!(!needs_owner(&fake, k.t));
    // the coordinator asks about a ticket of the dev: Review stays, the label is set
    let _ = fake.poll_json(&c);
    let o = run_ok(&fake, &c, &["ask", &k.ts, "--body", "Is the scope right?"]);
    assert!(o.stdout.contains("; label needs-owner"), "{}", o.all());
    assert!(!o.stdout.contains("Status "), "{}", o.all());
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Review"));
    assert!(needs_owner(&fake, k.t));
}

#[test]
fn ask_on_a_done_ticket_is_refused() {
    let (fake, c, b) = crew();
    let k = delivered(&fake, &c, &b, "IT question done");
    run_ok(&fake, &c, &["verify", &k.ts]);
    fake.merge_pr(k.pr.number);
    run_ok(&fake, &c, &["close", &k.ts, "--verified"]);
    refused_no_write(
        &fake,
        &c,
        &["ask", &k.ts, "--body", "One more thing?"],
        "status-done-final",
    );
}

#[test]
fn ask_blocks_the_in_progress_ticket_and_take_resumes_it_after_the_owners_answer() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT question blocks");
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let o = run_ok(&fake, &b, &["ask", &ts, "--body", "Which layout?"]);
    assert!(
        o.stdout.contains("; label needs-owner") && o.stdout.contains("Status Blocked"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &item).as_deref(), Some("Blocked"));
    assert!(needs_owner(&fake, t));
    // `mine` says the ticket waits for the owner
    let m = fake.run(&b.dir, &["mine"]);
    assert!(
        m.stdout.contains("Blocked, waits for the owner"),
        "{}",
        m.all()
    );
    // a coordinator answer does not release it, the owner's message does
    let _ = fake.poll_json(&c);
    run_ok(
        &fake,
        &c,
        &[
            "say",
            &ts,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "I think the first",
        ],
    );
    let _ = fake.poll_json(&b);
    run_refused(&fake, &b, &["take", &ts], "status-answer-missing");
    fake.owner_comment_now(t, "The second one.");
    let _ = fake.poll_json(&b);
    let m = fake.run(&b.dir, &["mine"]);
    assert!(m.stdout.contains("Blocked, answered"), "{}", m.all());
    let o = run_ok(&fake, &b, &["take", &ts]);
    assert!(o.stdout.contains("resumed"), "{}", o.all());
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn mine_says_a_review_ticket_with_an_open_question_waits_for_the_owner() {
    let (fake, c, b) = crew();
    let k = asked(&fake, &c, &b, "IT question mine");
    let m = fake.run(&b.dir, &["mine"]);
    assert_eq!(m.code, 1, "{}", m.all());
    assert!(
        m.stdout.contains("Review, waits for the owner's answer"),
        "{}",
        m.all()
    );
    fake.owner_comment_now(k.t, "Go.");
    let m = fake.run(&b.dir, &["mine"]);
    assert!(
        m.stdout.contains("Review, waits for the coordinator"),
        "{}",
        m.all()
    );
}

#[test]
fn reassign_to_the_same_agent_takes_the_question_back_and_verify_stays_refused_until_then() {
    let (fake, c, b) = crew();
    let k = asked(&fake, &c, &b, "IT question withdraw");
    let o = run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &k.ts,
            "--to",
            &b.name,
            "--reason",
            "late",
            "--why",
            "ask me, not the owner",
        ],
    );
    assert!(o.stdout.contains("answer to "), "{}", o.all());
    assert!(o.stdout.contains("info to the owner"), "{}", o.all());
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Ready"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
    let bodies: Vec<String> = fake
        .comments_of(k.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.contains("ask me, not the owner") && b.contains("withdraws: question")),
        "{bodies:?}"
    );
    assert!(
        bodies.iter().any(|b| b.contains(&format!(
            "question on #{} taken back by the coordinator",
            k.t
        ))),
        "{bodies:?}"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_moved_head_fails_as_from_review_and_the_label_goes() {
    let (fake, c, b) = crew();
    let k = answered(&fake, &c, &b, "IT question head moved");
    fake.push(&k.pr.branch);
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(
        o.stdout.contains("verify FAIL") && o.stdout.contains("PR head moved"),
        "{}",
        o.all()
    );
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert_eq!(status(&fake, &k.item).as_deref(), Some("In Progress"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
    // close after the failed verify is refused
    let o = fake.run(&c.dir, &["close", &k.ts, "--verified"]);
    assert_eq!(o.code, 2, "{}", o.all());
}

#[test]
fn a_headless_coordinator_never_acts_on_a_ticket_with_an_open_question() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = answered(&fake, &c, &b, "IT question headless");
    // the answer closes the question; ask again so that one is open
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["ask", &k.ts, "--body", "And the docs?"]);
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    for args in [vec!["verify", k.ts.as_str()], vec!["merge", k.ts.as_str()]] {
        let before = writes(&fake);
        let o = fake.run(headless.path(), &args);
        assert_eq!(o.code, 2, "{args:?}: {}", o.all());
        assert!(
            o.stderr.contains("[rule=headless-scope]")
                && o.stderr.contains("has an open question to the owner"),
            "{args:?}: {}",
            o.all()
        );
        assert_eq!(writes(&fake), before, "{args:?}: {}", o.all());
    }
    assert_eq!(reports(&fake, k.t), 0);
    assert!(!fake.pr_now(k.pr.number).0);
    // control: after the owner answered, the interactive coordinator verifies the same ticket
    fake.owner_comment_now(k.t, "Docs are fine.");
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn a_question_of_a_dev_that_is_not_the_assignee_opens_nothing() {
    let (fake, c, b) = crew();
    let d = fake.agent("other@fake-d", Role::Dev);
    let k = delivered(&fake, &c, &b, "IT question stranger");
    let _ = fake.poll_json(&d);
    run_ok(
        &fake,
        &d,
        &[
            "say", &k.ts, "--to", "owner", "--kind", "question", "--body", "May I?",
        ],
    );
    // the signed question is on the ticket, but it opens nothing and blocks nothing
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn reassign_to_another_agent_withdraws_the_question_and_the_new_done_verifies() {
    let (fake, c, b) = crew();
    let d = fake.agent("other@fake-d", Role::Dev);
    let k = asked(&fake, &c, &b, "IT question reassign");
    let o = run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &k.ts,
            "--to",
            &d.name,
            "--reason",
            "late",
            "--why",
            "other agent",
        ],
    );
    assert!(o.stdout.contains("info to the owner"), "{}", o.all());
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Ready"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
    assert!(
        fake.comments_of(k.t)
            .iter()
            .any(|(_, _, b)| b.contains("withdraws: question")),
    );
    // the new agent delivers; verify works without an owner answer
    let _ = fake.poll_json(&d);
    run_ok(&fake, &d, &["take", &k.ts]);
    let pr = fake.pr_for(k.t, &d.name);
    let _ = fake.poll_json(&d);
    run_ok(
        &fake,
        &d,
        &["done", &k.ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn a_moved_head_of_a_busy_agent_queues_the_ticket_in_ready() {
    let (fake, c, b) = crew();
    let k = answered(&fake, &c, &b, "IT question busy");
    // after `done` the agent is free and takes another ticket
    let other = ticket(&fake, "IT question other");
    let other_item = item_in(&run_ok(
        &fake,
        &c,
        &["assign", &other.to_string(), "--to", &b.name],
    ));
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &other.to_string()]);
    fake.push(&k.pr.branch);
    let o = run_ok(&fake, &c, &["verify", &k.ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(o.stdout.contains("Status moved to Ready"), "{}", o.all());
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Ready"));
    assert_eq!(
        fake.item_field(&k.item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert!(fake.item_field(&k.item, "Order").is_some());
    assert_eq!(status(&fake, &other_item).as_deref(), Some("In Progress"));
    assert!(!needs_owner(&fake, k.t), "{:?}", fake.labels_of(k.t));
}

#[test]
fn the_headless_tick_leaves_a_ticket_with_an_open_question() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = asked(&fake, &c, &b, "IT question tick");
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    let before = writes(&fake);
    let o = fake.run(headless.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(writes(&fake), before, "{}", o.all());
    assert_eq!(reports(&fake, k.t), 0);
    assert!(!fake.pr_now(k.pr.number).0);
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Review"));
}

// ---- legacy: a ticket that an old binary put in Needs Human --------------------------------

#[test]
fn a_ticket_set_to_needs_human_by_hand_reads_as_blocked_and_runs_through_to_a_passed_verify() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT legacy resume");
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["ask", &ts, "--body", "Which layout?"]);
    // an old binary wrote this option; the new binary reads it as Blocked
    fake.set_item(&item, "Status", "Needs Human");
    let m = fake.run(&b.dir, &["mine"]);
    assert_eq!(m.code, 1, "{}", m.all());
    assert!(
        m.stdout.contains("Blocked, waits for the owner"),
        "{}",
        m.all()
    );
    fake.owner_comment_now(t, "The second one.");
    let _ = fake.poll_json(&b);
    let m = fake.run(&b.dir, &["mine"]);
    assert_eq!(m.code, 0, "{}", m.all());
    assert!(m.stdout.contains("Blocked, answered"), "{}", m.all());
    let o = run_ok(&fake, &b, &["take", &ts]);
    assert!(o.stdout.contains("resumed"), "{}", o.all());
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert_eq!(status(&fake, &item).as_deref(), Some("Review"));
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn on_a_ticket_set_to_needs_human_by_hand_verify_merge_and_close_verified_need_a_ticket_in_review()
{
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let k = answered(&fake, &c, &b, "IT legacy exits");
    // an old binary wrote this option; the new binary reads it as Blocked
    fake.set_item(&k.item, "Status", "Needs Human");
    refused_no_write(&fake, &c, &["verify", &k.ts], "verify-needs-review");
    refused_no_write(&fake, &c, &["merge", &k.ts], "verify-needs-review");
    refused_no_write(
        &fake,
        &c,
        &["close", &k.ts, "--verified"],
        "status-unknown-transition",
    );
    assert_eq!(status(&fake, &k.item).as_deref(), Some("Needs Human"));
}

#[test]
fn unblock_withdraws_an_owner_question_that_a_plain_answer_leaves_open() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT unblock question");
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["ask", &ts, "--body", "Which layout?"]);
    let _ = fake.poll_json(&c);
    // a plain coordinator answer warns on stderr and is posted
    let o = run_ok(
        &fake,
        &c,
        &[
            "say",
            &ts,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "the first",
        ],
    );
    assert!(
        o.stderr.contains("a plain answer does not unblock")
            && o.stderr.contains(&format!("crewboard unblock {ts}")),
        "{}",
        o.all()
    );
    // take names exactly what is missing
    let _ = fake.poll_json(&b);
    let o = run_refused(&fake, &b, &["take", &ts], "status-answer-missing");
    assert!(
        o.stderr.contains("is Blocked by a question to the owner")
            && o.stderr.contains(&format!("`crewboard unblock {ts}`"))
            && o.stderr
                .contains("Do not ask again: wait for the answer, then take."),
        "{}",
        o.all()
    );
    // unblock says that the question is withdrawn, and take passes
    let o = run_ok(&fake, &c, &["unblock", &ts]);
    assert!(
        o.stdout
            .contains("unblocked: the open question to the owner is withdrawn"),
        "{}",
        o.all()
    );
    let _ = fake.poll_json(&b);
    let o = run_ok(&fake, &b, &["take", &ts]);
    assert!(o.stdout.contains("resumed"), "{}", o.all());
    assert_eq!(status(&fake, &item).as_deref(), Some("In Progress"));
    assert!(!needs_owner(&fake, t), "{:?}", fake.labels_of(t));
    // control: an answer that already withdraws the question gives no warning
    run_ok(&fake, &b, &["ask", &ts, "--body", "And the colour?"]);
    let _ = fake.poll_json(&c);
    let o = run_ok(
        &fake,
        &c,
        &[
            "say",
            &ts,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "blue\nwithdraws: question",
        ],
    );
    assert!(!o.stderr.contains("does not unblock"), "{}", o.all());
}
