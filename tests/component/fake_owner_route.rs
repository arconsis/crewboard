//! The owner route and the closing keyword (spec/tickets#workflow-files-the-owner-route),
//! against the fake GitHub of `tests/support/board.rs`.
//!
//! A PR that holds the workflow change only as a patch must not close its ticket, and a ticket
//! that closed anyway is put right by the coordinator.

use crate::support::board::{Agent, FakeBoard, Out, Pull, REPO};
use crate::support::cli::{crew, item_in, run_ok, writes};
use crewboard::config::ConfigDir;
use crewboard::protocol::names::{AgentName, Kind};
use crewboard::state::StateCache;

const GATE: &str = "**Gate:** the PR of this ticket is open and refers to it.\n";

fn status(fake: &FakeBoard, item: &str) -> String {
    fake.item_field(item, "Status").unwrap_or_default()
}

/// The number of GET requests whose path holds `part`.
fn gets(fake: &FakeBoard, part: &str) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(part))
        .count()
}

/// Questions of a ticket that start with `marker`.
fn questions(fake: &FakeBoard, t: u64, marker: &str) -> usize {
    fake.comments_of(t)
        .iter()
        .filter(|(_, _, b)| b.contains(marker) && b.contains("kind=question"))
        .count()
}

struct Owner {
    t: u64,
    ts: String,
    item: String,
    pr: Pull,
}

/// A ticket on the owner route, assigned to B and taken. The PR holds the patch and, to start
/// with, the text `body`.
fn owner_ticket(fake: &FakeBoard, c: &Agent, b: &Agent, body: &str) -> Owner {
    let text = format!("Goal: integration test ticket.\n\nRoute: owner-workflow\n{GATE}CI: none\n");
    let t = fake.ticket_issue("IT owner route", &text, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(fake, c, &["assign", &ts, "--to", &b.name]));
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.pr_file(pr.number, &format!("docs/patches/{t}-workflow.patch"));
    fake.set_pr_text(pr.number, "Work item", &body.replace("{t}", &ts));
    let _ = fake.poll_json(b);
    Owner { t, ts, item, pr }
}

fn done(fake: &FakeBoard, b: &Agent, o: &Owner) -> Out {
    fake.run(
        &b.dir,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    )
}

fn marker(o: &Owner) -> String {
    format!("owner-route-merged: {}", o.pr.url)
}

/// The owner merges the PR before the patch is applied; GitHub closes the issue; the board
/// automation moves the ticket to Done.
fn merged_by_mistake(fake: &FakeBoard, o: &Owner) {
    fake.merge_pr(o.pr.number);
    fake.close_issue(o.t);
    fake.set_item(&o.item, "Status", "Done");
}

#[test]
fn done_with_a_closing_keyword_on_the_owner_route_is_refused_and_writes_nothing() {
    let (fake, c, b) = crew();
    // every form of a closing keyword for this ticket, in the body or in the title
    for (title, body) in [
        ("Work", "Closes #{t}"),
        ("Work", "this fixes: #{t}."),
        ("Work", "RESOLVED #{t}"),
        ("Work", "Closes {repo}#{t}"),
        ("Work", "Fixes https://github.com/{repo}/issues/{t}"),
        ("Closes #{t}", "Refs #{t}"),
    ] {
        let o = owner_ticket(&fake, &c, &b, "x");
        let fill = |s: &str| s.replace("{t}", &o.ts).replace("{repo}", REPO);
        fake.set_pr_text(o.pr.number, &fill(title), &fill(body));
        let before = writes(&fake);
        let comments = fake.comments_of(o.t).len();
        let r = done(&fake, &b, &o);
        assert_eq!(r.code, 2, "{title:?} {body:?}: {}", r.all());
        assert!(
            r.stderr.contains("owner-route-closing-keyword"),
            "{}",
            r.all()
        );
        assert!(r.stderr.contains(&format!("Refs #{}", o.t)), "{}", r.all());
        assert_eq!(writes(&fake), before, "nothing written: {title:?} {body:?}");
        assert_eq!(fake.comments_of(o.t).len(), comments);
        assert_eq!(status(&fake, &o.item), "In Progress");
        // free the dev agent for the next ticket of the loop: the PR text says Refs now
        fake.set_pr_text(o.pr.number, "Work", &format!("Refs #{}", o.t));
        run_ok(
            &fake,
            &b,
            &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
        );
    }
}

