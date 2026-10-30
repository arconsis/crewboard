//! `crewboard lock`: the host lock of a shared host (docs/design/spec/host-lock.md). The logic is in
//! `crate::lock`; this file reads the environment, builds the board probe and runs one
//! subcommand. It returns the raw exit code (0, 1, 2, 3, 64, 75, 76, 77, or that of CMD).
//!
//! The board check asks the board in process. The hidden test seam `--crewboard BIN` asks a stub
//! binary through `BIN status --json` instead, so that the shell suite in `skill/loops/tests`
//! runs unchanged against this code.

#[cfg(unix)]
use crate::base::ignore::best_effort_output;
use crate::fsutil::find_on_path;
#[cfg(unix)]
use crate::lock::args::RunCtx;
use crate::lock::board::{BoardAgent, BoardProbe, ProbeError};
use std::path::Path;

/// The refusal of a build without Unix process control.
#[cfg(not(unix))]
const UNIX_ONLY: &str = "refused [rule=unix-only]: crewboard lock needs a Unix host (macOS or Linux); this build has no loop on Windows";

/// The cause of a failed board check: the first line of the error text, 200 characters at most.
fn cause_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(200)
        .collect()
}

/// `lock take|release|status|break|run` skip the loader of the command. The board check needs the
/// token, so it loads the env file here; a file that does not load fails the check.
fn load_for_probe(config_dir: Option<&Path>) -> Result<(), ProbeError> {
    if let Ok(dir) = crate::config::ConfigDir::resolve(config_dir) {
        crate::agent_env::init(&dir.0, None);
    }
    match crate::agent_env::failure() {
        Some(e) => Err(ProbeError::Failed {
            cause: cause_line(&e.to_string()),
            exit: Some(4),
        }),
        None => Ok(()),
    }
}

/// Asks the board in process: the agent rows that `crewboard status --json` prints.
pub struct InProcessProbe {
    pub global: super::Global,
}

impl BoardProbe for InProcessProbe {
    fn agents(&self) -> Result<Vec<BoardAgent>, ProbeError> {
        load_for_probe(self.global.config_dir.as_deref())?;
        match super::status::agent_rows(&self.global) {
            Ok(rows) => Ok(rows
                .into_iter()
                .map(|r| BoardAgent {
                    name: r.name,
                    liveness: r.liveness,
                    state: r.state.unwrap_or_default(),
                })
                .collect()),
            Err(e) => Err(ProbeError::Failed {
                cause: cause_line(&e.to_string()),
                exit: None,
            }),
        }
    }
}

/// Asks another binary: `BIN [--config-dir D] status --json [--initiative I]`.
pub struct SubprocessProbe {
    pub bin: String,
    pub config_dir: Option<String>,
    pub initiative: Option<String>,
}

impl BoardProbe for SubprocessProbe {
    fn agents(&self) -> Result<Vec<BoardAgent>, ProbeError> {
        use std::process::{Command, Stdio};
        load_for_probe(self.config_dir.as_deref().map(Path::new))?;
        let mut c = Command::new(&self.bin);
        if let Some(d) = &self.config_dir {
            c.arg("--config-dir").arg(d);
        }
        c.args(["status", "--json"]);
        if let Some(i) = &self.initiative {
            c.arg("--initiative").arg(i);
        }
        // the other binary gets the token and the settings of the env file
        if let Some(env) = crate::agent_env::current() {
            env.apply(&mut c);
        }
        let out =
            crate::spawn::output_retrying(c.stdin(Stdio::null())).map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    ProbeError::NotFound(format!("crewboard not found ({})", self.bin))
                }
                _ => ProbeError::Failed {
                    cause: cause_line(&e.to_string()),
                    exit: None,
                },
            })?;
        if !out.status.success() {
            return Err(ProbeError::Failed {
                cause: cause_line(&String::from_utf8_lossy(&out.stderr)),
                exit: out.status.code(),
            });
        }
        let v: serde_json::Value =
            serde_json::from_slice(&out.stdout).map_err(|_| ProbeError::NoList)?;
        let rows = v
            .get("agents")
            .and_then(|a| a.as_array())
            .ok_or(ProbeError::NoList)?;
        let text = |r: &serde_json::Value, k: &str| {
            r.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Ok(rows
            .iter()
            .map(|r| BoardAgent {
                name: text(r, "name"),
                liveness: text(r, "liveness"),
                state: text(r, "state"),
            })
            .collect())
    }
}

