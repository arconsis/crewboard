//! `crewboard agent add --role headless-coordinator --from <dir>`: the config dir of the headless
//! coordinator, made from the dir of the interactive coordinator (the same agent).
//!
//! Three steps: the agent dir, the config (a copy of `<initiative>.toml` with `headless = true` as
//! its first line) and the key (`keys/<agent>`, mode 600 in a dir with mode 700). Every check that
//! can refuse runs before the first write, so a refusal leaves no half-made dir. It prints no key.

use super::add::{
    AddArgs, Fail, PREFIX, default_env_file, env_fix, fail_lines, is_linked, make_agent_dir,
    scheduler_for, strip_prefix,
};
use super::shim::sq;
use super::{AgentRuntime, absolute};
use crate::agent_env;
use crate::fsutil::{self, Mode};
use crate::protocol::names::AgentName;
use std::path::{Path, PathBuf};

const TOTAL: usize = 3;

fn bad(step: usize, name: &'static str, what: String, fix: String) -> u8 {
    let f = Fail {
        step,
        name,
        what,
        fix,
        code: 4,
    };
    for l in fail_lines(&f, TOTAL, "", None) {
        eprintln!("{l}");
    }
    f.code
}

fn say(step: usize, name: &str, already: bool, text: &str) {
    println!(
        "{PREFIX} [{step}/{TOTAL}] {name}: {} ({text})",
        if already { "already done" } else { "done" }
    );
}

/// The agent config of the source dir: the one `<initiative>.toml` that has a `name = ` line.
/// Returns the files that qualify.
fn agent_configs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|f| {
                    let n = f
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    n.ends_with(".toml")
                        && !n.ends_with(".board.toml")
                        && f.is_file()
                        && std::fs::read_to_string(f)
                            .is_ok_and(|t| t.lines().any(|l| l.starts_with("name = ")))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Does this config text say `headless = true` (white space around `=` allowed)?
pub(super) fn says_headless(text: &str) -> bool {
    text.lines().any(|l| {
        l.strip_prefix("headless")
            .and_then(|r| r.trim_start().strip_prefix('='))
            .is_some_and(|v| v.trim() == "true")
    })
}

/// The agent name from the `name = "..."` line.
pub(super) fn name_of(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let v = l.strip_prefix("name = \"")?.strip_suffix('"')?;
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// The text of the headless config: `headless = true` first, then the source.
pub(super) fn headless_text(source: &str) -> String {
    format!("headless = true\n{source}")
}

/// Write a file with mode 600, atomically. The dir must exist.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fsutil::write_atomic(path, bytes, Mode::Exact(0o600))
}

fn usage(m: &str) -> u8 {
    eprintln!("{PREFIX} {m}");
    4
}

/// What the checks of `run_headless` found: everything the three steps need. Every check that
/// can refuse is made before the first write.
struct Plan {
    dir: PathBuf,
    env: PathBuf,
    name: String,
    initiative: String,
    src_text: String,
    key_bytes: Vec<u8>,
    dst: PathBuf,
    /// The headless config exists already (and is one).
    dst_exists: bool,
}

/// The options that this role takes, and the source dir. Returns the absolute `--from`.
fn check_options(a: &AddArgs) -> Result<PathBuf, u8> {
    if a.target.is_some()
        || a.board_file.is_some()
        || a.repo.is_some()
        || a.name.is_some()
        || a.runtime.is_some()
        || a.work_dir.is_some()
        || a.crew_login.is_some()
        || a.model.is_some()
        || a.initiative.is_some()
    {
        return Err(usage(
            "--role headless-coordinator takes only --from, --agent-dir and --env-file",
        ));
    }
    let Some(from) = &a.from else {
        return Err(usage(
            "--role headless-coordinator needs --from <the agent dir of the interactive coordinator>",
        ));
    };
    let from = absolute(from);
    if !from.is_dir() {
        return Err(bad(
            1,
            "agent dir",
            format!("--from is not a directory: {}", from.display()),
            "pass the agent dir of the interactive coordinator".into(),
        ));
    }
    Ok(from)
}

/// The source: one agent config of the interactive dir, not headless itself, with a valid
/// name. Returns the config path, its text and the agent name.
fn read_source(from: &Path) -> Result<(PathBuf, String, String), u8> {
    let configs = agent_configs(from);
    let [src] = configs.as_slice() else {
        return Err(bad(
            2,
            "config",
            format!(
                "{} has {} agent configs, not one",
                from.display(),
                configs.len()
            ),
            format!(
                "run: crewboard agent add <repo> --role coordinator --agent-dir {} ... (join first)",
                sq(&from.display().to_string())
            ),
        ));
    };
    let src_text = match std::fs::read_to_string(src) {
        Ok(t) => t,
        Err(e) => {
            return Err(bad(
                2,
                "config",
                format!("cannot read {}: {e}", src.display()),
                "check the permissions of the interactive dir".into(),
            ));
        }
    };
    if says_headless(&src_text) {
        return Err(bad(
            2,
            "config",
            format!("{} is already headless", src.display()),
            "pass the dir of the INTERACTIVE coordinator as --from".into(),
        ));
    }
    let Some(name) = name_of(&src_text) else {
        return Err(bad(
            2,
            "config",
            format!("no name in {}", src.display()),
            "run join for the coordinator first".into(),
        ));
    };
    if let Err(e) = AgentName::parse(&name) {
        return Err(bad(
            2,
            "config",
            format!("the agent name in {} is not valid: {e}", src.display()),
            "run join for the coordinator again".into(),
        ));
    }
    Ok((src.clone(), src_text, name))
}

/// The env file: the flag, else the default of the initiative when it exists.
fn env_file_of(a: &AddArgs, initiative: &str) -> Result<PathBuf, u8> {
    match &a.env_file {
        Some(f) => Ok(absolute(f)),
        None => match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
            Some(h) => {
                let d = default_env_file(Path::new(&h), initiative);
                if !d.is_file() {
                    return Err(usage(&format!(
                        "--env-file is required: {} does not exist (the default env file is used only when it exists)",
                        d.display()
                    )));
                }
                Ok(d)
            }
            None => Err(usage(
                "--env-file is required: cannot find the home directory",
            )),
        },
    }
}