#[test]
fn done_with_refs_goes_to_review_with_an_owner_question_and_a_keyword_for_another_issue_does_not_count()
 {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    let r = done(&fake, &b, &o);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.contains("Status Review"), "{}", r.stdout);
    assert_eq!(status(&fake, &o.item), "Review");
    // the question to the owner is open: the label is set
    assert!(fake.labels_of(o.t).contains(&"needs-owner".to_string()));
    // verify is refused: the owner applies the patch and merges the PR by hand
    for answered in [false, true] {
        if answered {
            fake.owner_comment_now(o.t, "Seen.");
        }
        let before = writes(&fake);
        let v = fake.run(&c.dir, &["verify", &o.ts]);
        assert_eq!(v.code, 2, "{}", v.all());
        assert!(v.stderr.contains("verify-owner-route"), "{}", v.all());
        assert!(
            v.stderr.contains(&format!("then crewboard close {}", o.t)),
            "{}",
            v.all()
        );
        assert_eq!(writes(&fake), before);
    }
    // the owner question: the first lines say apply the patch and do not merge
    let q = fake
        .comments_of(o.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("owner-route: ") && b.contains("git apply"))
        .expect("owner question");
    let head: String = q
        .lines()
        .filter(|l| !l.starts_with("<!--") && !l.trim().is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        head.contains("Apply the patch to the branch and push"),
        "{q}"
    );
    assert!(head.contains("Do NOT merge the PR as it is"), "{q}");
    assert!(
        head.contains("after your merge the coordinator closes it"),
        "{q}"
    );

    // a keyword for ANOTHER issue is no closing keyword for this ticket
    let o2 = owner_ticket(
        &fake,
        &c,
        &b,
        "Refs #{t}\n\nCloses #9999\nFixes other/repo#1",
    );
    let r = done(&fake, &b, &o2);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(status(&fake, &o2.item), "Review");
}

#[test]
fn done_needs_a_link_to_the_ticket_on_the_owner_route() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "No link at all.");
    let r = done(&fake, &b, &o);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.stderr.contains("evidence-pr-not-linked"), "{}", r.all());
    assert!(r.stderr.contains("Refs #"), "{}", r.all());
    assert_eq!(status(&fake, &o.item), "In Progress");
}

#[test]
fn after_the_owner_applied_the_patch_closes_is_right_again() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    assert_eq!(status(&fake, &o.item), "Review");
    // the owner applies the patch on the branch (workflow files in the PR) and answers
    fake.pr_file(o.pr.number, ".github/workflows/ci.yml");
    let head = fake.push(&o.pr.branch);
    fake.owner_comment_now(o.t, "Applied and pushed.");
    let _ = fake.poll_json(&b);
    // the dev agent says Closes again: the normal route
    fake.set_pr_text(o.pr.number, "Work", &format!("Closes #{}", o.t));
    let r = fake.run(&b.dir, &["done", &o.ts, "--pr", &o.pr.url, "--sha", &head]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.contains("Status Review"), "{}", r.stdout);
    assert_eq!(status(&fake, &o.item), "Review");
}

#[test]
fn a_done_ticket_with_a_merged_patch_only_pr_is_reopened_once() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    merged_by_mistake(&fake, &o);
    assert!(!fake.issue_is_open(o.t));

    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert!(fake.issue_is_open(o.t), "the issue is open again");
    // Done is final: the reopen writes no Status, the owner moves the ticket
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(fake.labels_of(o.t).contains(&"needs-owner".to_string()));
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
    let q = fake
        .comments_of(o.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains(&marker(&o)))
        .unwrap();
    assert!(q.contains("git apply docs/patches/"), "{q}");
    assert!(q.contains("do NOT treat the merged PR"), "{q}");

    // a second poll does nothing: no new comment, no PATCH, no read of the ticket or its PR
    let comment_reads = gets(&fake, &format!("issues/{}/comments", o.t));
    let pr_reads = gets(&fake, "/pulls/");
    let patches = fake
        .requests()
        .iter()
        .filter(|r| r.method == "PATCH")
        .count();
    let n_comments = fake.comments_of(o.t).len();
    let (v, _) = fake.poll_json(&c);
    assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
    assert_eq!(fake.comments_of(o.t).len(), n_comments);
    assert_eq!(status(&fake, &o.item), "Done");
    assert_eq!(
        fake.requests()
            .iter()
            .filter(|r| r.method == "PATCH")
            .count(),
        patches
    );
    assert_eq!(
        gets(&fake, &format!("issues/{}/comments", o.t)),
        comment_reads
    );
    assert_eq!(gets(&fake, "/pulls/"), pr_reads);
}

