//! `agent add --wait-approval`: wait for the owner to approve the agent, then run `start`.
//!
//! After the join the owner has one thing to do. `agent add` prints the exact command for the
//! owner, then runs `start` again every 15 seconds. `start` reads the registry and refuses an
//! agent that is not approved before it writes anything, so a poll writes nothing. The wait ends
//! when `start` gives another answer (done, or a failed check) or after 30 minutes. Ctrl-C ends
//! it at once; no step is half done then, and the same command run again goes on.
//!
//! This file holds the parts that need nothing of `agent add`: the two numbers, the poll loop and
//! the Ctrl-C note. The lines and the polling of `start` are in `add.rs`.

use crate::clock::Clock;
use std::time::Duration;

/// Seconds between two polls.
const INTERVAL_SECS: u64 = 15;
/// The longest wait: 30 minutes.
const MAX_SECS: u64 = 1800;

/// How often to poll and for how long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Wait {
    pub interval: Duration,
    pub max: Duration,
}

impl Wait {
    pub(super) fn new(interval: Option<u64>, max: Option<u64>) -> Self {
        Wait {
            interval: Duration::from_secs(interval.unwrap_or(INTERVAL_SECS)),
            max: Duration::from_secs(max.unwrap_or(MAX_SECS)),
        }
    }
}

/// The text after Ctrl-C. A fixed byte string: the handler may call only `write` and `_exit`.
const INTERRUPTED: &[u8] = b"agent add: interrupted while it waited for the approval. Nothing is half done: run the same command again.\n";

extern "C" fn on_interrupt(_sig: libc::c_int) {
    // SAFETY: write and _exit are async-signal-safe; the buffer is a static byte string.
    unsafe {
        libc::write(2, INTERRUPTED.as_ptr().cast(), INTERRUPTED.len());
        libc::_exit(130);
    }
}

/// Ctrl-C during the wait ends the process at once with the note above (exit 130).
pub(super) fn note_interrupt() {
    // SAFETY: the handler is an `extern "C"` function that lives for the whole process.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_interrupt as *const () as libc::sighandler_t,
        )
    };
}

/// The poll loop. `poll` runs one check (its argument is the number of the poll, from 0) and
/// gives `Some(answer)` to end the wait or `None` for "not yet". `progress` is called every
/// fourth poll with the minutes waited. The result is `None` when the time ran out. Pure but for
/// the clock: a test passes a [`Clock`] that does not sleep.
pub(super) fn poll_until<T>(
    w: &Wait,
    clock: &dyn Clock,
    mut poll: impl FnMut(u32) -> Option<T>,
    mut progress: impl FnMut(u64),
) -> Option<T> {
    let begin = clock.now();
    let max = i64::try_from(w.max.as_secs()).unwrap_or(i64::MAX);
    let mut polls = 0u32;
    loop {
        let waited = clock.now() - begin;
        if waited >= max {
            return None;
        }
        let left = Duration::from_secs(u64::try_from(max - waited).unwrap_or(0));
        clock.sleep(w.interval.min(left));
        if let Some(answer) = poll(polls) {
            return Some(answer);
        }
        polls += 1;
        if polls.is_multiple_of(4) {
            progress(u64::try_from(clock.now() - begin).unwrap_or(0) / 60);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;

    #[test]
    fn the_defaults_are_15_seconds_and_30_minutes() {
        let w = Wait::new(None, None);
        assert_eq!(w.interval, Duration::from_secs(15));
        assert_eq!(w.max, Duration::from_secs(1800));
        assert_eq!(Wait::new(Some(1), Some(2)).max, Duration::from_secs(2));
    }

    #[test]
    fn the_loop_polls_every_interval_and_ends_when_the_answer_changes() {
        let clock = FakeClock::at(1000);
        let w = Wait::new(Some(15), Some(1800));
        let mut seen = Vec::new();
        let mut minutes = Vec::new();
        let end = poll_until(
            &w,
            &clock,
            |n| {
                seen.push(n);
                (n == 5).then_some("approved")
            },
            |m| minutes.push(m),
        );
        assert_eq!(end, Some("approved"));
        assert_eq!(seen, [0, 1, 2, 3, 4, 5]);
        // one progress line after the fourth poll: 4 x 15 s = 1 minute
        assert_eq!(minutes, [1]);
        assert_eq!(clock.now(), 1000 + 6 * 15);
    }

    #[test]
    fn the_loop_gives_up_after_thirty_minutes_and_polls_120_times() {
        let clock = FakeClock::at(0);
        let w = Wait::new(None, None);
        let mut polls = 0;
        let end: Option<()> = poll_until(
            &w,
            &clock,
            |_| {
                polls += 1;
                None
            },
            |_| {},
        );
        assert_eq!(end, None);
        assert_eq!(polls, 120);
        assert_eq!(clock.now(), 1800);
    }

    #[test]
    fn an_answer_at_the_first_poll_ends_the_wait_at_once() {
        let clock = FakeClock::at(0);
        let w = Wait::new(Some(15), Some(1800));
        let end = poll_until(&w, &clock, |_| Some("failed"), |_| {});
        assert_eq!(end, Some("failed"));
        assert_eq!(clock.now(), 15);
    }
}
