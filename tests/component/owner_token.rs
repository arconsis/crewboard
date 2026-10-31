//! The token of an owner command (spec/identity#author-and-signature): the owner's own `gh` login,
//! or `--env-file`. An agent token in `GH_TOKEN` is ignored with one note, a `gh` login that is
//! not the board owner is refused, and an agent dir keeps the agent order, so it never reaches
//! the `gh` login. Agent commands keep their order.
//!
//! The fake GitHub records the bearer of each request, so a test sees WHICH token reached the
//! API. The own `gh auth token` is the hidden test variable here (it runs on every platform);
//! the Unix tests at the end run a fake `gh` script instead.

use crate::support::board::{FakeBoard, INITIATIVE, OWNER, Out, REPO};
use crate::support::cli::{crew, writes};
use crewboard::github::TEST_GH_AUTH_TOKEN_ENV;
use std::path::Path;
use std::process::Command;

/// `crewboard --config-dir <dir> <args>` against the fake with no token variable of the
/// machine. `env` adds variables.
fn cb(fake: &FakeBoard, dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(dir)
        .args(args)
        .env("CREWBOARD_API_BASE", fake.base())
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_ENTERPRISE_TOKEN")
        .env_remove("GITHUB_ENTERPRISE_TOKEN")
        .env_remove(TEST_GH_AUTH_TOKEN_ENV)
        .envs(env.iter().copied())
        .output()
        .expect("run crewboard");
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).to_string(),
        stderr: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}

/// The bearers of all requests the fake got, without duplicates.
fn bearers(fake: &FakeBoard) -> Vec<String> {
    let mut v: Vec<String> = fake
        .requests()
        .iter()
        .map(|r| r.bearer.clone().unwrap_or_default())
        .collect();
    v.sort();
    v.dedup();
    v
}

const GH_LOGIN: (&str, &str) = (TEST_GH_AUTH_TOKEN_ENV, "ghp_test_own_gh_login");
const CREW_VAR: (&str, &str) = ("GH_TOKEN", "ghp_test_crew_token");

fn audit() -> Vec<&'static str> {
    vec!["audit", "--initiative", INITIATIVE]
}

/// The env file `name` in `dir` with `GH_TOKEN=token`, mode 600.
fn env_file(dir: &Path, name: &str, token: &str) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, format!("GH_TOKEN={token}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    p
}

#[test]
fn an_owner_command_ignores_a_crew_token_and_runs_with_the_own_gh_login() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    let o = cb(&fake, owner.path(), &audit(), &[CREW_VAR, GH_LOGIN]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_own_gh_login"], "{}", o.all());
    assert!(
        o.stderr.contains("note: GH_TOKEN is set and ignored"),
        "{}",
        o.all()
    );
    assert!(!o.all().contains("ghp_test_"), "a token value: {}", o.all());
    // twin: no token variable, no note
    let o = cb(&fake, owner.path(), &audit(), &[GH_LOGIN]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.stderr.contains("note:"), "{}", o.all());
}

#[test]
fn the_note_names_both_variables_once_for_several_names() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    // two names that never joined: both are refused, the note comes once
    let o = cb(
        &fake,
        owner.path(),
        &[
            "approve",
            "nobody@fake-x",
            "nobody@fake-y",
            "--initiative",
            INITIATIVE,
        ],
        &[CREW_VAR, ("GITHUB_TOKEN", "ghp_test_crew_2"), GH_LOGIN],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert_eq!(
        o.stderr
            .matches("note: GH_TOKEN and GITHUB_TOKEN are set and ignored")
            .count(),
        1,
        "{}",
        o.all()
    );
    assert_eq!(bearers(&fake), ["ghp_test_own_gh_login"], "{}", o.all());
}

#[test]
fn a_gh_login_that_is_not_the_board_owner_is_refused() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    fake.set_viewer("crew-bot");
    for args in [
        audit(),
        vec!["approve", "hermes@fake-b", "--initiative", INITIATIVE],
        vec![
            "pause",
            "--agent",
            "hermes@fake-b",
            "--initiative",
            INITIATIVE,
        ],
        vec![
            "stop",
            "--agent",
            "hermes@fake-b",
            "--force",
            "--initiative",
            INITIATIVE,
        ],
        vec!["move", "7", "Ready", "--initiative", INITIATIVE],
    ] {
        let o = cb(&fake, owner.path(), &args, &[GH_LOGIN]);
        assert_eq!(o.code, 2, "{args:?}: {}", o.all());
        assert!(
            o.stderr.contains("owner-command-not-owner")
                && o.stderr.contains("`gh auth status` shows crew-bot")
                && o.stderr.contains("the board owner is fake-owner"),
            "{args:?}: {}",
            o.all()
        );
    }
    assert_eq!(writes(&fake), 0);
}

#[test]
fn init_checks_the_gh_login_against_the_owner_it_writes() {
    let fake = FakeBoard::new();
    let dir = tempfile::tempdir().unwrap();
    let o = cb(
        &fake,
        dir.path(),
        &[
            "init",
            "--initiative",
            "demo",
            "--repo",
            REPO,
            "--owner-login",
            "someone-else",
            "--insecure-one-account",
        ],
        &[CREW_VAR, GH_LOGIN],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("owner-command-not-owner")
            && o.stderr.contains("the board owner is someone-else"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), 0);
    assert_eq!(bearers(&fake), ["ghp_test_own_gh_login"]);
}

