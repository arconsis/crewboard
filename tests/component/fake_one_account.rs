//! The identity modes of a board (spec/identity#identity-modes). `insecure_one_account = true`
//! is the one-person trial: the agents use the owner's token. Without it the board is a crew:
//! with `crew_login` a shared agent account, without it one own account per agent. A crew board
//! never lets an agent act with the owner's token. Against the fake GitHub; plain `cargo test`.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REPO};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused, writes};
use crewboard::config::ConfigDir;
use crewboard::rules::Role;

/// The refusal of the owner's token on a crew board.
const OWNER_TOKEN: &str = "agent-uses-owner-token";
const FILE: &str = ".crewboard.toml";

fn init_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec![
        "init",
        "--initiative",
        "trial",
        "--repo",
        REPO,
        "--project",
        "15",
        "--no-pr",
    ];
    v.extend_from_slice(extra);
    v
}

fn cd(a: &Agent) -> ConfigDir {
    ConfigDir(a.dir.clone())
}

/// The board file of a crew where each agent uses its own account: no key at all.
fn own_accounts_toml() -> String {
    FakeBoard::board_toml().replace("insecure_one_account = true\n", "")
}

/// The agent's own copy of the board is an own-accounts board.
fn on_own_accounts_board(a: &Agent) {
    let c = cd(a);
    let mut cfg = c.load_agent(INITIATIVE).unwrap();
    cfg.board.insecure_one_account = false;
    cfg.board.crew_login = None;
    c.save_agent(&cfg).unwrap();
    assert!(!c.load_agent(INITIATIVE).unwrap().insecure_one_account);
}

#[test]
fn init_without_a_flag_makes_an_own_accounts_board_and_says_so() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(owner.path(), &init_args(&[]));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("identity: own accounts")
            && o.stdout
                .contains("Write role on the repos and on the Project"),
        "{}",
        o.stdout
    );
    assert!(
        !o.stdout.contains("WARNING: one-account board"),
        "{}",
        o.stdout
    );
    let text = std::fs::read_to_string(owner.path().join("trial.board.toml")).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    assert!(!t.contains_key("crew_login"), "{text}");
    assert!(!t.contains_key("insecure_one_account"), "{text}");
    no_unhandled(&fake);
}

#[test]
fn init_with_the_flag_writes_the_key_and_warns() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(owner.path(), &init_args(&["--insecure-one-account"]));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("WARNING: one-account board"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("identity: own accounts"), "{}", o.stdout);
    let text = std::fs::read_to_string(owner.path().join("trial.board.toml")).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    assert_eq!(t["insecure_one_account"].as_bool(), Some(true), "{text}");
    assert!(!t.contains_key("crew_login"), "{text}");
    no_unhandled(&fake);
}

#[test]
fn init_with_a_crew_login_and_the_flag_is_a_usage_error() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(
        owner.path(),
        &init_args(&["--crew-login", "crew-bot", "--insecure-one-account"]),
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(o.stderr.contains("cannot be used with"), "{}", o.stderr);
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

#[test]
fn every_agent_command_on_an_own_accounts_board_refuses_the_owner_token_flag_or_not() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &own_accounts_toml());
    let a = fake.agent("claude@fake-a", Role::Dev);
    on_own_accounts_board(&a);
    // an empty body file (`/dev/null` does not exist on Windows)
    let body_dir = tempfile::tempdir().unwrap();
    let body = body_dir.path().join("empty.txt");
    std::fs::write(&body, "").unwrap();
    let body = body.display().to_string();
    let commands: [&[&str]; 8] = [
        &["poll"],
        &["watch", "--timeout", "1s", "--interval", "1s"],
        &[
            "say",
            "1",
            "--to",
            "owner",
            "--kind",
            "info",
            "--body-file",
            &body,
        ],
        &["heartbeat"],
        &["mine", "--quiet"],
        &["tick"],
        &["start"],
        &["start", "--insecure-one-account"],
    ];
    for args in commands {
        let o = run_refused(&fake, &a, args, OWNER_TOKEN);
        assert!(
            o.stderr.contains("their own account"),
            "{args:?}: {}",
            o.stderr
        );
    }
    // a refusal writes nothing, and the flag is not kept
    assert_eq!(writes(&fake), 0);
    assert!(!cd(&a).load_agent(INITIATIVE).unwrap().insecure_one_account);
    // read only commands of the owner view stay open
    for args in [
        &["status"][..],
        &["status", "--agents"][..],
        &["agent", "profile", "show"][..],
    ] {
        let o = fake.run(&a.dir, args);
        assert!(!o.stderr.contains(OWNER_TOKEN), "{args:?}: {}", o.all());
    }
    // control: the agent's own account passes the token check
    fake.set_viewer("dev-a");
    let o = fake.run(&a.dir, &["poll", "--quiet"]);
    assert!(!o.stderr.contains(OWNER_TOKEN), "{}", o.all());
    assert_ne!(o.code, 2, "{}", o.all());
}

