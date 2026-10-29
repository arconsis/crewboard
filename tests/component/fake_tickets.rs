//! Ticket lifecycle of the CLI against a fake GitHub.
//!
//! These tests run in plain `cargo test`: no token, no sandbox, no real GitHub. They replace
//! nine tests of `it_sandbox.rs` (see `CONTRIBUTING.md`). The fake lives in
//! `tests/support/board.rs` and `tests/support/board/tickets.rs`. A coordinator and a dev
//! agent are made by `FakeBoard::agent`; the issues, branches and PRs that a person or the
//! dev agent makes outside crewboard are seeded by `ticket_issue`, `pr_for` and `push`.

use crate::support::board::{Agent, FakeBoard, Out, REGISTRY, REPO};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused, writes};
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use serde_json::Value;

/// AUTHORITY.md that lets the coordinator merge.
const MERGE_AUTHORITY: &str = "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n";

/// A ticket issue as a person makes it. `ci_none` adds the `CI: none` line.
fn ticket(fake: &FakeBoard, title: &str, ci_none: bool, gate: bool) -> u64 {
    let mut body = String::from("Goal: integration test ticket.\n\n");
    if gate {
        body.push_str(GATE);
    }
    if ci_none {
        body.push_str("CI: none\n");
    }
    fake.ticket_issue(title, &body, &["crew:work"])
}

fn field(fake: &FakeBoard, item: &str, name: &str) -> Option<String> {
    fake.item_field(item, name)
}

/// The comment id in "... comment <id> ..." of an output.
fn comment_id(o: &Out) -> u64 {
    o.stdout
        .split("comment ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no comment id in: {}", o.stdout))
}

/// Assign `t` to B, B takes it, B opens a PR and reports done. Returns `(item, pr)`.
fn delivered(
    fake: &FakeBoard,
    c: &Agent,
    b: &Agent,
    t: u64,
) -> (String, crate::support::board::Pull) {
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    // the agent reads its events before `done` (`done` refuses with unread events)
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    (item, pr)
}

#[test]
fn a_ticket_goes_from_assign_to_close_under_the_writer_rules() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT round trip", true, true);
    let ts = t.to_string();

    // assign: Kind, Gate, Agent, Status Ready; the info comment carries the item id
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let item = item_in(&o);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        field(&fake, &item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(field(&fake, &item, "Kind").as_deref(), Some("Work"));
    assert!(
        field(&fake, &item, "Gate")
            .unwrap()
            .contains("PR of this ticket")
    );
    // Assign sets the label `agent: <name>` in the color of that name
    let want = format!("agent: {}", b.name);
    assert!(fake.labels_of(t).contains(&want), "{:?}", fake.labels_of(t));
    assert_eq!(
        fake.label_color(&want).as_deref(),
        Some(crewboard::labels::agent_label_color(&b.name))
    );
    // a second assign is refused by a rule
    run_refused(
        &fake,
        &c,
        &["assign", &ts, "--to", &b.name],
        "ticket-already-assigned",
    );

    // B's poll: the signed assignment and the board event
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    let evs = v["events"].as_array().unwrap();
    assert!(
        evs.iter().any(|e| e["class"] == "message"
            && e["from"] == c.name.as_str()
            && e["body"].as_str().unwrap().contains("Assigned to you")),
        "{v}"
    );
    assert!(
        evs.iter()
            .any(|e| e["class"] == "ticket" && e["kind"] == "assignment" && e["issue"] == t),
        "{v}"
    );

    // the coordinator cannot take; B can
    run_refused(&fake, &c, &["take", &ts], "coordinator-cannot-take");
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );

    // B's work: done with a head that moved is refused, with the real head it passes
    let pr = fake.pr_for(t, &b.name);
    let h2 = fake.push(&pr.branch);
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        "evidence-head-moved",
    );
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress"),
        "a refusal changes nothing"
    );
    let o = run_ok(&fake, &b, &["done", &ts, "--pr", &pr.url, "--sha", &h2]);
    assert!(o.stdout.contains("ci: none"), "{}", o.stdout);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    assert!(field(&fake, &item, "Evidence").unwrap().contains(&h2));

    // wrong writers and a missing verify
    run_refused(
        &fake,
        &b,
        &["close", &ts, "--verified"],
        "status-wrong-writer",
    );
    run_refused(
        &fake,
        &c,
        &["close", &ts, "--verified"],
        "status-verify-required",
    );

    // the coordinator's poll shows the done message; verify passes; close
    let (vc, _) = fake.poll_json(&c);
    assert!(
        vc["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "done" && e["body"].as_str().unwrap().contains(&h2)),
        "{vc}"
    );
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.stdout);
    // the pass ends with the full next command
    assert!(
        o.stdout.contains(&format!(
            "next: crewboard close {REPO}#{t} --verified (after you checked the Gate)\n"
        )),
        "{}",
        o.stdout
    );
    run_ok(&fake, &c, &["close", &ts, "--verified"]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Done"));
    // Close keeps the agent label as history
    assert!(fake.labels_of(t).contains(&want), "{:?}", fake.labels_of(t));
    no_unhandled(&fake);
}

#[test]
fn ci_none_needs_the_declaration_and_a_foreign_pr_is_refused() {
    let (fake, c, b) = crew();
    let undeclared = ticket(&fake, "IT ci not declared", false, true);
    let ts = undeclared.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(undeclared, &b.name);
    // no CI exists for the commit and the ticket does not declare 'CI: none': refused
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url],
        "evidence-ci-missing",
    );
    // a PR of a repo outside the initiative is refused before any call to it
    let before = fake.requests().len();
    run_refused(
        &fake,
        &b,
        &[
            "done",
            &ts,
            "--pr",
            "https://github.com/other-owner/other-repo/pull/1",
        ],
        "evidence-pr-foreign-repo",
    );
    assert!(
        fake.requests()[before..]
            .iter()
            .all(|r| !r.path.contains("other-repo") && !r.path.contains("/pulls/1")),
        "{:?}",
        &fake.requests()[before..]
    );
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );

    // control: the same kind of PR is accepted when the ticket declares 'CI: none'
    let declared = ticket(&fake, "IT ci declared", true, true);
    let tsd = declared.to_string();
    // B holds an open ticket, so the second one is taken after the first is reassigned
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--reason",
            "late",
            "--why",
            "it: next ticket",
        ],
    );
    let _ = fake.poll_json(&b);
    let item2 = item_in(&run_ok(&fake, &c, &["assign", &tsd, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &tsd]);
    let pr2 = fake.pr_for(declared, &b.name);
    run_ok(&fake, &b, &["done", &tsd, "--pr", &pr2.url]);
    assert_eq!(field(&fake, &item2, "Status").as_deref(), Some("Review"));
    no_unhandled(&fake);
}

#[test]
fn verify_fails_after_a_push_and_block_and_ask_resume_only_after_an_answer() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT verify fails", true, true);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    run_ok(&fake, &b, &["done", &ts, "--pr", &pr.url]);
    // a new push after done: verify fails and the ticket goes back to In Progress
    fake.push(&pr.branch);
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(
        o.stdout.contains("verify FAIL") && o.stdout.contains("PR head moved"),
        "{}",
        o.stdout
    );
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
    let o = fake.run(&c.dir, &["close", &ts, "--verified"]);
    assert_eq!(o.code, 2, "close after a failed verify: {}", o.all());

    // block; resume without an answer is refused; the owner answers; resume passes.
    // B reads the coordinator's verify result first (spec/events#unread-messages-on-a-ticket).
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", &ts, "--why", "need a decision"]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Blocked"));
    run_refused(&fake, &b, &["take", &ts], "status-answer-missing");
    fake.owner_comment_now(t, "Take the second approach.");
    let (v, _) = fake.poll_json(&b);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "owner"
                && e["body"].as_str().unwrap().contains("second approach")),
        "the owner message in B's ticket reaches B: {v}"
    );
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );

    // ask: Blocked; a coordinator answer is not enough, an owner message is
    run_ok(
        &fake,
        &b,
        &["ask", &ts, "--body", "May I change the layout?"],
    );
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Blocked"));
    // A ticket with an open question carries the label `needs-owner`
    assert!(
        fake.labels_of(t).contains(&"needs-owner".to_string()),
        "{:?}",
        fake.labels_of(t)
    );
    // the coordinator reads B's messages first (spec/events#unread-messages-on-a-ticket), then
    // answers
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
            "I think yes",
        ],
    );
    let o = fake.run(&b.dir, &["take", &ts]);
    assert_eq!(
        o.code,
        2,
        "a coordinator answer does not release a question: {}",
        o.all()
    );
    fake.owner_comment_now(t, "Yes, change it.");
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
    // ... and the ticket loses the label when the owner has answered and the agent takes it
    assert!(
        !fake.labels_of(t).contains(&"needs-owner".to_string()),
        "{:?}",
        fake.labels_of(t)
    );
    no_unhandled(&fake);
}

