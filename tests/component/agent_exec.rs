//! `crewboard agent exec`, `agent env` and `agent with-env`.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn cb(args: &[&str], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_crewboard"))
        .args(args)
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .env_remove("CREWBOARD_CONFIG_DIR")
        .env("PATH", "/usr/bin:/bin")
        .envs(env.iter().copied())
        .output()
        .expect("run crewboard")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

/// An agent dir with the gh shim dir and the empty gh config dir.
fn agent_dir() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("dev1");
    std::fs::create_dir_all(d.join("bin")).unwrap();
    std::fs::create_dir_all(d.join("gh-empty-config")).unwrap();
    (t, d)
}

const SHOW: &str = r#"printf "%s|%s|%s|%s|%s|%s" "${PATH%%:*}" "${GH_CONFIG_DIR:-unset}" "${CREWBOARD_CONFIG_DIR:-unset}" "${GH_TOKEN:-unset}" "${GITHUB_TOKEN:-unset}" "${GH_ENTERPRISE_TOKEN:-unset}""#;

#[test]
fn exec_sets_the_agent_exports_and_removes_the_caller_tokens() {
    let (_t, d) = agent_dir();
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            d.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            SHOW,
        ],
        &[
            ("GH_TOKEN", "ghp_test_owner"),
            ("GITHUB_TOKEN", "ghp_test_owner2"),
            ("GH_ENTERPRISE_TOKEN", "ghp_test_owner3"),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let dir = d.display();
    assert_eq!(
        text(&o.stdout),
        format!("{dir}/bin|{dir}/gh-empty-config|{dir}|unset|unset|unset")
    );
}

#[test]
fn exec_uses_the_config_dir_flag_when_there_is_no_agent_dir_flag() {
    let (_t, d) = agent_dir();
    let o = cb(
        &[
            "--config-dir",
            d.to_str().unwrap(),
            "agent",
            "exec",
            "--",
            "/bin/sh",
            "-c",
            SHOW,
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(text(&o.stdout).contains(&format!("|{}|", d.display())));
    // and the variable, when there is no flag at all
    let o = cb(
        &["agent", "exec", "--", "/bin/sh", "-c", SHOW],
        &[("CREWBOARD_CONFIG_DIR", d.to_str().unwrap())],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert!(text(&o.stdout).starts_with(&format!("{}/bin|", d.display())));
}

#[test]
fn exec_without_the_shim_dirs_leaves_the_path_and_the_gh_config_alone() {
    let t = tempfile::tempdir().unwrap();
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            t.path().to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            SHOW,
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(
        text(&o.stdout),
        format!("/usr/bin|unset|{}|unset|unset|unset", t.path().display())
    );
}

#[test]
fn exec_keeps_the_exit_code_of_the_command() {
    let (_t, d) = agent_dir();
    for code in [0, 7, 75] {
        let o = cb(
            &[
                "agent",
                "exec",
                "--agent-dir",
                d.to_str().unwrap(),
                "--",
                "/bin/sh",
                "-c",
                &format!("exit {code}"),
            ],
            &[],
        );
        assert_eq!(o.status.code(), Some(code));
    }
}

#[test]
fn exec_finds_the_command_on_the_shim_dir_first() {
    let (_t, d) = agent_dir();
    let tool = d.join("bin").join("mytool");
    std::fs::write(&tool, "#!/bin/sh\necho from-the-shim-dir\n").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            d.to_str().unwrap(),
            "--",
            "mytool",
        ],
        &[],
    );
    assert_eq!(
        text(&o.stdout).trim(),
        "from-the-shim-dir",
        "{}",
        text(&o.stderr)
    );
}

#[test]
fn exec_with_a_missing_dir_exits_4_and_runs_nothing() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let nope = t.path().join("nope");
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            nope.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            &format!(": >'{}'", marker.display()),
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(4), "{}", text(&o.stderr));
    assert!(
        text(&o.stderr).contains(&format!("agent dir not found: {}", nope.display())),
        "{}",
        text(&o.stderr)
    );
    assert!(!marker.exists());
    // a missing command is a usage error: exit 64
    let (_t2, d) = agent_dir();
    let o = cb(
        &["agent", "exec", "--agent-dir", d.to_str().unwrap(), "--"],
        &[],
    );
    assert_eq!(o.status.code(), Some(64), "{}", text(&o.stderr));
}

#[test]
fn exec_says_127_for_a_command_that_does_not_exist() {
    let (_t, d) = agent_dir();
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            d.to_str().unwrap(),
            "--",
            "no-such-command-xyz",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(127));
    assert!(text(&o.stderr).contains("no-such-command-xyz: command not found"));
}

