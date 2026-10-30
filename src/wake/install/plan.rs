//! The plan of `wake install` and `wake uninstall`: which files to write or remove and which
//! commands to run. Nothing here writes; `apply` does. The names and the paths are stable
//! across releases, so an earlier install is found and replaced, never doubled.

use super::sys::Sys;
use super::units::{self, UnitVars, cron_marker, sq};
use super::{Os, Scheduler};
use crate::wake::job::{self, abs, program};
use crate::wake::settings::WakeArgs;
use crate::wake::state::safe_key;
use std::path::{Path, PathBuf};

/// The host that the units are made for.
#[derive(Debug, Clone)]
pub struct Host {
    pub home: PathBuf,
    pub xdg_config: Option<PathBuf>,
    /// `$PATH` of this process.
    pub path: String,
    pub os: Os,
    pub user: Option<String>,
    pub uid: String,
}

impl Host {
    pub fn systemd_dir(&self) -> PathBuf {
        self.xdg_config
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| self.home.join(".config"))
            .join("systemd/user")
    }

    pub fn plist(&self, name: &str) -> PathBuf {
        self.home
            .join("Library/LaunchAgents")
            .join(format!("{name}.plist"))
    }
}

/// Which agent a unit is for.
#[derive(Debug, Clone)]
pub struct Target {
    /// The config dir of the agent, absolute.
    pub config_dir: PathBuf,
    pub initiative: String,
    /// The agent name; `None` for the headless coordinator.
    pub agent: Option<String>,
}

/// The unit name of a target. `crewboard-<initiative>-<agent dir>`, without the initiative when
/// the agent dir starts with `<initiative>-`. `old` is the doubled name that earlier versions
/// gave; units with that name keep working and are found again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub current: String,
    pub old: String,
}

pub fn names(initiative: &str, config_dir: &Path) -> Names {
    let base = config_dir
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default();
    let old = format!("crewboard-{}", safe_key(&format!("{initiative}-{base}")));
    let current = if base.starts_with(&format!("{initiative}-")) {
        format!("crewboard-{}", safe_key(&base))
    } else {
        old.clone()
    };
    Names { current, old }
}

/// The wrapper script of a unit: next to the agent config, or in the script dir of Hermes.
pub fn wrapper_path(host: &Host, t: &Target, sched: Scheduler, name: &str) -> PathBuf {
    if sched == Scheduler::Hermes {
        host.home
            .join(".hermes/scripts")
            .join(format!("{name}-wake.sh"))
    } else {
        t.config_dir.join(format!("{name}-wake.sh"))
    }
}

fn cron_file(t: &Target, name: &str) -> PathBuf {
    t.config_dir.join(format!("{name}.cron"))
}

/// The crontab of the user; empty when there is none.
pub(super) fn crontab(sys: &dyn Sys) -> String {
    let r = sys.run("crontab", &["-l".to_string()], None);
    if r.ok { r.stdout } else { String::new() }
}

fn in_crontab(sys: &dyn Sys, name: &str) -> bool {
    crontab(sys).contains(&cron_marker(name))
}

/// Is a unit of this scheduler with this name there? (For Hermes the wrapper is the unit.)
fn unit_there(host: &Host, t: &Target, sys: &dyn Sys, sched: Scheduler, name: &str) -> bool {
    let sd = host.systemd_dir();
    match sched {
        Scheduler::Systemd => {
            sys.exists(&sd.join(format!("{name}.service")))
                || sys.exists(&sd.join(format!("{name}.timer")))
        }
        Scheduler::Launchd => sys.exists(&host.plist(name)),
        Scheduler::Cron => sys.exists(&cron_file(t, name)) || in_crontab(sys, name),
        Scheduler::Hermes => sys.exists(&wrapper_path(host, t, sched, name)),
        Scheduler::Auto => false,
    }
}

/// Is a unit or a wrapper with this name there?
pub fn installed_as(host: &Host, t: &Target, sys: &dyn Sys, sched: Scheduler, name: &str) -> bool {
    sys.exists(&wrapper_path(host, t, sched, name)) || unit_there(host, t, sys, sched, name)
}

/// The names to look for: the current one and the doubled one of an older version.
fn all_names(n: &Names) -> Vec<String> {
    let mut v = vec![n.current.clone()];
    if n.old != n.current {
        v.push(n.old.clone());
    }
    v
}

// --- the plan of an install --------------------------------------------------------------

