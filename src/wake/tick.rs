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
    /// tick of such a unit runs no checks of a `checks:` Gate.
    pub(super) fn tick(&mut self) -> u8 {
        let model = self.s.direct.is_some();
        let wd = self.s.work_dir.clone().filter(|_| !model);
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
        }
        if !model {
            return 0;
        }
        let listed = self.steps.judgment();
        self.coordinator(listed.as_ref(), quiet)
    }
}