#[test]
fn exec_does_not_need_a_loadable_env_file() {
    // a broken agent.env must not stop the shell of the agent: `doctor` reports it
    let (_t, d) = agent_dir();
    let f = d.join("agent.env");
    std::fs::write(&f, "GH_TOKEN=ghp_test_x\n").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    let o = cb(
        &[
            "agent",
            "exec",
            "--agent-dir",
            d.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            "echo up",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    assert_eq!(text(&o.stdout).trim(), "up");
}

fn env_lines_of(args: &[&str]) -> Output {
    cb(args, &[("GH_TOKEN", "ghp_test_owner")])
}

#[test]
fn env_prints_the_three_export_lines_and_no_token() {
    let (_t, d) = agent_dir();
    let o = env_lines_of(&["agent", "env", "--agent-dir", d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    let dir = d.display();
    assert_eq!(
        text(&o.stdout),
        format!(
            "export PATH='{dir}/bin':\"$PATH\"\nexport GH_CONFIG_DIR='{dir}/gh-empty-config'\nexport CREWBOARD_CONFIG_DIR='{dir}'\n"
        )
    );
    assert!(!text(&o.stdout).contains("ghp_"));
    // eval of the lines sets the three values
    let script = format!(
        "PATH=/usr/bin:/bin; eval \"$(cat <<'EOF'\n{}EOF\n)\"; printf '%s|%s|%s' \"${{PATH%%:*}}\" \"$GH_CONFIG_DIR\" \"$CREWBOARD_CONFIG_DIR\"",
        text(&o.stdout)
    );
    let sh = Command::new("/bin/sh")
        .args(["-c", &script])
        .output()
        .unwrap();
    assert_eq!(
        text(&sh.stdout),
        format!("{dir}/bin|{dir}/gh-empty-config|{dir}")
    );
}

#[test]
fn env_of_a_bare_dir_prints_the_config_dir_only_and_a_missing_dir_exits_4() {
    let t = tempfile::tempdir().unwrap();
    let o = env_lines_of(&["agent", "env", "--agent-dir", t.path().to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        text(&o.stdout),
        format!("export CREWBOARD_CONFIG_DIR='{}'\n", t.path().display())
    );
    let nope = t.path().join("nope");
    let o = env_lines_of(&["agent", "env", "--agent-dir", nope.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(4), "{}", text(&o.stderr));
    assert!(text(&o.stdout).is_empty());
    assert!(text(&o.stderr).contains("agent dir not found"));
}

fn env_file(dir: &Path, name: &str, body: &str, mode: u32) -> PathBuf {
    let f = dir.join(name);
    std::fs::write(&f, body).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(mode)).unwrap();
    f
}

#[test]
fn with_env_gives_the_command_the_token_and_prints_nothing() {
    let t = tempfile::tempdir().unwrap();
    let f = env_file(
        t.path(),
        "ok.env",
        "GH_TOKEN=ghp_test_file\nGH_HOST=h.example\nFOO=bar\n",
        0o600,
    );
    let o = cb(
        &[
            "agent",
            "with-env",
            "--env-file",
            f.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            r#"printf "%s|%s|%s|%s" "$GH_TOKEN" "${GH_HOST:-unset}" "${FOO:-unset}" "${GITHUB_TOKEN:-unset}""#,
        ],
        &[
            ("GH_TOKEN", "ghp_test_owner"),
            ("GITHUB_TOKEN", "ghp_test_owner2"),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", text(&o.stderr));
    // the file token wins, GH_HOST of the file loads, FOO does not, and the caller's other
    // token variable is gone
    assert_eq!(text(&o.stdout), "ghp_test_file|h.example|unset|unset");
    // with-env is silent: no warning for FOO on every gh call of an agent
    assert!(text(&o.stderr).is_empty(), "{}", text(&o.stderr));
}

#[test]
fn with_env_refuses_what_the_loader_refuses_and_runs_nothing() {
    let t = tempfile::tempdir().unwrap();
    let marker = t.path().join("ran");
    let run = format!(": >'{}'", marker.display());
    let open = env_file(t.path(), "open.env", "GH_TOKEN=ghp_test_file\n", 0o644);
    let none = env_file(t.path(), "none.env", "OTHER=1\n", 0o600);
    let broken = env_file(
        t.path(),
        "broken.env",
        "GH_TOKEN=\"ghp_test_broken\n",
        0o600,
    );
    for (f, what) in [
        (&open, "mode 644"),
        (&none, "no GH_TOKEN"),
        (&broken, "line 1"),
    ] {
        let o = cb(
            &[
                "agent",
                "with-env",
                "--env-file",
                f.to_str().unwrap(),
                "--",
                "/bin/sh",
                "-c",
                &run,
            ],
            &[],
        );
        assert_eq!(o.status.code(), Some(4), "{what}: {}", text(&o.stderr));
        assert!(
            text(&o.stderr).contains(what),
            "{what}: {}",
            text(&o.stderr)
        );
        assert!(
            !text(&o.stderr).contains("ghp_test_"),
            "{}",
            text(&o.stderr)
        );
        assert!(!marker.exists(), "{what}: the command ran");
    }
    // a missing file, and no --env-file at all
    let o = cb(
        &[
            "agent",
            "with-env",
            "--env-file",
            t.path().join("x.env").to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            &run,
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(4));
    assert!(text(&o.stderr).contains("not found"));
    let o = cb(&["agent", "with-env", "--", "/bin/sh", "-c", &run], &[]);
    assert_eq!(o.status.code(), Some(64));
    assert!(text(&o.stderr).contains("--env-file is required"));
    assert!(!marker.exists());
}

#[test]
fn with_env_keeps_the_exit_code() {
    let t = tempfile::tempdir().unwrap();
    let f = env_file(t.path(), "ok.env", "GH_TOKEN=ghp_test_file\n", 0o600);
    let o = cb(
        &[
            "agent",
            "with-env",
            "--env-file",
            f.to_str().unwrap(),
            "--",
            "/bin/sh",
            "-c",
            "exit 9",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(9));
    // the command may also follow without `--`
    let o = cb(
        &[
            "agent",
            "with-env",
            "--env-file",
            f.to_str().unwrap(),
            "/bin/sh",
            "-c",
            "exit 3",
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(3));
}
