//! `crewboard wake install|uninstall|status` against a fake HOME and fake `systemctl`,
//! `launchctl`, `loginctl` and `crontab` programs on the PATH: no test touches a real scheduler.
//! `CREWBOARD_WAKE_OS` plays the operating system, so each scheduler runs on any host.
#![cfg(unix)]

use crewboard::config::{AgentConfig, BoardConfig, ConfigDir};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXE: &str = env!("CARGO_BIN_EXE_crewboard");
const BOARD: &str = "initiative = \"demo\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn all(&self) -> String {
        format!(
            "exit {}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}

fn exe(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Rig {
        let rig = Rig {
            dir: tempfile::tempdir().unwrap(),
        };
        let fakes = rig.path("fakes");
        for c in ["systemctl", "launchctl"] {
            exe(
                &fakes.join(c),
                &format!(
                    "#!/bin/bash\necho \"{c} $*\" >>\"$RIG/calls\"\ncase \"$*\" in *is-enabled*) echo enabled;; esac\nexit 0\n"
                ),
            );
        }
        exe(
            &fakes.join("loginctl"),
            "#!/bin/bash\necho \"loginctl $*\" >>\"$RIG/calls\"\nif [ -f \"$RIG/linger\" ]; then echo Linger=yes; else echo Linger=no; fi\n",
        );
        exe(
            &fakes.join("crontab"),
            "#!/bin/bash\necho \"crontab $*\" >>\"$RIG/calls\"\nif [ \"$1\" = -l ]; then [ -f \"$RIG/crontab\" ] || exit 1; cat \"$RIG/crontab\"; else cat >\"$RIG/crontab.new\"; mv \"$RIG/crontab.new\" \"$RIG/crontab\"; fi\n",
        );
        for r in ["claude", "codex", "hermes"] {
            exe(&rig.path("nvm").join(r), "#!/bin/sh\nexit 0\n");
        }
        std::fs::create_dir_all(rig.path("wd")).unwrap();
        std::fs::create_dir_all(rig.path("home")).unwrap();
        std::fs::write(rig.path("env"), "GH_TOKEN=ghp_faketoken\n").unwrap();
        std::fs::set_permissions(rig.path("env"), std::fs::Permissions::from_mode(0o600)).unwrap();
        rig
    }

    fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }

    fn home(&self) -> PathBuf {
        self.path("home")
    }

    fn agent(&self, name: &str) -> PathBuf {
        let d = self.home().join(".config/crewboard/agents").join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The commands of the scheduler that ran, without the read-only `crontab -l` (the scan for
    /// wake jobs of other names reads the crontab on every install).
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("calls"))
            .unwrap_or_default()
            .lines()
            .filter(|l| *l != "crontab -l")
            .map(str::to_string)
            .collect()
    }

    fn crontab(&self) -> String {
        std::fs::read_to_string(self.path("crontab")).unwrap_or_default()
    }

    /// `crewboard <args>` with a clean environment: the fake HOME, the fake scheduler programs.
    fn run(&self, os: &str, args: &[&str]) -> Out {
        let o = Command::new(EXE)
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("USER", "wf")
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("fakes").display()),
            )
            .env("RIG", self.dir.path())
            .env("CREWBOARD_WAKE_OS", os)
            .output()
            .unwrap();
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    /// `wake install` for a dev agent with codex; `extra` after the common options.
    fn install(&self, os: &str, sched: &str, agent_dir: &Path, extra: &[&str]) -> Out {
        let rt = self.path("nvm/codex").display().to_string();
        self.install_rt(os, sched, agent_dir, "codex", &rt, extra)
    }

    fn install_rt(
        &self,
        os: &str,
        sched: &str,
        agent_dir: &Path,
        runtime: &str,
        bin: &str,
        extra: &[&str],
    ) -> Out {
        let env = self.path("env").display().to_string();
        let wd = self.path("wd").display().to_string();
        let dir = agent_dir.display().to_string();
        let mut a = vec![
            "--env-file",
            &env,
            "wake",
            "install",
            "--scheduler",
            sched,
            "--agent-dir",
            &dir,
            "--agent",
            "dev@host-a",
            "--initiative",
            "demo",
            "--runtime",
            runtime,
            "--work-dir",
            &wd,
            "--runtime-bin",
            bin,
        ];
        a.extend_from_slice(extra);
        self.run(os, &a)
    }

    fn uninstall(&self, os: &str, agent_dir: &Path, extra: &[&str]) -> Out {
        let dir = agent_dir.display().to_string();
        let mut a = vec![
            "wake",
            "uninstall",
            "--agent-dir",
            &dir,
            "--initiative",
            "demo",
        ];
        a.extend_from_slice(extra);
        self.run(os, &a)
    }

    fn systemd_dir(&self) -> PathBuf {
        self.home().join(".config/systemd/user")
    }
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn systemd_install_writes_the_units_enables_them_and_prints_the_one_command_left() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let o = r.install("Linux", "systemd", &dir, &["--every", "10m"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let state = r.home().join(".local/state/crewboard/agents/demo-dev1");
    assert_eq!(
        o.stdout,
        format!(
            "installed crewboard-demo-dev1 (systemd, every 10m); log: {}/wake.log\nnext: loginctl enable-linger \"$USER\"\n",
            state.display()
        )
    );
    let w = dir.join("crewboard-demo-dev1-wake.sh");
    assert_eq!(mode(&w), 0o700);
    let svc = std::fs::read_to_string(r.systemd_dir().join("crewboard-demo-dev1.service")).unwrap();
    assert!(
        svc.contains(&format!("ExecStart=/bin/bash {}\n", w.display())),
        "{svc}"
    );
    let timer = std::fs::read_to_string(r.systemd_dir().join("crewboard-demo-dev1.timer")).unwrap();
    assert!(timer.contains("OnUnitActiveSec=600s\n"), "{timer}");
    assert!(state.is_dir());
    let wrapper = std::fs::read_to_string(&w).unwrap();
    assert!(
        wrapper.contains(&format!("exec '{EXE}' 'wake' '--agent' 'dev@host-a'")),
        "{wrapper}"
    );
    assert_eq!(
        r.calls(),
        [
            "systemctl --user daemon-reload",
            "systemctl --user enable crewboard-demo-dev1.timer",
            "systemctl --user restart crewboard-demo-dev1.timer",
            "loginctl show-user wf --property=Linger",
        ]
    );
}

#[test]
fn with_linger_on_nothing_is_left_to_do_and_a_second_install_replaces_the_first() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    std::fs::write(r.path("linger"), "").unwrap();
    let a = r.install("Linux", "systemd", &dir, &[]);
    assert_eq!(a.code, 0, "{}", a.all());
    assert!(!a.stdout.contains("next:"), "{}", a.all());
    let b = r.install("Linux", "systemd", &dir, &["--every", "2m"]);
    assert_eq!(b.code, 0, "{}", b.all());
    let timer = std::fs::read_to_string(r.systemd_dir().join("crewboard-demo-dev1.timer")).unwrap();
    assert!(timer.contains("OnUnitActiveSec=120s\n"), "{timer}");
    let names: Vec<_> = std::fs::read_dir(r.systemd_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names.len(),
        2,
        "one service and one timer, no copy: {names:?}"
    );
    // no temp file stays next to the units
    assert!(
        std::fs::read_dir(&dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp."))
    );
}

#[test]
fn the_installed_wrapper_runs_from_a_minimal_environment() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    assert_eq!(r.install("Linux", "systemd", &dir, &[]).code, 0);
    let o = Command::new("/bin/bash")
        .arg(dir.join("crewboard-demo-dev1-wake.sh"))
        .arg("--print-cmd")
        .env_clear()
        .env("HOME", r.home())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{out}{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let wd = r.path("wd");
    assert!(
        out.starts_with(&format!(
            "cd {} && {} exec ",
            wd.display(),
            r.path("nvm/codex").display()
        )),
        "{out}"
    );
}

#[test]
fn launchd_install_boots_out_then_bootstraps_the_plist() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let o = r.install("Darwin", "launchd", &dir, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .starts_with("installed crewboard-demo-dev1 (launchd, every 5m); log: "),
        "{}",
        o.all()
    );
    assert!(!o.stdout.contains("next:"), "{}", o.all());
    let plist = r
        .home()
        .join("Library/LaunchAgents/crewboard-demo-dev1.plist");
    let text = std::fs::read_to_string(&plist).unwrap();
    assert!(
        text.contains("<key>Label</key><string>crewboard-demo-dev1</string>"),
        "{text}"
    );
    assert!(text.contains("<integer>300</integer>"), "{text}");
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let uid = uid.trim();
    assert_eq!(
        r.calls(),
        [
            format!("launchctl bootout gui/{uid}/crewboard-demo-dev1"),
            format!("launchctl bootstrap gui/{uid} {}", plist.display()),
        ]
    );
}