#[test]
fn an_unread_answer_refuses_say_and_override_unread_posts_and_the_cursor_stays() {
    let (fake, c, b) = crew();
    // the check guards board tickets only: make one, assign it to B, B takes it
    let t = ticket(&fake, "IT unread answer", true, true);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    // the coordinator reads B's ack first (the rule applies to every agent), then answers
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
            "decision: use the other crate",
        ],
    );
    let unread = comment_id(&o);
    let say = |extra: &[&str]| {
        let mut args = vec![
            "say",
            ts.as_str(),
            "--to",
            c.name.as_str(),
            "--kind",
            "report",
            "--body",
            "I hold until you decide",
        ];
        args.extend_from_slice(extra);
        fake.run(&b.dir, &args)
    };
    let posts = || fake.comments_of(t).len();

    // refusal: exit 2, the rule, the unread message named; nothing is posted
    let before = posts();
    let o = say(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    let text = format!("{}{}", o.stdout, o.stderr);
    assert!(text.contains("unread-message-on-ticket"), "{text}");
    assert!(text.contains(&format!("issuecomment-{unread}")), "{text}");
    // the refusal shows the text of the message: no extra watch call to see it
    assert!(text.contains("    decision: use the other crate"), "{text}");
    assert_eq!(posts(), before, "a refusal posts nothing");

    // --override-unread posts, with the override line
    let o = say(&["--override-unread"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("--override-unread"), "{}", o.stdout);
    assert!(
        o.stdout.contains(&format!(
            "override: unread-message-on-ticket (comments {unread})"
        )),
        "{}",
        o.stdout
    );
    assert_eq!(posts(), before + 1);

    // the cursor did not move: the next poll still delivers the message, then say passes
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["comment_id"].as_u64() == Some(unread)),
        "{v}"
    );
    let o = say(&[]);
    assert_eq!(o.code, 0, "say after the read: {}", o.all());
    assert_eq!(posts(), before + 2);
    no_unhandled(&fake);
}

/// spec/events#unread-messages-on-a-ticket: `take` prints and marks as read what came before its
/// ack; a message after the take still blocks `say` until a poll; the registry issue is never
/// guarded.
#[test]
fn take_marks_earlier_messages_read_and_the_registry_is_never_guarded() {
    let (fake, c, b) = crew();
    let a = fake.agent("claude@fake-a", Role::Dev);
    let t = ticket(&fake, "IT take marks read", true, true);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    // take prints the assignment message and counts it as read: say passes at once
    let o = run_ok(&fake, &b, &["take", &ts]);
    assert!(o.stdout.contains("marked as read"), "{}", o.stdout);
    assert!(o.stdout.contains("Assigned to you"), "{}", o.stdout);
    run_ok(
        &fake,
        &b,
        &[
            "say", &ts, "--to", &c.name, "--kind", "report", "--body", "started",
        ],
    );
    // a message after the take blocks say until B has polled. The coordinator has B's
    // report unread, so it reads first (the rule applies to every agent).
    let _ = fake.poll_json(&c);
    run_ok(
        &fake,
        &c,
        &[
            "say", &ts, "--to", &b.name, "--kind", "answer", "--body", "use x",
        ],
    );
    let say_b = || {
        fake.run(
            &b.dir,
            &[
                "say", &ts, "--to", &c.name, "--kind", "report", "--body", "waiting",
            ],
        )
    };
    let o = say_b();
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        format!("{}{}", o.stdout, o.stderr).contains("unread-message-on-ticket"),
        "{}",
        o.all()
    );
    let (v, code) = fake.poll_json(&b);
    assert_eq!(code, 0, "{v}");
    assert_eq!(say_b().code, 0, "say after the poll");

    // the registry issue is never guarded, even with a message to B on it. The message is
    // newer than B's cursor, so it IS unread: the poll after the say delivers it.
    let (reg_msg, _) = fake.signed_now(&a.key, &a.name, &b.name, Kind::Info, REGISTRY, "hello B");
    let o = fake.run(
        &b.dir,
        &[
            "say",
            &REGISTRY.to_string(),
            "--to",
            &a.name,
            "--kind",
            "info",
            "--body",
            "hello A",
        ],
    );
    assert_eq!(o.code, 0, "say on the registry issue: {}", o.all());
    let (v, _) = fake.poll_json(&b);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["comment_id"].as_u64() == Some(reg_msg)),
        "the registry message was unread, and say still passed: {v}"
    );
    no_unhandled(&fake);
}