/// Is `bin` the binary that is running now? Then the work (the board check of `lock`, the
/// `watch` of `monitor`) goes in process. A name without a directory is looked up on the PATH
/// as a shell does (the first executable match).
pub(crate) fn is_this_binary(bin: &str) -> bool {
    is_this_binary_on(bin, std::env::var_os("PATH").as_deref())
}

fn is_this_binary_on(bin: &str, path: Option<&std::ffi::OsStr>) -> bool {
    let bare = !bin.is_empty() && Path::new(bin).components().count() == 1;
    let found = if bare {
        path.and_then(|p| find_on_path(bin, p))
    } else {
        Some(Path::new(bin).to_path_buf())
    };
    let (Some(Ok(a)), Ok(b)) = (found.map(std::fs::canonicalize), std::env::current_exe()) else {
        return false;
    };
    std::fs::canonicalize(b).is_ok_and(|b| a == b)
}

#[cfg(unix)]
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Run `crewboard lock <args>`. `config_dir` is the global `--config-dir`.
pub fn run(config_dir: Option<&Path>, args: &[String]) -> i32 {
    #[cfg(not(unix))]
    {
        let _ = (config_dir, args);
        eprintln!("{UNIX_ONLY}");
        2
    }
    #[cfg(unix)]
    {
        run_unix(config_dir, args, &RunCtx::default())
    }
}

/// `crewboard lock run <dir> ... -- CMD...` for a caller in this process (the gate): the command
/// starts in `cwd`, and its stdout and stderr go to the file `output` (truncated first). The
/// result is the exit code of `run`.
#[cfg(unix)]
pub fn run_in(config_dir: Option<&Path>, args: &[String], cwd: &Path, output: &Path) -> i32 {
    let ctx = RunCtx {
        cwd: Some(cwd.to_path_buf()),
        output: Some(output.to_path_buf()),
        ..RunCtx::default()
    };
    run_unix(config_dir, args, &ctx)
}

/// `crewboard lock ci-status <dir>` in process: the line that says what is seen of CI (a
/// `Runner.Worker` process, or an entry of the waiting list next to `dir`), or `None`. A build
/// without Unix process control sees nothing.
pub fn ci_present(dir: &Path) -> Option<String> {
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
    #[cfg(unix)]
    {
        use crate::clock::{Clock, SystemClock};
        let ci_max_age = 21600; // the default of `--ci-max-age`
        crate::lock::ci::present(
            dir,
            &lock_host_name(),
            ci_max_age,
            SystemClock.now(),
            &crate::lock::procs::SystemProcs,
        )
    }
}

/// The name of this host as the lock files record it (`$HOST_LOCK_HOST` for a test).
#[cfg(unix)]
fn lock_host_name() -> String {
    env_nonempty("HOST_LOCK_HOST")
        .or_else(crate::lock::sys::short_hostname)
        .unwrap_or_else(|| "host".to_string())
}

/// `lock run` starts a command. Under a scope (a headless config, the system scope file, or a wake
/// that the scheduler started: `$CREWBOARD_WAKE`) it is allowed only for a lock dir that the wake
/// names with `--host-lock` (`$CREWBOARD_LOCK_RUN_DIRS`, a `:` list). The wake sets both
/// variables for its model; a `crewboard ...` command line of the model cannot set them (the
/// allowlist matches commands that start with `crewboard`). The other subcommands are not
/// restricted. `Err` is the refusal line.
#[cfg(unix)]
fn run_grant(dir: &str, config_dir: Option<&Path>) -> Result<(), String> {
    let wake = env_nonempty("CREWBOARD_WAKE").is_some();
    let scoped = match crate::headless::in_scope(config_dir) {
        Ok(s) => s,
        Err(e) => return Err(e.to_string()),
    };
    if !wake && !scoped {
        return Ok(());
    }
    run_grant_decision(
        dir,
        env_nonempty("CREWBOARD_LOCK_RUN_DIRS")
            .as_deref()
            .unwrap_or(""),
    )
}

/// The decision of [`run_grant`] once a scope binds the process: is `dir` in the `:` list?
#[cfg(unix)]
fn run_grant_decision(dir: &str, granted: &str) -> Result<(), String> {
    let want = crate::lock::phys_dir(dir);
    if granted
        .split(':')
        .filter(|g| !g.is_empty())
        .any(|g| crate::lock::phys_dir(g) == want)
    {
        return Ok(());
    }
    Err(format!(
        "refused [rule=lock-run-not-granted]: `lock run` starts a command, and a wake or headless scope allows it only for a lock dir that the wake names with --host-lock; {dir} is not one of them. Use take and release, or ask the owner to install the wake with --host-lock {dir}"
    ))
}