#[test]
fn a_done_ticket_whose_pr_changed_the_workflow_files_is_left_alone() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    // the owner applied the patch on the branch of the PR, then merged it
    fake.pr_file(o.pr.number, ".github/workflows/ci.yml");
    merged_by_mistake(&fake, &o);
    let n_comments = fake.comments_of(o.t).len();
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(!fake.issue_is_open(o.t));
    assert_eq!(questions(&fake, o.t, &marker(&o)), 0);
    assert_eq!(fake.comments_of(o.t).len(), n_comments);
    // it was judged: the next poll does not read the PR again
    let pr_reads = gets(&fake, "/pulls/");
    let _ = fake.poll_json(&c);
    assert_eq!(gets(&fake, "/pulls/"), pr_reads);
}

#[test]
fn a_done_ticket_whose_owner_route_pr_is_not_merged_yet_is_left_alone() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    fake.set_item(&o.item, "Status", "Done");
    let (v, _) = fake.poll_json(&c);
    assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(status(&fake, &o.item), "Done");
    // not judged for good: when the PR is merged later, the net finds it
    fake.merge_pr(o.pr.number);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(fake.issue_is_open(o.t));
}

#[test]
fn a_failed_reopen_is_repeated_without_a_second_question() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    merged_by_mistake(&fake, &o);
    // the reopen of the issue fails once, after the question
    fake.fail_once("PATCH", &format!("issues/{}", o.t), "open");
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-check-failed"), "{v}");
    assert_eq!(
        status(&fake, &o.item),
        "Done",
        "the reopen writes no Status"
    );
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(fake.issue_is_open(o.t));
    assert!(fake.labels_of(o.t).contains(&"needs-owner".to_string()));
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1, "one question per PR");
}

