//! `mine` decides on owned work and `take` prints the next steps, against a fake
//! GitHub. `mine` exits 1 while every owned ticket only waits (CI of the PR head runs, no
//! answer yet, Review) and 0 when a ticket needs the agent now.

use crate::support::board::{Agent, FakeBoard, REPO};
use crate::support::cli::{crew, run_ok};

fn ticket(fake: &FakeBoard, title: &str, extra: &str) -> u64 {
    let body = format!(
        "Goal: integration test ticket.\n\n**Gate:** the PR of this ticket is open and closes it.\nCI: none\n{extra}"
    );
    fake.ticket_issue(title, &body, &["crew:work"])
}

/// `mine` of the agent: (exit code, stdout). `--quiet` must print nothing and give the same
/// exit code.
fn mine(fake: &FakeBoard, who: &Agent) -> (i32, String) {
    let q = fake.run(&who.dir, &["mine", "--quiet"]);
    assert!(q.stdout.is_empty(), "--quiet prints nothing: {}", q.all());
    let o = fake.run(&who.dir, &["mine"]);
    assert_eq!(
        o.code,
        q.code,
        "same verdict with and without --quiet: {}",
        o.all()
    );
    (o.code, o.stdout)
}

/// `mine --json` of the agent: (exit code, parsed object). `--quiet` and `--json` conflict.
fn mine_json(fake: &FakeBoard, who: &Agent) -> (i32, serde_json::Value) {
    let o = fake.run(&who.dir, &["mine", "--json"]);
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim())
        .unwrap_or_else(|e| panic!("mine --json prints one JSON object ({e}): {}", o.all()));
    assert!(v.is_object(), "{v}");
    (o.code, v)
}

fn taken(fake: &FakeBoard, c: &Agent, b: &Agent, t: u64) {
    let ts = t.to_string();
    run_ok(fake, c, &["assign", &ts, "--to", &b.name]);
    run_ok(fake, b, &["take", &ts]);
}

#[test]
fn issue_145_mine_waits_while_the_ci_of_the_pr_head_runs_and_wakes_on_its_result() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT mine ci", "");
    taken(&fake, &c, &b, t);
    // In Progress without a PR: work
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("work: no PR yet"), "{out}");
    // a PR whose CI still runs: nothing to do now
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run_running(9001, &pr.head);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("nothing to do"), "{out}");
    assert!(out.contains("is still running"), "{out}");
    // the run fails: work
    fake.complete_run(9001, "failure");
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("is red"), "{out}");
    // a fix is pushed and its CI runs: wait again; it passes: work (deliver)
    let head = fake.push(&pr.branch);
    fake.workflow_run_running(9002, &head);
    assert_eq!(mine(&fake, &b).0, 1);
    fake.complete_run(9002, "success");
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("is green"), "{out}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn issue_145_review_waits_and_a_failed_verify_is_work_until_the_head_moves() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT mine verify", "");
    let ts = t.to_string();
    taken(&fake, &c, &b, t);
    let pr = fake.pr_for(t, &b.name);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
    );
    // Review: the coordinator has the next step
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Review, waits for the coordinator"), "{out}");
    // the head moved after done: verify fails, the ticket is In Progress again; its head is
    // the one that verify saw, so the agent has work
    let head = fake.push(&pr.branch);
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify FAIL"), "{}", o.all());
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("verify failed"), "{out}");
    // the agent pushes a fix and its CI runs: it waits
    let fixed = fake.push(&pr.branch);
    assert_ne!(fixed, head);
    fake.workflow_run_running(9101, &fixed);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(!out.contains("verify failed"), "{out}");
}

