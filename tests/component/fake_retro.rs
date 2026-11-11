//! `retro` (spec/retro), end to end against the fake GitHub: a small crew runs two tickets, and
//! the report counts what the signed log shows. `retro` writes nothing, and a comment that only
//! looks like a verify result counts nowhere.

use crate::support::Req;
use crate::support::board::{Agent, FakeBoard, INITIATIVE, OWNER, Out};
use crate::support::cli::{GATE, crew, no_unhandled, run_ok, writes};
use crewboard::protocol::names::Kind;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// AUTHORITY.md that lets the coordinator merge.
const MERGE_AUTHORITY: &str = "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n";

const NOTE: &str = "Gate part 2 (the migration test) is missing";

fn is_read(r: &Req) -> bool {
    r.method == "GET" || (r.path == "/graphql" && !r.body.contains("mutation"))
}

/// Every file under `dir` with its content: the config, the key and the state of an agent.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                todo.push(p);
            } else {
                out.insert(p.display().to_string(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Run `retro` in `dir` and prove that it wrote nothing: no request that writes, and no file
/// of the config dir changed (no cursor, no state file).
fn retro(fake: &FakeBoard, dir: &Path, args: &[&str]) -> Out {
    let (before, files_before) = (writes(fake), files(dir));
    let mut argv = vec!["retro", "--initiative", INITIATIVE];
    argv.extend_from_slice(args);
    let o = fake.run(dir, &argv);
    assert_eq!(writes(fake), before, "retro wrote: {}", o.all());
    assert_eq!(files(dir), files_before, "retro changed a file");
    o
}

fn report(o: &Out) -> Value {
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["initiative"], INITIATIVE);
    v["report"].clone()
}

/// Assign, take, PR and `done`: the ticket is in Review.
fn delivered(
    fake: &FakeBoard,
    c: &Agent,
    b: &Agent,
    title: &str,
) -> (u64, crate::support::board::Pull) {
    let body = format!("Goal: integration test ticket.\n\n{GATE}CI: none\n");
    let t = fake.ticket_issue(title, &body, &["crew:work"]);
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &b.name]);
    run_ok(fake, b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(b);
    run_ok(fake, b, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
    (t, pr)
}

/// Ticket 1: a failed Gate check, a second `done`, a pass and the merge. Ticket 2: the head
/// moves before the verify, then the owner merges and the coordinator closes.
fn two_tickets(fake: &FakeBoard, c: &Agent, b: &Agent) -> (u64, u64) {
    fake.file("AUTHORITY.md", MERGE_AUTHORITY);
    let (t1, pr1) = delivered(fake, c, b, "IT retro rework");
    let ts1 = t1.to_string();
    let o = run_ok(fake, c, &["verify", &ts1, "--fail", "--body", NOTE]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    let _ = fake.poll_json(b);
    run_ok(
        fake,
        b,
        &["done", &ts1, "--pr", &pr1.url, "--sha", &pr1.head],
    );
    let o = run_ok(fake, c, &["verify", &ts1]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
    run_ok(fake, c, &["merge", &ts1]);

    let (t2, pr2) = delivered(fake, c, b, "IT retro head moved");
    let ts2 = t2.to_string();
    fake.push(&pr2.branch);
    let o = run_ok(fake, c, &["verify", &ts2]);
    assert!(o.stdout.contains("failed: PR head moved"), "{}", o.all());
    fake.merge_pr(pr2.number);
    fake.close_issue(t2);
    run_ok(fake, c, &["close", &ts2]);
    (t1, t2)
}

#[test]
fn a_board_without_an_assigned_ticket_gives_exit_1_and_a_bad_since_exit_64() {
    let (fake, c, _b) = crew();
    let o = retro(&fake, &c.dir, &[]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(
        o.stdout
            .contains("retro of initiative fake: no ticket with a signed assignment"),
        "{}",
        o.all()
    );
    let o = retro(&fake, &c.dir, &["--json"]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert_eq!(report(&o)["tickets"], 0);
    // a usage error, before any request
    let from = fake.requests().len();
    let o = retro(&fake, &c.dir, &["--since", "yesterday"]);
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(o.stderr.contains("bad --since"), "{}", o.all());
    assert_eq!(fake.requests().len(), from);
    no_unhandled(&fake);
}

#[test]
fn the_report_counts_the_signed_log_of_two_tickets_and_writes_nothing() {
    let (fake, c, b) = crew();
    let (t1, t2) = two_tickets(&fake, &c, &b);

    let o = retro(&fake, &c.dir, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let text = &o.stdout;
    for want in [
        "retro of initiative fake: 2 ticket(s), ".to_string(),
        "  tickets with an assignment: 2\n  delivered tickets (at least one done): 2\n".to_string(),
        "  tickets verified: 2\n  first verify passed: 0 of 2 (0%); 1 of 2 (50%) when a fail that is accepted-after-head-moved counts as a pass\n".to_string(),
        "  tickets with a verify fail: 2 (1 without accepted-after-head-moved)\n".to_string(),
        "  verify fails: 2 (1 without accepted-after-head-moved)\n".to_string(),
        format!("  review: 1\n    #{t1} http://"),
        format!("  accepted-after-head-moved: 1\n    #{t2} http://"),
        format!("  #{t1}: 1 fail(s) http://"),
        "  questions to the owner: 0\n".to_string(),
        "  reassigned tickets: 0\n  dropped tickets: 0\n  closes: merged 1, owner-merged 1\n"
            .to_string(),
        "  assignment to first done: median 0.0 h, p90 0.0 h (2 ticket(s))\n".to_string(),
        "  first done to close: median 0.0 h, p90 0.0 h (2 ticket(s))\n".to_string(),
    ] {
        assert!(text.contains(&want), "missing {want:?} in:\n{text}");
    }
    // the accepted fail is no head-moved rework, and the text names no agent
    assert!(!text.contains("  head-moved:"), "{text}");
    assert!(!text.contains(&b.name) && !text.contains(&c.name), "{text}");

    // the JSON has the same numbers, and the URL and the agents of each event
    let r = report(&retro(&fake, &c.dir, &["--json"]));
    assert_eq!(
        (&r["tickets"], &r["delivered"]),
        (&Value::from(2), &Value::from(2))
    );
    assert_eq!(
        r["verify"],
        serde_json::json!({"verified": 2, "first_pass": 0, "first_pass_or_accepted": 1,
                           "tickets_with_fail": 2, "fails": 2,
                           "rework_tickets": 1, "rework_fails": 1})
    );
    let classes: Vec<(&str, u64)> = r["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["class"].as_str().unwrap(), c["count"].as_u64().unwrap()))
        .collect();
    assert_eq!(classes, [("review", 1), ("accepted-after-head-moved", 1)]);
    let fail = &r["classes"][0]["events"][0];
    assert_eq!(fail["issue"], t1);
    assert_eq!(
        (&fail["from"], &fail["to"]),
        (&Value::from(c.name.as_str()), &Value::from(b.name.as_str()))
    );
    // the URL is the comment of the failed verify
    let fail_comment = fake
        .comments_of(t1)
        .into_iter()
        .find(|(_, _, body)| body.contains("verify: fail"))
        .map(|(id, _, _)| id)
        .unwrap();
    assert!(
        fail["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("#issuecomment-{fail_comment}")),
        "{fail}"
    );
    assert_eq!(r["rounds"].as_array().unwrap().len(), 1);
    assert_eq!(r["rounds"][0]["issue"], t1);
    assert_eq!(
        r["flow"]["closes"],
        serde_json::json!({"merged": 1, "owner-merged": 1})
    );
    assert_eq!(r["flow"]["assign_to_done"]["tickets"], 2);
    assert_eq!(r["period"]["outside_window"], 0);

    // the owner reads the same report from a config dir without an agent
    let owner = fake.owner_dir();
    let o2 = retro(&fake, owner.path(), &["--json"]);
    assert_eq!(o2.code, 0, "{}", o2.all());
    assert_eq!(report(&o2), r);
    // and a dev agent may read it too: the comments are open to every reader of the board
    assert_eq!(report(&retro(&fake, &b.dir, &["--json"])), r);
    no_unhandled(&fake);
}

#[test]
fn a_comment_that_only_looks_like_a_signed_message_counts_nowhere() {
    let (fake, c, b) = crew();
    let (t1, t2) = two_tickets(&fake, &c, &b);
    let before = report(&retro(&fake, &c.dir, &["--json"]));

    let now = crewboard::clock::now_epoch();
    let fail = "verify: fail\nfailed: CI of abc is not green: build\n";
    // a foreign account, without and with a crew header
    fake.comment_at(t1, "mallory", fail, now, now);
    let forged = format!(
        "<!-- crew v=1 from={} to={} kind=report re=none ts={} sig=AAAA -->\n**{} -> {}** · report\n\n{fail}",
        c.name,
        b.name,
        crewboard::clock::format_ts(now),
        c.name,
        b.name
    );
    fake.comment_at(t1, "mallory", &forged, now, now);
    // the account of the crew with a header that no key signed
    fake.comment_at(t2, OWNER, &forged, now, now);
    // a copy of a real signed message of ticket 1 on ticket 2: the signature names ticket 1
    let real = fake
        .comments_of(t1)
        .into_iter()
        .find(|(_, _, body)| body.contains("verify: fail") && body.contains(NOTE))
        .map(|(_, _, body)| body)
        .unwrap();
    fake.comment_at(t2, OWNER, &real, now, now);
    // a plain comment of the owner account is the owner's word, not a verify result
    fake.comment_at(t2, OWNER, fail, now, now);
    // a question without a signature on an issue that is no ticket
    let other = fake.ticket_issue("IT retro no ticket", "Goal: none.\n", &["crew:work"]);
    fake.comment_at(other, "mallory", "which API? done-needs-pr", now, now);

    let after = report(&retro(&fake, &c.dir, &["--json"]));
    assert_eq!(after, before);
    assert_eq!(after["verify"]["fails"], 2);
    assert!(
        !after.to_string().contains("ci-red"),
        "the look-alike made a class: {after}"
    );
}

#[test]
fn a_signed_message_with_a_time_outside_the_replay_window_counts_once() {
    // GitHub may stamp a comment late, or the clock of a host is wrong: the message is the
    // sender's own, so the retro counts it. A copy of it does not count again.
    let (fake, c, b) = crew();
    let (t1, _t2) = two_tickets(&fake, &c, &b);
    let before = report(&retro(&fake, &c.dir, &["--json"]));
    assert_eq!(before["andon"]["questions_coordinator"]["count"], 0);

    let old = crewboard::clock::now_epoch() - 2 * 3600;
    let q = "which base branch?";
    let id = fake.signed_with_ts(&b.key, &b.name, "coordinator", Kind::Question, t1, q, old);
    // the same signed text again: a copy
    let now = crewboard::clock::now_epoch();
    fake.comment_at(t1, OWNER, &fake.comment_body(id), now, now);
    // a signature of a key that the crew does not trust, with the same wrong time
    let stranger = crewboard::protocol::signature::generate_key().unwrap();
    fake.signed_with_ts(
        &stranger,
        &b.name,
        "coordinator",
        Kind::Question,
        t1,
        q,
        old - 1,
    );

    let o = retro(&fake, &c.dir, &[]);
    assert!(
        o.stdout
            .contains("1 message(s) with a signed time outside the replay window count too"),
        "{}",
        o.all()
    );
    let after = report(&retro(&fake, &c.dir, &["--json"]));
    assert_eq!(after["period"]["outside_window"], 1);
    assert_eq!(after["andon"]["questions_coordinator"]["count"], 1);
    assert!(
        after["andon"]["questions_coordinator"]["events"][0]["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("#issuecomment-{id}"))
    );
    assert_eq!(
        after["period"]["messages"].as_u64().unwrap(),
        before["period"]["messages"].as_u64().unwrap() + 1
    );
}

#[test]
fn since_reads_the_changed_comments_and_then_the_whole_log_of_each_ticket() {
    let (fake, c, b) = crew();
    let (t1, t2) = two_tickets(&fake, &c, &b);
    let full = retro(&fake, &c.dir, &["--json"]);

    // without --since: the feed of the repo, no read of a single issue
    let from = fake.requests().len();
    retro(&fake, &c.dir, &["--json"]);
    let all: Vec<Req> = fake.requests()[from..].to_vec();
    assert!(all.iter().all(is_read));
    let feed = |r: &&Req| r.path.contains("/issues/comments");
    let of_issue = |r: &&Req, n: u64| r.path.contains(&format!("/issues/{n}/comments"));
    assert_eq!(all.iter().filter(feed).count(), 1, "one page of the feed");
    assert!(!all.iter().any(|r| of_issue(&r, t1) || of_issue(&r, t2)));
    // the count of this board: a change of it needs a reason (docs/design/spec/retro.md#cost)
    assert_eq!(
        all.len(),
        READS,
        "{:#?}",
        all.iter().map(|r| &r.path).collect::<Vec<_>>()
    );

    // --since: the feed since then, and each touched ticket in full
    let from = fake.requests().len();
    let o = retro(&fake, &c.dir, &["--json", "--since", "1h"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let since: Vec<Req> = fake.requests()[from..].to_vec();
    assert_eq!(since.iter().filter(feed).count(), 1);
    assert_eq!(since.iter().filter(|r| of_issue(r, t1)).count(), 1);
    assert_eq!(since.iter().filter(|r| of_issue(r, t2)).count(), 1);
    assert_eq!(since.len(), READS + 2);
    let (a, b_) = (report(&o), report(&full));
    assert_eq!(a, b_, "the same tickets give the same report");
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert!(v["since"].is_string(), "{v}");

    // a time after every message: no ticket, exit 1
    let later = crewboard::clock::format_ts(crewboard::clock::now_epoch() + 3600);
    let o = retro(&fake, &c.dir, &["--since", &later]);
    assert_eq!(o.code, 1, "{}", o.all());
    assert!(o.stdout.contains(&format!("Since {later}:")), "{}", o.all());
    no_unhandled(&fake);
}

/// The reads of `retro` without `--since` on a board whose feed fits one page: the login of
/// the token, the board, AUTHORITY.md, the registry issue, the agent items and the feed.
const READS: usize = 6;
