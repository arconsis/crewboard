//! The wake command of a dev agent for `crewboard wake --runtime R`: read the options, check them
//! and build the argument vector, the working dir and the extra environment of the wake child
//! (see `crate::wake::adapters`). Without `--runtime` the wake runs the hidden `--wake-cmd`.

use crate::fsutil::{find_on_path, is_executable};
use crate::wake::adapters::{self, Hook, Runtime, Spec};
use crate::wake::state::Paths;
use crate::wake::words;
use clap::Args;
use std::path::{Path, PathBuf};

/// The options that make the wake command of a dev agent. All of them need `--runtime`.
#[derive(Debug, Args, Clone, Default)]
pub struct AgentArgs {
    /// The runtime of the wake: claude, codex or hermes. The wake command, its prompt and its
    /// permission flags come from it (`--print-cmd` shows them).
    #[arg(long)]
    pub runtime: Option<String>,
    /// The checkout of the agent. The wake command starts there.
    #[arg(long)]
    pub work_dir: Option<PathBuf>,
    /// The dir of the `gh` shim (from `crewboard agent shim`): first on the PATH of the wake, and
    /// `GH_CONFIG_DIR` is the empty config dir next to it.
    #[arg(long, hide = true)]
    pub shim_dir: Option<PathBuf>,
    /// The model of the wake, given to the runtime as `--model ID`.
    #[arg(long)]
    pub model: Option<String>,
    /// claude: one more allowed tool, appended to the default list. Repeat it for more.
    #[arg(long, hide = true)]
    pub allow: Vec<String>,
    /// Replace the default permission flags of the runtime (one string, split like a shell).
    #[arg(long, allow_hyphen_values = true, hide = true)]
    pub runtime_flags: Option<String>,
    /// The runtime binary (default: the runtime name, looked up on the PATH of the wake).
    #[arg(long, hide = true)]
    pub runtime_bin: Option<PathBuf>,
    /// Text added to the end of the wake prompt.
    #[arg(long, hide = true)]
    pub prompt_extra: Option<String>,
    /// A host lock dir of this host: the prompt names it for heavy jobs, and the wake may run
    /// `crewboard lock run` for this dir only. Repeat it for more.
    #[arg(long, hide = true)]
    pub host_lock: Vec<PathBuf>,
    /// claude: no events hook.
    #[arg(long, hide = true)]
    pub no_events_hook: bool,
    /// claude: seconds between two polls of the events hook (default 300).
    #[arg(long, hide = true)]
    pub events_every: Option<String>,
    /// Print the wake command (or the tick line) and exit 0. Starts nothing.
    #[arg(long, hide = true)]
    pub print_cmd: bool,
}

/// The wake command of a dev agent, ready to start.
#[derive(Debug, Clone)]
pub struct Direct {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// Set in the environment of the wake child, after the environment of `wake`.
    pub env: Vec<(String, String)>,
    /// `cd <work dir> && <words>`: the text of `--print-cmd` and of the log line.
    pub text: String,
    /// The `PATH` that finds the runtime: the one of the wake child.
    pub path: Option<String>,
}

/// What the options are made from.
pub struct Inputs<'a> {
    pub a: &'a AgentArgs,
    pub tick: bool,
    pub has_wake_cmd: bool,
    pub agent: Option<&'a str>,
    pub initiative: Option<&'a str>,
    pub config_dir: Option<&'a Path>,
    pub paths: &'a Paths,
    pub events_file: &'a Path,
    /// `--crewboard BIN` of the wake (test seam).
    pub crewboard: Option<&'a str>,
    /// This binary, for the command of the events hook.
    pub exe: Option<&'a Path>,
    /// `$PATH` of this process.
    pub path_env: Option<&'a str>,
    /// `$HOME`: where the installed skill is looked up for the read access of a Claude wake.
    pub home: Option<&'a Path>,
}