#[test]
fn ticket_new_gate_ci_writes_the_typed_gate_and_refuses_another_gate() {
    let (fake, c, b) = crew();
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.md");
    let own = dir.path().join("own.md");
    std::fs::write(&plain, "Goal: make CI pass.\nScope: one file.\n").unwrap();
    std::fs::write(&own, "Goal: x.\nGate: run the demo by hand\n").unwrap();
    let (plain, own) = (plain.to_str().unwrap(), own.to_str().unwrap());
    // refusal: the body has its own Gate; nothing is created
    let o = fake.run(
        &c.dir,
        &[
            "ticket",
            "new",
            "--title",
            "T",
            "--body-file",
            own,
            "--gate-ci",
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("ticket-gate-conflict"), "{}", o.stderr);
    assert!(fake.open_titles("crew:work").is_empty());
    // pass: no Gate line in the body, the Gate field and the assignment carry `ci`
    let o = fake.run(
        &c.dir,
        &[
            "ticket",
            "new",
            "--title",
            "CI only",
            "--body-file",
            plain,
            "--gate-ci",
            "--assign",
            &b.name,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let t: u64 = o
        .stdout
        .split(&format!("{REPO}#"))
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{}", o.stdout));
    let item_id = fake.item_of_issue(t).expect("on the board");
    assert_eq!(field(&fake, &item_id, "Gate").as_deref(), Some("ci"));
    // the signed assignment holds the line that the headless tick accepts
    let (v, _) = fake.poll_json(&b);
    let req = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["body"].as_str())
        .find(|b| b.contains("gate:"))
        .unwrap_or_else(|| panic!("{v}"));
    assert!(req.lines().any(|l| l == "gate: ci"), "{req}");
}

#[test]
fn ticket_new_and_adopt_put_gated_tickets_on_the_board_and_status_lists_them() {
    let (fake, c, b) = crew();
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.md");
    let nogate = dir.path().join("nogate.md");
    std::fs::write(
        &good,
        format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
    )
    .unwrap();
    std::fs::write(&nogate, "Goal: a ticket without a gate.\n").unwrap();
    let (good, nogate) = (good.to_str().unwrap(), nogate.to_str().unwrap());
    let created = |o: &Out| -> u64 {
        o.stdout
            .split(&format!("{REPO}#"))
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("no created issue in: {}", o.stdout))
    };
    let new = |who: &Agent, title: &str, file: &str, extra: &[&str]| {
        let mut args = vec!["ticket", "new", "--title", title, "--body-file", file];
        args.extend_from_slice(extra);
        fake.run(&who.dir, &args)
    };
    let listed =
        |t: u64, status: &str| {
            let st = run_ok(&fake, &c, &["status", "--json"]);
            let sv: Value = serde_json::from_str(&st.stdout).unwrap();
            sv["tickets"].as_array().unwrap().iter().any(|x| {
                x["ticket"].as_str() == Some(&format!("{REPO}#{t}")) && x["status"] == status
            })
        };

    // refusal: no Gate line; nothing is created
    let o = new(&c, "IT new no gate", nogate, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("status-gate-missing"), "{}", o.stderr);
    assert!(
        o.stderr.contains("see `crewboard ticket new --template`"),
        "{}",
        o.stderr
    );
    // refusal: a dev agent is not the coordinator; nothing is created
    let o = new(&b, "IT new by dev", good, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("status-wrong-writer"), "{}", o.stderr);
    // refusal: --assign to the coordinator (not a dev) is checked before the issue is made
    let o = new(&c, "IT new bad assign", good, &["--assign", &c.name]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("not a dev agent"), "{}", o.stderr);
    assert!(
        fake.open_titles("crew:work").is_empty(),
        "a refusal creates nothing: {:?}",
        fake.open_titles("crew:work")
    );

    // pass: a valid ticket lands in Backlog, with Kind, Gate and the info comment
    let o = new(&c, "IT new parked", good, &["--backlog"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t = created(&o);
    let item = item_in(&o);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Backlog"));
    assert_eq!(field(&fake, &item, "Kind").as_deref(), Some("Work"));
    assert!(
        field(&fake, &item, "Gate")
            .unwrap()
            .contains("PR of this ticket")
    );
    assert!(fake.labels_of(t).contains(&"crew:work".to_string()));
    assert_eq!(fake.open_titles("crew:work"), vec!["IT new parked"]);
    // `status` lists it (this was the bug: no info comment, so the coordinator's view missed it)
    assert!(listed(t, "Backlog"), "status lists the parked ticket");
    // the parked ticket can be assigned afterwards
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Ready"));

    // adopt: an issue that exists already (here made without crewboard)
    let t2 = ticket(&fake, "IT adopt me", true, true);
    let o = run_ok(&fake, &c, &["ticket", "adopt", &t2.to_string()]);
    let item2 = item_in(&o);
    assert_eq!(field(&fake, &item2, "Status").as_deref(), Some("Backlog"));
    // a second adopt changes nothing and says so
    let writes = || {
        fake.requests()
            .iter()
            .filter(|r| r.method != "GET" && (r.path != "/graphql" || r.body.contains("mutation")))
            .count()
    };
    let before = writes();
    let o = run_ok(&fake, &c, &["ticket", "adopt", &t2.to_string()]);
    assert!(o.stdout.contains("on the board already"), "{}", o.stdout);
    assert_eq!(writes(), before, "a second adopt writes nothing");
    // refusal: an issue without a Gate line cannot be adopted
    let t3 = ticket(&fake, "IT adopt no gate", true, false);
    let o = fake.run(&c.dir, &["ticket", "adopt", &t3.to_string()]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("status-gate-missing"), "{}", o.stderr);
    assert!(fake.item_of_issue(t3).is_none(), "a refusal adds no item");

    // --assign: created, on the board, Ready, Agent and Order set
    let o = new(&c, "IT new assigned", good, &["--assign", &b.name]);
    assert_eq!(o.code, 0, "{}", o.all());
    let t4 = created(&o);
    let item4 = fake.item_of_issue(t4).expect("item of the new ticket");
    assert_eq!(field(&fake, &item4, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        field(&fake, &item4, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(field(&fake, &item4, "Order").as_deref(), Some("2"));
    no_unhandled(&fake);
}

/// Assign, take, PR, done and a passed verify for a fresh ticket.
fn in_verified_review(
    fake: &FakeBoard,
    c: &Agent,
    b: &Agent,
    title: &str,
) -> (u64, String, crate::support::board::Pull) {
    let t = ticket(fake, title, true, true);
    let (item, pr) = delivered(fake, c, b, t);
    let o = run_ok(fake, c, &["verify", &t.to_string()]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.stdout);
    (t, item, pr)
}

#[test]
fn merge_needs_the_authority_the_verified_head_and_then_merges_and_closes() {
    let (fake, c, b) = crew();

    // ---- ticket 1: refusals
    let (t1, item1, pr1) = in_verified_review(&fake, &c, &b, "IT merge refusals");
    let ts1 = t1.to_string();
    // authority denies (no AUTHORITY.md: the defaults have `coordinator_may_merge: false`)
    run_refused(&fake, &c, &["merge", &ts1], "authority-merge-not-allowed");
    assert!(!fake.pr_now(pr1.number).0);
    // an old file: the flag is true, but `merge` is still in needs_owner. The deprecated
    // entry still refuses, and the text names the key that replaces it.
    fake.file(
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_merge: true\nneeds_owner: [merge, release]\n---\n\nOld file.\n",
    );
    let o = run_refused(&fake, &c, &["merge", &ts1], "authority-needs-owner");
    assert!(
        o.stderr
            .contains("set coordinator_may_merge: false instead"),
        "{}",
        o.stderr
    );
    assert!(!fake.pr_now(pr1.number).0);
    // a dev agent is not the coordinator
    run_refused(&fake, &b, &["merge", &ts1], "verify-coordinator-only");
    // now the owner lets the coordinator merge
    fake.file(
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n",
    );
    // the head moved after the verify: refused, nothing merged, ticket stays in Review
    let moved = fake.push(&pr1.branch);
    run_refused(&fake, &c, &["merge", &ts1], "evidence-head-moved");
    let (merged, head) = fake.pr_now(pr1.number);
    assert!(!merged);
    assert_eq!(head, moved);
    assert_ne!(head, pr1.head);
    assert_eq!(field(&fake, &item1, "Status").as_deref(), Some("Review"));
    assert!(fake.branch_exists(&pr1.branch));

    // ---- ticket 2: everything holds, so it merges at the verified SHA
    let (t2, item2, pr2) = in_verified_review(&fake, &c, &b, "IT merge passes");
    let ts2 = t2.to_string();
    let o = run_ok(&fake, &c, &["merge", &ts2]);
    assert!(o.stdout.contains("merged at head"), "{}", o.stdout);
    assert!(o.stdout.contains(&pr2.head), "{}", o.stdout);
    let (merged, _) = fake.pr_now(pr2.number);
    assert!(merged, "{}", o.stdout);
    // the merge call carried the verified SHA
    let put = fake
        .requests()
        .into_iter()
        .find(|r| r.method == "PUT" && r.path.ends_with(&format!("/pulls/{}/merge", pr2.number)))
        .expect("merge call");
    let sent: Value = serde_json::from_str(&put.body).unwrap();
    assert_eq!(sent["sha"], pr2.head.as_str());
    // the dev branch is deleted; the branch of the refused ticket is not
    assert!(!fake.branch_exists(&pr2.branch));
    assert!(fake.branch_exists(&pr1.branch));
    // the ticket is Done and the signed notice is on it
    assert_eq!(field(&fake, &item2, "Status").as_deref(), Some("Done"));
    assert!(
        fake.comments_of(t2)
            .iter()
            .any(|(_, _, body)| body.contains("Merged by the coordinator"))
    );
    // a second merge is refused (nothing left to merge)
    let o = fake.run(&c.dir, &["merge", &ts2]);
    assert_eq!(o.code, 2, "{}", o.all());
    no_unhandled(&fake);
}

#[test]
fn after_a_merge_whose_done_write_failed_close_verified_finishes_and_a_bare_close_does_not() {
    let (fake, c, b) = crew();
    fake.file(
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n",
    );
    let (t, item, pr) = in_verified_review(&fake, &c, &b, "IT merge then failed write");
    let ts = t.to_string();
    fake.fail_select_write(Some("Done"));
    let o = run_refused(&fake, &c, &["merge", &ts], "merge-done-ticket-open");
    assert!(
        o.stderr
            .contains(&format!("Finish with `crewboard close {ts} --verified`")),
        "{}",
        o.stderr
    );
    assert!(fake.pr_now(pr.number).0, "the PR is merged");
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    fake.fail_select_write(None);
    // the issue is closed by the merge, but the merge actor is not the owner: a bare close finds
    // no owner merge
    fake.close_issue(t);
    run_refused(&fake, &c, &["close", &ts], "close-needs-reason");
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    run_ok(&fake, &c, &["close", &ts, "--verified"]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Done"));
    no_unhandled(&fake);
}

/// The single-select writes so far (Status, State): `updateProjectV2ItemFieldValue` with an
/// option id.
fn select_writes(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| {
            r.path == "/graphql"
                && r.body.contains("updateProjectV2ItemFieldValue")
                && r.body.contains("singleSelectOptionId")
        })
        .count()
}

/// After a merge refusal (DIRTY) the assignee updates the branch and delivers the
/// new head from Review. The Status stays Review; verify judges the new done; merge merges at
/// the new head. The same head, a new head without CI and another dev agent are refused.
#[test]
fn after_a_merge_refusal_the_assignee_re_delivers_from_review_and_the_new_head_merges() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", MERGE_AUTHORITY);
    // a ticket with CI: the re-delivery judges the CI of the new head like any done
    let t = fake.ticket_issue(
        "IT re-delivery",
        &format!("Goal: integration test ticket.\n\n{GATE}"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9550, &pr.head, "push", "success");
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert!(
        run_ok(&fake, &c, &["verify", &ts])
            .stdout
            .contains("verify PASS")
    );
    let _ = fake.poll_json(&b);

    // refused: a done with the same head, with --sha (before any read of the PR) and without it
    let before = writes(&fake);
    let pr_reads = || {
        fake.requests()
            .iter()
            .filter(|r| r.path.contains("/pulls/"))
            .count()
    };
    let reads = pr_reads();
    let o = run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        "status-no-change",
    );
    // the refusal says that the first `done` landed
    assert!(
        o.stderr
            .contains("ticket is already Review: your earlier `done` landed; wait for the coordinator's verify"),
        "{}",
        o.stderr
    );
    assert_eq!(
        pr_reads(),
        reads,
        "the same --sha is refused before the PR is read"
    );
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url],
        "status-no-change",
    );
    assert_eq!(writes(&fake), before, "a refused done writes nothing");

    // merge is refused (DIRTY): the request to the dev names the re-delivery command
    fake.set_mergeable_state(pr.number, "dirty");
    let command = format!("crewboard done {ts} --pr {} --sha <new head>", pr.url);
    let o = run_refused(&fake, &c, &["merge", &ts], "pr-not-mergeable");
    assert!(o.stderr.contains(&command), "{}", o.all());
    assert!(
        fake.comments_of(t)
            .iter()
            .any(|(_, _, body)| body.contains("conflicts with main") && body.contains(&command)),
        "{:?}",
        fake.comments_of(t)
    );
    assert!(!fake.pr_now(pr.number).0);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));

    // the dev merges the base branch in and pushes: a new head
    let new = fake.push(&pr.branch);
    fake.set_mergeable_state(pr.number, "clean");
    let _ = fake.poll_json(&b);
    // refused: the new head has no CI run yet (the checks of done hold), nothing is written
    let before = writes(&fake);
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &new],
        "evidence-ci-missing",
    );
    // refused: another dev agent, as before
    let d = fake.agent("codex@fake-d", Role::Dev);
    run_refused(
        &fake,
        &d,
        &["done", &ts, "--pr", &pr.url, "--sha", &new],
        "status-no-change",
    );
    assert_eq!(
        writes(&fake),
        before,
        "a refused re-delivery writes nothing"
    );

    // pass: CI of the new head is green; the assignee delivers it from Review
    fake.workflow_run(9551, &new, "push", "success");
    let selects = select_writes(&fake);
    let o = run_ok(&fake, &b, &["done", &ts, "--pr", &pr.url, "--sha", &new]);
    assert!(
        o.stdout
            .contains("done: Status Review (re-delivery: the Status stays)"),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains(&format!("sha: {new}")), "{}", o.all());
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    assert_eq!(select_writes(&fake), selects, "no Status or State write");
    assert!(
        field(&fake, &item, "Evidence").is_some_and(|e| e.contains(&new)),
        "{:?}",
        field(&fake, &item, "Evidence")
    );
    // the coordinator gets the signed done as an event
    let (v, _) = fake.poll_json(&c);
    assert!(
        v["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["class"] == "message"
                && e["from"] == b.name.as_str()
                && e["body"].as_str().unwrap().contains(&new)),
        "{v}"
    );

    // verify judges the new done (the old one would fail: its head moved) and merge merges
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let o = run_ok(&fake, &c, &["merge", &ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
    assert!(o.stdout.contains(&new), "{}", o.all());
    assert_eq!(fake.pr_now(pr.number), (true, new));
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Done"));
    no_unhandled(&fake);
}

/// A ticket for workflow files goes the owner route (both directions).
#[test]
fn a_workflow_ticket_goes_the_owner_route_and_a_plain_ticket_does_not() {
    let (fake, c, b) = crew();
    let body = format!("Goal: integration test ticket.\n\nRoute: owner-workflow\n{GATE}CI: none\n");
    let t = fake.ticket_issue("IT owner route", &body, &["crew:work"]);
    let ts = t.to_string();
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    assert!(o.stdout.contains("route: owner-workflow"), "{}", o.stdout);
    let item = item_in(&o);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    // refusal: no patch in the PR
    run_refused(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        "workflow-needs-owner",
    );
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
    // pass: the patch is in the PR
    fake.pr_file(pr.number, &format!("docs/patches/{t}-workflow.patch"));
    // On the owner route the PR says `Refs`, not `Closes`
    fake.set_pr_text(pr.number, "IT owner route", &format!("IT.\n\nRefs #{t}"));
    let head = fake.push(&pr.branch);
    let o = run_ok(&fake, &b, &["done", &ts, "--pr", &pr.url, "--sha", &head]);
    assert!(o.stdout.contains("Status Review"), "{}", o.stdout);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    assert!(fake.labels_of(t).contains(&"needs-owner".to_string()));
    let n_owner = fake
        .comments_of(t)
        .iter()
        .filter(|(_, _, b)| b.contains("owner-route: ") && b.contains("git apply"))
        .count();
    assert_eq!(n_owner, 1, "exactly one owner message");

    // control: a ticket without the mark and without the patch takes the normal route
    let t2 = ticket(&fake, "IT plain route", true, true);
    let ts2 = t2.to_string();
    // B has no ticket In Progress that blocks: the first one waits for the owner
    let o = run_ok(&fake, &c, &["assign", &ts2, "--to", &b.name]);
    assert!(!o.stdout.contains("route: owner-workflow"), "{}", o.stdout);
    let item2 = item_in(&o);
    run_ok(&fake, &b, &["take", &ts2]);
    let pr2 = fake.pr_for(t2, &b.name);
    // the agent reads its events before `done`
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts2, "--pr", &pr2.url, "--sha", &pr2.head],
    );
    assert_eq!(field(&fake, &item2, "Status").as_deref(), Some("Review"));
    no_unhandled(&fake);
}

