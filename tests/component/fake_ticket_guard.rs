//! The guard of a ticket (spec/tickets#guarded-paths), against the fake GitHub: `Guard:` lines
//! of the body name paths that the change of the ticket must not touch. The first assignment
//! signs them as `guard:` lines, every later assignment copies the signed lines, and `done`, a
//! verify PASS, `close --verified` and `merge` refuse a PR that changes a guarded file (rule
//! `guarded-path-changed`). The readers take the signed lines, never the body. The headless
//! `tick` is in `fake_headless.rs`, and `--must-fail-at` in `fake_run_check.rs`.

use crate::support::authority::AUTHORITY;
use crate::support::board::{Agent, FakeBoard, Out, Pull, REPO};
use crate::support::cli::{GATE, crew, no_unhandled, run_ok, run_refused, writes};
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;

const RULE: &str = "guarded-path-changed";
const INVALID: &str = "ticket-guard-invalid";

/// A `CI: none` body with one `Guard:` line for each glob.
fn body(guard: &[&str]) -> String {
    let lines: String = guard.iter().map(|g| format!("Guard: {g}\n")).collect();
    format!("Goal: make the test pass.\n\n{GATE}CI: none\n{lines}")
}

/// The body of the latest assignment message of the ticket.
fn assignment(fake: &FakeBoard, issue: u64) -> String {
    fake.comments_of(issue)
        .into_iter()
        .map(|(_, _, b)| b)
        .rfind(|b| b.contains("Assigned to you"))
        .unwrap_or_else(|| panic!("no assignment message on #{issue}"))
}

/// The `guard:` lines of a message, in their order.
fn guard_lines(msg: &str) -> Vec<String> {
    msg.lines()
        .filter_map(|l| l.strip_prefix("guard: "))
        .map(str::to_string)
        .collect()
}

/// A ticket with the guard `tests/**` and `ci/check.sh`, assigned to `b` and taken, with an
/// open PR that changes `pr_files`.
fn taken(fake: &FakeBoard, c: &Agent, b: &Agent, pr_files: &[&str]) -> (u64, Pull) {
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue(
        "IT guard",
        &body(&["tests/**", "ci/check.sh"]),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &b.name]);
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, pr_files);
    let _ = fake.poll_json(b);
    (t, pr)
}

fn done(fake: &FakeBoard, b: &Agent, t: u64, pr: &Pull) -> Out {
    fake.run(
        &b.dir,
        &["done", &t.to_string(), "--pr", &pr.url, "--sha", &pr.head],
    )
}

fn status(fake: &FakeBoard, t: u64) -> Option<String> {
    fake.item_field(&fake.item_of_issue(t).unwrap(), "Status")
}

/// True if a request merged a PR.
fn merge_call(fake: &FakeBoard) -> bool {
    fake.requests()
        .iter()
        .any(|r| r.method == "PUT" && r.path.ends_with("/merge"))
}

/// The number of the ticket that a `ticket new` answer names.
fn created(o: &Out) -> u64 {
    o.stdout
        .split(&format!("{REPO}#"))
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{}", o.stdout))
}

