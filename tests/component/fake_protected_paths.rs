//! A PR that changes a protected path is merged by the owner only (rule
//! `protected-path-owner-merge`, spec/board#authority, spec/tickets#merge), against the fake
//! GitHub. `verify` and `merge` read the CURRENT file list of the PR; `done` records the
//! protected paths in a `protected-paths:` line. The headless `tick` is in `fake_headless.rs`.

use crate::support::authority::{AUTHORITY, authority_with};
use crate::support::board::{Agent, FakeBoard, Pull};
use crate::support::cli::{GATE, crew, no_unhandled, run_ok, run_refused, writes};

const RULE: &str = "protected-path-owner-merge";
const WORKFLOW: &str = ".github/workflows/ci.yml";

/// A `CI: none` ticket that is assigned, taken and delivered with the files that `pr_files`
/// names. The coordinator may merge (AUTHORITY.md: `coordinator_may_merge: true`, `needs_owner`
/// without `merge`).
fn delivered(
    fake: &FakeBoard,
    c: &Agent,
    b: &Agent,
    title: &str,
    pr_files: &[&str],
) -> (u64, Pull) {
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue(
        title,
        &format!("Goal: protected path test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &b.name]);
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, pr_files);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    (t, pr)
}

/// True if a request merged a PR.
fn merge_call(fake: &FakeBoard) -> bool {
    fake.requests()
        .iter()
        .any(|r| r.method == "PUT" && r.path.ends_with("/merge"))
}

/// The body of the latest `done` message that the coordinator sees.
fn done_body(fake: &FakeBoard, c: &Agent) -> String {
    let (v, _) = fake.poll_json(c);
    v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["class"] == "message" && e["kind"] == "done")
        .filter_map(|e| e["body"].as_str())
        .next_back()
        .unwrap_or_else(|| panic!("no done message in {v}"))
        .to_string()
}

#[test]
fn a_crew_pr_that_touches_a_workflow_file_is_refused_by_verify_and_by_merge() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT workflow", &["src/it.rs", WORKFLOW]);
    let ts = t.to_string();
    // (e) the `done` is accepted, and its message names the protected path
    let done = done_body(&fake, &c);
    assert!(
        done.lines()
            .any(|l| l == format!("protected-paths: {WORKFLOW}")),
        "{done}"
    );
    let before = writes(&fake);
    // verify: a refusal, not a failed verify; the rule names the path and the next command
    let v = run_refused(&fake, &c, &["verify", &ts], RULE);
    assert!(v.stderr.contains(WORKFLOW), "{}", v.stderr);
    assert!(
        v.stderr.contains(&format!(
            "crewboard close {}",
            format_args!("{}#{t}", crate::support::board::REPO)
        )),
        "{}",
        v.stderr
    );
    // merge: refused although the flag is true and `merge` is not in needs_owner
    run_refused(&fake, &c, &["merge", &ts], RULE);
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert!(!merge_call(&fake));
    assert!(!fake.pr_now(pr.number).0);
    assert_eq!(
        fake.item_field(&fake.item_of_issue(t).unwrap(), "Status")
            .as_deref(),
        Some("Review")
    );
    no_unhandled(&fake);
}

#[test]
fn verify_fail_is_allowed_on_a_protected_pr_but_a_pass_and_close_verified_are_refused() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT fail", &["src/it.rs", WORKFLOW]);
    let ts = t.to_string();
    let item = fake.item_of_issue(t).unwrap();
    // a pass is refused, nothing is written
    let before = writes(&fake);
    run_refused(&fake, &c, &["verify", &ts], RULE);
    assert_eq!(writes(&fake), before);
    // close --verified is refused too (no passed verify, and the PR is protected)
    run_refused(&fake, &c, &["close", &ts, "--verified"], RULE);
    assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Review"));
    // --fail is a normal failed verify: it moves the ticket back
    let o = run_ok(
        &fake,
        &c,
        &[
            "verify",
            &ts,
            "--fail",
            "--body",
            "the Gate check found a defect",
        ],
    );
    assert!(o.stdout.contains("verify FAIL"), "{}", o.stdout);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    assert!(!merge_call(&fake));
    assert!(!fake.pr_now(pr.number).0);
}

