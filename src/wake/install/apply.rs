//! The side of `wake install|uninstall|status`: read the host and the agent config, make the
//! plan (`plan`), then write the files and run the commands, or print them with
//! `--dry-run`.

use super::foreign::{Foreign, find_foreign, replace_plan};
use super::plan::{
    Cmd, Host, InstallIn, InstallPlan, OutFile, Removal, Target, installed, plan_install,
    plan_uninstall,
};
use super::sys::{RealSys, Sys};
use super::units;
use super::{Flags, InstallArgs, Os, Scheduler, UnitArgs, WakeAction};
use crate::base::ids;
use crate::clock;
use crate::config::ConfigDir;
use crate::error::CbError;
use crate::fsutil::Mode;
use crate::wake::job::{abs, program, with_install_dirs};
use crate::wake::settings::{GlobalFlags, Plan, WakeArgs, resolve};
use std::path::{Path, PathBuf};

fn cfg_err(m: impl Into<String>) -> CbError {
    CbError::config(m)
}

pub fn run(action: &WakeAction, f: &Flags) -> Result<String, CbError> {
    let sys = RealSys;
    let host = real_host(&sys)?;
    match action {
        WakeAction::Install(a) => install(a, f, &host, &sys),
        WakeAction::Uninstall(a) => uninstall(a, f, &host, &sys),
        WakeAction::Status(a) => status(a, f, &host, &sys),
    }
}

/// The host of this process. `CREWBOARD_WAKE_OS` (`Darwin` or `Linux`) is a test seam.
fn real_host(sys: &dyn Sys) -> Result<Host, CbError> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let home = var("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| cfg_err("HOME is not set; the units go below it"))?;
    let os = match var("CREWBOARD_WAKE_OS").as_deref() {
        Some("Darwin") => Os::Mac,
        Some("Linux") => Os::Linux,
        Some(_) => Os::Other,
        None if cfg!(target_os = "macos") => Os::Mac,
        None if cfg!(target_os = "linux") => Os::Linux,
        None => Os::Other,
    };
    let uid = sys.run("id", &["-u".to_string()], None);
    Ok(Host {
        home,
        xdg_config: var("XDG_CONFIG_HOME").map(PathBuf::from),
        path: std::env::var("PATH").unwrap_or_default(),
        os,
        user: var("USER").or_else(|| var("LOGNAME")),
        uid: uid.stdout.trim().to_string(),
    })
}

/// Which agent: the config dir, the initiative and the agent name. What is not given comes from
/// the agent config of the dir.
fn target(
    agent_dir: Option<&Path>,
    f: &Flags,
    initiative: Option<&str>,
    agent: Option<&str>,
    need_agent: bool,
) -> Result<Target, CbError> {
    let dir = agent_dir
        .or(f.config_dir.as_deref())
        .filter(|d| !d.as_os_str().is_empty())
        .map(abs)
        .ok_or_else(|| {
            cfg_err("--agent-dir is required (the names of the unit come from the config dir)")
        })?;
    let initiative = initiative.filter(|i| !i.is_empty());
    let agent = agent.filter(|a| !a.is_empty());
    let (initiative, agent) = match (initiative, agent, need_agent) {
        (Some(i), a, false) => (i.to_string(), a.map(str::to_string)),
        (Some(i), Some(a), true) => (i.to_string(), Some(a.to_string())),
        (i, a, _) => {
            let conf = ConfigDir::resolve(Some(&dir))
                .and_then(|cd| cd.load_agent_auto(i))
                .map_err(|e| cfg_err(format!("{e}; or pass --initiative and --agent")))?;
            (
                i.map_or(conf.board.initiative.clone(), str::to_string),
                a.map(str::to_string).or(need_agent.then_some(conf.name)),
            )
        }
    };
    Ok(Target {
        config_dir: dir,
        initiative,
        agent,
    })
}