#[test]
fn an_explicit_env_file_wins_for_an_owner_command() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    let f = env_file(owner.path(), "owner.env", "ghp_test_owner_file");
    let o = cb(
        &fake,
        owner.path(),
        &[
            "--env-file",
            f.to_str().unwrap(),
            "audit",
            "--initiative",
            INITIATIVE,
        ],
        &[CREW_VAR, GH_LOGIN],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_owner_file"], "{}", o.all());
    assert!(!o.stderr.contains("note:"), "{}", o.all());
    // a file of another account is refused, and the fix names the file
    fake.set_viewer("crew-bot");
    let o = cb(
        &fake,
        owner.path(),
        &[
            "--env-file",
            f.to_str().unwrap(),
            "audit",
            "--initiative",
            INITIATIVE,
        ],
        &[GH_LOGIN],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("owner-command-not-owner")
            && o.stderr.contains("the token of --env-file is crew-bot"),
        "{}",
        o.all()
    );
}

#[test]
fn an_owner_command_in_an_agent_dir_keeps_the_agent_order() {
    let (fake, _c, b) = crew();
    // an agent dir by its empty gh config dir, with a good agent.env
    std::fs::create_dir(b.dir.join("gh-empty-config")).unwrap();
    env_file(&b.dir, "agent.env", "ghp_test_agent_file");
    // the agent's token, not the gh login (the trial: the agent token is the owner's)
    let o = cb(&fake, &b.dir, &audit(), &[CREW_VAR, GH_LOGIN]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_agent_file"], "{}", o.all());
    assert!(!o.stderr.contains("note:"), "{}", o.all());
    // an agent token there is refused, and the fix names the own terminal
    fake.set_viewer("crew-bot");
    let o = cb(&fake, &b.dir, &audit(), &[GH_LOGIN]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("owner-command-not-owner")
            && o.stderr.contains("is the dir of an agent")
            && o.stderr.contains("own config dir"),
        "{}",
        o.all()
    );
    assert_eq!(bearers(&fake), ["ghp_test_agent_file"], "{}", o.all());
    // --env-file is the owner's explicit choice: it wins in an agent dir too
    fake.set_viewer(OWNER);
    let f = env_file(&b.dir, "owner.env", "ghp_test_owner_file");
    let o = cb(
        &fake,
        &b.dir,
        &[
            "--env-file",
            f.to_str().unwrap(),
            "audit",
            "--initiative",
            INITIATIVE,
        ],
        &[GH_LOGIN],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(bearers(&fake).contains(&"ghp_test_owner_file".to_string()));
}

#[test]
fn agent_commands_keep_the_token_variable() {
    let (fake, c, b) = crew();
    let o = cb(&fake, &b.dir, &["status"], &[CREW_VAR, GH_LOGIN]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_crew_token"], "{}", o.all());
    assert!(!o.stderr.contains("note:"), "{}", o.all());
    // the coordinator's forced stop in its own config dir: the agent order, not the gh login
    let o = cb(
        &fake,
        &c.dir,
        &["stop", "--agent", &b.name, "--force"],
        &[CREW_VAR, GH_LOGIN],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_crew_token"], "{}", o.all());
}

/// A fake `gh` in a new dir: `gh auth token` writes the `GH_TOKEN` it saw to `saw` and prints
/// `token`.
#[cfg(unix)]
fn fake_gh(token: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let saw = d.path().join("gh-saw");
    let gh = d.path().join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\n[ \"$1 $2\" = 'auth token' ] || exit 1\nprintf %s \"${{GH_TOKEN:-unset}}/${{GITHUB_TOKEN:-unset}}\" > '{}'\necho {token}\n",
            saw.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    (d, saw)
}

#[cfg(unix)]
#[test]
fn the_real_gh_runs_without_the_token_variables_of_the_caller() {
    let fake = FakeBoard::new();
    let owner = fake.owner_dir();
    let (gh, saw) = fake_gh("ghp_test_from_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let o = cb(
        &fake,
        owner.path(),
        &audit(),
        &[
            ("PATH", path.as_str()),
            CREW_VAR,
            ("GITHUB_TOKEN", "ghp_test_crew_2"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_from_gh"], "{}", o.all());
    assert_eq!(std::fs::read_to_string(&saw).unwrap(), "unset/unset");
}

#[cfg(unix)]
#[test]
fn an_agent_dir_without_agent_env_never_runs_gh() {
    let (fake, _c, b) = crew();
    std::fs::create_dir(b.dir.join("gh-empty-config")).unwrap();
    let (gh, saw) = fake_gh("ghp_test_from_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let o = cb(&fake, &b.dir, &audit(), &[("PATH", path.as_str())]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("owner-token-in-agent-dir"), "{}", o.all());
    assert!(!saw.exists(), "`gh auth token` ran");
    assert!(fake.requests().is_empty());
}