#[test]
fn the_file_list_is_read_when_verify_and_merge_run_not_when_done_ran() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT later change", &["src/it.rs"]);
    let ts = t.to_string();
    assert!(!done_body(&fake, &c).contains("protected-paths:"));
    // verify passes on the clean list
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    // the PR changes AUTHORITY.md after the verify (the head is the same in the fake)
    fake.pr_file(pr.number, "AUTHORITY.md");
    let before = writes(&fake);
    run_refused(&fake, &c, &["merge", &ts], RULE);
    run_refused(&fake, &c, &["verify", &ts], RULE);
    assert_eq!(writes(&fake), before);
    assert!(!merge_call(&fake));
}

#[test]
fn a_rename_out_of_a_protected_path_is_refused() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT rename", &["src/it.rs"]);
    fake.pr_rename(pr.number, "docs/old-authority.md", "AUTHORITY.md");
    let v = run_refused(&fake, &c, &["verify", &t.to_string()], RULE);
    assert!(v.stderr.contains("AUTHORITY.md"), "{}", v.stderr);
    run_refused(&fake, &c, &["merge", &t.to_string()], RULE);
    assert!(!merge_call(&fake));
}

#[test]
fn a_pr_with_more_than_1000_files_is_refused_and_1000_files_without_a_hit_pass() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT many files", &["src/it.rs"]);
    let many: Vec<String> = (0..1001).map(|i| format!("src/gen/f{i}.rs")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    fake.set_pr_files(pr.number, &refs);
    let v = run_refused(&fake, &c, &["verify", &t.to_string()], RULE);
    assert!(v.stderr.contains("more than 1000 files"), "{}", v.stderr);
    run_refused(&fake, &c, &["merge", &t.to_string()], RULE);
    assert!(!merge_call(&fake));
    // control: exactly 1000 files and no hit are not refused for their number
    fake.set_pr_files(pr.number, &refs[..1000]);
    let v = run_ok(&fake, &c, &["verify", &t.to_string()]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    // a protected file as file 1001 is seen (the list is read up to 1001)
    let mut at_end: Vec<&str> = refs[..1000].to_vec();
    at_end.push(".crewboard.toml");
    fake.set_pr_files(pr.number, &at_end);
    run_refused(&fake, &c, &["merge", &t.to_string()], RULE);
}

/// Like `delivered`, with `protected_paths: [tests/**, scripts/ci-*.sh]` in AUTHORITY.md.
fn delivered_with_extra(fake: &FakeBoard, c: &Agent, b: &Agent, files: &[&str]) -> (u64, Pull) {
    let (t, pr) = delivered(fake, c, b, "IT extra", &["src/it.rs"]);
    fake.file(
        "AUTHORITY.md",
        &authority_with(
            "fake-owner",
            "release",
            "protected_paths: [tests/**, scripts/ci-*.sh]\n",
        ),
    );
    fake.set_pr_files(pr.number, files);
    (t, pr)
}

#[test]
fn a_path_of_protected_paths_is_for_the_owner_like_a_built_in_path() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered_with_extra(&fake, &c, &b, &["src/it.rs", "scripts/ci-lint.sh"]);
    let ts = t.to_string();
    let before = writes(&fake);
    let v = run_refused(&fake, &c, &["verify", &ts], RULE);
    assert!(v.stderr.contains("scripts/ci-lint.sh"), "{}", v.stderr);
    assert!(
        v.stderr
            .contains("protected_paths in AUTHORITY.md (tests/**, scripts/ci-*.sh)"),
        "{}",
        v.stderr
    );
    run_refused(&fake, &c, &["merge", &ts], RULE);
    run_refused(&fake, &c, &["close", &ts, "--verified"], RULE);
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert!(!merge_call(&fake));
    assert!(!fake.pr_now(pr.number).0);
    // control: a PR outside the globs merges
    fake.set_pr_files(
        pr.number,
        &["src/it.rs", "scripts/other.sh", "src/tests/x.rs"],
    );
    run_ok(&fake, &c, &["verify", &ts]);
    let m = run_ok(&fake, &c, &["merge", &ts]);
    assert!(m.stdout.contains("merged"), "{}", m.stdout);
}

