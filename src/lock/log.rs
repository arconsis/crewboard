//! The log lines and the answer lines of `crewboard lock`. A log line goes to stderr and to
//! `--log FILE`: `<UTC ts> host-lock: <text>`. The prefix `host-lock:` stays (the CI steps and the
//! wake wrapper grep for it). An answer line goes to stdout. A test collects both in memory.

use crate::base::ignore::best_effort_output;
use crate::clock::{Clock, format_ts};
use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;

enum Sink {
    Process,
    Memory {
        err: RefCell<Vec<String>>,
        out: RefCell<Vec<String>>,
    },
}

pub struct Log<'a> {
    clock: &'a dyn Clock,
    file: Option<PathBuf>,
    sink: Sink,
}

impl<'a> Log<'a> {
    /// Writes to stderr and stdout of this process, and to `file` when given.
    pub fn new(clock: &'a dyn Clock, file: Option<PathBuf>) -> Self {
        Log {
            clock,
            file,
            sink: Sink::Process,
        }
    }

    /// Collects the lines in memory (tests, and a caller that wants the first line of an answer).
    pub fn memory(clock: &'a dyn Clock) -> Self {
        Log {
            clock,
            file: None,
            sink: Sink::Memory {
                err: RefCell::new(Vec::new()),
                out: RefCell::new(Vec::new()),
            },
        }
    }

    /// One line to stderr and to `--log`.
    pub fn say(&self, text: &str) {
        let line = format!("{} host-lock: {text}", format_ts(self.clock.now()));
        match &self.sink {
            Sink::Process => {
                // a closed stderr (`... 2>&1 | grep -q held`) must not panic
                best_effort_output(writeln!(std::io::stderr(), "{line}"));
                if let Some(f) = &self.file
                    && let Ok(mut h) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(f)
                {
                    best_effort_output(writeln!(h, "{line}"));
                }
            }
            Sink::Memory { err, .. } => err.borrow_mut().push(line),
        }
    }

    /// One answer line to stdout.
    pub fn out(&self, text: &str) {
        match &self.sink {
            Sink::Process => {
                let mut o = std::io::stdout().lock();
                best_effort_output(writeln!(o, "{text}"));
                best_effort_output(o.flush());
            }
            Sink::Memory { out, .. } => out.borrow_mut().push(text.to_string()),
        }
    }

    /// The log lines so far, with their time stamp and prefix, then the answer lines.
    pub fn lines(&self) -> Vec<String> {
        match &self.sink {
            Sink::Memory { err, out } => err
                .borrow()
                .iter()
                .chain(out.borrow().iter())
                .cloned()
                .collect(),
            Sink::Process => Vec::new(),
        }
    }

    /// The log lines so far, without their time stamp and prefix (tests).
    #[cfg(test)]
    pub fn said(&self) -> Vec<String> {
        match &self.sink {
            Sink::Memory { err, .. } => err
                .borrow()
                .iter()
                .map(|l| {
                    l.split_once(" host-lock: ")
                        .map_or(l.clone(), |p| p.1.into())
                })
                .collect(),
            Sink::Process => Vec::new(),
        }
    }

    /// The answer lines so far (tests).
    #[cfg(test)]
    pub fn printed(&self) -> Vec<String> {
        match &self.sink {
            Sink::Memory { out, .. } => out.borrow().clone(),
            Sink::Process => Vec::new(),
        }
    }
}
