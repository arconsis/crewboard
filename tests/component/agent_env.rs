//! The env file of an agent: which token a command uses, and what it refuses.
//!
//! The fake GitHub records the bearer of each request, so a test sees WHICH token reached the
//! API. Each test sets the environment of the child explicitly: the token variables are removed
//! first, so a token of the machine running the tests never leaks in.
#![cfg(unix)]

use crate::support::board::{Agent, FakeBoard, Out};
use crate::support::cli::crew;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

fn write(path: &Path, text: &str, mode: u32) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// `crewboard --config-dir <dir> <args>` against the fake, with no token variable and no `gh`
/// of the machine. `env` adds variables.
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
        .env("PATH", "/usr/bin:/bin")
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

/// A fake `gh` in a new dir: `gh auth token` prints `token` and leaves a marker file.
fn fake_gh(token: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("gh-ran");
    write(
        &d.path().join("gh"),
        &format!(
            "#!/bin/sh\n: >'{}'\n[ \"$1 $2\" = 'auth token' ] && echo {token}\n",
            marker.display()
        ),
        0o755,
    );
    (d, marker)
}

fn agent_with_env_file(text: &str, mode: u32) -> (FakeBoard, Agent) {
    let (fake, _c, b) = crew();
    write(&b.dir.join("agent.env"), text, mode);
    (fake, b)
}