/// The absolute path of `p`, as given otherwise.
pub fn abs(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

/// The path that `cd DIR && pwd` prints: absolute, without `//` and `.`.
fn logical(p: &Path) -> PathBuf {
    abs(p).components().collect()
}

/// Is any option of the dev agent set?
fn any_set(a: &AgentArgs) -> Option<&'static str> {
    [
        (a.shim_dir.is_some(), "--shim-dir"),
        (a.model.is_some(), "--model"),
        (!a.allow.is_empty(), "--allow"),
        (a.runtime_flags.is_some(), "--runtime-flags"),
        (a.runtime_bin.is_some(), "--runtime-bin"),
        (a.prompt_extra.is_some(), "--prompt-extra"),
        (a.no_events_hook, "--no-events-hook"),
        (a.events_every.is_some(), "--events-every"),
    ]
    .iter()
    .find(|(set, _)| *set)
    .map(|(_, n)| *n)
}

/// The PATH of the wake and the environment that the wake adds.
type WakeEnv = (Option<String>, Vec<(String, String)>);

/// The PATH of the wake (the install dirs of crewboard at the end, the `gh` shim first) and the
/// environment that the wake adds: the `gh` shim config, the PATH, and the grant for `lock run`.
fn wake_env(
    a: &AgentArgs,
    path_env: Option<&str>,
    host_locks: &[PathBuf],
) -> Result<WakeEnv, String> {
    // the PATH of the wake: the install dirs of crewboard at the end, the gh shim first
    let mut path = path_env.map(str::to_string);
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(sd) = &a.shim_dir {
        if !is_executable(&sd.join("gh")) {
            return Err(format!(
                "no gh shim in {} (run: crewboard agent shim --agent-dir <agent dir>)",
                sd.display()
            ));
        }
        let sd = logical(sd);
        let empty = sd
            .parent()
            .unwrap_or(Path::new("/"))
            .join("gh-empty-config");
        if !empty.is_dir() {
            return Err(format!("no gh-empty-config next to {}", sd.display()));
        }
        path = Some(match path {
            Some(p) if !p.is_empty() => format!("{}:{p}", sd.display()),
            _ => sd.display().to_string(),
        });
        env.push(("GH_CONFIG_DIR".into(), empty.to_string_lossy().into_owned()));
    }
    if let Some(p) = &path {
        env.push(("PATH".into(), p.clone()));
    }
    // the model runs inside a wake: `crewboard lock run` starts a command, so under a wake it is
    // allowed only for the lock dirs named here; a command line of the model cannot set these
    let dirs = host_locks
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    env.push(("CREWBOARD_WAKE".into(), "1".into()));
    env.push(("CREWBOARD_LOCK_RUN_DIRS".into(), dirs));
    Ok((path, env))
}

/// The program of the runtime: its name, or the binary that `--runtime-bin` gives.
fn runtime_bin(a: &AgentArgs, runtime: Runtime, path: Option<&str>) -> Result<String, String> {
    // a given binary is a path, or a name that is looked up on the PATH of the wake; the wake
    // command starts in another dir, so a relative path is made absolute
    let bin = match a.runtime_bin.as_ref() {
        None => runtime.name().to_string(),
        Some(b) if b.as_os_str().to_string_lossy().contains('/') => {
            abs(b).to_string_lossy().into_owned()
        }
        Some(b) => program(&b.to_string_lossy(), path)
            .to_string_lossy()
            .into_owned(),
    };
    if a.runtime_bin.is_some() && !is_executable(Path::new(&bin)) {
        return Err(format!(
            "the runtime {} is gone (moved or removed); run crewboard wake install again",
            a.runtime_bin
                .as_ref()
                .map_or(String::new(), |b| b.display().to_string())
        ));
    }
    Ok(bin)
}

/// The settings of the events hook of a Claude wake; `None` for another runtime or with
/// `--no-events-hook`.
fn events_hook_settings(
    i: &Inputs,
    runtime: Runtime,
    initiative: &str,
    every: u32,
    config_dir: Option<&Path>,
) -> Result<Option<String>, String> {
    let a = i.a;
    Ok(if runtime == Runtime::Claude && !a.no_events_hook {
        let exe = i.exe.ok_or_else(|| {
            "cannot find the path of this program for the events hook".to_string()
        })?;
        let seam = i
            .crewboard
            .filter(|b| !crate::ops::lock_cmd::is_this_binary(b));
        Some(adapters::hook_settings(&Hook {
            exe,
            initiative,
            every,
            log: &i.paths.log,
            config_dir,
            crewboard: seam,
        }))
    } else {
        None
    })
}