/// The re-arm line of `resume` of an agent dir is `crewboard --config-dir <agent-dir> monitor`.
fn o_has_agent_dir_line(stdout: &str) -> bool {
    stdout.contains("crewboard --config-dir ") && stdout.contains(" monitor ")
}

#[test]
fn resume_of_the_coordinator_lists_a_done_without_verify_until_it_is_verified() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT resume", true, true);
    let ts = t.to_string();
    let key = format!("{REPO}#{t}");
    delivered(&fake, &c, &b, t);

    // resume: the checks of start, then the block with the done that waits for a verify
    let o = run_ok(&fake, &c, &["resume"]);
    assert!(
        o.stdout.contains("selftest"),
        "start checks ran: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("to-do after the restart (coordinator)"),
        "{}",
        o.stdout
    );
    let unverified = |o: &Out| -> String {
        o.stdout
            .split("tickets in Review")
            .next()
            .unwrap()
            .split("done messages without a verify")
            .nth(1)
            .unwrap_or_else(|| panic!("no done block: {}", o.stdout))
            .to_string()
    };
    assert!(
        unverified(&o).contains(&key),
        "unverified done missing: {}",
        o.stdout
    );
    assert!(
        o.stdout.contains("events since the cursor:"),
        "{}",
        o.stdout
    );
    // no [loop] table: the defaults
    assert!(
        o.stdout.contains("crewboard monitor ") && o.stdout.contains("--timeout 30m"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("-- --interval"), "{}", o.stdout);
    // the [loop] table changes the re-arm command
    let cfg_path = crewboard::config::ConfigDir(c.dir.clone()).agent_config_path("fake");
    let original = std::fs::read_to_string(&cfg_path).unwrap();
    std::fs::write(
        &cfg_path,
        format!("{original}\n[loop]\ninterval = \"20s\"\ntimeout = \"7m\"\nlog = \"/tmp/it-resume.log\"\n"),
    )
    .unwrap();
    let o = run_ok(&fake, &c, &["resume"]);
    assert!(o.stdout.contains("--timeout 7m"), "{}", o.stdout);
    assert!(
        o.stdout.contains("--log /tmp/it-resume.log"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("-- --interval 20s"), "{}", o.stdout);
    // an invalid table is a config error (exit 4), not a default
    std::fs::write(
        &cfg_path,
        format!("{original}\n[loop]\ninterval = \"soon\"\n"),
    )
    .unwrap();
    let o = fake.run(&c.dir, &["resume"]);
    assert_eq!(o.code, 4, "{}", o.all());
    std::fs::write(&cfg_path, &original).unwrap();
    // the default form is plain `crewboard monitor` (the control of the next case)
    assert!(!o_has_agent_dir_line(
        &run_ok(&fake, &c, &["resume"]).stdout
    ));
    // an agent dir (agent.env is a link): the line names the config dir, and the binary loads
    // agent.env itself
    let target = c.dir.join("crew-token.env");
    std::fs::write(&target, "GH_TOKEN=unused\n").unwrap();
    // crewboard loads agent.env now, so the file must have a private mode
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let link = c.dir.join("agent.env");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).unwrap();
    #[cfg(not(unix))]
    std::fs::copy(&target, &link).unwrap();
    let o = run_ok(&fake, &c, &["resume"]);
    assert!(o_has_agent_dir_line(&o.stdout), "{}", o.stdout);
    assert!(
        o.stdout
            .contains(&format!("--config-dir {} monitor", c.dir.display())),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("unused"), "{}", o.stdout);
    std::fs::remove_file(&link).unwrap();

    // after the verify the done is not listed
    run_ok(&fake, &c, &["verify", &ts]);
    let o = run_ok(&fake, &c, &["resume"]);
    assert!(
        !unverified(&o).contains(&key),
        "a verified done must not be listed: {}",
        o.stdout
    );
    run_ok(&fake, &c, &["close", &ts, "--verified"]);
    no_unhandled(&fake);
}

// ---- one In Progress ticket per agent, judged by the board ----

/// Assign `n` tickets to B (priorities as given). Returns `(ticket number, item id)` each. The
/// queue follows the priority, as a coordinator who sorted it would write it: the `Order` is the
/// rank of the priority times ten plus the position in the list (`P0` first; equal priorities by
/// position), so every ticket has an `Order` and the interrupt rule is the rule of a migrated
/// queue.
fn assigned(fake: &FakeBoard, c: &Agent, b: &Agent, prios: &[&str]) -> Vec<(String, String)> {
    prios
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = ticket(fake, &format!("IT one per agent {i}"), true, true).to_string();
            let rank = p.trim_start_matches('P').parse::<usize>().unwrap_or(4);
            let order = (rank * 10 + i).to_string();
            let item = item_in(&run_ok(
                fake,
                c,
                &["assign", &t, "--to", &b.name, "--order", &order],
            ));
            (t, item)
        })
        .collect()
}

/// B has a stale State: `active`, although tickets are In Progress.
fn stale_active(fake: &FakeBoard, b: &Agent) {
    fake.set_item(&b.item_id, "State", "active");
}

#[test]
fn a_take_of_ready_is_refused_while_another_ticket_is_in_progress_whatever_the_state() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    run_ok(&fake, &b, &["take", &ts[0].0]);
    stale_active(&fake, &b);
    // refusal: State `active` does not skip the check
    let o = run_refused(&fake, &b, &["take", &ts[1].0], "take-agent-busy");
    assert!(
        o.stderr.contains(&format!("{REPO}#{}", ts[0].0)),
        "the busy ticket is named: {}",
        o.all()
    );
    assert_eq!(
        field(&fake, &ts[1].1, "Status").as_deref(),
        Some("Ready"),
        "nothing was written"
    );
    // pass: the first ticket leaves In Progress, then the take goes through
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", &ts[0].0, "--why", "wait"]);
    run_ok(&fake, &b, &["take", &ts[1].0]);
    assert_eq!(
        field(&fake, &ts[1].1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, &b.item_id, "State").as_deref(), Some("active"));
    no_unhandled(&fake);
}

