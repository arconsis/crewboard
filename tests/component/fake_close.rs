//! `close <n>` (spec/tickets#close-after-an-owner-merge and
//! spec/tickets#close-by-judgment), against the fake GitHub of `tests/support/board.rs`.
//!
//! The case: the owner merges the PR of a crew ticket by hand. GitHub closes the ticket issue,
//! but the board item stays where it was. The coordinator (and the headless `tick`) moves it to
//! Done when ALL of these hold: the issue is closed; a PR in a repo of the initiative is merged
//! and its body closes the issue; the merge actor is the owner. Without an owner merge, `close`
//! needs `--reason "<text>"` (the coordinator's judgment) and is refused with
//! `close-needs-reason` otherwise. Each refusal writes nothing. Done is final.

use crate::support::authority::authority_with;
use crate::support::board::{Agent, FakeBoard, Pull};
use crate::support::cli::{GATE, item_in, run_ok, writes};
use crewboard::rules::Role;

/// AUTHORITY.md with an owner login that is NOT the owner of the board file.
fn authority(owner: &str) -> String {
    authority_with(owner, "release", "")
}

struct Case {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    t: u64,
    item: String,
    pr: Pull,
}

/// Refused with `rule` (exit 2), the message holds `text`, and nothing was written.
fn refused_no_write(
    fake: &FakeBoard,
    dir: &std::path::Path,
    args: &[&str],
    rule: &str,
    text: &str,
) {
    let before = writes(fake);
    let o = fake.run(dir, args);
    assert_eq!(o.code, 2, "{args:?}: {}", o.all());
    assert!(
        o.stderr.contains(rule),
        "{rule} missing for {args:?}: {}",
        o.all()
    );
    assert!(
        o.stderr.contains(text),
        "{text:?} missing for {args:?}: {}",
        o.all()
    );
    assert_eq!(writes(fake), before, "the refusal wrote: {}", o.all());
}

fn status(k: &Case) -> Option<String> {
    k.fake.item_field(&k.item, "Status")
}

/// A ticket that stands in `stage`, with a PR that closes it. In Review the PR is the evidence
/// of a signed `done`. In the other stages there is no `done`, and the timeline of the issue
/// names the PR (the owner merged a PR that the board does not know).
fn case(stage: &str) -> Case {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(&format!("IT 163 {stage}"), &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    match stage {
        "review" => {
            run_ok(
                &fake,
                &b,
                &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
            );
        }
        "asked" => {
            // `ask` blocks the ticket and sets the label
            run_ok(
                &fake,
                &b,
                &["ask", &ts, "--body", "Release go for this change?"],
            );
            fake.cross_reference(t, pr.number);
        }
        "blocked" => {
            run_ok(&fake, &b, &["block", &ts, "--why", "waits for the owner"]);
            fake.cross_reference(t, pr.number);
        }
        "in-progress" => fake.cross_reference(t, pr.number),
        "ready" => {
            // the work was delivered, then the question was taken back to the queue
            run_ok(
                &fake,
                &b,
                &["ask", &ts, "--body", "Release go for this change?"],
            );
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
                    "back to the queue",
                ],
            );
            fake.cross_reference(t, pr.number);
        }
        other => panic!("stage {other}"),
    }
    let k = Case {
        fake,
        c,
        b,
        t,
        item,
        pr,
    };
    let want = match stage {
        "review" => "Review",
        "asked" | "blocked" => "Blocked",
        "ready" => "Ready",
        _ => "In Progress",
    };
    assert_eq!(status(&k).as_deref(), Some(want));
    k
}

/// The owner merges the PR in the browser; GitHub closes the issue.
fn owner_merges(k: &Case) {
    k.fake.merge_pr(k.pr.number);
    k.fake.close_issue(k.t);
}

fn needs_owner(k: &Case) -> bool {
    k.fake.labels_of(k.t).contains(&"needs-owner".to_string())
}