#[test]
fn cron_install_adds_one_marked_line_once_and_uninstall_removes_only_that_line() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    std::fs::write(r.path("crontab"), "0 3 * * * /usr/bin/true\n").unwrap();
    assert_eq!(r.install("Linux", "cron", &dir, &["--every", "7m"]).code, 0);
    assert_eq!(r.install("Linux", "cron", &dir, &["--every", "7m"]).code, 0);
    let tab = r.crontab();
    assert_eq!(
        tab.matches("# crewboard:crewboard-demo-dev1").count(),
        1,
        "{tab}"
    );
    assert!(
        tab.starts_with("0 3 * * * /usr/bin/true\n*/7 * * * * /bin/bash '"),
        "{tab}"
    );
    assert!(dir.join("crewboard-demo-dev1.cron").is_file());
    let u = r.uninstall("Linux", &dir, &[]);
    assert_eq!(u.code, 0, "{}", u.all());
    assert_eq!(r.crontab(), "0 3 * * * /usr/bin/true\n");
    assert!(!dir.join("crewboard-demo-dev1.cron").exists());
    assert!(!dir.join("crewboard-demo-dev1-wake.sh").exists());
}

#[test]
fn hermes_install_puts_the_wrapper_in_the_hermes_dir_and_prints_the_cron_create() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let env = r.path("env").display().to_string();
    let wd = r.path("wd").display().to_string();
    let rt = r.path("nvm/hermes").display().to_string();
    let d = dir.display().to_string();
    let o = r.run(
        "Linux",
        &[
            "--env-file",
            &env,
            "wake",
            "install",
            "--agent-dir",
            &d,
            "--agent",
            "dev@host-a",
            "--initiative",
            "demo",
            "--runtime",
            "hermes",
            "--work-dir",
            &wd,
            "--runtime-bin",
            &rt,
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        r.home()
            .join(".hermes/scripts/crewboard-demo-dev1-wake.sh")
            .is_file()
    );
    assert!(
        o.stdout.contains("next: hermes cron create --name crewboard-demo-dev1 --no-agent --script crewboard-demo-dev1-wake.sh --deliver local --failure-deliver origin \"every 5m\"\n"),
        "{}",
        o.all()
    );
    assert!(r.calls().is_empty(), "{:?}", r.calls());
}

