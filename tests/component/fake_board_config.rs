//! `start` and `resume` refresh the agent's copy of `.crewboard.toml` from the
//! default branch, and `status` says when the copy differs. Against the fake GitHub; plain
//! `cargo test`.

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REPO};
use crewboard::config::ConfigDir;
use crewboard::rules::Role;

const FILE: &str = ".crewboard.toml";

fn base() -> String {
    FakeBoard::board_toml().to_string()
}

/// The board file with a `[stale]` table, as an edit of the default branch.
fn with_stale(limit: &str) -> String {
    format!("{}\n[stale]\nno_push = \"{limit}\"\n", base())
}

fn cfg_path(a: &Agent) -> std::path::PathBuf {
    ConfigDir(a.dir.clone()).agent_config_path(INITIATIVE)
}

fn contents_reads(fake: &FakeBoard) -> usize {
    fake.requests()
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains(&format!("/contents/{FILE}")))
        .count()
}

fn agent(fake: &FakeBoard) -> Agent {
    fake.agent("claude@fake-a", Role::Dev)
}

#[test]
fn a_changed_stale_table_reaches_the_agent_config_at_start_and_the_line_names_it() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    fake.set_file(REPO, FILE, &with_stale("2h"));
    let before = ConfigDir(a.dir.clone()).load_agent(INITIATIVE).unwrap();
    assert!(before.board.stale.is_none() && before.board_sha.is_none());

    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains(&format!("board config updated from {REPO}@")),
        "{}",
        o.stdout
    );
    let line = o
        .stdout
        .lines()
        .find(|l| l.starts_with("board config updated"))
        .unwrap();
    assert!(line.ends_with(": stale"), "only `stale` changed: {line}");
    let after = ConfigDir(a.dir.clone()).load_agent(INITIATIVE).unwrap();
    assert_eq!(
        after.board.stale.unwrap().no_push.as_deref(),
        Some("2h"),
        "{}",
        std::fs::read_to_string(cfg_path(&a)).unwrap()
    );
    assert!(after.board_sha.is_some(), "the source SHA is kept");
    assert_eq!(contents_reads(&fake), 1, "one read of the file per start");

    // a second start has nothing to update
    let text = std::fs::read_to_string(cfg_path(&a)).unwrap();
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("board config updated"), "{}", o.stdout);
    assert_eq!(std::fs::read_to_string(cfg_path(&a)).unwrap(), text);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn an_unchanged_file_writes_nothing() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    fake.set_file(REPO, FILE, &base());
    let text = std::fs::read_to_string(cfg_path(&a)).unwrap();
    // Set the mtime into the past, so a rewrite always shows as a different mtime, whatever
    // the resolution of the file system.
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(cfg_path(&a))
        .unwrap()
        .set_modified(past)
        .unwrap();
    let mtime = std::fs::metadata(cfg_path(&a)).unwrap().modified().unwrap();

    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        !o.stdout.contains("board config") && !o.stdout.contains("note: cannot read"),
        "{}",
        o.stdout
    );
    assert_eq!(std::fs::read_to_string(cfg_path(&a)).unwrap(), text);
    assert_eq!(
        std::fs::metadata(cfg_path(&a)).unwrap().modified().unwrap(),
        mtime,
        "the file was rewritten"
    );
    assert_eq!(contents_reads(&fake), 1);
}

#[test]
fn a_changed_project_number_is_refused_with_its_rule_and_the_copy_stays() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    fake.set_file(
        REPO,
        FILE,
        &base().replace("project_number = 15", "project_number = 16"),
    );
    let text = std::fs::read_to_string(cfg_path(&a)).unwrap();
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("board-identity-changed") && o.stderr.contains("project_number"),
        "{}",
        o.stderr
    );
    assert_eq!(std::fs::read_to_string(cfg_path(&a)).unwrap(), text);
    // the refusal comes right after the read of the file: nothing follows it
    let reqs = fake.requests();
    assert!(
        reqs.last().unwrap().path.ends_with(FILE),
        "{:?}",
        reqs.last()
    );

    // control: a change of a non-identity key passes in the same setup
    fake.set_file(
        REPO,
        FILE,
        &format!("{}heartbeat_interval = \"20s\"\n", base()),
    );
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains(": heartbeat_interval"), "{}", o.stdout);
}

