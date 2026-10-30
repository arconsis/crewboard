//! The result files of detached jobs in `<state dir>/results/`. A finished result (a line
//! `rc=<digits> end=...`, see `crate::lock::result`) is reported once: `<file>.seen` keeps a copy
//! of the `rc=` line that was reported, and a new run of the job (a new line) is reported again.
//! Only this agent's dir is read. A file that starts with `#!` is a script, not a result.

use super::state::{read_text, write_atomic};
use crate::lock::result::ResultFile;
use std::path::{Path, PathBuf};

/// A finished result that nobody has reported yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub file: PathBuf,
    /// The `rc=` line as it stands in the file: what `<file>.seen` will keep.
    pub rc_line: String,
    pub result: ResultFile,
}

fn seen_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".seen");
    PathBuf::from(s)
}

/// The unreported results of `dir`, in name order. Needs an events file to name them in, so
/// `has_events_file` false gives none.
pub fn pending(dir: &Path, has_events_file: bool) -> Vec<Pending> {
    if !has_events_file {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let name = file.file_name()?.to_string_lossy().to_string();
            if name.ends_with(".seen") || name.ends_with(".log") || name.contains(".tmp.") {
                return None;
            }
            if !file.is_file() {
                return None;
            }
            let text = String::from_utf8_lossy(&std::fs::read(&file).ok()?).into_owned();
            let rc_line = ResultFile::rc_line(&text)?.to_string();
            let result = ResultFile::parse(&text)?;
            if read_text(&seen_path(&file)).as_deref() == Some(rc_line.as_str()) {
                return None;
            }
            Some(Pending {
                file,
                rc_line,
                result,
            })
        })
        .collect()
}

/// The line of the events file for a finished job.
pub fn event_line(p: &Pending) -> String {
    format!(
        "detached job finished: ticket {} rc={} log={} result={}",
        p.result.ticket.as_deref().unwrap_or("unknown"),
        p.result.rc,
        p.result.log,
        p.file.display()
    )
}

/// Mark a result as reported: `<file>.seen` gets a copy of its `rc=` line.
pub fn mark_seen(p: &Pending) -> std::io::Result<()> {
    write_atomic(&seen_path(&p.file), format!("{}\n", p.rc_line).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(r: &Path, n: &str, t: &str) {
        std::fs::write(r.join(n), t).unwrap();
    }

    fn names(v: &[Pending]) -> Vec<String> {
        v.iter()
            .map(|p| p.file.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn rs1_a_finished_result_is_pending_until_it_is_marked_and_a_new_run_is_pending_again() {
        let d = tempfile::tempdir().unwrap();
        w(
            d.path(),
            "7.result",
            "rc=0 end=2026-10-21T10:00:00Z log=/r/7.log\nticket=7\n",
        );
        let p = pending(d.path(), true);
        assert_eq!(names(&p), vec!["7.result"]);
        assert_eq!(
            event_line(&p[0]),
            format!(
                "detached job finished: ticket 7 rc=0 log=/r/7.log result={}",
                d.path().join("7.result").display()
            )
        );
        mark_seen(&p[0]).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.path().join("7.result.seen")).unwrap(),
            "rc=0 end=2026-10-21T10:00:00Z log=/r/7.log\n"
        );
        assert!(pending(d.path(), true).is_empty(), "reported once");
        // a new run writes a new line and is reported again; no ticket line: unknown
        w(
            d.path(),
            "7.result",
            "rc=1 end=2026-10-21T11:00:00Z log=/r/7.log\n",
        );
        let p = pending(d.path(), true);
        assert_eq!(names(&p), vec!["7.result"]);
        assert!(event_line(&p[0]).starts_with("detached job finished: ticket unknown rc=1 "));
    }

    #[test]
    fn the_seen_file_of_an_older_release_counts() {
        // a seen file of an older release holds the rc line copied by a plain redirect
        let d = tempfile::tempdir().unwrap();
        w(d.path(), "7.result", "rc=0 end=T log=l\nticket=7\n");
        w(d.path(), "7.result.seen", "rc=0 end=T log=l\n");
        assert!(pending(d.path(), true).is_empty());
        w(d.path(), "7.result.seen", "rc=0 end=OTHER log=l\n");
        assert_eq!(pending(d.path(), true).len(), 1);
    }

    #[test]
    fn rs2_only_finished_regular_files_that_are_no_scripts_are_pending() {
        let d = tempfile::tempdir().unwrap();
        w(d.path(), "8.result", "ticket=8\n");
        w(
            d.path(),
            "239-chain.sh",
            "#!/bin/bash\nrc=${PIPESTATUS[0]}\nrc=0 end=x log=y\n",
        );
        w(
            d.path(),
            "10.result",
            "rc=0 end=2026-10-21T10:00:00Z log=a.log\nticket=10\n",
        );
        w(d.path(), "10.result.log", "rc=0 end=x log=y\n");
        w(d.path(), "11.result.tmp.42", "rc=0 end=x log=y\n");
        std::fs::create_dir(d.path().join("sub")).unwrap();
        assert_eq!(names(&pending(d.path(), true)), vec!["10.result"]);
        // without an events file nothing is pending; a missing dir gives none
        assert!(pending(d.path(), false).is_empty());
        assert!(pending(&d.path().join("nope"), true).is_empty());
    }

    #[test]
    fn rs3_only_the_given_dir_is_read() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("other/results")).unwrap();
        std::fs::create_dir_all(d.path().join("results")).unwrap();
        w(
            &d.path().join("other/results"),
            "9.result",
            "rc=0 end=x log=x.log\nticket=9\n",
        );
        assert!(pending(&d.path().join("results"), true).is_empty());
        w(
            &d.path().join("results"),
            "11.result",
            "rc=3 end=x log=y.log\nticket=11\n",
        );
        let p = pending(&d.path().join("results"), true);
        assert_eq!(names(&p), vec!["11.result"]);
        assert!(!d.path().join("other/results/9.result.seen").exists());
    }

    #[test]
    fn files_of_the_older_release_in_the_fixtures_read() {
        // result files of an older release, kept as bytes in tests/fixtures
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wake/results");
        let p = pending(&dir, true);
        assert_eq!(
            names(&p),
            vec!["7.result", "bare.result"],
            "the unfinished one is not pending"
        );
        let seven = &p[0].result;
        assert_eq!((seven.rc, seven.ticket.as_deref()), (0, Some("7")));
        assert_eq!(
            seven.sha.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(seven.log, "/state/results/7.result.log");
        assert_eq!((p[1].result.rc, p[1].result.ticket.clone()), (137, None));
    }
}
