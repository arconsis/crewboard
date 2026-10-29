//! The owner file `<dir>/owner`: `key=value` lines. It is part of the public contract of the host
//! lock (CI steps and project tools read and write it), so the key order and the texts stay.
//!
//! Valid = a non-empty `agent=` value and an all-digit `started_epoch=` value.

use crate::fsutil::Mode;
use std::io;
use std::path::{Path, PathBuf};

/// What `take` writes. `pid` is kept as text (`0` means none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerFile {
    pub agent: String,
    pub host: String,
    pub pid: String,
    pub started: String,
    pub started_epoch: i64,
    pub reason: String,
    pub expect: Option<u64>,
    pub detached: bool,
}

/// The file is line based: a newline in a value becomes a blank.
pub fn one_line(s: &str) -> String {
    s.replace('\n', " ")
}

impl OwnerFile {
    /// The lines in the fixed order that readers expect: agent, host, pid, started, started_epoch,
    /// reason, expect (only when given), detached=1 (only for a detached job).
    pub fn render(&self) -> String {
        let mut s = format!(
            "agent={}\nhost={}\npid={}\nstarted={}\nstarted_epoch={}\nreason={}\n",
            self.agent, self.host, self.pid, self.started, self.started_epoch, self.reason
        );
        if let Some(e) = self.expect {
            s.push_str(&format!("expect={e}\n"));
        }
        if self.detached {
            s.push_str("detached=1\n");
        }
        s
    }
}

/// Write the owner file in one step: a temp file `owner.tmp.<pid>.<n>`, then `rename`. A reader
/// never sees half a file. The lock dir must exist: a dir that was taken over is not made again.
pub fn write_atomic(dir: &Path, o: &OwnerFile) -> io::Result<()> {
    crate::fsutil::write_atomic(&dir.join("owner"), o.render().as_bytes(), Mode::Default)
}

/// The value of the first line that starts with `key=` (`sed -n "s/^key=//p" | head -n 1`).
pub fn field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content
        .lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn blank(s: Option<&str>) -> bool {
    s.is_none_or(|v| v.chars().all(char::is_whitespace))
}

fn digits(s: Option<&str>) -> bool {
    s.is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
}

/// Is `content` (a non-empty regular owner file) a valid owner file?
pub fn is_valid(content: &str) -> bool {
    !blank(field(content, "agent")) && digits(field(content, "started_epoch"))
}

/// What is wrong with an invalid owner file, for the text `foreign holder: owner (<problem>): ...`.
pub fn problem(content: Option<&str>) -> String {
    let Some(content) = content else {
        return "not a regular file".to_string();
    };
    let head = &content.as_bytes()[..content.len().min(4096)];
    let has_pair = String::from_utf8_lossy(head).lines().any(|l| {
        let mut chars = l.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && l.split_once('=')
                .is_some_and(|(k, _)| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    });
    if !has_pair {
        return "no key=value fields".to_string();
    }
    let mut p: Vec<&str> = Vec::new();
    if blank(field(content, "agent")) {
        p.push("no agent= value");
    }
    if !digits(field(content, "started_epoch")) {
        p.push("no numeric started_epoch=");
    }
    p.join(", ")
}

/// The path `<dir><suffix>` (for `.takeover`, `.waiting`, `.stale.*`, `.rel.*`).
pub fn sibling(dir: &Path, suffix: &str) -> PathBuf {
    let mut s = dir.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> OwnerFile {
        OwnerFile {
            agent: "a@x".into(),
            host: "hostA".into(),
            pid: "42".into(),
            started: "2026-10-24T10:00:00Z".into(),
            started_epoch: 1_792_836_000,
            reason: "cargo test".into(),
            expect: None,
            detached: false,
        }
    }

    #[test]
    fn h1_the_owner_file_has_the_lines_in_the_old_order() {
        assert_eq!(
            sample().render(),
            "agent=a@x\nhost=hostA\npid=42\nstarted=2026-10-24T10:00:00Z\nstarted_epoch=1792836000\nreason=cargo test\n"
        );
        let mut o = sample();
        o.expect = Some(1800);
        o.detached = true;
        assert!(
            o.render()
                .ends_with("reason=cargo test\nexpect=1800\ndetached=1\n")
        );
    }

    #[test]
    fn the_written_file_is_valid_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        write_atomic(d.path(), &sample()).unwrap();
        let c = std::fs::read_to_string(d.path().join("owner")).unwrap();
        assert!(is_valid(&c));
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["owner".to_string()]);
    }

    #[test]
    fn o_validity_needs_agent_and_numeric_started_epoch() {
        assert!(is_valid("agent=a\nstarted_epoch=5\n"));
        // each missing piece alone makes it invalid
        assert!(!is_valid("agent=\nstarted_epoch=5\n"));
        assert!(!is_valid("agent=   \nstarted_epoch=5\n"));
        assert!(!is_valid("agent=a\n"));
        assert!(!is_valid("agent=a\nstarted_epoch=yesterday\n"));
        assert!(!is_valid("agent=a\nstarted_epoch=\n"));
        assert!(!is_valid("agent-a@host-b\n"));
        // the FIRST agent= line counts, as `sed | head -n 1` did
        assert!(!is_valid("agent=\nagent=b\nstarted_epoch=5\n"));
    }

    #[test]
    fn o_the_problem_texts() {
        assert_eq!(problem(None), "not a regular file");
        assert_eq!(problem(Some("agent-a@host-b\n")), "no key=value fields");
        assert_eq!(
            problem(Some("agent=a\nhost=h\n")),
            "no numeric started_epoch="
        );
        assert_eq!(
            problem(Some("agent=\nstarted_epoch=1\n")),
            "no agent= value"
        );
        assert_eq!(
            problem(Some("started_epoch=yesterday\n")),
            "no agent= value, no numeric started_epoch="
        );
    }

    #[test]
    fn field_reads_the_first_matching_line_only() {
        let c = "pid=5\nxpid=9\npid=7\nagent=a\n";
        assert_eq!(field(c, "pid"), Some("5"));
        assert_eq!(field(c, "agent"), Some("a"));
        assert_eq!(field(c, "host"), None);
    }

    #[test]
    fn a_newline_in_a_value_is_a_blank() {
        assert_eq!(one_line("a\nb"), "a b");
    }
}