#[test]
fn a_changed_insecure_one_account_key_is_an_identity_change() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    // the copy holds the key; the default branch drops it: a silent switch to a stricter or
    // looser identity mode is refused, and the copy stays
    fake.set_file(
        REPO,
        FILE,
        &base().replace("insecure_one_account = true\n", ""),
    );
    let text = std::fs::read_to_string(cfg_path(&a)).unwrap();
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("board-identity-changed") && o.stderr.contains("insecure_one_account"),
        "{}",
        o.stderr
    );
    assert_eq!(std::fs::read_to_string(cfg_path(&a)).unwrap(), text);
    // control: the same file with the key passes
    fake.set_file(REPO, FILE, &base());
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    // the key survives the write of the copy
    let c = ConfigDir(a.dir.clone()).load_agent(INITIATIVE).unwrap();
    assert!(c.board.insecure_one_account);
}

#[test]
fn a_changed_main_repo_or_registry_issue_is_refused_too() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    for (from, to, key) in [
        (
            "repos = [\"fake-owner/fake-repo\"]",
            "repos = [\"fake-owner/other\"]",
            "repos (main repo)",
        ),
        ("registry_issue = 1", "registry_issue = 9", "registry_issue"),
        (
            "owner_login = \"fake-owner\"",
            "owner_login = \"x\"",
            "owner_login",
        ),
    ] {
        fake.set_file(REPO, FILE, &base().replace(from, to));
        let o = fake.run(&a.dir, &["start"]);
        assert_eq!(o.code, 2, "{key}: {}", o.all());
        assert!(
            o.stderr.contains("board-identity-changed") && o.stderr.contains(key),
            "{key}: {}",
            o.stderr
        );
    }
}

#[test]
fn a_failed_read_keeps_the_copy_says_so_once_and_start_succeeds() {
    for status in [500, 404] {
        let fake = FakeBoard::new();
        let a = agent(&fake);
        if status == 500 {
            fake.fail_file(REPO, FILE, 500);
        }
        let text = std::fs::read_to_string(cfg_path(&a)).unwrap();
        let o = fake.run(&a.dir, &["start"]);
        assert_eq!(o.code, 0, "{status}: {}", o.all());
        assert_eq!(
            o.stdout.matches("note: cannot read").count(),
            1,
            "{status}: {}",
            o.stdout
        );
        assert!(o.stdout.contains("state: active") || o.stdout.contains("already active"));
        assert_eq!(std::fs::read_to_string(cfg_path(&a)).unwrap(), text);
    }
}

#[test]
fn other_top_level_keys_and_tables_survive_the_rewrite() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    let original = std::fs::read_to_string(cfg_path(&a)).unwrap();
    std::fs::write(
        cfg_path(&a),
        format!("headless = true\n{original}\n[loop]\ninterval = \"20s\"\n"),
    )
    .unwrap();
    fake.set_file(REPO, FILE, &with_stale("3h"));
    let o = fake.run(&a.dir, &["start"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("board config updated"), "{}", o.stdout);
    let cd = ConfigDir(a.dir.clone());
    assert!(
        cd.is_headless().unwrap(),
        "{}",
        std::fs::read_to_string(cfg_path(&a)).unwrap()
    );
    let c = cd.load_agent(INITIATIVE).unwrap();
    assert_eq!(c.loop_cfg.unwrap().interval.as_deref(), Some("20s"));
    assert_eq!(c.board.stale.unwrap().no_push.as_deref(), Some("3h"));
    assert!(
        !cfg_path(&a).with_extension("toml.tmp").exists(),
        "no temp file stays"
    );
}

#[test]
fn status_shows_when_the_copy_differs_with_one_read_and_resume_clears_it() {
    let fake = FakeBoard::new();
    let a = agent(&fake);
    let differs =
        "board config differs from fake-owner/fake-repo default branch; run crewboard resume";
    fake.set_file(REPO, FILE, &base());
    let o = fake.run(&a.dir, &["status"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stdout.contains("board config differs"), "{}", o.stdout);
    assert_eq!(contents_reads(&fake), 1, "one read per status run");

    fake.set_file(REPO, FILE, &with_stale("2h"));
    let o = fake.run(&a.dir, &["status"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains(differs), "{}", o.stdout);
    assert_eq!(contents_reads(&fake), 2, "one more read, not more");
    let j = fake.run(&a.dir, &["status", "--json"]);
    assert!(
        j.stdout.contains("\"board_config_differs\":true"),
        "{}",
        j.stdout
    );

    let o = fake.run(&a.dir, &["resume"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("board config updated"), "{}", o.stdout);
    let o = fake.run(&a.dir, &["status"]);
    assert!(!o.stdout.contains("board config differs"), "{}", o.stdout);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}
