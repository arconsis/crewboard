//! One GitHub account per agent: a contributor joins with its own account and token, the
//! owner's approval binds that account to the agent, and a message of the agent counts only from
//! that account. A shared crew account keeps working beside it. Against the fake GitHub; plain
//! `cargo test`.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, OWNER, REGISTRY, REPO, THREAD, sign};
use crate::support::cli::no_unhandled;
#[cfg(unix)]
use crate::support::cli::{GATE, item_in, run_ok};
use crewboard::clock::now_epoch;
use crewboard::config::ConfigDir;
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use serde_json::Value;
use std::path::Path;

const FILE: &str = ".crewboard.toml";
/// The shared crew account of the board (the coordinator acts as it).
const CREW: &str = "fake-crew";

/// The board file of a crew board: a crew login, no one-account key.
fn crew_toml() -> String {
    FakeBoard::board_toml().replace(
        "insecure_one_account = true\n",
        &format!("crew_login = \"{CREW}\"\n"),
    )
}

/// The board file of a crew where each agent uses its own account: no key at all.
fn own_toml() -> String {
    FakeBoard::board_toml().replace("insecure_one_account = true\n", "")
}

/// A fake board whose default branch holds the crew board file.
fn crew_board() -> FakeBoard {
    board_with(&crew_toml())
}

fn board_with(toml: &str) -> FakeBoard {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, toml);
    fake
}

/// The agent's own copy of the board names the crew login.
fn on_crew_board(a: &Agent) {
    let c = ConfigDir(a.dir.clone());
    let mut cfg = c.load_agent(INITIATIVE).unwrap();
    cfg.board.crew_login = Some(CREW.into());
    cfg.board.insecure_one_account = false;
    c.save_agent(&cfg).unwrap();
}

/// A config dir of the owner with the crew board file.
fn owner_dir() -> tempfile::TempDir {
    owner_dir_with(&crew_toml())
}

fn owner_dir_with(toml: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        ConfigDir(tmp.path().to_path_buf()).board_config_path(INITIATIVE),
        toml,
    )
    .unwrap();
    tmp
}

/// `crewboard approve <name>` by the owner.
fn approve(fake: &FakeBoard, owner: &Path, name: &str) -> crate::support::board::Out {
    fake.set_viewer(OWNER);
    fake.run(owner, &["approve", name, "--initiative", INITIATIVE])
}

/// `join` of a new agent with the token of `login`.
#[cfg(unix)]
fn join_as(fake: &FakeBoard, dir: &Path, name: &str, login: &str) {
    fake.set_viewer(login);
    let o = fake.run(
        dir,
        &[
            "join",
            "--agent",
            name,
            "--runtime",
            "codex",
            "--model",
            "it",
            "--repo",
            REPO,
        ],
    );
    assert_eq!(o.code, 0, "join {name} as {login}: {}", o.all());
}

/// A `gh` for the agent's shell: `gh api user --jq .login` answers `login`.
#[cfg(unix)]
fn gh_says(dir: &Path, login: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(format!("gh-{login}"));
    std::fs::write(&p, format!("#!/bin/sh\necho {login}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p.display().to_string()
}

fn security_of(v: &Value, id: u64) -> Vec<Value> {
    v["security"]
        .as_array()
        .unwrap_or_else(|| panic!("no security list: {v}"))
        .iter()
        .filter(|e| e["comment_id"].as_u64() == Some(id))
        .cloned()
        .collect()
}

#[cfg(unix)]
#[test]
fn two_contributors_join_with_their_own_accounts_get_approved_take_and_deliver() {
    let fake = crew_board();
    let c = fake.agent_as("codex@fake-c", Role::Coordinator, CREW);
    on_crew_board(&c);
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = ("hermes@fake-a", "hermes@fake-b");
    join_as(&fake, dir_a.path(), a, "dev-a");
    join_as(&fake, dir_b.path(), b, "dev-b");
    // the joins are comments of the two accounts
    let joins: Vec<String> = fake
        .comments_of(REGISTRY)
        .into_iter()
        .filter(|(_, _, body)| body.contains("Join request."))
        .map(|(_, login, _)| login)
        .collect();
    assert_eq!(joins, [CREW, "dev-a", "dev-b"]);

    // the owner approves each with its account
    let owner = owner_dir();
    for (name, login) in [(a, "dev-a"), (b, "dev-b")] {
        let o = approve(&fake, owner.path(), name);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains(&format!("account {login}")),
            "{}",
            o.stdout
        );
        assert!(
            fake.comments_of(REGISTRY)
                .iter()
                .any(|(_, l, body)| l == OWNER
                    && body.starts_with(&format!("/crew approve {name} "))
                    && body.contains(&format!("login={login}"))),
            "the approval names the account of the join"
        );
    }

    // start: the token and plain gh must be the agent's own account
    let tools = tempfile::tempdir().unwrap();
    let start =
        |dir: &Path, gh: &str| fake.run_env(dir, dir, &["start"], &[("CREWBOARD_AGENT_GH", gh)]);
    fake.set_viewer("dev-b");
    let o = start(dir_a.path(), &gh_says(tools.path(), "dev-a"));
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("agent-token-not-crew") && o.stderr.contains("the account dev-a"),
        "{}",
        o.all()
    );
    fake.set_viewer("dev-a");
    let o = start(dir_a.path(), &gh_says(tools.path(), "dev-b"));
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("agent-gh-not-crew") && o.stderr.contains("acts as dev-a"),
        "{}",
        o.all()
    );
    let o = start(dir_a.path(), &gh_says(tools.path(), "dev-a"));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("plain `gh` acts as dev-a"), "{}", o.all());
    fake.set_viewer("dev-b");
    let o = start(dir_b.path(), &gh_says(tools.path(), "dev-b"));
    assert_eq!(o.code, 0, "{}", o.all());

    // the coordinator (shared crew account) assigns; each dev takes and delivers as itself
    let body = format!("Goal: accounts ticket.\n\n{GATE}CI: none\n");
    for (name, login, dir) in [(a, "dev-a", dir_a.path()), (b, "dev-b", dir_b.path())] {
        let t = fake.ticket_issue(name, &body, &["crew:work"]);
        let ts = t.to_string();
        fake.set_viewer(CREW);
        let item = item_in(&run_ok(&fake, &c, &["assign", &ts, "--to", name]));
        fake.set_viewer(login);
        let o = fake.run(dir, &["take", &ts]);
        assert_eq!(o.code, 0, "{}", o.all());
        let pr = fake.pr_for(t, name);
        let _ = fake.run(dir, &["poll", "--json"]);
        let o = fake.run(dir, &["done", &ts, "--pr", &pr.url, "--sha", &pr.head]);
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Review"));
    }
    // the signed log agrees with the board for both tickets, and status names the accounts
    fake.set_viewer(CREW);
    let o = run_ok(&fake, &c, &["status", "--json"]);
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["status_log_mismatches"], serde_json::json!([]), "{v}");
    assert_eq!(v["security_event_count"], 0, "{v}");
    for (name, login) in [(a, "dev-a"), (b, "dev-b"), (c.name.as_str(), CREW)] {
        let row = v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{v}"))
            .clone();
        assert_eq!(row["login"], login, "{row}");
        assert_eq!(row["trusted"], true, "{row}");
    }
    no_unhandled(&fake);
}

