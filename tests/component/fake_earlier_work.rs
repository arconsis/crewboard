//! The work that an earlier agent leaves (spec/tickets#work-that-an-earlier-agent-left), against
//! the fake GitHub: the reassign notice names the open PR and the branch, `assign` and `take`
//! say so, `take --continue-pr` checks and records the adoption, and `done` accepts the branch.

use crate::support::board::{Agent, FakeBoard, REPO};
use crate::support::cli::{GATE, no_unhandled, run_ok, run_refused, writes};
use crewboard::rules::Role;

fn crew() -> (FakeBoard, Agent, Agent, Agent) {
    let fake = FakeBoard::new();
    let c = fake.agent("codex@fake-c", Role::Coordinator);
    let a = fake.agent("claude@fake-a", Role::Dev);
    let b = fake.agent("hermes@fake-b", Role::Dev);
    (fake, c, a, b)
}

/// A ticket that `who` took and that has a PR of its own branch.
fn in_progress_with_pr(
    fake: &FakeBoard,
    c: &Agent,
    who: &Agent,
) -> (u64, crate::support::board::Pull) {
    let t = fake.ticket_issue(
        "IT earlier work",
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &who.name]);
    run_ok(fake, who, &["take", &ts]);
    let pr = fake.pr_for(t, &who.name);
    (t, pr)
}

fn status(fake: &FakeBoard, t: u64) -> Option<String> {
    fake.item_field(&fake.item_of_issue(t).unwrap(), "Status")
}

fn notices(fake: &FakeBoard, t: u64) -> Vec<String> {
    fake.comments_of(t)
        .into_iter()
        .map(|(_, _, b)| b)
        .filter(|b| b.contains("Reassigned. Reason:"))
        .collect()
}