#[test]
fn file_token_beats_env_token_in_agent_dir() {
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o600);
    let o = cb(
        &fake,
        &b.dir,
        &["status"],
        &[
            ("GH_TOKEN", "ghp_test_env"),
            ("GITHUB_TOKEN", "ghp_test_env2"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_file"], "{}", o.all());
    assert!(!o.all().contains("ghp_test_"), "{}", o.all());
}

#[test]
fn plain_dir_keeps_env_token() {
    // the twin of the test above: no agent.env, so the variable is the token
    let (fake, _c, b) = crew();
    let o = cb(&fake, &b.dir, &["status"], &[("GH_TOKEN", "ghp_test_env")]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_env"]);
    // GITHUB_TOKEN alone works too
    let o = cb(
        &fake,
        &b.dir,
        &["status"],
        &[("GITHUB_TOKEN", "ghp_test_env2")],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(bearers(&fake).contains(&"ghp_test_env2".to_string()));
}

#[test]
fn an_explicit_env_file_beats_the_caller_token_in_a_plain_dir() {
    let (fake, _c, b) = crew();
    let f = b.dir.join("elsewhere.env");
    write(&f, "GH_TOKEN=ghp_test_file\n", 0o600);
    let o = cb(
        &fake,
        &b.dir,
        &["--env-file", f.to_str().unwrap(), "status"],
        &[("GH_TOKEN", "ghp_test_env")],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_file"]);
}

#[test]
fn gh_fallback_works_in_plain_dir() {
    let (fake, _c, b) = crew();
    let (gh, marker) = fake_gh("ghp_test_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let o = cb(&fake, &b.dir, &["status"], &[("PATH", path.as_str())]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_gh"]);
    assert!(marker.exists());
}

#[test]
fn gh_fallback_refused_in_agent_dir() {
    let (fake, _c, b) = crew();
    // an agent dir by its empty gh config dir; no agent.env, no token variable
    std::fs::create_dir(b.dir.join("gh-empty-config")).unwrap();
    let (gh, marker) = fake_gh("ghp_test_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let o = cb(&fake, &b.dir, &["status"], &[("PATH", path.as_str())]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("rule=owner-token-in-agent-dir"),
        "{}",
        o.all()
    );
    assert!(o.stderr.contains("agent.env"), "{}", o.all());
    // gh was not asked and the API saw nothing
    assert!(!marker.exists(), "gh ran");
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    // the twin: a token variable in the same dir is used (a plain dir test shows the same)
    let o = cb(&fake, &b.dir, &["status"], &[("GH_TOKEN", "ghp_test_env")]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn no_token_in_file_exits_4() {
    let (fake, b) = agent_with_env_file("GH_HOST=github.com\nGH_TOKEN=\n", 0o600);
    let o = cb(&fake, &b.dir, &["status"], &[("GH_TOKEN", "ghp_test_env")]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("agent.env has no GH_TOKEN or GITHUB_TOKEN"),
        "{}",
        o.all()
    );
    // fail closed: the token of the caller is NOT used instead
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

#[test]
fn mode_644_exits_4() {
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o644);
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("mode 644"), "{}", o.all());
    assert!(!o.all().contains("ghp_test_file"));
    assert!(fake.requests().is_empty());
    // the mode of the target counts, not that of a link
    let (fake, _c, b) = crew();
    let target = b.dir.join("crew.env");
    write(&target, "GH_TOKEN=ghp_test_file\n", 0o644);
    std::os::unix::fs::symlink(&target, b.dir.join("agent.env")).unwrap();
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_file"]);
}

#[test]
fn bad_line_exits_4_without_the_value() {
    let (fake, b) =
        agent_with_env_file("GH_HOST=github.com\nGH_TOKEN=ghp_test_secret more\n", 0o600);
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("agent.env line 2:"), "{}", o.all());
    assert!(o.stderr.contains("space"), "{}", o.all());
    assert!(!o.all().contains("ghp_test_secret"), "{}", o.all());
    assert!(fake.requests().is_empty());
    // the twin: the same value in quotes loads, with its space
    write(
        &b.dir.join("agent.env"),
        "GH_TOKEN=\"ghp_test_secret more\"\n",
        0o600,
    );
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_secret more"]);
}

#[test]
fn nothing_in_the_file_is_run() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("ran");
    let (fake, b) = agent_with_env_file(
        &format!(
            "GH_TOKEN=$(touch${{IFS}}{m})\nGH_X=`touch${{IFS}}{m}`\n",
            m = marker.display()
        ),
        0o600,
    );
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!marker.exists(), "the file was run as a script");
    // the text reaches the API as it is
    assert_eq!(bearers(&fake).len(), 1);
    assert!(
        bearers(&fake)[0].starts_with("$(touch"),
        "{:?}",
        bearers(&fake)
    );
}

#[test]
fn a_key_outside_the_list_warns_once_and_is_not_loaded() {
    let (fake, b) = agent_with_env_file(
        "GH_TOKEN=ghp_test_file\nFOO=1\nCREWBOARD_HEARTBEAT_INTERVAL=10m\n",
        0o600,
    );
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    let warns = o
        .stderr
        .lines()
        .filter(|l| l.contains("key FOO ignored"))
        .count();
    assert_eq!(warns, 1, "{}", o.all());
    assert!(
        o.stderr
            .contains("agent.env: key FOO ignored (not a crewboard setting)"),
        "{}",
        o.all()
    );
    assert!(!o.stderr.contains("CREWBOARD_HEARTBEAT_INTERVAL"));
}

#[test]
fn monitor_prints_a_bad_env_file_as_its_one_line() {
    let (fake, _c, b) = crew();
    let f = b.dir.join("open.env");
    write(&f, "GH_TOKEN=ghp_test_file\n", 0o644);
    let o = cb(
        &fake,
        &b.dir,
        &[
            "--env-file",
            f.to_str().unwrap(),
            "monitor",
            "--pause",
            "0",
            // a loop that did load the file must end by itself, not wait for events
            "--timeout",
            "1s",
            "--max-cycles",
            "1",
        ],
        &[],
    );
    assert_eq!(o.code, 4, "{}", o.all());
    assert_eq!(o.stdout.lines().count(), 1, "{}", o.all());
    assert!(
        o.stdout.starts_with("crewboard-watch: exit 4: ") && o.stdout.contains("mode 644"),
        "{}",
        o.all()
    );
    assert!(fake.requests().is_empty());
}

// ---- a file that does not load stops every command that needs a token ----

#[test]
fn a_broken_agent_env_fails_closed_and_the_api_sees_nothing() {
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o644);
    let (gh, marker) = fake_gh("ghp_test_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    // with a token variable of the caller, and with only the `gh` login: neither takes over
    for env in [
        &[("GH_TOKEN", "ghp_test_env"), ("PATH", path.as_str())][..],
        &[("PATH", path.as_str())][..],
    ] {
        // `doctor --board` never changes its exit code: it prints the loader error as a warning
        let o = cb(&fake, &b.dir, &["doctor", "--board"], env);
        assert_eq!(o.code, 0, "{}", o.all());
        assert!(o.stdout.contains("mode 644"), "{}", o.all());
        for args in [&["status"][..], &["monitor", "--max-cycles", "1"][..]] {
            let o = cb(&fake, &b.dir, args, env);
            assert_eq!(o.code, 4, "{args:?}: {}", o.all());
            assert!(o.all().contains("mode 644"), "{args:?}: {}", o.all());
        }
        // `doctor --setup` reports the broken file, and exits 2 as for any failed check
        let o = cb(&fake, &b.dir, &["doctor", "--setup"], env);
        assert_eq!(o.code, 2, "{}", o.all());
        let l = o
            .stdout
            .lines()
            .find(|l| l.contains("agent env:"))
            .expect("agent env line");
        assert!(l.starts_with("FAIL") && l.contains("mode 644"), "{l}");
        assert!(
            o.stdout
                .lines()
                .any(|l| l.starts_with("FAIL") && l.contains("token:")),
            "{}",
            o.all()
        );
    }
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    assert!(!marker.exists(), "gh was asked for a token");
}

#[test]
fn the_events_hook_with_a_broken_agent_env_exits_0_says_nothing_and_uses_no_token() {
    // the hook never blocks a tool call: its poll fails with exit 4 (no token is loaded), the
    // failure is one log line, and neither the token variable nor `gh` takes over
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o644);
    let (gh, marker) = fake_gh("ghp_test_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let tmp = tempfile::tempdir().unwrap();
    let events = tmp.path().join("events.txt");
    std::fs::write(&events, "== Events:\n").unwrap();
    let log = tmp.path().join("hook.log");
    let ev = events.to_str().unwrap();
    let o = cb(
        &fake,
        &b.dir,
        &[
            "hook",
            "post-tool-use",
            "--every",
            "0",
            "--log",
            log.to_str().unwrap(),
        ],
        &[
            ("GH_TOKEN", "ghp_test_env"),
            ("PATH", path.as_str()),
            ("CREWBOARD_EVENTS_FILE", ev),
        ],
    );
    assert_eq!((o.code, o.stdout.as_str()), (0, ""), "{}", o.all());
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        text.contains("wake-events-hook: crewboard poll failed with exit 4; no events delivered"),
        "{text}"
    );
    assert_eq!(std::fs::read_to_string(&events).unwrap(), "== Events:\n");
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
    assert!(!marker.exists(), "gh was asked for a token");
}

#[test]
fn a_config_dir_that_cannot_be_found_fails_closed() {
    // no --config-dir, no CREWBOARD_CONFIG_DIR, no HOME: nothing may fall back to another token
    let (fake, _c, _b) = crew();
    let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .arg("status")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("CREWBOARD_API_BASE", fake.base())
        .env("GH_TOKEN", "ghp_test_env")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(fake.requests().is_empty());
}

// ---- the other precedence rows ----

#[test]
fn an_env_file_flag_beats_a_different_agent_env_in_an_agent_dir() {
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_agent\n", 0o600);
    let f = b.dir.join("other.env");
    write(&f, "GH_TOKEN=ghp_test_flag\n", 0o600);
    let o = cb(
        &fake,
        &b.dir,
        &["--env-file", f.to_str().unwrap(), "status"],
        &[("GH_TOKEN", "ghp_test_env")],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(bearers(&fake), ["ghp_test_flag"]);
    // the twin: without the flag the agent.env decides
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(bearers(&fake).contains(&"ghp_test_agent".to_string()));
}

#[test]
fn an_agent_dir_marked_only_by_bin_gh_is_refused_without_a_token() {
    let (fake, _c, b) = crew();
    std::fs::create_dir(b.dir.join("bin")).unwrap();
    write(&b.dir.join("bin").join("gh"), "#!/bin/sh\n", 0o755);
    let (gh, marker) = fake_gh("ghp_test_gh");
    let path = format!("{}:/usr/bin:/bin", gh.path().display());
    let o = cb(&fake, &b.dir, &["status"], &[("PATH", path.as_str())]);
    assert_eq!(o.code, 2, "{}", o.all());
    assert!(
        o.stderr.contains("rule=owner-token-in-agent-dir"),
        "{}",
        o.all()
    );
    assert!(!marker.exists() && fake.requests().is_empty());
}

#[test]
fn a_file_of_another_user_is_refused() {
    // The owner check needs a file that belongs to someone else. Only root can make one, so this
    // test does nothing for other users; the check itself is a unit test with an injected uid
    // (`agent_env::tests::mode_and_owner_checks`).
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o600);
    let f = b.dir.join("agent.env");
    let c = std::ffi::CString::new(f.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::chown(c.as_ptr(), 65534, 65534) }, 0);
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("belongs to uid 65534"), "{}", o.all());
}