/// One command of the scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    /// A failure does not stop the run (a `bootout` of a unit that is not loaded).
    pub ignore_failure: bool,
}

impl Cmd {
    pub fn new(program: &str, args: &[&str]) -> Cmd {
        Cmd {
            program: program.to_string(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
            stdin: None,
            ignore_failure: false,
        }
    }

    pub fn tolerant(mut self) -> Cmd {
        self.ignore_failure = true;
        self
    }

    /// The command as a line for a person.
    pub fn line(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            s.push(' ');
            let plain = a
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._/+:@,=-".contains(&b));
            s.push_str(&if plain { a.clone() } else { sq(a) });
        }
        if self.stdin.is_some() {
            s.push_str("   (the new crontab on standard input)");
        }
        s
    }
}

/// A file to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutFile {
    pub path: PathBuf,
    pub content: String,
    pub mode: u32,
}

#[derive(Debug)]
pub struct InstallPlan {
    pub name: String,
    pub scheduler: Scheduler,
    pub secs: u64,
    pub files: Vec<OutFile>,
    pub mkdirs: Vec<PathBuf>,
    pub cmds: Vec<Cmd>,
    /// After the commands: is a systemd user linger needed for the timer to run without login?
    pub linger_check: bool,
    /// The one command that the user has to run.
    pub next: Option<String>,
    /// A line for standard error.
    pub note: Option<String>,
    pub log: PathBuf,
}

/// What an install is made from.
pub struct InstallIn<'a> {
    pub run: &'a WakeArgs,
    pub scheduler: Scheduler,
    pub secs: u64,
    pub target: &'a Target,
    /// `--env-file`, absolute.
    pub env_file: Option<&'a Path>,
    pub state_dir: &'a Path,
    pub log: &'a Path,
    /// The crewboard that the wrapper runs, absolute.
    pub crewboard: &'a Path,
    pub ts: &'a str,
}

fn is_dev(a: &WakeArgs) -> bool {
    !a.tick
}

/// `auto`: launchd on macOS; on Linux systemd --user when it works, else cron; Hermes for a
/// Hermes agent.
pub fn pick_scheduler(
    want: Scheduler,
    host: &Host,
    sys: &dyn Sys,
    runtime: Option<&str>,
) -> Scheduler {
    if want != Scheduler::Auto {
        return want;
    }
    if runtime == Some("hermes") {
        return Scheduler::Hermes;
    }
    match host.os {
        Os::Mac => Scheduler::Launchd,
        Os::Linux
            if sys
                .run(
                    "systemctl",
                    &["--user".to_string(), "show-environment".to_string()],
                    None,
                )
                .ok =>
        {
            Scheduler::Systemd
        }
        Os::Linux | Os::Other => Scheduler::Cron,
    }
}

/// The checks of a dev agent; the text of the first wrong option.
fn check_dev(a: &WakeArgs, t: &Target, sys: &dyn Sys) -> Result<String, String> {
    if t.agent.is_none() {
        return Err("--agent is required for a dev agent".into());
    }
    let j = &a.agent_job;
    match j.work_dir.as_deref() {
        Some(w) if sys.is_dir(&abs(w)) => {}
        Some(w) => {
            return Err(format!(
                "--work-dir must be an existing directory: {}",
                w.display()
            ));
        }
        None => return Err("--work-dir must be an existing directory: ".into()),
    }
    let runtime = j.runtime.clone().unwrap_or_default();
    if !["claude", "codex", "hermes"].contains(&runtime.as_str()) {
        return Err("--runtime must be claude, codex or hermes".into());
    }
    if let Some(e) = &j.events_every
        && (e.is_empty() || !e.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(format!("--events-every needs whole seconds, got '{e}'"));
    }
    if !j.allow.is_empty() && runtime != "claude" {
        return Err("--allow is for --runtime claude (Codex and Hermes have no tool list)".into());
    }
    Ok(runtime)
}

