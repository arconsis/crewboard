//! The default branch of the main repo: `start` and `doctor --board` warn when nothing protects
//! it against a direct push of the agent account, and say `unknown` when the token cannot read
//! the answer (`domain::branch_guard`, spec/board#authority), against the fake GitHub.

use crate::support::board::{Agent, FakeBoard, Out};
use crate::support::cli::{crew, run_ok};
use serde_json::json;

const WARNING: &str = "warning: the default branch main of fake-owner/fake-repo is not protected: the agent account can push to it and change AUTHORITY.md or .crewboard.toml without a pull request. Fix: require a pull request for the branch, with owner or CODEOWNERS review for AUTHORITY.md and .crewboard.toml (docs/guides/security.md)";

fn doctor_board(fake: &FakeBoard) -> Out {
    let dir = fake.owner_dir();
    let o = fake.run(dir.path(), &["doctor", "--board"]);
    assert_eq!(o.code, 0, "{}", o.all());
    o
}

fn start(fake: &FakeBoard, who: &Agent) -> Out {
    // `resume` runs the checks of `start` for an agent that is active already
    run_ok(fake, who, &["resume"])
}

fn lines_with<'a>(o: &'a Out, needle: &str) -> Vec<&'a str> {
    o.stdout.lines().filter(|l| l.contains(needle)).collect()
}

fn branch_reads(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| {
            r.method == "GET" && r.path.ends_with("/branches/main") && !r.path.contains("/rules/")
        })
        .count()
}

#[test]
fn an_unprotected_default_branch_gives_one_warning_line_with_the_fix() {
    let (fake, c, _b) = crew();
    // the fake answers `protected: false` unless a test says otherwise
    let o = doctor_board(&fake);
    assert_eq!(lines_with(&o, "warning: the default branch"), [WARNING]);
    let s = start(&fake, &c);
    assert_eq!(lines_with(&s, "warning: the default branch"), [WARNING]);
    // the check wrote nothing and made no unknown request
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_protected_default_branch_gives_no_warning_and_says_what_it_cannot_read() {
    let (fake, c, _b) = crew();
    // a ruleset only: `protected` is true and the classic `protection.enabled` is false
    fake.set_branch_answer(json!({"name": "main", "protected": true,
        "protection": {"enabled": false, "required_status_checks":
            {"enforcement_level": "off", "contexts": [], "checks": []}}}));
    for o in [doctor_board(&fake), start(&fake, &c)] {
        assert!(lines_with(&o, "warning: the default branch").is_empty());
        let l = lines_with(
            &o,
            "default branch: main of fake-owner/fake-repo is protected",
        );
        assert_eq!(l.len(), 1, "{}", o.all());
        assert!(
            l[0].contains("whether its rule requires a pull request is not readable"),
            "{}",
            l[0]
        );
    }
}

#[test]
fn a_branch_that_the_token_cannot_read_is_unknown_not_a_warning() {
    let (fake, c, _b) = crew();
    fake.set_branch_error(403, "Resource not accessible by personal access token");
    for o in [doctor_board(&fake), start(&fake, &c)] {
        assert!(lines_with(&o, "warning: the default branch").is_empty());
        let l = lines_with(
            &o,
            "default branch: protection of fake-owner/fake-repo unknown",
        );
        assert_eq!(l.len(), 1, "{}", o.all());
        assert!(
            l[0].contains("branch main cannot be read (HTTP 403: Resource not accessible"),
            "{}",
            l[0]
        );
    }
}

#[test]
fn the_check_reads_the_branch_once_per_run() {
    let (fake, _c, _b) = crew();
    let before = branch_reads(&fake);
    doctor_board(&fake);
    assert_eq!(branch_reads(&fake) - before, 1);
}

#[test]
fn a_ruleset_that_init_made_is_seen_by_start_and_doctor() {
    let (fake, c, _b) = crew();
    // control: no ruleset, the warning shows
    assert_eq!(
        lines_with(&doctor_board(&fake), "warning: the default branch").len(),
        1
    );
    // the ruleset that `init` creates; the branch answer is the default one (`protected:
    // false`), as for a repo where only the rules endpoint shows the ruleset
    fake.set_rules_answer(
        200,
        json!([{"type": "pull_request", "ruleset_source_type": "Repository"}]),
    );
    for o in [doctor_board(&fake), start(&fake, &c)] {
        assert!(lines_with(&o, "warning: the default branch").is_empty());
        assert_eq!(
            lines_with(
                &o,
                "default branch: main of fake-owner/fake-repo is protected"
            )
            .len(),
            1,
            "{}",
            o.all()
        );
    }
    // an active ruleset of the fake (made by `init`) is read the same way
    let (fake, _c, _b) = crew();
    fake.add_ruleset("crewboard: default branch", "active");
    assert!(lines_with(&doctor_board(&fake), "warning: the default branch").is_empty());
}

#[test]
fn a_rules_read_that_fails_is_unknown_not_a_warning() {
    let (fake, c, _b) = crew();
    // `protected: false`, and the rules read fails: a ruleset may apply, so no guess
    fake.set_rules_answer(500, json!({"message": "boom"}));
    for o in [doctor_board(&fake), start(&fake, &c)] {
        assert!(lines_with(&o, "warning: the default branch").is_empty());
        let l = lines_with(
            &o,
            "default branch: protection of fake-owner/fake-repo unknown",
        );
        assert_eq!(l.len(), 1, "{}", o.all());
        assert!(
            l[0].contains("rules of branch main cannot be read (HTTP 500: boom)"),
            "{}",
            l[0]
        );
    }
}