#[test]
fn judged_tickets_are_not_read_again_and_a_plain_done_ticket_costs_one_read() {
    let (fake, c, b) = crew();
    // a Done ticket on the normal route
    let text = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue("IT plain", &text, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    let reads = || gets(&fake, &format!("issues/{t}/comments"));
    // the baseline: a poll while the ticket is in Review
    let _ = fake.poll_json(&c);
    let r0 = reads();
    let _ = fake.poll_json(&c);
    let base = reads() - r0;
    fake.merge_pr(pr.number);
    fake.set_item(&item, "Status", "Done");
    let r1 = reads();
    let _ = fake.poll_json(&c);
    let first = reads() - r1;
    // one read for the owner-route judgement, one for the check of the Done against the log
    // (`ops::log_check`); the owner merged by hand, so the log shows Review and the Done is
    // seen again until the item is quiet, without a read
    assert_eq!(first, base + 2, "one read each to judge it");
    let pulls = gets(&fake, "/pulls/");
    let r2 = reads();
    let _ = fake.poll_json(&c);
    assert_eq!(reads() - r2, base, "judged once, not read again");
    assert_eq!(
        gets(&fake, "/pulls/"),
        pulls,
        "no PR read for a plain ticket"
    );
    assert_eq!(status(&fake, &item), "Done");
}

/// A copy of the coordinator's config dir with `headless = true`.
fn headless_dir(c: &Agent) -> tempfile::TempDir {
    let to = tempfile::tempdir().unwrap();
    let from = ConfigDir(c.dir.clone());
    let into = ConfigDir(to.path().to_path_buf());
    let src = from.agent_config_path("fake");
    let text = std::fs::read_to_string(&src).unwrap();
    std::fs::write(
        into.agent_config_path("fake"),
        format!("headless = true\n{text}"),
    )
    .unwrap();
    let name = crewboard::protocol::names::AgentName::parse(&c.name).unwrap();
    let key_to = into.key_path(&name);
    std::fs::create_dir_all(key_to.parent().unwrap()).unwrap();
    std::fs::copy(from.key_path(&name), &key_to).unwrap();
    to
}

#[test]
fn the_headless_tick_only_logs_and_the_interactive_poll_reopens() {
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    let h = headless_dir(&c);
    // the headless dir learns the ticket while it is open
    let _ = fake.run(h.path(), &["poll", "--json"]);
    merged_by_mistake(&fake, &o);

    let w = writes(&fake);
    let t = fake.run(h.path(), &["tick"]);
    assert_eq!(t.code, 0, "{}", t.all());
    assert!(
        t.stdout.contains(&format!(
            "left for the interactive coordinator: {}#{}",
            REPO, o.t
        )),
        "{}",
        t.all()
    );
    assert_eq!(writes(&fake), w, "the headless tick writes nothing");
    // a headless poll does not reopen either
    let _ = fake.run(h.path(), &["poll", "--json"]);
    assert_eq!(writes(&fake), w, "the headless poll writes nothing for it");
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(!fake.issue_is_open(o.t));

    // the interactive coordinator does the reopen
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(status(&fake, &o.item), "Done");
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
}

/// The owner route after the dev agent's `done`: the ticket is in Review, the PR text says
/// `Refs`, so GitHub closes nothing when the owner merges.
fn delivered(fake: &FakeBoard, c: &Agent, b: &Agent) -> Owner {
    let o = owner_ticket(fake, c, b, "Refs #{t}");
    run_ok(
        fake,
        b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    assert_eq!(status(fake, &o.item), "Review");
    o
}

/// `close` is refused with `close-needs-reason`, names `text`, and writes nothing.
fn close_refused(fake: &FakeBoard, c: &Agent, o: &Owner, text: &str) {
    let before = writes(fake);
    let r = fake.run(&c.dir, &["close", &o.ts]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.stderr.contains("close-needs-reason"), "{}", r.all());
    assert!(r.stderr.contains(text), "{text:?}: {}", r.all());
    assert_eq!(writes(fake), before, "the refusal wrote: {}", r.all());
    assert_eq!(status(fake, &o.item), "Review");
    assert!(fake.issue_is_open(o.t));
}

#[test]
fn close_after_the_owner_applied_the_patch_and_merged_the_pr_ends_the_ticket() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    // the owner applies the patch on the branch (a workflow file in the PR) and merges by hand;
    // the PR has no closing keyword, so GitHub leaves the issue open
    fake.pr_file(o.pr.number, ".github/workflows/ci.yml");
    fake.merge_pr(o.pr.number);
    assert!(fake.issue_is_open(o.t));
    let r = fake.run(&c.dir, &["close", &o.ts]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.stdout.contains("Status Done"), "{}", r.stdout);
    assert_eq!(status(&fake, &o.item), "Done");
    assert!(!fake.issue_is_open(o.t), "close ends the issue");
    let notice = fake
        .comments_of(o.t)
        .into_iter()
        .map(|(_, _, b)| b)
        .find(|b| b.contains("merged by the owner"))
        .expect("the owner-merge notice");
    assert!(
        notice.contains(&format!("closed: owner merged {}", o.pr.url)),
        "{notice}"
    );
}

#[test]
fn close_is_refused_when_the_merged_owner_route_pr_has_no_workflow_file() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    // merged with the patch only: the workflow change is not live
    fake.merge_pr(o.pr.number);
    close_refused(&fake, &c, &o, "no file under .github/workflows/");
}

#[test]
fn close_is_refused_when_someone_else_merged_the_owner_route_pr() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    fake.pr_file(o.pr.number, ".github/workflows/ci.yml");
    fake.merge_pr_by(o.pr.number, "fake-crew");
    close_refused(&fake, &c, &o, "merged by fake-crew, not by the owner");
}

/// The patch file of the PR names the workflow file that it changes.
fn patch_text(file: &str) -> String {
    format!("diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1 +1 @@\n-a\n+b\n")
}

