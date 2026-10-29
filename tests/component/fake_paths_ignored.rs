//! `done`, `verify` and `merge` for a PR whose files every workflow ignores (spec/ci#files-that-every-workflow-ignores), against the fake GitHub of
//! `tests/support/board.rs`.
//!
//! The fake has no CI run for any commit. A ticket without `CI: none` is accepted only when the
//! workflows show that no run can exist: every workflow that runs for the PR ignores every
//! changed file, no required check waits, and the reader understood every workflow.

use crate::support::board::{Agent, FakeBoard, Out, REPO};
use crate::support::cli::{crew, item_in, run_ok};
use serde_json::json;

/// A CI-only Gate, so that the headless merge reaches its evidence check.
const BODY: &str = "Goal: integration test ticket.\n\nGate: ci\n";

/// The common form: push to crew branches and to main, docs paths ignored.
const CI_PATHS_IGNORE: &str = "name: ci\non:\n  push:\n    branches: [main, 'crew/**']\n    paths-ignore:\n      - 'docs/**'\n      - README.md\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo test\n";

const EVIDENCE: &str = "ci: none (all files paths-ignored: docs/**, README.md)";

fn workflow(fake: &FakeBoard, name: &str, yaml: &str) {
    fake.set_file(REPO, &format!(".github/workflows/{name}"), yaml);
}

struct Case {
    t: u64,
    item: String,
    pr: crate::support::board::Pull,
}

/// A ticket assigned to B and taken; its PR changes `files`.
fn case(fake: &FakeBoard, c: &Agent, b: &Agent, files: &[&str]) -> Case {
    let t = fake.ticket_issue("IT paths-ignored", BODY, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, files);
    let _ = fake.poll_json(b);
    Case { t, item, pr }
}

fn done(fake: &FakeBoard, b: &Agent, k: &Case) -> Out {
    fake.run(
        &b.dir,
        &[
            "done",
            &k.t.to_string(),
            "--pr",
            &k.pr.url,
            "--sha",
            &k.pr.head,
        ],
    )
}

/// `done` is refused with `evidence-ci-missing`, the reason contains `why`, and nothing moved.
fn refused_with(fake: &FakeBoard, b: &Agent, k: &Case, why: &str) -> Out {
    let o = done(fake, b, k);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-ci-missing"), "{}", o.all());
    assert!(
        o.stderr.contains("Not accepted as paths-ignored") && o.stderr.contains(why),
        "want '{why}': {}",
        o.all()
    );
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("In Progress")
    );
    o
}

fn gets(fake: &FakeBoard, part: &str) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(part))
        .count()
}

#[test]
fn docs_only_files_that_paths_ignore_skips_are_accepted_with_the_evidence_line() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    let k = case(
        &fake,
        &c,
        &b,
        &["docs/guide.md", "README.md", "docs/a/b.md"],
    );
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("Status Review"), "{}", o.stdout);
    assert!(o.stdout.contains(EVIDENCE), "{}", o.stdout);
    assert_eq!(
        fake.item_field(&k.item, "Status").as_deref(),
        Some("Review")
    );
    let ev = fake.item_field(&k.item, "Evidence").unwrap_or_default();
    assert!(
        ev.contains("ci=none (all files paths-ignored: docs/**, README.md)"),
        "{ev}"
    );
    // the signed done message carries the line
    assert!(
        fake.comments_of(k.t)
            .iter()
            .any(|(_, _, body)| body.contains("kind=done") && body.contains(EVIDENCE)),
        "no done message with the evidence line"
    );
    // the reads of the judgement: the workflow directory, one workflow file, the branch and
    // its rules, each once
    assert_eq!(gets(&fake, "/contents/.github/workflows"), 2);
    let branch_reads = fake
        .requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path == format!("/repos/{REPO}/branches/main"))
        .count();
    assert_eq!(branch_reads, 1);
    assert_eq!(gets(&fake, "/rules/branches/main"), 1);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn one_file_that_is_not_ignored_is_refused_naming_the_file_and_the_workflow() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    let k = case(&fake, &c, &b, &["docs/guide.md", "src/lib.rs"]);
    refused_with(
        &fake,
        &b,
        &k,
        "file src/lib.rs is not ignored by workflow ci.yml (push does not ignore it)",
    );
    // control: without the code file the same PR is accepted
    fake.set_pr_files(k.pr.number, &["docs/guide.md"]);
    let o = done(&fake, &b, &k);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("ci: none (all files paths-ignored: docs/**)"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_workflow_that_cannot_be_read_is_refused() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    // a flow list over two lines: the reader does not know this form
    workflow(
        &fake,
        "lint.yml",
        "on:\n  push:\n    paths-ignore: ['docs/**',\n      'README.md']\n",
    );
    let k = case(&fake, &c, &b, &["docs/guide.md"]);
    refused_with(&fake, &b, &k, "workflow lint.yml cannot be read");
    // a read error of a workflow file is a refusal too, with the reason
    workflow(&fake, "lint.yml", CI_PATHS_IGNORE);
    fake.fail_file(REPO, ".github/workflows/lint.yml", 403);
    refused_with(
        &fake,
        &b,
        &k,
        "the workflows of fake-owner/fake-repo cannot be read",
    );
    // control: a readable second workflow of the same form is accepted
    workflow(&fake, "lint.yml", CI_PATHS_IGNORE);
    assert_eq!(done(&fake, &b, &k).code, 0);
}