/// Every check that can refuse, before the first write.
fn plan(a: &AddArgs) -> Result<Plan, u8> {
    let from = check_options(a)?;
    let dir = match a.agent_dir.as_deref().or(a.config_dir.as_deref()) {
        Some(d) => absolute(d),
        None => PathBuf::from(format!("{}-headless", from.display())),
    };
    let (src, src_text, name) = read_source(&from)?;
    let key = from.join("keys").join(&name);
    let initiative = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let env = env_file_of(a, &initiative)?;
    if let Err(e) = agent_env::load(&dir, Some(&env)) {
        let m = strip_prefix(&e);
        let fix = env_fix(&m, &env);
        return Err(bad(1, "agent dir", m, fix));
    }
    if dir == from {
        return Err(bad(
            1,
            "agent dir",
            format!("the headless dir is the interactive dir: {}", dir.display()),
            format!(
                "pass another --agent-dir, for example {}-headless",
                from.display()
            ),
        ));
    }
    if !key.is_file() {
        return Err(bad(
            3,
            "key",
            format!("no key file {}", key.display()),
            "run join for the coordinator first (it writes the key)".into(),
        ));
    }
    let key_bytes = match std::fs::read(&key) {
        Ok(b) => b,
        Err(e) => {
            return Err(bad(
                3,
                "key",
                format!("cannot read {}: {e}", key.display()),
                "check the permissions of the interactive dir".into(),
            ));
        }
    };
    let dst = dir.join(format!("{initiative}.toml"));
    // a headless config that is there must be one
    let dst_text = std::fs::read_to_string(&dst).ok();
    if dst.exists() && dst_text.as_deref().and_then(|t| t.lines().next()) != Some("headless = true")
    {
        return Err(bad(
            2,
            "config",
            format!("{} exists and is not headless", dst.display()),
            format!("move {} away and run this command again", dst.display()),
        ));
    }
    Ok(Plan {
        dir,
        env,
        name,
        initiative,
        src_text,
        key_bytes,
        dst,
        dst_exists: dst_text.is_some(),
    })
}

/// Step 1: the agent dir and `agent.env`.
fn step_agent_dir(p: &Plan) -> Result<(), u8> {
    let (dir, env) = (&p.dir, &p.env);
    if dir.is_dir() && is_linked(dir, env) {
        if let Err(e) = fsutil::set_mode(dir, 0o700) {
            return Err(bad(
                1,
                "agent dir",
                format!("cannot set the mode of {}: {e}", dir.display()),
                format!("chmod 700 {}", dir.display()),
            ));
        }
        say(1, "agent dir", true, &dir.display().to_string());
    } else {
        if let Err(e) = make_agent_dir(dir, env) {
            return Err(bad(
                1,
                "agent dir",
                format!("cannot make {} or link its agent.env: {e}", dir.display()),
                format!(
                    "check the permissions of {}",
                    dir.parent().unwrap_or(dir).display()
                ),
            ));
        }
        say(
            1,
            "agent dir",
            false,
            &format!("{}/agent.env -> {}", dir.display(), env.display()),
        );
    }
    Ok(())
}

