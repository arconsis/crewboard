//! The one-account mode needs consent. A board without `crew_login` makes the agents use the
//! owner's token, so `init` and every agent command refuse until the owner or the agent says
//! so: the board key `insecure_one_account`, the agent config key, or `start
//! --insecure-one-account`. Against the fake GitHub; plain `cargo test`.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REPO};
use crate::support::cli::{GATE, crew, item_in, no_unhandled, run_ok, run_refused, writes};
use crewboard::config::ConfigDir;
use crewboard::rules::Role;

const RULE: &str = "one-account-needs-flag";
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

/// The board file as the default branch of an agent without the owner's consent holds it.
fn board_without_key() -> String {
    FakeBoard::board_toml().replace("insecure_one_account = true\n", "")
}

/// Take the board key out of the agent's own copy: the agent has no consent of any kind.
fn without_consent(a: &Agent) {
    let c = cd(a);
    let mut cfg = c.load_agent(INITIATIVE).unwrap();
    cfg.board.insecure_one_account = false;
    c.save_agent(&cfg).unwrap();
    assert!(!c.load_agent(INITIATIVE).unwrap().insecure_one_account);
}

#[test]
fn init_without_a_flag_is_refused_before_any_request() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = fake.run(owner.path(), &init_args(&[]));
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(RULE), "{}", o.all());
    assert!(
        o.stderr.contains("--crew-login") && o.stderr.contains("--insecure-one-account"),
        "the refusal names both ways out: {}",
        o.stderr
    );
    // not one request, so nothing was written
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    assert!(
        !owner.path().join("trial.board.toml").exists(),
        "no board config"
    );
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
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("cannot be used with"), "{}", o.stderr);
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

/// Upgrade path (b) of an existing trial board: the agent and the default branch hold the same
/// key-less file, and the agent gives its consent once. The refusal of a silent flip of the key
/// at `start` is `fake_board_config::a_changed_insecure_one_account_key_is_an_identity_change`.
#[test]
fn start_without_the_flag_is_refused_and_with_it_passes_and_persists() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &board_without_key());
    let a = fake.agent("claude@fake-a", Role::Dev);
    without_consent(&a);

    let o = run_refused(&fake, &a, &["start"], RULE);
    assert!(o.stderr.contains("--insecure-one-account"), "{}", o.stderr);
    // the refusal comes before any write, and the agent config has no key
    assert_eq!(writes(&fake), 0);
    let text = std::fs::read_to_string(cd(&a).agent_config_path(INITIATIVE)).unwrap();
    assert!(!text.contains("insecure_one_account"), "{text}");

    let o = run_ok(&fake, &a, &["start", "--insecure-one-account"]);
    assert!(o.stdout.contains("role brief"), "{}", o.stdout);
    let c = cd(&a).load_agent(INITIATIVE).unwrap();
    assert!(c.insecure_one_account, "the key is persisted");
    assert!(
        !c.board.insecure_one_account,
        "the board copy is not touched"
    );
    let text = std::fs::read_to_string(cd(&a).agent_config_path(INITIATIVE)).unwrap();
    assert!(text.contains("insecure_one_account = true"), "{text}");

    // later runs need no flag
    run_ok(&fake, &a, &["start"]);
    no_unhandled(&fake);
}