#[test]
fn a_workflow_file_on_the_base_branch_without_the_added_lines_is_still_a_finding() {
    // (file on the base branch, its text): the patch adds the line `b` to ci.yml
    for (file, text) in [
        (".github/workflows/ci.yml", "a\n"),
        (".github/workflows/other.yml", "b\n"),
        ("src/main.rs", "b\n"),
    ] {
        let (fake, c, b) = crew();
        let o = delivered(&fake, &c, &b);
        fake.file(
            &format!("docs/patches/{}-workflow.patch", o.t),
            &patch_text(".github/workflows/ci.yml"),
        );
        fake.file(file, text);
        // a commit is no proof either
        fake.base_commit(".github/workflows/ci.yml", "2999-01-01T00:00:00Z");
        merged_by_mistake(&fake, &o);
        let (v, _) = fake.poll_json(&c);
        assert!(
            v.to_string().contains("owner-route-reopened"),
            "{file}: {v}"
        );
        assert!(fake.issue_is_open(o.t));
    }
}

/// A finding, then the answer, then the issue closed again and the item changed.
fn settled_after(answer: impl Fn(&FakeBoard, &Agent, &Owner)) {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    merged_by_mistake(&fake, &o);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
    // the coordinator answers, removes the label, closes the issue; the item changes
    answer(&fake, &c, &o);
    fake.close_issue(o.t);
    // each change gets its own stamp (the clock of the fake counts seconds)
    fake.touch_item_ago(&o.item, 1000);
    let n_comments = fake.comments_of(o.t).len();
    for k in 0..2 {
        let (v, _) = fake.poll_json(&c);
        assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
        assert!(!fake.issue_is_open(o.t), "the issue stays closed");
        assert_eq!(fake.comments_of(o.t).len(), n_comments);
        fake.touch_item_ago(&o.item, 2000 + 1000 * k);
    }
}

#[test]
fn a_finding_that_the_owner_answered_is_settled() {
    settled_after(|fake, _, o| {
        fake.owner_comment_now(o.t, "I handle it.");
    });
}

#[test]
fn a_finding_that_the_coordinator_withdrew_is_settled() {
    settled_after(|fake, c, o| {
        run_ok(
            fake,
            c,
            &[
                "say",
                &o.ts,
                "--to",
                "owner",
                "--kind",
                "answer",
                "--body",
                "settled by the owner\nwithdraws: question",
            ],
        );
    });
}

#[test]
fn a_finding_that_is_not_answered_stays_a_finding_and_writes_once() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    merged_by_mistake(&fake, &o);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    // the writes changed the item: it is judged again, and every write is in place already
    fake.touch_item_ago(&o.item, 1000);
    let before = fake.requests().len();
    let _ = fake.poll_json(&c);
    let issue_writes: Vec<String> = fake.requests()[before..]
        .iter()
        .filter(|r| {
            let own = format!("/issues/{}", o.t);
            r.method != "GET" && (r.path.ends_with(&own) || r.path.contains(&format!("{own}/")))
        })
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    assert!(
        issue_writes.is_empty(),
        "a second judgement writes nothing to the issue: {issue_writes:?}"
    );
    assert!(fake.issue_is_open(o.t));
    assert_eq!(questions(&fake, o.t, &marker(&o)), 1);
}

#[test]
fn close_accepts_a_patch_whose_added_lines_are_on_the_base_branch() {
    for (text, ok) in [("b\n", true), ("a\nb\n", false), ("a\n", false)] {
        let (fake, c, b) = crew();
        let o = delivered(&fake, &c, &b);
        fake.file(
            &format!("docs/patches/{}-workflow.patch", o.t),
            &patch_text(".github/workflows/ci.yml"),
        );
        fake.file(".github/workflows/ci.yml", text);
        // the owner merges the patch-only PR by hand; the issue stays open
        fake.merge_pr(o.pr.number);
        if ok {
            let r = fake.run(&c.dir, &["close", &o.ts]);
            assert_eq!(r.code, 0, "{}", r.all());
            assert_eq!(status(&fake, &o.item), "Done");
            assert!(!fake.issue_is_open(o.t));
        } else {
            close_refused(
                &fake,
                &c,
                &o,
                "the PR changed no file under .github/workflows/",
            );
        }
    }
}

#[test]
fn close_on_a_normal_ticket_with_an_open_issue_is_unchanged() {
    let (fake, c, b) = crew();
    let text = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue("IT plain", &text, &["crew:work"]);
    let ts = t.to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    fake.pr_file(pr.number, ".github/workflows/ci.yml");
    fake.merge_pr(pr.number);
    let before = writes(&fake);
    let r = fake.run(&c.dir, &["close", &ts]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.stderr.contains("the issue is still open"), "{}", r.all());
    assert!(!r.stderr.contains("owner route"), "{}", r.all());
    assert_eq!(writes(&fake), before);
    assert_eq!(status(&fake, &item), "Review");
}