/// A crew without any shared account: `init` without `--crew-login` and without
/// `--insecure-one-account`, then every agent joins with its own account.
#[cfg(unix)]
#[test]
fn an_own_accounts_board_from_init_to_delivered_tickets() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(
        owner.path(),
        &[
            "init",
            "--initiative",
            INITIATIVE,
            "--repo",
            REPO,
            "--project",
            "15",
            "--no-pr",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("identity: own accounts"), "{}", o.stdout);
    let toml = std::fs::read_to_string(
        ConfigDir(owner.path().to_path_buf()).board_config_path(INITIATIVE),
    )
    .unwrap();
    assert!(
        !toml.contains("crew_login") && !toml.contains("insecure_one_account"),
        "{toml}"
    );
    // the init pull request is merged: the default branch holds the board file
    fake.set_file(REPO, FILE, &toml);

    // an agent with the owner's token cannot join
    let stray = tempfile::tempdir().unwrap();
    fake.set_viewer(OWNER);
    let o = fake.run(
        stray.path(),
        &[
            "join",
            "--agent",
            "claude@fake-o",
            "--runtime",
            "codex",
            "--model",
            "it",
            "--repo",
            REPO,
        ],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("agent-uses-owner-token"), "{}", o.all());

    // three contributors, three accounts
    let agents = [
        ("codex@fake-c", "dev-c", tempfile::tempdir().unwrap()),
        ("hermes@fake-a", "dev-a", tempfile::tempdir().unwrap()),
        ("hermes@fake-b", "dev-b", tempfile::tempdir().unwrap()),
    ];
    for (name, login, dir) in &agents {
        join_as(&fake, dir.path(), name, login);
    }
    for (name, login, _) in &agents {
        fake.set_viewer(OWNER);
        let role = if name.starts_with("codex") {
            "coordinator"
        } else {
            "dev"
        };
        let o = fake.run(
            owner.path(),
            &["approve", name, "--initiative", INITIATIVE, "--role", role],
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains(&format!("account {login}")),
            "{}",
            o.stdout
        );
    }
    let tools = tempfile::tempdir().unwrap();
    for (_, login, dir) in &agents {
        fake.set_viewer(login);
        let o = fake.run_env(
            dir.path(),
            dir.path(),
            &["start"],
            &[("CREWBOARD_AGENT_GH", &gh_says(tools.path(), login))],
        );
        assert_eq!(o.code, 0, "{}", o.all());
    }
    let (coord, coord_login, coord_dir) = (&agents[0].0, agents[0].1, agents[0].2.path());
    let body = format!("Goal: own accounts ticket.\n\n{GATE}CI: none\n");
    for (name, login, dir) in &agents[1..] {
        let t = fake.ticket_issue(name, &body, &["crew:work"]);
        let ts = t.to_string();
        fake.set_viewer(coord_login);
        let o = fake.run(coord_dir, &["assign", &ts, "--to", name]);
        assert_eq!(o.code, 0, "{coord}: {}", o.all());
        let item = item_in(&o);
        fake.set_viewer(login);
        let o = fake.run(dir.path(), &["take", &ts]);
        assert_eq!(o.code, 0, "{}", o.all());
        let pr = fake.pr_for(t, name);
        let _ = fake.run(dir.path(), &["poll", "--json"]);
        let o = fake.run(
            dir.path(),
            &["done", &ts, "--pr", &pr.url, "--sha", &pr.head],
        );
        assert_eq!(o.code, 0, "{}", o.all());
        assert_eq!(fake.item_field(&item, "Status").as_deref(), Some("Review"));
    }
    fake.set_viewer(OWNER);
    let o = fake.run(owner.path(), &["status", "--json"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["status_log_mismatches"], serde_json::json!([]), "{v}");
    assert_eq!(v["security_event_count"], 0, "{v}");
    let logins: Vec<String> = v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["login"].as_str().unwrap_or("-").to_string())
        .collect();
    assert_eq!(logins, ["dev-c", "dev-a", "dev-b"], "{v}");
    assert!(!o.stdout.contains("ONE ACCOUNT"), "{}", o.stdout);
    no_unhandled(&fake);
}