#[test]
fn the_coordinator_closes_a_ticket_whose_pr_the_owner_merged() {
    for stage in ["review", "asked", "blocked", "in-progress"] {
        let k = case(stage);
        let ts = k.t.to_string();
        owner_merges(&k);
        if stage == "asked" {
            assert!(
                needs_owner(&k),
                "control: the label is set before the close"
            );
        }
        let o = run_ok(&k.fake, &k.c, &["close", &ts]);
        assert!(
            o.stdout.contains("closed: Status Done"),
            "{stage}: {}",
            o.all()
        );
        assert_eq!(status(&k).as_deref(), Some("Done"), "{stage}");
        assert!(!needs_owner(&k), "{stage}: the label went");
        // the signed info to the assignee: the PR, the merge commit, who merged
        let merge_sha = format!("{:040x}", 0xabc0_u128 + u128::from(k.pr.number));
        let notice = k
            .fake
            .comments_of(k.t)
            .into_iter()
            .map(|(_, _, body)| body)
            .find(|b| b.contains("merged by the owner"))
            .unwrap_or_else(|| panic!("{stage}: no notice"));
        assert!(notice.contains(&format!("pr: {}", k.pr.url)), "{notice}");
        assert!(
            notice.contains(&format!("closed: owner merged {}", k.pr.url)),
            "{notice}"
        );
        assert!(
            notice.contains(&format!("merge-commit: {merge_sha}")),
            "{notice}"
        );
        // idempotent: a ticket that is Done is status-no-change, and nothing is written
        refused_no_write(
            &k.fake,
            &k.c.dir,
            &["close", &ts],
            "status-no-change",
            "already Done",
        );
        assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
    }
}

