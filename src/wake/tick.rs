//! `--tick`: the headless coordinator without a model, under the lock. With `--runtime` the
//! model wake of the unattended coordinator follows it (`coordinator`).

use super::flow::Flow;
use crate::wake::policy;

impl Flow<'_, '_> {
    /// One `tick` call (with the retries). A tick that gives the same text as the last one is a
    /// quiet poll; else its lines go to the log. The result is the exit code.
    ///
    /// A unit with `--runtime` then asks whether a model starts (`Flow::coordinator`). It
    /// gives the tick no checkout: `--work-dir` is the start dir of the model there, and the
    /// tick of such a unit runs no checks of a `checks:` Gate. A tick that is not quiet says
    /// so in the log, with its lines; a quiet tick writes nothing, as in a `--tick` unit.
    /// The tick does not depend on the model: a model command that could not be built is one
    /// log line per run, and the exit code is the one of the tick. Standard error gets that
    /// line once per error text: a scheduler that mails what a unit writes there must not mail
    /// at each run.
    pub(super) fn tick(&mut self) -> u8 {
        let model = self.s.model_unit;
        let wd = self.s.work_dir.clone().filter(|_| !model);
        if model {
            // the count file is read ahead of the tick: at the first tick of a UTC day it
            // holds the wakes of the day that ended, which the daily digest names
            let w = crate::wake::state::read_coord_wakes(&self.s.paths.coord_wakes);
            let (day, count) = w.as_ref().map_or(("", 0), |w| (w.day.as_str(), w.count));
            self.steps.model_wakes(day, count);
        }
        let (rc, out) = self.call("tick", &mut |s, o| s.tick(wd.as_deref(), o));
        if let Some(code) = self.signalled() {
            return code;
        }
        if rc.code != 0 {
            return self.fail("tick", &rc);
        }
        self.net_ok();
        let text = String::from_utf8_lossy(&out).into_owned();
        let last = std::fs::read(&self.s.paths.last_tick)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        self.put(&self.s.paths.last_tick, &out);
        let quiet = policy::tick_is_quiet(&text, last.as_deref());
        if quiet {
            self.note_quiet();
        } else {
            for l in policy::tick_log_lines(&text) {
                self.log.line(&l);
            }
            if model {
                self.log.line(super::job::NO_CHECKS);
            }
        }
        if !model {
            return 0;
        }
        let told = &self.s.paths.model_error;
        if let Some(e) = &self.s.model_error {
            let line = format!(
                "the model of this unit cannot start: {e}. The tick ran; repair the unit (crewboard wake install)"
            );
            // the file holds the error that standard error got: `doctor --setup` shows it
            let known = crate::wake::state::read_text(told);
            if known.as_deref() == Some(e.trim_end_matches('\n')) {
                self.log.line(&line);
            } else {
                self.put(told, e.as_bytes());
                self.log.say(&line);
            }
            return 0;
        }
        if told.exists() {
            crate::base::ignore::best_effort_cleanup(std::fs::remove_file(told));
        }
        let listed = self.steps.judgment();
        self.coordinator(listed.as_ref(), quiet)
    }
}
