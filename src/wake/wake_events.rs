//! The events of a wake: the events file that the wake command reads, the receipts, the copy
//! that a failed wake keeps for the next one (`<events file>.unhandled`) and the check, after a
//! good wake, for events that needed an act and got none.

use super::flow::Flow;
use crate::clock::format_ts;
use crate::wake::events::{self, MinePart, PollPart};
use crate::wake::receipts;
use crate::wake::results::{self, Pending};
use std::path::Path;

/// How the run goes on after the events file is made.
pub enum Prepared {
    /// Go on to the wake command (also when there is no events file).
    Ready,
    /// The run ends with this code (exit 4 of a call, a write that failed, a signal).
    End(u8),
}

/// The start of the first line of kept events after a stop at the time budget.
const BUDGET_HEADER: &str = "The last wake hit the time budget.";

impl Flow<'_, '_> {
    /// The events file of the wake, with `--events-file`: the kept events, `poll` (which moves the
    /// cursor), the finished results and `mine`, written to `<file>.tmp` and renamed. Only after
    /// the rename are `<file>.unhandled` removed and the results marked as reported, so a failed
    /// write loses nothing. Then one receipt line per event in the log.
    pub(super) fn prepare_events(&mut self) -> Prepared {
        let Some(ev) = self.s.events_file.clone() else {
            return Prepared::Ready;
        };
        match self.write_events(&ev).and_then(|()| self.log_receipts(&ev)) {
            Ok(()) => Prepared::Ready,
            Err(code) => Prepared::End(code),
        }
    }

    /// Build and write the events file. `Err(code)` ends the run.
    fn write_events(&mut self, ev: &Path) -> Result<(), u8> {
        if let Some(parent) = ev.parent().filter(|p| !p.as_os_str().is_empty())
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            self.log
                .say(&format!("cannot make the dir of {}: {e}", ev.display()));
            return Err(4);
        }
        let kept_path = events::unhandled_path(ev);
        // a read error other than "not there" must not drop the only copy of the kept events
        let kept = match std::fs::read(&kept_path) {
            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.log.say(&format!(
                    "cannot read the kept events {}: {e}",
                    kept_path.display()
                ));
                return Err(4);
            }
        };
        let ts = format_ts(self.clock.now());
        let pending = results::pending(&self.s.paths.results, true);
        let (poll, mine) = self.ask_board()?;
        let lines: Vec<String> = pending.iter().map(results::event_line).collect();
        let text = events::compose(&kept, &ts, &poll, &lines, &mine);
        if let Err(e) =
            crate::fsutil::write_atomic(ev, text.as_bytes(), crate::fsutil::Mode::Default)
        {
            self.log.say(&format!(
                "cannot write the events file {}: {e}",
                ev.display()
            ));
            return Err(4);
        }
        if let Err(e) = std::fs::remove_file(&kept_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            // the new file holds the kept events too; stop, so the next run does not wake in a loop
            self.log.say(&format!(
                "cannot remove {}: {e}; the events are in {}",
                kept_path.display(),
                ev.display()
            ));
            return Err(4);
        }
        self.mark_reported(&pending);
        Ok(())
    }

    /// The two board reads of the events file: `poll` and `mine`. The poll moves the cursor.
    /// Events that it returned are lost when the run ends before the file is written (a signal,
    /// a failed `mine`, a write error); a cursor that moves only after the file is written would
    /// close this gap.
    fn ask_board(&mut self) -> Result<(PollPart, MinePart), u8> {
        let (rc, out) = self.call("poll", &mut |s, o| s.poll(false, o));
        if let Some(code) = self.signalled() {
            return Err(code);
        }
        let poll = match rc.code {
            0 => PollPart::Events(String::from_utf8_lossy(&out).into_owned()),
            1 => PollPart::NoEvents,
            4 => return Err(self.fail("poll", &rc)),
            n => {
                self.log.line(&format!(
                    "crewboard poll for the events file failed: exit {n}: {}",
                    rc.msg
                ));
                PollPart::Failed(n)
            }
        };
        let (rc, out) = self.call("mine", &mut |s, o| s.mine_text(o));
        if let Some(code) = self.signalled() {
            return Err(code);
        }
        let mine = match rc.code {
            0 | 1 => MinePart::Text(String::from_utf8_lossy(&out).into_owned()),
            4 => return Err(self.fail("mine", &rc)),
            n => MinePart::Failed(n),
        };
        Ok((poll, mine))
    }

    fn mark_reported(&self, pending: &[Pending]) {
        for p in pending {
            if let Err(e) = results::mark_seen(p) {
                self.log.line(&format!(
                    "cannot mark {} as reported: {e}",
                    p.file.display()
                ));
            }
        }
    }

    /// The receipts: one log line per event that the wake gets. No board read.
    fn log_receipts(&mut self, ev: &Path) -> Result<(), u8> {
        let (rc, out) = self.call("acted", &mut |s, o| s.acted_list(ev, o));
        if let Some(code) = self.signalled() {
            return Err(code);
        }
        if rc.code == 0 {
            for l in receipts::delivered_lines(&String::from_utf8_lossy(&out)) {
                self.log.line(&l);
            }
        } else {
            self.log.line(&format!(
                "event receipts not written: crewboard acted exit {}: {}",
                rc.code, rc.msg
            ));
        }
        Ok(())
    }

    /// A wake that failed keeps its events file for the next wake.
    pub(super) fn keep_events(&self) {
        self.keep_events_with(None);
    }

    /// [`Self::keep_events`], with a line of its own before the file. The events file of a wake
    /// that began with kept events holds the header of the earlier stop: that line is dropped, so
    /// the header is there once and first, then the kept events, then the poll.
    pub(super) fn keep_events_with(&self, first: Option<&str>) {
        let Some(ev) = &self.s.events_file else {
            return;
        };
        let Ok(text) = std::fs::read(ev) else {
            return;
        };
        let text = with_budget_header(&text, first);
        let kept = events::unhandled_path(ev);
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&kept)
            .and_then(|mut f| std::io::Write::write_all(&mut f, &text));
        if let Err(e) = r {
            self.log.line(&format!(
                "cannot keep the events in {}: {e}",
                kept.display()
            ));
        }
    }

    /// After a wake with exit 0: which events that need an act got none? Each goes to
    /// `<events file>.unhandled` once (the call appends them, behind a marker). `Err(code)` ends
    /// the run (exit 4 of the call, or a signal).
    pub(super) fn check_acted(&mut self, since: &str) -> Result<(), u8> {
        let Some(ev) = self.s.events_file.clone().filter(|e| e.is_file()) else {
            return Ok(());
        };
        let kept = events::unhandled_path(&ev);
        let (rc, out) = self.call("acted", &mut |s, o| s.acted_check(&ev, since, &kept, o));
        if let Some(code) = self.signalled() {
            return Err(code);
        }
        match rc.code {
            0 => {
                for m in receipts::misses(&String::from_utf8_lossy(&out)) {
                    self.log.line(&m.log_text());
                }
            }
            4 => return Err(self.fail("acted", &rc)),
            n => self.log.line(&format!(
                "event check failed: crewboard acted exit {n}: {}",
                rc.msg
            )),
        }
        Ok(())
    }
}

/// `text` with the header lines of earlier budget stops removed and `first` (when there is one)
/// put before it.
pub(super) fn with_budget_header(text: &[u8], first: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 128);
    if let Some(line) = first {
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    for l in text.split_inclusive(|b| *b == b'\n') {
        if !l.starts_with(BUDGET_HEADER.as_bytes()) {
            out.extend_from_slice(l);
        }
    }
    out
}
