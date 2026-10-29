//! `doctor --setup` against a fake GitHub: one ready agent passes every check, and each check
//! has its red proof (a setup where exactly that check fails, with its fix line).
//!
//! Each run gets its own HOME (the runtime check), a PATH with the agent's gh shim first, and a
//! git work dir with the crew identity. Nothing reaches github.com. The fakes are shell
//! scripts, so the file runs on macOS and Linux (the agent hosts), not on Windows.
#![cfg(unix)]

use crate::support::board::{Agent, FakeBoard, OWNER, Out, REPO};
use crewboard::config::ConfigDir;
use crewboard::rules::Role;
use std::path::{Path, PathBuf};
use std::process::Command;

const CREW: &str = "fake-crew";
/// `GET users/{login}` of the fake answers id 2.
const EMAIL: &str = "2+fake-crew@users.noreply.github.com";
const AUTHORITY: &str = "---\ninitiative: fake\nowner: fake-owner\n---\nGoal: a test.\n";
const CHECKS: &[&str] = &[
    "agent config",
    "agent env",
    "token source",
    "agent env keys",
    "agent dir mode",
    "token",
    "token scopes",
    "push access",
    "gh shim",
    "git identity",
    "runtime",
    "board",
    "AUTHORITY.md",
    "registry",
];

struct Setup {
    fake: FakeBoard,
    agent: Agent,
    work: tempfile::TempDir,
    home: tempfile::TempDir,
    /// The directory that the runs put first on the PATH (default: the agent's shim dir).
    path_head: PathBuf,
}

fn write_exe(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `<dir>/agent.env`, a plain file with the given mode.
fn write_env_file(dir: &Path, text: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("agent.env");
    let _ = std::fs::remove_file(&p);
    std::fs::write(&p, text).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {o:?}");
}

/// A fake `gh` that answers `api user --jq .login` with `login`.
fn gh_says(login: &str) -> String {
    format!("#!/bin/sh\necho {login}\n")
}

/// An agent that is ready: approved, crew token, classic token with repo and project, push
/// access, the shim first on the PATH, the crew identity in its checkout, `claude` in
/// ~/.local/bin, AUTHORITY.md merged.
fn ready_with(agent: impl FnOnce(&FakeBoard) -> Agent) -> Setup {
    let fake = FakeBoard::new();
    fake.set_viewer(CREW);
    fake.set_scopes("project, repo");
    fake.set_file(REPO, "AUTHORITY.md", AUTHORITY);
    let agent = agent(&fake);
    // the board names the crew login; the runtime is Claude Code
    let cd = ConfigDir(agent.dir.clone());
    let mut cfg = cd.load_agent("fake").unwrap();
    cfg.board.crew_login = Some(CREW.into());
    cfg.board.insecure_one_account = false;
    cfg.runtime = "claude-code/test-model".into();
    cd.save_agent(&cfg).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&agent.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    write_env_file(&agent.dir, "GH_TOKEN=ghp_faketoken\n", 0o600);
    let shim = agent.dir.join("bin").join("gh");
    write_exe(&shim, &gh_says(CREW));
    let work = tempfile::tempdir().unwrap();
    git(work.path(), &["init", "-q"]);
    git(work.path(), &["config", "user.name", CREW]);
    git(work.path(), &["config", "user.email", EMAIL]);
    let helper = format!("!{} auth git-credential", shim.display());
    git(work.path(), &["config", "credential.helper", &helper]);
    let home = tempfile::tempdir().unwrap();
    write_exe(
        &home.path().join(".local/bin/claude"),
        "#!/bin/sh\nexit 0\n",
    );
    let path_head = agent.dir.join("bin");
    Setup {
        fake,
        agent,
        work,
        home,
        path_head,
    }
}

fn ready() -> Setup {
    ready_with(|f| f.agent("claude@host-b", Role::Dev))
}

fn doctor(s: &Setup) -> Out {
    let path = format!("{}:/usr/bin:/bin", s.path_head.display());
    let home = s.home.path().display().to_string();
    s.fake.run_env(
        &s.agent.dir,
        s.work.path(),
        &["doctor", "--setup"],
        &[
            ("HOME", &home),
            ("PATH", &path),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ],
    )
}

/// The line of one check, and its status word (`ok`, `FAIL`, `skip`).
fn line<'a>(o: &'a Out, check: &str) -> (&'a str, &'a str) {
    let l = o
        .stdout
        .lines()
        .find(|l| {
            l.split_once("  ")
                .is_some_and(|(_, rest)| rest.trim_start().starts_with(&format!("{check}:")))
        })
        .unwrap_or_else(|| panic!("no line for {check}:\n{}", o.all()));
    (l.split_whitespace().next().unwrap(), l)
}