#[test]
fn uninstall_disables_removes_both_unit_files_and_keeps_the_log() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    assert_eq!(r.install("Linux", "systemd", &dir, &[]).code, 0);
    let other = r.agent("dev2");
    assert_eq!(r.install("Linux", "systemd", &other, &[]).code, 0);
    let state = r.home().join(".local/state/crewboard/agents/demo-dev1");
    std::fs::write(state.join("wake.log"), "x\n").unwrap();
    let o = r.uninstall("Linux", &dir, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        r.calls()
            .contains(&"systemctl --user disable --now crewboard-demo-dev1.timer".to_string())
    );
    assert!(!r.systemd_dir().join("crewboard-demo-dev1.timer").exists());
    assert!(!r.systemd_dir().join("crewboard-demo-dev1.service").exists());
    assert!(!dir.join("crewboard-demo-dev1-wake.sh").exists());
    assert!(
        r.systemd_dir().join("crewboard-demo-dev2.timer").exists(),
        "the other agent stays"
    );
    assert!(state.join("wake.log").is_file());
    assert!(
        o.stdout.contains(&format!(
            "kept: the state and the log in {}",
            state.display()
        )),
        "{}",
        o.all()
    );
    // a second uninstall finds nothing and says so
    let again = r.uninstall("Linux", &dir, &[]);
    assert_eq!(again.code, 0, "{}", again.all());
    assert!(
        again.stdout.starts_with("nothing installed for demo in "),
        "{}",
        again.all()
    );
}