/// The words after `crewboard wake` in the wrapper: the options that the run needs, in a fixed
/// order, paths made absolute. A coordinator gets only the options of the tick.
fn wake_words(i: &InstallIn, runtime: Option<&str>, rt_bin: Option<&Path>) -> Vec<String> {
    let a = i.run;
    let j = &a.agent_job;
    let mut w: Vec<String> = Vec::new();
    let mut put = |flag: &str, v: Option<String>| {
        w.push(flag.to_string());
        if let Some(v) = v {
            w.push(v);
        }
    };
    let p = |x: &Path| Some(abs(x).to_string_lossy().into_owned());
    if a.tick {
        put("--tick", None);
    }
    if let Some(n) = &i.target.agent {
        put("--agent", Some(n.clone()));
    }
    put("--initiative", Some(i.target.initiative.clone()));
    if let Some(e) = i.env_file {
        put("--env-file", p(e));
    }
    put("--config-dir", p(&i.target.config_dir));
    if is_dev(a) {
        put("--runtime", runtime.map(str::to_string));
        put("--work-dir", j.work_dir.as_deref().and_then(p));
        if let Some(s) = &j.shim_dir {
            put("--shim-dir", p(s));
        }
        if let Some(f) = &j.runtime_flags {
            put("--runtime-flags", Some(f.clone()));
        }
        for t in &j.allow {
            put("--allow", Some(t.clone()));
        }
        if let Some(m) = &j.model {
            put("--model", Some(m.clone()));
        }
        if let Some(x) = &j.prompt_extra {
            put("--prompt-extra", Some(x.clone()));
        }
    }
    for l in &j.host_lock {
        put("--host-lock", p(l));
    }
    if is_dev(a) {
        if j.no_events_hook {
            put("--no-events-hook", None);
        } else if runtime == Some("claude")
            && let Some(e) = &j.events_every
        {
            put("--events-every", Some(e.clone()));
        }
        put("--runtime-bin", rt_bin.and_then(p));
    }
    if let Some(x) = &a.state_dir {
        put("--state-dir", p(x));
    }
    if let Some(x) = &a.lock_dir {
        put("--lock-dir", p(x));
    }
    if let Some(x) = &a.log {
        put("--log", p(x));
    }
    if let Some(x) = &a.events_file {
        put("--events-file", p(x));
    }
    for (flag, v) in [
        ("--work-backoff", a.work_backoff.clone()),
        ("--heartbeat-every", a.heartbeat_every.clone()),
        ("--stale-after", a.stale_after.map(|n| n.to_string())),
        ("--empty-grace", a.empty_grace.map(|n| n.to_string())),
        ("--max-wake", a.max_wake.clone()),
        ("--retry-delays", a.retry_delays.clone()),
    ] {
        if v.is_some() {
            put(flag, v);
        }
    }
    w
}

/// The runtime binary: `--runtime-bin`, else the runtime on the PATH of this shell.
fn runtime_bin(a: &WakeArgs, runtime: &str, host: &Host, sys: &dyn Sys) -> Result<PathBuf, String> {
    let path = job::with_install_dirs(&host.path, host.home.to_str(), &|d| sys.is_dir(d));
    let found = match &a.agent_job.runtime_bin {
        Some(b) => program(&b.to_string_lossy(), Some(&path)),
        None => program(runtime, Some(&path)),
    };
    if sys.is_executable(&found) {
        Ok(abs(&found))
    } else {
        Err(format!(
            "{runtime} is not on the PATH of this shell; install it, or pass --runtime-bin <absolute path>"
        ))
    }
}

/// The name to use: the current one, or the doubled one of an older install that is still there.
fn pick_name(host: &Host, t: &Target, sys: &dyn Sys, s: Scheduler) -> (String, Option<String>) {
    let n = names(&t.initiative, &t.config_dir);
    if n.old != n.current
        && !installed_as(host, t, sys, s, &n.current)
        && installed_as(host, t, sys, s, &n.old)
    {
        let note = format!(
            "note: keeping the installed name {}; to use {} run `crewboard wake uninstall` first, then install again",
            n.old, n.current
        );
        return (n.old, Some(note));
    }
    (n.current, None)
}