#[test]
fn a_join_on_an_own_accounts_board_with_the_owner_token_is_refused_before_any_write() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &own_accounts_toml());
    let dir = tempfile::tempdir().unwrap();
    let join = |extra: &[&str]| {
        let mut args = vec![
            "join",
            "--agent",
            "hermes@fake-b",
            "--runtime",
            "codex",
            "--model",
            "it",
            "--repo",
            REPO,
        ];
        args.extend_from_slice(extra);
        fake.run(dir.path(), &args)
    };
    for extra in [&[][..], &["--insecure-one-account"][..]] {
        let o = join(extra);
        assert_eq!(o.code, 2, "{}", o.all());
        assert!(o.stderr.contains(OWNER_TOKEN), "{}", o.all());
    }
    assert_eq!(writes(&fake), 0, "no write");
    let c = ConfigDir(dir.path().to_path_buf());
    assert!(!c.agent_config_path(INITIATIVE).exists(), "no agent config");
    // control: the contributor's own token joins
    fake.set_viewer("dev-b");
    let o = join(&[]);
    assert_eq!(o.code, 0, "{}", o.all());
    no_unhandled(&fake);
}

#[test]
fn a_crew_board_ignores_the_flag_on_join() {
    let fake = FakeBoard::new();
    let crew = FakeBoard::board_toml().replace(
        "insecure_one_account = true\n",
        "crew_login = \"crew-bot\"\n",
    );
    fake.set_file(REPO, FILE, &crew);
    fake.set_viewer("crew-bot");
    let dir = tempfile::tempdir().unwrap();
    let o = fake.run(
        dir.path(),
        &[
            "join",
            "--agent",
            "hermes@fake-b",
            "--runtime",
            "codex",
            "--model",
            "it",
            "--repo",
            REPO,
            "--insecure-one-account",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let p = ConfigDir(dir.path().to_path_buf()).agent_config_path(INITIATIVE);
    let text = std::fs::read_to_string(p).unwrap();
    assert!(text.contains("crew_login"), "a crew board: {text}");
    assert!(!text.contains("insecure_one_account"), "{text}");
}

#[test]
fn a_join_on_a_trial_board_that_fails_after_the_early_save_leaves_no_consent_on_disk() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, FakeBoard::board_toml());
    // the registration comment of the join fails once
    fake.fail_once("POST", "/comments", "");
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "join",
        "--agent",
        "hermes@fake-b",
        "--runtime",
        "codex",
        "--model",
        "it",
        "--repo",
        REPO,
        "--insecure-one-account",
    ];
    let o = fake.run(dir.path(), &args);
    assert_ne!(o.code, 0, "{}", o.all());
    let c = ConfigDir(dir.path().to_path_buf());
    // the early save may hold the agent item, but never the agent's own consent
    assert!(
        !c.load_agent(INITIATIVE)
            .is_ok_and(|a| a.insecure_one_account)
    );
    // control: the retry succeeds and then holds the consent
    let o = fake.run(dir.path(), &args);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(c.load_agent(INITIATIVE).unwrap().insecure_one_account);
}