#[test]
fn a_resume_of_blocked_is_refused_while_another_ticket_is_in_progress() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    let (a, a_item) = (&ts[0].0, &ts[0].1);
    run_ok(&fake, &b, &["take", a]);
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", a, "--why", "need a decision"]);
    fake.owner_comment_now(a.parse().unwrap(), "Go on.");
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts[1].0]);
    stale_active(&fake, &b);
    // refusal: the answer is there, but ticket 2 is In Progress and the State is `active`
    let o = run_refused(&fake, &b, &["take", a], "take-agent-busy");
    assert!(
        o.stderr.contains(&format!("{REPO}#{}", ts[1].0)),
        "{}",
        o.all()
    );
    assert_eq!(field(&fake, a_item, "Status").as_deref(), Some("Blocked"));
    // pass: ticket 2 is blocked, the resume goes through
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", &ts[1].0, "--why", "wait"]);
    run_ok(&fake, &b, &["take", a]);
    assert_eq!(
        field(&fake, a_item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

/// B holds two In Progress tickets (an old state) and a P0 ticket waits. Returns the tickets.
fn two_in_progress(fake: &FakeBoard, c: &Agent, b: &Agent) -> Vec<(String, String)> {
    let ts = assigned(fake, c, b, &["P2", "P2", "P0"]);
    run_ok(fake, b, &["take", &ts[0].0]);
    // the second In Progress ticket, as old boards have it
    fake.set_item(&ts[1].1, "Status", "In Progress");
    let _ = fake.poll_json(b);
    ts
}

#[test]
fn interrupt_with_two_in_progress_lists_them_and_park_chooses_one() {
    let (fake, c, b) = crew();
    let ts = two_in_progress(&fake, &c, &b);
    let key = |i: usize| format!("{REPO}#{}", ts[i].0);
    // refusal: no --park; the message lists both candidates and names the flag
    let o = run_refused(
        &fake,
        &b,
        &["take", &ts[2].0, "--interrupt", "--why", "P0"],
        "interrupt-park-ambiguous",
    );
    assert!(
        o.stderr.contains(&key(0)) && o.stderr.contains(&key(1)) && o.stderr.contains("--park"),
        "{}",
        o.all()
    );
    assert_eq!(field(&fake, &ts[2].1, "Status").as_deref(), Some("Ready"));
    // pass: --park names the second one; it is parked and the P0 ticket is taken
    run_ok(
        &fake,
        &b,
        &[
            "take",
            &ts[2].0,
            "--interrupt",
            "--park",
            &ts[1].0,
            "--why",
            "P0",
        ],
    );
    assert_eq!(field(&fake, &ts[1].1, "Status").as_deref(), Some("Blocked"));
    assert_eq!(
        field(&fake, &ts[0].1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(
        field(&fake, &ts[2].1, "Status").as_deref(),
        Some("In Progress")
    );
    let parked = fake
        .comments_of(ts[1].0.parse().unwrap())
        .into_iter()
        .any(|(_, _, body)| body.contains(&format!("parked for #{}", ts[2].0)));
    assert!(parked, "the park message is on the parked ticket");
    no_unhandled(&fake);
}

#[test]
fn park_must_name_an_in_progress_ticket_of_the_agent() {
    let (fake, c, b) = crew();
    let ts = two_in_progress(&fake, &c, &b);
    // a Ready ticket of the agent (not In Progress)
    let extra = assigned(&fake, &c, &b, &["P3"]);
    let o = run_refused(
        &fake,
        &b,
        &[
            "take",
            &ts[2].0,
            "--interrupt",
            "--park",
            &extra[0].0,
            "--why",
            "P0",
        ],
        "interrupt-park-not-in-progress",
    );
    assert!(
        o.stderr.contains(&format!("{REPO}#{}", ts[0].0)),
        "the message lists what can be parked: {}",
        o.all()
    );
    for t in &ts[..2] {
        assert_eq!(field(&fake, &t.1, "Status").as_deref(), Some("In Progress"));
    }
    assert_eq!(field(&fake, &ts[2].1, "Status").as_deref(), Some("Ready"));
    no_unhandled(&fake);
}

#[test]
fn interrupt_with_nothing_in_progress_keeps_its_own_refusal() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P0"]);
    let o = run_refused(
        &fake,
        &b,
        &["take", &ts[0].0, "--interrupt", "--why", "P0"],
        "interrupt-nothing-to-park",
    );
    assert!(
        o.stderr.contains("you have none") && !o.stderr.contains("--park"),
        "{}",
        o.all()
    );
    // the pass direction: one In Progress ticket needs no --park, and a stale `active` State
    // does not matter (the board decides)
    let more = assigned(&fake, &c, &b, &["P2"]);
    run_ok(&fake, &b, &["take", &more[0].0]);
    stale_active(&fake, &b);
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts[0].0, "--interrupt", "--why", "P0"]);
    assert_eq!(
        field(&fake, &more[0].1, "Status").as_deref(),
        Some("Blocked")
    );
    assert_eq!(
        field(&fake, &ts[0].1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, &b.item_id, "State").as_deref(), Some("active"));
    no_unhandled(&fake);
}

/// The board of an older version: the tickets have a Priority and no `Order`.
fn without_order(fake: &FakeBoard, items: &[&str]) {
    for i in items {
        fake.clear_item(i, "Order");
    }
}

#[test]
fn interrupt_needs_a_strictly_lower_order() {
    let (fake, c, b) = crew();
    // default Orders 1, 2, 3 (the end of the queue each time)
    let mut ts = Vec::new();
    for i in 0..3 {
        let t = ticket(&fake, &format!("IT interrupt order {i}"), true, true).to_string();
        let item = item_in(&run_ok(&fake, &c, &["assign", &t, "--to", &b.name]));
        assert_eq!(
            field(&fake, &item, "Order").as_deref(),
            Some(&*(i + 1).to_string())
        );
        ts.push((t, item));
    }
    run_ok(&fake, &b, &["take", &ts[0].0]);
    let _ = fake.poll_json(&b);
    let w = writes(&fake);
    // refusal: Order 2 is behind Order 1; the message names the way
    let o = run_refused(
        &fake,
        &b,
        &["take", &ts[1].0, "--interrupt", "--why", "urgent"],
        "interrupt-not-ahead",
    );
    assert!(
        o.stderr.contains("new: 2, in progress: 1") && o.stderr.contains("assign <n> --order"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert_eq!(
        field(&fake, &ts[0].1, "Status").as_deref(),
        Some("In Progress")
    );
    // pass: the coordinator puts the ticket first; now it interrupts
    run_ok(&fake, &c, &["assign", &ts[2].0, "--order", "0"]);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["take", &ts[2].0, "--interrupt", "--why", "urgent"],
    );
    assert_eq!(field(&fake, &ts[0].1, "Status").as_deref(), Some("Blocked"));
    assert_eq!(
        field(&fake, &ts[2].1, "Status").as_deref(),
        Some("In Progress")
    );
    // an equal Order is not ahead
    run_ok(&fake, &c, &["assign", &ts[1].0, "--order", "0"]);
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["take", &ts[1].0, "--interrupt", "--why", "urgent"],
        "interrupt-not-ahead",
    );
    no_unhandled(&fake);
}

/// B blocked ticket 0 (Order 20), the coordinator or owner has answered it, and B works on
/// ticket 1 (Order 21) now. Returns the tickets; `answer` says whether the owner answers ticket 0.
fn blocked_then_busy(
    fake: &FakeBoard,
    c: &Agent,
    b: &Agent,
    answer: bool,
) -> Vec<(String, String)> {
    let ts = assigned(fake, c, b, &["P2", "P2"]);
    run_ok(fake, b, &["take", &ts[0].0]);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["block", &ts[0].0, "--why", "need a decision"]);
    if answer {
        fake.owner_comment_now(ts[0].0.parse().unwrap(), "Go on.");
    }
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["take", &ts[1].0]);
    ts
}

#[test]
fn interrupt_resumes_an_answered_blocked_ticket_that_is_ahead_and_parks_the_running_one() {
    let (fake, c, b) = crew();
    let ts = blocked_then_busy(&fake, &c, &b, true);
    let (a, a_item, run, run_item) = (&ts[0].0, &ts[0].1, &ts[1].0, &ts[1].1);
    // a plain take is refused: the agent is busy
    run_refused(&fake, &b, &["take", a], "take-agent-busy");
    let before = fake.requests().len();
    let o = run_ok(
        &fake,
        &b,
        &["take", a, "--interrupt", "--why", "A is answered"],
    );
    assert!(o.stdout.contains("resumed"), "{}", o.all());
    assert_eq!(
        field(&fake, a_item, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, run_item, "Status").as_deref(), Some("Blocked"));
    // the order of the writes: the park message of the running ticket comes before the ack of A
    let new: Vec<String> = fake.requests()[before..]
        .iter()
        .filter(|r| r.method == "POST")
        .map(|r| r.path.clone())
        .collect();
    let at = |n: &str| {
        new.iter()
            .position(|x| x.ends_with(&format!("/issues/{n}/comments")))
    };
    let (park, ack) = (at(run), at(a));
    assert!(
        park.is_some() && ack.is_some() && park < ack,
        "park before ack: {new:?}"
    );
    assert!(
        fake.comments_of(run.parse().unwrap())
            .iter()
            .any(|x| x.2.contains(&format!("parked for #{a}: A is answered"))),
        "the park message is written"
    );
    assert_eq!(
        field(&fake, &b.item_id, "Doing").as_deref(),
        Some(&*format!("{REPO}#{a}"))
    );
    no_unhandled(&fake);
}

#[test]
fn interrupt_of_a_blocked_ticket_without_its_answer_is_refused_and_writes_nothing() {
    let (fake, c, b) = crew();
    let ts = blocked_then_busy(&fake, &c, &b, false);
    let w = writes(&fake);
    run_refused(
        &fake,
        &b,
        &["take", &ts[0].0, "--interrupt", "--why", "urgent"],
        "status-answer-missing",
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert_eq!(
        field(&fake, &ts[1].1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, &ts[0].1, "Status").as_deref(), Some("Blocked"));
}

#[test]
fn interrupt_of_an_answered_blocked_ticket_that_is_not_ahead_is_refused() {
    let (fake, c, b) = crew();
    let ts = blocked_then_busy(&fake, &c, &b, true);
    // the queue changes: A is behind the running ticket now
    fake.set_item(&ts[0].1, "Order", "30");
    let _ = fake.poll_json(&b);
    let w = writes(&fake);
    run_refused(
        &fake,
        &b,
        &["take", &ts[0].0, "--interrupt", "--why", "urgent"],
        "interrupt-not-ahead",
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert_eq!(
        field(&fake, &ts[1].1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, &ts[0].1, "Status").as_deref(), Some("Blocked"));
}

#[test]
fn interrupt_of_an_owner_question_needs_the_owner_answer_like_a_plain_take() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    run_ok(&fake, &b, &["take", &ts[0].0]);
    run_ok(&fake, &b, &["ask", &ts[0].0, "--body", "Which layout?"]);
    let _ = fake.poll_json(&c);
    run_ok(
        &fake,
        &c,
        &[
            "say",
            &ts[0].0,
            "--to",
            &b.name,
            "--kind",
            "answer",
            "--body",
            "the first",
        ],
    );
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts[1].0]);
    let w = writes(&fake);
    let o = run_refused(
        &fake,
        &b,
        &["take", &ts[0].0, "--interrupt", "--why", "urgent"],
        "status-answer-missing",
    );
    assert!(o.stderr.contains("question to the owner"), "{}", o.all());
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
    assert_eq!(
        field(&fake, &ts[1].1, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn interrupt_of_a_review_ticket_is_refused_with_the_new_text() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    run_ok(&fake, &b, &["take", &ts[1].0]);
    fake.set_item(&ts[0].1, "Status", "Review");
    let _ = fake.poll_json(&b);
    let w = writes(&fake);
    let o = run_refused(
        &fake,
        &b,
        &["take", &ts[0].0, "--interrupt", "--why", "urgent"],
        "interrupt-needs-ready-ticket",
    );
    assert!(
        o.stderr
            .contains("needs a Ready ticket or an answered Blocked ticket of yours"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), w, "a refusal writes nothing");
}

#[test]
fn a_ticket_without_order_never_interrupts() {
    let (fake, c, b) = crew();
    let mut ts = Vec::new();
    for i in 0..3 {
        let t = ticket(&fake, &format!("IT interrupt mixed {i}"), true, true).to_string();
        let item = item_in(&run_ok(&fake, &c, &["assign", &t, "--to", &b.name]));
        ts.push((t, item));
    }
    run_ok(&fake, &b, &["take", &ts[0].0]);
    // a ticket that has lost its Order (an old board) is never ahead of a ticket that has one
    without_order(&fake, &[&ts[2].1]);
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["take", &ts[2].0, "--interrupt", "--why", "urgent"],
        "interrupt-not-ahead",
    );
    no_unhandled(&fake);
}

