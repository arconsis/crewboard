//! Request budget: how many reads each command that loads tickets makes against the fake
//! GitHub. The test pins the number of reads, not their order or URLs. The loaders (ticket
//! discovery, ticket loading) are shared code; a change there must not add a read to any
//! command. A test fails with the command's name when the number moves. A write is not counted
//! here (`support::cli::writes` does that); a read is any request that is not a write.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REPO};
use crate::support::cli::{GATE, crew, run_ok};

/// A request that does not write: a GET, or a GraphQL query.
fn reads(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| {
            r.method == "GET" || (r.path.starts_with("/graphql") && !r.body.contains("mutation"))
        })
        .count()
}

/// Run one command and return the number of reads it made. Exit 0 or 1 (`mine` waits with 1).
fn spent(fake: &FakeBoard, who: &Agent, args: &[&str]) -> usize {
    let before = reads(fake);
    let o = fake.run(&who.dir, args);
    assert!(o.code <= 1, "{args:?}: {}", o.all());
    reads(fake) - before
}

fn ticket(fake: &FakeBoard, title: &str) -> String {
    fake.ticket_issue(
        title,
        &format!("Goal: budget test.\n\n{GATE}CI: none\n"),
        &["crew:work"],
    )
    .to_string()
}

#[test]
fn commands_that_load_tickets_make_the_same_number_of_reads() {
    let (fake, c, b) = crew();
    // one ticket in Review (delivered), one in progress, one ready, one closed by the owner
    let t1 = ticket(&fake, "IT budget review");
    let t2 = ticket(&fake, "IT budget progress");
    let t3 = ticket(&fake, "IT budget ready");
    run_ok(&fake, &c, &["assign", &t1, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &t1]);
    let pr = fake.pr_for(t1.parse().unwrap(), &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &t1, "--pr", &pr.url, "--sha", &pr.head],
    );
    run_ok(&fake, &c, &["assign", &t2, "--to", &b.name]);
    run_ok(&fake, &c, &["assign", &t3, "--to", &b.name]);
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &t2]);

    // each command runs once, in this order, on the same board; the cache of the agent
    // carries over, as it does in use
    let mut got = vec![("poll (dev)", spent(&fake, &b, &["poll"]))];
    got.push(("poll (dev, warm)", spent(&fake, &b, &["poll"])));
    got.push(("poll (coordinator)", spent(&fake, &c, &["poll"])));
    got.push(("mine", spent(&fake, &b, &["mine"])));
    got.push(("resume", spent(&fake, &b, &["resume"])));
    got.push(("status (dev)", spent(&fake, &b, &["status"])));
    got.push(("status (coordinator)", spent(&fake, &c, &["status"])));
    got.push(("status --json", spent(&fake, &c, &["status", "--json"])));
    got.push(("agents", spent(&fake, &c, &["status", "--agents"])));
    got.push((
        "audit",
        spent(&fake, &c, &["audit", "--initiative", INITIATIVE]),
    ));
    got.push(("prune", spent(&fake, &c, &["prune"])));
    got.push((
        "ask (load ticket)",
        spent(&fake, &b, &["ask", &t2, "--body", "Which layout?"]),
    ));
    got.push(("take (load ticket)", spent(&fake, &b, &["take", &t3])));
    got.push((
        "verify (load ticket)",
        spent(
            &fake,
            &c,
            &["verify", &t1, "--fail", "--body", "gate not met"],
        ),
    ));
    // The reads of each command above; a change here needs a reason.
    let want: [(&str, usize); 14] = [
        // The context reads AUTHORITY.md once (for `former_repos`) and keeps the text. A command
        // that read the file before reads it once, as often as before (status, take, verify,
        // done, assign; `resume` and the tick one or more times less). A command that did not
        // read it before pays +1 (poll, mine, agents, audit, prune, ask, watch, acted).
        ("poll (dev)", 7),
        ("poll (dev, warm)", 7),
        ("poll (coordinator)", 8), // 11 before: the earlier `assign` calls now fill the ticket index
        ("mine", 10),
        // 29: `resume` read AUTHORITY.md three times before (selftest forgery, selftest, start);
        // it reads it once now; and `start` reads the default branch of the repo and its
        // protection (2 reads, `domain::branch_guard`), and the rules of the branch when
        // `protected` is false (1 read)
        ("resume", 29),
        ("status (dev)", 23),
        ("status (coordinator)", 23),
        ("status --json", 23),
        ("agents", 10),
        ("audit", 12),
        ("prune", 13),
        ("ask (load ticket)", 12), // +1: the Status is read again before the question is posted
        ("take (load ticket)", 12), // +1: the same read before the ack
        ("verify (load ticket)", 18), // +1: the Status is read again before the write
    ];
    assert_eq!(got, want.to_vec(), "reads per command: (command, reads)");
}