#[cfg(unix)]
fn run_unix(config_dir: Option<&Path>, args: &[String], job: &RunCtx) -> i32 {
    use crate::clock::SystemClock;
    use crate::lock::args::{ArgEnv, Cmd, Parse, Parsed};
    use crate::lock::board::BoardCheck;
    use crate::lock::help::HELP;
    use crate::lock::log::Log;
    use crate::lock::procs::SystemProcs;
    use crate::lock::run::dispatch;
    use crate::lock::{Grant, Opts, Session};
    use std::io::Write;

    let clock = SystemClock;
    let env = ArgEnv {
        max_age: env_nonempty("HOST_LOCK_MAX_AGE"),
        empty_grace: env_nonempty("HOST_LOCK_EMPTY_GRACE"),
        config_dir: config_dir
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| env_nonempty("CREWBOARD_CONFIG_DIR")),
    };
    let p: Parsed = match crate::lock::args::parse(args, &env) {
        Ok(Parse::Help) => {
            best_effort_output(writeln!(std::io::stdout(), "{HELP}"));
            return 0;
        }
        Ok(Parse::Run(p)) => *p,
        Err(msg) => {
            Log::new(&clock, None).say(&msg);
            return 64;
        }
    };
    if p.cmd == Cmd::Run
        && let Err(line) = run_grant(&p.dir, config_dir)
    {
        best_effort_output(writeln!(std::io::stderr(), "{line}"));
        return 2;
    }
    let log = Log::new(&clock, p.log.as_ref().map(Into::into));
    let procs = SystemProcs;
    let probe: Option<Box<dyn BoardProbe>> = if p.no_board {
        None
    } else {
        match p.crewboard.as_deref().filter(|b| !is_this_binary(b)) {
            Some(bin) => Some(Box::new(SubprocessProbe {
                bin: bin.to_string(),
                config_dir: p.config_dir.clone(),
                initiative: p.initiative.clone(),
            })),
            None => Some(Box::new(InProcessProbe {
                global: super::Global {
                    config_dir: p.config_dir.as_ref().map(Into::into),
                    initiative: p.initiative.clone(),
                    ..Default::default()
                },
            })),
        }
    };
    let ttl = env_nonempty("HOST_LOCK_BOARD_TTL")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(60);
    let host = lock_host_name();
    let board = BoardCheck::new(
        probe,
        ttl,
        p.config_dir.is_some(),
        p.cmd == Cmd::Status,
        &log,
    );
    let mut s = Session {
        dir: p.dir.clone().into(),
        host,
        opts: Opts {
            owner: crate::lock::owner::one_line(&p.owner),
            reason: crate::lock::owner::one_line(&p.reason),
            wait: p.wait,
            poll: p.poll,
            expect: p.expect,
            max_age: p.max_age,
            empty_grace: p.empty_grace,
            ci_max_age: p.ci_max_age,
            ci_mode: p.ci_mode,
            gate: p.gate,
        },
        clock: &clock,
        procs: &procs,
        log: &log,
        board,
        grant: Grant {
            dir: env_nonempty("CREWBOARD_HOST_LOCK"),
            pid: env_nonempty("CREWBOARD_HOST_LOCK_PID"),
        },
        nested: false,
        lock_pid: None,
        detached: false,
    };
    let cfg_dir = p
        .config_dir
        .as_ref()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            crate::config::ConfigDir::resolve(config_dir)
                .ok()
                .map(|d| d.0)
        });
    let ctx = RunCtx {
        orig_args: std::env::args_os().skip(1).collect(),
        supervise: env_nonempty("HOST_LOCK_SUPERVISE"),
        cwd: job.cwd.clone(),
        output: job.output.clone(),
        // the job of an agent dir must not inherit a token of the caller
        env_remove: if cfg_dir
            .as_deref()
            .is_some_and(crate::agent_env::is_agent_dir)
        {
            crate::agent_env::TOKEN_VARS
                .iter()
                .map(|v| (*v).to_string())
                .collect()
        } else {
            Vec::new()
        },
    };
    dispatch(&mut s, &p, &ctx)
}