/// The number of the new ticket that a `ticket split` answer names (`split: <old> -> <new>;`).
fn split_off(o: &Out) -> u64 {
    o.stdout
        .split(&format!("-> {REPO}#"))
        .nth(1)
        .and_then(|r| r.split(';').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{}", o.stdout))
}

#[test]
fn ticket_new_guard_writes_the_body_lines_and_the_assignment_signs_them() {
    let (fake, c, b) = crew();
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t.md");
    std::fs::write(&file, format!("Goal: make the test pass.\n\n{GATE}")).unwrap();
    let file = file.to_str().unwrap();
    let o = run_ok(
        &fake,
        &c,
        &[
            "ticket",
            "new",
            "--title",
            "Guarded",
            "--body-file",
            file,
            "--guard",
            "tests/**",
            "--guard",
            "ci/check.sh",
            "--assign",
            &b.name,
        ],
    );
    let t = created(&o);
    // the body has one Guard line per glob
    let (_, text) = fake.issue_text(t).unwrap();
    assert!(
        text.ends_with("\n\nGuard: tests/**\nGuard: ci/check.sh\n"),
        "{text}"
    );
    // the assignment signs them, after the gate and the ci line, one line per glob
    let msg = assignment(&fake, t);
    assert_eq!(guard_lines(&msg), ["tests/**", "ci/check.sh"], "{msg}");
    let at = |l: &str| msg.find(l).unwrap_or_else(|| panic!("no {l} in {msg}"));
    assert!(at("\ngate: ") < at("\nci: ") && at("\nci: ") < at("\nguard: tests/**"));
    assert!(at("\nguard: ci/check.sh") < at("\norder: "), "{msg}");
    // the answer names what was signed
    assert!(
        o.stdout
            .contains("; guard 'tests/**, ci/check.sh'; ci: required\n"),
        "{}",
        o.stdout
    );
    // a ticket without a guard has no guard line, and its answer names none
    let plain = fake.ticket_issue("IT plain", &body(&[]), &["crew:work"]);
    let o = run_ok(&fake, &c, &["assign", &plain.to_string(), "--to", &b.name]);
    assert!(guard_lines(&assignment(&fake, plain)).is_empty());
    assert!(!o.stdout.contains("guard"), "{}", o.stdout);
    no_unhandled(&fake);
}

#[test]
fn a_guard_that_is_not_valid_refuses_ticket_new_and_assign_and_nothing_is_written() {
    let (fake, c, b) = crew();
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain.md");
    std::fs::write(&plain, format!("Goal: x.\n\n{GATE}")).unwrap();
    let plain = plain.to_str().unwrap();
    let before = writes(&fake);
    // each limit, as a flag value
    let long = "a".repeat(201);
    for bad in ["/etc/passwd", "a/../b", "", "a\nguard: b", long.as_str()] {
        let o = fake.run(
            &c.dir,
            &[
                "ticket",
                "new",
                "--title",
                "T",
                "--body-file",
                plain,
                "--guard",
                bad,
            ],
        );
        assert_eq!(o.code, 2, "{bad:?}: {}", o.all());
        assert!(o.stderr.contains(INVALID), "{bad:?}: {}", o.all());
    }
    // more than 20 globs
    let many: Vec<String> = (0..21).map(|i| format!("f{i}")).collect();
    let mut args = vec!["ticket", "new", "--title", "T", "--body-file", plain];
    for g in &many {
        args.extend(["--guard", g.as_str()]);
    }
    let o = fake.run(&c.dir, &args);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains(INVALID) && o.stderr.contains("21 guard globs"),
        "{}",
        o.all()
    );
    // a Guard line of the body file is judged the same way
    let own = dir.path().join("own.md");
    std::fs::write(&own, format!("Goal: x.\n\n{GATE}Guard: ../outside\n")).unwrap();
    let o = fake.run(
        &c.dir,
        &[
            "ticket",
            "new",
            "--title",
            "T",
            "--body-file",
            own.to_str().unwrap(),
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(INVALID), "{}", o.all());
    assert!(fake.open_titles("crew:work").is_empty());
    assert_eq!(writes(&fake), before, "a refusal wrote something");

    // `assign` of an issue that a person wrote: refused before the first write
    let t = fake.ticket_issue("IT bad guard", &body(&["/abs/path"]), &["crew:work"]);
    let before = writes(&fake);
    let o = run_refused(
        &fake,
        &c,
        &["assign", &t.to_string(), "--to", &b.name],
        INVALID,
    );
    assert!(o.stderr.contains("'/abs/path'"), "{}", o.stderr);
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert!(fake.item_of_issue(t).is_none());
    // control: the same issue with a valid guard is assigned
    fake.set_issue_body(t, &body(&["abs/path"]));
    run_ok(&fake, &c, &["assign", &t.to_string(), "--to", &b.name]);
    assert_eq!(guard_lines(&assignment(&fake, t)), ["abs/path"]);
    no_unhandled(&fake);
}

#[test]
fn every_later_assignment_copies_the_signed_guard_and_warns_about_a_body_edit() {
    let (fake, c, b) = crew();
    let d = fake.agent("claude@fake-d", Role::Dev);
    fake.file("AUTHORITY.md", AUTHORITY);
    let signed = ["tests/**", "ci/check.sh"];
    let t = fake.ticket_issue("IT copy", &body(&signed), &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    assert_eq!(guard_lines(&assignment(&fake, t)), signed);
    const WARNING: &str = "warning: the Guard lines in the body of";
    const STAYS: &str = "The signed guard stays: 'tests/**, ci/check.sh'";

    // the move of a queued ticket, with the body as it was: a copy, and no warning
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &d.name]);
    assert_eq!(guard_lines(&assignment(&fake, t)), signed);
    assert!(!o.stdout.contains(WARNING), "{}", o.stdout);

    // a holder of an agent token removes one Guard line from the body
    fake.set_issue_body(t, &body(&["ci/check.sh"]));
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    assert_eq!(guard_lines(&assignment(&fake, t)), signed);
    assert!(
        o.stdout.contains(WARNING) && o.stdout.contains(STAYS),
        "{}",
        o.stdout
    );

    // the body loses every Guard line: `reassign --to` keeps the signed guard
    fake.set_issue_body(t, &body(&[]));
    let o = run_ok(
        &fake,
        &c,
        &[
            "reassign",
            &ts,
            "--to",
            &d.name,
            "--reason",
            "late",
            "--why",
            "it: again",
        ],
    );
    assert_eq!(guard_lines(&assignment(&fake, t)), signed);
    assert!(
        o.stdout.contains(WARNING) && o.stdout.contains(STAYS),
        "{}",
        o.stdout
    );

    // the body gets a glob that covers nothing: `assign` after a `reassign` keeps the signed one
    fake.set_issue_body(t, &body(&["nothing/**"]));
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "late", "--why", "it: back"],
    );
    // `--order 7`: without it this message has the bytes of the first assignment, and inside
    // one second the replay rule drops the second of two equal messages
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &b.name, "--order", "7"]);
    let msg = assignment(&fake, t);
    assert_eq!(guard_lines(&msg), signed, "{msg}");
    assert!(
        o.stdout.contains(WARNING) && o.stdout.contains(STAYS),
        "{}",
        o.stdout
    );

    // and the signed guard is what `done` reads: the edited body lets nothing through
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, &["src/it.rs", "tests/a.rs"]);
    let _ = fake.poll_json(&b);
    let o = done(&fake, &b, t, &pr);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(RULE), "{}", o.all());
    no_unhandled(&fake);
}