#[test]
fn an_install_of_the_shell_script_with_the_doubled_name_is_replaced_in_place() {
    let r = Rig::new();
    let dir = r.agent("demo-claude");
    // what `agent-wake.sh --install systemd` of an earlier version left behind
    std::fs::create_dir_all(r.systemd_dir()).unwrap();
    std::fs::write(
        r.systemd_dir().join("crewboard-demo-demo-claude.timer"),
        "[Timer]\n",
    )
    .unwrap();
    std::fs::write(
        r.systemd_dir().join("crewboard-demo-demo-claude.service"),
        "[Service]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("crewboard-demo-demo-claude-wake.sh"),
        "#!/bin/bash\nexec /bin/bash /x/agent-wake.sh --role dev\n",
    )
    .unwrap();
    let o = r.install("Linux", "systemd", &dir, &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .starts_with("installed crewboard-demo-demo-claude (systemd"),
        "{}",
        o.all()
    );
    assert!(
        o.stderr
            .contains("keeping the installed name crewboard-demo-demo-claude"),
        "{}",
        o.all()
    );
    let w = std::fs::read_to_string(dir.join("crewboard-demo-demo-claude-wake.sh")).unwrap();
    assert!(w.contains("'wake'"), "the wrapper is the new one: {w}");
    assert!(!r.systemd_dir().join("crewboard-demo-claude.timer").exists());
    assert!(!dir.join("crewboard-demo-claude-wake.sh").exists());
    // uninstall removes the old name as well
    assert_eq!(r.uninstall("Linux", &dir, &[]).code, 0);
    assert!(
        !r.systemd_dir()
            .join("crewboard-demo-demo-claude.timer")
            .exists()
    );
    assert!(!dir.join("crewboard-demo-demo-claude-wake.sh").exists());
}

#[test]
fn dry_run_prints_the_units_and_the_commands_and_changes_nothing() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let o = r.install("Linux", "systemd", &dir, &["--dry-run"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("Description=crewboard wake for dev@host-a in demo"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout
            .contains("--- would run\nsystemctl --user daemon-reload\n"),
        "{}",
        o.stdout
    );
    assert!(
        o.stdout.contains("loginctl enable-linger \"$USER\""),
        "{}",
        o.stdout
    );
    assert!(!r.systemd_dir().exists());
    assert!(!dir.join("crewboard-demo-dev1-wake.sh").exists());
    assert!(r.calls().is_empty(), "{:?}", r.calls());
}

#[test]
fn a_wrong_option_exits_4_and_writes_nothing() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let on_linux = r.install("Linux", "launchd", &dir, &[]);
    assert_eq!(on_linux.code, 4, "{}", on_linux.all());
    assert!(
        on_linux
            .stderr
            .contains("launchd is the scheduler of macOS"),
        "{}",
        on_linux.all()
    );
    let nope = r.path("nope/codex").display().to_string();
    let no_rt = r.install_rt("Linux", "systemd", &dir, "codex", &nope, &[]);
    assert_eq!(no_rt.code, 4, "{}", no_rt.all());
    let bad_every = r.install("Linux", "systemd", &dir, &["--every", "soon"]);
    assert_eq!(bad_every.code, 64, "{}", bad_every.all());
    let claude = r.path("nvm/claude").display().to_string();
    let mac_cron = r.install_rt("Darwin", "cron", &dir, "claude", &claude, &[]);
    assert_eq!(mac_cron.code, 4, "{}", mac_cron.all());
    assert!(mac_cron.stderr.contains("keychain"), "{}", mac_cron.all());
    let no_dir = r.run("Linux", &["wake", "install", "--scheduler", "cron"]);
    assert_eq!(no_dir.code, 4, "{}", no_dir.all());
    assert!(!r.systemd_dir().exists());
    assert!(!dir.join("crewboard-demo-dev1-wake.sh").exists());
    assert!(r.calls().is_empty(), "{:?}", r.calls());
}

