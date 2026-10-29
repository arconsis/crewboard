//! `ci_ignore` of AUTHORITY.md (spec/ci#jobs-that-ci_ignore-skips), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! The case: a job is red for a reason outside the ticket. The team lists its name in
//! `ci_ignore` on the default branch. `done`, `verify` and `merge` do not count that job and
//! name it. An unlisted red job still blocks. At least one counted job must be green. A list on
//! the PR branch is never read.

use crate::support::authority::{NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Out, Pull};
use crewboard::rules::Role;

const BODY: &str = "Goal: integration test ticket.\n\nGate: ci\n";

fn authority(ci_ignore: &str) -> String {
    authority_with("fake-owner", NEEDS_OWNER_ALL, ci_ignore)
}

const LIST: &str = "ci_ignore:\n  - deploy-preview\n";

struct Case {
    ts: String,
    pr: Pull,
}

/// A taken ticket with a PR. The head has the push run 9001 (`run`) with the jobs `jobs`
/// (`(name, conclusion)`).
fn case(auth: &str, run: &str, jobs: &[(&str, &str)]) -> (FakeBoard, Agent, Agent, Case) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    fake.file("AUTHORITY.md", auth);
    let t = fake.ticket_issue("IT ci ignore", BODY, &["crew:work"]);
    let ts = t.to_string();
    let o = fake.run(&c.dir, &["assign", &ts, "--to", &b.name]);
    assert_eq!(o.code, 0, "{}", o.all());
    let o = fake.run(&b.dir, &["take", &ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9001, &pr.head, "push", run);
    for (i, (name, conclusion)) in jobs.iter().enumerate() {
        fake.check_run(100 + i as u64, 9001, &pr.head, name, conclusion);
    }
    let _ = fake.poll_json(&b);
    (fake, c, b, Case { ts, pr })
}

fn done(fake: &FakeBoard, b: &Agent, k: &Case) -> Out {
    fake.run(
        &b.dir,
        &["done", &k.ts, "--pr", &k.pr.url, "--sha", &k.pr.head],
    )
}

#[test]
fn a_red_listed_job_does_not_block_done_verify_and_merge_and_is_named() {
    let (fake, c, b, k) = case(
        &authority(LIST),
        "failure",
        &[("build", "success"), ("deploy-preview", "failure")],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("ignored (AUTHORITY.md ci_ignore): deploy-preview = failure"),
        "{}",
        o.all()
    );
    let o = fake.run(&c.dir, &["verify", &k.ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    // the signed result of verify names the ignored job too
    let issue: u64 = k.ts.parse().unwrap();
    assert!(
        fake.comments_of(issue).iter().any(|(_, _, body)| body
            .contains("ignored (AUTHORITY.md ci_ignore): deploy-preview = failure")
            && body.contains("verify")),
        "{:?}",
        fake.comments_of(issue)
    );
    let o = fake.run(&c.dir, &["merge", &k.ts]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("merged at head"), "{}", o.all());
}

#[test]
fn a_prefix_entry_ignores_the_jobs_with_that_prefix() {
    let (fake, _c, b, k) = case(
        &authority("ci_ignore: [\"deploy-*\"]\n"),
        "failure",
        &[
            ("build", "success"),
            ("deploy-preview", "failure"),
            ("deploy-docs", "failure"),
        ],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("deploy-preview = failure") && o.stdout.contains("deploy-docs = failure"),
        "{}",
        o.all()
    );
}

#[test]
fn a_red_unlisted_job_still_blocks_next_to_a_listed_one() {
    let (fake, _c, b, k) = case(
        &authority(LIST),
        "failure",
        &[
            ("build", "failure"),
            ("deploy-preview", "failure"),
            ("test", "success"),
        ],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-not-green"), "{}", o.all());
    assert!(o.stderr.contains("(build)"), "{}", o.all());
}

#[test]
fn without_a_list_nothing_is_ignored() {
    let (fake, _c, b, k) = case(
        &authority(""),
        "failure",
        &[("build", "success"), ("deploy-preview", "failure")],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-not-green"), "{}", o.all());
}

#[test]
fn when_every_job_is_ignored_the_ci_is_not_green() {
    let (fake, _c, b, k) = case(
        &authority(LIST),
        "failure",
        &[("deploy-preview", "failure")],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("every CI job is ignored"), "{}", o.all());
    // a green job that is ignored does not make the CI green either
    let (fake, _c, b, k) = case(
        &authority(LIST),
        "success",
        &[("deploy-preview", "success")],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 2, "{}", o.all());
}

#[test]
fn the_list_on_the_pr_branch_is_not_read() {
    let (fake, c, b, k) = case(
        &authority(""),
        "failure",
        &[("build", "success"), ("deploy-preview", "failure")],
    );
    // the PR branch changes AUTHORITY.md to ignore its own red job
    fake.branch_file_set(&k.pr.branch, "AUTHORITY.md", &authority(LIST));
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-not-green"), "{}", o.all());
    let o = fake.run(&c.dir, &["verify", &k.ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    // no read of AUTHORITY.md names a branch
    let reads: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.path.contains("/contents/AUTHORITY.md"))
        .collect();
    assert!(!reads.is_empty());
    assert!(reads.iter().all(|r| !r.path.contains("ref=")), "{reads:?}");
}
