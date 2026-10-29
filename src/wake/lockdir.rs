//! The wake lock: a directory made with `mkdir` that holds `owner`, `pid` (of the wake) and
//! `started` (epoch). A waiter calls it STALE for a named reason only:
//!
//! - `pid-dead`: the pid in the lock is gone,
//! - `too-old`: the lock is held for longer than `stale_after` seconds,
//! - `empty-after-grace`: there is no pid file and the directory is older than the grace (a fresh
//!   empty directory is a waker between `mkdir` and its first write, never stale).
//!
//! A stale lock is taken over under the guard directory `<lock>.takeover`: the waiter that holds
//! the guard judges the lock again, so a lock that another waiter took over meanwhile is not moved
//! away. At most three attempts.

use crate::clock::Clock;
use crate::lock::fsmeta::mtime_epoch;
use crate::lock::procs::{Alive, Procs};
use crate::lock::takeover::{Outcome, take_over_guarded};
use std::path::{Path, PathBuf};
use std::time::Duration;
#[cfg(all(test, unix))]
use std::time::UNIX_EPOCH;

const ATTEMPTS: u32 = 3;

/// When a lock is stale.
#[derive(Debug, Clone, Copy)]
pub struct Rules {
    pub stale_after: i64,
    pub empty_grace: i64,
}

/// Why a lock is stale: `name: text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stale {
    pub name: &'static str,
    pub text: String,
}

impl std::fmt::Display for Stale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.text)
    }
}

/// The result of [`acquire`].
#[derive(Debug)]
pub enum Acquired {
    /// This process holds the lock; dropping the value releases it.
    Held(Held),
    /// A live wake holds it: its pid and start, as written in the lock.
    Busy { pid: String, started: String },
    /// Three attempts did not give the lock.
    Cannot,
}

/// The lock of this process.
#[derive(Debug)]
pub struct Held {
    dir: PathBuf,
    pid: u32,
}

impl Drop for Held {
    /// Remove the lock if it still names this process.
    fn drop(&mut self) {
        if read(&self.dir.join("pid")).as_deref() == Some(self.pid.to_string().as_str()) {
            std::fs::remove_dir_all(&self.dir).ok(); // cleanup; a leftover file is harmless
        }
    }
}

/// The content of a file without the trailing line breaks.
fn read(p: &Path) -> Option<String> {
    Some(
        std::fs::read_to_string(p)
            .ok()?
            .trim_end_matches('\n')
            .to_string(),
    )
}

/// Is the lock stale? `None` when it is held, or when it does not exist.
pub fn judge(dir: &Path, rules: &Rules, now: i64, pids: &dyn Procs) -> Option<Stale> {
    let pid = read(&dir.join("pid")).unwrap_or_default();
    if pid.is_empty() {
        // no pid file: a fresh directory is a waker between mkdir and its first write
        let age = now - mtime_epoch(dir)?;
        if age > rules.empty_grace {
            return Some(Stale {
                name: "empty-after-grace",
                text: format!(
                    "no pid file and the lock is {age} s old (grace {} s)",
                    rules.empty_grace
                ),
            });
        }
        return None;
    }
    let alive = pid
        .parse::<u32>()
        .is_ok_and(|p| p != 0 && pids.alive(p) == Alive::Yes);
    if !alive {
        return Some(Stale {
            name: "pid-dead",
            text: format!("owner pid {pid} is gone"),
        });
    }
    let started: Option<i64> = read(&dir.join("started")).and_then(|s| s.trim().parse().ok());
    if let Some(s) = started
        && now - s > rules.stale_after
    {
        return Some(Stale {
            name: "too-old",
            text: format!(
                "owner pid {pid} holds it for {} s, over the budget of {} s",
                now - s,
                rules.stale_after
            ),
        });
    }
    None
}