/// The wake command of the dev agent; `None` when `--runtime` is not given. An error is the text
/// of a wrong option (exit 4).
pub fn build(i: &Inputs) -> Result<Option<Direct>, String> {
    let a = i.a;
    let Some(rt_name) = a.runtime.as_deref().filter(|r| !r.is_empty()) else {
        if let Some(flag) = any_set(a) {
            return Err(format!(
                "{flag} needs --runtime (it shapes the wake command of a dev agent)"
            ));
        }
        return Ok(None);
    };
    if i.tick {
        return Err("--runtime and --tick exclude each other: a tick starts no model".to_string());
    }
    if i.has_wake_cmd {
        return Err("--runtime and --wake-cmd exclude each other".to_string());
    }
    let Some(runtime) = Runtime::parse(rt_name) else {
        return Err("--runtime must be claude, codex or hermes".to_string());
    };
    let agent = i
        .agent
        .ok_or_else(|| "--agent is required for a dev agent".to_string())?;
    let initiative = i
        .initiative
        .ok_or_else(|| "--initiative is required".to_string())?;
    let work_dir = a
        .work_dir
        .as_deref()
        .ok_or_else(|| "--work-dir is required for a dev agent".to_string())?;
    if !work_dir.is_dir() {
        return Err(format!(
            "--work-dir must be an existing directory: {}",
            work_dir.display()
        ));
    }
    let every: u32 = match a.events_every.as_deref() {
        None => 300,
        Some(s) => s
            .parse()
            .map_err(|_| format!("--events-every needs whole seconds, got '{s}'"))?,
    };
    let flags = match a.runtime_flags.as_deref() {
        None => None,
        Some(s) => Some(words::split(s).map_err(|e| format!("--runtime-flags: {e}"))?),
    };
    let host_locks: Vec<PathBuf> = a.host_lock.iter().map(|p| abs(p)).collect();
    if let Some(bad) = host_locks
        .iter()
        .find(|p| p.to_string_lossy().contains(':'))
    {
        // CREWBOARD_LOCK_RUN_DIRS is a `:` list: a colon in a dir would grant a second dir
        return Err(format!(
            "--host-lock {} has a colon; the grant for `lock run` is a list that a colon splits",
            bad.display()
        ));
    }
    // the dir that holds each lock: where the heavy jobs write their logs. `/` has none.
    let parents: Vec<PathBuf> = host_locks
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| !p.as_os_str().is_empty())
        .collect();

    let (path, env) = wake_env(a, i.path_env, &host_locks)?;
    let bin = runtime_bin(a, runtime, path.as_deref())?;
    let config_dir = i.config_dir.map(abs);
    let prompt = adapters::prompt(
        agent,
        initiative,
        i.events_file,
        &i.paths.dir,
        &host_locks,
        config_dir.as_deref(),
        a.prompt_extra.as_deref(),
    );
    let settings = events_hook_settings(i, runtime, initiative, every, config_dir.as_deref())?;
    let argv = adapters::argv(&Spec {
        runtime,
        bin: &bin,
        model: a.model.as_deref().filter(|m| !m.is_empty()),
        flags,
        allow: &a.allow,
        state_dir: &i.paths.dir,
        config_dir: config_dir.as_deref(),
        host_lock_parents: &parents,
        skill_dirs: &adapters::skill_read_dirs(i.home),
        settings_json: settings.as_deref(),
        prompt: &prompt,
    })?;
    Ok(Some(Direct {
        text: adapters::shell_line(work_dir, &argv),
        argv,
        cwd: work_dir.to_path_buf(),
        env,
        path,
    }))
}

