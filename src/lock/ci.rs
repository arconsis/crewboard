//! CI first: an agent does not take the lock while a CI job runs or waits on this host.
//! CI is seen in two ways: a process `Runner.Worker`, and a file in the waiting list
//! `<dir>.waiting/<name>`. A CI step registers with `ci-wait` before it waits for the lock and
//! leaves with `ci-done`.

use super::fsmeta::mtime_epoch;
use super::owner::{field, sibling};
use super::procs::Procs;
use std::io;
use std::path::{Path, PathBuf};

pub fn waiting_dir(dir: &Path) -> PathBuf {
    sibling(dir, ".waiting")
}

/// The file name of a waiting entry: every byte outside `A-Za-z0-9._@-` becomes `_`
/// (`tr -c 'A-Za-z0-9._@-' '_'`).
pub fn safe_name(owner: &str) -> String {
    owner
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'@' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// What is seen of CI, as a line, or `None`. An entry older than `ci_max_age`, and an entry whose
/// recorded pid is dead on this host, is removed on the way.
pub fn present(
    dir: &Path,
    host: &str,
    ci_max_age: u64,
    now: i64,
    procs: &dyn Procs,
) -> Option<String> {
    if procs.ci_process_line().is_some() {
        return Some("a CI job runs on this host (Runner.Worker)".to_string());
    }
    let w = waiting_dir(dir);
    let mut names: Vec<String> = std::fs::read_dir(&w)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for n in names {
        let f = w.join(&n);
        if !f.is_file() {
            continue;
        }
        let Some(m) = mtime_epoch(&f) else { continue };
        if now - m > super::judge::secs(ci_max_age) {
            std::fs::remove_file(&f).ok(); // cleanup; a leftover file is harmless
            continue;
        }
        // a recorded pid that is dead on this host: the job is gone, so the entry is too
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let epid = field(&text, "pid")
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
            .unwrap_or("");
        let ehost = field(&text, "host").unwrap_or("");
        if !epid.is_empty()
            && epid != "0"
            && ehost == host
            && !super::judge::alive_text(procs, epid)
        {
            std::fs::remove_file(&f).ok(); // cleanup; a leftover file is harmless
            continue;
        }
        return Some(format!(
            "a CI job waits on this host (waiting list entry {n})"
        ));
    }
    None
}

/// Put `owner` on the waiting list. `pid` is recorded when it is a number other than 0.
pub fn register(
    dir: &Path,
    owner: &str,
    host: &str,
    started: &str,
    pid: Option<&str>,
    writer_pid: u32,
) -> io::Result<()> {
    let w = waiting_dir(dir);
    std::fs::create_dir_all(&w)?;
    let mut text = format!("name={owner}\nhost={host}\nstarted={started}\n");
    if let Some(p) = pid
        && !p.is_empty()
        && p != "0"
        && p.bytes().all(|b| b.is_ascii_digit())
    {
        text.push_str(&format!("pid={p}\n"));
    }
    let tmp = sibling(dir, &format!(".waiting.tmp.{writer_pid}"));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, w.join(safe_name(owner))).inspect_err(|_| {
        std::fs::remove_file(&tmp).ok(); // cleanup; a leftover file is harmless
    })
}

/// Leave the waiting list; the list directory goes when it is empty.
pub fn unregister(dir: &Path, owner: &str) {
    let w = waiting_dir(dir);
    std::fs::remove_file(w.join(safe_name(owner))).ok(); // cleanup; a leftover file is harmless
    std::fs::remove_dir(&w).ok(); // cleanup; a leftover file is harmless
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::procs::FakeProcs;
    use std::time::{Duration, UNIX_EPOCH};

    const NOW: i64 = 1_792_836_000;

    fn old(path: &Path, age: i64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs((NOW - age) as u64))
            .unwrap();
    }

    #[test]
    fn safe_names_replace_every_odd_byte() {
        assert_eq!(safe_name("ci-job_1@x.y"), "ci-job_1@x.y");
        assert_eq!(safe_name("a b/c"), "a_b_c");
        assert_eq!(safe_name("ä"), "__");
    }

    #[test]
    fn h13_a_runner_process_is_ci_no_process_is_not() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let p = FakeProcs::new();
        assert_eq!(present(&lock, "hostA", 3600, NOW, &p), None);
        p.set_ci(Some("/x/Runner.Worker spawnclient"));
        assert_eq!(
            present(&lock, "hostA", 3600, NOW, &p).unwrap(),
            "a CI job runs on this host (Runner.Worker)"
        );
    }

    #[test]
    fn h14_a_waiting_entry_is_ci_and_unregister_removes_the_list() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let p = FakeProcs::new();
        register(&lock, "ci-job-1", "hostA", "t", None, 1).unwrap();
        assert_eq!(
            present(&lock, "hostA", 3600, real_now(), &p).unwrap(),
            "a CI job waits on this host (waiting list entry ci-job-1)"
        );
        unregister(&lock, "ci-job-1");
        assert_eq!(present(&lock, "hostA", 3600, real_now(), &p), None);
        assert!(!waiting_dir(&lock).exists());
    }

    fn real_now() -> i64 {
        crate::clock::now_epoch()
    }

    #[test]
    fn h14c_an_entry_older_than_ci_max_age_is_ignored_and_removed() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let p = FakeProcs::new();
        register(&lock, "ci-old", "hostA", "t", None, 1).unwrap();
        let f = waiting_dir(&lock).join("ci-old");
        old(&f, 7200);
        assert_eq!(present(&lock, "hostA", 3600, NOW, &p), None);
        assert!(!f.exists());
        // the other direction: the same entry younger than the limit blocks
        register(&lock, "ci-young", "hostA", "t", None, 1).unwrap();
        let g = waiting_dir(&lock).join("ci-young");
        old(&g, 60);
        assert!(present(&lock, "hostA", 3600, NOW, &p).is_some());
    }

    #[test]
    fn h19_the_entry_records_a_pid_a_dead_pid_is_removed_a_live_one_blocks() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        register(&lock, "ci-dead", "hostA", "t", Some("222"), 1).unwrap();
        let text = std::fs::read_to_string(waiting_dir(&lock).join("ci-dead")).unwrap();
        assert!(text.contains("pid=222\n"));
        let p = FakeProcs::new().alive_pids(&[111]);
        assert_eq!(present(&lock, "hostA", 3600, real_now(), &p), None);
        assert!(!waiting_dir(&lock).join("ci-dead").exists());
        register(&lock, "ci-live", "hostA", "t", Some("111"), 1).unwrap();
        assert!(present(&lock, "hostA", 3600, real_now(), &p).is_some());
    }

    #[test]
    fn h19d_a_foreign_host_pid_and_an_entry_without_pid_still_block() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let p = FakeProcs::new();
        register(&lock, "ci-far", "hostB", "t", Some("222"), 1).unwrap();
        assert!(present(&lock, "hostA", 3600, real_now(), &p).is_some());
        unregister(&lock, "ci-far");
        register(&lock, "ci-old", "hostA", "t", None, 1).unwrap();
        assert!(present(&lock, "hostA", 3600, real_now(), &p).is_some());
        // pid 0 is no pid
        let text = std::fs::read_to_string(waiting_dir(&lock).join("ci-old")).unwrap();
        assert!(!text.contains("pid="));
        register(&lock, "ci-zero", "hostA", "t", Some("0"), 1).unwrap();
        let text = std::fs::read_to_string(waiting_dir(&lock).join("ci-zero")).unwrap();
        assert!(!text.contains("pid="));
    }
}
