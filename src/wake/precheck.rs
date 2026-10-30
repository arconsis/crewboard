//! The precheck: the cheap check before the lock, with no model. One `poll --quiet`; on a quiet
//! board the run goes on only for events that an earlier wake kept, a finished detached job that
//! nobody reported, or owned work whose back-off is over.

use super::flow::{Flow, Reason};
use crate::wake::events;
use crate::wake::policy;
use crate::wake::results;
use crate::wake::state;

/// How the run goes on after the precheck.
pub enum Precheck {
    /// Go on to the lock and the wake.
    Wake(Reason),
    /// A quiet poll: nothing to do, exit 0.
    Quiet,
    /// The run ends with this code.
    End(u8),
}

/// The answer of the check for owned work.
enum Work {
    Due,
    NotDue,
    End(u8),
}

impl Flow<'_, '_> {
    /// The cheap check: no model, no lock. The back-off file is cleared when the whole precheck
    /// succeeded, not after the poll alone.
    pub(super) fn precheck(&mut self) -> Precheck {
        let (rc, _) = self.call("poll", &mut |s, o| s.poll(true, o));
        if let Some(code) = self.signalled() {
            return Precheck::End(code);
        }
        let mut reason = Reason::Events;
        match rc.code {
            1 => {
                if let Some(why) = self.kept_events() {
                    self.log.line(&why);
                } else {
                    match self.work_wake_due() {
                        Work::Due => reason = Reason::Work,
                        Work::NotDue => {
                            self.net_ok();
                            self.note_quiet();
                            return Precheck::Quiet;
                        }
                        Work::End(code) => return Precheck::End(code),
                    }
                }
            }
            0 => {}
            _ => return Precheck::End(self.fail("poll", &rc)),
        }
        self.net_ok();
        Precheck::Wake(reason)
    }

    /// A reason to wake although the board is quiet, as a log line: events that an earlier wake
    /// kept in `<events file>.unhandled`, or a finished detached job that nobody reported. Both
    /// need an events file: it carries them to the wake.
    fn kept_events(&self) -> Option<String> {
        let ev = self.s.events_file.as_deref()?;
        let kept = events::unhandled_path(ev);
        // After two stops at the time budget in a row on one ticket, the kept events wait for the
        // work back-off like any other work: waking at every tick would spend without end.
        let held =
            state::read_budget_streak(&self.s.paths.budget_streak).is_some_and(|(n, _)| n >= 2);
        if !held && std::fs::metadata(&kept).is_ok_and(|m| m.len() > 0) {
            return Some(format!(
                "events kept from an earlier wake are in {}",
                kept.display()
            ));
        }
        if !results::pending(&self.s.paths.results, true).is_empty() {
            return Some(format!(
                "a detached job has finished (result file in {})",
                self.s.paths.results.display()
            ));
        }
        None
    }

    /// Does the agent own an In Progress or Ready ticket, and is the back-off over? A wake that
    /// ends without done or block leaves such a ticket behind; without this check nothing would
    /// wake the agent again.
    fn work_wake_due(&mut self) -> Work {
        let last = state::read_last_wake(&self.s.paths.last_wake);
        if !policy::work_due(last, self.clock.now(), self.s.work_backoff) {
            return Work::NotDue;
        }
        let (rc, _) = self.call("mine", &mut |s, _| s.mine_quiet());
        if let Some(code) = self.signalled() {
            return Work::End(code);
        }
        match rc.code {
            0 => Work::Due,
            3 | 4 => Work::End(self.fail("mine --quiet", &rc)),
            _ => Work::NotDue,
        }
    }
}
