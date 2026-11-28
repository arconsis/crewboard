//! `--tick --runtime R`: the model wake of the unattended coordinator
//! (spec/agents#unattended-coordinator), after the tick and under the same lock.
//!
//! The tick gives the judgment list and the verdicts of the day as values
//! ([`crate::wake::steps::Steps::judgment`]). The start rule is pure
//! ([`policy::coord_start`]). Before a model starts, two files are written, each in one step:
//! the judgment file that the model reads, and `coordinator-wakes` with the count of the day.
//! A write that fails means no start.
//!
//! What this phase never does: it runs no `poll` and no duty, it writes no heartbeat (no
//! ticker runs beside the model), and it posts nothing. The model posts through crewboard, and
//! the scope and the budget of the headless config dir decide there.

use super::WakeExit;
use super::flow::{Flow, Reason};
use super::runtime::Ended;
use crate::clock::format_ts;
use crate::fsutil::{self, Mode};
use crate::wake::policy::{self, CoordFacts, CoordWakes};
use crate::wake::state;
use crate::wake::steps::Listed;
use std::path::Path;

/// The UTC day of a time (`YYYY-MM-DD`).
fn day_of(now: i64) -> String {
    format_ts(now).chars().take(10).collect()
}

impl Flow<'_, '_> {
    /// Start the model when the judgment list needs it. `listed`: the values of the tick.
    /// `tick_quiet`: the tick gave no new line, so a reason for "no model" is no news either.
    /// The result is the exit code.
    pub(super) fn coordinator(&mut self, listed: Option<&Listed>, tick_quiet: bool) -> u8 {
        let Some(runtime) = self.s.direct.as_ref().map(|d| d.runtime) else {
            return 0;
        };
        let now = self.clock.now();
        let today = day_of(now);
        let lines = listed.map(Listed::list_lines).unwrap_or_default();
        let hash = policy::list_hash(&lines);
        let stored = state::read_coord_wakes(&self.s.paths.coord_wakes);
        let facts = CoordFacts {
            mode: listed.map(|l| l.mode.as_str()),
            listed: lines.len(),
            hash: &hash,
            used: listed.map_or(0, |l| l.used),
            budget: listed.map_or(0, |l| l.budget),
            paused: self.wake_paused(),
            stored: stored.as_ref(),
            today: &today,
            now,
            runtime,
        };
        let next = match policy::coord_start(&facts) {
            Ok(next) => next,
            Err(why) => {
                if let Some(text) = why.text(&format_ts).filter(|_| !tick_quiet) {
                    self.log.line(&format!(
                        "{} ticket(s) need a judgment; no model starts: {text}",
                        lines.len()
                    ));
                }
                return 0;
            }
        };
        let Some(listed) = listed else {
            return 0;
        };
        if !self.write_before_start(listed, &next, now) {
            return 0;
        }
        self.run_model(lines.len(), &next)
    }

    /// The judgment file, then the count of the day. `false`: a write failed, and no model
    /// starts. The judgment file comes first: when it fails, no wake is counted.
    fn write_before_start(&self, listed: &Listed, next: &CoordWakes, now: i64) -> bool {
        let write = |path: &Path, text: String| {
            fsutil::write_atomic(path, text.as_bytes(), Mode::Default)
                .map_err(|e| {
                    self.log.say(&format!(
                        "cannot write {}: {e}; no model starts",
                        path.display()
                    ));
                })
                .is_ok()
        };
        write(&self.s.paths.judgment, listed.file_text(&format_ts(now)))
            && write(&self.s.paths.coord_wakes, next.render())
    }

    /// Start the model and wait for it. No heartbeat ticker runs beside it (`Flow::watch`).
    fn run_model(&mut self, tickets: usize, next: &CoordWakes) -> u8 {
        let cmd = self
            .s
            .direct
            .as_ref()
            .map(|d| d.text.clone())
            .unwrap_or_default();
        self.log.say(&format!(
            "{tickets} ticket(s) need a judgment; waking the model (wake {} of {} today): {cmd}",
            next.count,
            policy::MAX_COORD_WAKES_PER_DAY
        ));
        match self.run_wake_command(&cmd, Reason::Judgment) {
            Ended::Signal(code) => code,
            Ended::Budget => {
                // the model started and worked until its budget: it runs
                self.wake_ran();
                self.log.say(&format!(
                    "model wake stopped at the time budget of {}",
                    self.s.max_wake.text
                ));
                0
            }
            Ended::Exit(0) => {
                self.wake_ran();
                self.log.say("model wake finished OK");
                0
            }
            Ended::Exit(rc) => {
                let line = self.wake_failed(rc);
                self.log.say(&line);
                WakeExit::WakeFailed.code()
            }
        }
    }
}
