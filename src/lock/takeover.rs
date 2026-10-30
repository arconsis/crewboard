//! Takeover of a stale lock: one winner, atomic. Only one taker passes the guard
//! `<dir>.takeover` (a `mkdir`), re-checks that the lock is still stale, renames it to a unique
//! name and then makes its own. Nobody does remove-then-mkdir on the live name. The takeover
//! prints ONE log line with the old owner and the reason.

use super::Session;
use super::fsmeta::mtime_epoch;
use super::judge::{self, Judgement};
use super::owner::sibling;
use super::snapshot::Snapshot;
use crate::base::ignore::best_effort_cleanup;
use crate::clock::Clock;
use std::path::{Path, PathBuf};

/// A guard older than this belongs to a dead taker.
pub const GUARD_MAX_AGE: i64 = 60;

/// What one try to take a lock over did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// This process moved the stale lock away.
    Took,
    /// Another taker holds the guard (a guard older than [`GUARD_MAX_AGE`] is removed), or a new
    /// holder took the lock in the moment between the check and the rename (it is put back).
    Contended,
    /// The lock was gone, live, or could not be moved when this process looked again under the
    /// guard.
    NotStale,
}

/// A `<lock>.stale.*` directory that a failed or killed taker left is removed after this age.
const STALE_SWEEP_AGE: i64 = 3600;

/// The guard directory of a lock.
pub fn guard_path(dir: &Path) -> PathBuf {
    sibling(dir, ".takeover")
}

/// Remove the `<lock>.stale.*` siblings that are older than [`STALE_SWEEP_AGE`] (best effort).
fn sweep_stale(dir: &Path, now: i64) {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return;
    };
    let prefix = format!("{}.stale.", name.to_string_lossy());
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for e in entries.flatten() {
        let old = e.file_name().to_string_lossy().starts_with(&prefix)
            && mtime_epoch(&e.path()).is_some_and(|m| now - m > STALE_SWEEP_AGE);
        if old {
            best_effort_cleanup(std::fs::remove_dir_all(e.path()));
        }
    }
}

/// THE takeover of a stale lock, for every kind of lock (the host lock and the wake lock). The
/// caller decides what "stale" means for its own files: `recheck(path)` gives `Some(line)` when
/// the lock at `path` is stale (`line` is the log line), `None` when it is gone or live. It runs
/// under the guard on the lock, and once more on the moved directory: a new holder that took the
/// lock between the first check and the rename is put back (never two live holders), and the try
/// is `Contended`. `say` gets the line once the lock is moved away. `pid` and the time make the
/// name of the moved directory unique, `<dir>.stale.<pid>.<now>`.
pub fn take_over_guarded(
    dir: &Path,
    clock: &dyn Clock,
    pid: u32,
    mut recheck: impl FnMut(&Path) -> Option<String>,
    say: &mut dyn FnMut(&str),
) -> Outcome {
    let guard = guard_path(dir);
    if std::fs::create_dir(&guard).is_err() {
        // another taker is taking over, or its process died. Two contenders can both see an old
        // guard and the second may remove a fresh one; the check under the guard and the atomic
        // rename keep that safe, the worst case is one more `Contended`.
        if mtime_epoch(&guard).is_some_and(|m| clock.now() - m > GUARD_MAX_AGE) {
            best_effort_cleanup(std::fs::remove_dir(&guard));
        }
        return Outcome::Contended;
    }
    // we hold the guard. Look again: the lock may be gone or live by now.
    let mut outcome = Outcome::NotStale;
    if let Some(line) = recheck(dir) {
        // (the same pid in the same second after a failed removal can collide: then the rename
        // fails, the outcome is NotStale and the next second is free)
        let moved = sibling(dir, &format!(".stale.{pid}.{}", clock.now()));
        if std::fs::rename(dir, &moved).is_ok() {
            if recheck(&moved).is_some() {
                say(&line);
                best_effort_cleanup(std::fs::remove_dir_all(&moved));
                sweep_stale(dir, clock.now());
                outcome = Outcome::Took;
            } else {
                // a new holder was quicker than we: give its lock back
                outcome = Outcome::Contended;
                if dir.exists() || std::fs::rename(&moved, dir).is_err() {
                    say(&format!(
                        "a live lock {} was moved to {} by mistake and could not be put back; both stay",
                        dir.display(),
                        moved.display()
                    ));
                }
            }
        }
    }
    best_effort_cleanup(std::fs::remove_dir(&guard));
    outcome
}

