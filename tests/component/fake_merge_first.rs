//! The "merge first" hints (skill/references/coordinator.md, "Each cycle: merge first"), against a fake GitHub:
//! `mine` shows how far the PR head is behind its base, `done` warns about it, `status` shows
//! how long a Review ticket has waited for `verify`. All three are read-only advice: the
//! exit codes, the evidence and the board writes stay as they were.

use crate::support::board::{Agent, FakeBoard};
use crate::support::cli::{crew, run_ok};
use crewboard::protocol::names::Kind;

fn ticket(fake: &FakeBoard, title: &str) -> u64 {
    fake.ticket_issue(
        title,
        "Goal: integration test ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n",
        &["crew:work"],
    )
}

fn taken(fake: &FakeBoard, c: &Agent, b: &Agent, t: u64) {
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &b.name]);
    run_ok(fake, b, &["take", &ts]);
}

fn compares(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains("/compare/"))
        .count()
}

#[test]
fn mine_shows_how_far_the_pr_is_behind_main_and_the_exit_code_stays() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT behind mine");
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run_running(9001, &pr.head);
    fake.set_behind(pr.number, 3);
    // CI of the head runs: nothing to do now, behind or not (exit 1); the hint is shown
    let o = fake.run(&b.dir, &["mine"]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(o.stdout.contains("nothing to do"), "{}", o.all());
    assert!(
        o.stdout
            .contains("hint: behind main by 3 commits; merge main before done"),
        "{}",
        o.all()
    );
    // exactly one compare read for the one owned open PR
    assert_eq!(compares(&fake), 1, "{:?}", fake.requests());
    // the JSON form carries the number
    let j = fake.run(&b.dir, &["mine", "--json"]);
    assert_eq!(j.code, 1, "{}", j.all());
    let v: serde_json::Value = serde_json::from_str(j.stdout.trim()).unwrap();
    assert_eq!(v["now"][0]["behind_by"], 3, "{v}");
    assert_eq!(v["needs_agent"], false, "{v}");
    // twin: not behind -> no hint, same verdict
    fake.set_behind(pr.number, 0);
    let o = fake.run(&b.dir, &["mine"]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(!o.stdout.contains("hint:"), "{}", o.all());
    // green CI and behind: work (as before), with the hint
    fake.complete_run(9001, "success");
    fake.set_behind(pr.number, 1);
    let o = fake.run(&b.dir, &["mine"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("hint: behind main by 1 commit; merge main before done"),
        "{}",
        o.all()
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn mine_skips_the_hint_silently_when_the_compare_fails() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT behind error");
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run_running(9001, &pr.head);
    fake.set_behind(pr.number, 2);
    fake.fail_once("GET", "/compare/", "");
    let o = fake.run(&b.dir, &["mine"]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(!o.stdout.contains("hint:"), "{}", o.all());
    assert!(o.stderr.is_empty(), "{}", o.all());
    // the next run reads it again
    let o = fake.run(&b.dir, &["mine"]);
    assert!(o.stdout.contains("behind main by 2 commits"), "{}", o.all());
}

#[test]
fn done_warns_when_the_head_is_behind_and_does_not_refuse() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT behind done");
    let ts = t.to_string();
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    fake.set_behind(pr.number, 2);
    let o = run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert!(
        o.stdout.contains(
            "warning: your branch is 2 commits behind main; merge main and run done again if CI should test the latest code"
        ),
        "{}",
        o.all()
    );
    assert_eq!(o.stdout.matches("behind main").count(), 1, "{}", o.all());
    // the delivery went through: the ticket is in Review
    let s = run_ok(&fake, &c, &["status", "--json"]);
    let v: serde_json::Value = serde_json::from_str(s.stdout.trim()).unwrap();
    assert_eq!(v["tickets"][0]["status"], "Review", "{v}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn done_prints_no_warning_when_the_head_is_not_behind() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT not behind done");
    let ts = t.to_string();
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    let o = run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    assert!(!o.stdout.contains("behind"), "{}", o.all());
    assert_eq!(compares(&fake), 1, "done reads the compare once");
}

#[test]
fn status_shows_how_long_a_review_ticket_has_waited_for_verify() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT review wait");
    let ts = t.to_string();
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    // just delivered: the age is shown, no mark
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains("    done ") && o.stdout.contains("s ago"),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("waiting for verify"), "{}", o.all());
    let v: serde_json::Value =
        serde_json::from_str(run_ok(&fake, &c, &["status", "--json"]).stdout.trim()).unwrap();
    assert_eq!(v["tickets"][0]["waiting_for_verify"], false, "{v}");
    assert!(
        v["tickets"][0]["review_wait_s"].as_i64().unwrap() < 60,
        "{v}"
    );
    // a second ticket was delivered two hours ago: marked, with the time of the done
    let t3 = ticket(&fake, "IT old review");
    taken(&fake, &c, &b, t3);
    let pr3 = fake.pr_for(t3, &b.name);
    let body = format!(
        "Done. Evidence follows.\npr: {}\nsha: {}\nci: none\n",
        pr3.url, pr3.head
    );
    fake.signed_ago(&b.key, &b.name, "coordinator", Kind::Done, t3, &body, 7200);
    fake.set_item(&fake.item_of_issue(t3).unwrap(), "Status", "Review");
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains("waiting for verify since 20"),
        "{}",
        o.all()
    );
    assert!(o.stdout.contains("Z (2h)"), "{}", o.all());
    // the fresh delivery of the first ticket is still unmarked
    assert_eq!(
        o.stdout.matches("waiting for verify").count(),
        1,
        "{}",
        o.all()
    );
    let v: serde_json::Value =
        serde_json::from_str(run_ok(&fake, &c, &["status", "--json"]).stdout.trim()).unwrap();
    let rows = v["tickets"].as_array().unwrap();
    let find = |n: u64| {
        rows.iter()
            .find(|r| r["ticket"].as_str().unwrap().ends_with(&format!("#{n}")))
            .unwrap_or_else(|| panic!("no row for #{n}: {v}"))
    };
    assert_eq!(find(t3)["waiting_for_verify"], true, "{v}");
    assert!(find(t3)["review_wait_s"].as_i64().unwrap() >= 7200, "{v}");
    assert!(find(t3)["review_done_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(find(t)["waiting_for_verify"], false, "{v}");
    // a ticket that is not in Review has no wait
    let t2 = ticket(&fake, "IT no wait");
    run_ok(&fake, &c, &["assign", &t2.to_string(), "--to", &b.name]);
    let v: serde_json::Value =
        serde_json::from_str(run_ok(&fake, &c, &["status", "--json"]).stdout.trim()).unwrap();
    let r = v["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["ticket"].as_str().unwrap().ends_with(&format!("#{t2}")))
        .unwrap_or_else(|| panic!("no row for #{t2}: {v}"));
    assert!(r["review_wait_s"].is_null(), "{r}");
    assert_eq!(r["waiting_for_verify"], false, "{r}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
