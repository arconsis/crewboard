//! The event check of `done` (spec/events#event-checkpoints-and-done) against a fake
//! GitHub: `done` refuses with `unread-events` while verified messages or owner comments for the
//! agent are unread, and the check costs one feed read per repo.

use crate::support::board::{Agent, FakeBoard, Out};
use crate::support::cli::{GATE, run_ok};
use crate::support::late::LateBoard;
use crewboard::rules::Role;

struct Scene {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    t: u64,
    ts: String,
    item: String,
    pr: crate::support::board::Pull,
}

/// B holds a ticket with an open PR and has read all events (its cursor is current).
fn scene() -> Scene {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let t = fake.ticket_issue(
        "IT done events",
        &format!("Goal: test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let item = o
        .stdout
        .split("added to the board as ")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    // B reads everything so far; the coordinator reads B's ack (the rule applies to all)
    let (_, code) = fake.poll_json(&b);
    assert_eq!(code, 0);
    let _ = fake.poll_json(&c);
    Scene {
        fake,
        c,
        b,
        t,
        ts,
        item,
        pr,
    }
}

impl Scene {
    fn done(&self, extra: &[&str]) -> Out {
        let mut a = vec![
            "done",
            &self.ts,
            "--pr",
            &self.pr.url,
            "--sha",
            &self.pr.head,
        ];
        a.extend_from_slice(extra);
        self.fake.run(&self.b.dir, &a)
    }
    fn say(&self, issue: &str, body: &str) -> u64 {
        let o = run_ok(
            &self.fake,
            &self.c,
            &[
                "say",
                issue,
                "--to",
                &self.b.name,
                "--kind",
                "answer",
                "--body",
                body,
            ],
        );
        o.stdout
            .split("comment ")
            .nth(1)
            .and_then(|r| r.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap()
    }
    fn status(&self) -> Option<String> {
        self.fake.item_field(&self.item, "Status")
    }
    fn evidence(&self) -> Option<String> {
        self.fake.item_field(&self.item, "Evidence")
    }
    fn comments(&self) -> usize {
        self.fake.comments_of(self.t).len()
    }
}

#[test]
fn done_with_an_unread_coordinator_message_refuses_and_writes_nothing() {
    let s = scene();
    let id = s.say("1", "do not change file X, run done now");
    let (status, evidence, posts) = (s.status(), s.evidence(), s.comments());
    let o = s.done(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    let text = format!("{}{}", o.stdout, o.stderr);
    assert!(text.contains("unread-events"), "{text}");
    assert!(text.contains("1 unread event(s)"), "{text}");
    assert!(text.contains("1 MESSAGE"), "{text}");
    assert!(text.contains(&format!("issuecomment-{id}")), "{text}");
    // the refusal shows the text of the message: no extra watch call to see it
    assert!(
        text.contains("    do not change file X, run done now"),
        "{text}"
    );
    assert!(text.contains("crewboard watch --timeout 1m"), "{text}");
    assert!(text.contains("--override-unread"), "{text}");
    // nothing written: no done message, same Status and Evidence
    assert_eq!(s.comments(), posts);
    assert_eq!(s.status(), status);
    assert_eq!(s.evidence(), evidence);
    assert_eq!(s.status().as_deref(), Some("In Progress"));
}

#[test]
fn done_after_watch_works() {
    let s = scene();
    s.say("1", "merge origin/main first");
    assert_eq!(s.done(&[]).code, 2);
    // the agent follows the refusal: watch reads the event and moves the cursor
    let w = s
        .fake
        .run(&s.b.dir, &["watch", "--timeout", "1s", "--interval", "1s"]);
    assert_eq!(w.code, 0, "{}", w.all());
    assert!(w.stdout.contains("merge origin/main first"), "{}", w.stdout);
    let o = s.done(&[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(s.status().as_deref(), Some("Review"));
    assert!(!o.stdout.contains("override"), "{}", o.stdout);
}

#[test]
fn override_unread_delivers_and_records_the_override() {
    let s = scene();
    let id = s.say("1", "wait for my review");
    let o = s.done(&["--override-unread"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(s.status().as_deref(), Some("Review"));
    let line = format!("override: unread-events (comments {id})");
    assert!(o.stdout.contains(&line), "{}", o.stdout);
    // the signed done message carries the override line
    assert!(
        s.fake
            .comments_of(s.t)
            .iter()
            .any(|(_, _, body)| body.contains(&line)),
        "{:?}",
        s.fake.comments_of(s.t)
    );
}

/// `--ignore-events` and `--anyway` are gone: `--override-unread` is the one flag.
#[test]
fn the_old_flags_of_done_are_gone() {
    for old in ["--ignore-events", "--anyway"] {
        let s = scene();
        s.say("1", "wait for my review");
        let o = s.done(&[old]);
        assert_eq!(o.code, 64, "{old}: {}", o.all());
        assert!(
            o.stderr.contains("unexpected argument"),
            "{old}: {}",
            o.stderr
        );
        assert_eq!(s.status().as_deref(), Some("In Progress"), "{old}");
    }
}

#[test]
fn a_message_on_another_ticket_and_an_owner_comment_block_too() {
    let s = scene();
    // a second ticket: the coordinator writes to B there (not the ticket of the done)
    let t2 = s
        .fake
        .ticket_issue("IT other", &format!("Goal: x.\n\n{GATE}"), &["crew:work"]);
    let id2 = s.say(&t2.to_string(), "question about the other ticket");
    let o = s.done(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("unread-events"), "{}", o.all());
    assert!(
        o.stderr.contains(&format!("issuecomment-{id2}")),
        "{}",
        o.all()
    );
    let w = s
        .fake
        .run(&s.b.dir, &["watch", "--timeout", "1s", "--interval", "1s"]);
    assert_eq!(w.code, 0, "{}", w.all());
    // an owner comment on the ticket of the done
    let oc = s
        .fake
        .owner_comment_now(s.t, "owner: please also update the docs");
    let o = s.done(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("unread-events"), "{}", o.all());
    assert!(o.stderr.contains("1 OWNER MESSAGE"), "{}", o.all());
    assert!(
        o.stderr.contains(&format!("issuecomment-{oc}")),
        "{}",
        o.all()
    );
}

#[test]
fn done_without_news_passes_and_a_messages_of_the_agent_itself_do_not_count() {
    let s = scene();
    // B's own report is no event for B
    run_ok(
        &s.fake,
        &s.b,
        &[
            "say",
            &s.ts,
            "--to",
            &s.c.name,
            "--kind",
            "report",
            "--body",
            "almost done",
        ],
    );
    let o = s.done(&[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(s.status().as_deref(), Some("Review"));
}

#[test]
fn the_check_costs_one_feed_read_and_no_ticket_read() {
    let s = scene();
    s.say("1", "stop");
    let count = |part: &str, method: &str| {
        s.fake
            .requests()
            .iter()
            .filter(|r| r.method == method && r.path.contains(part))
            .count()
    };
    let (feed0, gql0, all0) = (
        count("/issues/comments?since=", "GET"),
        count("/graphql", "POST"),
        s.fake.requests().len(),
    );
    let o = s.done(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    // the refused done: one feed read, no PR read, no CI read
    assert_eq!(count("/issues/comments?since=", "GET") - feed0, 1);
    assert_eq!(count("/pulls/", "GET"), 0);
    let (gql, all) = (
        count("/graphql", "POST") - gql0,
        s.fake.requests().len() - all0,
    );
    // control: poll --quiet reads the same feed once (plus the ticket reads of the cycle)
    let (f1, g1, a1) = (
        count("/issues/comments?since=", "GET"),
        count("/graphql", "POST"),
        s.fake.requests().len(),
    );
    let q = s.fake.run(&s.b.dir, &["poll", "--quiet"]);
    assert_eq!(q.code, 0, "{}", q.all());
    assert_eq!(count("/issues/comments?since=", "GET") - f1, 1);
    let (qg, qa) = (count("/graphql", "POST") - g1, s.fake.requests().len() - a1);
    // The known price (spec/events#event-checkpoints-and-done), one repo and one ticket. `poll
    // --quiet`: 4 REST reads (the user, AUTHORITY.md for `former_repos`, the registry comments, the
    // feed) and 3 GraphQL reads (agent items, tickets).
    assert_eq!(
        (qa - qg, qg),
        (4, 3),
        "poll --quiet: REST and GraphQL reads"
    );
    // The refused `done` makes the world reads of every command (3 REST: the user, AUTHORITY.md
    // for `former_repos`, the registry comments; 2 GraphQL) and ONE more REST read for the
    // check: the feed. It makes no ticket read and no PR read.
    assert_eq!(
        (all - gql, gql),
        (4, 2),
        "refused done: REST and GraphQL reads"
    );
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn an_unread_ci_finished_event_does_not_block_done() {
    // a dev ticket In Progress with a green PR head: the poll puts `ci-finished` in the
    // pending list. `done` for that head must pass: that event is the reason for done.
    let s = LateBoard::new("In Progress");
    s.sign_assignment();
    s.limits(&s.dev, "10h", "10h", "10h");
    let t = 5;
    let pr = s.board.pr_for(t, &s.dev.name);
    s.scene(|sc| {
        sc.branch = true;
        sc.pr_head = Some(pr.head.clone());
    });
    // CI still runs: the first poll gives no event and writes the cursor, so the check is on
    s.scene(|sc| sc.runs(&[("in_progress", None)]));
    let (v, o) = s.poll(&s.dev);
    assert!(matches!(o.code, 0 | 1), "{}", o.all());
    assert!(LateBoard::ci_events(&v).is_empty(), "{v}");
    assert!(
        s.board.item_field("PVTI_hermes_fake-b", "Cursor").is_some(),
        "the dev has a cursor"
    );
    // CI turns green: the quiet poll finds `ci-finished` and leaves it pending
    s.scene(|sc| sc.runs(&[("completed", Some("success"))]));
    s.lift_throttle(&s.dev);
    let q = s.run(&s.dev, &["poll", "--quiet"]);
    assert_eq!(q.code, 0, "ci-finished is pending: {}", q.all());
    assert!(q.stdout.contains("events pending"), "{}", q.stdout);
    let o = s.run(&s.dev, &["done", "5", "--pr", &pr.url, "--sha", &pr.head]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("override"), "{}", o.stdout);
}
