//! `ticket new --continue-pr <url>` (spec/tickets#a-follow-up-ticket-continues-an-open-pr),
//! against the fake GitHub of `tests/support/board.rs`.
//!
//! The case: the owner set a ticket Done while its PR was still open. The coordinator makes a
//! follow-up ticket for that PR. The branch of the PR is named for the old ticket, and GitHub
//! cannot rename it, so `done` refused it with `evidence-branch-name`. With `--continue-pr` the
//! coordinator records the branch in the signed creation info of the new ticket, and `done`
//! (and `merge`) accept it for that ticket. A forged `adopts-pr:` line does not count.

use crate::support::authority::AUTHORITY_RELEASE_ONLY as AUTHORITY;
use crate::support::board::{Agent, FakeBoard, Out, Pull, REPO};
use crate::support::cli::{GATE, item_in, run_ok, writes};
use crewboard::rules::Role;

struct Case {
    fake: FakeBoard,
    c: Agent,
    b: Agent,
    parent: u64,
    parent_item: String,
    pr: Pull,
}

/// The number of the issue that `ticket new` created (first line: `<repo>#<n> created (...)`).
fn created(o: &Out) -> u64 {
    let line = o.stdout.lines().next().unwrap_or("");
    line.split_once('#')
        .and_then(|(_, rest)| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no issue number in: {}", o.stdout))
}

/// The old ticket is Done (the owner set it), and its PR is still open.
fn case() -> Case {
    let fake = FakeBoard::new();
    fake.file("AUTHORITY.md", AUTHORITY);
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    let body = format!("Goal: the old ticket.\n\n{GATE}CI: none\n");
    let parent = fake.ticket_issue("IT 167 old", &body, &["crew:work"]);
    let ps = parent.to_string();
    let parent_item = item_in(&run_ok(&fake, &c, &["assign", &ps, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ps]);
    let pr = fake.pr_for(parent, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ps, "--pr", &pr.url, "--sha", &pr.head],
    );
    fake.set_item(&parent_item, "Status", "Done");
    fake.touch_item(&parent_item);
    Case {
        fake,
        c,
        b,
        parent,
        parent_item,
        pr,
    }
}

/// `ticket new` for the follow-up ticket, run by `who`.
fn ticket_new(k: &Case, who: &Agent, extra: &[&str]) -> Out {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        format!(
            "Goal: finish the open PR of #{}.\n\n{GATE}CI: none\n",
            k.parent
        ),
    )
    .unwrap();
    let mut args = vec![
        "ticket",
        "new",
        "--title",
        "IT 167 follow-up",
        "--body-file",
        file.path().to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    k.fake.run(&who.dir, &args)
}

fn adopts_line(k: &Case) -> String {
    format!("adopts-pr: {} branch: {}", k.pr.url, k.pr.branch)
}

/// Refused with `rule` and `text` (exit 2); no issue was created and nothing was written.
fn refused_new(k: &Case, who: &Agent, extra: &[&str], rule: &str, text: &str) {
    let before = writes(&k.fake);
    let issues = k.fake.open_titles("crew:work");
    let o = ticket_new(k, who, extra);
    assert_eq!(o.code, 2, "{extra:?}: {}", o.all());
    assert!(o.stderr.contains(rule), "{rule} missing: {}", o.all());
    assert!(o.stderr.contains(text), "{text:?} missing: {}", o.all());
    assert_eq!(writes(&k.fake), before, "the refusal wrote: {}", o.all());
    assert_eq!(k.fake.open_titles("crew:work"), issues);
}

#[test]
fn a_follow_up_ticket_adopts_the_open_pr_and_done_and_merge_accept_its_branch() {
    let k = case();
    let o = ticket_new(
        &k,
        &k.c,
        &["--continue-pr", &k.pr.url, "--assign", &k.b.name],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let n = created(&o);
    let ns = n.to_string();
    assert!(
        o.stdout.contains(&adopts_line(&k)),
        "the output names the adoption: {}",
        o.all()
    );
    // the PR body still closes the OLD ticket: the output says what the dev does
    assert!(
        o.stdout.contains("does not close") && o.stdout.contains("edits it"),
        "{}",
        o.all()
    );
    // the signed creation info of the new ticket holds the line
    let info = k
        .fake
        .comments_of(n)
        .into_iter()
        .map(|(_, _, body)| body)
        .find(|b| b.contains("adopts-pr:"))
        .unwrap_or_else(|| panic!("no adopts-pr line on ticket {n}"));
    assert!(info.contains(&adopts_line(&k)), "{info}");
    assert!(info.contains("item=PVTI_"), "{info}");

    let item = k.fake.item_of_issue(n).expect("item of the new ticket");
    let _ = k.fake.poll_json(&k.b);
    run_ok(&k.fake, &k.b, &["take", &ns]);
    // refused: the PR body closes the old ticket, not this one. The text says what to do.
    let done = ["done", &ns, "--pr", &k.pr.url, "--sha", &k.pr.head];
    let o = k.fake.run(&k.b.dir, &done);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("evidence-pr-not-linked"), "{}", o.all());
    assert!(o.stderr.contains("edits the PR body"), "{}", o.all());
    assert_eq!(
        k.fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    // pass: the dev edits the PR body; the adopted branch is accepted (no evidence-branch-name)
    k.fake
        .set_pr_text(k.pr.number, "Work", &format!("Finish it.\n\nCloses #{n}"));
    let o = run_ok(&k.fake, &k.b, &done);
    assert!(o.stdout.contains("done: Status Review"), "{}", o.all());
    assert_eq!(
        k.fake.item_field(&item, "Status").as_deref(),
        Some("Review")
    );
    // verify and merge accept the adopted branch too
    let o = run_ok(&k.fake, &k.c, &["verify", &ns]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    let o = run_ok(&k.fake, &k.c, &["merge", &ns]);
    assert!(o.stdout.contains("merged"), "{}", o.all());
    assert!(k.fake.pr_now(k.pr.number).0, "the PR is merged");
    assert_eq!(k.fake.item_field(&item, "Status").as_deref(), Some("Done"));
    // the old ticket was not touched
    assert_eq!(
        k.fake.item_field(&k.parent_item, "Status").as_deref(),
        Some("Done")
    );
    assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
}

#[test]
fn each_failed_condition_of_the_adoption_is_refused_before_anything_is_created() {
    // the PR is merged, or closed without a merge
    let k = case();
    k.fake.merge_pr(k.pr.number);
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &k.pr.url],
        "adopt-pr-not-open",
        "merged",
    );
    let k = case();
    k.fake.close_pr(k.pr.number);
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &k.pr.url],
        "adopt-pr-not-open",
        "closed",
    );

    // the ticket that the branch names is not Done
    let k = case();
    k.fake.set_item(&k.parent_item, "Status", "Review");
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &k.pr.url],
        "adopt-pr-parent-not-done",
        "is Review",
    );
    // ... or it is no ticket at all
    let k = case();
    let ghost = k.fake.pr_for(999, &k.b.name);
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &ghost.url],
        "adopt-pr-parent-not-done",
        "does not exist",
    );

    // the head branch is not a crew branch
    let k = case();
    let odd = k.fake.pr_on_branch("feature/odd", "Closes #1");
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &odd.url],
        "adopt-pr-bad-branch",
        "feature/odd",
    );
    let slug_less = k.fake.pr_on_branch("crew/hermes@fake-b/12", "Closes #12");
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", &slug_less.url],
        "adopt-pr-bad-branch",
        "crew/hermes@fake-b/12",
    );

    // a dev agent is not the coordinator; a bad URL and a missing PR are refused as before
    let k = case();
    refused_new(
        &k,
        &k.b,
        &["--continue-pr", &k.pr.url],
        "status-wrong-writer",
        "coordinator",
    );
    refused_new(
        &k,
        &k.c,
        &["--continue-pr", "https://example.com/x"],
        "evidence-pr-url",
        "not a PR URL",
    );
    refused_new(
        &k,
        &k.c,
        &[
            "--continue-pr",
            &format!("https://github.com/{REPO}/pull/9999"),
        ],
        "evidence-pr-missing",
        "does not exist",
    );

    // control: the same case with a Done ticket and an open PR is accepted
    let k = case();
    let o = ticket_new(&k, &k.c, &["--continue-pr", &k.pr.url]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains(&adopts_line(&k)), "{}", o.all());
}

