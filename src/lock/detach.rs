//! `run --detach --result FILE`: the caller takes the lock with its own pid, removes
//! the old result, and starts a supervisor: this binary again, with the same arguments, in its own
//! session (`setsid`), with `HOST_LOCK_SUPERVISE=<caller pid>`. The supervisor writes its own pid and
//! `detached=1` in the owner file (so the stale rules judge the job), runs the command in its own
//! process group, writes the result file and releases the lock. The caller waits (at most 15 s)
//! for that owner file and returns. A kill of the caller (the time budget of a wake) reaches
//! neither the job nor its lock.

use super::args::{Parsed, RunCtx};
use super::owner::sibling;
use super::result::ResultFile;
use super::{Session, phys_dir, sys};
use crate::base::ignore::{best_effort_cleanup, best_effort_output, child_may_have_ended};
use crate::clock::format_ts;
use crate::fsutil::Mode;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long the caller waits for the supervisor: 150 tries of 0.1 s.
const TRIES: u32 = 150;

fn result_path(p: &Parsed) -> String {
    phys_dir(p.result.as_deref().unwrap_or(""))
}

fn log_path(result: &str) -> PathBuf {
    sibling(Path::new(result), ".log")
}

/// The caller side. The lock is taken with the pid of this process already.
pub fn start(s: &mut Session, p: &Parsed, ctx: &RunCtx) -> i32 {
    let raw = p.result.as_deref().unwrap_or("");
    if let Some(parent) = Path::new(raw).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok(); // a missing dir shows as an error at the next write
    }
    let result = result_path(p);
    let rpath = PathBuf::from(&result);
    remove_old_results(&rpath);
    let log = log_path(&result);
    let Ok(out) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
    else {
        s.log.say(&format!("cannot write {}", log.display()));
        let _ = s.release();
        return super::LockExit::No.code();
    };
    let err = out.try_clone();
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("crewboard"));
    let mut cmd = Command::new(exe);
    cmd.args(&ctx.orig_args)
        .env("HOST_LOCK_SUPERVISE", s.procs.pid().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(err.map_or_else(|_| Stdio::null(), Stdio::from));
    sys::new_session(&mut cmd);
    let mut child = match crate::spawn::spawn_retrying(&mut cmd) {
        Ok(c) => c,
        Err(e) => {
            s.log.say(&format!("the detached job did not start ({e})"));
            let _ = s.release();
            return super::LockExit::No.code();
        }
    };
    let launcher = child.id();
    let own = s.procs.pid().to_string();
    for _ in 0..TRIES {
        let snap = s.snapshot();
        let jp = snap.owner_field("pid").unwrap_or("").to_string();
        if rpath.exists() {
            // a job that ended before we looked
            s.log.say("detached job ended at once");
            s.log.out(&format!(
                "detached: pid={launcher} log={} result={result}",
                log.display()
            ));
            child_may_have_ended(child.try_wait());
            return 0;
        }
        if snap.owner_field("detached") == Some("1") && !jp.is_empty() && jp != own {
            s.log.say(&format!("detached job started: pid={jp}"));
            s.log.out(&format!(
                "detached: pid={jp} log={} result={result}",
                log.display()
            ));
            return 0;
        }
        s.clock.sleep(Duration::from_millis(100));
    }
    s.log.say(&format!(
        "the detached job did not start (see {})",
        log.display()
    ));
    // TERM, a short grace, then KILL: a slow supervisor must not hold this call
    sys::signal_pid(launcher, libc::SIGTERM);
    let mut ended = false;
    for _ in 0..20 {
        if child.try_wait().is_ok_and(|st| st.is_some()) {
            ended = true;
            break;
        }
        s.clock.sleep(Duration::from_millis(100));
    }
    if !ended {
        child_may_have_ended(child.kill());
        child_may_have_ended(child.wait());
    }
    let _ = s.release();
    1
}

/// `F` and every `F.tmp.*` (the temp files of an earlier supervisor).
fn remove_old_results(rpath: &Path) {
    best_effort_cleanup(std::fs::remove_file(rpath));
    let (Some(dir), Some(name)) = (rpath.parent(), rpath.file_name()) else {
        return;
    };
    let prefix = format!("{}.tmp.", name.to_string_lossy());
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(Result::ok) {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                best_effort_cleanup(std::fs::remove_file(e.path()));
            }
        }
    }
}

/// The supervisor side: it runs the command, writes the result and releases the lock.
pub fn supervise(s: &mut Session, p: &Parsed, ctx: &RunCtx, parent: &str) -> i32 {
    sys::catch_term_int_hup();
    sys::ignore_hup();
    let result = result_path(p);
    let snap = s.snapshot();
    if snap.owner_field("pid") != Some(parent) || !s.owns(&snap) {
        s.log.say(&format!(
            "the lock {} is not held for the detached job; not starting it",
            s.dir.display()
        ));
        return super::LockExit::No.code();
    }
    s.detached = true;
    let own = s.procs.pid();
    if s.write_owner(&own.to_string()).is_err() {
        return super::LockExit::No.code();
    }
    let mut cmd = Command::new(&p.run_cmd[0]);
    cmd.args(&p.run_cmd[1..])
        .env("CREWBOARD_HOST_LOCK", phys_dir(&s.dir.to_string_lossy()))
        .env("CREWBOARD_HOST_LOCK_PID", own.to_string())
        .env_remove("HOST_LOCK_SUPERVISE")
        .stdin(Stdio::null());
    for v in &ctx.env_remove {
        cmd.env_remove(v);
    }
    // its own process group: a TERM of the supervisor goes to the whole group of the job
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let rc = match crate::spawn::spawn_retrying(&mut cmd) {
        Err(e) => {
            best_effort_output(writeln!(std::io::stderr(), "{}: {}", p.run_cmd[0], e));
            if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            }
        }
        Ok(mut child) => loop {
            match child.try_wait() {
                Ok(Some(st)) => break crate::spawn::code_of(st),
                Ok(None) => {}
                Err(_) => break 1,
            }
            if let Some(sig) = sys::caught()
                && sig != libc::SIGHUP
            {
                sys::signal_group(child.id(), sig);
            }
            s.clock.sleep(Duration::from_millis(50));
        },
    };
    let text = ResultFile {
        rc: u64::try_from(rc).unwrap_or(1),
        end: format_ts(s.clock.now()),
        log: log_path(&result).display().to_string(),
        ticket: (!p.ticket.is_empty()).then(|| p.ticket.clone()),
        sha: (!p.sha.is_empty()).then(|| p.sha.clone()),
    }
    .render();
    if crate::fsutil::write_atomic(Path::new(&result), text.as_bytes(), Mode::Default).is_err() {
        s.log.say(&format!("cannot write the result file {result}"));
    }
    if s.release() != 0 {
        s.log.say(&format!(
            "the lock {} was not released after the detached job",
            s.dir.display()
        ));
    }
    0
}