/// Exactly the named check fails, with a fix; every other check is ok or skipped.
fn only_fails(o: &Out, check: &str, fix_part: &str) -> String {
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(o.stderr.contains("rule=setup-incomplete"), "{}", o.all());
    let (st, l) = line(o, check);
    assert_eq!(st, "FAIL", "{}", o.all());
    assert!(l.contains(". Fix: "), "{l}");
    assert!(
        l.contains(fix_part),
        "fix of {check} lacks {fix_part:?}: {l}"
    );
    let fails = o.stdout.lines().filter(|l| l.starts_with("FAIL")).count();
    assert_eq!(fails, 1, "{}", o.all());
    l.to_string()
}

/// The named check fails with a fix, and so does `token` (no client without the env file); every
/// other check is ok or skipped. The fake saw no request.
fn both_fail(o: &Out, check: &str, fix_part: &str) -> String {
    assert_eq!(o.code, 2, "{}", o.all());
    let (st, l) = line(o, check);
    assert_eq!(st, "FAIL", "{}", o.all());
    assert!(l.contains(fix_part), "{l}");
    assert_eq!(line(o, "token").0, "FAIL", "{}", o.all());
    let fails = o.stdout.lines().filter(|l| l.starts_with("FAIL")).count();
    assert_eq!(fails, 2, "{}", o.all());
    l.to_string()
}

