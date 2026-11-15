//! The result file of a detached job (`run --detach --result FILE`). The writer (`detach`) and
//! the readers (`crewboard wake`) use this one type. The format is stable across releases, so a
//! file of an older release reads the same:
//!
//! ```text
//! rc=<digits> end=<utc> log=<path>
//! ticket=<n>          (optional)
//! sha=<head>          (optional)
//! ```
//!
//! A file counts as a result when one of its lines has the shape `rc=<digits> end=...`. A file that
//! starts with `#!` is a script and never a result.
//!
//! `<file>.seen` holds the `rc=` line that the wake reported. A result whose `rc=` line is not in
//! its `.seen` file is unreported: nobody has read it yet. The wake and a second start of the same
//! job (`detach`) use this one rule.

use std::path::{Path, PathBuf};

/// A finished detached job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultFile {
    /// The exit code of the job (128 plus the signal for a job that a signal ended).
    pub rc: u64,
    /// When it ended: `YYYY-MM-DDTHH:MM:SSZ`.
    pub end: String,
    /// The log of the job.
    pub log: String,
    pub ticket: Option<String>,
    pub sha: Option<String>,
}

/// The line `rc=<digits> end=...` as it stands in the file: the key that `<file>.seen` keeps.
fn is_rc_line(l: &str) -> bool {
    let Some(rest) = l.strip_prefix("rc=") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && rest[digits..].starts_with(" end=")
}

/// `<file>.seen`: the copy of the `rc=` line that was reported.
pub fn seen_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".seen");
    PathBuf::from(s)
}

/// Does `<file>.seen` hold `rc_line`? Then this result was reported.
pub fn is_reported(file: &Path, rc_line: &str) -> bool {
    std::fs::read_to_string(seen_path(file)).is_ok_and(|t| t.trim_end_matches('\n') == rc_line)
}

/// The `rc=` line of `file` when the file is a finished result that nobody has reported. `None`
/// for a missing file, a file without an `rc=` line, a script and a reported result.
pub fn unreported_rc_line(file: &Path) -> Option<String> {
    let text = String::from_utf8_lossy(&std::fs::read(file).ok()?).into_owned();
    let line = ResultFile::rc_line(&text)?.to_string();
    (!is_reported(file, &line)).then_some(line)
}

impl ResultFile {
    /// The text of the file, as the supervisor writes it.
    pub fn render(&self) -> String {
        let mut t = format!("rc={} end={} log={}\n", self.rc, self.end, self.log);
        if let Some(n) = &self.ticket {
            t.push_str(&format!("ticket={n}\n"));
        }
        if let Some(s) = &self.sha {
            t.push_str(&format!("sha={s}\n"));
        }
        t
    }

    /// The first line of the file that has the shape of the `rc=` line, as it stands.
    pub fn rc_line(text: &str) -> Option<&str> {
        if text.starts_with("#!") {
            return None;
        }
        text.lines().find(|l| is_rc_line(l))
    }

