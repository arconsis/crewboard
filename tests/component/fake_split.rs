//! `ticket split` against a fake GitHub. No token, no sandbox, no real GitHub.
//!
//! A coordinator splits the open part of an In Progress ticket off to a new ticket for the
//! same agent. The headless refusal is in `fake_headless.rs`.

use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::writes;
use crewboard::rules::Role;

const AUTH_NO_SPLIT: &str = "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: false\ncoordinator_may_merge: false\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [merge, release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: no split.\n";

const OLD_BODY: &str =
    "Goal: fix three flaky tests.\n\n**Gate:** all three tests pass 20 times in a row.\nCI: none\n";

struct Setup {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    old: u64,
    old_item: String,
    file: tempfile::TempDir,
}

/// A ticket assigned to B with P1 and taken by B (In Progress).
fn in_progress() -> Setup {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let old = fake.ticket_issue("Three flaky tests", OLD_BODY, &["crew:work"]);
    let ts = old.to_string();
    let o = ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let old_item = o
        .stdout
        .split("added to the board as ")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    ok(&fake, &b, &["take", &ts]);
    Setup {
        fake,
        c,
        b,
        old,
        old_item,
        file: tempfile::tempdir().unwrap(),
    }
}

/// The same, and B stops the ticket with `block`. This is the typical split case:
/// the dev blocks with "the finished part can merge".
fn stopped() -> Setup {
    let s = in_progress();
    let ts = s.old.to_string();
    let _ = s.fake.poll_json(&s.b);
    ok(
        &s.fake,
        &s.b,
        &["block", &ts, "--why", "A is done, B is open"],
    );
    assert_eq!(
        s.fake.item_field(&s.old_item, "Status").as_deref(),
        Some("Blocked")
    );
    s
}

fn ok(fake: &FakeBoard, who: &Agent, args: &[&str]) -> Out {
    let o = fake.run(&who.dir, args);
    assert_eq!(o.code, 0, "{} {args:?}: {}", who.name, o.all());
    o
}

