//! `crewboard agent shim`: the generated `gh` stub, the real `gh`, the refusal to overwrite a file
//! that is not a crewboard stub, and the git identity of a checkout.
#![cfg(unix)]

use super::agent_add::{Setup, run_deadline, write_exe};
use crate::support::board::Out;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const EXE: &str = env!("CARGO_BIN_EXE_crewboard");

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// An agent dir with `agent.env` linked to the env file of the setup.
fn agent_dir(s: &Setup) -> PathBuf {
    let d = s.path("agent");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink(&s.env_file, d.join("agent.env")).unwrap();
    d
}

fn shim(s: &Setup, extra: &[&str], path: &str) -> Out {
    let mut c = Command::new(EXE);
    c.arg("agent")
        .arg("shim")
        .args(extra)
        .env_clear()
        .env("PATH", path)
        .env("HOME", s.path("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    run_deadline(c, Duration::from_secs(60))
}

fn std_path(s: &Setup) -> String {
    format!("{}:/usr/bin:/bin", s.realgh.display())
}

#[test]
fn the_stub_is_the_three_lines_with_absolute_paths_mode_755() {
    let s = Setup::new();
    let d = agent_dir(&s);
    let o = shim(
        &s,
        &["--agent-dir", &d.display().to_string()],
        &std_path(&s),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("[1/1] gh shim: done "), "{}", o.stdout);
    assert_eq!(
        std::fs::read_to_string(d.join("bin/gh")).unwrap(),
        format!(
            "#!/bin/sh\n# crewboard gh shim for {d}: runs the real gh as the crew account.\nexec '{EXE}' agent with-env --env-file '{d}/agent.env' -- '{gh}' \"$@\"\n",
            d = d.display(),
            gh = s.realgh.join("gh").display()
        )
    );
    assert_eq!(mode(&d.join("bin/gh")), 0o755);
    assert_eq!(mode(&d.join("gh-empty-config")), 0o700);
    // no temp file stays
    let names: Vec<_> = std::fs::read_dir(d.join("bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("gh")]);
}

#[test]
fn a_second_run_is_already_done_and_the_real_gh_is_never_the_stub() {
    let s = Setup::new();
    let d = agent_dir(&s);
    let dir = d.display().to_string();
    assert_eq!(shim(&s, &["--agent-dir", &dir], &std_path(&s)).code, 0);
    // the agent PATH has the stub first: the real gh is still the second one
    let path = format!("{}/bin:{}", d.display(), std_path(&s));
    let o = shim(&s, &["--agent-dir", &dir], &path);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("[1/1] gh shim: already done "),
        "{}",
        o.stdout
    );
    assert!(
        std::fs::read_to_string(d.join("bin/gh"))
            .unwrap()
            .contains(&format!("-- '{}'", s.realgh.join("gh").display())),
        "the stub names another gh"
    );
}

#[test]
fn the_stub_runs_the_real_gh_with_the_crew_token_and_a_bare_gh_has_none() {
    let s = Setup::new();
    let d = agent_dir(&s);
    assert_eq!(
        shim(
            &s,
            &["--agent-dir", &d.display().to_string()],
            &std_path(&s)
        )
        .code,
        0
    );
    let via = Command::new(d.join("bin/gh"))
        .args(["version"])
        .env_clear()
        // no HOME: a runtime may strip it, and the stub must still find the crew token
        .env("PATH", "/usr/bin:/bin")
        .env("GH_TOKEN", "ghp_caller_owner_token")
        .output()
        .unwrap();
    let via = String::from_utf8_lossy(&via.stdout).to_string();
    assert!(
        via.contains("gh-saw token=add-token-SECRET-123456 args=version"),
        "{via}"
    );
    // the stub loads the file, not the variable of the caller
    assert!(!via.contains("ghp_caller_owner_token"), "{via}");
    let bare = Command::new(s.realgh.join("gh"))
        .args(["version"])
        .env_clear()
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&bare.stdout).contains("token=unset"));
}

#[test]
fn a_file_that_is_not_a_crewboard_stub_is_refused_and_kept() {
    let s = Setup::new();
    let d = agent_dir(&s);
    write_exe(&d.join("bin/gh"), "#!/bin/sh\necho mine\n");
    let o = shim(
        &s,
        &["--agent-dir", &d.display().to_string()],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("is not a crewboard gh stub"),
        "{}",
        o.all()
    );
    assert_eq!(
        std::fs::read_to_string(d.join("bin/gh")).unwrap(),
        "#!/bin/sh\necho mine\n"
    );
    // twin: the older stub is replaced
    write_exe(
        &d.join("bin/gh"),
        "#!/bin/bash\n# crewboard gh shim: run the real gh as the crew account (made by an older crewboard).\nexec bash x\n",
    );
    let o = shim(
        &s,
        &["--agent-dir", &d.display().to_string()],
        &std_path(&s),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        std::fs::read_to_string(d.join("bin/gh"))
            .unwrap()
            .contains("agent with-env")
    );
}

#[test]
fn no_real_gh_a_missing_dir_and_a_missing_env_file_are_exit_4() {
    let s = Setup::new();
    let d = agent_dir(&s);
    let dir = d.display().to_string();
    // an empty dir: a runner may have a gh in /usr/bin, so no system dir can be on this PATH
    let empty = s.path("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let o = shim(&s, &["--agent-dir", &dir], &empty.display().to_string());
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("no real gh found"), "{}", o.all());
    assert!(!d.join("bin/gh").exists());
    let o = shim(
        &s,
        &["--agent-dir", &s.path("none").display().to_string()],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("agent dir not found"), "{}", o.all());
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &dir,
            "--env-file",
            &s.path("none.env").display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("env file not found"), "{}", o.all());
    // a --real-gh that is not executable
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &dir,
            "--real-gh",
            &s.path("nogh").display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
}