#[test]
fn status_warns_when_an_agent_has_more_than_one_ticket_in_progress() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    run_ok(&fake, &b, &["take", &ts[0].0]);
    // direction 1: one In Progress ticket, no warning
    let o = run_ok(&fake, &c, &["status"]);
    assert!(!o.stdout.contains("tickets In Progress"), "{}", o.stdout);
    let v: Value = serde_json::from_str(&run_ok(&fake, &c, &["status", "--json"]).stdout).unwrap();
    assert_eq!(v["multiple_in_progress"], serde_json::json!([]));
    // direction 2: two
    fake.set_item(&ts[1].1, "Status", "In Progress");
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains(&format!(
            "! {} has 2 tickets In Progress: {REPO}#{}, {REPO}#{}",
            b.name, ts[0].0, ts[1].0
        )),
        "{}",
        o.stdout
    );
    let v: Value = serde_json::from_str(&run_ok(&fake, &c, &["status", "--json"]).stdout).unwrap();
    assert_eq!(v["multiple_in_progress"][0]["agent"], b.name.as_str());
    assert_eq!(
        v["multiple_in_progress"][0]["tickets"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

// ---- `done` on the finished part of a split Blocked ticket ---------------------

/// B holds ticket A (CI: none) and stops it with `block`. Returns `(A, item of A)`.
fn stopped_ticket(fake: &FakeBoard, c: &Agent, b: &Agent) -> (u64, String) {
    let a = ticket(fake, "IT split finished part", true, true);
    let at = a.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &at, "--to", &b.name]));
    run_ok(fake, b, &["take", &at]);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["block", &at, "--why", "A is done, B is open"]);
    assert_eq!(field(fake, &item, "Status").as_deref(), Some("Blocked"));
    (a, item)
}

/// The coordinator splits A; B takes the new ticket. Returns `(new ticket, its item)`.
fn split_and_take(fake: &FakeBoard, c: &Agent, b: &Agent, a: u64) -> (u64, String) {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("open.md");
    std::fs::write(&f, "The open part.\n\n**Gate:** the open part works.\n").unwrap();
    let o = run_ok(
        fake,
        c,
        &[
            "ticket",
            "split",
            &a.to_string(),
            "--title",
            "IT open part",
            "--body-file",
            f.to_str().unwrap(),
        ],
    );
    // "split: <old> -> <repo>#<new>; ..."
    let new: u64 = o
        .stdout
        .split(&format!("-> {REPO}#"))
        .nth(1)
        .and_then(|r| r.split(';').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no new ticket in: {}", o.stdout));
    let item = fake.item_of_issue(new).expect("item of the new ticket");
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["take", &new.to_string()]);
    assert_eq!(field(fake, &item, "Status").as_deref(), Some("In Progress"));
    (new, item)
}

fn done_of_split() {
    let (fake, c, b) = crew();
    let (a, a_item) = stopped_ticket(&fake, &c, &b);
    let (new, new_item) = split_and_take(&fake, &c, &b, a);
    let pr = fake.pr_for(a, &b.name);
    let _ = fake.poll_json(&b);
    let o = run_ok(
        &fake,
        &b,
        &["done", &a.to_string(), "--pr", &pr.url, "--sha", &pr.head],
    );
    assert!(o.stdout.contains("done: Status Review"), "{}", o.all());
    // the finished part is in Review; the open part is still In Progress and still the work
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Review"));
    assert!(
        field(&fake, &a_item, "Evidence")
            .unwrap()
            .contains(&pr.head),
        "evidence is set"
    );
    assert_eq!(
        field(&fake, &new_item, "Status").as_deref(),
        Some("In Progress"),
        "ticket {new}"
    );
    assert_eq!(
        field(&fake, &b.item_id, "State").as_deref(),
        Some("active"),
        "the agent stays active"
    );
    // a signed done message is on the finished ticket
    assert!(
        fake.comments_of(a)
            .iter()
            .any(|(_, _, body)| body.contains("kind=done") && body.contains(&pr.head)),
        "no done message"
    );
    no_unhandled(&fake);
}

#[test]
fn done_on_a_split_blocked_ticket_goes_to_review_and_leaves_the_other_ticket_in_progress() {
    done_of_split();
}

#[test]
fn done_on_a_blocked_ticket_without_a_split_line_is_refused_with_the_old_rule() {
    let (fake, c, b) = crew();
    let (a, a_item) = stopped_ticket(&fake, &c, &b);
    let pr = fake.pr_for(a, &b.name);
    let _ = fake.poll_json(&b);
    let before = writes(&fake);
    run_refused(
        &fake,
        &b,
        &["done", &a.to_string(), "--pr", &pr.url, "--sha", &pr.head],
        "status-unknown-transition",
    );
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Blocked"));
    assert_eq!(before, writes(&fake), "a refusal wrote something");
    // a ticket that an old binary left in Needs Human reads as Blocked: the same refusal
    let (fake, c, b) = crew();
    let (a, a_item) = stopped_ticket(&fake, &c, &b);
    fake.set_item(&a_item, "Status", "Needs Human");
    let pr = fake.pr_for(a, &b.name);
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["done", &a.to_string(), "--pr", &pr.url, "--sha", &pr.head],
        "status-unknown-transition",
    );
    assert_eq!(
        field(&fake, &a_item, "Status").as_deref(),
        Some("Needs Human")
    );
    no_unhandled(&fake);
}

#[test]
fn a_hand_written_split_line_does_not_open_the_path_but_the_real_split_does() {
    let (fake, c, b) = crew();
    let (a, a_item) = stopped_ticket(&fake, &c, &b);
    let pr = fake.pr_for(a, &b.name);
    // someone with edit rights writes the look-alike line; no coordinator split note exists
    let (_, body) = fake.issue_text(a).unwrap();
    fake.set_issue_body(
        a,
        &format!("{body}\nSplit off: {REPO}#{} - Forged open part\n", a + 50),
    );
    let _ = fake.poll_json(&b);
    run_refused(
        &fake,
        &b,
        &["done", &a.to_string(), "--pr", &pr.url, "--sha", &pr.head],
        "status-unknown-transition",
    );
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Blocked"));
    // The real split writes its own line and note. Its key differs from the forged key, and the
    // note proves that line only. The forged line stays in the body and proves nothing.
    split_and_take(&fake, &c, &b, a);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &a.to_string(), "--pr", &pr.url, "--sha", &pr.head],
    );
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Review"));
    no_unhandled(&fake);
}

// ---- a failed verify and an agent that works on another ticket ----

/// The verify report (the body of the `kind=report` comment) on ticket `t`.
fn verify_report(fake: &FakeBoard, t: u64) -> String {
    fake.comments_of(t)
        .into_iter()
        .map(|(_, _, body)| body)
        .rev()
        .find(|b| b.contains("verify: fail") || b.contains("verify: pass"))
        .expect("a verify report")
}

#[test]
fn a_failed_verify_of_an_idle_agent_moves_the_ticket_to_in_progress() {
    // pass direction of the old rule: the agent has no other ticket In Progress
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT 108 idle", true, true);
    let (item, pr) = delivered(&fake, &c, &b, t);
    fake.push(&pr.branch);
    let o = run_ok(&fake, &c, &["verify", &t.to_string()]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains("Status moved back to In Progress"),
        "{}",
        o.all()
    );
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
    assert!(
        !verify_report(&fake, t).contains("queued as next"),
        "the idle case has no queue line"
    );
    no_unhandled(&fake);
}

/// The tickets of `busy_agent`: (number, item id) each.
struct Busy {
    a: (u64, String, crate::support::board::Pull),
    other: (u64, String),
    third: (u64, String),
}

/// A in Review (done), B In Progress for the same agent, C Ready with Order 3 for it.
fn busy_agent(fake: &FakeBoard, c: &Agent, b: &Agent) -> Busy {
    let a = ticket(fake, "IT 108 A", true, true);
    let (a_item, a_pr) = delivered(fake, c, b, a);
    let other = ticket(fake, "IT 108 other", true, true);
    let other_item = item_in(&run_ok(
        fake,
        c,
        &["assign", &other.to_string(), "--to", &b.name],
    ));
    run_ok(fake, b, &["take", &other.to_string()]);
    let third = ticket(fake, "IT 108 third", true, true);
    let third_item = item_in(&run_ok(
        fake,
        c,
        &[
            "assign",
            &third.to_string(),
            "--to",
            &b.name,
            "--order",
            "3",
        ],
    ));
    Busy {
        a: (a, a_item, a_pr),
        other: (other, other_item),
        third: (third, third_item),
    }
}

