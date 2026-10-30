//! The checks before the poll, in this order: an older version that still runs, the stop file of
//! an earlier exit 4, and the net back-off of earlier exits 3. Each one that ends the run ends it
//! with exit 0.

use super::flow::Flow;
use crate::base::ignore::best_effort_cleanup;
use crate::fsutil::{self, Mode};
use crate::lock::procs::Alive;
use crate::wake::policy::{self, NetBackoff};
use crate::wake::state;
use std::path::{Path, PathBuf};

/// How the run goes on after the checks.
pub enum Preflight {
    /// Go on to the precheck.
    Go,
    /// The run ends with this code.
    Done(u8),
}

impl Flow<'_, '_> {
    pub(super) fn preflight(&self) -> Preflight {
        if let Some(code) = self.legacy_state() {
            return Preflight::Done(code);
        }
        if self.stopped() {
            return Preflight::Done(0);
        }
        if let Some(b) = state::read_text(&self.s.paths.net).map(|t| NetBackoff::parse(&t))
            && b.pauses(self.clock.now())
        {
            self.note_quiet();
            return Preflight::Done(0);
        }
        Preflight::Go
    }

    /// Old per-initiative state of an older version: its `last-wake` is read once into the state
    /// dir of this agent, and a live old wake lock that is this agent's (or names no agent) means
    /// that an older copy still runs: no new wake. `Some(0)` ends the run.
    fn legacy_state(&self) -> Option<u8> {
        let legacy = self.s.legacy_dir.as_ref()?;
        let last = &self.s.paths.last_wake;
        if !last.exists()
            && let Ok(text) = std::fs::read(legacy.join("last-wake"))
            && let Err(e) = fsutil::write_atomic(last, &text, Mode::Default)
        {
            self.log
                .line(&format!("cannot copy the old last-wake: {e}"));
        }
        let lock = legacy.join("wake.lock");
        if !lock.is_dir() {
            return None;
        }
        let pid: u32 = std::fs::read_to_string(lock.join("pid"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        if self.pids.alive(pid) != Alive::Yes {
            return None;
        }
        // The old lock names no agent; the command line of its holder does. A holder with another
        // --config-dir is another agent of the initiative: its wake is not ours.
        let holder = self
            .pids
            .command(pid)
            .and_then(|c| config_dir_of_command(&c));
        let ours = self
            .s
            .config_dir
            .as_ref()
            .map(|c| c.to_string_lossy().into_owned());
        let other = match (&holder, &ours) {
            (Some(h), Some(o)) => h.trim_end_matches('/') != o.trim_end_matches('/'),
            _ => false,
        };
        if other {
            return None;
        }
        self.log.line(&format!(
            "an older per-initiative wake runs (pid {pid}, lock {}); not starting another",
            lock.display()
        ));
        Some(0)
    }

    /// An earlier exit 4 stopped the wakes. `true`: still stopped, nothing to do. A change of the
    /// env file or the agent config after the stop lifts it.
    fn stopped(&self) -> bool {
        let stop = &self.s.paths.stop;
        let Some(stop_time) = std::fs::metadata(stop).ok().and_then(|m| m.modified().ok()) else {
            return false;
        };
        let times: Vec<(PathBuf, Option<std::time::SystemTime>)> = self
            .watched_files()
            .into_iter()
            .map(|f| {
                let t = std::fs::metadata(&f)
                    .ok()
                    .filter(|m| m.is_file())
                    .and_then(|m| m.modified().ok());
                (f, t)
            })
            .collect();
        let Some(changed) = policy::stop_lifted_by(stop_time, &times) else {
            return true;
        };
        best_effort_cleanup(std::fs::remove_file(stop));
        self.log.say(&format!(
            "stop lifted: {} changed after the stop; polling again",
            changed.display()
        ));
        false
    }

    /// The env file, then `*.toml`, `agent.env` and `keys/*` of the config dir.
    fn watched_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.s.env_file.iter().cloned().collect();
        if let Some(c) = &self.s.config_dir {
            let list = |dir: &Path, keep: &dyn Fn(&str) -> bool| -> Vec<PathBuf> {
                let mut f: Vec<PathBuf> = std::fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .filter_map(|e| e.ok())
                    .filter(|e| keep(&e.file_name().to_string_lossy()))
                    .map(|e| e.path())
                    .collect();
                f.sort();
                f
            };
            v.extend(list(c, &|n| n.ends_with(".toml") && !n.starts_with('.')));
            v.push(c.join("agent.env"));
            v.extend(list(&c.join("keys"), &|n| !n.starts_with('.')));
        }
        v
    }
}

/// The word after `--config-dir ` in a command line.
pub(super) fn config_dir_of_command(cmd: &str) -> Option<String> {
    let (_, rest) = cmd.split_once("--config-dir ")?;
    rest.split(' ').next().map(str::to_string)
}
