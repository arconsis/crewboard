//! `--tick --runtime R`: the model wake of the unattended coordinator
//! (spec/agents#unattended-coordinator), after the tick and under the same lock.
//!
//! The tick gives the judgment list and the verdicts of the day as values
//! ([`crate::wake::steps::Steps::judgment`]). The start rule is pure
//! ([`policy::coord_start`]). Before a model starts, two files are written, each in one step:
//! the judgment file that the model reads, and `coordinator-wakes` with the count of the day.
//! A write that fails means no start. A run that starts no model notes a list that changed
//! ([`policy::coord_note`]), so a ticket that leaves the list and comes back is a change.
//!
//! What this phase never does: it runs no `poll` and no duty, it writes no heartbeat (no
//! ticker runs beside the model), and it posts nothing. The model posts through crewboard, and
//! the scope and the budget of the headless config dir decide there.

use super::WakeExit;
use super::flow::{Flow, Reason};
use super::runtime::Ended;
use crate::base::ignore::best_effort_cleanup;
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
        let listed = listed.map(|l| self.with_plain_keys(l));
        let listed = listed.as_ref();
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
                // the list of this tick, for the next run: a change that starts no model now
                // (the distance, the pause, the limit of the day) waits for one
                if let Some(note) = policy::coord_note(&facts)
                    && let Err(e) = fsutil::write_atomic(
                        &self.s.paths.coord_wakes,
                        note.render().as_bytes(),
                        Mode::Default,
                    )
                {
                    self.log.line(&format!(
                        "cannot write {}: {e}",
                        self.s.paths.coord_wakes.display()
                    ));
                }
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
        // a new run of the unread check: this model sees each unread message once
        let rc = self.steps.model_run_starts(now);
        if rc.code != 0 {
            self.log.say(&format!(
                "cannot start a run of the unread check in the state of the config dir: {}; no model starts",
                rc.msg
            ));
            return 0;
        }
        if !self.write_before_start(listed, &next, now) {
            return 0;
        }
        self.run_model(lines.len(), &next)
    }

    /// The list without a ticket whose key is not `<owner>/<repo>#<n>` in plain characters. The
    /// keys come from the config and the ticket index of crewboard; the check keeps any other
    /// text out of the judgment file and the prompt, whatever a later change does to a key.
    fn with_plain_keys(&self, listed: &Listed) -> Listed {
        let mut out = listed.clone();
        out.needs
            .retain(|(key, _)| policy::is_plain_ticket_key(key));
        let dropped = listed.needs.len() - out.needs.len();
        if dropped > 0 {
            self.log.line(&format!(
                "{dropped} ticket(s) of the judgment list have a key that is not <owner>/<repo>#<n>; they are left out"
            ));
        }
        out
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
                let streak = self.count_budget_stop("-");
                self.log.say(&format!(
                    "model wake stopped at the time budget of {} ({streak} in a row)",
                    self.s.max_wake.text
                ));
                0
            }
            Ended::Exit(0) => {
                self.wake_ran();
                best_effort_cleanup(std::fs::remove_file(&self.s.paths.budget_streak));
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