#[test]
fn every_failed_condition_is_close_needs_reason_and_writes_nothing() {
    // the issue is still open
    let k = case("review");
    k.fake.merge_pr(k.pr.number);
    refused_no_write(
        &k.fake,
        &k.c.dir,
        &["close", &k.t.to_string()],
        "close-needs-reason",
        "the issue is still open",
    );
    // control: the same ticket closes once GitHub has closed the issue
    k.fake.close_issue(k.t);
    run_ok(&k.fake, &k.c, &["close", &k.t.to_string()]);
    assert_eq!(status(&k).as_deref(), Some("Done"));

    // the PR is not merged (open, or closed without a merge)
    let k = case("review");
    k.fake.close_issue(k.t);
    refused_no_write(
        &k.fake,
        &k.c.dir,
        &["close", &k.t.to_string()],
        "close-needs-reason",
        "is not merged",
    );
    k.fake.close_pr(k.pr.number);
    refused_no_write(
        &k.fake,
        &k.c.dir,
        &["close", &k.t.to_string()],
        "close-needs-reason",
        "is not merged",
    );

    // the merge actor is not the owner (the crew account merged it)
    let k = case("review");
    k.fake.merge_pr_by(k.pr.number, "fake-crew");
    k.fake.close_issue(k.t);
    refused_no_write(
        &k.fake,
        &k.c.dir,
        &["close", &k.t.to_string()],
        "close-needs-reason",
        "merged by fake-crew, not by the owner (fake-owner)",
    );

    // `owner_login` of the board file decides, not `owner:` of AUTHORITY.md (which `start` keeps
    // equal): a file that names another login changes nothing for `close`
    let k = case("review");
    owner_merges(&k);
    k.fake.file("AUTHORITY.md", &authority("someone-else"));
    run_ok(&k.fake, &k.c, &["close", &k.t.to_string()]);
    assert_eq!(status(&k).as_deref(), Some("Done"));

    // the body of the PR does not close this ticket
    let k = case("review");
    owner_merges(&k);
    k.fake.set_pr_text(k.pr.number, "Work", "Refs ticket only");
    refused_no_write(
        &k.fake,
        &k.c.dir,
        &["close", &k.t.to_string()],
        "close-needs-reason",
        "does not close",
    );

    // no PR is named: no done, and the timeline holds no cross-reference
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let t = fake.ticket_issue(
        "IT 163 no pr",
        &format!("Goal: x.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["ask", &ts, "--body", "Go?"]);
    fake.close_issue(t);
    refused_no_write(
        &fake,
        &c.dir,
        &["close", &ts],
        "close-needs-reason",
        "no PR is named",
    );
}

#[test]
fn a_ticket_back_in_ready_after_its_work_closes_after_the_owner_merge() {
    let k = case("ready");
    assert_eq!(status(&k).as_deref(), Some("Ready"));
    owner_merges(&k);
    let ts = k.t.to_string();
    let o = run_ok(&k.fake, &k.c, &["close", &ts]);
    assert!(o.stdout.contains("Status Done"), "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Done"));
    assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
}

#[test]
fn only_the_coordinator_closes_and_the_flags_do_not_mix() {
    let k = case("review");
    owner_merges(&k);
    let ts = k.t.to_string();
    // a dev agent: the usual coordinator-only rule
    refused_no_write(
        &k.fake,
        &k.b.dir,
        &["close", &ts],
        "status-wrong-writer",
        "coordinator action",
    );
    assert_eq!(status(&k).as_deref(), Some("Review"));
    // --verified and --reason exclude each other, and the old flags are gone (clap, before any
    // config or read)
    let before = k.fake.requests().len();
    let o = k
        .fake
        .run(&k.c.dir, &["close", &ts, "--verified", "--why", "x"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("cannot be used with"), "{}", o.all());
    for gone in ["--merge", "--merged", "--wont-fix"] {
        let o = k.fake.run(&k.c.dir, &["close", &ts, gone]);
        assert_ne!(o.code, 0, "{gone}: {}", o.all());
        assert!(o.stderr.contains("unexpected argument"), "{}", o.all());
    }
    let o = k
        .fake
        .run(&k.c.dir, &["close", &ts, "--superseded-by", "2"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert_eq!(k.fake.requests().len(), before);
}

#[test]
fn a_reason_is_added_as_a_line_to_the_note_of_an_owner_merge() {
    let k = case("review");
    owner_merges(&k);
    let ts = k.t.to_string();
    run_ok(&k.fake, &k.c, &["close", &ts, "--why", "reviewed\nby hand"]);
    assert_eq!(status(&k).as_deref(), Some("Done"));
    let notice = k
        .fake
        .comments_of(k.t)
        .into_iter()
        .map(|(_, _, body)| body)
        .find(|b| b.contains("merged by the owner"))
        .expect("a notice");
    assert!(
        notice.contains(&format!("closed: owner merged {}", k.pr.url)),
        "{notice}"
    );
    assert!(notice.contains("reason: reviewed by hand"), "{notice}");
}

/// A config dir of the coordinator with `headless = true`.
fn headless(k: &Case) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&k.c, dir.path());
    dir
}

#[test]
fn the_headless_tick_closes_review_and_asked_tickets_after_an_owner_merge() {
    for stage in ["review", "asked"] {
        let k = case(stage);
        let dir = headless(&k);
        // control: an open issue is left alone, and the tick writes nothing for it
        let o = k.fake.run(dir.path(), &["tick"]);
        assert_eq!(o.code, 0, "{stage}: {}", o.all());
        assert!(!o.stdout.contains("closed: Status Done"), "{}", o.all());
        assert_ne!(status(&k).as_deref(), Some("Done"), "{stage}");
        // the owner merges: GitHub closes the issue
        owner_merges(&k);
        let o = k.fake.run(dir.path(), &["tick"]);
        assert_eq!(o.code, 0, "{stage}: {}", o.all());
        assert!(
            o.stdout.contains("closed: Status Done"),
            "{stage}: {}",
            o.all()
        );
        assert_eq!(status(&k).as_deref(), Some("Done"), "{stage}");
        assert!(!needs_owner(&k), "{stage}");
        // a second tick has nothing to close
        let w = writes(&k.fake);
        let o = k.fake.run(dir.path(), &["tick"]);
        assert_eq!(o.code, 0, "{stage}: {}", o.all());
        assert!(!o.stdout.contains("closed: Status Done"), "{}", o.all());
        assert_eq!(writes(&k.fake), w, "{stage}: the second tick wrote");
        assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
    }
}

#[test]
fn the_headless_tick_does_not_close_a_ticket_that_someone_else_merged() {
    for stage in ["review", "asked"] {
        let k = case(stage);
        let dir = headless(&k);
        k.fake.merge_pr_by(k.pr.number, "fake-crew");
        k.fake.close_issue(k.t);
        let o = k.fake.run(dir.path(), &["tick"]);
        assert_eq!(o.code, 0, "{stage}: {}", o.all());
        assert!(!o.stdout.contains("closed: Status Done"), "{}", o.all());
        assert_ne!(status(&k).as_deref(), Some("Done"), "{stage}");
        // both are left for the interactive coordinator
        assert!(
            o.stdout.contains("left for the interactive coordinator"),
            "{stage}: {}",
            o.all()
        );
    }
}

#[test]
fn the_headless_tick_leaves_a_blocked_ticket_whose_issue_the_owner_closed_without_a_merge() {
    let k = case("blocked");
    let dir = headless(&k);
    k.fake.close_issue(k.t);
    let key = format!("{}#{}", crate::support::board::REPO, k.t);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains(&format!(
            "left for the interactive coordinator: {key}, Blocked, issue closed:"
        )),
        "{}",
        o.all()
    );
    assert_eq!(status(&k).as_deref(), Some("Blocked"));
    // the line repeats and writes nothing
    let w = writes(&k.fake);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("Blocked, issue closed:"), "{}", o.all());
    assert_eq!(writes(&k.fake), w, "the second tick wrote");
    assert_eq!(status(&k).as_deref(), Some("Blocked"));
}