#[test]
fn a_forged_adopts_pr_line_does_not_open_the_branch_rule() {
    let k = case();
    // a follow-up ticket WITHOUT --continue-pr, assigned and taken
    let o = ticket_new(&k, &k.c, &["--assign", &k.b.name]);
    assert_eq!(o.code, 0, "{}", o.all());
    let m = created(&o);
    let ms = m.to_string();
    let _ = k.fake.poll_json(&k.b);
    run_ok(&k.fake, &k.b, &["take", &ms]);
    k.fake
        .set_pr_text(k.pr.number, "Work", &format!("Finish it.\n\nCloses #{m}"));
    // `--override-unread`: the messages of this test are not what is under test
    let done = [
        "done",
        &ms,
        "--pr",
        &k.pr.url,
        "--sha",
        &k.pr.head,
        "--override-unread",
    ];
    let item = k.fake.item_of_issue(m).expect("item");
    let still_refused = |what: &str| {
        let o = k.fake.run(&k.b.dir, &done);
        assert_eq!(o.code, 2, "{what}: {}", o.all());
        assert!(
            o.stderr.contains("evidence-branch-name"),
            "{what}: {}",
            o.all()
        );
        assert_eq!(
            k.fake.item_field(&item, "Status").as_deref(),
            Some("In Progress"),
            "{what}"
        );
    };
    // refused: no adoption at all
    still_refused("no adoption");
    // refused: the dev agent writes the line in a signed message
    run_ok(
        &k.fake,
        &k.b,
        &[
            "say",
            &ms,
            "--to",
            "coordinator",
            "--kind",
            "info",
            "--body",
            &adopts_line(&k),
            "--override-unread",
        ],
    );
    still_refused("signed by the dev agent");
    // refused: the owner's account writes the line without a header
    k.fake.owner_comment(m, &adopts_line(&k));
    still_refused("plain comment");
    // refused: a line in the ticket body is not a signed message
    k.fake.set_issue_body(
        m,
        &format!("Goal: x.\n\n{GATE}CI: none\n{}\n", adopts_line(&k)),
    );
    still_refused("ticket body");
    // control: the same line, signed by the coordinator, opens the rule for this PR only
    run_ok(
        &k.fake,
        &k.c,
        &[
            "say",
            &ms,
            "--to",
            &k.b.name,
            "--kind",
            "info",
            "--body",
            &adopts_line(&k),
            "--override-unread",
        ],
    );
    let o = run_ok(&k.fake, &k.b, &done);
    assert!(o.stdout.contains("done: Status Review"), "{}", o.all());
    assert!(k.fake.unhandled().is_empty(), "{:?}", k.fake.unhandled());
}