#[test]
fn done_is_refused_for_a_changed_added_deleted_or_renamed_guarded_file() {
    let (fake, c, b) = crew();
    let (t, pr) = taken(&fake, &c, &b, &["src/it.rs"]);
    let before = writes(&fake);
    // the file list of GitHub names a modified, an added and a deleted file by its path
    for files in [
        vec!["src/it.rs", "tests/a.rs"],
        vec!["tests/deep/new_case.rs"],
        vec!["ci/check.sh", "src/it.rs"],
        vec!["CI/Check.sh"],
    ] {
        fake.set_pr_files(pr.number, &files);
        let o = done(&fake, &b, t, &pr);
        assert_eq!(o.code, 2, "{files:?}: {}", o.all());
        assert!(o.stderr.contains(RULE), "{files:?}: {}", o.all());
    }
    // a rename out of the guard (the old name) and into it (the new name)
    for (new, old) in [
        ("src/moved.rs", "tests/a.rs"),
        ("tests/moved.rs", "src/a.rs"),
    ] {
        fake.set_pr_files(pr.number, &["src/it.rs"]);
        fake.pr_rename(pr.number, new, old);
        let o = done(&fake, &b, t, &pr);
        assert_eq!(o.code, 2, "{new} from {old}: {}", o.all());
        assert!(o.stderr.contains(RULE), "{new} from {old}: {}", o.all());
    }
    // the text names the file, its glob and the way out, and no file outside the guard
    fake.set_pr_files(pr.number, &["src/it.rs", "tests/a.rs", "ci/check.sh"]);
    let o = done(&fake, &b, t, &pr);
    assert!(
        o.stderr
            .contains("changes tests/a.rs (guard: tests/**), ci/check.sh (guard: ci/check.sh):"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains("No flag overrides a guard")
            && o.stderr
                .contains("the owner changes such a file in a separate change")
            && o.stderr.contains("a new ticket without that guard"),
        "{}",
        o.stderr
    );
    assert!(!o.stderr.contains("src/it.rs"), "{}", o.stderr);
    // nothing was written: no message, no field, no Status
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));

    // control: the same PR without a guarded file is delivered
    fake.set_pr_files(
        pr.number,
        &[
            "src/it.rs",
            "src/tests/x.rs",
            "ci/other.sh",
            "testsuite/a.rs",
        ],
    );
    let o = done(&fake, &b, t, &pr);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(status(&fake, t).as_deref(), Some("Review"));
    no_unhandled(&fake);
}