impl Setup {
    fn body_file(&self, text: &str) -> String {
        let p = self.file.path().join("open-part.md");
        std::fs::write(&p, text).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn split(&self, who: &Agent, title: &str, file: &str) -> Out {
        self.fake.run(
            &who.dir,
            &[
                "ticket",
                "split",
                &self.old.to_string(),
                "--title",
                title,
                "--body-file",
                file,
            ],
        )
    }

    /// The command is refused with `rule`, and nothing was written.
    fn refused(&self, who: &Agent, rule: &str) {
        let file = self.body_file("Flaky tests B and C.\n");
        let before = writes(&self.fake);
        let issues = self.fake.open_titles("crew:work").len();
        let o = self.split(who, "Flaky tests B and C", &file);
        assert_eq!(o.code, 2, "{}", o.all());
        assert!(o.stderr.contains(rule), "{rule} missing: {}", o.all());
        assert_eq!(writes(&self.fake), before, "a refusal wrote something");
        assert_eq!(self.fake.open_titles("crew:work").len(), issues);
        assert_eq!(self.fake.issue_text(self.old).unwrap().1, OLD_BODY);
        assert!(self.fake.unhandled().is_empty());
    }
}

#[test]
fn a_split_creates_the_linked_ticket_queues_it_and_posts_one_note() {
    let s = in_progress();
    let file = s.body_file("Flaky tests B and C are still open.\n");
    let comments_before = s.fake.comments_of(s.old).len();
    let o = ok(
        &s.fake,
        &s.c,
        &[
            "ticket",
            "split",
            &s.old.to_string(),
            "--title",
            "Flaky tests B and C",
            "--body-file",
            &file,
        ],
    );
    let new = s.old + 1;
    assert!(
        o.stdout.contains(&format!(
            "-> {REPO}#{new}; now: created, linked, queued, note posted"
        )),
        "{}",
        o.stdout
    );
    // the new ticket: text, link back, Gate and Priority copied
    let (title, body) = s.fake.issue_text(new).unwrap();
    assert_eq!(title, "Flaky tests B and C");
    assert!(
        body.contains("Flaky tests B and C are still open."),
        "{body}"
    );
    assert!(
        body.contains(&format!("Split from: {REPO}#{}", s.old)),
        "{body}"
    );
    assert!(
        body.contains("Gate: all three tests pass 20 times in a row."),
        "{body}"
    );
    let item = s.fake.item_of_issue(new).expect("item of the new ticket");
    assert_eq!(s.fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        s.fake.item_field(&item, "Agent").as_deref(),
        Some(s.b.name.as_str())
    );
    assert_eq!(s.fake.item_field(&item, "Priority"), None);
    assert!(
        s.fake
            .item_field(&item, "Gate")
            .unwrap()
            .contains("20 times")
    );
    // the queue entry: the signed assignment on the new ticket is for B
    let queued = s.fake.comments_of(new);
    assert!(
        queued.iter().any(|(_, _, b)| b.contains("Assigned to you")),
        "{queued:?}"
    );
    // the old ticket: its text plus ONE line, still In Progress, ONE new signed note
    let (_, old_body) = s.fake.issue_text(s.old).unwrap();
    assert_eq!(
        old_body,
        format!("{OLD_BODY}\nSplit off: {REPO}#{new} - Flaky tests B and C\n")
    );
    assert_eq!(
        s.fake.item_field(&s.old_item, "Status").as_deref(),
        Some("In Progress")
    );
    let after = s.fake.comments_of(s.old);
    assert_eq!(after.len(), comments_before + 1, "{after:?}");
    let note = &after.last().unwrap().2;
    assert!(
        note.contains("shrank") && note.contains(&format!("#{new}")),
        "{note}"
    );
    assert!(
        note.contains("kind=info") && note.contains("sig=") && note.contains(&s.c.name),
        "{note}"
    );
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn a_gate_line_in_the_body_file_wins_over_the_old_gate() {
    let s = in_progress();
    let file = s.body_file("Open part.\n\n**Gate:** the demo runs by hand.\n");
    ok(
        &s.fake,
        &s.c,
        &[
            "ticket",
            "split",
            &s.old.to_string(),
            "--title",
            "Own gate",
            "--body-file",
            &file,
        ],
    );
    let new = s.old + 1;
    let (_, body) = s.fake.issue_text(new).unwrap();
    assert!(!body.contains("20 times"), "{body}");
    let item = s.fake.item_of_issue(new).unwrap();
    assert_eq!(
        s.fake.item_field(&item, "Gate").as_deref(),
        Some("the demo runs by hand.")
    );
}

#[test]
fn a_second_identical_split_creates_nothing_and_says_so() {
    let s = in_progress();
    let file = s.body_file("Flaky tests B and C.\n");
    ok(
        &s.fake,
        &s.c,
        &[
            "ticket",
            "split",
            &s.old.to_string(),
            "--title",
            "Flaky tests B and C",
            "--body-file",
            &file,
        ],
    );
    let before = writes(&s.fake);
    let titles = s.fake.open_titles("crew:work").len();
    let o = s.split(&s.c, "Flaky tests B and C", &file);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("already"), "{}", o.stdout);
    assert_eq!(writes(&s.fake), before, "the second split wrote something");
    assert_eq!(s.fake.open_titles("crew:work").len(), titles);
    // control: another title is another split and does create a ticket
    let o = s.split(&s.c, "Flaky test D", &file);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(s.fake.open_titles("crew:work").len(), titles + 1);
}

#[test]
fn a_split_is_refused_when_authority_forbids_it_and_nothing_is_written() {
    let s = in_progress();
    s.fake.file("AUTHORITY.md", AUTH_NO_SPLIT);
    s.refused(&s.c, "authority-split-not-allowed");
}

#[test]
fn a_dev_agent_cannot_split_and_nothing_is_written() {
    let s = in_progress();
    s.refused(&s.b, "status-wrong-writer");
}

#[test]
fn only_an_in_progress_ticket_is_split() {
    let s = in_progress();
    s.fake.set_item(&s.old_item, "Status", "Review");
    s.refused(&s.c, "split-not-in-progress");
}

/// One split breaks at the request `(method, path, body)`; the same command again repairs it.
fn broken_split_is_repaired(
    s: Setup,
    status: &str,
    method: &str,
    path_of: impl Fn(u64) -> String,
    body: &str,
) {
    let file = s.body_file("Flaky tests B and C.\n");
    s.fake.fail_once(method, &path_of(s.old), body);
    let first = s.split(&s.c, "Flaky tests B and C", &file);
    assert_ne!(
        first.code,
        0,
        "the injected failure did not fire: {}",
        first.all()
    );
    let second = s.split(&s.c, "Flaky tests B and C", &file);
    assert_eq!(second.code, 0, "{}", second.all());
    assert!(second.stdout.contains("split: "), "{}", second.stdout);
    let new = s.old + 1;
    // exactly one new ticket
    assert_eq!(
        s.fake
            .open_titles("crew:work")
            .iter()
            .filter(|t| *t == "Flaky tests B and C")
            .count(),
        1
    );
    assert!(
        s.fake.issue_text(new + 1).is_none(),
        "a second ticket exists"
    );
    // both links
    let (_, nb) = s.fake.issue_text(new).unwrap();
    assert!(
        nb.contains(&format!("Split from: {REPO}#{}", s.old)),
        "{nb}"
    );
    let (_, ob) = s.fake.issue_text(s.old).unwrap();
    assert_eq!(
        ob,
        format!("{OLD_BODY}\nSplit off: {REPO}#{new} - Flaky tests B and C\n")
    );
    // the queue entry
    let item = s.fake.item_of_issue(new).expect("item");
    assert_eq!(s.fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        s.fake.item_field(&item, "Agent").as_deref(),
        Some(s.b.name.as_str())
    );
    let assigned = s
        .fake
        .comments_of(new)
        .iter()
        .filter(|(_, _, b)| b.contains("Assigned to you"))
        .count();
    assert_eq!(assigned, 1);
    // exactly one note on the old ticket, and the old ticket stays In Progress
    let notes = s
        .fake
        .comments_of(s.old)
        .iter()
        .filter(|(_, _, b)| b.contains("shrank"))
        .count();
    assert_eq!(notes, 1);
    assert_eq!(
        s.fake.item_field(&s.old_item, "Status").as_deref(),
        Some(status)
    );
    // a third run changes nothing
    let before = writes(&s.fake);
    let third = s.split(&s.c, "Flaky tests B and C", &file);
    assert_eq!(third.code, 0, "{}", third.all());
    assert_eq!(writes(&s.fake), before);
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn a_split_that_failed_after_the_create_is_repaired_by_the_same_command() {
    // the PATCH of the old body fails: the new issue exists, nothing else
    broken_split_is_repaired(
        in_progress(),
        "In Progress",
        "PATCH",
        |old| format!("/issues/{old}"),
        "",
    );
}

#[test]
fn a_split_that_failed_after_the_link_is_repaired_by_the_same_command() {
    // the board item of the new ticket fails: the issue and the link exist
    broken_split_is_repaired(
        in_progress(),
        "In Progress",
        "POST",
        |_| "/graphql".to_string(),
        "addProjectV2ItemById",
    );
}

#[test]
fn a_split_that_failed_after_the_assign_is_repaired_by_the_same_command() {
    // the note on the old ticket fails: the ticket is queued already
    broken_split_is_repaired(
        in_progress(),
        "In Progress",
        "POST",
        |old| format!("/issues/{old}/comments"),
        "",
    );
}

#[test]
fn a_split_of_a_blocked_ticket_that_failed_after_the_assign_is_repaired_without_a_second_ticket() {
    // The note fails; the rerun finishes the split, the ticket stays Blocked
    broken_split_is_repaired(
        stopped(),
        "Blocked",
        "POST",
        |old| format!("/issues/{old}/comments"),
        "",
    );
}

#[test]
fn a_split_of_a_blocked_ticket_that_failed_after_the_create_is_repaired_without_a_second_ticket() {
    broken_split_is_repaired(
        stopped(),
        "Blocked",
        "PATCH",
        |old| format!("/issues/{old}"),
        "",
    );
}

/// Split of a stopped ticket. The old ticket keeps its status; the new ticket is
/// queued; ONE note tells the agent that `done` needs no take.
fn stopped_ticket_is_split() {
    let s = stopped();
    let file = s.body_file("Flaky tests B and C are still open.\n");
    let comments_before = s.fake.comments_of(s.old).len();
    let o = s.split(&s.c, "Flaky tests B and C", &file);
    assert_eq!(o.code, 0, "{}", o.all());
    let new = s.old + 1;
    assert!(
        o.stdout.contains(&format!(
            "-> {REPO}#{new}; now: created, linked, queued, note posted; already done: -; {REPO}#{} stays Blocked",
            s.old
        )),
        "{}",
        o.stdout
    );
    let (_, old_body) = s.fake.issue_text(s.old).unwrap();
    assert_eq!(
        old_body,
        format!("{OLD_BODY}\nSplit off: {REPO}#{new} - Flaky tests B and C\n")
    );
    let item = s.fake.item_of_issue(new).expect("item of the new ticket");
    assert_eq!(s.fake.item_field(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(
        s.fake.item_field(&item, "Agent").as_deref(),
        Some(s.b.name.as_str())
    );
    // the old ticket keeps its status and its agent
    assert_eq!(
        s.fake.item_field(&s.old_item, "Status").as_deref(),
        Some("Blocked")
    );
    assert_eq!(
        s.fake.item_field(&s.old_item, "Agent").as_deref(),
        Some(s.b.name.as_str())
    );
    let after = s.fake.comments_of(s.old);
    assert_eq!(after.len(), comments_before + 1, "{after:?}");
    let note = &after.last().unwrap().2;
    assert!(
        note.contains("shrank")
            && note.contains(&format!("done {REPO}#{} --pr", s.old))
            && note.contains("No `take` is needed"),
        "{note}"
    );
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn a_blocked_ticket_is_split_and_stays_blocked() {
    stopped_ticket_is_split();
}

#[test]
fn a_ready_or_done_ticket_is_still_not_split() {
    for status in ["Ready", "Done", "Backlog"] {
        let s = in_progress();
        s.fake.set_item(&s.old_item, "Status", status);
        s.refused(&s.c, "split-not-in-progress");
    }
}