    /// Read a result file. `None` for a script and for a file without an `rc=` line (a job that
    /// has not finished).
    pub fn parse(text: &str) -> Option<ResultFile> {
        let line = Self::rc_line(text)?;
        let rest = &line["rc=".len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        // a number beyond u64 is no reason to hide the file: it counts as the largest one
        let rc = rest[..digits].parse().unwrap_or(u64::MAX);
        let after = &rest[digits + " end=".len()..];
        let end = after.split(' ').next().unwrap_or("").to_string();
        // the log is everything after the last ` log=` of the line (it may hold blanks)
        let log = line
            .rfind(" log=")
            .map(|i| line[i + " log=".len()..].to_string())
            .unwrap_or_default();
        let value = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key))
                .map(str::to_string)
                .filter(|v| !v.is_empty())
        };
        Some(ResultFile {
            rc,
            end,
            log,
            ticket: value("ticket="),
            sha: value("sha="),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> ResultFile {
        ResultFile {
            rc: 3,
            end: "2026-10-21T10:00:00Z".into(),
            log: "/r/7.result.log".into(),
            ticket: Some("7".into()),
            sha: Some("abc123".into()),
        }
    }

    #[test]
    fn an_rc_that_overflows_still_makes_a_result() {
        let r = ResultFile::parse("rc=99999999999999999999999 end=T log=l\n").unwrap();
        assert_eq!((r.rc, r.end.as_str()), (u64::MAX, "T"));
        let r = ResultFile::parse("rc=4294967296 end=T log=l\n").unwrap();
        assert_eq!(r.rc, 4_294_967_296);
    }

    #[test]
    fn the_writer_and_the_reader_agree() {
        let r = full();
        assert_eq!(ResultFile::parse(&r.render()), Some(r.clone()));
        // optional lines stay out of the file
        let bare = ResultFile {
            ticket: None,
            sha: None,
            ..r
        };
        assert_eq!(
            bare.render(),
            "rc=3 end=2026-10-21T10:00:00Z log=/r/7.result.log\n"
        );
        assert_eq!(ResultFile::parse(&bare.render()), Some(bare));
    }

    #[test]
    fn a_file_of_the_shell_script_reads_the_same() {
        // the format of an older release: the rc line, then ticket= and sha=
        let old =
            "rc=0 end=2026-10-21T10:00:00Z log=/x/results/7.result.log\nticket=7\nsha=0123abc\n";
        let r = ResultFile::parse(old).unwrap();
        assert_eq!(
            (r.rc, r.end.as_str(), r.ticket.as_deref(), r.sha.as_deref()),
            (0, "2026-10-21T10:00:00Z", Some("7"), Some("0123abc"))
        );
        assert_eq!(r.log, "/x/results/7.result.log");
        assert_eq!(r.render(), old, "the writer gives the old bytes back");
        // no ticket line: none
        let r = ResultFile::parse("rc=1 end=2026-10-21T11:00:00Z log=a.log\n").unwrap();
        assert_eq!((r.rc, r.ticket, r.sha), (1, None, None));
        // a log path with a blank and a text that holds ` log=` once more: the last one counts
        let r = ResultFile::parse("rc=0 end=T log=/a b/c log=/d e.log\n").unwrap();
        assert_eq!(r.log, "/d e.log");
        // the rc line may come after other lines
        assert_eq!(
            ResultFile::parse("ticket=9\nrc=2 end=T log=l\n")
                .unwrap()
                .ticket
                .as_deref(),
            Some("9")
        );
    }

    #[test]
    fn what_is_no_result_is_none() {
        for text in [
            "",
            "ticket=8\n",
            "rc=0\n",
            "rc= end=x\n",
            "rcx=0 end=x\n",
            " rc=0 end=x\n",
            "#!/bin/bash\nrc=0 end=x log=y\n",
            "#!/bin/bash\nrun-chain\nrc=${PIPESTATUS[0]}\nexit \"$rc\"\n",
        ] {
            assert_eq!(ResultFile::parse(text), None, "{text:?}");
        }
        // control: the shape counts
        assert!(ResultFile::parse("rc=0 end=x log=y\n").is_some());
    }

    #[test]
    fn the_rc_line_is_the_key_of_the_seen_file() {
        let t = "ticket=7\nrc=0 end=T log=l\nsha=x\n";
        assert_eq!(ResultFile::rc_line(t), Some("rc=0 end=T log=l"));
        assert_eq!(ResultFile::rc_line("nothing\n"), None);
    }

    #[test]
    fn a_result_is_unreported_until_the_seen_file_holds_its_rc_line() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("7.result");
        assert_eq!(seen_path(&f), d.path().join("7.result.seen"));
        assert_eq!(unreported_rc_line(&f), None, "no file");
        std::fs::write(&f, "ticket=7\n").unwrap();
        assert_eq!(unreported_rc_line(&f), None, "a job that has not finished");
        std::fs::write(&f, "rc=0 end=T log=l\nticket=7\n").unwrap();
        assert_eq!(unreported_rc_line(&f).as_deref(), Some("rc=0 end=T log=l"));
        // the seen file of an older run does not match
        std::fs::write(seen_path(&f), "rc=1 end=OLD log=l\n").unwrap();
        assert_eq!(unreported_rc_line(&f).as_deref(), Some("rc=0 end=T log=l"));
        assert!(!is_reported(&f, "rc=0 end=T log=l"));
        // reported: the same line, with or without the newline at the end
        for seen in ["rc=0 end=T log=l\n", "rc=0 end=T log=l"] {
            std::fs::write(seen_path(&f), seen).unwrap();
            assert_eq!(unreported_rc_line(&f), None, "{seen:?}");
            assert!(is_reported(&f, "rc=0 end=T log=l"));
        }
        // a script is never a result
        std::fs::write(&f, "#!/bin/sh\nrc=0 end=T log=l\n").unwrap();
        std::fs::remove_file(seen_path(&f)).unwrap();
        assert_eq!(unreported_rc_line(&f), None);
    }
}