/// The state dir of the agent, as `crewboard wake --print-state-dir` says.
fn state_dir(run: &WakeArgs, t: &Target, f: &Flags) -> Result<PathBuf, CbError> {
    let mut a = run.clone();
    a.print_state_dir = true;
    a.agent = t.agent.clone();
    a.initiative = Some(t.initiative.clone());
    a.agent_dir = None;
    let g = GlobalFlags {
        config_dir: Some(t.config_dir.clone()),
        env_file: f.env_file.clone(),
        exe: None,
    };
    let env = |k: &str| std::env::var(k).ok();
    match resolve(&a, &g, &env, None) {
        Ok(Plan::PrintStateDir(d)) => Ok(d),
        Ok(_) => Err(cfg_err("cannot find the state dir")),
        Err(b) => Err(cfg_err(b.text)),
    }
}

/// The crewboard that the wrapper runs: `--crewboard`, `$CREWBOARD_BIN`, `crewboard` on the
/// PATH of this shell, else this program.
fn crewboard_bin(run: &WakeArgs, host: &Host, sys: &dyn Sys) -> Result<PathBuf, CbError> {
    let path = with_install_dirs(&host.path, host.home.to_str(), &|d| sys.is_dir(d));
    let want = run.crewboard.clone().filter(|b| !b.is_empty()).or_else(|| {
        std::env::var("CREWBOARD_BIN")
            .ok()
            .filter(|b| !b.is_empty())
    });
    let found = match want {
        Some(w) => program(&w, Some(&path)),
        None => {
            let on_path = program("crewboard", Some(&path));
            if sys.is_executable(&on_path) {
                on_path
            } else {
                std::env::current_exe().map_err(|e| {
                    cfg_err(format!(
                        "crewboard is not on the PATH of this shell and its own path is unknown ({e}); pass --crewboard <absolute path>"
                    ))
                })?
            }
        }
    };
    if sys.is_executable(&found) {
        Ok(abs(&found))
    } else {
        Err(cfg_err(format!(
            "crewboard not found: {}; pass --crewboard <absolute path>",
            found.display()
        )))
    }
}

fn install(a: &InstallArgs, f: &Flags, host: &Host, sys: &dyn Sys) -> Result<String, CbError> {
    let run = &a.run;
    if run.print_state_dir || run.agent_job.print_cmd || run.wake_cmd.is_some() {
        return Err(cfg_err(
            "--print-state-dir, --print-cmd and --wake-cmd are options of the wake, not of the install",
        ));
    }
    let secs = clock::parse_secs(&a.every)
        .filter(|s| *s > 0)
        .ok_or_else(|| {
            cfg_err(format!(
                "--every needs a duration such as 5m, got '{}'",
                a.every
            ))
        })?;
    let t = target(
        run.agent_dir.as_deref(),
        f,
        run.initiative.as_deref(),
        run.agent.as_deref(),
        !run.tick,
    )?;
    let state = state_dir(run, &t, f)?;
    let log = run
        .log
        .as_deref()
        .map_or_else(|| state.join("wake.log"), abs);
    let crewboard = crewboard_bin(run, host, sys)?;
    let env_file = f.env_file.as_deref().map(abs);
    let ts = clock::now_ts();
    let plan = plan_install(
        host,
        sys,
        &InstallIn {
            run,
            scheduler: a.scheduler,
            secs,
            target: &t,
            env_file: env_file.as_deref(),
            state_dir: &state,
            log: &log,
            crewboard: &crewboard,
            ts: &ts,
        },
    )
    .map_err(cfg_err)?;
    if let Some(n) = &plan.note {
        eprintln!("{n}");
    }
    let found = find_foreign(host, sys, &t);
    if !found.is_empty() && !a.replace {
        return Err(refuse_foreign(&found, &t));
    }
    let date = ts.get(..10).unwrap_or_default().replace('-', "");
    let gone: Vec<Removal> = found
        .iter()
        .map(|f| replace_plan(host, sys, &t, f, &date))
        .collect();
    if a.dry_run {
        let mut out = render_dry(&plan);
        for r in &gone {
            out.push_str(&render_removal(r));
        }
        return Ok(out);
    }
    for r in &gone {
        let text = apply_removal(r, sys)?;
        eprint!("{text}");
    }
    apply_install(plan, host, sys)
}

