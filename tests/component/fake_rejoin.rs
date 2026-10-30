//! A re-join keeps every key of the agent config that `join` does not own, and a
//! join for another agent name is refused. Against the fake GitHub; plain `cargo test`.
//!
//! `join` is refused in a headless config dir (spec/agents#headless-coordinator), before it reads
//! or writes anything. The writer keeps `headless` all the same (unit test in `src/config/dir.rs`).

use crate::support::board::{Agent, FakeBoard, INITIATIVE, REGISTRY, REPO};
use crate::support::headless::{Hub, Scenario};
use crewboard::config::ConfigDir;
use crewboard::rules::Role;

const FILE: &str = ".crewboard.toml";

fn cfg_path(dir: &std::path::Path) -> std::path::PathBuf {
    ConfigDir(dir.to_path_buf()).agent_config_path(INITIATIVE)
}

fn board_with_file() -> FakeBoard {
    let fake = FakeBoard::new();
    fake.set_file(REPO, FILE, FakeBoard::board_toml());
    fake
}

fn join(fake: &FakeBoard, dir: &std::path::Path, name: &str) -> crate::support::board::Out {
    fake.run(
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
    )
}

fn agent(fake: &FakeBoard) -> Agent {
    fake.agent("claude@fake-a", Role::Dev)
}

#[test]
fn a_rejoin_keeps_loop_tools_and_every_other_key_it_does_not_own() {
    let fake = board_with_file();
    let a = agent(&fake);
    let p = cfg_path(&a.dir);
    let original = std::fs::read_to_string(&p).unwrap();
    std::fs::write(
        &p,
        format!(
            "future_key = \"kept\"\ntools = [\"git\"]\n{original}\n[loop]\ninterval = \"20s\"\n\n[future_table]\nx = 1\n"
        ),
    )
    .unwrap();
    let o = join(&fake, &a.dir, "claude@fake-a");
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(&p).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    assert_eq!(t["future_key"].as_str(), Some("kept"), "{text}");
    assert_eq!(t["future_table"]["x"].as_integer(), Some(1), "{text}");
    assert_eq!(t["loop"]["interval"].as_str(), Some("20s"), "{text}");
    assert_eq!(t["tools"][0].as_str(), Some("git"), "{text}");
    // the item is the one that the config already had: no second agent item
    assert_eq!(t["item_id"].as_str(), Some(a.item_id.as_str()), "{text}");
    assert!(!p.with_extension("toml.tmp").exists(), "no temp file stays");
}

#[test]
fn a_fresh_join_writes_no_headless_key_and_no_loop_table() {
    let fake = board_with_file();
    let dir = tempfile::tempdir().unwrap();
    let o = join(&fake, dir.path(), "claude@fake-a");
    assert_eq!(o.code, 0, "{}", o.all());
    let text = std::fs::read_to_string(cfg_path(dir.path())).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    assert!(!t.contains_key("headless"), "{text}");
    assert!(!t.contains_key("loop"), "{text}");
    assert!(!ConfigDir(dir.path().to_path_buf()).is_headless().unwrap());
}

#[test]
fn a_join_for_another_name_is_refused_with_its_rule_and_the_config_stays() {
    let fake = board_with_file();
    let a = agent(&fake);
    let p = cfg_path(&a.dir);
    let before = std::fs::read_to_string(&p).unwrap();
    let o = join(&fake, &a.dir, "hermes@fake-b");
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=one-agent-per-config]"),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("claude@fake-a"), "{}", o.all());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    // pass direction: the same name joins again
    let ok = join(&fake, &a.dir, "claude@fake-a");
    assert_eq!(ok.code, 0, "{}", ok.all());
}

#[test]
fn another_name_is_refused_even_when_the_existing_config_does_not_load() {
    let fake = board_with_file();
    let a = agent(&fake);
    let p = cfg_path(&a.dir);
    // a typo inside [loop] makes load_agent fail; the file still names its agent
    let broken = format!(
        "{}\n[loop]\nintervall = \"20s\"\n",
        std::fs::read_to_string(&p).unwrap()
    );
    std::fs::write(&p, &broken).unwrap();
    assert!(ConfigDir(a.dir.clone()).load_agent(INITIATIVE).is_err());
    let o = join(&fake, &a.dir, "hermes@fake-b");
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("[rule=one-agent-per-config]"),
        "{}",
        o.all()
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), broken);
}

#[test]
fn a_join_in_a_headless_dir_is_refused_and_the_scope_stays_on() {
    let h = Hub::new(Scenario {
        body: "Goal: fix it\nGate: ci\n",
        may_merge: true,
        ci_green: true,
    });
    h.board.set_file(REPO, FILE, FakeBoard::board_toml());
    let p = cfg_path(h.headless_dir.path());
    let before = std::fs::read_to_string(&p).unwrap();
    assert!(before.starts_with("headless = true\n"), "{before}");
    let o = h.headless(&[
        "join",
        "--agent",
        "claude@fake-a",
        "--runtime",
        "codex",
        "--model",
        "it",
        "--repo",
        REPO,
    ]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("[rule=headless-scope]"), "{}", o.all());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), before);
    // start still prints the scope line, and an out-of-scope command is still refused
    let s = h.headless(&["start"]);
    assert_eq!(s.code, 0, "{}", s.all());
    assert!(s.stdout.contains("scope: headless"), "{}", s.all());
    let d = h.headless(&["pause"]);
    assert_eq!(d.code, 2, "{}", d.all());
    assert!(d.stderr.contains("[rule=headless-scope]"), "{}", d.all());
    // control: the interactive dir of the same agent can join again
    let ok = h.interactive(&[
        "join",
        "--agent",
        "claude@fake-a",
        "--runtime",
        "codex",
        "--model",
        "it",
        "--repo",
        REPO,
    ]);
    assert_eq!(ok.code, 0, "{}", ok.all());
}