#[test]
fn a_verify_pass_close_verified_and_merge_are_refused_and_a_verify_fail_is_not() {
    let (fake, c, b) = crew();
    let (t, pr) = taken(&fake, &c, &b, &["src/it.rs"]);
    let ts = t.to_string();
    assert_eq!(done(&fake, &b, t, &pr).code, 0);
    // the PR gets a guarded file after the `done` (the head is the same in the fake)
    fake.pr_file(pr.number, "tests/a.rs");
    let before = writes(&fake);
    let v = run_refused(&fake, &c, &["verify", &ts], RULE);
    assert!(
        v.stderr.contains("tests/a.rs (guard: tests/**)"),
        "{}",
        v.stderr
    );
    run_refused(&fake, &c, &["merge", &ts], RULE);
    run_refused(&fake, &c, &["close", &ts, "--verified"], RULE);
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert!(!merge_call(&fake));
    assert_eq!(status(&fake, t).as_deref(), Some("Review"));

    // a pass on the clean list, then a guarded file is renamed away: merge stays refused
    fake.set_pr_files(pr.number, &["src/it.rs"]);
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    fake.pr_rename(pr.number, "src/moved.rs", "ci/check.sh");
    let before = writes(&fake);
    let m = run_refused(&fake, &c, &["merge", &ts], RULE);
    assert!(
        m.stderr.contains("ci/check.sh (guard: ci/check.sh)"),
        "{}",
        m.stderr
    );
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert!(!merge_call(&fake));
    assert!(!fake.pr_now(pr.number).0);

    // a failed verify stays possible: it sends the ticket back
    let o = run_ok(
        &fake,
        &c,
        &["verify", &ts, "--fail", "--body", "the PR changes the test"],
    );
    assert!(o.stdout.contains("verify FAIL"), "{}", o.stdout);
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
    no_unhandled(&fake);
}

/// A PR that somebody merged by hand after a passed verify: `close --verified` reads the files
/// of the PR again and does not close the ticket as verified.
#[test]
fn close_verified_is_refused_for_a_merged_pr_that_changes_a_guarded_file() {
    let (fake, c, b) = crew();
    let (t, pr) = taken(&fake, &c, &b, &["src/it.rs"]);
    let ts = t.to_string();
    assert_eq!(done(&fake, &b, t, &pr).code, 0);
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    fake.pr_file(pr.number, "tests/a.rs");
    fake.merge_pr(pr.number);
    let before = writes(&fake);
    run_refused(&fake, &c, &["close", &ts, "--verified"], RULE);
    assert_eq!(writes(&fake), before, "a refusal wrote something");
    assert_eq!(status(&fake, t).as_deref(), Some("Review"));

    // control: the same steps without the guarded file close the ticket
    let (t2, pr2) = taken(&fake, &c, &b, &["src/it.rs"]);
    let ts2 = t2.to_string();
    assert_eq!(done(&fake, &b, t2, &pr2).code, 0);
    run_ok(&fake, &c, &["verify", &ts2]);
    fake.merge_pr(pr2.number);
    run_ok(&fake, &c, &["close", &ts2, "--verified"]);
    assert_eq!(status(&fake, t2).as_deref(), Some("Done"));
}