#[test]
fn the_headless_scope_lets_a_bare_close_through_and_refuses_the_judgments() {
    let k = case("review");
    owner_merges(&k);
    let dir = headless(&k);
    let ts = k.t.to_string();
    // refused: `close --verified` and `close --why` stay outside the scope
    let before = k.fake.requests().len();
    let denied: [&[&str]; 2] = [&["close", &ts, "--verified"], &["close", &ts, "--why", "x"]];
    for args in denied {
        let o = k.fake.run(dir.path(), args);
        assert_eq!(o.code, 2, "{args:?}: {}", o.all());
        assert!(o.stderr.contains("headless-scope"), "{}", o.all());
    }
    assert_eq!(k.fake.requests().len(), before, "a request was sent");
    // pass: a bare close after an owner merge is no judgment
    let o = k.fake.run(dir.path(), &["close", &ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Done"));
}

/// One ticket per dev in work (`stage` is `in-progress` or `blocked`), each with a PR
/// that closes it. Returns the ticket, its item and its PR.
fn work_ticket(fake: &FakeBoard, c: &Agent, name: &str, stage: &str) -> (u64, String, Pull) {
    let b = fake.agent(&format!("{name}@fake-{name}"), Role::Dev);
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(&format!("IT 192 {name}"), &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    if stage == "blocked" {
        run_ok(fake, &b, &["block", &ts, "--why", "waits for the owner"]);
    }
    fake.cross_reference(t, pr.number);
    (t, item, pr)
}

/// The requests that read the issue `t` (the issue itself, its comments or its timeline).
fn issue_reads(fake: &FakeBoard, t: u64) -> usize {
    let part = format!("/issues/{t}");
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&part))
        .count()
}

#[test]
fn the_headless_tick_closes_in_progress_and_blocked_tickets_after_an_owner_merge() {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let (t1, i1, p1) = work_ticket(&fake, &c, "hermes", "in-progress");
    let (t2, i2, p2) = work_ticket(&fake, &c, "atlas", "blocked");
    let (t3, i3, _p3) = work_ticket(&fake, &c, "iris", "in-progress");
    assert_eq!(
        fake.item_field(&i1, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(fake.item_field(&i2, "Status").as_deref(), Some("Blocked"));
    let dir = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, dir.path());
    // control: all issues open, so the tick reads no issue for any of the three tickets
    let before: Vec<usize> = [t1, t2, t3]
        .iter()
        .map(|t| issue_reads(&fake, *t))
        .collect();
    let o = fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("closed: Status Done"), "{}", o.all());
    for (t, b) in [t1, t2, t3].iter().zip(&before) {
        assert_eq!(issue_reads(&fake, *t), *b, "open issue #{t} was read");
    }
    let before3 = issue_reads(&fake, t3);
    // the owner merges two PRs; GitHub closes their issues
    for (t, p) in [(t1, &p1), (t2, &p2)] {
        fake.merge_pr(p.number);
        fake.close_issue(t);
    }
    let o = fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        o.stdout.matches("closed: Status Done").count(),
        2,
        "{}",
        o.all()
    );
    assert_eq!(fake.item_field(&i1, "Status").as_deref(), Some("Done"));
    assert_eq!(fake.item_field(&i2, "Status").as_deref(), Some("Done"));
    assert_eq!(
        fake.item_field(&i3, "Status").as_deref(),
        Some("In Progress")
    );
    assert_eq!(issue_reads(&fake, t3), before3, "the open issue was read");
    // the dev hears one note about the merge (the close), not a second one about the merged PR
    for t in [t1, t2] {
        let notes = fake
            .comments_of(t)
            .into_iter()
            .filter(|(_, _, b)| b.contains("merged by the owner") || b.contains("was merged by"))
            .count();
        assert_eq!(notes, 1, "ticket #{t}");
    }
    // a second tick changes nothing
    let w = writes(&fake);
    let o = fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(writes(&fake), w, "the second tick wrote: {}", o.all());
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn the_headless_tick_leaves_a_blocked_ticket_whose_pr_has_no_closing_keyword() {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let (t, item, pr) = work_ticket(&fake, &c, "hermes", "blocked");
    fake.merge_pr(pr.number);
    fake.close_issue(t);
    fake.set_pr_text(pr.number, "Work", "Ticket fake-owner/fake-repo#1 only");
    let dir = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, dir.path());
    let w = writes(&fake);
    let o = fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Blocked"));
    assert!(!o.stdout.contains("closed: Status Done"), "{}", o.all());
    // the refusal is `close-needs-reason`: the ticket is left for the interactive coordinator
    assert!(
        o.stdout.contains("left for the interactive coordinator"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), w, "the tick wrote: {}", o.all());
}