#[test]
fn done_records_a_path_of_protected_paths_in_the_protected_paths_line() {
    let (fake, c, b) = crew();
    fake.file(
        "AUTHORITY.md",
        &authority_with("fake-owner", "release", "protected_paths: [tests/**]\n"),
    );
    let t = fake.ticket_issue(
        "IT done line",
        &format!("Goal: x.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, &["tests/a.rs", "src/it.rs"]);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let done = done_body(&fake, &c);
    assert!(
        done.lines().any(|l| l == "protected-paths: tests/a.rs"),
        "{done}"
    );
}

#[test]
fn a_bad_protected_paths_entry_is_refused_and_named() {
    let (fake, c, b) = crew();
    let (t, _pr) = delivered(&fake, &c, &b, "IT bad entry", &["src/it.rs"]);
    fake.file(
        "AUTHORITY.md",
        &authority_with(
            "fake-owner",
            "release",
            "protected_paths: [tests/**, /etc/x]\n",
        ),
    );
    let v = run_refused(
        &fake,
        &c,
        &["verify", &t.to_string()],
        "authority-bad-front-matter",
    );
    assert!(v.stderr.contains("protected_paths"), "{}", v.stderr);
    assert!(v.stderr.contains("'/etc/x'"), "{}", v.stderr);
}

#[test]
fn a_pr_that_changes_only_other_files_merges() {
    let (fake, c, b) = crew();
    // names that only look protected
    let (t, pr) = delivered(
        &fake,
        &c,
        &b,
        "IT clean",
        &[
            "src/it.rs",
            "docs/AUTHORITY.md",
            "sub/.crewboard.toml",
            ".github/workflows-x/a.yml",
        ],
    );
    let ts = t.to_string();
    assert!(!done_body(&fake, &c).contains("protected-paths:"));
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    let m = run_ok(&fake, &c, &["merge", &ts]);
    assert!(m.stdout.contains("merged"), "{}", m.stdout);
    assert!(merge_call(&fake));
    assert!(fake.pr_now(pr.number).0);
}

#[test]
fn a_file_list_that_cannot_be_read_is_a_refusal_not_a_pass() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT unreadable", &["src/it.rs"]);
    // the files call fails (a 500 once)
    fake.fail_once("GET", &format!("pulls/{}/files", pr.number), "");
    let v = run_refused(&fake, &c, &["verify", &t.to_string()], RULE);
    assert!(v.stderr.contains("cannot be read"), "{}", v.stderr);
    assert!(!merge_call(&fake));
}

#[test]
fn a_codeowners_file_at_the_root_or_in_docs_is_for_the_owner() {
    for file in ["CODEOWNERS", "docs/CODEOWNERS", ".github/CODEOWNERS"] {
        let (fake, c, b) = crew();
        let (t, _pr) = delivered(&fake, &c, &b, "IT codeowners", &["src/it.rs", file]);
        assert!(
            done_body(&fake, &c).contains(&format!("protected-paths: {file}")),
            "{file}"
        );
        run_refused(&fake, &c, &["merge", &t.to_string()], RULE);
        assert!(!merge_call(&fake), "{file}");
    }
}

#[test]
fn done_with_a_cut_file_list_fails_closed_and_writes_nothing() {
    let (fake, c, b) = crew();
    let (t, pr) = delivered(&fake, &c, &b, "IT cut list", &["src/it.rs"]);
    // a second ticket whose PR lists 1001 files, the protected one is beyond the cut
    let t2 = fake.ticket_issue(
        "IT cut list 2",
        &format!("Goal: protected path test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t2.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr2 = fake.pr_for(t2, &b.name);
    let many: Vec<String> = (0..1500).map(|i| format!("src/gen/f{i}.rs")).collect();
    let mut refs: Vec<&str> = many.iter().map(String::as_str).collect();
    refs.push(".github/workflows/ci.yml");
    fake.set_pr_files(pr2.number, &refs);
    let _ = fake.poll_json(&b);
    let before = writes(&fake);
    let o = fake.run(&b.dir, &["done", &ts, "--pr", &pr2.url, "--sha", &pr2.head]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(RULE), "{}", o.all());
    assert!(o.stderr.contains("more than 1000 files"), "{}", o.all());
    assert_eq!(writes(&fake), before);
    let _ = (t, pr);
}