#[test]
fn a_failing_scheduler_command_is_an_error_with_its_text() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    exe(
        &r.path("fakes/systemctl"),
        "#!/bin/bash\necho 'no bus' >&2\nexit 1\n",
    );
    let o = r.install("Linux", "systemd", &dir, &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("`systemctl --user daemon-reload` failed: no bus"),
        "{}",
        o.all()
    );
}

#[test]
fn the_names_and_the_agent_come_from_the_agent_config_and_a_coordinator_ticks() {
    let r = Rig::new();
    let dir = r.agent("coord");
    let cd = ConfigDir(dir.clone());
    cd.save_agent(&AgentConfig {
        name: "coord@host-a".into(),
        runtime: "claude-code".into(),
        host: "host-a".into(),
        item_id: "PVTI_x".into(),
        board: BoardConfig::parse(BOARD).unwrap(),
        tools: None,
        board_sha: None,
        loop_cfg: None,
        wake: None,
        insecure_one_account: false,
    })
    .unwrap();
    let d = dir.display().to_string();
    let env = r.path("env").display().to_string();
    let o = r.run(
        "Linux",
        &[
            "--env-file",
            &env,
            "wake",
            "install",
            "--scheduler",
            "cron",
            "--agent-dir",
            &d,
            "--tick",
        ],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .starts_with("installed crewboard-demo-coord (cron, every 5m)"),
        "{}",
        o.all()
    );
    let w = std::fs::read_to_string(dir.join("crewboard-demo-coord-wake.sh")).unwrap();
    assert!(
        w.contains("'wake' '--tick' '--initiative' 'demo' '--env-file'"),
        "{w}"
    );
    assert!(!w.contains("--runtime"), "{w}");
    assert!(r.crontab().contains("# crewboard:crewboard-demo-coord"));
}

#[test]
fn status_names_the_unit_whether_it_is_enabled_and_the_last_run() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    let d = dir.display().to_string();
    let none = r.run(
        "Linux",
        &["wake", "status", "--agent-dir", &d, "--initiative", "demo"],
    );
    assert_eq!(none.code, 0, "{}", none.all());
    assert!(
        none.stdout.starts_with("no unit for demo in "),
        "{}",
        none.all()
    );
    assert!(none.stdout.contains("last run: never"), "{}", none.all());
    assert_eq!(r.install("Linux", "systemd", &dir, &[]).code, 0);
    let state = r.home().join(".local/state/crewboard/agents/demo-dev1");
    std::fs::write(state.join("wake.log"), "x\n").unwrap();
    let o = r.run(
        "Linux",
        &["wake", "status", "--agent-dir", &d, "--initiative", "demo"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .starts_with("crewboard-demo-dev1 (systemd): installed, enabled\nlast run: 20"),
        "{}",
        o.all()
    );
}