#[test]
fn issue_145_a_block_or_a_question_waits_until_an_answer_comes() {
    let (fake, c, b) = crew();
    let t = ticket(&fake, "IT mine block", "");
    let ts = t.to_string();
    taken(&fake, &c, &b, t);
    run_ok(&fake, &b, &["block", &ts, "--why", "waits for a decision"]);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Blocked, no answer yet"), "{out}");
    let _ = fake.poll_json(&c);
    run_ok(&fake, &c, &["unblock", &ts]);
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains(&format!("crewboard take {t}")), "{out}");

    // a question: `ask` blocks the ticket; only an owner message answers it
    let q = ticket(&fake, "IT mine ask", "");
    let qs = q.to_string();
    run_ok(&fake, &b, &["take", &ts]);
    run_ok(&fake, &b, &["block", &ts, "--why", "hold"]);
    taken(&fake, &c, &b, q);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["ask", &qs, "--body", "May I change the layout?"],
    );
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("Blocked, waits for the owner"), "{out}");
    fake.owner_comment(q, "Yes, change it.");
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("Blocked, answered"), "{out}");
    assert!(out.contains(&format!("crewboard take {q}")), "{out}");
}

#[test]
fn issue_145_an_agent_without_tickets_has_nothing_to_do() {
    let (fake, _c, b) = crew();
    let (code, out) = mine(&fake, &b);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no owned ticket"), "{out}");
}

#[test]
fn take_prints_the_branch_and_the_pr_command_with_the_keyword_of_the_route() {
    let (fake, c, b) = crew();
    // the default prefix (no AUTHORITY.md) and the normal route: Closes
    let t = ticket(&fake, "IT take: next steps!", "");
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    let o = run_ok(&fake, &b, &["take", &ts]);
    let branch = format!("crew/{}/{t}-it-take-next-steps", b.name);
    assert!(
        o.stdout.contains(&format!("branch: {branch}\n")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!(
            "gh pr create --repo {REPO} --head '{branch}' --title 'IT take: next steps!' --body 'Closes #{t}'"
        )),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!("crewboard done {t} --pr")),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("Refs #"), "{}", o.stdout);
    run_ok(&fake, &b, &["block", &ts, "--why", "hold"]);

    // the prefix of AUTHORITY.md and the owner route: Refs and the patch path
    fake.set_file(
        REPO,
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\ndev_branch_prefix: team/\n---\n\n# text\n",
    );
    let w = ticket(&fake, "IT workflow change", "Route: owner-workflow\n");
    let ws = w.to_string();
    run_ok(&fake, &c, &["assign", &ws, "--to", &b.name]);
    let o = run_ok(&fake, &b, &["take", &ws]);
    let branch = format!("team/{}/{w}-it-workflow-change", b.name);
    assert!(
        o.stdout.contains(&format!("branch: {branch}\n")),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains(&format!("--body 'Refs #{w}'")),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("Closes #"), "{}", o.stdout);
    assert!(
        o.stdout
            .contains(&format!("docs/patches/{w}-workflow.patch")),
        "{}",
        o.stdout
    );
}