#[test]
fn a_rejoin_keeps_the_agent_consent_to_a_trial_board() {
    let fake = board_with_file();
    let a = agent(&fake);
    let cd = ConfigDir(a.dir.clone());
    cd.set_agent_one_account(INITIATIVE).unwrap();

    let o = join(&fake, &a.dir, "claude@fake-a");
    assert_eq!(o.code, 0, "{}", o.all());
    let c = cd.load_agent(INITIATIVE).unwrap();
    assert!(
        c.insecure_one_account,
        "{}",
        std::fs::read_to_string(cfg_path(&a.dir)).unwrap()
    );
    assert!(c.board.insecure_one_account);

    // on a board without the key (each agent its own account) the owner's token is refused
    // before any write, whatever the agent's config says
    let no_key = FakeBoard::board_toml().replace("insecure_one_account = true\n", "");
    fake.set_file(REPO, FILE, &no_key);
    let dir = tempfile::tempdir().unwrap();
    let o = join(&fake, dir.path(), "claude@fake-b");
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("agent-uses-owner-token"), "{}", o.stderr);
    assert!(o.stderr.contains("their own account"), "{}", o.stderr);
    assert!(!cfg_path(dir.path()).exists());
}

#[test]
fn join_names_the_host_of_the_agent_name_not_the_machine() {
    // The agent name is public in every signed comment; a private machine name must not reach the
    // registry through the Host field when the owner chose a neutral name.
    let fake = board_with_file();
    let a = agent(&fake);
    let o = join(&fake, &a.dir, "claude@fake-a");
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        fake.item_field(&a.item_id, "Host").as_deref(),
        Some("fake-a")
    );
}

/// `join --runtime claude --model m-1` posts the join body that `join --runtime claude-code/m-1`
/// posted before D7: the wire format (v=1) and the registries stay as they are.
#[test]
fn join_with_runtime_and_model_posts_the_label_of_before() {
    let fake = board_with_file();
    for (name, args, label) in [
        (
            "claude@fake-n",
            &["--runtime", "claude", "--model", "m-1"][..],
            "claude-code/m-1",
        ),
        ("codex@fake-o", &["--runtime", "codex"][..], "codex"),
        (
            "hermes@fake-p",
            &["--runtime", "hermes", "--model", "vendor/m"][..],
            "hermes/vendor/m",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut argv = vec!["join", "--agent", name];
        argv.extend_from_slice(args);
        argv.extend_from_slice(&["--repo", REPO]);
        let o = fake.run(dir.path(), &argv);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(
            o.stdout.contains(&format!("agent {name} ({label})")),
            "{}",
            o.all()
        );
        let t: toml::Table =
            toml::from_str(&std::fs::read_to_string(cfg_path(dir.path())).unwrap()).unwrap();
        assert_eq!(t["runtime"].as_str(), Some(label));
        let item = t["item_id"].as_str().unwrap().to_string();
        assert_eq!(fake.item_field(&item, "Runtime").as_deref(), Some(label));
        let agent = crewboard::protocol::names::AgentName::parse(name).unwrap();
        let (key, created) = crewboard::config::load_or_create_key(
            &ConfigDir(dir.path().to_path_buf()).key_path(&agent),
        )
        .unwrap();
        assert!(!created, "join made the key");
        let key_line = crewboard::config::public_line(&key, &agent).unwrap();
        let host = name.split_once('@').unwrap().1;
        // the fixture: the join body of before, written out
        let fixture =
            format!("Join request.\nitem={item}\nkey={key_line}\nruntime={label}\nhost={host}\n");
        let posted: Vec<String> = fake
            .comments_of(REGISTRY)
            .into_iter()
            .map(|(_, _, b)| b)
            .filter(|b| b.contains(&format!("item={item}\n")))
            .collect();
        assert_eq!(posted.len(), 1, "{posted:?}");
        assert!(posted[0].contains(&fixture), "{}", posted[0]);
    }
}

/// The old form `--runtime <runtime>/<model>` is a usage error (exit 4) that shows the new form,
/// and nothing is read or written.
#[test]
fn join_refuses_the_old_runtime_form_with_the_new_form_in_the_message() {
    let fake = board_with_file();
    let dir = tempfile::tempdir().unwrap();
    let o = fake.run(
        dir.path(),
        &[
            "join",
            "--agent",
            "claude@fake-n",
            "--runtime",
            "claude-code/m-1",
            "--repo",
            REPO,
        ],
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(
        o.stderr.contains("use: --runtime claude --model m-1"),
        "{}",
        o.all()
    );
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    assert!(!cfg_path(dir.path()).exists());
}