#[test]
fn a_ready_agent_passes_every_setup_check_and_writes_nothing() {
    let s = ready();
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    for c in CHECKS {
        let (st, l) = line(&o, c);
        assert_eq!(st, "ok", "{l}\n{}", o.all());
    }
    assert!(
        !o.stdout.contains("FAIL") && !o.stdout.contains("skip"),
        "{}",
        o.all()
    );
    assert!(
        line(&o, "registry")
            .1
            .contains("claude@host-b is approved as dev"),
        "{}",
        o.all()
    );
    assert!(line(&o, "token scopes").1.contains("project, repo"));
    // read only: no REST write, no GraphQL mutation
    for r in s.fake.requests() {
        assert!(
            r.method == "GET" || (r.method == "POST" && r.path == "/graphql"),
            "{} {}",
            r.method,
            r.path
        );
        assert!(!r.body.contains("mutation"), "{}", r.body);
    }
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn red_an_env_file_open_to_others_fails_the_agent_env_check() {
    let s = ready();
    write_env_file(&s.agent.dir, "GH_TOKEN=ghp_faketoken\n", 0o644);
    let o = doctor(&s);
    let l = line(&o, "agent env").1.to_string();
    assert_eq!(line(&o, "agent env").0, "FAIL", "{}", o.all());
    assert!(l.contains("mode 644") && l.contains("chmod 600"), "{l}");
    assert!(l.contains("ln -sfn"), "{l}");
    // the twin: the same file with mode 600 is the ready agent, which passes
    assert!(!l.contains("ghp_faketoken"));
    // no client without the file: the token check fails and the fake saw no request
    assert_eq!(line(&o, "token").0, "FAIL", "{}", o.all());
    assert!(s.fake.requests().is_empty(), "{:?}", s.fake.requests());
}

#[test]
fn red_an_env_file_without_a_token_or_with_a_bad_line_fails_the_agent_env_check() {
    let s = ready();
    write_env_file(&s.agent.dir, "GH_HOST=github.com\n", 0o600);
    let o = doctor(&s);
    // a file that does not load also leaves the token check without a client
    let l = both_fail(&o, "agent env", "ln -sfn");
    assert!(l.contains("no GH_TOKEN or GITHUB_TOKEN"), "{l}");
    assert_eq!(line(&o, "token source").0, "skip", "{}", o.all());
    write_env_file(&s.agent.dir, "GH_TOKEN=ghp_secretvalue more\n", 0o600);
    let o = doctor(&s);
    let l = both_fail(&o, "agent env", "ln -sfn");
    assert!(l.contains("line 1"), "{l}");
    assert!(!o.all().contains("ghp_secretvalue"), "{}", o.all());
}

#[test]
fn red_an_agent_dir_without_a_token_source_fails_the_token_source_check() {
    let s = ready();
    // no agent.env, and the caller has no token variable: crewboard would use the login of gh
    std::fs::remove_file(s.agent.dir.join("agent.env")).unwrap();
    let path = format!("{}:/usr/bin:/bin", s.path_head.display());
    let home = s.home.path().display().to_string();
    let raw = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("--config-dir")
        .arg(&s.agent.dir)
        .args(["doctor", "--setup"])
        .current_dir(s.work.path())
        .env("CREWBOARD_API_BASE", s.fake.base())
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env("HOME", &home)
        .env("PATH", &path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    let o = Out {
        code: raw.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&raw.stdout).to_string(),
        stderr: String::from_utf8_lossy(&raw.stderr).to_string(),
    };
    let (st, l) = line(&o, "token source");
    assert_eq!(st, "FAIL", "{}", o.all());
    assert!(l.contains("gh auth token in an agent dir"), "{l}");
    assert!(l.contains("agent.env"), "{l}");
    // the twin: with a token variable the source is that variable
    let o = doctor(&s);
    assert_eq!(line(&o, "token source").0, "ok", "{}", o.all());
    assert!(line(&o, "token source").1.contains("env GH_TOKEN"));
}

#[test]
fn an_ignored_key_is_named_as_a_warning_and_does_not_fail() {
    let s = ready();
    write_env_file(&s.agent.dir, "GH_TOKEN=ghp_faketoken\nFOO=1\n", 0o600);
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    let (st, l) = line(&o, "agent env keys");
    assert_eq!(st, "ok", "{l}");
    assert!(
        l.contains("warning: agent.env: key FOO ignored (not a crewboard setting)"),
        "{l}"
    );
}

#[test]
fn red_an_agent_dir_that_others_can_enter_fails_the_dir_mode_check() {
    use std::os::unix::fs::PermissionsExt;
    let s = ready();
    std::fs::set_permissions(&s.agent.dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = doctor(&s);
    let l = only_fails(&o, "agent dir mode", "chmod 700");
    assert!(l.contains("is 755"), "{l}");
}

#[test]
fn red_no_agent_config_fails_first_and_skips_the_rest() {
    let s = ready();
    let empty = tempfile::tempdir().unwrap();
    let path = format!("{}:/usr/bin:/bin", s.path_head.display());
    let o = s.fake.run_env(
        empty.path(),
        s.work.path(),
        &["doctor", "--setup"],
        &[("PATH", &path)],
    );
    only_fails(&o, "agent config", "crewboard agent add");
    for c in &CHECKS[1..] {
        if *c != "token scopes" {
            assert_eq!(line(&o, c).0, "skip", "{c}\n{}", o.all());
        }
    }
    assert!(s.fake.requests().is_empty(), "{:?}", s.fake.requests());
}

#[test]
fn red_the_owner_token_fails_the_token_check() {
    let s = ready();
    s.fake.set_viewer(OWNER);
    let o = doctor(&s);
    let l = only_fails(&o, "token", "must hold the token of the crew account");
    assert!(l.contains("this token is the owner fake-owner"), "{l}");
    assert_eq!(line(&o, "board").0, "skip", "{}", o.all());
}

#[test]
fn red_a_classic_token_without_the_project_scope_fails_the_scope_check() {
    let s = ready();
    s.fake.set_scopes("repo, workflow");
    let o = doctor(&s);
    let l = only_fails(&o, "token scopes", "scopes repo and project only");
    assert!(l.contains("lacks the scope(s) project"), "{l}");
    // the pass direction (`project, repo`) is the ready agent; a fine-grained token (no
    // header) is a unit test of `judge_scopes`
}

#[test]
fn red_no_write_access_fails_the_push_check() {
    let s = ready();
    s.fake.set_push(false);
    let o = doctor(&s);
    let l = only_fails(&o, "push access", "with the Write role");
    assert!(
        l.contains("fake-owner/fake-repo: the token has no write access"),
        "{l}"
    );
}

#[test]
fn red_a_shim_that_loads_another_token_fails_the_gh_check() {
    let s = ready();
    write_exe(&s.agent.dir.join("bin").join("gh"), &gh_says(OWNER));
    let o = doctor(&s);
    let l = only_fails(
        &o,
        "gh shim",
        "must hold one line GH_TOKEN=<token of fake-crew>",
    );
    assert!(l.contains("acts as the owner fake-owner"), "{l}");
}

#[test]
fn red_a_shell_without_the_agent_exports_fails_the_gh_check_with_the_exec_fix() {
    let mut s = ready();
    // the owner's gh comes first on the PATH; the shim exists but is not used
    let other = tempfile::tempdir().unwrap();
    write_exe(&other.path().join("gh"), &gh_says(OWNER));
    s.path_head = other.path().to_path_buf();
    let o = doctor(&s);
    only_fails(&o, "gh shim", "crewboard agent exec");
}

#[test]
fn red_a_wrong_git_identity_fails_the_git_check() {
    let s = ready();
    git(
        s.work.path(),
        &["config", "user.email", "someone@example.org"],
    );
    let o = doctor(&s);
    let l = only_fails(&o, "git identity", "crewboard agent shim --agent-dir");
    assert!(l.contains("user.email is someone@example.org"), "{l}");
    assert!(l.contains(&format!("--email {EMAIL}")), "{l}");
}

#[test]
fn red_a_work_dir_that_is_no_checkout_fails_the_git_check() {
    let mut s = ready();
    s.work = tempfile::tempdir().unwrap();
    let o = doctor(&s);
    let l = only_fails(
        &o,
        "git identity",
        "git clone https://github.com/fake-owner/fake-repo",
    );
    assert!(l.contains("is not a git checkout"), "{l}");
}

#[test]
fn a_runtime_outside_the_scheduler_path_passes_with_a_note_and_a_missing_one_fails() {
    let mut s = ready();
    std::fs::remove_file(s.home.path().join(".local/bin/claude")).unwrap();
    // pass: `claude` is on the agent's own PATH (a Homebrew or npm dir), not on the scheduler
    // PATH. `crewboard wake install` stores the absolute path, so this is ok with a note.
    let brew = tempfile::tempdir().unwrap();
    write_exe(&brew.path().join("claude"), "#!/bin/sh\n");
    s.path_head = PathBuf::from(format!(
        "{}:{}",
        s.agent.dir.join("bin").display(),
        brew.path().display()
    ));
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    let (st, l) = line(&o, "runtime");
    assert_eq!(st, "ok", "{l}");
    assert!(
        l.contains(&brew.path().join("claude").display().to_string()),
        "{l}"
    );
    assert!(l.contains("crewboard wake install"), "{l}");
    // red: not on the agent's PATH and not on the scheduler PATH
    s.path_head = s.agent.dir.join("bin");
    let o = doctor(&s);
    let l = only_fails(&o, "runtime", "install claude");
    assert!(l.contains("claude is not on the PATH of this agent"), "{l}");
}

#[test]
fn red_an_unreadable_board_fails_the_board_check() {
    let s = ready();
    s.fake
        .fail_board("Could not resolve to a ProjectV2 with the number 15.");
    let o = doctor(&s);
    only_fails(&o, "board", "the Write role on the board");
    assert_eq!(line(&o, "registry").0, "skip", "{}", o.all());
}

#[test]
fn red_authority_md_missing_or_with_another_owner_fails_its_check() {
    let s = ready();
    s.fake.fail_file(REPO, "AUTHORITY.md", 404);
    let o = doctor(&s);
    only_fails(&o, "AUTHORITY.md", "merges the init pull request");
    let s = ready();
    s.fake.set_file(
        REPO,
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: someone-else\n---\n",
    );
    let o = doctor(&s);
    let l = only_fails(&o, "AUTHORITY.md", "fixes the front matter");
    assert!(l.contains("authority-owner-mismatch"), "{l}");
}

#[test]
fn red_a_pending_agent_fails_the_registry_check_with_the_approve_command() {
    let s = ready_with(|f| f.joined("claude@host-b"));
    let o = doctor(&s);
    let l = only_fails(
        &o,
        "registry",
        "ask the owner to run: crewboard approve claude@host-b --initiative fake",
    );
    assert!(l.contains("the owner has not approved it yet"), "{l}");
}

#[test]
fn setup_and_board_do_not_mix() {
    let s = ready();
    let o = s.fake.run(&s.agent.dir, &["doctor", "--setup", "--board"]);
    assert_ne!(o.code, 0, "{}", o.all());
    assert!(o.stderr.contains("cannot be used with"), "{}", o.all());
}

#[test]
fn a_board_without_a_crew_login_fails_the_token_check_until_there_is_consent() {
    let s = ready();
    let cd = ConfigDir(s.agent.dir.clone());
    let mut cfg = cd.load_agent("fake").unwrap();
    cfg.board.crew_login = None;
    cfg.board.insecure_one_account = false;
    cd.save_agent(&cfg).unwrap();

    let o = doctor(&s);
    let (st, l) = line(&o, "token");
    assert_eq!(st, "FAIL", "{}", o.all());
    assert!(
        l.contains("one-account") || l.contains("no crew_login"),
        "{l}"
    );
    assert!(l.contains("start --insecure-one-account"), "{l}");

    // with the consent of the agent the check passes and says what the identity is
    cd.set_agent_one_account("fake").unwrap();
    let o = doctor(&s);
    let (st, l) = line(&o, "token");
    assert_eq!(st, "ok", "{}", o.all());
    assert!(l.contains("identity: ONE ACCOUNT (insecure)"), "{l}");
}

#[test]
fn in_trial_mode_the_workflow_scope_is_an_ok_note_and_for_a_real_crew_it_still_fails() {
    // a real crew: the workflow scope is a failure
    let s = ready();
    s.fake.set_scopes("project, repo, workflow");
    let o = doctor(&s);
    let l = only_fails(&o, "token scopes", "scopes repo and project only");
    assert!(l.contains("workflow"), "{l}");

    // trial mode (one account, the agent consents): the same token is an ok note
    let cd = ConfigDir(s.agent.dir.clone());
    let mut cfg = cd.load_agent("fake").unwrap();
    cfg.board.crew_login = None;
    cfg.board.insecure_one_account = false;
    cd.save_agent(&cfg).unwrap();
    cd.set_agent_one_account("fake").unwrap();
    let o = doctor(&s);
    let (st, l) = line(&o, "token scopes");
    assert_eq!(st, "ok", "{}", o.all());
    assert!(
        l.contains("trial mode: one account; the workflow scope is expected"),
        "{l}"
    );
    // another wide scope stays a failure in trial mode
    s.fake.set_scopes("project, repo, workflow, delete_repo");
    let o = doctor(&s);
    assert_eq!(line(&o, "token scopes").0, "FAIL", "{}", o.all());
}

const NO_PROTECTED_PATHS: &str = "warning: Gate: ci tickets merge without a diff review; list the files that define your CI and tests in protected_paths";

fn make_headless(s: &Setup) {
    let toml = ConfigDir(s.agent.dir.clone()).agent_config_path("fake");
    let text = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(&toml, format!("headless = true\n{text}")).unwrap();
}

fn warnings(o: &Out) -> Vec<&str> {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("warning: Gate: ci"))
        .collect()
}

#[test]
fn a_headless_agent_with_no_protected_paths_gets_one_warning_and_exit_0() {
    let s = ready();
    make_headless(&s);
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    let w = warnings(&o);
    assert_eq!(w.len(), 1, "{}", o.all());
    assert!(w[0].starts_with(NO_PROTECTED_PATHS), "{}", w[0]);
    // the checks themselves are unchanged
    for c in CHECKS {
        assert_eq!(line(&o, c).0, "ok", "{}", o.all());
    }
    assert!(s.fake.unhandled().is_empty(), "{:?}", s.fake.unhandled());
}

#[test]
fn red_twins_no_warning_with_protected_paths_or_without_the_headless_key() {
    // headless, and the list is set: no warning
    let s = ready();
    make_headless(&s);
    s.fake.set_file(
        REPO,
        "AUTHORITY.md",
        "---\ninitiative: fake\nowner: fake-owner\nprotected_paths: [tests/**]\n---\nGoal: a test.\n",
    );
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(warnings(&o).is_empty(), "{}", o.all());
    // not headless, and the list is empty: no warning (this host has no headless coordinator)
    let s = ready();
    let o = doctor(&s);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(warnings(&o).is_empty(), "{}", o.all());
}