#[test]
fn an_explicit_env_file_is_the_one_the_stub_names() {
    let s = Setup::new();
    let d = s.path("bare");
    std::fs::create_dir_all(&d).unwrap();
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &d.display().to_string(),
            "--env-file",
            &s.env_file.display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        std::fs::read_to_string(d.join("bin/gh"))
            .unwrap()
            .contains(&format!("--env-file '{}'", s.env_file.display())),
    );
}

fn checkout(s: &Setup, name: &str) -> PathBuf {
    let w = s.path(name);
    std::fs::create_dir_all(&w).unwrap();
    let o = Command::new("git")
        .arg("-C")
        .arg(&w)
        .args(["init", "-q"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(o.status.success());
    w
}

fn conf(w: &Path, key: &str) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(w)
        .args(["config", key])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

#[test]
fn work_dir_gets_the_crew_identity_from_the_crew_id_file() {
    let s = Setup::new();
    let d = agent_dir(&s);
    std::fs::write(d.join("crew-id"), "crew-bot 4242\n").unwrap();
    let w = checkout(&s, "wd");
    let a = [
        "--agent-dir",
        &d.display().to_string(),
        "--work-dir",
        &w.display().to_string(),
    ]
    .map(String::from);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let o = shim(&s, &a, &std_path(&s));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("[2/2] git config: done (crew-bot <4242+crew-bot@users.noreply.github.com>)"),
        "{}",
        o.stdout
    );
    assert_eq!(conf(&w, "user.name"), "crew-bot");
    assert_eq!(
        conf(&w, "user.email"),
        "4242+crew-bot@users.noreply.github.com"
    );
    let o = shim(&s, &a, &std_path(&s));
    assert!(
        o.stdout.contains("[2/2] git config: already done"),
        "{}",
        o.stdout
    );
}

#[test]
fn work_dir_without_a_crew_id_or_a_checkout_is_refused() {
    let s = Setup::new();
    let d = agent_dir(&s);
    let w = checkout(&s, "wd");
    let dir = d.display().to_string();
    let o = shim(
        &s,
        &["--agent-dir", &dir, "--work-dir", &w.display().to_string()],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("no crew login"), "{}", o.all());
    assert_eq!(conf(&w, "user.email"), "");
    std::fs::write(d.join("crew-id"), "crew-bot 4242\n").unwrap();
    let plain = s.path("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &dir,
            "--work-dir",
            &plain.display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("not a git checkout"), "{}", o.all());
    // a login that differs from the cached one has no id
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &dir,
            "--work-dir",
            &w.display().to_string(),
            "--crew-login",
            "other",
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("no id for the crew login other"),
        "{}",
        o.all()
    );
}

