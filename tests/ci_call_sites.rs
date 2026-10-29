//! The commands that judge CI call `fetch_ci` once each and pass the named runs on.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use std::fs;
use std::path::Path;

fn source(rel: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)).unwrap()
}

#[test]
fn done_verify_and_merge_all_read_the_named_runs_and_judge_them_like_the_default() {
    // the ticket commands of `src/ops/tickets/`, without the files that have their own checks
    let tickets: String = [
        "mod",
        "assign",
        "block",
        "close",
        "done",
        "in_progress",
        "local_gate",
        "pr_evidence",
        "reassign",
        "take",
        "verify",
    ]
    .iter()
    .map(|f| source(&format!("src/ops/tickets/{f}.rs")))
    .collect();
    let merge = source("src/ops/tickets/merge.rs");
    // done: the `--run` values are parsed before the PR is read, then passed to the fetch
    assert_eq!(
        tickets
            .matches("named_runs(ac, &l.pr_url.repo, a.runs)")
            .count(),
        1
    );
    assert_eq!(tickets.matches("fetch_ci(").count(), 2); // done and verify
    // verify and merge (and tick, which runs both): ONE helper adds the named runs of the
    // evidence to `--run`, and ONE fetch judges them
    assert_eq!(
        tickets
            .matches("evidence_runs(ac, &pr_url.repo, &ev, va.runs)")
            .count(),
        1
    );
    assert!(merge.contains("super::pr_evidence::evidence_runs(ac, &pr_url.repo, &ev, runs)"));
    assert!(merge.contains("fetch_ci("));
    // verify words the information-only case as a failure, like done
    assert!(tickets.contains("CiFacts::OnlyInformation { info, .. } => out.failures.push"));
}