#[test]
fn required_status_checks_are_refused_and_named() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    let k = case(&fake, &c, &b, &["docs/guide.md"]);
    // classic branch protection, readable through the branch answer
    fake.set_branch_answer(json!({"name": "main", "protected": true, "protection": {
        "enabled": true, "required_status_checks": {"enforcement_level": "non_admins",
        "contexts": ["ci / test"], "checks": [{"context": "ci / test", "app_id": 15368}]}}}));
    refused_with(
        &fake,
        &b,
        &k,
        "branch main requires the status checks ci / test",
    );
    // a ruleset with required status checks
    fake.set_branch_answer(json!({"name": "main", "protected": false}));
    fake.set_rules_answer(
        200,
        json!([{"type": "required_status_checks", "ruleset_source_type": "Repository",
                "ruleset_id": 1, "parameters": {"required_status_checks": [{"context": "ci"}]}}]),
    );
    refused_with(
        &fake,
        &b,
        &k,
        "a ruleset of branch main has the rule required_status_checks",
    );
    // rules that cannot be read
    fake.set_rules_answer(404, json!({"message": "Not Found"}));
    refused_with(&fake, &b, &k, "the rules of branch main cannot be read");
    // control: a plan without rulesets (GitHub's 403 text) and no required checks pass
    fake.set_rules_answer(
        403,
        json!({"message": "Upgrade to GitHub Pro or make this repository public to enable this feature."}),
    );
    assert_eq!(done(&fake, &b, &k).code, 0);
}

#[test]
fn a_pull_request_workflow_without_filters_is_a_real_missing_ci() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    workflow(
        &fake,
        "pr.yml",
        "on:\n  pull_request:\n    branches: [main]\n",
    );
    let k = case(&fake, &c, &b, &["docs/guide.md"]);
    refused_with(
        &fake,
        &b,
        &k,
        "file docs/guide.md is not ignored by workflow pr.yml (pull_request has no paths filter)",
    );
    // control: the same workflow for another base branch does not run for this PR
    workflow(
        &fake,
        "pr.yml",
        "on:\n  pull_request:\n    branches: [develop]\n",
    );
    assert_eq!(done(&fake, &b, &k).code, 0);
}

#[test]
fn verify_gives_the_same_judgement_and_shows_it_in_its_result() {
    let (fake, c, b) = crew();
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    let k = case(&fake, &c, &b, &["docs/guide.md", "README.md"]);
    assert_eq!(done(&fake, &b, &k).code, 0);
    let ts = k.t.to_string();
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let result_has = |text: &str| {
        fake.comments_of(k.t)
            .iter()
            .any(|(_, _, body)| body.contains("verify: ") && body.contains(text))
    };
    assert!(
        result_has(EVIDENCE),
        "the verify result does not show the line"
    );
    // the owner adds a pull_request workflow: a run should exist now, so verify fails
    let t2 = case(&fake, &c, &b, &["docs/other.md"]);
    assert_eq!(done(&fake, &b, &t2).code, 0);
    workflow(&fake, "pr.yml", "on: pull_request\n");
    let o = fake.run(&c.dir, &["verify", &t2.t.to_string()]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    assert!(
        o.stdout.contains("Not accepted as paths-ignored")
            && o.stdout.contains("not ignored by workflow pr.yml"),
        "{}",
        o.all()
    );
}

#[test]
fn the_headless_merge_refuses_a_paths_ignored_ticket_and_the_interactive_one_merges() {
    let (fake, c, b) = crew();
    fake.file(
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n",
    );
    workflow(&fake, "ci.yml", CI_PATHS_IGNORE);
    let k = case(&fake, &c, &b, &["docs/guide.md"]);
    assert_eq!(done(&fake, &b, &k).code, 0);
    let ts = k.t.to_string();
    assert!(
        run_ok(&fake, &c, &["verify", &ts])
            .stdout
            .contains("verify PASS")
    );
    let headless = tempfile::tempdir().unwrap();
    crate::support::headless::copy_headless(&c, headless.path());
    let reads = fake.requests().len();
    let o = fake.run(headless.path(), &["merge", &ts]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("[rule=headless-scope]"), "{}", o.all());
    assert!(
        o.stderr
            .contains("the evidence is 'ci: none (all files paths-ignored: docs/**)'")
            && o.stderr.contains("not CI only"),
        "{}",
        o.all()
    );
    assert!(!fake.pr_now(k.pr.number).0);
    // the refusal comes before any read of the PR or the CI
    assert!(
        fake.requests()[reads..]
            .iter()
            .all(|r| !r.path.contains("/pulls/") && !r.path.contains("/actions/runs")),
        "{:?}",
        &fake.requests()[reads..]
    );
    // the headless tick leaves it with its own reason, before it tries the merge
    let o = fake.run(headless.path(), &["tick"]);
    assert!(
        o.stdout
            .contains("no CI run proves the change, so the Gate is not CI only")
            && !o.stdout.contains("merge refused"),
        "{}",
        o.all()
    );
    assert!(!fake.pr_now(k.pr.number).0);
    // control: the interactive coordinator merges it (the `done` judgement runs again)
    let o = run_ok(&fake, &c, &["merge", &ts]);
    assert!(o.stdout.contains("merged at head"), "{}", o.stdout);
    assert!(fake.pr_now(k.pr.number).0);
}