#[test]
fn the_compat_options_of_the_old_script_make_the_stub_in_another_dir_and_only_the_git_identity() {
    let s = Setup::new();
    let d = s.path("agent5");
    std::fs::create_dir_all(&d).unwrap();
    let bin = d.join("shims");
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &d.display().to_string(),
            "--bin-dir",
            &bin.display().to_string(),
            "--env-file",
            &s.env_file.display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(bin.join("gh").is_file() && d.join("gh-empty-config").is_dir());
    // only the git identity: no stub is written
    let w = checkout(&s, "wd");
    let d2 = s.path("agent6");
    let o = shim(
        &s,
        &[
            "--only-git",
            "--agent-dir",
            &d2.display().to_string(),
            "--work-dir",
            &w.display().to_string(),
            "--crew-login",
            "crew-bot",
            "--email",
            "crew@users.noreply.example",
        ],
        "/usr/bin:/bin",
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(conf(&w, "user.email"), "crew@users.noreply.example");
    assert!(!d2.exists(), "--only-git writes no stub and no dir");
}

#[test]
fn a_linked_worktree_gets_the_identity_in_its_own_scope_and_the_main_checkout_keeps_its_own() {
    let s = Setup::new();
    let d = agent_dir(&s);
    std::fs::write(d.join("crew-id"), "crew-bot 4242\n").unwrap();
    let main = checkout(&s, "owner");
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(o.status.success(), "{args:?}: {o:?}");
    };
    git(&["config", "user.email", "owner@example.org"]);
    git(&["config", "user.name", "owner"]);
    git(&[
        "-c",
        "user.email=o@x",
        "-c",
        "user.name=o",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    let wt = s.path("wt");
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "crewwt",
        &wt.display().to_string(),
    ]);
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &d.display().to_string(),
            "--work-dir",
            &wt.display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        conf(&wt, "user.email"),
        "4242+crew-bot@users.noreply.github.com"
    );
    assert_eq!(conf(&main, "user.email"), "owner@example.org");
    assert_eq!(conf(&main, "url.https://github.com/.insteadOf"), "");
    assert_eq!(
        conf(&wt, "url.https://github.com/.insteadOf"),
        "git@github.com:"
    );
}

#[test]
fn with_env_works_without_home_for_an_explicit_file_and_not_without_one() {
    let s = Setup::new();
    let run = |args: &[&str]| {
        let mut c = Command::new(EXE);
        c.args(args)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GH_TOKEN", "ghp_caller_owner_token");
        run_deadline(c, Duration::from_secs(60))
    };
    let env = s.env_file.display().to_string();
    let o = run(&[
        "agent",
        "with-env",
        "--env-file",
        &env,
        "--",
        "/bin/sh",
        "-c",
        "printf %s \"$GH_TOKEN\"",
    ]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(o.stdout, "add-token-SECRET-123456");
    // twin: without a file there is no config dir and no token: `init` reads the token
    // stops with exit 4 and does not fall back to `gh auth token` (the owner's login)
    let spy = s.path("spy");
    let log = s.path("gh-was-called");
    write_exe(
        &spy.join("gh"),
        &format!(
            "#!/bin/sh\necho called > '{}'\necho ghp_owner_from_gh\n",
            log.display()
        ),
    );
    let mut c = Command::new(EXE);
    c.args([
        "init",
        "--initiative",
        "demo",
        "--repo",
        "o/r",
        "--crew-login",
        "crew-bot",
    ])
    .env_clear()
    .env("PATH", format!("{}:/usr/bin:/bin", spy.display()));
    let o = run_deadline(c, Duration::from_secs(60));
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("home directory"), "{}", o.all());
    assert!(!log.exists(), "`gh auth token` was used");
}