#[test]
fn a_pr_that_stays_off_the_guard_is_verified_and_merged() {
    let (fake, c, b) = crew();
    let (t, pr) = taken(&fake, &c, &b, &["src/it.rs", "docs/tests/readme.md"]);
    let ts = t.to_string();
    assert_eq!(done(&fake, &b, t, &pr).code, 0);
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);
    let m = run_ok(&fake, &c, &["merge", &ts]);
    assert!(m.stdout.contains("merged"), "{}", m.stdout);
    assert!(merge_call(&fake));
    no_unhandled(&fake);
}

/// The body is not signed. A `Guard:` line that the body got after the assignment guards
/// nothing, and a ticket whose assignment has no `guard:` line works as it did.
#[test]
fn a_guard_line_that_is_only_in_the_body_is_not_read() {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue("IT no guard", &body(&[]), &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    fake.set_issue_body(t, &body(&["tests/**"]));
    let pr = fake.pr_for(t, &b.name);
    fake.set_pr_files(pr.number, &["tests/a.rs"]);
    let _ = fake.poll_json(&b);
    assert_eq!(done(&fake, &b, t, &pr).code, 0);
    let v = run_ok(&fake, &c, &["verify", &ts]);
    assert!(v.stdout.contains("verify PASS"), "{}", v.stdout);

    // a `guard:` line of the dev agent is no signed guard either
    let t2 = fake.ticket_issue("IT dev line", &body(&[]), &["crew:work"]);
    let ts2 = t2.to_string();
    run_ok(&fake, &c, &["assign", &ts2, "--to", &b.name]);
    fake.signed(
        &b.key,
        &b.name,
        &c.name,
        Kind::Request,
        t2,
        "Assigned to you: x\ngate: ci\nguard: tests/**\n",
    );
    run_ok(&fake, &b, &["take", &ts2]);
    let pr2 = fake.pr_for(t2, &b.name);
    fake.set_pr_files(pr2.number, &["tests/a.rs"]);
    let _ = fake.poll_json(&b);
    assert_eq!(done(&fake, &b, t2, &pr2).code, 0);
}

#[test]
fn a_split_hands_the_signed_guard_to_the_new_ticket_unless_the_file_has_its_own() {
    let (fake, c, b) = crew();
    let (t, _pr) = taken(&fake, &c, &b, &["src/it.rs"]);
    let ts = t.to_string();
    // the body of the old ticket loses its Guard lines: the split takes the signed guard
    fake.set_issue_body(t, &body(&[]));
    let dir = tempfile::tempdir().unwrap();
    let open = dir.path().join("open.md");
    std::fs::write(&open, "The second case is still open.\n").unwrap();
    let o = run_ok(
        &fake,
        &c,
        &[
            "ticket",
            "split",
            &ts,
            "--title",
            "Second case",
            "--body-file",
            open.to_str().unwrap(),
        ],
    );
    let new = split_off(&o);
    let (_, text) = fake.issue_text(new).unwrap();
    assert!(
        text.contains("\nGuard: tests/**\nGuard: ci/check.sh\n"),
        "{text}"
    );
    assert_eq!(
        guard_lines(&assignment(&fake, new)),
        ["tests/**", "ci/check.sh"]
    );

    // a file with its own Guard lines keeps them
    let own = dir.path().join("own.md");
    std::fs::write(&own, "The third case.\nGuard: golden/\n").unwrap();
    let o = run_ok(
        &fake,
        &c,
        &[
            "ticket",
            "split",
            &ts,
            "--title",
            "Third case",
            "--body-file",
            own.to_str().unwrap(),
        ],
    );
    assert_eq!(guard_lines(&assignment(&fake, split_off(&o))), ["golden/"]);
    no_unhandled(&fake);
}
