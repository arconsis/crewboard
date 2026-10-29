//! A PR that was merged while the dev still works on it (spec/tickets#a-pr-merged-while-the-dev-works), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! (a) `done --pr <url>` on a merged PR is refused with `pr-already-merged` before any write.
//! (b) `tick` tells the assignee of an In Progress ticket, by ONE signed `info`, that the PR of
//! the ticket was merged. A second tick posts nothing.

use crate::support::board::{Agent, FakeBoard, Pull};
use crate::support::cli::{GATE, item_in, run_ok};
use crewboard::rules::Role;

const NOTE: &str = "do not push to its branch";

struct Case {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    t: u64,
    ts: String,
    item: String,
    pr: Pull,
}

/// A ticket the dev took, with an open PR. `delivered`: the dev ran `done`, and the coordinator
/// failed the verify, so the ticket is In Progress again with the PR in its Evidence field.
fn case(delivered: bool) -> Case {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue("IT 185", &body, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    if delivered {
        run_ok(
            &fake,
            &b,
            &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        );
        run_ok(
            &fake,
            &c,
            &["verify", &ts, "--fail", "--body", "Gate part 2 is missing"],
        );
        assert_eq!(
            fake.item_field(&item, "Status").as_deref(),
            Some("In Progress")
        );
        assert!(
            fake.item_field(&item, "Evidence")
                .is_some_and(|e| e.contains(&pr.url)),
            "control: the Evidence field keeps the PR"
        );
    }
    Case {
        fake,
        c,
        b,
        t,
        ts,
        item,
        pr,
    }
}

fn merge_sha(k: &Case) -> String {
    format!("{:040x}", 0xabc0_u128 + u128::from(k.pr.number))
}

fn headless(k: &Case) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&k.c, dir.path());
    dir
}

fn status(k: &Case) -> Option<String> {
    k.fake.item_field(&k.item, "Status")
}

/// The signed bodies on the ticket that hold the note.
fn told(k: &Case) -> Vec<String> {
    k.fake
        .comments_of(k.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains(NOTE))
        .collect()
}

fn pr_reads(k: &Case) -> usize {
    k.fake
        .requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&format!("/pulls/{}", k.pr.number)))
        .count()
}

#[test]
fn done_on_a_merged_pr_is_refused_before_any_write() {
    let k = case(false);
    k.fake.merge_pr(k.pr.number);
    let comments = k.fake.comments_of(k.t).len();
    let o = k.fake.run(
        &k.b.dir,
        &["done", &k.ts, "--pr", &k.pr.url, "--sha", &k.pr.head],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("pr-already-merged"), "{}", o.all());
    for text in [
        format!("PR {} was merged at {}", k.pr.url, merge_sha(&k)),
        "by fake-owner".to_string(),
        "push nothing more to its branch".to_string(),
        "open a new PR from the default branch".to_string(),
    ] {
        assert!(o.stderr.contains(&text), "{text}: {}", o.all());
    }
    // nothing was written: no done message, no Evidence, no Status change
    assert_eq!(k.fake.comments_of(k.t).len(), comments);
    assert_eq!(status(&k).as_deref(), Some("In Progress"));
    assert_eq!(k.fake.item_field(&k.item, "Evidence"), None);
    // a merge by someone else is refused the same way
    let k2 = case(false);
    k2.fake.merge_pr_by(k2.pr.number, "fake-crew");
    let o = k2.fake.run(
        &k2.b.dir,
        &["done", &k2.ts, "--pr", &k2.pr.url, "--sha", &k2.pr.head],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("pr-already-merged"), "{}", o.all());
    assert!(o.stderr.contains("by fake-crew"), "{}", o.all());
}

#[test]
fn done_with_a_new_pr_from_the_default_branch_passes() {
    let k = case(false);
    k.fake.merge_pr(k.pr.number);
    let branch = format!("crew/{}/{}-rest", k.b.name, k.t);
    let new = k
        .fake
        .pr_on_branch(&branch, &format!("Rest.\n\nCloses #{}", k.t));
    let o = run_ok(
        &k.fake,
        &k.b,
        &["done", &k.ts, "--pr", &new.url, "--sha", &new.head],
    );
    assert!(o.stdout.contains("done: Status Review"), "{}", o.all());
    assert_eq!(status(&k).as_deref(), Some("Review"));
    // control: an open PR is not refused (the same call shape as in every other test)
    let k = case(false);
    run_ok(
        &k.fake,
        &k.b,
        &["done", &k.ts, "--pr", &k.pr.url, "--sha", &k.pr.head],
    );
    assert_eq!(status(&k).as_deref(), Some("Review"));
}

#[test]
fn the_tick_tells_the_dev_once_that_its_pr_was_merged() {
    let k = case(true);
    let dir = headless(&k);
    // control: an open PR gets no note
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(told(&k).is_empty(), "{:?}", told(&k));

    k.fake.merge_pr(k.pr.number);
    let reads = pr_reads(&k);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    // one PR read for the ticket in this tick
    assert_eq!(pr_reads(&k) - reads, 1, "{}", o.all());
    let n = told(&k);
    assert_eq!(n.len(), 1, "{n:?}");
    let want = format!(
        "PR {} was merged by fake-owner at {}; {NOTE}; a remaining change needs a new PR from the default branch.",
        k.pr.url,
        merge_sha(&k)
    );
    assert!(n[0].contains(&want), "{want}: {}", n[0]);
    assert!(n[0].contains("kind=info"), "{}", n[0]);
    assert!(n[0].contains(&format!("to={}", k.b.name)), "{}", n[0]);
    assert!(o.stdout.contains(&k.ts), "{}", o.all());
    // the Status does not change: the tick tells, it judges nothing
    assert_eq!(status(&k).as_deref(), Some("In Progress"));

    // a second tick posts nothing and reads no PR
    let reads = pr_reads(&k);
    let comments = k.fake.comments_of(k.t).len();
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(told(&k).len(), 1);
    assert_eq!(k.fake.comments_of(k.t).len(), comments);
    assert_eq!(pr_reads(&k), reads, "a told PR is not read again");

    // a fresh config dir has no cache: it reads the PR, finds the note and posts nothing
    let dir2 = headless(&k);
    let o = k.fake.run(dir2.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(told(&k).len(), 1, "{}", o.all());
    assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
}

#[test]
fn a_merge_by_anyone_is_told_and_a_ticket_in_review_is_not() {
    // someone else merged
    let k = case(true);
    let dir = headless(&k);
    k.fake.merge_pr_by(k.pr.number, "fake-crew");
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let n = told(&k);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(n[0].contains("merged by fake-crew at"), "{}", n[0]);

    // a ticket in Review is not told (the Review paths handle it)
    let k = case(false);
    run_ok(
        &k.fake,
        &k.b,
        &["done", &k.ts, "--pr", &k.pr.url, "--sha", &k.pr.head],
    );
    let dir = headless(&k);
    k.fake.merge_pr(k.pr.number);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(told(&k).is_empty(), "{:?}", told(&k));

    // an In Progress ticket without a done has no PR in its Evidence field: no read, no note
    let k = case(false);
    let dir = headless(&k);
    k.fake.merge_pr(k.pr.number);
    let reads = pr_reads(&k);
    let o = k.fake.run(dir.path(), &["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(told(&k).is_empty(), "{:?}", told(&k));
    assert_eq!(pr_reads(&k), reads);
}