#[test]
fn the_reassign_notice_names_the_open_pr_and_the_new_agent_adopts_it_and_done_accepts_its_branch() {
    let (fake, c, a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    let ts = t.to_string();
    let o = run_ok(&fake, &c, &["reassign", &ts, "--reason", "dead"]);
    // the output and the signed notice name the PR and the branch
    assert!(
        o.stdout
            .contains(&format!("earlier PR of the old agent: {}", pr.url)),
        "{}",
        o.stdout
    );
    let line = format!("open-pr: {} branch: {}", pr.url, pr.branch);
    let n = notices(&fake, t);
    assert_eq!(n.len(), 1, "{n:?}");
    assert!(
        n[0].contains(&line) && n[0].contains("kind=info"),
        "{}",
        n[0]
    );
    // `assign` to the new agent says how to continue
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    let hint = format!(
        "an earlier PR exists: {}; continue on it with `crewboard take {REPO}#{t} --continue-pr {}` or close it",
        pr.url, pr.url
    );
    assert!(o.stdout.contains(&hint), "{}", o.stdout);
    let _ = fake.poll_json(&a);
    let o = run_ok(&fake, &a, &["take", &ts, "--continue-pr", &pr.url]);
    assert!(
        o.stdout.contains("adopted the open PR of an earlier agent")
            && o.stdout.contains(&pr.branch)
            && o.stdout
                .contains(&format!("crewboard done {t} --pr {}", pr.url)),
        "{}",
        o.stdout
    );
    assert!(
        fake.comments_of(t)
            .iter()
            .any(|(_, _, body)| body.contains("Taken.")
                && body.contains(&format!("adopts-pr: {} branch: {}", pr.url, pr.branch))),
        "the ack records the adoption"
    );
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
    let _ = fake.poll_json(&a);
    let o = run_ok(
        &fake,
        &a,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert!(o.stdout.contains("Review"), "{}", o.stdout);
    no_unhandled(&fake);
}

#[test]
fn take_without_the_flag_names_the_earlier_pr_and_done_still_accepts_its_branch() {
    let (fake, c, a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    let ts = t.to_string();
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead", "--to", &a.name],
    );
    let _ = fake.poll_json(&a);
    let o = run_ok(&fake, &a, &["take", &ts]);
    assert!(
        o.stdout.contains(&format!(
            "an earlier PR exists: {}; continue on its branch",
            pr.url
        )),
        "{}",
        o.stdout
    );
    let _ = fake.poll_json(&a);
    run_ok(
        &fake,
        &a,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
}

#[test]
fn a_pr_that_no_notice_names_cannot_be_adopted() {
    // control: the notice of the coordinator is what allows the branch, nothing else
    let (fake, c, a, b) = crew();
    let t = fake.ticket_issue(
        "IT no notice",
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    let pr = fake.pr_for(t, &b.name);
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert!(!o.stdout.contains("an earlier"), "{}", o.stdout);
    let _ = fake.poll_json(&a);
    run_refused(
        &fake,
        &a,
        &["take", &ts, "--continue-pr", &pr.url],
        "adopt-pr-not-recorded",
    );
}

#[test]
fn take_adopt_pr_refuses_every_pr_that_the_notice_does_not_name_or_that_is_not_open() {
    let (fake, c, a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    let ts = t.to_string();
    // a second, unrelated PR of the same old agent for another ticket
    let other = fake.pr_for(t + 100, &b.name);
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead", "--to", &a.name],
    );
    let _ = fake.poll_json(&a);
    let before = writes(&fake);
    for (url, rule) in [
        (other.url.as_str(), "adopt-pr-not-recorded"),
        (
            "https://github.com/elsewhere/else/pull/3",
            "repo-not-in-initiative",
        ),
        ("not a url", "evidence-pr-url"),
    ] {
        let o = run_refused(&fake, &a, &["take", &ts, "--continue-pr", url], rule);
        assert_eq!(writes(&fake), before, "{rule}: {}", o.all());
    }
    // the PR was closed meanwhile
    fake.close_pr(pr.number);
    let o = run_refused(
        &fake,
        &a,
        &["take", &ts, "--continue-pr", &pr.url],
        "adopt-pr-not-open",
    );
    assert_eq!(writes(&fake), before, "{}", o.all());
    // a closed PR is not named any more
    let o = run_ok(&fake, &a, &["take", &ts]);
    assert!(!o.stdout.contains("an earlier PR exists"), "{}", o.stdout);
    assert_eq!(status(&fake, t).as_deref(), Some("In Progress"));
}

#[test]
fn the_pr_must_name_the_ticket_and_the_flag_needs_a_first_take() {
    let (fake, c, a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    let ts = t.to_string();
    run_ok(
        &fake,
        &c,
        &["reassign", &ts, "--reason", "dead", "--to", &a.name],
    );
    fake.set_pr_text(pr.number, "unrelated", "no link here");
    let _ = fake.poll_json(&a);
    run_refused(
        &fake,
        &a,
        &["take", &ts, "--continue-pr", &pr.url],
        "adopt-pr-not-linked",
    );
    // with the link the take works; a second take of the running ticket cannot adopt again
    fake.set_pr_text(pr.number, "work", &format!("Closes #{t}"));
    run_ok(&fake, &a, &["take", &ts, "--continue-pr", &pr.url]);
    run_refused(
        &fake,
        &a,
        &["take", &ts, "--continue-pr", &pr.url],
        "status-",
    );
}

#[test]
fn a_ticket_that_the_agent_did_not_leave_a_pr_for_gets_no_line() {
    let (fake, c, a, b) = crew();
    let t = fake.ticket_issue(
        "IT no work left",
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let o = run_ok(&fake, &c, &["reassign", &ts, "--reason", "dead"]);
    assert!(!o.stdout.contains("earlier"), "{}", o.stdout);
    let n = notices(&fake, t);
    assert!(
        !n[0].contains("open-pr:") && !n[0].contains("earlier-branch:"),
        "{n:?}"
    );
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert!(!o.stdout.contains("an earlier"), "{}", o.stdout);
}

#[test]
fn a_pushed_branch_without_a_pr_goes_into_the_notice_as_an_earlier_branch() {
    let (fake, c, a, b) = crew();
    let t = fake.ticket_issue(
        "IT branch only",
        &format!("Goal: integration test ticket.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    );
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let branch = format!("crew/{}/{t}-half-done", b.name);
    fake.push_branch(&branch);
    let o = run_ok(&fake, &c, &["reassign", &ts, "--reason", "dead"]);
    assert!(
        o.stdout
            .contains(&format!("earlier branch of the old agent: {branch}")),
        "{}",
        o.stdout
    );
    assert!(notices(&fake, t)[0].contains(&format!("earlier-branch: {branch}")));
    let o = run_ok(&fake, &c, &["assign", &ts, "--to", &a.name]);
    assert!(
        o.stdout
            .contains(&format!("an earlier branch exists: {branch}")),
        "{}",
        o.stdout
    );
}

#[test]
fn stop_force_lists_the_earlier_pr_and_a_follow_up_ticket_may_adopt_it() {
    let (fake, c, _a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    let o = run_ok(&fake, &c, &["stop", "--agent", &b.name, "--force"]);
    assert!(
        o.stdout
            .contains(&format!("{REPO}#{t} (earlier PR {})", pr.url)),
        "{}",
        o.stdout
    );
    assert_eq!(status(&fake, t).as_deref(), Some("Backlog"));
    // a follow-up ticket adopts the PR of the returned ticket: before, only a Done parent passed
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        format!("Goal: finish the PR of #{t}.\n\n{GATE}CI: none\n"),
    )
    .unwrap();
    let args = [
        "ticket",
        "new",
        "--title",
        "IT follow-up",
        "--body-file",
        file.path().to_str().unwrap(),
        "--continue-pr",
        &pr.url,
    ];
    let o = fake.run(&c.dir, &args);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("adopts-pr:"), "{}", o.stdout);
}

#[test]
fn a_ticket_in_backlog_without_a_notice_is_still_no_parent_for_adopt_pr() {
    let (fake, c, _a, b) = crew();
    let (t, pr) = in_progress_with_pr(&fake, &c, &b);
    // back in Backlog by hand: no notice names the PR
    let item = fake.item_of_issue(t).unwrap();
    fake.set_item(&item, "Status", "Backlog");
    fake.touch_item(&item);
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        file.path(),
        format!("Goal: finish the PR of #{t}.\n\n{GATE}CI: none\n"),
    )
    .unwrap();
    let args = [
        "ticket",
        "new",
        "--title",
        "IT follow-up",
        "--body-file",
        file.path().to_str().unwrap(),
        "--continue-pr",
        &pr.url,
    ];
    let o = fake.run(&c.dir, &args);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("adopt-pr-parent-not-done"), "{}", o.all());
}
