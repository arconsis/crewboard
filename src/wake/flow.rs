//! One run of `crewboard wake` as a sequence of phases. [`Flow`] holds what the phases share (the
//! settings, the calls to the board, the clock, the log, the signal) and the small services that
//! more than one phase needs: a call with retries, the net back-off, the exit 4 stop. The phases
//! are in their own modules; each returns an enum that says how the run goes on:
//!
//! 1. `preflight`: an older version, the stop file, the net back-off. `Done(code)` or go on.
//! 2. `precheck`: the cheap poll. `Wake(reason)`, `Quiet` or `End(code)`.
//! 3. the lock (here): `Held` or `End(code)`.
//! 4. under the lock: `tick`, or `wake_events` (the events file and the receipts) and
//!    `finish` (the wake command through `runtime`, then its results and the back-off).

use super::{WakeExit, precheck, preflight, wake_events};
use crate::clock::{Clock, format_ts};
use crate::fsutil::{self, Mode};
use crate::lock::procs::Procs;
use crate::wake::lockdir::{self, Acquired, Held, Rules};
use crate::wake::policy::{self, NetBackoff, Quiet};
use crate::wake::state::{self, Log};
use crate::wake::steps::{StepRc, Steps};
use std::cell::Cell;
use std::path::Path;
use std::time::Duration;

use super::settings::Settings;
use crate::base::ignore::best_effort_cleanup;

/// Why a run goes on to the wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Events,
    Work,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Events => "events",
            Reason::Work => "work",
        }
    }
}

/// One run.
pub struct Flow<'a, 'l> {
    pub s: &'a Settings,
    pub steps: &'a mut dyn Steps,
    pub clock: &'a dyn Clock,
    pub pids: &'a dyn Procs,
    pub log: &'a Log<'l>,
    /// The short host name, for the owner text of the lock.
    pub host: &'a str,
    /// The signal that stopped the run, once caught.
    pub sig: Cell<Option<i32>>,
    /// Where a caught signal comes from (the handler of the process; a test gives its own).
    pub signals: &'a dyn Fn() -> Option<i32>,
    /// Makes the board calls of the heartbeat thread, which has its own; none: no heartbeat.
    pub heartbeat_steps: Option<&'a (dyn Fn() -> Box<dyn Steps> + Sync)>,
}

/// The wake lock phase.
enum Lock {
    Held(Held),
    /// Another wake runs, or the lock cannot be taken: the run ends with this code.
    End(u8),
}

impl Flow<'_, '_> {
    /// Run the flow. The result is the exit code.
    pub fn execute(&mut self) -> u8 {
        if let preflight::Preflight::Done(code) = self.preflight() {
            return code;
        }
        let mut reason = Reason::Events;
        if !self.s.tick {
            match self.precheck() {
                precheck::Precheck::Wake(r) => reason = r,
                precheck::Precheck::Quiet => return 0,
                precheck::Precheck::End(code) => return code,
            }
        }
        if let Some(code) = self.signalled() {
            return code;
        }
        let held = match self.take_lock() {
            Lock::Held(h) => h,
            Lock::End(code) => return code,
        };
        let code = self.under_lock(reason);
        drop(held);
        code
    }

    /// What the run does while it holds the lock.
    fn under_lock(&mut self, reason: Reason) -> u8 {
        if let Some(code) = self.signalled() {
            return code;
        }
        if self.s.tick {
            return self.tick();
        }
        match self.prepare_events() {
            wake_events::Prepared::Ready => self.wake(reason),
            wake_events::Prepared::End(code) => code,
        }
    }

    /// Take the wake lock.
    fn take_lock(&mut self) -> Lock {
        let owner = format!(
            "poll-and-wake@{}{}",
            self.host,
            self.s
                .agent
                .as_deref()
                .map(|a| format!(" {a}"))
                .unwrap_or_default()
        );
        let rules = Rules {
            stale_after: self.s.stale_after,
            empty_grace: self.s.empty_grace,
        };
        let log = self.log;
        match lockdir::acquire(
            &self.s.paths.lock,
            &owner,
            &rules,
            self.clock,
            self.pids,
            &mut |l| log.say(l),
        ) {
            Acquired::Held(h) => Lock::Held(h),
            Acquired::Busy { pid, started } => {
                self.log.say(&format!(
                    "a wake already runs (pid {pid}, started {started}); not starting another"
                ));
                Lock::End(WakeExit::Ok.code())
            }
            Acquired::Cannot => {
                self.log.say(&format!(
                    "cannot take the lock {}",
                    self.s.paths.lock.display()
                ));
                Lock::End(WakeExit::LockUnavailable.code())
            }
        }
    }