#[test]
fn mine_json_matches_text_verdicts() {
    let (fake, c, b) = crew();
    // no owned ticket: exit 1, no verdict, no first ticket
    let (code, v) = mine_json(&fake, &b);
    let (tcode, text) = mine(&fake, &b);
    assert_eq!((code, tcode), (1, 1), "{v} / {text}");
    assert_eq!(v["needs_agent"], false);
    assert_eq!(v["agent"], b.name.as_str());
    assert!(v["rows"].as_array().unwrap().is_empty(), "{v}");
    assert!(v["now"].as_array().unwrap().is_empty(), "{v}");
    assert!(v["first_ticket"].is_null(), "{v}");
    assert!(text.contains("no owned ticket"), "{text}");

    // the same fixtures as the text test: no PR, CI runs, red, green
    let t = ticket(&fake, "IT mine json", "");
    taken(&fake, &c, &b, t);
    let agree = |fake: &FakeBoard, code: i32, word: &str, pr: bool| {
        let (jcode, v) = mine_json(fake, &b);
        let (tcode, text) = mine(fake, &b);
        assert_eq!((jcode, tcode), (code, code), "{v} / {text}");
        assert_eq!(v["needs_agent"], code == 0, "{v}");
        let now = &v["now"][0];
        assert_eq!(now["verdict"], word, "{v}");
        // both forms print the same word and the same text for the ticket
        let line = format!("{word}: {}", now["text"].as_str().unwrap());
        assert!(text.contains(&line), "{line} not in {text}");
        assert_eq!(now["pr"].is_string(), pr, "{v}");
        let row = &v["rows"][0];
        assert_eq!(row["status"], "In Progress", "{v}");
        assert_eq!(row["issue"], t, "{v}");
        assert_eq!(row["repo"], REPO, "{v}");
        assert_eq!(row["ticket"], format!("{REPO}#{t}"), "{v}");
        assert!(row["parked_for"].is_null(), "{v}");
        assert_eq!(v["first_ticket"], format!("{REPO}#{t}"), "{v}");
        v
    };
    agree(&fake, 0, "work", false);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run_running(9301, &pr.head);
    let v = agree(&fake, 1, "wait", true);
    assert_eq!(v["now"][0]["pr"], format!("{REPO}#{}", pr.number), "{v}");
    fake.complete_run(9301, "failure");
    agree(&fake, 0, "work", true);
    let head = fake.push(&pr.branch);
    fake.workflow_run_running(9302, &head);
    agree(&fake, 1, "wait", true);
    fake.complete_run(9302, "success");
    agree(&fake, 0, "work", true);
    // the twin: the two forms cannot be mixed
    let o = fake.run(&b.dir, &["mine", "--json", "--quiet"]);
    assert_eq!(o.code, 64, "{}", o.all());
}

#[test]
fn mine_json_first_ticket_prefers_in_progress() {
    let (fake, c, b) = crew();
    // a Ready ticket with the lower number and the lower Order, and an In Progress ticket
    let ready = ticket(&fake, "IT mine json ready", "");
    let busy = ticket(&fake, "IT mine json busy", "");
    run_ok(&fake, &c, &["assign", &ready.to_string(), "--to", &b.name]);
    taken(&fake, &c, &b, busy);
    let (code, v) = mine_json(&fake, &b);
    assert_eq!(code, 0, "{v}");
    let rows = v["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{v}");
    assert_eq!(v["first_ticket"], format!("{REPO}#{busy}"), "{v}");
    // the twin: with no In Progress ticket the first row is the first ticket
    run_ok(&fake, &b, &["block", &busy.to_string(), "--why", "waits"]);
    let (_, v) = mine_json(&fake, &b);
    let first = v["rows"][0]["ticket"].clone();
    assert_eq!(v["first_ticket"], first, "{v}");
    assert_eq!(v["first_ticket"], format!("{REPO}#{ready}"), "{v}");
}

#[test]
fn a_token_without_read_project_gives_one_line_with_the_fix_and_exit_4() {
    let fake = FakeBoard::new();
    let who = fake.agent("claude@fake-a", crewboard::rules::Role::Dev);
    let msg = "Your token has not been granted the required scopes to execute this query. The \
               'projectV2' field requires one of the following scopes: ['read:project'], but \
               your token has only been granted the: ['gist', 'read:org', 'repo', 'workflow'] \
               scopes. Please modify your token's scopes at: https://github.com/settings/tokens.";
    let errs: Vec<serde_json::Value> = (0..10)
        .map(|_| serde_json::json!({"type": "INSUFFICIENT_SCOPES", "message": msg}))
        .collect();
    fake.fail_board_errors(serde_json::Value::Array(errs));
    let o = fake.run(&who.dir, &["mine"]);
    assert_eq!(o.code, 4, "{}", o.all());
    let err = o.stderr.trim();
    assert_eq!(
        err.lines().count(),
        1,
        "one line, not ten copies of one sentence: {err}"
    );
    assert!(
        err.contains("lacks the scope read:project (it has: gist, read:org, repo, workflow)")
            && err.contains("gh auth refresh -h github.com -s project"),
        "{err}"
    );
    assert!(err.len() < 300, "{err}");
}
