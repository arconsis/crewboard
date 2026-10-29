//! The repo moves to another owner, and the messages that were signed under the
//! old name still count while AUTHORITY.md lists it with a cutoff.
//!
//! The crew works under `fake-owner/fake-repo` (the old name). `rename_repo` then makes GitHub
//! report `new-owner/new-repo` everywhere, and the agents' config names the new repo. Every
//! earlier message carries the old name inside its signature.

use crate::support::authority::{AUTHORITY, NEEDS_OWNER_ALL, authority_with};
use crate::support::board::{Agent, FakeBoard, Pull, REPO};
use crate::support::cli::{GATE, crew, run_ok};
use crewboard::clock::{format_ts, now_epoch};

const NEW: &str = "new-owner/new-repo";

/// A crew with a ticket that is assigned and taken under the old name (its PR is open), then
/// moved. `until` is the cutoff of `former_repos` (`None`: the owner has not listed the old
/// name). Returns the ticket number and its PR.
fn moved(until: Option<i64>) -> (FakeBoard, Agent, Agent, u64, Pull) {
    moved_with(until, false)
}

/// [`moved`]; with `delivered` the dev runs `done` before the move, so the signed `done` names
/// the PR under the old name.
fn moved_with(until: Option<i64>, delivered: bool) -> (FakeBoard, Agent, Agent, u64, Pull) {
    let (fake, c, b) = crew();
    fake.file("AUTHORITY.md", AUTHORITY);
    let t = fake.ticket_issue("IT move", &format!("Goal: x.\n\n{GATE}"), &["crew:work"]);
    let ts = t.to_string();
    run_ok(&fake, &c, &["assign", &ts, "--to", &b.name]);
    run_ok(&fake, &b, &["take", &ts]);
    let pr = fake.pr_for(t, &b.name);
    fake.workflow_run(9001, &pr.head, "push", "success");
    let _ = fake.poll_json(&b);
    if delivered {
        run_ok(
            &fake,
            &b,
            &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        );
    }

    fake.rename_repo(NEW);
    fake.retarget_agent(&c, NEW);
    fake.retarget_agent(&b, NEW);
    let extra = until
        .map(|u| {
            format!(
                "former_repos:\n  - {{ name: {REPO}, until: {} }}\n",
                format_ts(u)
            )
        })
        .unwrap_or_default();
    fake.file_in(
        NEW,
        "AUTHORITY.md",
        &authority_with("fake-owner", NEEDS_OWNER_ALL, &extra),
    );
    (fake, c, b, t, pr)
}

fn status(fake: &FakeBoard, who: &Agent) -> String {
    run_ok(fake, who, &["status"]).stdout
}

