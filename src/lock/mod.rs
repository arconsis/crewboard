//! The host lock: a directory with an owner file that a dead agent cannot hold
//! forever. Agents on a shared host take one lock before a heavy job; if the owner dies, the next
//! taker sees that the lock is STALE and takes it over. The CLI is `crewboard lock`;
//!
//! Layout on disk (the public contract for CI steps, docs/guides/ci-and-host-lock.md):
//! `<dir>`, `<dir>/owner`, `<dir>.takeover`, `<dir>.waiting/<name>`, `<dir>.stale.*`, `<dir>.rel.*`.
//!
//! The logic imports nothing from the rest of the crate but `clock`. The board check is a trait
//! (`board::BoardProbe`); `ops::lock_cmd` builds the real one. Time and processes sit behind
//! `Clock` and `procs::Procs`, so the rules run in a test without a sleep.

pub mod args;
pub mod board;
pub mod ci;
#[cfg(unix)]
pub mod detach;
mod exit;
pub mod fsmeta;
pub mod help;
pub mod judge;
pub mod log;
pub mod owner;
pub mod procs;
pub mod result;
pub mod run;
pub mod snapshot;
#[cfg(unix)]
pub mod sys;
pub mod takeover;

use crate::base::ignore::best_effort_cleanup;
use crate::clock::{Clock, format_ts};
use board::BoardCheck;
pub use exit::LockExit;
use judge::{Judgement, Rules};
use log::Log;
use owner::{OwnerFile, sibling};
use procs::{Alive, Procs};
use snapshot::Snapshot;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The grant that `run` gives to its child (re-entrancy).
#[derive(Debug, Clone, Default)]
pub struct Grant {
    /// `$CREWBOARD_HOST_LOCK`
    pub dir: Option<String>,
    /// `$CREWBOARD_HOST_LOCK_PID`
    pub pid: Option<String>,
}

/// The options that the lock logic needs.
#[derive(Debug, Clone)]
pub struct Opts {
    pub owner: String,
    pub reason: String,
    pub wait: u64,
    pub poll: Duration,
    pub expect: Option<u64>,
    pub max_age: u64,
    pub empty_grace: u64,
    pub ci_max_age: u64,
    pub ci_mode: bool,
    pub gate: bool,
}

pub struct Session<'a> {
    pub dir: PathBuf,
    pub host: String,
    pub opts: Opts,
    pub clock: &'a dyn Clock,
    pub procs: &'a dyn Procs,
    pub log: &'a Log<'a>,
    pub board: BoardCheck<'a>,
    pub grant: Grant,
    /// This process got the lock by re-entry: it must not release it.
    pub nested: bool,
    /// The holder pid that `run` reports to its child.
    pub lock_pid: Option<String>,
    /// The owner file of this process is a detached job.
    pub detached: bool,
}

/// The physical path (symlinks resolved), also when the last part does not exist yet.
pub fn phys_dir(p: &str) -> String {
    let path = Path::new(p);
    if path.is_dir()
        && let Ok(c) = std::fs::canonicalize(path)
    {
        return c.to_string_lossy().to_string();
    }
    let parent = match path.parent() {
        Some(d) if d.as_os_str().is_empty() => Path::new("."),
        Some(d) => d,
        None => return p.to_string(),
    };
    let (Some(name), Ok(c)) = (path.file_name(), std::fs::canonicalize(parent)) else {
        return p.to_string();
    };
    c.join(name).to_string_lossy().to_string()
}