/// Take the lock. `owner` is the text of the `owner` file. `say` gets the line for the takeover
/// of a stale lock.
pub fn acquire(
    dir: &Path,
    owner: &str,
    rules: &Rules,
    clock: &dyn Clock,
    pids: &dyn Procs,
    say: &mut dyn FnMut(&str),
) -> Acquired {
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).ok(); // a missing dir shows as an error at the next write
    }
    for _ in 0..ATTEMPTS {
        match std::fs::create_dir(dir) {
            Ok(()) => {
                let me = pids.pid();
                let wrote = std::fs::write(dir.join("owner"), format!("{owner}\n"))
                    .and_then(|()| std::fs::write(dir.join("pid"), format!("{me}\n")))
                    .and_then(|()| {
                        std::fs::write(dir.join("started"), format!("{}\n", clock.now()))
                    });
                if wrote.is_err() {
                    // a lock without its files would be judged stale by the next waiter
                    std::fs::remove_dir_all(dir).ok(); // cleanup; a leftover file is harmless
                    return Acquired::Cannot;
                }
                return Acquired::Held(Held {
                    dir: dir.to_path_buf(),
                    pid: me,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if judge(dir, rules, clock.now(), pids).is_some() {
                    take_over(dir, rules, clock, pids, say);
                } else {
                    return Acquired::Busy {
                        pid: read(&dir.join("pid")).unwrap_or_default(),
                        started: read(&dir.join("started")).unwrap_or_default(),
                    };
                }
            }
            Err(_) => return Acquired::Cannot,
        }
    }
    Acquired::Cannot
}

/// One try to move a stale lock away, through the one takeover of `crate::lock`. The line that
/// `say` gets names the reason and the old owner. A busy guard costs a second of waiting.
fn take_over(
    dir: &Path,
    rules: &Rules,
    clock: &dyn Clock,
    pids: &dyn Procs,
    say: &mut dyn FnMut(&str),
) {
    let recheck = |at: &Path| {
        if !at.is_dir() {
            return None;
        }
        let why = judge(at, rules, clock.now(), pids)?;
        Some(format!(
            "STALE lock ({why}, owner {}); taking it over",
            read(&at.join("owner")).unwrap_or_default()
        ))
    };
    if take_over_guarded(dir, clock, pids.pid(), recheck, say) == Outcome::Contended {
        clock.sleep(Duration::from_secs(1));
    }
}

/// Set the modification time of a path (a test helper that the component tests share).
#[cfg(all(test, unix))]
pub(crate) fn set_mtime(p: &Path, epoch: i64) {
    let t = UNIX_EPOCH + Duration::from_secs(epoch.max(0) as u64);
    std::fs::File::open(p).unwrap().set_modified(t).unwrap();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::lock::takeover::guard_path;
    use std::cell::{Cell, RefCell};

    const NOW: i64 = 1_700_000_000;

    /// A table of processes; `flip_after` makes every pid live after that many questions.
    struct Table {
        live: Vec<u32>,
        me: u32,
        asked: Cell<u32>,
        live_after: Option<u32>,
    }

    impl Table {
        fn new(live: &[u32]) -> Table {
            Table {
                live: live.to_vec(),
                me: 4242,
                asked: Cell::new(0),
                live_after: None,
            }
        }
    }

    impl Procs for Table {
        fn pid(&self) -> u32 {
            self.me
        }
        fn ppid(&self) -> u32 {
            1
        }
        fn alive(&self, pid: u32) -> Alive {
            self.asked.set(self.asked.get() + 1);
            let live = match self.live_after {
                Some(n) if self.asked.get() > n => true,
                _ => self.live.contains(&pid),
            };
            if live { Alive::Yes } else { Alive::No }
        }
        fn lookup(&self, _: u32) -> Option<crate::lock::procs::ProcInfo> {
            None
        }
        fn ci_process_line(&self) -> Option<String> {
            None
        }
        fn command(&self, _: u32) -> Option<String> {
            None
        }
    }

    fn rules() -> Rules {
        Rules {
            stale_after: 14_400,
            empty_grace: 30,
        }
    }

    fn plant(dir: &Path, owner: &str, pid: &str, started: i64) {
        std::fs::create_dir(dir).unwrap();
        std::fs::write(dir.join("owner"), format!("{owner}\n")).unwrap();
        if !pid.is_empty() {
            std::fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
        }
        std::fs::write(dir.join("started"), format!("{started}\n")).unwrap();
    }

    fn go(dir: &Path, clock: &FakeClock, pids: &Table, rules: &Rules) -> (Acquired, Vec<String>) {
        let said = RefCell::new(Vec::new());
        let r = acquire(dir, "poll-and-wake@h dev", rules, clock, pids, &mut |l| {
            said.borrow_mut().push(l.to_string())
        });
        (r, said.into_inner())
    }

    #[test]
    fn a_free_lock_is_taken_with_the_three_files_and_released_on_drop() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("state/wake.lock");
        let clock = FakeClock::at(NOW);
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &rules());
        let Acquired::Held(h) = r else {
            panic!("not held")
        };
        assert!(said.is_empty());
        assert_eq!(
            read(&dir.join("owner")).as_deref(),
            Some("poll-and-wake@h dev")
        );
        assert_eq!(read(&dir.join("pid")).as_deref(), Some("4242"));
        assert_eq!(
            read(&dir.join("started")).as_deref(),
            Some(NOW.to_string().as_str())
        );
        drop(h);
        assert!(!dir.exists());
    }

    #[test]
    fn p4_a_live_lock_is_busy_and_names_its_pid_and_start() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "o", "77", NOW - 100);
        let clock = FakeClock::at(NOW);
        let (r, said) = go(&dir, &clock, &Table::new(&[77]), &rules());
        let Acquired::Busy { pid, started } = r else {
            panic!("not busy")
        };
        assert_eq!((pid.as_str(), started), ("77", (NOW - 100).to_string()));
        assert!(said.is_empty());
        assert!(dir.is_dir(), "the lock of the live wake stays");
    }

    #[test]
    fn p5_a_dead_pid_is_taken_over_and_said() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "old-owner", "77", NOW - 100);
        let clock = FakeClock::at(NOW);
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &rules());
        assert!(matches!(r, Acquired::Held(_)));
        assert_eq!(
            said,
            vec!["STALE lock (pid-dead: owner pid 77 is gone, owner old-owner); taking it over"]
        );
        assert_eq!(read(&dir.join("pid")).as_deref(), Some("4242"));
        let left: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 1, "no guard or stale dir is left: {left:?}");
    }

    #[test]
    fn p5b_a_live_lock_is_stale_only_over_the_budget() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "o", "77", NOW - 100);
        let clock = FakeClock::at(NOW);
        let live = Table::new(&[77]);
        // young: respected
        let young = Rules {
            stale_after: 3600,
            empty_grace: 30,
        };
        assert!(matches!(
            go(&dir, &clock, &live, &young).0,
            Acquired::Busy { .. }
        ));
        // over the budget: taken, with the named reason
        let old = Rules {
            stale_after: 10,
            empty_grace: 30,
        };
        let (r, said) = go(&dir, &clock, &live, &old);
        assert!(matches!(r, Acquired::Held(_)));
        assert!(
            said[0].starts_with(
                "STALE lock (too-old: owner pid 77 holds it for 100 s, over the budget of 10 s"
            ),
            "{said:?}"
        );
    }

    #[test]
    fn p5c_a_fresh_empty_lock_is_never_stale() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        std::fs::create_dir(&dir).unwrap();
        let clock = FakeClock::at(crate::clock::now_epoch());
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &rules());
        assert!(matches!(r, Acquired::Busy { .. }));
        assert!(said.is_empty() && dir.is_dir());
    }

    #[test]
    fn p5d_an_empty_lock_older_than_the_grace_is_taken_with_the_named_reason() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        std::fs::create_dir(&dir).unwrap();
        set_mtime(&dir, NOW - 100);
        let clock = FakeClock::at(NOW);
        let five = Rules {
            stale_after: 14_400,
            empty_grace: 5,
        };
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &five);
        assert!(matches!(r, Acquired::Held(_)));
        assert!(
            said[0].starts_with(
                "STALE lock (empty-after-grace: no pid file and the lock is 100 s old (grace 5 s)"
            ),
            "{said:?}"
        );
        // control: the same directory inside the grace is not taken
        let dir2 = t.path().join("two.lock");
        std::fs::create_dir(&dir2).unwrap();
        set_mtime(&dir2, NOW - 3);
        assert!(matches!(
            go(&dir2, &clock, &Table::new(&[]), &five).0,
            Acquired::Busy { .. }
        ));
    }

    #[test]
    fn a_lock_that_turns_live_while_the_guard_is_held_is_not_moved() {
        // the first look says "dead", the look under the guard says "alive": another waiter took
        // the lock over in between
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "o", "77", NOW - 100);
        let clock = FakeClock::at(NOW);
        let mut p = Table::new(&[]);
        p.live_after = Some(1);
        let (r, said) = go(&dir, &clock, &p, &rules());
        // attempt 1: stale, guard, re-check live: not moved. attempt 2: live and young: busy
        assert!(matches!(r, Acquired::Busy { .. }), "{r:?}");
        assert!(said.is_empty());
        assert_eq!(
            read(&dir.join("pid")).as_deref(),
            Some("77"),
            "the live lock is untouched"
        );
        assert!(!guard_path(&dir).exists(), "the guard is removed");
        // twin: a lock that stays dead under the guard IS moved
        let dir2 = t.path().join("two.lock");
        plant(&dir2, "o", "78", NOW - 100);
        assert!(matches!(
            go(&dir2, &clock, &Table::new(&[]), &rules()).0,
            Acquired::Held(_)
        ));
    }

    #[test]
    fn a_fresh_guard_blocks_the_takeover_and_three_attempts_give_up() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "o", "77", NOW - 100);
        let guard = guard_path(&dir);
        std::fs::create_dir(&guard).unwrap();
        set_mtime(&guard, NOW);
        let clock = FakeClock::at(NOW);
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &rules());
        assert!(matches!(r, Acquired::Cannot), "{r:?}");
        assert!(said.is_empty());
        assert_eq!(clock.now(), NOW + 3, "one second of waiting per attempt");
        assert!(dir.is_dir() && guard.is_dir(), "a fresh guard is respected");
    }

    #[test]
    fn a_guard_older_than_a_minute_belongs_to_a_dead_waiter() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        plant(&dir, "o", "77", NOW - 1000);
        let guard = guard_path(&dir);
        std::fs::create_dir(&guard).unwrap();
        set_mtime(&guard, NOW - 61);
        let clock = FakeClock::at(NOW);
        let (r, said) = go(&dir, &clock, &Table::new(&[]), &rules());
        // attempt 1 removes the old guard, attempt 2 takes over, attempt 3 takes the lock
        assert!(matches!(r, Acquired::Held(_)), "{r:?}");
        assert_eq!(said.len(), 1);
    }

    #[test]
    fn a_lock_that_cannot_be_made_is_cannot_not_busy() {
        let t = tempfile::tempdir().unwrap();
        // the parent of the lock is a file
        let f = t.path().join("file");
        std::fs::write(&f, "x").unwrap();
        let clock = FakeClock::at(NOW);
        let (r, _) = go(&f.join("wake.lock"), &clock, &Table::new(&[]), &rules());
        assert!(matches!(r, Acquired::Cannot), "{r:?}");
    }

    #[test]
    fn release_leaves_a_lock_that_names_another_process() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("wake.lock");
        let clock = FakeClock::at(NOW);
        let (r, _) = go(&dir, &clock, &Table::new(&[]), &rules());
        let Acquired::Held(h) = r else { panic!() };
        std::fs::write(dir.join("pid"), "999\n").unwrap();
        drop(h);
        assert!(dir.is_dir(), "a lock of another pid stays");
    }

    #[test]
    fn the_rules_hold_at_their_boundaries() {
        let t = tempfile::tempdir().unwrap();
        let live = Table::new(&[77]);
        let r = Rules {
            stale_after: 100,
            empty_grace: 30,
        };
        // too-old: exactly the budget is not over it, one second more is
        let dir = t.path().join("a.lock");
        plant(&dir, "o", "77", NOW - 100);
        assert_eq!(judge(&dir, &r, NOW, &live), None);
        assert_eq!(
            judge(&dir, &r, NOW + 1, &live).map(|s| s.name),
            Some("too-old")
        );
        // empty-after-grace: exactly the grace is not over it
        let empty = t.path().join("b.lock");
        std::fs::create_dir(&empty).unwrap();
        set_mtime(&empty, NOW - 30);
        assert_eq!(judge(&empty, &r, NOW, &live), None);
        assert_eq!(
            judge(&empty, &r, NOW + 1, &live).map(|s| s.name),
            Some("empty-after-grace")
        );
        // a guard of exactly 60 s stays, one of 61 s goes
        for (age, gone) in [(60, false), (61, true)] {
            let dir = t.path().join(format!("g{age}.lock"));
            plant(&dir, "o", "78", NOW - 1000);
            let guard = guard_path(&dir);
            std::fs::create_dir(&guard).unwrap();
            set_mtime(&guard, NOW - age);
            let clock = FakeClock::at(NOW);
            take_over(&dir, &r, &clock, &Table::new(&[]), &mut |_| {});
            assert_eq!(!guard.exists(), gone, "guard of {age} s");
        }
    }
}