#[test]
fn shim_has_no_default_agent_dir_and_writes_nothing_without_one() {
    let s = Setup::new();
    let cfg = s.path("home/.config/crewboard");
    std::fs::create_dir_all(&cfg).unwrap();
    let o = shim(&s, &[], &std_path(&s));
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr.contains("no agent dir") && o.stderr.contains("--agent-dir"),
        "{}",
        o.all()
    );
    assert_eq!(
        std::fs::read_dir(&cfg).unwrap().count(),
        0,
        "something was written"
    );
    assert!(!s.path("home/.config/crewboard/bin").exists());
    // twin: $CREWBOARD_CONFIG_DIR names the dir
    let d = agent_dir(&s);
    let mut c = Command::new(EXE);
    c.args(["agent", "shim"])
        .env_clear()
        .env("PATH", std_path(&s))
        .env("HOME", s.path("home"))
        .env("CREWBOARD_CONFIG_DIR", &d);
    let o = run_deadline(c, Duration::from_secs(60));
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(d.join("bin/gh").is_file());
}

#[test]
fn the_stub_of_another_agent_is_not_taken_as_the_real_gh() {
    let s = Setup::new();
    let a = agent_dir(&s);
    assert_eq!(
        shim(
            &s,
            &["--agent-dir", &a.display().to_string()],
            &std_path(&s)
        )
        .code,
        0
    );
    // a second agent: --real-gh names the first stub: refused, nothing written
    let b = s.path("agent-b");
    std::fs::create_dir_all(&b).unwrap();
    let envb = s.env_file.display().to_string();
    let o = shim(
        &s,
        &[
            "--agent-dir",
            &b.display().to_string(),
            "--env-file",
            &envb,
            "--real-gh",
            &a.join("bin/gh").display().to_string(),
        ],
        &std_path(&s),
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("crewboard gh stub"), "{}", o.all());
    assert!(!b.join("bin/gh").exists());
    // on the PATH, the first agent's stub is skipped and the real gh is found
    let path = format!("{}/bin:{}", a.display(), std_path(&s));
    let o = shim(
        &s,
        &["--agent-dir", &b.display().to_string(), "--env-file", &envb],
        &path,
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        std::fs::read_to_string(b.join("bin/gh"))
            .unwrap()
            .contains(&format!("-- '{}'", s.realgh.join("gh").display()))
    );
}

#[test]
fn git_runs_without_the_token_variables_of_the_caller() {
    let s = Setup::new();
    let d = agent_dir(&s);
    std::fs::write(d.join("crew-id"), "crew-bot 4242\n").unwrap();
    let w = checkout(&s, "wd");
    // a git that records its environment, then runs the real one
    let log = s.path("git-env");
    write_exe(
        &s.path("fakegit/git"),
        &format!(
            "#!/bin/sh\nenv >> '{}'\nexec /usr/bin/git \"$@\"\n",
            log.display()
        ),
    );
    let mut c = Command::new(EXE);
    c.args([
        "agent",
        "shim",
        "--agent-dir",
        &d.display().to_string(),
        "--work-dir",
        &w.display().to_string(),
    ])
    .env_clear()
    .env(
        "PATH",
        format!("{}:{}", s.path("fakegit").display(), std_path(&s)),
    )
    .env("HOME", s.path("home"))
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("GH_TOKEN", "ghp_caller_owner_token")
    .env("GITHUB_TOKEN", "ghp_caller_owner_token2")
    .env("GH_ENTERPRISE_TOKEN", "ghe_caller3")
    .env("GITHUB_ENTERPRISE_TOKEN", "ghe_caller4");
    let o = run_deadline(c, Duration::from_secs(60));
    assert_eq!(o.code, 0, "{}", o.all());
    let seen = std::fs::read_to_string(&log).unwrap();
    assert!(seen.contains("PATH="), "the fake git did not run: {seen}");
    assert!(
        !seen.contains("TOKEN"),
        "a token variable reached git: {seen}"
    );
}