impl<'a> Session<'a> {
    pub fn rules(&self) -> Rules {
        Rules {
            host: self.host.clone(),
            max_age: self.opts.max_age,
            empty_grace: self.opts.empty_grace,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot::read(&self.dir)
    }

    /// Judge the lock as it is on disk now.
    pub fn judge(&mut self, snap: &Snapshot) -> Judgement {
        let rules = self.rules();
        let now = self.clock.now();
        judge::judge(snap, &rules, now, self.procs, &mut self.board)
    }

    pub fn describe(&self) -> String {
        judge::describe_owner(&self.snapshot())
    }

    fn notes(&self, snap: &Snapshot) -> String {
        judge::held_notes(snap, self.clock.now())
    }

    pub fn write_owner(&self, pid: &str) -> std::io::Result<()> {
        let now = self.clock.now();
        let o = OwnerFile {
            agent: self.opts.owner.clone(),
            host: self.host.clone(),
            pid: pid.to_string(),
            started: format_ts(now),
            started_epoch: now,
            reason: self.opts.reason.clone(),
            expect: self.opts.expect,
            detached: self.detached,
        };
        owner::write_owner(&self.dir, &o)
    }

    /// Make the lock dir, or take a stale one over, and write the owner file. `Ok(true)` = got it,
    /// `Ok(false)` = held. `Err` is a line for the log: the dir cannot be made, or the owner file
    /// cannot be written (the empty dir is removed again, so it blocks nobody).
    pub fn try_take(&mut self, pid: &str) -> Result<bool, String> {
        if let Some(parent) = self.dir.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok(); // a missing dir shows as an error at the next write
        }
        match std::fs::create_dir(&self.dir) {
            Ok(()) => return self.owner_or_undo(pid).map(|()| true),
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => {
                return Err(format!("cannot make {}: {e}", self.dir_text()));
            }
            Err(_) => {}
        }
        // held. Is it stale?
        let snap = self.snapshot();
        if snap.exists
            && matches!(self.judge(&snap), Judgement::Stale { .. })
            && self.take_over()
            && std::fs::create_dir(&self.dir).is_ok()
        {
            return self.owner_or_undo(pid).map(|()| true);
        }
        Ok(false)
    }

    fn owner_or_undo(&self, pid: &str) -> Result<(), String> {
        self.write_owner(pid).map_err(|e| {
            best_effort_cleanup(std::fs::remove_dir_all(&self.dir));
            format!(
                "cannot write the owner file in {}: {e}; lock not taken",
                self.dir_text()
            )
        })
    }

    /// Is the lock held on this host by a live ancestor of this process? The pid if so.
    pub fn ancestor_holder(&self) -> Option<u32> {
        let snap = self.snapshot();
        if !snap.owner_present() || snap.owner_field("host") != Some(self.host.as_str()) {
            return None;
        }
        let pid = snap.owner_field("pid")?;
        if pid.is_empty() || pid == "0" || !pid.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let pid: u32 = pid.parse().ok()?;
        if pid == self.procs.pid() || self.procs.alive(pid) != Alive::Yes || !self.is_ancestor(pid)
        {
            return None;
        }
        Some(pid)
    }

    /// Is `target` a proper ancestor of this process? The walk starts at the parent.
    fn is_ancestor(&self, target: u32) -> bool {
        let mut p = self.procs.ppid();
        for _ in 0..64 {
            if p <= 1 {
                return false;
            }
            if p == target {
                return true;
            }
            match self.procs.lookup(p).and_then(|i| i.ppid) {
                Some(next) => p = next,
                None => return false,
            }
        }
        false
    }

    /// The environment says: this dir is held by `held` and my caller told me.
    pub fn granted(&self, held: u32) -> bool {
        let (Some(dir), Some(pid)) = (&self.grant.dir, &self.grant.pid) else {
            return false;
        };
        !dir.is_empty()
            && *pid == held.to_string()
            && phys_dir(dir) == phys_dir(&self.dir.to_string_lossy())
    }

    fn dir_text(&self) -> String {
        self.dir.display().to_string()
    }

