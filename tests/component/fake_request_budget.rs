//! The number of GitHub reads of `done`, `verify`, `tick` and `init` against the fake GitHub.
//! A change in the code that reads CI evidence or creates the project objects must not add or
//! drop a read. A test fails when the count changes; the new count goes into the table with a
//! reason.
//!
//! A read is a GET or a GraphQL call without a mutation.

use crate::support::Req;
use crate::support::authority::AUTHORITY;
use crate::support::board::{FakeBoard, REPO};
use crate::support::cli::{crew, run_ok};
use crate::support::headless::{Hub, Scenario};

fn is_read(r: &Req) -> bool {
    r.method == "GET" || (r.path == "/graphql" && !r.body.contains("mutation"))
}

fn reads_since(requests: Vec<Req>, from: usize) -> usize {
    requests[from..].iter().filter(|r| is_read(r)).count()
}

/// The reads of the CI evidence of one commit: runs, check runs, both status calls and the
/// branch protection.
fn ci_reads(requests: &[Req]) -> usize {
    requests
        .iter()
        .filter(|r| {
            is_read(r)
                && (r.path.contains("/actions/runs")
                    || r.path.contains("/check-runs")
                    || r.path.contains("/status")
                    || r.path.contains("/protection/"))
        })
        .count()
}

/// `DONE`, `VERIFY` and `TICK` are `(all reads, CI reads)` of that command for a ticket whose PR
/// head has a green push run. `INIT` is the number of reads of `init`.
// The context reads AUTHORITY.md once (for `former_repos`) and hands the text to every later
// read of the default-branch file, so these commands read it as often as before.
const DONE: (usize, usize) = (22, 5); // +1 compare read (head behind base), +1 Status read again before the write
const VERIFY: (usize, usize) = (16, 5);
const TICK: (usize, usize) = (41, 10); // +1: Status read again before the write
const INIT: usize = 34; // +4: the option writer reads the list again before each of its 4 writes; +3: the branch rule looks for a rule (branch, rules, rulesets)

#[test]
fn done_and_verify_read_what_they_read_before() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue(
        "IT budget",
        "Goal: integration test ticket.\n\nGate: ci\n",
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9001, &pr.head, "push", "success");
    let _ = fake.poll_json(&b);

    let from = fake.requests().len();
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let after_done = fake.requests();
    let done = (
        reads_since(fake.requests(), from),
        ci_reads(&after_done[from..]),
    );

    let from = fake.requests().len();
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let after_verify = fake.requests();
    let verify = (
        reads_since(fake.requests(), from),
        ci_reads(&after_verify[from..]),
    );
    assert_eq!(done, DONE, "done: (reads, CI reads)");
    assert_eq!(verify, VERIFY, "verify: (reads, CI reads)");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn tick_reads_what_it_read_before() {
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: true,
    });
    let from = h.requests().len();
    let o = h.headless(&["tick"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("merged"), "{}", o.all());
    let all = h.requests();
    let tick = (reads_since(h.requests(), from), ci_reads(&all[from..]));
    assert_eq!(tick, TICK, "tick: (reads, CI reads)");
}

#[test]
fn init_reads_what_it_read_before() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(
        owner.path(),
        &[
            "init",
            "--initiative",
            "fake",
            "--repo",
            REPO,
            "--project",
            "15",
            "--insecure-one-account",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(reads_since(fake.requests(), 0), INIT, "init: reads");
}