// ---- the proof is tied to the ticket, and it is a content check ----------------------------

/// The patch of ticket `o`: it adds `line` to the workflow file `file`.
fn add_line_patch(file: &str, line: &str) -> String {
    format!(
        "diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1 +1,2 @@\n name: ci\n+{line}\n"
    )
}

fn put_patch(fake: &FakeBoard, o: &Owner, text: &str) {
    fake.file(&format!("docs/patches/{}-workflow.patch", o.t), text);
}

/// The agent `b` signs a `done` by hand (it holds its key, so it can skip the checks of the CLI).
fn rogue_done(fake: &FakeBoard, b: &Agent, o: &Owner, pr: &Pull, route_line: &str) {
    let body = format!(
        "Done. Evidence follows.\npr: {}\nsha: {}\nci: none\n{route_line}\n",
        pr.url, pr.head
    );
    fake.signed_ago(&b.key, &b.name, "coordinator", Kind::Done, o.t, &body, 10);
}

/// An old PR of another ticket that changed a workflow file; the owner merged it.
fn old_owner_merged_pr(fake: &FakeBoard, b: &Agent) -> Pull {
    let old = fake.pr_for(9000, &b.name);
    fake.pr_file(old.number, ".github/workflows/ci.yml");
    fake.merge_pr(old.number);
    old
}

fn judged(c: &Agent, item: &str) -> Option<String> {
    let cd = ConfigDir(c.dir.clone());
    let p = cd.state_path("fake", &AgentName::parse(&c.name).unwrap());
    StateCache::load(&p)
        .owner
        .owner_route_judged
        .get(item)
        .cloned()
}

#[test]
fn a_done_that_names_an_old_pr_of_another_ticket_does_not_close_this_ticket() {
    for issue_open in [true, false] {
        let (fake, c, b) = crew();
        let o = delivered(&fake, &c, &b);
        let old = old_owner_merged_pr(&fake, &b);
        rogue_done(
            &fake,
            &b,
            &o,
            &old,
            &format!("owner-route: docs/patches/{}-workflow.patch", o.t),
        );
        if !issue_open {
            fake.close_issue(o.t);
        }
        let before = writes(&fake);
        let r = fake.run(&c.dir, &["close", &o.ts]);
        assert_eq!(r.code, 2, "open={issue_open}: {}", r.all());
        assert!(r.stderr.contains("close-needs-reason"), "{}", r.all());
        assert_eq!(writes(&fake), before, "{}", r.all());
        assert_eq!(status(&fake, &o.item), "Review");
    }
}

#[test]
fn a_done_with_the_patch_of_another_ticket_does_not_close_this_ticket() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    // the PR of this ticket holds the workflow file, but the done names another patch
    fake.pr_file(o.pr.number, ".github/workflows/ci.yml");
    fake.merge_pr(o.pr.number);
    rogue_done(
        &fake,
        &b,
        &o,
        &o.pr,
        "owner-route: docs/patches/9999-workflow.patch",
    );
    let r = fake.run(&c.dir, &["close", &o.ts]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(
        r.stderr.contains("docs/patches/9999-workflow.patch"),
        "{}",
        r.all()
    );
    assert_eq!(status(&fake, &o.item), "Review");
}

#[test]
fn the_net_does_not_settle_a_done_that_names_an_old_pr_of_another_ticket() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    let old = old_owner_merged_pr(&fake, &b);
    rogue_done(
        &fake,
        &b,
        &o,
        &old,
        &format!("owner-route: docs/patches/{}-workflow.patch", o.t),
    );
    fake.close_issue(o.t);
    fake.set_item(&o.item, "Status", "Done");
    let (v, _) = fake.poll_json(&c);
    assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
    assert_eq!(judged(&c, &o.item), None, "not judged for good");
}