/// The signed bodies of kind info on an issue that hold `needle`.
fn infos(fake: &FakeBoard, issue: u64, needle: &str) -> Vec<String> {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains("kind=info") && b.contains(needle))
        .collect()
}

struct Plain {
    n: u64,
    ns: String,
    item: String,
}

/// A ticket without a PR in `stage`. `backlog`: on the board without an assignee. `ready`:
/// assigned, not taken.
fn plain(fake: &FakeBoard, c: &Agent, b: &Agent, stage: &str) -> Plain {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let n = fake.ticket_issue(&format!("IT 184 {stage}"), &body, &["crew:work"]);
    let ns = n.to_string();
    let item = if stage == "backlog" {
        item_in(&run_ok(fake, c, &["ticket", "adopt", &ns]))
    } else {
        item_in(&run_ok(fake, c, &["assign", &ns, "--to", &b.name]))
    };
    match stage {
        "backlog" | "ready" => {}
        "in-progress" => {
            run_ok(fake, b, &["take", &ns]);
        }
        "blocked" => {
            run_ok(fake, b, &["take", &ns]);
            run_ok(fake, b, &["block", &ns, "--why", "waits for x"]);
        }
        "asked" => {
            run_ok(fake, b, &["take", &ns]);
            run_ok(fake, b, &["ask", &ns, "--body", "Which way?"]);
        }
        other => panic!("stage {other}"),
    }
    Plain { n, ns, item }
}

fn crew() -> (FakeBoard, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    (fake, c, b)
}

#[test]
fn a_ticket_without_a_pr_is_closed_by_reason_from_every_status_but_done() {
    for stage in ["backlog", "ready", "in-progress", "blocked", "asked"] {
        let (fake, c, b) = crew();
        let t = plain(&fake, &c, &b, stage);
        let o = run_ok(
            &fake,
            &c,
            &["close", &t.ns, "--why", "not  needed\nany more"],
        );
        assert!(
            o.stdout.contains("closed: Status Done"),
            "{stage}: {}",
            o.all()
        );
        assert_eq!(
            fake.item_field(&t.item, "Status").as_deref(),
            Some("Done"),
            "{stage}"
        );
        assert!(
            !fake.labels_of(t.n).contains(&"needs-owner".to_string()),
            "{stage}: the label went"
        );
        // ONE note: to the assignee, else to the coordinator
        let notes = infos(&fake, t.n, "closed: not needed any more");
        assert_eq!(notes.len(), 1, "{stage}: {notes:?}");
        let to = if stage == "backlog" {
            "to=coordinator".to_string()
        } else {
            format!("to={}", b.name)
        };
        assert!(notes[0].contains(&to), "{stage}: {}", notes[0]);
        // a repeat is status-no-change and writes nothing
        refused_no_write(
            &fake,
            &c.dir,
            &["close", &t.ns, "--why", "again"],
            "status-no-change",
            "already Done",
        );
        assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
    }
}