#[test]
fn a_failed_verify_of_a_busy_agent_queues_the_ticket_first_and_leaves_the_other_alone() {
    let (fake, c, b) = crew();
    let Busy {
        a: (a, a_item, a_pr),
        other: (other, other_item),
        third: (third_n, third_item),
    } = busy_agent(&fake, &c, &b);
    let b_state = field(&fake, &b.item_id, "State");
    fake.push(&a_pr.branch);
    let o = run_ok(&fake, &c, &["verify", &a.to_string()]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    // Ready, same agent, first in the queue (Order 3 of the third ticket minus 1)
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        field(&fake, &a_item, "Agent").as_deref(),
        Some(b.name.as_str())
    );
    assert_eq!(field(&fake, &a_item, "Order").as_deref(), Some("2"));
    // the other ticket and the agent are untouched
    assert_eq!(
        field(&fake, &other_item, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(field(&fake, &third_item, "Order").as_deref(), Some("3"));
    assert_eq!(field(&fake, &b.item_id, "State"), b_state);
    // the signed report says which case applied
    let report = verify_report(&fake, a);
    assert!(
        report.contains(&format!(
            "verify failed; {} works on {REPO}#{other}; queued as next",
            b.name
        )),
        "{report}"
    );
    // the queue of the agent: the re-queued ticket comes first
    let (v, _) = fake.poll_json(&b);
    let _ = v;
    let m = run_ok(&fake, &b, &["mine"]);
    let first = m.stdout.find(&format!("#{a}")).expect("A in mine");
    let third = m.stdout.find(&format!("#{third_n}")).expect("C in mine");
    assert!(first < third, "A first: {}", m.stdout);
    no_unhandled(&fake);
}

#[test]
fn the_requeued_ticket_is_taken_after_done_on_the_other_ticket() {
    let (fake, c, b) = crew();
    let Busy {
        a: (a, a_item, a_pr),
        other: (other, other_item),
        ..
    } = busy_agent(&fake, &c, &b);
    fake.push(&a_pr.branch);
    run_ok(&fake, &c, &["verify", &a.to_string()]);
    let _ = fake.poll_json(&b);
    // refusal: the agent still works on the other ticket
    run_refused(&fake, &b, &["take", &a.to_string()], "take-agent-busy");
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Ready"));
    // pass: after `done` on the other ticket the take works as usual
    let pr = fake.pr_for(other, &b.name);
    run_ok(
        &fake,
        &b,
        &[
            "done",
            &other.to_string(),
            "--pr",
            &pr.url,
            "--sha",
            &pr.head,
        ],
    );
    assert_eq!(
        field(&fake, &other_item, "Status").as_deref(),
        Some("Review")
    );
    run_ok(&fake, &b, &["take", &a.to_string()]);
    assert_eq!(
        field(&fake, &a_item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn a_passed_verify_of_a_busy_agent_changes_nothing() {
    let (fake, c, b) = crew();
    let Busy {
        a: (a, a_item, _),
        other: (_, other_item),
        third: (_, third_item),
    } = busy_agent(&fake, &c, &b);
    let o = run_ok(&fake, &c, &["verify", &a.to_string()]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    assert_eq!(field(&fake, &a_item, "Status").as_deref(), Some("Review"));
    // the Order that `assign` gave (the first ticket of the agent) is still there
    assert_eq!(field(&fake, &a_item, "Order").as_deref(), Some("1"));
    assert_eq!(
        field(&fake, &other_item, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(
        field(&fake, &third_item, "Status").as_deref(),
        Some("Ready")
    );
    assert!(!verify_report(&fake, a).contains("queued as next"));
    no_unhandled(&fake);
}

/// Assign `t` to B, B takes it and blocks it. The coordinator has read B's block.
/// Returns the item id.
fn blocked_by_b(fake: &FakeBoard, c: &Agent, b: &Agent, t: u64) -> String {
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    run_ok(fake, b, &["block", &ts, "--why", "waits for #999"]);
    assert_eq!(field(fake, &item, "Status").as_deref(), Some("Blocked"));
    let _ = fake.poll_json(c);
    item
}

fn say_info(fake: &FakeBoard, c: &Agent, b: &Agent, ts: &str) -> Out {
    run_ok(
        fake,
        c,
        &[
            "say",
            ts,
            "--to",
            &b.name,
            "--kind",
            "info",
            "--body",
            "#999 is merged, take this ticket",
        ],
    )
}

#[test]
fn unblock_posts_one_answer_and_take_then_works() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT unblock", true, true);
    let ts = t.to_string();
    let item = blocked_by_b(&fake, &c, &b, t);
    let before = fake.comments_of(t).len();
    let o = run_ok(&fake, &c, &["unblock", &ts]);
    assert!(o.stdout.contains("unblocked"), "{}", o.all());
    let after = fake.comments_of(t);
    assert_eq!(after.len(), before + 1, "exactly one comment is posted");
    let last = &after.last().unwrap().2;
    assert!(
        last.contains("kind=answer") && last.contains(&format!("to={}", b.name)),
        "{last}"
    );
    assert!(
        last.contains(&format!("resume allowed: run take {t}")),
        "{last}"
    );
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn unblock_takes_a_body_and_a_dev_may_not_unblock() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT unblock body", true, true);
    let ts = t.to_string();
    blocked_by_b(&fake, &c, &b, t);
    // a dev is refused with the named rule and nothing is written
    let w = writes(&fake);
    run_refused(&fake, &b, &["unblock", &ts], "unblock-coordinator-only");
    assert_eq!(writes(&fake), w);
    run_ok(
        &fake,
        &c,
        &["unblock", &ts, "--body", "Go on with the second approach."],
    );
    assert!(
        fake.comments_of(t)
            .last()
            .unwrap()
            .2
            .contains("Go on with the second approach."),
    );
}

#[test]
fn unblock_on_a_ticket_that_is_not_blocked_is_refused_and_writes_nothing() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT unblock refused", true, true);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let _ = fake.poll_json(&c);
    let w = writes(&fake);
    let n = fake.comments_of(t).len();
    let o = run_refused(&fake, &c, &["unblock", &ts], "unblock-needs-blocked");
    assert!(o.stderr.contains("In Progress"), "{}", o.all());
    assert_eq!(writes(&fake), w, "nothing is written");
    assert_eq!(fake.comments_of(t).len(), n);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn say_info_to_the_assignee_of_a_blocked_ticket_warns_and_posts() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT info warns", true, true);
    let ts = t.to_string();
    blocked_by_b(&fake, &c, &b, t);
    let before = fake.comments_of(t).len();
    let o = say_info(&fake, &c, &b, &ts);
    assert!(
        o.stderr.contains("info does not unblock")
            && o.stderr.contains(&format!("crewboard unblock {t}"))
            && o.stderr.contains("--kind answer"),
        "{}",
        o.all()
    );
    assert_eq!(fake.comments_of(t).len(), before + 1, "the info is posted");
}

#[test]
fn say_info_on_an_in_progress_ticket_or_an_answer_on_a_blocked_one_prints_no_warning() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT info quiet", true, true);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let _ = fake.poll_json(&c);
    let o = say_info(&fake, &c, &b, &ts);
    assert!(!o.stderr.contains("does not unblock"), "{}", o.all());
    // control: an answer on a Blocked ticket does not warn either
    let t2 = ticket(&fake, "IT answer quiet", true, true);
    let t2s = t2.to_string();
    // B is busy with the first ticket: block it first
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", &ts, "--why", "wait"]);
    run_ok(&fake, &c, &["assign", &t2s, "--to", &b.name]);
    let _ = fake.poll_json(&c);
    let o = run_ok(
        &fake,
        &c,
        &[
            "say", &ts, "--to", &b.name, "--kind", "answer", "--body", "go on",
        ],
    );
    assert!(!o.stderr.contains("does not unblock"), "{}", o.all());
}

#[test]
fn take_after_only_an_info_names_the_case_and_after_the_unblock_it_works() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT take info", true, true);
    let ts = t.to_string();
    let item = blocked_by_b(&fake, &c, &b, t);
    say_info(&fake, &c, &b, &ts);
    let _ = fake.poll_json(&b);
    let o = run_refused(&fake, &b, &["take", &ts], "status-answer-missing");
    assert!(
        o.stderr
            .contains("coordinator info was posted after the block")
            && o.stderr
                .contains("only an answer (or an owner message) unblocks")
            && o.stderr.contains(&format!("crewboard unblock {t}")),
        "{}",
        o.all()
    );
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Blocked"));
    // control: without any info the old text stays
    let t2 = ticket(&fake, "IT take plain", true, true);
    let t2s = t2.to_string();
    // B has no ticket In Progress now: Blocked does not count
    blocked_by_b(&fake, &c, &b, t2);
    let o = run_refused(&fake, &b, &["take", &t2s], "status-answer-missing");
    assert!(
        !o.stderr.contains("coordinator info was posted"),
        "{}",
        o.all()
    );
    // the unblock opens the first ticket
    run_ok(&fake, &c, &["unblock", &ts]);
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &ts]);
    assert_eq!(
        field(&fake, &item, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn take_block_take_and_done_write_no_agent_state_and_doing_follows_the_board() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2", "P2"]);
    let doing = || field(&fake, &b.item_id, "Doing");
    let state = || field(&fake, &b.item_id, "State");
    let n = |i: usize| ts[i].0.parse::<u64>().unwrap();
    assert_eq!(state().as_deref(), Some("active"));
    // take: State stays active, Doing names the ticket
    run_ok(&fake, &b, &["take", &ts[0].0]);
    assert_eq!(state().as_deref(), Some("active"));
    assert_eq!(doing(), Some(format!("{REPO}#{}", ts[0].0)));
    // status derives "busy" from the board
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains(&format!("busy {REPO}#{}", ts[0].0)),
        "{}",
        o.stdout
    );
    let o = run_ok(&fake, &c, &["status", "--json"]);
    let v: Value = serde_json::from_str(&o.stdout).unwrap();
    let agent = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == b.name.as_str())
        .unwrap();
    assert_eq!(agent["in_progress"][0], format!("{REPO}#{}", ts[0].0));
    // block: nothing else In Progress, so Doing goes idle; State is untouched
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["block", &ts[0].0, "--why", "wait"]);
    assert_eq!(state().as_deref(), Some("active"));
    assert_eq!(doing().as_deref(), Some("idle"));
    let o = run_ok(&fake, &c, &["status"]);
    assert!(!o.stdout.contains("busy "), "{}", o.stdout);
    // take the next, then done: Doing follows, State stays
    run_ok(&fake, &b, &["take", &ts[1].0]);
    assert_eq!(doing(), Some(format!("{REPO}#{}", ts[1].0)));
    let pr = fake.pr_for(n(1), &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts[1].0, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert_eq!(state().as_deref(), Some("active"));
    assert_eq!(doing().as_deref(), Some("idle"));
    no_unhandled(&fake);
}

#[test]
fn a_board_with_the_legacy_state_working_assigns_and_takes_and_never_writes_it_again() {
    let (fake, c, b) = crew();
    fake.set_item(&b.item_id, "State", "working");
    let ts = assigned(&fake, &c, &b, &["P2"]);
    run_ok(&fake, &b, &["take", &ts[0].0]);
    assert_eq!(
        field(&fake, &b.item_id, "State").as_deref(),
        Some("working"),
        "the legacy value is read as active and left alone"
    );
    assert_eq!(
        field(&fake, &b.item_id, "Doing"),
        Some(format!("{REPO}#{}", ts[0].0))
    );
    // status reads it as busy
    let o = run_ok(&fake, &c, &["status"]);
    assert!(o.stdout.contains("busy "), "{}", o.stdout);
}

#[test]
fn a_take_by_an_agent_that_is_not_active_is_refused_with_the_command_that_fixes_it() {
    let (fake, c, b) = crew();
    let ts = assigned(&fake, &c, &b, &["P2"]);
    for (state, rule, fix) in [
        ("approved", "agent-not-active", "crewboard start"),
        ("draining", "agent-not-active", "crewboard join"),
        ("paused", "agent-paused", "crewboard resume"),
    ] {
        fake.set_item(&b.item_id, "State", state);
        let w = writes(&fake);
        let o = run_refused(&fake, &b, &["take", &ts[0].0], rule);
        assert!(o.stderr.contains(fix), "{state}: {}", o.stderr);
        assert_eq!(writes(&fake), w, "{state}: a refusal writes nothing");
        assert_eq!(field(&fake, &ts[0].1, "Status").as_deref(), Some("Ready"));
    }
}

#[test]
fn stop_works_from_approved() {
    let (fake, _c, b) = crew();
    fake.set_item(&b.item_id, "State", "approved");
    let o = run_ok(&fake, &b, &["stop"]);
    assert!(o.stdout.contains("left"), "{}", o.stdout);
    assert_eq!(field(&fake, &b.item_id, "State").as_deref(), Some("left"));
}

/// The bodies of the "Assigned to you" messages that the poll of `who` delivered.
fn assignment_bodies(fake: &FakeBoard, who: &Agent) -> Vec<String> {
    let (v, _) = fake.poll_json(who);
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "message")
        .filter_map(|e| e["body"].as_str())
        .filter(|b| b.starts_with("Assigned to you"))
        .map(str::to_string)
        .collect()
}

#[test]
fn assign_signs_the_gate_and_reassign_copies_it_forward() {
    let (fake, c, b) = crew();
    // a body with two Gate lines is signed as one line with both: it is not "CI only"
    let t = fake.ticket_issue(
        "IT signed gate",
        "Goal: x.\nGate: ci\nGate: run the demo by hand\nCI: none\n",
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let first = assignment_bodies(&fake, &b);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(
        first[0]
            .lines()
            .any(|l| l == "gate: ci | run the demo by hand"),
        "{}",
        first[0]
    );
    // the board field keeps the first Gate line, as before
    let item = fake.item_of_issue(t).unwrap();
    assert_eq!(field(&fake, &item, "Gate").as_deref(), Some("ci"));
    // reassign (same ticket, still Ready): the new assignment message has the gate line again
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--to",
            &b.name,
            "--reason",
            "late",
            "--why",
            "it: again",
        ],
    );
    let second = assignment_bodies(&fake, &b);
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        second[0].contains("reason: agent is late and does not answer: it: again"),
        "{}",
        second[0]
    );
    assert!(
        second[0]
            .lines()
            .any(|l| l == "gate: ci | run the demo by hand"),
        "{}",
        second[0]
    );
    // control: the line stays the SIGNED gate even if the body is edited afterwards
    fake.set_issue_body(t, "Goal: x.\nGate: ci\n");
    run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--to",
            &b.name,
            "--reason",
            "late",
            "--why",
            "it: third",
        ],
    );
    let third = assignment_bodies(&fake, &b);
    assert!(
        third[0]
            .lines()
            .any(|l| l == "gate: ci | run the demo by hand"),
        "{}",
        third[0]
    );
    no_unhandled(&fake);
}