#[test]
fn a_patch_path_with_a_fragment_or_query_mark_is_not_applied() {
    for bad in [".github/workflows/ci.yml#", ".github/workflows/ci.yml?x=1"] {
        let (fake, c, b) = crew();
        let o = delivered(&fake, &c, &b);
        put_patch(&fake, &o, &add_line_patch(bad, "on: push"));
        // a commit on the base branch, and the file with the line: the path is still no proof
        fake.base_commit(".github/workflows/ci.yml", "2999-01-01T00:00:00Z");
        fake.file(".github/workflows/ci.yml", "name: ci\non: push\n");
        merged_by_mistake(&fake, &o);
        let (v, _) = fake.poll_json(&c);
        assert!(v.to_string().contains("owner-route-reopened"), "{bad}: {v}");
        assert!(fake.issue_is_open(o.t));
    }
}

#[test]
fn an_unreadable_patch_is_not_applied() {
    // the patch file is in the PR but cannot be read at the head (here: not on the fake)
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    fake.base_commit(".github/workflows/ci.yml", "2999-01-01T00:00:00Z");
    merged_by_mistake(&fake, &o);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
}

#[test]
fn a_patch_that_names_no_workflow_file_is_not_applied() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    put_patch(&fake, &o, "no diff here\n");
    fake.base_commit(".github/workflows/ci.yml", "2999-01-01T00:00:00Z");
    merged_by_mistake(&fake, &o);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
}

#[test]
fn the_patch_of_another_ticket_on_the_base_branch_is_no_proof() {
    let (fake, c, b) = crew();
    let a = delivered(&fake, &c, &b);
    put_patch(
        &fake,
        &a,
        &add_line_patch(".github/workflows/ci.yml", "on: push"),
    );
    // ticket B patches the same file; its line is live, the line of ticket A is not
    let bt = delivered(&fake, &c, &b);
    put_patch(
        &fake,
        &bt,
        &add_line_patch(".github/workflows/ci.yml", "env: B"),
    );
    fake.file(".github/workflows/ci.yml", "name: ci\nenv: B\n");
    fake.base_commit(".github/workflows/ci.yml", "2999-01-01T00:00:00Z");
    merged_by_mistake(&fake, &a);
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
    assert!(fake.issue_is_open(a.t));
}

#[test]
fn the_added_lines_on_the_base_branch_are_the_proof_whatever_the_commit_message_says() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    put_patch(
        &fake,
        &o,
        &add_line_patch(".github/workflows/ci.yml", "on: push"),
    );
    // the owner applied the patch with a message of their own and merged the patch-only PR;
    // the fake lists no commit for the file at all
    fake.file(".github/workflows/ci.yml", "name: ci\non: push\n");
    merged_by_mistake(&fake, &o);
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    assert!(!v.to_string().contains("owner-route-reopened"), "{v}");
    assert!(!fake.issue_is_open(o.t));
}

#[test]
fn close_after_a_pr_with_a_closing_keyword_needs_the_applied_patch_too() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    put_patch(
        &fake,
        &o,
        &add_line_patch(".github/workflows/ci.yml", "on: push"),
    );
    // someone adds Closes to the PR text after the done; the owner merges the patch-only PR early
    fake.set_pr_text(o.pr.number, "Work", &format!("Closes #{}", o.t));
    fake.merge_pr(o.pr.number);
    fake.close_issue(o.t);
    let before = writes(&fake);
    let r = fake.run(&c.dir, &["close", &o.ts]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.stderr.contains("close-needs-reason"), "{}", r.all());
    assert_eq!(writes(&fake), before, "{}", r.all());
    assert_eq!(status(&fake, &o.item), "Review");
}