#[test]
fn a_command_substitution_value_is_an_error_and_never_runs() {
    let d = tempfile::tempdir().unwrap();
    let marker = d.path().join("ran");
    let (fake, b) =
        agent_with_env_file(&format!("GH_TOKEN=$(touch {})\n", marker.display()), 0o600);
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(o.stderr.contains("agent.env line 1:"), "{}", o.all());
    assert!(!marker.exists(), "the value was run");
    // and the line that reads a file
    write(
        &b.dir.join("agent.env"),
        "GH_TOKEN=$(cat /etc/passwd)\n",
        0o600,
    );
    let o = cb(&fake, &b.dir, &["status"], &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(!o.all().contains("root:"), "{}", o.all());
}

#[test]
fn the_monitor_child_gets_the_file_token_and_loses_the_caller_variables() {
    let (fake, b) = agent_with_env_file("GH_TOKEN=ghp_test_file\n", 0o600);
    let d = tempfile::tempdir().unwrap();
    let rec = d.path().join("rec");
    let stub = d.path().join("stub");
    write(
        &stub,
        &format!(
            "#!/bin/sh\nprintf '%s|%s|%s' \"$GH_TOKEN\" \"${{GITHUB_TOKEN:-unset}}\" \"${{GH_ENTERPRISE_TOKEN:-unset}}\" >'{}'\nexit 1\n",
            rec.display()
        ),
        0o755,
    );
    let o = cb(
        &fake,
        &b.dir,
        &[
            "monitor",
            "--crewboard",
            stub.to_str().unwrap(),
            "--max-cycles",
            "1",
            "--pause",
            "0",
        ],
        &[
            ("GH_TOKEN", "ghp_test_env"),
            ("GITHUB_TOKEN", "ghp_test_env2"),
            ("GH_ENTERPRISE_TOKEN", "ghp_test_env3"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        std::fs::read_to_string(&rec).unwrap(),
        "ghp_test_file|unset|unset"
    );
}

#[test]
fn the_lock_probe_child_gets_the_file_token_and_loses_the_caller_variables() {
    use crate::support::lock::{BIN, HOST, Rig, now};
    let (_fake, _c, b) = crew();
    write(&b.dir.join("agent.env"), "GH_TOKEN=ghp_test_file\n", 0o600);
    let r = Rig::new();
    r.plant("l", &b.name, HOST, r.live(), now());
    let d = tempfile::tempdir().unwrap();
    let rec = d.path().join("rec");
    let stub = d.path().join("stub");
    write(
        &stub,
        &format!(
            "#!/bin/sh\nprintf '%s|%s' \"$GH_TOKEN\" \"${{GITHUB_TOKEN:-unset}}\" >'{}'\necho '{{\"agents\":[{{\"name\":\"{}\",\"liveness\":\"alive\",\"state\":\"active\"}}]}}'\n",
            rec.display(),
            b.name
        ),
        0o755,
    );
    let live = r.live().to_string();
    let o = Command::new(BIN)
        .arg("--config-dir")
        .arg(&b.dir)
        .args([
            "lock",
            "take",
            &r.s("l"),
            "--holder",
            "new@x",
            "--pid",
            &live,
        ])
        .args(["--crewboard", stub.to_str().unwrap()])
        .env("HOST_LOCK_HOST", HOST)
        .env("HOST_LOCK_PS_CMD", format!("cat {}", r.s("ps.txt")))
        .env("GH_TOKEN", "ghp_test_env")
        .env("GITHUB_TOKEN", "ghp_test_env2")
        .env_remove("CREWBOARD_CONFIG_DIR")
        .output()
        .unwrap();
    // the owner agent is alive on the stub board: held, 75
    assert_eq!(
        o.status.code(),
        Some(75),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&rec).unwrap(),
        "ghp_test_file|unset"
    );
}

#[test]
fn lock_run_in_an_agent_dir_gives_the_job_no_token_variable() {
    use crate::support::lock::BIN;
    let (_fake, _c, b) = crew();
    write(&b.dir.join("agent.env"), "GH_TOKEN=ghp_test_file\n", 0o600);
    let d = tempfile::tempdir().unwrap();
    let run = |cfg: &Path| {
        let o = Command::new(BIN)
            .arg("--config-dir")
            .arg(cfg)
            .args([
                "lock",
                "run",
                d.path().join("l").to_str().unwrap(),
                "--holder",
                "a@x",
                "--no-board",
                "--",
                "/usr/bin/env",
            ])
            // an empty process table: a CI runner on this host must not change the result
            .env("HOST_LOCK_PS_CMD", "true")
            .env("GH_TOKEN", "ghp_test_env")
            .env("GITHUB_TOKEN", "ghp_test_env2")
            .env_remove("CREWBOARD_HOST_LOCK")
            .env_remove("CREWBOARD_HOST_LOCK_PID")
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    };
    let out = run(&b.dir);
    assert!(
        !out.contains("GH_TOKEN=") && !out.contains("GITHUB_TOKEN="),
        "{out}"
    );
    // the twin: a plain dir keeps what the caller has
    let plain = tempfile::tempdir().unwrap();
    let out = run(plain.path());
    assert!(out.contains("GH_TOKEN=ghp_test_env"), "{out}");
}

#[test]
fn a_lock_without_a_token_works_with_a_broken_agent_env() {
    use crate::support::lock::BIN;
    let (_fake, _c, b) = crew();
    write(&b.dir.join("agent.env"), "GH_TOKEN=ghp_test_file\n", 0o644);
    let d = tempfile::tempdir().unwrap();
    let o = Command::new(BIN)
        .arg("--config-dir")
        .arg(&b.dir)
        .args([
            "lock",
            "status",
            d.path().join("l").to_str().unwrap(),
            "--no-board",
        ])
        .env("HOST_LOCK_PS_CMD", "true")
        .output()
        .unwrap();
    // a free lock: status says so (exit 1), the file is not read
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