/// The headless coordinator: `tick` finds the Review ticket through the ticket discovery, then
/// loads it.
#[test]
fn the_headless_tick_makes_the_same_number_of_reads() {
    use crate::support::headless::{Hub, Scenario};
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: true,
    });
    let hreads = |h: &Hub| {
        h.requests()
            .iter()
            .filter(|r| {
                r.method == "GET"
                    || (r.path.starts_with("/graphql") && !r.body.contains("mutation"))
            })
            .count()
    };
    let before = hreads(&h);
    let o = h.headless(&["tick"]);
    assert!(o.code <= 1, "{}", o.all());
    assert_eq!(hreads(&h) - before, 41, "reads of the headless tick"); // +1: Status read again before the write; AUTHORITY.md is read once by the context (42 before: twice)
}

/// `assign`, `done`, `say`, one `watch` cycle (the `collect` step) and one `acted` path, on a board of
/// their own so that the rows above keep their cache state.
#[test]
fn assign_done_watch_and_acted_make_the_same_number_of_reads() {
    let (fake, c, b) = crew();
    let t1 = ticket(&fake, "IT budget assign");
    let t2 = ticket(&fake, "IT budget watch");
    let mut got = vec![(
        "assign",
        spent(&fake, &c, &["assign", &t1, "--to", &b.name]),
    )];
    let _ = fake.poll_json(&b);
    run_ok(&fake, &b, &["take", &t1]);
    let pr = fake.pr_for(t1.parse().unwrap(), &b.name);
    let _ = fake.poll_json(&b);
    got.push((
        "done",
        spent(
            &fake,
            &b,
            &["done", &t1, "--pr", &pr.url, "--sha", &pr.head],
        ),
    ));
    // an assignment is pending, so one `watch` cycle ends at once
    run_ok(&fake, &c, &["assign", &t2, "--to", &b.name]);
    got.push((
        "watch (one cycle)",
        spent(
            &fake,
            &b,
            &["watch", "--timeout", "30s", "--interval", "1s"],
        ),
    ));
    // `acted` on one event that needs an action: one comment list of the ticket
    let ts = crewboard::clock::now_epoch() - 120;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("events.txt");
    std::fs::write(
        &file,
        format!(
            "== Events (crewboard poll, x):\n== [1] MESSAGE ==\nfrom: sender  to: {}  kind: question  re: none\nwhere: {REPO}#{t2} comment 1\nts: {}\nurl: u\n\nquestion text\n\n== Your tickets (crewboard mine):\n",
            b.name,
            crewboard::clock::format_ts(ts)
        ),
    )
    .unwrap();
    got.push((
        "acted",
        spent(
            &fake,
            &b,
            &[
                "acted",
                "--events-file",
                file.to_str().unwrap(),
                "--since",
                &crewboard::clock::format_ts(ts),
            ],
        ),
    ));
    // `say`: one info note from the dev agent to the coordinator on the delivered ticket
    got.push((
        "say",
        spent(
            &fake,
            &b,
            &[
                "say",
                &t1,
                "--to",
                &c.name,
                "--kind",
                "info",
                "--body",
                "budget note",
            ],
        ),
    ));
    let want: [(&str, usize); 5] = [
        ("assign", 17), // +5: the default Order reads the open tickets of the agent (ticket index, feed); +1: the Status is read again before the write
        ("done", 22),   // +1 compare read (head behind base), +1 Status read again before the write
        ("watch (one cycle)", 7),
        ("acted", 6),
        ("say", 7),
    ];
    assert_eq!(got, want.to_vec(), "reads per command: (command, reads)");
}