impl Session<'_> {
    /// `true` if THIS process moved the stale lock away.
    pub fn take_over(&mut self) -> bool {
        let dir = self.dir.clone();
        let (clock, log, pid) = (self.clock, self.log, self.procs.pid());
        let recheck = |at: &Path| {
            let snap = Snapshot::read(at);
            if !snap.exists {
                return None;
            }
            let Judgement::Stale { reason, text } = self.judge(&snap) else {
                return None;
            };
            Some(format!(
                "took over stale lock {}: old owner {}; reason={} ({text})",
                self.dir.display(),
                judge::describe_owner(&snap),
                reason.name()
            ))
        };
        take_over_guarded(&dir, clock, pid, recheck, &mut |l| log.say(l)) == Outcome::Took
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::time::{Duration, UNIX_EPOCH};

    const NOW: i64 = 1_700_000_000;

    fn guard_aged(dir: &Path, age: i64) {
        let g = guard_path(dir);
        std::fs::create_dir(&g).unwrap();
        let t = UNIX_EPOCH + Duration::from_secs((NOW - age) as u64);
        std::fs::File::open(&g).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn t1_a_stale_lock_is_moved_said_once_and_leaves_no_guard() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let mut said = Vec::new();
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |_| Some("line".into()),
            &mut |l| said.push(l.to_string()),
        );
        assert_eq!((r, said), (Outcome::Took, vec!["line".to_string()]));
        assert_eq!(std::fs::read_dir(t.path()).unwrap().count(), 0);
    }

    #[test]
    fn t2_a_lock_that_is_not_stale_under_the_guard_stays_and_nothing_is_said() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let r = take_over_guarded(&dir, &FakeClock::at(NOW), 7, |_| None, &mut |_| {
            panic!("said")
        });
        assert_eq!(r, Outcome::NotStale);
        assert!(dir.is_dir() && !guard_path(&dir).exists());
    }

    #[test]
    fn t3_a_fresh_guard_is_contended_and_stays_an_old_one_goes() {
        let t = tempfile::tempdir().unwrap();
        for (age, gone) in [(GUARD_MAX_AGE, false), (GUARD_MAX_AGE + 1, true)] {
            let dir = t.path().join(format!("l{age}"));
            std::fs::create_dir(&dir).unwrap();
            guard_aged(&dir, age);
            let r = take_over_guarded(
                &dir,
                &FakeClock::at(NOW),
                7,
                |_| panic!("looked"),
                &mut |_| {},
            );
            assert_eq!(r, Outcome::Contended);
            assert_eq!(!guard_path(&dir).exists(), gone, "guard of {age} s");
            assert!(dir.is_dir(), "the lock is not touched by a contended try");
        }
    }

    #[test]
    fn t4_the_check_runs_under_the_guard_and_the_line_after_the_rename_with_the_unique_name() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let mut checks = 0;
        let mut seen = Vec::new();
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |at| {
                checks += 1;
                assert!(guard_path(&dir).is_dir(), "the check runs under the guard");
                (at.exists()).then(|| "line".to_string())
            },
            &mut |_| {
                seen.push((
                    dir.exists(),
                    t.path().join(format!("l.stale.7.{NOW}")).exists(),
                ));
            },
        );
        assert_eq!(r, Outcome::Took);
        assert_eq!(checks, 2, "once on the lock, once on the moved directory");
        assert_eq!(
            seen,
            vec![(false, true)],
            "said after the rename, before the removal"
        );
    }

    #[test]
    fn t5_a_lock_that_turns_live_after_the_rename_is_put_back_and_nothing_is_said() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("owner"), "new").unwrap();
        let mut n = 0;
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |_| {
                n += 1;
                (n == 1).then(|| "line".to_string())
            },
            &mut |_| panic!("said"),
        );
        assert_eq!(r, Outcome::Contended);
        assert!(dir.join("owner").is_file(), "the live lock is back");
        assert_eq!(
            std::fs::read_dir(t.path()).unwrap().count(),
            1,
            "no guard, no moved dir"
        );
    }

    #[test]
    fn t6_a_lock_that_cannot_be_put_back_stays_where_it_is_and_one_line_names_both() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let mut n = 0;
        let mut said = Vec::new();
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |_| {
                n += 1;
                if n == 2 {
                    // a third holder made the lock path again
                    std::fs::create_dir(&dir).unwrap();
                }
                (n == 1).then(|| "line".to_string())
            },
            &mut |l| said.push(l.to_string()),
        );
        assert_eq!(r, Outcome::Contended);
        assert!(dir.is_dir() && t.path().join(format!("l.stale.7.{NOW}")).is_dir());
        assert_eq!(said.len(), 1);
        assert!(
            said[0].contains("l.stale.7") && said[0].contains("by mistake"),
            "{said:?}"
        );
    }

    #[test]
    fn t7_a_failed_rename_says_nothing_and_removes_the_guard() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let taken = t.path().join(format!("l.stale.7.{NOW}"));
        std::fs::create_dir(&taken).unwrap();
        std::fs::write(taken.join("x"), "x").unwrap();
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |_| Some("l".into()),
            &mut |_| panic!("said"),
        );
        assert_eq!(r, Outcome::NotStale);
        assert!(dir.is_dir() && !guard_path(&dir).exists());
    }

    #[test]
    fn t8_a_takeover_sweeps_old_stale_leftovers_only() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("l");
        std::fs::create_dir(&dir).unwrap();
        let mk = |name: &str, age: i64| {
            let p = t.path().join(name);
            std::fs::create_dir(&p).unwrap();
            std::fs::write(p.join("x"), "x").unwrap();
            let m = UNIX_EPOCH + Duration::from_secs((NOW - age) as u64);
            std::fs::File::open(&p).unwrap().set_modified(m).unwrap();
            p
        };
        let old = mk("l.stale.1.1", STALE_SWEEP_AGE + 1);
        let young = mk("l.stale.2.2", STALE_SWEEP_AGE);
        let other = mk("m.stale.3.3", STALE_SWEEP_AGE + 100);
        let r = take_over_guarded(
            &dir,
            &FakeClock::at(NOW),
            7,
            |at| at.exists().then(|| "l".into()),
            &mut |_| {},
        );
        assert_eq!(r, Outcome::Took);
        assert!(!old.exists() && young.exists() && other.exists());
    }
}