/// `lock release DIR --holder HOLDER` in this process, for the wake that ended at its time budget.
/// Returns the exit code and the first line of what it said (a time stamp and `host-lock:` first,
/// as the command prints it); the lines are not printed. No board is asked.
#[cfg(unix)]
pub fn release_for(dir: &str, owner: &str) -> (i32, String) {
    use crate::clock::SystemClock;
    use crate::lock::args::{ArgEnv, Parse};
    use crate::lock::board::BoardCheck;
    use crate::lock::log::Log;
    use crate::lock::procs::SystemProcs;
    use crate::lock::{Grant, Opts, Session, sys};

    let clock = SystemClock;
    let env = ArgEnv {
        max_age: env_nonempty("HOST_LOCK_MAX_AGE"),
        empty_grace: env_nonempty("HOST_LOCK_EMPTY_GRACE"),
        config_dir: None,
    };
    let argv = [
        "release".to_string(),
        dir.to_string(),
        "--holder".into(),
        owner.into(),
    ];
    let p = match crate::lock::args::parse(&argv, &env) {
        Ok(Parse::Run(p)) => *p,
        Ok(Parse::Help) => return (64, "release: no lock command".to_string()),
        Err(msg) => return (64, msg),
    };
    let log = Log::memory(&clock);
    let procs = SystemProcs;
    let host = env_nonempty("HOST_LOCK_HOST")
        .or_else(sys::short_hostname)
        .unwrap_or_else(|| "host".to_string());
    let board = BoardCheck::new(None, 60, false, false, &log);
    let mut s = Session {
        dir: p.dir.clone().into(),
        host,
        opts: Opts {
            owner: crate::lock::owner::one_line(&p.owner),
            reason: crate::lock::owner::one_line(&p.reason),
            wait: p.wait,
            poll: p.poll,
            expect: p.expect,
            max_age: p.max_age,
            empty_grace: p.empty_grace,
            ci_max_age: p.ci_max_age,
            ci_mode: p.ci_mode,
            gate: p.gate,
        },
        clock: &clock,
        procs: &procs,
        log: &log,
        board,
        grant: Grant {
            dir: env_nonempty("CREWBOARD_HOST_LOCK"),
            pid: env_nonempty("CREWBOARD_HOST_LOCK_PID"),
        },
        nested: false,
        lock_pid: None,
        detached: false,
    };
    let code = s.release();
    let first = log.lines().into_iter().next().unwrap_or_default();
    (code, first)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stub `crewboard` that records its arguments and answers with `reply` (a shell fragment).
    fn stub(dir: &Path, reply: &str) -> (String, std::path::PathBuf) {
        let argv = dir.join("argv.txt");
        let bin = dir.join("crewboard");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\necho \"$*\" >>{}\n{reply}\n", argv.display()),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin.to_string_lossy().to_string(), argv)
    }

    const OK: &str = r#"echo '{"agents":[{"name":"own@x","liveness":"dead","state":"active"},{"name":"gone@x","liveness":"-","state":"left"},{"name":"bare@x"}]}'"#;

    fn probe(bin: &str, cfg: Option<&str>, ini: Option<&str>) -> SubprocessProbe {
        SubprocessProbe {
            bin: bin.to_string(),
            config_dir: cfg.map(str::to_string),
            initiative: ini.map(str::to_string),
        }
    }

    fn argv_of(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap().trim().to_string()
    }

    #[test]
    fn e6_the_config_dir_flows_to_the_stub_as_a_global_option_before_status() {
        let d = tempfile::tempdir().unwrap();
        let (bin, argv) = stub(d.path(), OK);
        let rows = probe(&bin, Some("/x/agentdir"), None).agents().unwrap();
        assert_eq!(argv_of(&argv), "--config-dir /x/agentdir status --json");
        assert_eq!(rows[0].name, "own@x");
        assert_eq!(
            (rows[0].liveness.as_str(), rows[0].state.as_str()),
            ("dead", "active")
        );
        assert_eq!(rows[1].state, "left");
        // a row without fields has empty words, as `// ""` of the jq filter gave
        assert_eq!(
            (rows[2].liveness.as_str(), rows[2].state.as_str()),
            ("", "")
        );
    }

    #[test]
    fn e6b_config_dir_and_initiative_are_passed_in_this_order() {
        let d = tempfile::tempdir().unwrap();
        let (bin, argv) = stub(d.path(), OK);
        probe(&bin, Some("/x/optdir"), Some("i1")).agents().unwrap();
        assert_eq!(
            argv_of(&argv),
            "--config-dir /x/optdir status --json --initiative i1"
        );
    }

    #[test]
    fn e6c_neither_set_gives_no_config_dir_argument() {
        let d = tempfile::tempdir().unwrap();
        let (bin, argv) = stub(d.path(), OK);
        probe(&bin, None, None).agents().unwrap();
        assert_eq!(argv_of(&argv), "status --json");
    }

    #[test]
    fn a_failing_stub_gives_its_first_stderr_line_capped_and_its_exit_code() {
        let d = tempfile::tempdir().unwrap();
        let long = "x".repeat(300);
        let (bin, _) = stub(
            d.path(),
            &format!(
                "echo 'error: API rate limit exceeded {long}' >&2; echo 'second line' >&2; exit 1"
            ),
        );
        let e = probe(&bin, None, None).agents().unwrap_err();
        let ProbeError::Failed { cause, exit } = e else {
            panic!("{e:?}")
        };
        assert!(cause.starts_with("error: API rate limit exceeded "));
        assert_eq!(cause.chars().count(), 200);
        assert!(!cause.contains("second line"));
        assert_eq!(exit, Some(1));
    }

    #[test]
    fn an_answer_without_an_agent_list_and_a_missing_binary_have_their_own_errors() {
        let d = tempfile::tempdir().unwrap();
        let (bin, _) = stub(d.path(), "echo '{}'");
        assert_eq!(
            probe(&bin, None, None).agents().unwrap_err(),
            ProbeError::NoList
        );
        let (bin, _) = stub(d.path(), "echo 'not json'");
        assert_eq!(
            probe(&bin, None, None).agents().unwrap_err(),
            ProbeError::NoList
        );
        let (bin, _) = stub(d.path(), r#"echo '{"agents":"x"}'"#);
        assert_eq!(
            probe(&bin, None, None).agents().unwrap_err(),
            ProbeError::NoList
        );
        assert_eq!(
            probe("/nonexistent/crewboard", None, None)
                .agents()
                .unwrap_err(),
            ProbeError::NotFound("crewboard not found (/nonexistent/crewboard)".into())
        );
    }

    #[test]
    fn lock_run_is_granted_only_for_a_dir_that_the_wake_names() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.lock.d"), d.path().join("b.lock.d"));
        let (a, b) = (
            a.to_string_lossy().to_string(),
            b.to_string_lossy().to_string(),
        );
        assert!(run_grant_decision(&a, &format!("{a}:{b}")).is_ok());
        // the other direction: nothing granted, or another dir, is refused with the named rule
        for granted in ["", &b] {
            let e = run_grant_decision(&a, granted).unwrap_err();
            assert!(e.contains("[rule=lock-run-not-granted]"), "{e}");
        }
        // a symlink spelling of a granted dir is the same dir
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(run_grant_decision(&link.to_string_lossy(), &real.to_string_lossy()).is_ok());
    }

    #[test]
    fn the_seam_names_this_binary_only_when_it_is_this_binary() {
        let me = std::env::current_exe().unwrap();
        assert!(is_this_binary(&me.to_string_lossy()));
        let d = tempfile::tempdir().unwrap();
        let (bin, _) = stub(d.path(), OK);
        assert!(!is_this_binary(&bin));
        assert!(!is_this_binary("/nonexistent/crewboard"));
    }

    #[cfg(unix)]
    #[test]
    fn a_bare_name_on_path_is_this_binary() {
        let me = std::env::current_exe().unwrap();
        let d = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(&me, d.path().join("crewboard")).unwrap();
        let other = tempfile::tempdir().unwrap();
        // another executable called `crewboard`
        stub(other.path(), OK);
        let path = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();
        // the first executable match counts
        assert!(is_this_binary_on("crewboard", Some(&path(&[d.path()]))));
        assert!(is_this_binary_on(
            "crewboard",
            Some(&path(&[d.path(), other.path()]))
        ));
        // twins: another binary of that name first, no such name, no PATH, a file that is not executable
        assert!(!is_this_binary_on(
            "crewboard",
            Some(&path(&[other.path(), d.path()]))
        ));
        assert!(!is_this_binary_on("nothing-here", Some(&path(&[d.path()]))));
        assert!(!is_this_binary_on("crewboard", None));
        assert!(!is_this_binary_on(
            "crewboard",
            Some(&path(&[other.path()]))
        ));
        use std::os::unix::fs::PermissionsExt;
        let plain = d.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_this_binary_on("plain", Some(&path(&[d.path()]))));
    }
}