    /// `take`: the loop of `do_take`. 0 got the lock, 75 held by a live owner, 76 CI runs or waits,
    /// 77 held by your own ancestor without a grant.
    pub fn take(&mut self, pid: &str) -> i32 {
        let d = self.dir_text();
        if self.dir.is_dir()
            && let Some(held) = self.ancestor_holder()
        {
            if self.granted(held) {
                self.nested = true;
                self.lock_pid = Some(held.to_string());
                self.log.say(&format!(
                        "re-entrant: {d} is already held by your parent process {held} ({}); not taking it again",
                        self.describe()
                    ));
                return LockExit::Ok.code();
            }
            self.log.say(&format!(
                    "held by your own parent process {held} ({}); not re-entered: {d} is not granted to this process (CREWBOARD_HOST_LOCK/CREWBOARD_HOST_LOCK_PID missing or different). Do not wrap a tool that takes this lock itself in run; let it honor CREWBOARD_HOST_LOCK",
                    self.describe()
                ));
            return LockExit::HeldByAncestor.code();
        }
        let start = self.clock.now();
        if self.opts.ci_mode
            && let Err(e) = self.ci_register(Some(pid))
        {
            // the wait goes on, but CI sees no waiter in the list
            self.log.say(&format!("cannot register as a waiter: {e}"));
        }
        let mut shown = false;
        loop {
            // a caller that handles signals (the gate) stops at once, before it takes the lock
            #[cfg(unix)]
            if sys::stop_requested().is_some() {
                if self.opts.ci_mode {
                    ci::unregister(&self.dir, &self.opts.owner);
                }
                return LockExit::Stopped.code();
            }
            // CI first: an agent neither takes a free lock nor takes a stale one over while CI
            // runs or waits (a --gate job skips this wait: a held lock still blocks it)
            let ci = if !self.opts.ci_mode && !self.opts.gate {
                self.ci_seen()
            } else {
                None
            };
            let rc = if ci.is_some() {
                LockExit::CiActive.code()
            } else {
                match self.try_take(pid) {
                    Ok(true) => {
                        if self.opts.ci_mode {
                            ci::unregister(&self.dir, &self.opts.owner);
                        }
                        self.lock_pid = Some(pid.to_string());
                        return LockExit::Ok.code();
                    }
                    Ok(false) => LockExit::Held.code(),
                    Err(line) => {
                        if self.opts.ci_mode {
                            ci::unregister(&self.dir, &self.opts.owner);
                        }
                        self.log.say(&line);
                        return LockExit::No.code();
                    }
                }
            };
            let now = self.clock.now();
            if now - start >= judge::secs(self.opts.wait) {
                if self.opts.ci_mode {
                    ci::unregister(&self.dir, &self.opts.owner);
                }
                match &ci {
                    Some(c) => self.log.say(&format!("CI first: {c}; not taking {d}")),
                    None => {
                        let snap = self.snapshot();
                        self.log.say(&format!(
                            "lock {d} is held by {}{}",
                            judge::describe_owner(&snap),
                            self.notes(&snap)
                        ));
                    }
                }
                return rc;
            }
            if !shown {
                match &ci {
                    Some(c) => self.log.say(&format!("CI first: {c}; waiting")),
                    None => {
                        let snap = self.snapshot();
                        self.log.say(&format!(
                            "waiting for lock {d} held by {}{}",
                            judge::describe_owner(&snap),
                            self.notes(&snap)
                        ));
                    }
                }
                shown = true;
            }
            self.clock.sleep(self.opts.poll);
        }
    }

    pub fn ci_seen(&self) -> Option<String> {
        ci::present(
            &self.dir,
            &self.host,
            self.opts.ci_max_age,
            self.clock.now(),
            self.procs,
        )
    }

    pub fn ci_register(&self, pid: Option<&str>) -> std::io::Result<()> {
        ci::register(
            &self.dir,
            &self.opts.owner,
            &self.host,
            &format_ts(self.clock.now()),
            pid,
            self.procs.pid(),
        )
    }

    /// Is the lock held by this agent on this host, with a valid owner file? Never an invalid
    /// owner file.
    pub(super) fn owns(&self, snap: &Snapshot) -> bool {
        snap.is_valid()
            && snap.owner_field("agent") == Some(self.opts.owner.as_str())
            && snap.owner_field("host") == Some(self.host.as_str())
    }

