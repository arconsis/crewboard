//! Network-failure behaviour of the CLI against a fake GitHub.
//!
//! These tests run in plain `cargo test`. They need no token, no sandbox and no real
//! GitHub. `CREWBOARD_API_BASE` points the CLI at a loopback server (see `tests/support`).
//! They replace the old `it_a_network_error_is_exit_3_with_the_status_line_and_never_exit_1`.
//! Each test waits about 7 s, because the CLI waits 2 s and 5 s between its 3 attempts.

use crate::support::{FakeGithub, Resp, closed_port_base};
use crewboard::config::{AgentConfig, BoardConfig, ConfigDir, load_or_create_key};
use crewboard::protocol::names::AgentName;
use std::path::Path;
use std::process::Command;

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A config dir with one joined agent. The key is a fresh test key.
fn agent_dir(root: &Path) {
    let cd = ConfigDir(root.to_path_buf());
    let board = BoardConfig {
        initiative: "fake".into(),
        project_owner: "fake-owner".into(),
        project_number: 1,
        repos: vec!["fake-owner/fake-repo".into()],
        owner_login: "fake-owner".into(),
        registry_issue: 1,
        authority: "AUTHORITY.md".into(),
        crew_login: None,
        insecure_one_account: true,
        heartbeat_interval: None,
        required_tools: vec![],
        smoke_cmd: None,
        detect_tools: vec![],
        stale: None,
        late: None,
        former_repos: vec![],
    };
    let agent = AgentConfig {
        name: "claude@fake".into(),
        runtime: "claude".into(),
        host: "fake".into(),
        item_id: "PVTI_fake".into(),
        board,
        tools: None,
        board_sha: None,
        loop_cfg: None,
        wake: None,
        insecure_one_account: false,
    };
    cd.save_agent(&agent).unwrap();
    let name = AgentName::parse("claude@fake").unwrap();
    load_or_create_key(&cd.key_path(&name)).unwrap();
}

fn poll(base: &str, token: &str) -> Out {
    let tmp = tempfile::tempdir().unwrap();
    agent_dir(tmp.path());
    let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(tmp.path())
        .arg("poll")
        .env("CREWBOARD_API_BASE", base)
        .env("GH_TOKEN", token)
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("run crewboard");
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).to_string(),
        stderr: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}

fn assert_status_line(o: &Out) {
    assert!(
        o.stderr.contains("cursor=") && o.stderr.contains("polls="),
        "{}",
        o.stderr
    );
}

#[test]
fn a_refused_connection_is_exit_3_with_the_status_line_after_3_attempts() {
    let o = poll(&closed_port_base(), "ghp_faketoken");
    assert_eq!(o.code, 3, "stdout: {}\nstderr: {}", o.stdout, o.stderr);
    assert_status_line(&o);
    assert!(o.stderr.contains("after 3 attempts"), "{}", o.stderr);
}

#[test]
fn a_gateway_error_is_retried_twice_then_exit_3_never_exit_1() {
    let fake = FakeGithub::start(|_| Resp::json(502, r#"{"message":"Bad Gateway"}"#));
    let o = poll(&fake.base(), "ghp_faketoken");
    assert_eq!(o.code, 3, "stdout: {}\nstderr: {}", o.stdout, o.stderr);
    assert_status_line(&o);
    assert!(o.stderr.contains("after 3 attempts"), "{}", o.stderr);
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert!(
        reqs.iter()
            .all(|r| r.method == "GET" && r.path == "/users/fake-owner"),
        "{reqs:?}"
    );
    assert!(
        reqs.iter()
            .all(|r| r.bearer.as_deref() == Some("ghp_faketoken"))
    );
}

#[test]
fn a_refused_token_is_exit_4_after_one_call_and_is_not_retried() {
    let fake = FakeGithub::start(|_| Resp::json(401, r#"{"message":"Bad credentials"}"#));
    let o = poll(&fake.base(), "ghp_notavalidtoken");
    assert_eq!(o.code, 4, "stdout: {}\nstderr: {}", o.stdout, o.stderr);
    assert_eq!(fake.requests().len(), 1, "{:?}", fake.requests());
}

#[test]
fn a_foreign_api_base_is_a_config_error_and_nothing_is_sent() {
    let fake = FakeGithub::start(|_| Resp::json(200, "{}"));
    let o = poll("https://evil.example", "ghp_faketoken");
    assert_eq!(o.code, 4, "stdout: {}\nstderr: {}", o.stdout, o.stderr);
    assert!(o.stderr.contains("CREWBOARD_API_BASE"), "{}", o.stderr);
    assert!(fake.requests().is_empty());
}