#[test]
fn the_template_and_the_kind_list_need_no_board_call() {
    let (fake, c, _b) = crew();
    let before = fake.requests().len();
    // `ticket new --template`: no --title, no --body-file, exit 0, the template and nothing else
    let o = fake.run(&c.dir, &["ticket", "new", "--template"]);
    assert_eq!(o.code, 0, "{}", o.all());
    // the template is the body format of docs/design/spec/tickets.md, byte for byte
    let spec = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/design/spec/tickets.md"),
    )
    .unwrap()
    // a Windows checkout may turn the line ends into CRLF
    .replace("\r\n", "\n");
    let block = spec
        .split("```text\n")
        .nth(1)
        .and_then(|r| r.split("```").next())
        .expect("tickets.md has the template block");
    assert_eq!(o.stdout, block);
    assert!(o.stdout.starts_with("Goal: ") && o.stdout.contains("\nGate: "));
    assert_eq!(fake.requests().len(), before, "the template calls no board");
    // a bad kind lists the valid kinds, from the enum
    let o = fake.run(
        &c.dir,
        &[
            "say", "1", "--to", "owner", "--kind", "shout", "--body", "x",
        ],
    );
    assert_ne!(o.code, 0, "{}", o.all());
    let list = Kind::ALL.map(Kind::as_str).join(", ");
    assert!(
        o.stderr
            .contains(&format!("unknown kind 'shout'; valid kinds: {list}")),
        "{}",
        o.stderr
    );
    // `say --help` lists them too
    let o = fake.run(&c.dir, &["say", "--help"]);
    assert!(
        o.stdout.contains(&format!("one of {list}.")),
        "{}",
        o.stdout
    );
}

/// Rule `close-verified-head-moved`: Done after `close --verified` means "accepted at the head
/// that the passing verify saw". A moved head and a PR closed without a merge are refused before
/// any write. A merged PR at the verified head, and a rerun after a new verify, pass.
#[test]
fn close_verified_needs_the_head_that_the_passing_verify_saw() {
    let (fake, c, b) = crew();
    let (t, item, pr) = in_verified_review(&fake, &c, &b, "IT close head");
    let ts = t.to_string();
    // the dev pushes again after the verify: no new done, no new verify
    let moved = fake.push(&pr.branch);
    let before = writes(&fake);
    let o = run_refused(
        &fake,
        &c,
        &["close", &ts, "--verified"],
        "close-verified-head-moved",
    );
    assert!(o.stderr.contains(&moved), "{}", o.stderr);
    assert!(o.stderr.contains(&pr.head), "{}", o.stderr);
    assert_eq!(writes(&fake), before, "the refusal wrote: {}", o.all());
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));

    // a PR that was closed without a merge is refused too
    fake.close_pr(pr.number);
    let o = run_refused(
        &fake,
        &c,
        &["close", &ts, "--verified"],
        "close-verified-head-moved",
    );
    assert!(o.stderr.contains("closed without a merge"), "{}", o.stderr);
    assert_eq!(writes(&fake), before, "the refusal wrote: {}", o.all());
    no_unhandled(&fake);
}

#[test]
fn close_verified_accepts_a_pr_that_was_merged_at_the_verified_head() {
    let (fake, c, b) = crew();
    let (t, item, pr) = in_verified_review(&fake, &c, &b, "IT close merged head");
    let ts = t.to_string();
    fake.merge_pr_by(pr.number, "somebody-else");
    run_ok(&fake, &c, &["close", &ts, "--verified"]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Done"));
    no_unhandled(&fake);
}

/// `close --verified` posts the signed note first and writes the Status last. A failed write
/// leaves a Review ticket (no "Done without proof"), and the rerun finishes.
#[test]
fn a_failed_status_write_of_close_verified_leaves_review_and_a_rerun_finishes() {
    let (fake, c, b) = crew();
    let (t, item, _pr) = in_verified_review(&fake, &c, &b, "IT close failed write");
    let ts = t.to_string();
    fake.fail_select_write(Some("Done"));
    let o = fake.run(&c.dir, &["close", &ts, "--verified"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Review"));
    let notes = |fake: &FakeBoard| {
        fake.comments_of(t)
            .iter()
            .filter(|(_, _, body)| body.contains(" kind=info "))
            .filter(|(_, _, body)| body.contains("Closed after a passed verify"))
            .count()
    };
    assert_eq!(notes(&fake), 1, "the note is posted before the write");
    fake.fail_select_write(None);
    run_ok(&fake, &c, &["close", &ts, "--verified"]);
    assert_eq!(field(&fake, &item, "Status").as_deref(), Some("Done"));
    no_unhandled(&fake);
}