#[test]
fn close_reads_the_pr_once_and_the_headless_tick_does_not_repeat_a_logged_finding() {
    let (fake, c, b) = crew();
    let o = delivered(&fake, &c, &b);
    put_patch(
        &fake,
        &o,
        &add_line_patch(".github/workflows/ci.yml", "on: push"),
    );
    fake.file(".github/workflows/ci.yml", "name: ci\non: push\n");
    fake.merge_pr(o.pr.number);
    let pr_gets = |fake: &FakeBoard| {
        fake.requests()
            .iter()
            .filter(|r| r.method == "GET" && r.path.ends_with(&format!("/pulls/{}", o.pr.number)))
            .count()
    };
    let before = pr_gets(&fake);
    let r = fake.run(&c.dir, &["close", &o.ts]);
    assert_eq!(r.code, 0, "{}", r.all());
    // the merge-note of `close` reads the PR itself (one more); the judgement read it once
    assert!(pr_gets(&fake) - before <= 2, "{}", pr_gets(&fake) - before);

    // a finding in the headless tick: logged once per change of the item
    let (fake, c, b) = crew();
    let o = owner_ticket(&fake, &c, &b, "Refs #{t}");
    run_ok(
        &fake,
        &b,
        &["done", &o.ts, "--pr", &o.pr.url, "--sha", &o.pr.head],
    );
    let h = headless_dir(&c);
    let _ = fake.run(h.path(), &["poll", "--json"]);
    merged_by_mistake(&fake, &o);
    let line = format!("left for the interactive coordinator: {REPO}#{}", o.t);
    let t1 = fake.run(h.path(), &["tick"]);
    assert!(t1.stdout.contains(&line), "{}", t1.all());
    let comments = gets(&fake, &format!("issues/{}/comments", o.t));
    let pulls = gets(&fake, &format!("/pulls/{}", o.pr.number));
    let t2 = fake.run(h.path(), &["tick"]);
    assert!(!t2.stdout.contains(&line), "{}", t2.all());
    assert_eq!(gets(&fake, &format!("/pulls/{}", o.pr.number)), pulls);
    assert!(
        gets(&fake, &format!("issues/{}/comments", o.t)) <= comments + 1,
        "no new judgement"
    );
    // the interactive coordinator still reopens it
    let (v, _) = fake.poll_json(&c);
    assert!(v.to_string().contains("owner-route-reopened"), "{v}");
}

#[test]
fn a_failed_judgement_keeps_the_findings_before_it_and_names_the_ticket() {
    let (fake, c, b) = crew();
    let first = delivered(&fake, &c, &b);
    let second = delivered(&fake, &c, &b);
    merged_by_mistake(&fake, &first);
    merged_by_mistake(&fake, &second);
    // the first is judged first (the newest change), the read of the files of the second fails
    fake.touch_item_ago(&first.item, 1000);
    fake.touch_item_ago(&second.item, 2000);
    fake.fail_once("GET", &format!("pulls/{}/files", second.pr.number), "");
    let (v, _) = fake.poll_json(&c);
    let text = v.to_string();
    assert!(text.contains("owner-route-reopened"), "{v}");
    assert!(
        fake.issue_is_open(first.t),
        "the finding before the failure is put right"
    );
    assert!(text.contains("owner-route-check-failed"), "{v}");
    assert!(text.contains(&format!("{REPO}#{}", second.t)), "{v}");
}

#[test]
fn done_refuses_a_hostile_branch_name_on_both_routes_and_writes_nothing() {
    for (name, patch) in [
        ("crew/{a}/{t}-x;touch${IFS}p", true),
        ("crew/{a}/{t}-x/%2e%2e/main", true),
        ("crew/{a}/{t}-x;touch${IFS}p", false),
        ("crew/{a}/{t}-x/%2e%2e/main", false),
    ] {
        let (fake, c, b) = crew();
        let route = if patch { "Route: owner-workflow\n" } else { "" };
        let text = format!("Goal: integration test ticket.\n\n{route}{GATE}CI: none\n");
        let t = fake.ticket_issue("IT hostile branch", &text, &["crew:work"]);
        let ts = t.to_string();
        run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
        run_ok(&fake, &b, &["take", &ts]);
        let branch = name.replace("{a}", &b.name).replace("{t}", &ts);
        let pr = fake.pr_on_branch(&branch, &format!("Refs #{t}\n\nCloses #{t}\n"));
        if patch {
            fake.pr_file(pr.number, &format!("docs/patches/{t}-workflow.patch"));
            fake.set_pr_text(pr.number, "Work item", &format!("Refs #{t}"));
        } else {
            fake.set_pr_text(pr.number, "Work item", &format!("Closes #{t}"));
        }
        let _ = fake.poll_json(&b);
        let before = writes(&fake);
        let o = fake.run(&b.dir, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
        assert_eq!(o.code, 2, "{branch}: {}", o.all());
        assert!(
            o.stderr.contains("evidence-branch-name"),
            "{branch}: {}",
            o.all()
        );
        assert_eq!(writes(&fake), before, "{branch}: {}", o.all());
        assert_eq!(questions(&fake, t, "owner-route:"), 0, "{branch}");
    }
}