#[test]
fn install_and_uninstall_are_refused_in_a_headless_config_but_status_is_not() {
    let r = Rig::new();
    let dir = r.agent("coord-headless");
    let cd = ConfigDir(dir.clone());
    cd.save_agent(&AgentConfig {
        name: "coord@host-a".into(),
        runtime: "claude-code".into(),
        host: "host-a".into(),
        item_id: "PVTI_x".into(),
        board: BoardConfig::parse(BOARD).unwrap(),
        tools: None,
        board_sha: None,
        loop_cfg: None,
        wake: None,
        insecure_one_account: false,
    })
    .unwrap();
    let text = std::fs::read_to_string(dir.join("demo.toml")).unwrap();
    std::fs::write(dir.join("demo.toml"), format!("headless = true\n{text}")).unwrap();
    let d = dir.display().to_string();
    let cfg = ["--config-dir", &d];
    let mut inst = cfg.to_vec();
    inst.extend([
        "wake",
        "install",
        "--tick",
        "--initiative",
        "demo",
        "--dry-run",
    ]);
    let o = r.run("Linux", &inst);
    assert_ne!(o.code, 0, "{}", o.all());
    let mut st = cfg.to_vec();
    st.extend(["wake", "status", "--initiative", "demo"]);
    assert_eq!(r.run("Linux", &st).code, 0);
}

/// Wake jobs of other names for `dev1`: a LaunchAgent that runs `poll.sh`, a LaunchAgent that
/// runs a wake.sh which calls agent-wake.sh, a systemd service and a crontab line.
fn foreign_jobs(r: &Rig, dir: &Path) {
    let d = dir.display();
    std::fs::write(
        dir.join("poll.sh"),
        format!("#!/bin/bash\nexec crewboard poll --config-dir {d}\n"),
    )
    .unwrap();
    std::fs::write(
        r.home().join("wake.sh"),
        format!("#!/bin/bash\nexec /x/agent-wake.sh --runtime codex --config-dir '{d}'\n"),
    )
    .unwrap();
    let la = r.home().join("Library/LaunchAgents");
    std::fs::create_dir_all(&la).unwrap();
    let plist = |label: &str, script: &str| {
        format!(
            "<?xml version=\"1.0\"?>\n<plist><dict>\n<key>Label</key><string>{label}</string>\n<key>ProgramArguments</key>\n<array><string>/bin/bash</string><string>{script}</string></array>\n</dict></plist>\n"
        )
    };
    std::fs::write(
        la.join("com.crewboard.demo.dev1.plist"),
        plist("com.crewboard.demo.dev1", &format!("{d}/poll.sh")),
    )
    .unwrap();
    std::fs::write(
        la.join("com.example.crewboard-headless.plist"),
        plist(
            "com.example.crewboard-headless",
            &r.home().join("wake.sh").display().to_string(),
        ),
    )
    .unwrap();
    std::fs::write(
        la.join("com.unrelated.plist"),
        plist("com.unrelated", "/usr/bin/true"),
    )
    .unwrap();
    std::fs::create_dir_all(r.systemd_dir()).unwrap();
    std::fs::write(
        r.systemd_dir().join("old-wake.service"),
        format!("[Service]\nExecStart=/bin/bash /x/agent-wake.sh --config-dir {d}\n"),
    )
    .unwrap();
    std::fs::write(
        r.path("crontab"),
        format!("0 3 * * * /usr/bin/true\n*/10 * * * * crewboard poll --config-dir {d}\n"),
    )
    .unwrap();
}

