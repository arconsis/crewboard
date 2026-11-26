//! The output of `agent add`: the lines, the failure shape, the log.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The prefix of every line.
pub(in crate::ops::agent) const PREFIX: &str = "agent add:";
pub(super) const TOTAL: usize = 7;

/// A value that is shorter than this is never treated as a token (it would garble the text).
const MIN_SECRET: usize = 8;

/// What to do about an env file that does not load. `m` is the message of the loader.
pub(in crate::ops::agent) fn env_fix(m: &str, env: &Path) -> String {
    if m.contains("not found:") {
        format!(
            "store the token of the agent account: crewboard agent token --initiative <initiative> (it writes {} with mode 600), or run this command with --token-stdin. Trial mode (one account): use the env file that holds the owner's token, the one you gave to `crewboard init --insecure-one-account`; do not run `agent token`",
            env.display()
        )
    } else if m.contains(" has mode ") {
        format!("chmod 600 {}", env.display())
    } else {
        format!(
            "check the env file {}: one line GH_TOKEN=<token of the agent account>, mode 600, your own file",
            env.display()
        )
    }
}

/// A failed step.
pub(in crate::ops::agent) struct Fail {
    pub step: usize,
    pub name: &'static str,
    pub what: String,
    pub fix: String,
    pub code: u8,
}

/// The lines of a failure, for standard error. Pure. `output` is the output of the step: its last
/// five lines are shown.
pub(in crate::ops::agent) fn fail_lines(
    f: &Fail,
    total: usize,
    output: &str,
    log: Option<&Path>,
) -> Vec<String> {
    let mut v = vec![format!(
        "{PREFIX} [{}/{total}] {}: FAIL: {}",
        f.step, f.name, f.what
    )];
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    for l in &lines[lines.len().saturating_sub(5)..] {
        v.push(format!("{PREFIX}   | {l}"));
    }
    v.push(format!("{PREFIX} fix: {}", f.fix));
    if let Some(l) = log {
        v.push(format!(
            "{PREFIX} the full output of each step is in {}",
            l.display()
        ));
    }
    v
}

/// Replace every occurrence of the secret. A secret shorter than [`MIN_SECRET`] is not replaced.
pub(super) fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= MIN_SECRET) {
        out = out.replace(s.as_str(), "***");
    }
    out
}

/// The state of one run: the step count, the secret to keep out of every text, the log.
pub(super) struct Run {
    pub secrets: Vec<String>,
    pub log: Option<PathBuf>,
    /// The output of the step that ran last.
    pub last: String,
}

impl Run {
    pub(super) fn new() -> Self {
        Run {
            secrets: Vec::new(),
            log: None,
            last: String::new(),
        }
    }

    /// Keep the output of a step for the failure text, and write it into the log. The token is
    /// replaced first, so it reaches neither.
    pub(super) fn record(&mut self, title: &str, output: &str) {
        self.last = redact(output, &self.secrets);
        let Some(log) = &self.log else { return };
        let r = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log)
            .and_then(|mut f| {
                writeln!(
                    f,
                    "== {} {}\n{}",
                    crate::clock::now_ts(),
                    redact(title, &self.secrets),
                    self.last.trim_end()
                )
            });
        if let Err(e) = r {
            eprintln!(
                "{PREFIX} warning: cannot write the log {}: {e}",
                log.display()
            );
        }
    }

    /// The lines of a failure. The token is replaced in the reason and in the fix too.
    fn fail_text(&self, f: &Fail) -> Vec<String> {
        let clean = Fail {
            step: f.step,
            name: f.name,
            what: redact(&f.what, &self.secrets),
            fix: redact(&f.fix, &self.secrets),
            code: f.code,
        };
        fail_lines(&clean, TOTAL, &self.last, self.log.as_deref())
    }

    pub(super) fn fail(&self, f: Fail) -> u8 {
        for l in self.fail_text(&f) {
            eprintln!("{l}");
        }
        f.code
    }
}

/// The result of a step that went well: it did the work, or the work was done already.
pub(super) struct Ok_ {
    pub(super) already: bool,
    pub(super) text: String,
}

pub(super) fn say(step: usize, name: &str, r: &Ok_) {
    println!(
        "{PREFIX} [{step}/{TOTAL}] {name}: {} ({})",
        if r.already { "already done" } else { "done" },
        r.text
    );
}