/// Make the plan of an install, or say what is wrong with the options (exit 4).
pub fn plan_install(host: &Host, sys: &dyn Sys, i: &InstallIn) -> Result<InstallPlan, String> {
    let a = i.run;
    if a.tick && a.agent_job.runtime.is_some() {
        return Err("--runtime and --tick exclude each other: a tick starts no model".into());
    }
    let runtime = if is_dev(a) {
        Some(check_dev(a, i.target, sys)?)
    } else {
        None
    };
    let sched = pick_scheduler(i.scheduler, host, sys, runtime.as_deref());
    if sched == Scheduler::Launchd && host.os != Os::Mac {
        return Err(
            "launchd is the scheduler of macOS; use --scheduler systemd or cron here".into(),
        );
    }
    if sched == Scheduler::Cron && host.os == Os::Mac && runtime.as_deref() == Some("claude") {
        return Err(
            "cron on macOS cannot read the keychain login of claude; use --scheduler launchd"
                .into(),
        );
    }
    let rt_bin = match &runtime {
        Some(r) => Some(runtime_bin(a, r, host, sys)?),
        None => None,
    };
    let (name, note) = pick_name(host, i.target, sys, sched);
    let w = wrapper_path(host, i.target, sched, &name);
    let path = job::with_install_dirs(&host.path, host.home.to_str(), &|d| sys.is_dir(d));
    let script = units::wrapper(
        sched.label(),
        i.ts,
        &path,
        &i.crewboard.to_string_lossy(),
        &wake_words(i, runtime.as_deref(), rt_bin.as_deref()),
    );
    let w_text = w.to_string_lossy().into_owned();
    let err_log = i.state_dir.join("scheduler.err");
    let err_text = err_log.to_string_lossy().into_owned();
    let uv = UnitVars {
        name: &name,
        agent: i.target.agent.as_deref(),
        initiative: &i.target.initiative,
        wrapper: &w_text,
        err_log: &err_text,
        secs: i.secs,
    };
    let mut plan = InstallPlan {
        name: name.clone(),
        scheduler: sched,
        secs: i.secs,
        files: vec![OutFile {
            path: w.clone(),
            content: script,
            mode: 0o700,
        }],
        mkdirs: vec![i.state_dir.to_path_buf()],
        cmds: Vec::new(),
        linger_check: false,
        next: None,
        note,
        log: i.log.to_path_buf(),
    };
    add_unit(&mut plan, host, sys, i.target, &uv);
    Ok(plan)
}

fn file(path: PathBuf, content: String) -> OutFile {
    OutFile {
        path,
        content,
        mode: 0o644,
    }
}

/// The unit files and the commands that enable them.
fn add_unit(plan: &mut InstallPlan, host: &Host, sys: &dyn Sys, t: &Target, uv: &UnitVars) {
    let name = uv.name;
    match plan.scheduler {
        Scheduler::Systemd => {
            let sd = host.systemd_dir();
            plan.files
                .push(file(sd.join(format!("{name}.service")), units::service(uv)));
            plan.files
                .push(file(sd.join(format!("{name}.timer")), units::timer(uv)));
            let timer = format!("{name}.timer");
            plan.cmds
                .push(Cmd::new("systemctl", &["--user", "daemon-reload"]));
            plan.cmds
                .push(Cmd::new("systemctl", &["--user", "enable", &timer]));
            plan.cmds
                .push(Cmd::new("systemctl", &["--user", "restart", &timer]));
            plan.linger_check = true;
        }
        Scheduler::Launchd => {
            let p = host.plist(name);
            plan.files.push(file(p.clone(), units::plist(uv)));
            let target = format!("gui/{}", host.uid);
            plan.cmds
                .push(Cmd::new("launchctl", &["bootout", &format!("{target}/{name}")]).tolerant());
            plan.cmds.push(Cmd::new(
                "launchctl",
                &["bootstrap", &target, &p.to_string_lossy()],
            ));
        }
        Scheduler::Cron => {
            let line = units::cron(uv);
            plan.files.push(file(cron_file(t, name), line.clone()));
            let marker = cron_marker(name);
            let mut text: String = crontab(sys)
                .lines()
                .filter(|l| !l.contains(&marker))
                .map(|l| format!("{l}\n"))
                .collect();
            text.push_str(&line);
            let mut c = Cmd::new("crontab", &["-"]);
            c.stdin = Some(text);
            plan.cmds.push(c);
        }
        Scheduler::Hermes => {
            let script = format!("{name}-wake.sh");
            plan.next = Some(units::hermes_create(name, &script, uv.secs));
        }
        Scheduler::Auto => {}
    }
}

// --- the plan of an uninstall -------------------------------------------------------------

/// What `uninstall` does for one unit.
#[derive(Debug)]
pub struct Removal {
    pub scheduler: Scheduler,
    pub name: String,
    pub before: Vec<Cmd>,
    pub remove: Vec<PathBuf>,
    pub after: Vec<Cmd>,
    /// A line for the user instead of a command (Hermes keeps its job list itself).
    pub hint: Option<String>,
    /// Files to copy `(from, to)` before they are removed.
    pub backups: Vec<(PathBuf, PathBuf)>,
    /// Files to write before the commands run (a copy of the crontab that is changed).
    pub saves: Vec<OutFile>,
}

