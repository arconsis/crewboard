//! `--tick`: the headless coordinator without a model, under the lock.

use super::flow::Flow;
use crate::wake::policy;

impl Flow<'_, '_> {
    /// One `tick` call (with the retries). A tick that gives the same text as the last one is a
    /// quiet poll; else its lines go to the log. The result is the exit code.
    pub(super) fn tick(&mut self) -> u8 {
        let (rc, out) = self.call("tick", &mut |s, o| s.tick(o));
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
        if policy::tick_is_quiet(&text, last.as_deref()) {
            self.note_quiet();
            return 0;
        }
        for l in policy::tick_log_lines(&text) {
            self.log.line(&l);
        }
        0
    }
}