    /// `release`: 0 released or nothing to release (also a nested release), 3 not the owner.
    /// It is atomic: the dir is renamed to a unique name first and removed after, so
    /// no other process sees an empty lock dir.
    pub fn release(&mut self) -> i32 {
        let d = self.dir_text();
        if !self.dir.is_dir() {
            self.log
                .say(&format!("nothing to release: {d} does not exist"));
            return LockExit::Ok.code();
        }
        if self.nested {
            return LockExit::Ok.code();
        }
        if let Some(held) = self.ancestor_holder()
            && self.granted(held)
        {
            self.log.say(&format!(
                "nested release: {d} belongs to your parent process {held}; the outer lock stays"
            ));
            return LockExit::Ok.code();
        }
        let snap = self.snapshot();
        if !self.owns(&snap) {
            self.log.say(&format!(
                "refused: {d} is held by {}, not by {}@{}",
                judge::describe_owner(&snap),
                self.opts.owner,
                self.host
            ));
            return LockExit::NotOwner.code();
        }
        let gone = sibling(
            &self.dir,
            &format!(
                ".rel.{}.{}.{}",
                self.procs.pid(),
                self.clock.now(),
                random_suffix()
            ),
        );
        if std::fs::rename(&self.dir, &gone).is_err() {
            self.log.say(&format!("cannot remove {d}"));
            return LockExit::NotOwner.code();
        }
        // Another taker may have replaced the lock between the check and the rename: look into the
        // renamed dir, and put it back if it is not ours.
        let moved = Snapshot::read(&gone);
        if !self.owns(&moved) {
            if std::fs::rename(&gone, &self.dir).is_err() {
                self.log
                    .say(&format!("cannot put back {d}: it is at {}", gone.display()));
            }
            self.log.say(&format!(
                "refused: {d} is held by {}, not by {}@{}",
                judge::describe_owner(&moved),
                self.opts.owner,
                self.host
            ));
            return LockExit::NotOwner.code();
        }
        best_effort_cleanup(std::fs::remove_dir_all(&gone));
        LockExit::Ok.code()
    }

    /// `status`: 0 held by a live owner, 1 free, 2 stale. It prints the answer.
    pub fn status(&mut self) -> i32 {
        if let Some(ci) = self.ci_seen() {
            self.log.out(&format!("ci: {ci}"));
        }
        let snap = self.snapshot();
        if !snap.exists {
            self.log.out("free");
            return LockExit::No.code();
        }
        if let Some(why) = self.judge(&snap).why() {
            self.log.out(&format!(
                "stale: {why}; owner {}",
                judge::describe_owner(&snap)
            ));
            return LockExit::Stale.code();
        }
        let now = self.clock.now();
        self.log.out(&format!(
            "held: {}{}{}",
            judge::describe_owner(&snap),
            judge::held_notes(&snap, now),
            judge::held_facts(&snap, &self.host, now, self.procs)
        ));
        LockExit::Ok.code()
    }

    /// `break --if-dead`: 0 free, or stale and removed; 3 held by a live owner (lock untouched).
    pub fn break_if_dead(&mut self) -> i32 {
        let snap = self.snapshot();
        if !snap.exists {
            self.log.out("free");
            return LockExit::Ok.code();
        }
        if matches!(self.judge(&snap), Judgement::Stale { .. }) && self.take_over() {
            return LockExit::Ok.code();
        }
        let snap = self.snapshot();
        if !snap.exists {
            self.log.out("free");
            return LockExit::Ok.code();
        }
        let now = self.clock.now();
        self.log.out(&format!(
            "refused: held: {}{}{}",
            judge::describe_owner(&snap),
            judge::held_notes(&snap, now),
            judge::held_facts(&snap, &self.host, now, self.procs)
        ));
        LockExit::NotOwner.code()
    }

    /// `ci-wait`: put this CI step on the waiting list.
    pub fn ci_wait(&self, pid: Option<&str>) -> i32 {
        let w = ci::waiting_dir(&self.dir);
        if self.ci_register(pid).is_err() {
            self.log
                .say(&format!("cannot write the waiting list {}", w.display()));
            return LockExit::No.code();
        }
        self.log.say(&format!(
            "registered CI entry {} in {}",
            self.opts.owner,
            w.display()
        ));
        LockExit::Ok.code()
    }

    pub fn ci_done(&self) -> i32 {
        ci::unregister(&self.dir, &self.opts.owner);
        LockExit::Ok.code()
    }

    /// `ci-status`: 0 and a line if CI runs or waits, else 1.
    pub fn ci_status(&self) -> i32 {
        match self.ci_seen() {
            Some(c) => {
                self.log.out(&c);
                LockExit::Ok.code()
            }
            None => LockExit::No.code(),
        }
    }
}

/// A number that differs between two releases in the same second.
fn random_suffix() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let nanos = crate::clock::now_nanos();
    nanos.wrapping_add(N.fetch_add(1, Ordering::Relaxed)) % 100_000
}