fn removal(host: &Host, t: &Target, sys: &dyn Sys, s: Scheduler, name: &str) -> Removal {
    let sd = host.systemd_dir();
    let mut r = Removal {
        scheduler: s,
        name: name.to_string(),
        before: Vec::new(),
        remove: Vec::new(),
        after: Vec::new(),
        hint: None,
        backups: Vec::new(),
        saves: Vec::new(),
    };
    match s {
        Scheduler::Systemd => {
            r.before.push(
                Cmd::new(
                    "systemctl",
                    &["--user", "disable", "--now", &format!("{name}.timer")],
                )
                .tolerant(),
            );
            r.remove.push(sd.join(format!("{name}.service")));
            r.remove.push(sd.join(format!("{name}.timer")));
            r.after
                .push(Cmd::new("systemctl", &["--user", "daemon-reload"]).tolerant());
        }
        Scheduler::Launchd => {
            r.before.push(
                Cmd::new(
                    "launchctl",
                    &["bootout", &format!("gui/{}/{name}", host.uid)],
                )
                .tolerant(),
            );
            r.remove.push(host.plist(name));
        }
        Scheduler::Cron => {
            let list = sys.run("crontab", &["-l".to_string()], None);
            if list.ok {
                let marker = cron_marker(name);
                let text: String = list
                    .stdout
                    .lines()
                    .filter(|l| !l.contains(&marker))
                    .map(|l| format!("{l}\n"))
                    .collect();
                let mut c = Cmd::new("crontab", &["-"]);
                c.stdin = Some(text);
                r.before.push(c);
            }
            r.remove.push(cron_file(t, name));
        }
        Scheduler::Hermes => {
            r.hint = Some(format!(
                "remove the Hermes job too: hermes cron list; hermes cron remove <job-id of {name}>"
            ));
        }
        Scheduler::Auto => {}
    }
    r.remove.push(wrapper_path(host, t, s, name));
    r
}

/// The schedulers to look at: the one given, else all.
fn schedulers(which: Option<Scheduler>) -> Vec<Scheduler> {
    match which {
        Some(Scheduler::Auto) | None => vec![
            Scheduler::Systemd,
            Scheduler::Launchd,
            Scheduler::Cron,
            Scheduler::Hermes,
        ],
        Some(s) => vec![s],
    }
}

/// The removals of an uninstall. With a scheduler given, both names are removed for it. With
/// none, every scheduler that has a unit or a wrapper of this agent is cleaned.
pub fn plan_uninstall(
    host: &Host,
    sys: &dyn Sys,
    t: &Target,
    which: Option<Scheduler>,
) -> Vec<Removal> {
    let n = names(&t.initiative, &t.config_dir);
    let scheds = schedulers(which);
    let explicit = matches!(which, Some(s) if s != Scheduler::Auto);
    let mut out = Vec::new();
    for s in scheds {
        for name in all_names(&n) {
            if explicit || unit_there(host, t, sys, s, &name) {
                out.push(removal(host, t, sys, s, &name));
            }
        }
    }
    // a wrapper without a unit (a unit that was removed by hand) goes too
    if !explicit {
        for name in all_names(&n) {
            let w = wrapper_path(host, t, Scheduler::Systemd, &name);
            if sys.exists(&w) && !out.iter().any(|r| r.name == name && r.remove.contains(&w)) {
                out.push(Removal {
                    scheduler: Scheduler::Auto,
                    name: name.clone(),
                    before: Vec::new(),
                    remove: vec![w],
                    after: Vec::new(),
                    hint: None,
                    backups: Vec::new(),
                    saves: Vec::new(),
                });
            }
        }
    }
    out
}

/// The units that exist for an agent: `(scheduler, name)`.
pub fn installed(
    host: &Host,
    sys: &dyn Sys,
    t: &Target,
    which: Option<Scheduler>,
) -> Vec<(Scheduler, String)> {
    let n = names(&t.initiative, &t.config_dir);
    let mut out = Vec::new();
    for s in schedulers(which) {
        for name in all_names(&n) {
            if unit_there(host, t, sys, s, &name) {
                out.push((s, name));
            }
        }
    }
    out
}
