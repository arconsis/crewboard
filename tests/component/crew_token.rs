//! `crewboard agent token` (old name `crew-token set`): the token goes from standard input to the env file, after the fake
//! GitHub vouched for it. Each refusal has a red proof: nothing is written, the token is in no
//! output.
#![cfg(unix)]

use crate::support::board::{FakeBoard, Out};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const TOKEN: &str = "ghp_crewTokenSECRET1234567890";

struct Setup {
    fake: FakeBoard,
    home: tempfile::TempDir,
}

impl Setup {
    fn new(login: &str, scopes: &str) -> Self {
        let fake = FakeBoard::new();
        fake.set_viewer(login);
        if !scopes.is_empty() {
            fake.set_scopes(scopes);
        }
        Setup {
            fake,
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn env_file(&self) -> PathBuf {
        self.home.path().join(".config/crewboard/my-crew.env")
    }

    /// Run `agent token` with `input` on standard input (a pipe, so no terminal).
    fn set(&self, input: &str, extra: &[&str]) -> Out {
        self.run(&["agent", "token"], input, extra)
    }

    /// Run the command `words` with `input` on standard input and the options `extra`.
    fn run(&self, words: &[&str], input: &str, extra: &[&str]) -> Out {
        let mut c = Command::new(env!("CARGO_BIN_EXE_crewboard"));
        c.args(words)
            .args(extra)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.home.path())
            .env("CREWBOARD_API_BASE", self.fake.base())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let o = child.wait_with_output().unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into(),
            stderr: String::from_utf8_lossy(&o.stderr).into(),
        }
    }
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn a_good_token_is_checked_and_written_private() {
    let s = Setup::new("crew-bot", "project, repo");
    let o = s.set(&format!("{TOKEN}\n"), &["--initiative", "my-crew"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("belongs to crew-bot"), "{}", o.all());
    assert!(o.stdout.contains("scopes project, repo"), "{}", o.all());
    assert!(
        !o.all().contains(TOKEN),
        "the token is printed:\n{}",
        o.all()
    );
    let f = s.env_file();
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        format!("GH_TOKEN={TOKEN}\n")
    );
    assert_eq!(mode(&f), 0o600);
    assert_eq!(mode(f.parent().unwrap()), 0o700);
    // the check sent the token to GitHub as the bearer, once, and to nothing else
    let reqs = s.fake.requests();
    assert!(
        reqs.iter().all(|r| r.bearer.as_deref() == Some(TOKEN)),
        "{reqs:?}"
    );
}

#[test]
fn a_second_run_replaces_the_file() {
    let s = Setup::new("crew-bot", "project, repo");
    assert_eq!(
        s.set(&format!("{TOKEN}\n"), &["--initiative", "my-crew"])
            .code,
        0
    );
    let o = s.set("ghp_otherTOKEN0987654321\n", &["--initiative", "my-crew"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("replaced"), "{}", o.all());
    assert!(
        std::fs::read_to_string(s.env_file())
            .unwrap()
            .contains("otherTOKEN")
    );
}

#[test]
fn a_token_without_the_project_scope_is_refused_and_nothing_is_written() {
    let s = Setup::new("crew-bot", "repo");
    let o = s.set(&format!("{TOKEN}\n"), &["--initiative", "my-crew"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("lacks the scope(s) project"),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("nothing was written"), "{}", o.all());
    assert!(!o.all().contains(TOKEN), "{}", o.all());
    assert!(!s.env_file().exists());
}

#[test]
fn a_token_with_the_workflow_scope_is_refused() {
    let s = Setup::new("crew-bot", "project, repo, workflow");
    let o = s.set(&format!("{TOKEN}\n"), &["--initiative", "my-crew"]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(!s.env_file().exists());
}

#[test]
fn in_trial_mode_the_workflow_scope_passes_with_one_note_and_other_wide_scopes_fail() {
    let s = Setup::new("fake-owner", "project, repo, workflow");
    let o = s.set(
        &format!("{TOKEN}\n"),
        &["--initiative", "my-crew", "--insecure-one-account"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    let notes = o
        .stdout
        .lines()
        .filter(|l| l.contains("trial mode"))
        .count();
    assert_eq!(notes, 1, "{}", o.all());
    assert!(!o.all().contains(TOKEN), "{}", o.all());
    assert!(s.env_file().exists());
    // red proof: the flag forgives `workflow` only
    let s = Setup::new("fake-owner", "project, repo, workflow, delete_repo");
    let o = s.set(
        &format!("{TOKEN}\n"),
        &["--initiative", "my-crew", "--insecure-one-account"],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(!s.env_file().exists());
}

#[test]
fn the_token_of_another_account_is_refused() {
    let s = Setup::new("fake-owner", "project, repo");
    let o = s.set(
        &format!("{TOKEN}\n"),
        &["--initiative", "my-crew", "--crew-login", "crew-bot"],
    );
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("belongs to fake-owner, not to crew-bot"),
        "{}",
        o.all()
    );
    assert!(!s.env_file().exists());
}

#[test]
fn input_that_is_no_token_never_reaches_github() {
    let s = Setup::new("crew-bot", "project, repo");
    for input in ["", "\n", "two words here 123456\n", "ghp_abc;rm -rf\n"] {
        let o = s.set(input, &["--initiative", "my-crew"]);
        assert_eq!(o.code, 4, "{input:?}: {}", o.all());
        assert!(!s.env_file().exists());
    }
    assert!(s.fake.requests().is_empty(), "{:?}", s.fake.requests());
}

#[test]
fn without_an_initiative_the_command_says_what_to_pass() {
    let s = Setup::new("crew-bot", "project, repo");
    let o = s.set(&format!("{TOKEN}\n"), &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("--initiative"), "{}", o.all());
}

#[test]
fn the_old_name_is_gone() {
    let s = Setup::new("crew-bot", "project, repo");
    let o = s.run(
        &["crew-token", "set"],
        &format!("{TOKEN}\n"),
        &["--initiative", "my-crew"],
    );
    assert_eq!(o.code, 64, "{}", o.all());
    assert!(o.stderr.contains("unrecognized subcommand"), "{}", o.all());
}