/// `--print-cmd` for a headless coordinator.
pub fn tick_text(log: &Path) -> String {
    format!(
        "crewboard tick (no model; the lines go to {})",
        log.display()
    )
}

/// The program to start: a name is looked up in `path` (the PATH of the wake child), so that the
/// `gh` shim dir and the install dirs count; a path stays as it is. A name that is not found
/// stays a name, and the start fails with the reason.
pub fn program(argv0: &str, path: Option<&str>) -> PathBuf {
    if argv0.contains('/') {
        return PathBuf::from(argv0);
    }
    path.and_then(|p| find_on_path(argv0, p))
        .unwrap_or_else(|| PathBuf::from(argv0))
}

/// `$PATH` with the install dirs of the `crewboard` binary (`~/.cargo/bin` only matters for a
/// binary built with `cargo install`; `~/.local/bin` is the dir of the one-line install) at the
/// end when they exist and are not on it: a scheduler starts a program with a minimal PATH. The
/// dirs are for finding crewboard, not for the tools of the project it serves.
pub fn with_install_dirs(path: &str, home: Option<&str>, exists: &dyn Fn(&Path) -> bool) -> String {
    let mut out = path.to_string();
    let Some(home) = home.filter(|h| !h.is_empty()) else {
        return out;
    };
    for d in [".cargo/bin", ".local/bin"] {
        let dir = format!("{home}/{d}");
        let on = out.split(':').any(|p| p == dir);
        if !on && exists(Path::new(&dir)) {
            if !out.is_empty() {
                out.push(':');
            }
            out.push_str(&dir);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rig {
        dir: tempfile::TempDir,
        paths: Paths,
    }

    impl Rig {
        fn new() -> Rig {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir(dir.path().join("wd")).unwrap();
            let state = dir.path().join("state");
            let paths = Paths::new(state, None, None);
            Rig { dir, paths }
        }
        fn p(&self, n: &str) -> PathBuf {
            self.dir.path().join(n)
        }
        fn args(&self, rt: &str) -> AgentArgs {
            AgentArgs {
                runtime: Some(rt.into()),
                work_dir: Some(self.p("wd")),
                ..AgentArgs::default()
            }
        }
        fn build(&self, a: &AgentArgs) -> Result<Option<Direct>, String> {
            self.build_with(a, false, true)
        }
        fn build_with(
            &self,
            a: &AgentArgs,
            tick: bool,
            agent: bool,
        ) -> Result<Option<Direct>, String> {
            let ev = self.paths.dir.join("events.txt");
            build(&Inputs {
                a,
                tick,
                has_wake_cmd: false,
                agent: agent.then_some("dev@host-a"),
                initiative: Some("demo"),
                config_dir: None,
                paths: &self.paths,
                events_file: &ev,
                crewboard: None,
                exe: Some(Path::new("/bin/crewboard")),
                path_env: Some("/usr/bin:/bin"),
                home: None,
            })
        }
    }

    #[test]
    fn no_runtime_no_job_and_an_option_of_the_agent_without_it_is_an_error() {
        let rig = Rig::new();
        assert!(rig.build(&AgentArgs::default()).unwrap().is_none());
        for a in [
            AgentArgs {
                model: Some("m".into()),
                ..AgentArgs::default()
            },
            AgentArgs {
                allow: vec!["x".into()],
                ..AgentArgs::default()
            },
            AgentArgs {
                no_events_hook: true,
                ..AgentArgs::default()
            },
            AgentArgs {
                events_every: Some("5".into()),
                ..AgentArgs::default()
            },
        ] {
            let e = rig.build(&a).unwrap_err();
            assert!(e.contains("needs --runtime"), "{e}");
        }
    }

    #[test]
    fn a6_refusals() {
        let rig = Rig::new();
        let e = |a: &AgentArgs| rig.build(a).unwrap_err();
        assert!(e(&rig.args("copilot")).contains("--runtime must be claude, codex or hermes"));
        let mut a = rig.args("claude");
        a.work_dir = Some(rig.p("nope"));
        assert!(e(&a).contains("--work-dir must be an existing directory"));
        a.work_dir = None;
        assert!(e(&a).contains("--work-dir is required"));
        let mut a = rig.args("claude");
        a.shim_dir = Some(rig.p("real"));
        std::fs::create_dir_all(rig.p("real")).unwrap();
        assert!(e(&a).contains("no gh shim in"));
        let mut a = rig.args("claude");
        a.events_every = Some("soon".into());
        assert!(e(&a).contains("--events-every needs whole seconds, got 'soon'"));
        let mut a = rig.args("claude");
        a.runtime_flags = Some("'open".into());
        assert!(e(&a).contains("--runtime-flags"));
        assert!(
            rig.build_with(&rig.args("claude"), false, false)
                .unwrap_err()
                .contains("--agent is required")
        );
        assert!(
            rig.build_with(&rig.args("claude"), true, true)
                .unwrap_err()
                .contains("--tick")
        );
        let mut a = rig.args("claude");
        a.runtime_bin = Some(rig.p("gone/claude"));
        assert!(e(&a).contains("is gone (moved or removed)"));
    }

    #[test]
    fn an_empty_model_is_no_model_the_config_dir_reaches_the_hook_and_the_seam_is_filtered() {
        let rig = Rig::new();
        let mut a = rig.args("claude");
        a.model = Some(String::new());
        let d = rig.build(&a).unwrap().unwrap();
        assert!(!d.argv.contains(&"--model".to_string()));
        let ev = rig.paths.dir.join("events.txt");
        let exe = std::env::current_exe().unwrap();
        let cfg = rig.p("cfg dir");
        let make = |crewboard: Option<&str>| {
            build(&Inputs {
                a: &rig.args("claude"),
                tick: false,
                has_wake_cmd: false,
                agent: Some("dev@host-a"),
                initiative: Some("demo"),
                config_dir: Some(&cfg),
                paths: &rig.paths,
                events_file: &ev,
                crewboard,
                exe: Some(&exe),
                path_env: Some("/usr/bin:/bin"),
                home: None,
            })
            .unwrap()
            .unwrap()
        };
        let settings = |d: &Direct| {
            let j = d.argv.windows(2).find(|w| w[0] == "--settings").unwrap()[1].clone();
            let v: serde_json::Value = serde_json::from_str(&j).unwrap();
            v["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let own = settings(&make(Some(exe.to_str().unwrap())));
        assert!(
            !own.contains("--crewboard"),
            "this binary is no seam: {own}"
        );
        assert!(
            own.contains("--config-dir 'cfg dir'")
                || own.contains(&format!("--config-dir '{}'", cfg.display())),
            "{own}"
        );
        let other = settings(&make(Some("/some/other/crewboard")));
        assert!(
            other.contains("--crewboard /some/other/crewboard"),
            "{other}"
        );
    }

    #[test]
    fn the_install_dirs_go_to_the_end_when_they_exist_and_are_not_there() {
        let both = |_: &Path| true;
        assert_eq!(
            with_install_dirs("/usr/bin", Some("/h"), &both),
            "/usr/bin:/h/.cargo/bin:/h/.local/bin"
        );
        // on the PATH already: no copy, the order stays
        assert_eq!(
            with_install_dirs("/h/.local/bin:/usr/bin", Some("/h"), &both),
            "/h/.local/bin:/usr/bin:/h/.cargo/bin"
        );
        let only_local = |p: &Path| p.ends_with(".local/bin");
        assert_eq!(
            with_install_dirs("/usr/bin", Some("/h"), &only_local),
            "/usr/bin:/h/.local/bin"
        );
        // neither dir exists, or no home: nothing is added
        assert_eq!(
            with_install_dirs("/usr/bin", Some("/h"), &|_| false),
            "/usr/bin"
        );
        assert_eq!(with_install_dirs("/usr/bin", None, &both), "/usr/bin");
        assert_eq!(
            with_install_dirs("", Some("/h"), &both),
            "/h/.cargo/bin:/h/.local/bin"
        );
    }

    /// Tests of Unix path, PATH-separator and permission semantics: the wake runs on Unix only.
    #[cfg(unix)]
    mod unix_tests {
        use super::*;

        impl Rig {
            fn shim(&self) -> PathBuf {
                let s = self.p("agent/bin");
                std::fs::create_dir_all(&s).unwrap();
                std::fs::create_dir_all(self.p("agent/gh-empty-config")).unwrap();
                let gh = s.join("gh");
                std::fs::write(&gh, "#!/bin/sh\n").unwrap();
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
                }
                s
            }
        }

        fn env_of<'a>(d: &'a Direct, k: &str) -> Option<&'a str> {
            d.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
        }

        #[test]
        fn a4_the_shim_dir_goes_first_on_the_path_and_names_the_empty_gh_config() {
            let rig = Rig::new();
            let mut a = rig.args("claude");
            let d = rig.build(&a).unwrap().unwrap();
            assert_eq!(env_of(&d, "PATH"), Some("/usr/bin:/bin"));
            assert_eq!(
                env_of(&d, "GH_CONFIG_DIR"),
                None,
                "A4b: no shim dir, no GH_CONFIG_DIR"
            );
            let shim = rig.shim();
            a.shim_dir = Some(shim.clone());
            let d = rig.build(&a).unwrap().unwrap();
            assert_eq!(
                env_of(&d, "PATH"),
                Some(format!("{}:/usr/bin:/bin", shim.display()).as_str())
            );
            assert_eq!(
                env_of(&d, "GH_CONFIG_DIR"),
                Some(rig.p("agent/gh-empty-config").to_str().unwrap())
            );
            // the empty config dir must be there
            std::fs::remove_dir(rig.p("agent/gh-empty-config")).unwrap();
            assert!(
                rig.build(&a)
                    .unwrap_err()
                    .contains("no gh-empty-config next to")
            );
        }

        #[test]
        fn host_lock_paths_are_made_absolute_a_colon_is_refused_and_root_has_no_parent() {
            let rig = Rig::new();
            let mut a = rig.args("claude");
            a.host_lock = vec![
                "rel/heavy.lock.d".into(),
                "/".into(),
                "/x/./y.lock//".into(),
            ];
            let d = rig.build(&a).unwrap().unwrap();
            let cwd = std::env::current_dir().unwrap();
            let dirs = env_of(&d, "CREWBOARD_LOCK_RUN_DIRS").unwrap();
            assert!(
                dirs.starts_with(&format!("{}:/:", cwd.join("rel/heavy.lock.d").display())),
                "{dirs}"
            );
            let adds: Vec<&String> = d
                .argv
                .windows(2)
                .filter(|w| w[0] == "--add-dir")
                .map(|w| &w[1])
                .collect();
            // the state dir, the parent of the relative lock, the parent of the third; `/` has none
            assert_eq!(adds.len(), 3, "{adds:?}");
            assert!(adds.iter().all(|p| !p.is_empty()));
            assert_eq!(adds[1].as_str(), cwd.join("rel").to_str().unwrap());
            assert_eq!(adds[2].as_str(), "/x");
            a.host_lock = vec!["/a:b/heavy".into()];
            assert!(rig.build(&a).unwrap_err().contains("has a colon"));
        }

        #[test]
        fn the_shim_dir_is_kept_logical_so_a_symlinked_agent_dir_still_finds_its_config() {
            let rig = Rig::new();
            let shim = rig.shim();
            let mut a = rig.args("claude");
            // `bin/./` names the same dir; the empty config dir is found next to `agent/bin`
            a.shim_dir = Some(shim.join("./"));
            let d = rig.build(&a).unwrap().unwrap();
            assert_eq!(
                env_of(&d, "GH_CONFIG_DIR"),
                Some(rig.p("agent/gh-empty-config").to_str().unwrap())
            );
            assert!(
                env_of(&d, "PATH")
                    .unwrap()
                    .starts_with(&format!("{}:", shim.display())),
                "{:?}",
                env_of(&d, "PATH")
            );
        }

        #[test]
        fn the_runtime_binary_is_a_path_or_a_name_on_the_path() {
            let rig = Rig::new();
            let mut a = rig.args("claude");
            // a name is looked up on the PATH of the wake
            a.runtime_bin = Some("sh".into());
            let d = rig.build(&a).unwrap().unwrap();
            assert!(
                d.argv[0].ends_with("/sh") && Path::new(&d.argv[0]).is_absolute(),
                "{:?}",
                d.argv[0]
            );
            a.runtime_bin = Some("no-such-runtime-x".into());
            assert!(
                rig.build(&a)
                    .unwrap_err()
                    .contains("is gone (moved or removed)")
            );
            // a relative path is made absolute
            std::fs::write(rig.p("rt"), "#!/bin/sh\n").unwrap();
            a.runtime_bin = Some(rig.p("rt"));
            assert!(rig.build(&a).is_err(), "not executable");
            a.runtime_bin = Some(PathBuf::from("rel/dir/rt"));
            assert!(rig.build(&a).unwrap_err().contains("is gone"));
        }

        #[test]
        fn a8c_a8d_the_wake_marks_itself_and_grants_lock_run_for_the_host_locks_only() {
            let rig = Rig::new();
            let mut a = rig.args("claude");
            let d = rig.build(&a).unwrap().unwrap();
            assert_eq!(env_of(&d, "CREWBOARD_WAKE"), Some("1"));
            assert_eq!(env_of(&d, "CREWBOARD_LOCK_RUN_DIRS"), Some(""));
            a.host_lock = vec!["/l/a.lock.d".into(), "/m/b.lock.d".into()];
            let d = rig.build(&a).unwrap().unwrap();
            assert_eq!(
                env_of(&d, "CREWBOARD_LOCK_RUN_DIRS"),
                Some("/l/a.lock.d:/m/b.lock.d")
            );
            assert!(
                d.argv
                    .last()
                    .unwrap()
                    .contains("crewboard lock run /l/a.lock.d --holder dev@host-a")
            );
            let adds: Vec<&String> = d
                .argv
                .windows(2)
                .filter(|w| w[0] == "--add-dir")
                .map(|w| &w[1])
                .collect();
            assert_eq!(
                adds.len(),
                3,
                "state dir and the two lock parents: {adds:?}"
            );
        }

        #[test]
        fn the_job_has_the_cwd_the_text_and_the_hook_only_for_claude() {
            let rig = Rig::new();
            let d = rig.build(&rig.args("claude")).unwrap().unwrap();
            assert_eq!(d.cwd, rig.p("wd"));
            assert!(
                d.text
                    .starts_with(&format!("cd {} && claude -p ", rig.p("wd").display())),
                "{}",
                d.text
            );
            assert!(d.argv.contains(&"--settings".to_string()));
            let mut a = rig.args("claude");
            a.no_events_hook = true;
            assert!(
                !rig.build(&a)
                    .unwrap()
                    .unwrap()
                    .argv
                    .contains(&"--settings".to_string())
            );
            for rt in ["codex", "hermes"] {
                let d = rig.build(&rig.args(rt)).unwrap().unwrap();
                assert!(!d.argv.contains(&"--settings".to_string()), "{rt}");
            }
            // the prompt names the events file in the state dir
            assert!(
                d.argv
                    .last()
                    .unwrap()
                    .contains(&format!("{}/events.txt", rig.paths.dir.display()))
            );
        }

        #[test]
        fn program_is_looked_up_in_the_path_of_the_child() {
            let rig = Rig::new();
            let shim = rig.shim();
            let path = format!("{}:/usr/bin", shim.display());
            assert_eq!(program("gh", Some(&path)), shim.join("gh"));
            assert_eq!(program("/abs/x", Some(&path)), PathBuf::from("/abs/x"));
            assert_eq!(
                program("no-such-program-x", Some(&path)),
                PathBuf::from("no-such-program-x")
            );
            assert_eq!(program("gh", None), PathBuf::from("gh"));
        }
    }
}