#[test]
fn status_has_no_one_account_line_on_a_crew_board_and_the_json_is_unchanged() {
    let (fake, c, _b) = crew();
    let o = run_ok(&fake, &c, &["status", "--json"]);
    assert!(!o.stdout.contains("ONE ACCOUNT"), "{}", o.stdout);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert!(v.get("identity").is_none(), "{v}");
    // both crew modes in the agent's copy: no one-account line. The token is the owner's, so
    // status falls back to the owner view of the board file.
    for crew_login in [Some("crew-bot"), None] {
        let cdir = cd(&c);
        let mut cfg = cdir.load_agent(INITIATIVE).unwrap();
        cfg.board.crew_login = crew_login.map(str::to_string);
        cfg.board.insecure_one_account = false;
        cdir.save_agent(&cfg).unwrap();
        let o = fake.run(&c.dir, &["status"]);
        assert!(!o.stdout.contains("ONE ACCOUNT"), "{}", o.all());
    }
}

#[test]
fn the_board_key_alone_lets_every_agent_command_pass() {
    let (fake, c, b) = crew();
    // the fake board holds the key: no agent has a consent of its own
    assert!(
        cd(&b)
            .load_agent(INITIATIVE)
            .unwrap()
            .board
            .insecure_one_account
    );
    assert!(!cd(&b).load_agent(INITIATIVE).unwrap().insecure_one_account);
    run_ok(&fake, &b, &["start"]);
    // the trial: the owner's token assigns and takes
    let body = format!("Goal: trial ticket.\n\n{GATE}");
    let t = fake
        .ticket_issue("trial", &body, &["crew:work"])
        .to_string();
    let item = item_in(&run_ok(&fake, &c, &["assign", &t, "--to", &b.name]));
    run_ok(&fake, &b, &["take", &t]);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn a_start_on_a_trial_board_with_the_flag_persists_the_consent() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, FakeBoard::board_toml());
    let a = fake.agent("claude@fake-a", Role::Dev);
    let o = run_ok(&fake, &a, &["start", "--insecure-one-account"]);
    assert!(o.stdout.contains("role brief"), "{}", o.stdout);
    assert!(cd(&a).load_agent(INITIATIVE).unwrap().insecure_one_account);
    // later runs need no flag
    run_ok(&fake, &a, &["start"]);
    no_unhandled(&fake);
}

#[test]
fn status_names_the_one_account_mode() {
    let (fake, c, _b) = crew();
    let o = run_ok(&fake, &c, &["status"]);
    assert!(
        o.stdout.contains("identity: ONE ACCOUNT (insecure)"),
        "{}",
        o.stdout
    );
}

#[test]
fn a_crew_board_ignores_the_flag_and_the_key() {
    let fake = FakeBoard::new();
    let a = fake.agent("claude@fake-a", Role::Dev);
    // a crew board: a crew login, no one-account key
    let c = cd(&a);
    let mut cfg = c.load_agent(INITIATIVE).unwrap();
    cfg.board.crew_login = Some("crew-bot".into());
    cfg.board.insecure_one_account = false;
    c.save_agent(&cfg).unwrap();

    // the owner's token is refused with its own rule, flag or not
    for args in [&["start"][..], &["start", "--insecure-one-account"][..]] {
        let o = run_refused(&fake, &a, args, OWNER_TOKEN);
        assert!(o.stderr.contains("crew-bot"), "{}", o.stderr);
    }
    // a refused start persists nothing
    assert!(!c.load_agent(INITIATIVE).unwrap().insecure_one_account);

    // the agent token passes without any flag: the consent is not asked on a crew board
    fake.set_viewer("crew-bot");
    let o = fake.run(&a.dir, &["poll", "--quiet"]);
    assert!(!o.stderr.contains(OWNER_TOKEN), "{}", o.all());
    assert_ne!(o.code, 2, "{}", o.all());
}