    /// The exit code of a run that TERM, INT or HUP stopped (128 plus the signal), once one came.
    pub(super) fn signalled(&self) -> Option<u8> {
        if self.sig.get().is_none() {
            let sig = (self.signals)()?;
            self.sig.set(Some(sig));
            self.log.say(&format!("stopped by signal {sig}"));
        }
        Some(u8::try_from(128 + self.sig.get()?).unwrap_or(255))
    }

    /// Write a state file; a failure is one line in the log.
    pub(super) fn put(&self, path: &Path, data: &[u8]) {
        if let Err(e) = fsutil::write_atomic(path, data, Mode::Default) {
            self.log
                .line(&format!("cannot write {}: {e}", path.display()));
        }
    }

    /// Count one quiet poll; one line per hour goes to the log.
    pub(super) fn note_quiet(&self) {
        let now = self.clock.now();
        let prev = state::read_text(&self.s.paths.quiet).and_then(|t| Quiet::parse(&t));
        let (q, line) = policy::quiet_note(prev, now, &format_ts(now));
        if let Some(l) = line {
            self.log.line(&l);
        }
        self.put(&self.s.paths.quiet, q.render().as_bytes());
    }

    // --- calls ---

    /// One call to the board. Exit 3 is retried after each delay of `--retry-delays`.
    pub(super) fn call(
        &mut self,
        name: &str,
        f: &mut dyn FnMut(&mut dyn Steps, &mut Vec<u8>) -> StepRc,
    ) -> (StepRc, Vec<u8>) {
        let mut out = Vec::new();
        let mut rc = f(&mut *self.steps, &mut out);
        for (i, d) in self.s.retry_delays.iter().enumerate() {
            if rc.code != 3 || self.signalled().is_some() {
                break;
            }
            self.log.line(&format!(
                "crewboard {name}: exit 3 (try {}): {}; retry in {d} s",
                i + 1,
                rc.msg
            ));
            self.clock.sleep(Duration::from_secs(*d));
            out.clear();
            rc = f(&mut *self.steps, &mut out);
        }
        (rc, out)
    }

    pub(super) fn net_ok(&self) {
        best_effort_cleanup(std::fs::remove_file(&self.s.paths.net));
    }

    /// Exit 3 after the retries: count the run; from the second run in a row skip the next runs
    /// for a growing pause; at the third tell the owner.
    fn net_failed(&self) {
        let prev = state::read_text(&self.s.paths.net)
            .map(|t| NetBackoff::parse(&t))
            .unwrap_or_default();
        let next = policy::net_failed(prev, self.clock.now());
        self.put(&self.s.paths.net, next.render().as_bytes());
        if next.fails == policy::NET_OWNER_RUN {
            self.log.say(&policy::net_owner_text(next.fails));
        }
    }

    /// A code other than 0 and 1 from a call: 3 and 4 have rules of their own, the others are
    /// logged. `call` is the text for the log (`mine --quiet`). Returns the exit code.
    pub(super) fn fail(&mut self, call: &str, rc: &StepRc) -> u8 {
        match rc.code {
            3 => {
                self.log
                    .say(&format!("crewboard {call} failed: exit 3: {}", rc.msg));
                self.net_failed();
                WakeExit::Network.code()
            }
            4 => self.stop_on_4(call, &rc.msg),
            n => {
                self.log
                    .say(&format!("crewboard {call} failed: exit {n}: {}", rc.msg));
                n
            }
        }
    }

    /// Exit 4 (config or key): write the stop file, tell the coordinator if crewboard can still
    /// post, exit 4.
    fn stop_on_4(&mut self, call: &str, reason: &str) -> u8 {
        if self.s.env_load_failed {
            // A file problem that `chmod` or a fixed line cures does not change the mtime that the
            // stop file waits for: no stop file. Every run says it again until the file is right.
            self.log.say(&format!(
                "crewboard {call} exit 4: {reason}. The env file cannot be used; fix it (the next run tries again)"
            ));
            return WakeExit::Config.code();
        }
        let line = format!("{} {call}: {reason}\n", format_ts(self.clock.now()));
        self.put(&self.s.paths.stop, line.as_bytes());
        self.log.say(&format!(
            "STOPPED: crewboard {call} exit 4: {reason}. No more wakes until the env file or the agent config changes, or you remove {}",
            self.s.paths.stop.display()
        ));
        if let Some(code) = self.signalled() {
            return code;
        }
        // a key or config error often stops this too; then nobody is told
        let mut out = Vec::new();
        let _ = self.steps.mine_text(&mut out);
        if let Some(t) = policy::first_ticket(&String::from_utf8_lossy(&out)) {
            let body = format!(
                "The wake wrapper of {} stopped: crewboard exit 4 ({reason}). It polls again when the owner has fixed the config or the token.",
                self.s.agent.as_deref().unwrap_or("this agent")
            );
            if self.steps.say(&t, &body, true).code == 0 {
                self.log.line(&format!("told the coordinator on {t}"));
            }
        }
        WakeExit::Config.code()
    }
}