#[test]
fn a_message_posted_from_another_agents_account_is_dropped_and_reported() {
    let fake = crew_board();
    let c = fake.agent_as("codex@fake-c", Role::Coordinator, CREW);
    let a = fake.agent_as("hermes@fake-a", Role::Dev, "dev-a");
    let b = fake.agent_as("hermes@fake-b", Role::Dev, "dev-b");
    for x in [&c, &a, &b] {
        on_crew_board(x);
    }
    // a's key signs a message to the coordinator; b's account posts it
    let text = sign(&a.key, THREAD, &a.name, &c.name, Kind::Info, "from a");
    let t = now_epoch() - 30;
    let wrong = fake.comment_at(THREAD, "dev-b", &text, t, t);
    let text = sign(
        &a.key,
        THREAD,
        &a.name,
        &c.name,
        Kind::Info,
        "from a, own account",
    );
    let right = fake.comment_at(THREAD, "dev-a", &text, t, t);

    fake.set_viewer(CREW);
    let (v, code) = fake.poll_json(&c);
    assert_eq!(code, 0, "{v}");
    let events: Vec<u64> = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["comment_id"].as_u64())
        .collect();
    assert!(events.contains(&right), "the own account delivers: {v}");
    assert!(
        !events.contains(&wrong),
        "another account never delivers: {v}"
    );
    let sec = security_of(&v, wrong);
    assert_eq!(sec.len(), 1, "{v}");
    assert_eq!(sec[0]["reason"], "foreign_author", "{v}");
    assert!(
        sec[0]["detail"]
            .as_str()
            .unwrap()
            .contains("the owner approved hermes@fake-a as the account dev-a"),
        "{v}"
    );
    assert!(security_of(&v, right).is_empty(), "{v}");

    // status lists the same event for the owner
    let owner = owner_dir();
    fake.set_viewer(OWNER);
    let o = fake.run(owner.path(), &["status", "--json"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let s: Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert!(
        s["security_event_list"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["comment_id"] == wrong && e["author"] == "dev-b"),
        "{s}"
    );
    no_unhandled(&fake);
}

#[test]
fn approve_shows_the_login_and_refuses_a_join_of_the_owner_account_on_both_crew_modes() {
    for toml in [crew_toml(), own_toml()] {
        approve_on(&toml);
    }
}

fn approve_on(toml: &str) {
    let fake = board_with(toml);
    let z = fake.joined_as("hermes@fake-z", "dev-z");
    let y = fake.joined("hermes@fake-y");
    let owner = owner_dir_with(toml);
    // the owner's account wrote the join of y: agents may not act as the owner
    let o = approve(&fake, owner.path(), &y.name);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("approve-owner-account") && o.stderr.contains("owner account"),
        "{}",
        o.all()
    );
    // a hand-written approval of y is void as well
    fake.owner_comment_now(REGISTRY, &format!("/crew approve {}", y.name));
    // z: the output and the comment name the account of the join
    let o = approve(&fake, owner.path(), &z.name);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("key ") && o.stdout.contains("account dev-z are frozen"),
        "{}",
        o.stdout
    );
    // the owner's status shows the account of each agent and the void approval
    let s = fake.run(owner.path(), &["status"]);
    assert_eq!(s.code, 0, "{}", s.all());
    let line = |name: &str| {
        s.stdout
            .lines()
            .find(|l| l.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("{}", s.stdout))
            .to_string()
    };
    assert!(line(&z.name).ends_with("login=dev-z"), "{}", s.stdout);
    assert!(line(&y.name).contains("pending"), "{}", s.stdout);
    assert!(
        line(&y.name).ends_with(&format!("login={OWNER}")),
        "{}",
        s.stdout
    );
    assert!(
        s.stdout.contains("owner account") && s.stdout.contains("void"),
        "{}",
        s.stdout
    );
    no_unhandled(&fake);
}