/// The refusal for jobs of other names: one line per job, then the fix.
fn refuse_foreign(found: &[Foreign], t: &Target) -> CbError {
    let mut m: Vec<String> = found.iter().map(Foreign::line).collect();
    m.push(format!(
        "fix: crewboard wake install --agent-dir {} ... --replace (removes those jobs; a copy of each file stays as <file>.bak-<date>)",
        t.config_dir.display()
    ));
    CbError::refused(ids::SECOND_WAKE_JOB, m.join("\n"))
}

/// `--dry-run` with `--replace`: what is unloaded, copied and removed.
fn render_removal(r: &Removal) -> String {
    let mut s = format!("--- would replace {} {}\n", r.scheduler.label(), r.name);
    for c in &r.before {
        s.push_str(&format!("{}\n", c.line()));
    }
    for (from, to) in &r.backups {
        s.push_str(&format!("cp {} {}\n", from.display(), to.display()));
    }
    for f in &r.saves {
        s.push_str(&format!("save the crontab to {}\n", f.path.display()));
    }
    for p in &r.remove {
        s.push_str(&format!("rm {}\n", p.display()));
    }
    for c in &r.after {
        s.push_str(&format!("{}\n", c.line()));
    }
    s
}

/// `--dry-run`: the files with their text, then the commands.
fn render_dry(p: &InstallPlan) -> String {
    let mut s = String::new();
    for f in &p.files {
        s.push_str(&format!(
            "--- {} (mode {:o})\n{}",
            f.path.display(),
            f.mode,
            f.content
        ));
        if !f.content.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push_str("--- would run\n");
    for c in &p.cmds {
        s.push_str(&format!("{}\n", c.line()));
    }
    if p.linger_check {
        s.push_str("loginctl show-user \"$USER\" --property=Linger\n");
        s.push_str("--- next, only if linger is off\nloginctl enable-linger \"$USER\"\n");
    }
    if let Some(n) = &p.next {
        s.push_str(&format!("--- next\n{n}\n"));
    }
    s
}

fn write_file(f: &OutFile) -> Result<(), CbError> {
    let cant = |e: std::io::Error| cfg_err(format!("cannot write {}: {e}", f.path.display()));
    if let Some(dir) = f.path.parent() {
        std::fs::create_dir_all(dir).map_err(cant)?;
    }
    crate::fsutil::write_atomic(&f.path, f.content.as_bytes(), Mode::Exact(f.mode)).map_err(cant)
}

/// Run one command; a failure that is not tolerated is an error with the text of the command.
fn run_cmd(sys: &dyn Sys, c: &Cmd) -> Result<(), CbError> {
    let r = sys.run(&c.program, &c.args, c.stdin.as_deref());
    if r.ok || c.ignore_failure {
        return Ok(());
    }
    Err(cfg_err(format!(
        "`{}` failed: {}",
        c.line(),
        r.stderr.trim()
    )))
}

fn linger_is_on(sys: &dyn Sys, host: &Host) -> bool {
    let Some(user) = &host.user else {
        return false;
    };
    let args = ["show-user", user, "--property=Linger"].map(str::to_string);
    let r = sys.run("loginctl", &args, None);
    r.ok && r.stdout.trim() == "Linger=yes"
}

fn apply_install(mut p: InstallPlan, host: &Host, sys: &dyn Sys) -> Result<String, CbError> {
    for d in &p.mkdirs {
        std::fs::create_dir_all(d)
            .map_err(|e| cfg_err(format!("cannot create {}: {e}", d.display())))?;
    }
    for f in &p.files {
        write_file(f)?;
    }
    for c in &p.cmds {
        run_cmd(sys, c)?;
    }
    if p.linger_check && !linger_is_on(sys, host) {
        p.next = Some("loginctl enable-linger \"$USER\"".to_string());
    }
    let mut out = format!(
        "installed {} ({}, every {}); log: {}\n",
        p.name,
        p.scheduler.label(),
        clock::fmt_age(i64::try_from(p.secs).unwrap_or(i64::MAX)),
        p.log.display()
    );
    if let Some(n) = &p.next {
        out.push_str(&format!("next: {n}\n"));
    }
    Ok(out)
}

// --- uninstall and status ------------------------------------------------------------------

fn unit_target(a: &UnitArgs, f: &Flags) -> Result<Target, CbError> {
    target(
        a.agent_dir.as_deref(),
        f,
        a.initiative.as_deref(),
        None,
        false,
    )
}

fn uninstall(a: &UnitArgs, f: &Flags, host: &Host, sys: &dyn Sys) -> Result<String, CbError> {
    let t = unit_target(a, f)?;
    let removals = plan_uninstall(host, sys, &t, a.scheduler);
    let mut out = String::new();
    if removals.is_empty() {
        out.push_str(&format!(
            "nothing installed for {} in {}\n",
            t.initiative,
            t.config_dir.display()
        ));
    }
    for r in &removals {
        out.push_str(&apply_removal(r, sys)?);
    }
    if !removals.is_empty()
        && let Ok(d) = state_dir(&WakeArgs::default(), &t, f)
    {
        out.push_str(&format!("kept: the state and the log in {}\n", d.display()));
    }
    Ok(out)
}

fn apply_removal(r: &Removal, sys: &dyn Sys) -> Result<String, CbError> {
    let mut out = String::new();
    for f in &r.saves {
        write_file(f)?;
    }
    for (from, to) in &r.backups {
        std::fs::copy(from, to).map_err(|e| {
            cfg_err(format!(
                "cannot copy {} to {}: {e}",
                from.display(),
                to.display()
            ))
        })?;
        out.push_str(&format!("backup: {}\n", to.display()));
    }
    for c in &r.before {
        run_cmd(sys, c)?;
    }
    for p in &r.remove {
        if sys.exists(p) {
            std::fs::remove_file(p)
                .map_err(|e| cfg_err(format!("cannot remove {}: {e}", p.display())))?;
            out.push_str(&format!("removed: {}\n", p.display()));
        }
    }
    for c in &r.after {
        run_cmd(sys, c)?;
    }
    if let Some(h) = &r.hint {
        out.push_str(&format!("{h}\n"));
    }
    Ok(out)
}

fn enabled_text(host: &Host, sys: &dyn Sys, s: Scheduler, name: &str) -> String {
    match s {
        Scheduler::Systemd => {
            let r = sys.run(
                "systemctl",
                &[
                    "--user".to_string(),
                    "is-enabled".to_string(),
                    format!("{name}.timer"),
                ],
                None,
            );
            let t = r.stdout.trim();
            if t.is_empty() {
                "not enabled".to_string()
            } else {
                t.to_string()
            }
        }
        Scheduler::Launchd => {
            let r = sys.run(
                "launchctl",
                &["print".to_string(), format!("gui/{}/{name}", host.uid)],
                None,
            );
            if r.ok { "loaded" } else { "not loaded" }.to_string()
        }
        Scheduler::Cron => {
            let r = sys.run("crontab", &["-l".to_string()], None);
            if r.ok && r.stdout.contains(&units::cron_marker(name)) {
                "in the crontab"
            } else {
                "not in the crontab"
            }
            .to_string()
        }
        Scheduler::Hermes | Scheduler::Auto => "see: hermes cron list".to_string(),
    }
}

/// The time of the last write of the log of the agent, as a timestamp.
fn last_run(dir: &Path) -> Option<String> {
    let m = std::fs::metadata(dir.join("wake.log"))
        .ok()?
        .modified()
        .ok()?;
    let secs = m.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(clock::format_ts(i64::try_from(secs).ok()?))
}

fn status(a: &UnitArgs, f: &Flags, host: &Host, sys: &dyn Sys) -> Result<String, CbError> {
    let t = unit_target(a, f)?;
    let mut out = String::new();
    let found = installed(host, sys, &t, a.scheduler);
    if found.is_empty() {
        out.push_str(&format!(
            "no unit for {} in {}\n",
            t.initiative,
            t.config_dir.display()
        ));
    }
    for (s, name) in &found {
        out.push_str(&format!(
            "{name} ({}): installed, {}\n",
            s.label(),
            enabled_text(host, sys, *s, name)
        ));
    }
    for j in find_foreign(host, sys, &t) {
        out.push_str(&format!("{}\n", j.line()));
    }
    if let Ok(d) = state_dir(&WakeArgs::default(), &t, f) {
        let when = last_run(&d).unwrap_or_else(|| "never".to_string());
        out.push_str(&format!("last run: {when} (state: {})\n", d.display()));
    }
    Ok(out)
}