#[test]
fn another_wake_job_for_the_same_agent_refuses_the_install_with_exit_2() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    foreign_jobs(&r, &dir);
    let o = r.install("Darwin", "launchd", &dir, &[]);
    assert_eq!(o.code, 2, "{}", o.all());
    let la = r.home().join("Library/LaunchAgents");
    for l in [
        format!(
            "another wake job runs this agent: launchd com.crewboard.demo.dev1 ({}/com.crewboard.demo.dev1.plist)",
            la.display()
        ),
        format!(
            "another wake job runs this agent: launchd com.example.crewboard-headless ({}/com.example.crewboard-headless.plist)",
            la.display()
        ),
        format!(
            "another wake job runs this agent: systemd old-wake ({}/old-wake.service)",
            r.systemd_dir().display()
        ),
    ] {
        assert!(o.stderr.contains(&l), "{l}\n{}", o.all());
    }
    assert!(
        o.stderr
            .contains("another wake job runs this agent: cron */10 * * * * crewboard poll"),
        "{}",
        o.all()
    );
    assert!(!o.stderr.contains("com.unrelated"), "{}", o.all());
    assert!(o.stderr.contains("--replace"), "{}", o.all());
    // nothing written, nothing started
    assert!(!la.join("crewboard-demo-dev1.plist").exists());
    assert!(!dir.join("crewboard-demo-dev1-wake.sh").exists());
    assert_eq!(
        r.calls().iter().filter(|c| *c != "crontab -l").count(),
        0,
        "{:?}",
        r.calls()
    );
    // status lists them
    let d = dir.display().to_string();
    let st = r.run(
        "Darwin",
        &["wake", "status", "--agent-dir", &d, "--initiative", "demo"],
    );
    assert_eq!(st.code, 0, "{}", st.all());
    assert_eq!(
        st.stdout
            .matches("another wake job runs this agent:")
            .count(),
        4,
        "{}",
        st.stdout
    );
}

#[test]
fn replace_unloads_backs_up_and_removes_the_other_jobs_then_installs() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    foreign_jobs(&r, &dir);
    let o = r.install("Darwin", "launchd", &dir, &["--replace"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let la = r.home().join("Library/LaunchAgents");
    let baks = |d: &Path, stem: &str| -> usize {
        std::fs::read_dir(d)
            .unwrap()
            .filter(|e| {
                let n = e
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .into_owned();
                n.starts_with(stem) && n.contains(".bak-")
            })
            .count()
    };
    assert!(!la.join("com.crewboard.demo.dev1.plist").exists());
    assert_eq!(baks(&la, "com.crewboard.demo.dev1.plist"), 1);
    assert!(!la.join("com.example.crewboard-headless.plist").exists());
    assert_eq!(baks(&la, "com.example.crewboard-headless.plist"), 1);
    assert!(!r.systemd_dir().join("old-wake.service").exists());
    assert_eq!(baks(&r.systemd_dir(), "old-wake.service"), 1);
    assert!(la.join("com.unrelated.plist").exists());
    let tab = r.crontab();
    assert!(tab.starts_with("0 3 * * * /usr/bin/true\n"), "{tab}");
    assert!(!tab.contains("crewboard poll"), "{tab}");
    assert_eq!(tab, "0 3 * * * /usr/bin/true\n");
    assert_eq!(baks(&dir, "crontab"), 1);
    // the scripts of the other jobs stay
    assert!(dir.join("poll.sh").exists());
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let uid = uid.trim();
    let calls = r.calls();
    assert!(
        calls.contains(&format!(
            "launchctl bootout gui/{uid}/com.crewboard.demo.dev1"
        )),
        "{calls:?}"
    );
    assert!(
        calls.contains(&format!(
            "launchctl bootout gui/{uid}/com.example.crewboard-headless"
        )),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"systemctl --user disable --now old-wake.timer".to_string()),
        "{calls:?}"
    );
    assert!(la.join("crewboard-demo-dev1.plist").is_file());
    // a second install finds only its own job
    assert_eq!(r.install("Darwin", "launchd", &dir, &[]).code, 0);
}

#[test]
fn dry_run_with_replace_prints_what_goes_and_changes_nothing() {
    let r = Rig::new();
    let dir = r.agent("dev1");
    foreign_jobs(&r, &dir);
    let o = r.install("Darwin", "launchd", &dir, &["--replace", "--dry-run"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("--- would replace launchd com.crewboard.demo.dev1\n"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains(".plist.bak-"), "{}", o.stdout);
    assert!(
        r.home()
            .join("Library/LaunchAgents/com.crewboard.demo.dev1.plist")
            .exists()
    );
    assert!(r.calls().is_empty());
}