#[test]
fn a_fresh_join_on_a_keyless_board_needs_the_flag_and_then_start_and_take_pass_without_it() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &board_without_key());
    let owner = fake.agent("codex@fake-c", Role::Coordinator);
    let dir = tempfile::tempdir().unwrap();
    let join = |extra: &[&str]| {
        let mut v = vec![
            "join",
            "--agent",
            "hermes@fake-b",
            "--runtime",
            "fake/it",
            "--repo",
            REPO,
        ];
        v.extend_from_slice(extra);
        fake.run(dir.path(), &v)
    };

    let o = join(&[]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains(RULE), "{}", o.all());
    assert_eq!(writes(&fake), 0, "no write");
    let c = ConfigDir(dir.path().to_path_buf());
    assert!(!c.agent_config_path(INITIATIVE).exists(), "no agent config");

    let o = join(&["--insecure-one-account"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(c.agent_config_path(INITIATIVE)).unwrap();
    assert!(text.contains("insecure_one_account = true"), "{text}");

    // the owner approves; then start and take need no flag
    let od = fake.owner_dir();
    let o = fake.run(
        od.path(),
        &["approve", "hermes@fake-b", "--initiative", INITIATIVE],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let o = fake.run(dir.path(), &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let body = format!("Goal: trial ticket.\n\n{GATE}");
    let t = fake
        .ticket_issue("trial", &body, &["crew:work"])
        .to_string();
    run_ok(&fake, &owner, &["assign", &t, "--to", "hermes@fake-b"]);
    let o = fake.run(dir.path(), &["take", &t]);
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
            "fake/it",
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
fn a_join_that_fails_after_the_early_save_leaves_no_consent_on_disk() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &board_without_key());
    // the registration comment of the join fails once
    fake.fail_once("POST", "/comments", "");
    let dir = tempfile::tempdir().unwrap();
    let args = [
        "join",
        "--agent",
        "hermes@fake-b",
        "--runtime",
        "fake/it",
        "--repo",
        REPO,
        "--insecure-one-account",
    ];
    let o = fake.run(dir.path(), &args);
    assert_ne!(o.code, 0, "{}", o.all());
    let c = ConfigDir(dir.path().to_path_buf());
    // the early save may hold the agent item, but never the consent
    let text = std::fs::read_to_string(c.agent_config_path(INITIATIVE)).unwrap_or_default();
    assert!(!text.contains("insecure_one_account"), "{text}");
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
    // a crew board: a crew login in the agent's copy
    let cdir = cd(&c);
    let mut cfg = cdir.load_agent(INITIATIVE).unwrap();
    cfg.board.crew_login = Some("crew-bot".into());
    cfg.board.insecure_one_account = false;
    cdir.save_agent(&cfg).unwrap();
    let o = run_ok(&fake, &c, &["status"]);
    assert!(!o.stdout.contains("ONE ACCOUNT"), "{}", o.stdout);
    assert!(o.stdout.contains("initiative fake"), "{}", o.stdout);
}

#[test]
fn every_agent_command_on_a_keyless_board_without_consent_is_refused() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &board_without_key());
    let a = fake.agent("claude@fake-a", Role::Dev);
    without_consent(&a);
    // an empty body file (`/dev/null` does not exist on Windows)
    let body_dir = tempfile::tempdir().unwrap();
    let body = body_dir.path().join("empty.txt");
    std::fs::write(&body, "").unwrap();
    let body = body.display().to_string();
    let commands: [&[&str]; 6] = [
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
    ];
    for args in commands {
        run_refused(&fake, &a, args, RULE);
    }
    // read only commands of the owner view stay open without consent
    for args in [
        &["status"][..],
        &["status", "--agents"][..],
        &["agent", "profile", "show"][..],
    ] {
        let o = fake.run(&a.dir, args);
        assert!(!o.stderr.contains(RULE), "{args:?}: {}", o.all());
    }
}

#[test]
fn a_start_with_the_flag_that_fails_for_another_reason_persists_nothing() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, &board_without_key());
    // joined, not approved: `start` refuses on the approval
    let a = fake.joined("claude@fake-a");
    without_consent(&a);
    let o = fake.run(&a.dir, &["start", "--insecure-one-account"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(!o.stderr.contains(RULE), "{}", o.all());
    let text = std::fs::read_to_string(cd(&a).agent_config_path(INITIATIVE)).unwrap();
    assert!(!text.contains("insecure_one_account"), "{text}");
}

#[test]
fn the_next_take_passes_without_the_flag_after_start_persisted_it() {
    let (fake, c, b) = crew();
    fake.set_file(REPO, FILE, &board_without_key());
    without_consent(&c);
    without_consent(&b);
    let body = format!("Goal: trial ticket.\n\n{GATE}");
    let t = fake
        .ticket_issue("trial", &body, &["crew:work"])
        .to_string();

    // the coordinator needs the flag once, then it can assign
    run_refused(&fake, &c, &["assign", &t, "--to", &b.name], RULE);
    run_ok(&fake, &c, &["start", "--insecure-one-account"]);
    let item = item_in(&run_ok(&fake, &c, &["assign", &t, "--to", &b.name]));

    // the dev agent is refused until its own start
    run_refused(&fake, &b, &["take", &t], RULE);
    run_ok(&fake, &b, &["start", "--insecure-one-account"]);
    run_ok(&fake, &b, &["take", &t]);
    assert_eq!(
        fake.item_field(&item, "Status").as_deref(),
        Some("In Progress")
    );
    no_unhandled(&fake);
}

#[test]
fn the_board_key_alone_lets_every_agent_command_pass() {
    let (fake, _c, b) = crew();
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
        let o = run_refused(&fake, &a, args, "agent-uses-owner-token");
        assert!(!o.stderr.contains(RULE), "{}", o.stderr);
    }
    // a refused start persists nothing
    assert!(!c.load_agent(INITIATIVE).unwrap().insecure_one_account);

    // the crew token passes without any flag: the consent is not asked on a crew board
    fake.set_viewer("crew-bot");
    let o = fake.run(&a.dir, &["poll", "--quiet"]);
    assert!(!o.stderr.contains(RULE), "{}", o.all());
    assert_ne!(o.code, 2, "{}", o.all());
}