#[test]
fn a_close_without_a_merge_and_without_a_reason_is_close_needs_reason() {
    let (fake, c, b) = crew();
    let t = plain(&fake, &c, &b, "blocked");
    refused_no_write(
        &fake,
        &c.dir,
        &["close", &t.ns],
        "close-needs-reason",
        "the issue is still open",
    );
    // the issue is closed, but no PR is named
    fake.close_issue(t.n);
    refused_no_write(
        &fake,
        &c.dir,
        &["close", &t.ns],
        "close-needs-reason",
        "no PR is named",
    );
    assert_eq!(
        fake.item_field(&t.item, "Status").as_deref(),
        Some("Blocked")
    );
}

#[test]
fn the_reason_must_have_text_and_pass_the_body_check() {
    let (fake, c, b) = crew();
    let t = plain(&fake, &c, &b, "blocked");
    for reason in ["", "   "] {
        refused_no_write(
            &fake,
            &c.dir,
            &["close", &t.ns, "--why", reason],
            "close-needs-reason",
            "the --why text is empty; give one non-empty line, or --verified after a passed verify",
        );
    }
    // a dev agent gets the writer rule first
    refused_no_write(
        &fake,
        &b.dir,
        &["close", &t.ns, "--why", ""],
        "status-wrong-writer",
        "coordinator action",
    );
    refused_no_write(
        &fake,
        &c.dir,
        &[
            "close",
            &t.ns,
            "--why",
            "key -----BEGIN PRIVATE KEY----- here",
        ],
        "secret-in-body",
        "",
    );
    assert_eq!(
        fake.item_field(&t.item, "Status").as_deref(),
        Some("Blocked")
    );
    run_ok(&fake, &c, &["close", &t.ns, "--why", "fine"]);
    assert_eq!(fake.item_field(&t.item, "Status").as_deref(), Some("Done"));
}

#[test]
fn only_the_coordinator_closes_by_reason() {
    let (fake, c, b) = crew();
    let t = plain(&fake, &c, &b, "in-progress");
    refused_no_write(
        &fake,
        &b.dir,
        &["close", &t.ns, "--why", "x"],
        "status-wrong-writer",
        "coordinator action",
    );
    assert_eq!(
        fake.item_field(&t.item, "Status").as_deref(),
        Some("In Progress")
    );
}

#[test]
fn a_ticket_that_is_done_is_final_for_reassign_and_ask() {
    let (fake, c, b) = crew();
    let t = plain(&fake, &c, &b, "in-progress");
    run_ok(&fake, &c, &["close", &t.ns, "--why", "not needed"]);
    assert_eq!(fake.item_field(&t.item, "Status").as_deref(), Some("Done"));
    refused_no_write(
        &fake,
        &c.dir,
        &["reassign", &t.ns, "--reason", "late", "--why", "try again"],
        "status-done-final",
        "only the owner moves it",
    );
    refused_no_write(
        &fake,
        &b.dir,
        &[
            "ask",
            &t.ns,
            "--body",
            "Is this still wanted?",
            "--override-unread",
        ],
        "status-done-final",
        "only the owner moves it",
    );
    assert_eq!(fake.item_field(&t.item, "Status").as_deref(), Some("Done"));
}

#[test]
fn assign_and_verify_on_a_done_ticket_are_refused() {
    let (fake, c, b) = crew();
    let t = plain(&fake, &c, &b, "in-progress");
    run_ok(&fake, &c, &["close", &t.ns, "--why", "not needed"]);
    refused_no_write(
        &fake,
        &c.dir,
        &["assign", &t.ns, "--to", &b.name],
        "status-done-final",
        "only the owner moves it",
    );
    refused_no_write(&fake, &c.dir, &["verify", &t.ns], "verify-needs-review", "");
    assert_eq!(fake.item_field(&t.item, "Status").as_deref(), Some("Done"));
}