#[test]
fn with_the_old_name_listed_the_registry_and_the_ticket_messages_still_count() {
    let (fake, c, b, t, pr) = moved(Some(now_epoch() + 3600));
    let s = status(&fake, &b);
    // registry: both agents are trusted (join and approval were signed under the old name)
    assert!(s.contains("agents (2):"), "{s}");
    assert!(s.contains("codex@fake-c           trusted"), "{s}");
    assert!(s.contains("hermes@fake-b          trusted"), "{s}");
    assert!(s.contains("security events: 0"), "{s}");
    // the ticket: one row under the new name, none under the old one
    assert!(s.contains(&format!("{NEW}#{t}")), "{s}");
    assert!(!s.contains(&format!("{REPO}#{t}")), "{s}");
    assert!(s.contains("tickets (1):"), "{s}");
    // the signed assignment of the dev is found
    let mine = fake.run(&b.dir, &["mine"]).stdout;
    assert!(mine.contains(&format!("{NEW}#{t}")), "{mine}");
    assert!(mine.contains("deliver with `done`"), "{mine}");
    // `done` needs that assignment and the Gate of the ticket: the dev delivers under the new
    // name, and the coordinator judges it with the assignment signed under the old name
    let ts = t.to_string();
    let pr_url = format!("https://github.com/{NEW}/pull/{}", pr.number);
    let _ = fake.poll_json(&b);
    run_ok(
        &fake,
        &b,
        &["done", &ts, "--pr", &pr_url, "--sha", &pr.head],
    );
    let o = run_ok(&fake, &c, &["verify", &ts]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn without_the_old_name_in_the_authority_file_every_earlier_message_is_refused() {
    let (fake, c, b, t, _) = moved(None);
    let s = status(&fake, &b);
    assert!(s.contains("agents (0):"), "{s}");
    assert!(s.contains("bad-signature"), "{s}");
    assert!(!s.contains("trusted"), "{s}");
    // the coordinator cannot verify: no signed `done` is found
    let o = fake.run(&c.dir, &["verify", &t.to_string()]);
    assert_ne!(o.code, 0, "{}", o.all());
}

#[test]
fn a_cutoff_before_the_messages_does_not_help() {
    // the messages of the crew are signed now; the cutoff is an hour ago
    let (fake, _c, b, _t, _) = moved(Some(now_epoch() - 3600));
    let s = status(&fake, &b);
    assert!(s.contains("agents (0):"), "{s}");
    assert!(s.contains("bad-signature"), "{s}");
}

/// The cells of the table row that starts with `key`.
fn row(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .find(|l| l.starts_with(key))
        .unwrap_or_else(|| panic!("no row {key} in:\n{text}"))
        .split_whitespace()
        .map(String::from)
        .collect()
}

fn dry_run(fake: &FakeBoard) -> String {
    let od = fake.owner_dir_for(NEW);
    let o = fake.run(od.path(), &["doctor", "--repo-move"]);
    assert_eq!(o.code, 0, "{}", o.all());
    o.stdout
}

#[test]
fn the_dry_run_counts_valid_messages_per_issue_under_each_name() {
    let (fake, _c, _b, t, _) = moved(Some(now_epoch() + 3600));
    let before = fake.requests().len();
    let o = dry_run(&fake);
    assert!(o.contains("current name new-owner/new-repo"), "{o}");
    assert!(
        o.contains(&format!("former {REPO} (until ")),
        "the former name has its own column: {o}"
    );
    // columns: issue, messages, current, former, accepted, none. The ticket: three messages
    // (assign, take and the like) were signed under the old name.
    let r = row(&o, &format!("{NEW}#{t}"));
    assert_eq!(r[1..], ["3", "0", "3", "3", "0"], "{o}");
    // the registry issue: two joins under the old name
    let reg = row(&o, &format!("{NEW}#1"));
    assert_eq!(reg[2..], ["0", "2", "2", "0"], "{o}");
    assert!(o.contains("messages that verify under no name: 0"), "{o}");
    // read only: nothing was written
    assert!(
        fake.requests()[before..]
            .iter()
            .all(|r| r.method == "GET" || r.path == "/graphql" && !r.body.contains("mutation")),
        "the dry run wrote"
    );
}

#[test]
fn the_dry_run_lists_what_verifies_under_no_name() {
    // the old name is not listed: every message of the crew is an orphan
    let (fake, _c, _b, t, _) = moved(None);
    let o = dry_run(&fake);
    assert!(o.contains("former_repos: none in AUTHORITY.md"), "{o}");
    assert!(!o.contains("messages that verify under no name: 0"), "{o}");
    assert!(
        o.contains(&format!("{NEW}#{t} comment")) && o.contains("signature fails under every name"),
        "{o}"
    );
    // listed, but the cutoff is before the messages: the reason says so
    let (fake, _c, _b, _t, _) = moved(Some(now_epoch() - 3600));
    let o = dry_run(&fake);
    assert!(
        o.contains("signed under a former name, but at or after its cutoff"),
        "{o}"
    );
}

#[test]
fn a_host_switches_with_join_and_start_refuses_the_new_name_before() {
    let (fake, _c, b, t, _) = moved(Some(now_epoch() + 3600));
    // the host config still names the old repo
    let cd = crewboard::config::ConfigDir(b.dir.clone());
    let mut a = cd.load_agent_auto(None).unwrap();
    a.board.repos = vec![REPO.to_string()];
    cd.save_agent(&a).unwrap();
    fake.file_in(
        NEW,
        ".crewboard.toml",
        &FakeBoard::board_toml().replace(REPO, NEW),
    );
    let o = fake.run(&b.dir, &["start"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("board-identity-changed"), "{}", o.all());
    // `join --repo` reads the new file and writes the config
    let o = fake.run(
        &b.dir,
        &[
            "join",
            "--agent",
            &b.name,
            "--runtime",
            "fake/it",
            "--repo",
            NEW,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let a = cd.load_agent_auto(None).unwrap();
    assert_eq!(a.board.repos, vec![NEW.to_string()]);
    let o = fake.run(&b.dir, &["status"]);
    assert!(o.stdout.contains(&format!("{NEW}#{t}")), "{}", o.all());
}

#[test]
fn a_done_signed_before_the_move_is_verified_after_it_with_the_pr_under_the_new_name() {
    // `done` names the PR as https://github.com/<old name>/pull/<n>
    let (fake, c, _b, t, pr) = moved_with(Some(now_epoch() + 3600), true);
    assert!(pr.url.contains(REPO), "{}", pr.url);
    let o = run_ok(&fake, &c, &["verify", &t.to_string()]);
    assert!(o.stdout.contains("verify PASS"), "{}", o.all());
}

#[test]
fn the_same_done_is_refused_when_the_old_name_is_not_listed() {
    let (fake, c, _b, t, _) = moved_with(None, true);
    let o = fake.run(&c.dir, &["verify", &t.to_string()]);
    assert_ne!(o.code, 0, "{}", o.all());
}

#[test]
fn a_failed_read_of_the_authority_file_warns_once_where_the_context_loads_and_fails_closed() {
    let (fake, _c, b, _t, _) = moved(Some(now_epoch() + 3600));
    // control: with the file readable the earlier messages count
    assert!(status(&fake, &b).contains("agents (2):"));
    fake.fail_file(NEW, "AUTHORITY.md", 500);
    let o = fake.run(&b.dir, &["status"]);
    let warnings = o.stderr.matches("former repo names not read").count();
    assert_eq!(warnings, 1, "{}", o.all());
    // no former names: the messages signed under the old name stay invalid
    assert!(o.stdout.contains("agents (0):"), "{}", o.all());
    assert!(o.stdout.contains("bad-signature"), "{}", o.all());
}