/// Step 2: the config, a copy of the interactive config with `headless = true` as its first line.
fn step_config(p: &Plan) -> Result<(), u8> {
    let (dst, initiative) = (&p.dst, &p.initiative);
    if p.dst_exists {
        if let Err(e) = fsutil::set_mode(dst, 0o600) {
            return Err(bad(
                2,
                "config",
                format!("cannot set the mode of {}: {e}", dst.display()),
                format!("chmod 600 {}", dst.display()),
            ));
        }
        say(2, "config", true, &format!("{initiative}.toml, headless"));
    } else {
        if let Err(e) = write_private(dst, headless_text(&p.src_text).as_bytes()) {
            return Err(bad(
                2,
                "config",
                format!("cannot write {}: {e}", dst.display()),
                format!("check the permissions of {}", p.dir.display()),
            ));
        }
        say(
            2,
            "config",
            false,
            &format!("{initiative}.toml copied, headless = true first"),
        );
    }
    Ok(())
}

/// Step 3: the key of the coordinator, mode 600 in a dir with mode 700.
fn step_key(p: &Plan) -> Result<(), u8> {
    let dkey_dir = p.dir.join("keys");
    let dkey = dkey_dir.join(&p.name);
    let same = std::fs::read(&dkey).is_ok_and(|b| b == p.key_bytes);
    let made = std::fs::create_dir_all(&dkey_dir)
        .and_then(|()| fsutil::set_mode(&dkey_dir, 0o700))
        .and_then(|()| {
            if same {
                fsutil::set_mode(&dkey, 0o600)
            } else {
                write_private(&dkey, &p.key_bytes)
            }
        });
    if let Err(e) = made {
        return Err(bad(
            3,
            "key",
            format!("cannot copy the key into {}: {e}", dkey_dir.display()),
            format!("check the permissions of {}", p.dir.display()),
        ));
    }
    say(
        3,
        "key",
        same,
        &if same {
            format!("keys/{}", p.name)
        } else {
            format!("keys/{} copied, mode 600", p.name)
        },
    );
    Ok(())
}

pub(super) fn run_headless(a: &AddArgs) -> u8 {
    let p = match plan(a) {
        Ok(p) => p,
        Err(code) => return code,
    };
    for step in [step_agent_dir, step_config, step_key] {
        if let Err(code) = step(&p) {
            return code;
        }
    }
    println!(
        "{PREFIX} note: the headless key is a guardrail, not a wall; for the headless guardrail see docs/guides/headless-setup.md in a clone of crewboard"
    );
    println!(
        "{PREFIX} next step: install the tick (the same command without `install` runs it once): crewboard wake install --scheduler {} --tick --agent-dir {} --initiative {} --env-file {}",
        scheduler_for(AgentRuntime::Claude),
        sq(&p.dir.display().to_string()),
        p.initiative,
        sq(&p.env.display().to_string()),
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_is_read_with_or_without_spaces_and_only_when_true() {
        assert!(says_headless("headless = true\nname = \"a@b\"\n"));
        assert!(says_headless("name = \"a@b\"\nheadless=true\n"));
        assert!(says_headless("headless   =   true  \n"));
        assert!(!says_headless("headless = false\n"));
        assert!(!says_headless("name = \"headless = true\"\n"));
        assert!(!says_headless("# headless = true\n"));
    }

    #[test]
    fn the_name_comes_from_its_own_line() {
        assert_eq!(
            name_of("runtime = \"x\"\nname = \"claude@host-a\"\n"),
            Some("claude@host-a".into())
        );
        assert_eq!(name_of("name = \"\"\n"), None);
        assert_eq!(name_of("runtime = \"x\"\n"), None);
    }

    #[test]
    fn the_headless_copy_has_the_key_first_and_keeps_the_rest() {
        let t = headless_text("name = \"a@b\"\n[board]\nx = 1\n");
        assert_eq!(t.lines().next(), Some("headless = true"));
        assert!(t.ends_with("name = \"a@b\"\n[board]\nx = 1\n"));
    }

    #[test]
    fn only_a_toml_with_a_name_line_is_an_agent_config() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("demo.toml"), "name = \"a@b\"\n").unwrap();
        std::fs::write(d.path().join("demo.board.toml"), "name = \"x\"\n").unwrap();
        std::fs::write(d.path().join("notes.toml"), "x = 1\n").unwrap();
        std::fs::write(d.path().join("plain.txt"), "name = \"y\"\n").unwrap();
        let v = agent_configs(d.path());
        assert_eq!(v, vec![d.path().join("demo.toml")]);
    }
}
