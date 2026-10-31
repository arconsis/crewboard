//! The replay window (spec/messages#signature, guides/security#threats-that-crewboard-handles).
//!
//! The canonical form binds the issue, the sender, the receiver, the kind, `re`, `ts` and the
//! body hash, but it has no nonce. A holder of the agent token can post a copy of an old valid
//! message as a new comment. The copy keeps its old `ts`, and GitHub gives it a new
//! `created_at`. So a message is fresh only when its signed `ts` is close to the GitHub
//! creation time of the comment that carries it.
//!
//! The window alone lets a copy through for 15 minutes. So a second rule refuses a copy
//! inside the window: a later comment on the issue that carries the same signature of the same
//! sender as an earlier valid comment ([`Seen`]).
//!
//! Pure: the times and the signatures are arguments.

use std::collections::HashMap;
use std::fmt;

/// A message may be signed this long before GitHub created the comment (a slow or retried
/// post).
pub const MAX_AGE_SECS: i64 = 15 * 60;

/// A message may be signed this long after the GitHub creation time (clock skew between the
/// host of the agent and GitHub).
pub const MAX_AHEAD_SECS: i64 = 5 * 60;

/// A copy is created at most this long after the original: the copy is signed at most
/// [`MAX_AGE_SECS`] before its creation, and the original is created at most [`MAX_AHEAD_SECS`]
/// before its own `ts`. A reader that wants to see the original of every fresh comment reads
/// this far back.
pub const COPY_LOOKBACK_SECS: i64 = MAX_AGE_SECS + MAX_AHEAD_SECS;

/// A signed `ts` outside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// Signed more than [`MAX_AGE_SECS`] before the comment was created: an old message that
    /// was posted again.
    TooOld { by_secs: i64 },
    /// Signed more than [`MAX_AHEAD_SECS`] after the comment was created: a wrong clock or a
    /// forged time.
    FromFuture { by_secs: i64 },
    /// Inside the window, but the same sender and signature are on the earlier comment `of`
    /// already: a copy of a message that was posted before.
    Copy { of: u64 },
}

impl fmt::Display for Replay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Replay::TooOld { by_secs } => write!(
                f,
                "signed {} before the comment was created (limit {}); a copy of an older message?",
                span(*by_secs),
                span(MAX_AGE_SECS)
            ),
            Replay::Copy { of } => write!(
                f,
                "same signature as the earlier comment {of}; a message counts once per issue"
            ),
            Replay::FromFuture { by_secs } => write!(
                f,
                "signed {} after the comment was created (limit {}); check the clock of the sender",
                span(*by_secs),
                span(MAX_AHEAD_SECS)
            ),
        }
    }
}

fn span(secs: i64) -> String {
    if secs >= 120 && secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// Is `ts` (the signed time, Unix seconds) inside `[created - 15 min, created + 5 min]`, with
/// `created` the GitHub `created_at` of the comment? Both limits are inclusive.
pub fn check_window(ts: i64, created: i64) -> Result<(), Replay> {
    let age = created - ts;
    if age > MAX_AGE_SECS {
        Err(Replay::TooOld { by_secs: age })
    } else if -age > MAX_AHEAD_SECS {
        Err(Replay::FromFuture { by_secs: -age })
    } else {
        Ok(())
    }
}

/// The signatures that valid comments of one issue carried so far (the copy rule). Feed it
/// the valid comments in the order of their ids: the first comment with a signature counts, a
/// later one with the same sender and signature is a copy. Record only a comment that passed
/// the signature and the window, so a refused comment never hides a later genuine one.
#[derive(Debug, Default, Clone)]
pub struct Seen {
    first: HashMap<(String, String), u64>,
}

impl Seen {
    /// Is the comment `id` with this sender and signature the first of its kind? Then it is
    /// recorded and `Ok`. Else `Err(Replay::Copy { of })` names the earlier comment.
    pub fn first_use(&mut self, from: &str, sig: &str, id: u64) -> Result<(), Replay> {
        match self.first.entry((from.to_string(), sig.to_string())) {
            std::collections::hash_map::Entry::Occupied(e) if *e.get() != id => {
                Err(Replay::Copy { of: *e.get() })
            }
            std::collections::hash_map::Entry::Occupied(_) => Ok(()),
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(id);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATED: i64 = 1_792_072_800;

    #[test]
    fn the_limits_themselves_are_fresh() {
        assert_eq!(check_window(CREATED - 900, CREATED), Ok(()));
        assert_eq!(check_window(CREATED + 300, CREATED), Ok(()));
    }

    #[test]
    fn one_second_past_a_limit_is_refused() {
        assert_eq!(
            check_window(CREATED - 901, CREATED),
            Err(Replay::TooOld { by_secs: 901 })
        );
        assert_eq!(
            check_window(CREATED + 301, CREATED),
            Err(Replay::FromFuture { by_secs: 301 })
        );
    }

    #[test]
    fn a_message_signed_at_the_creation_time_or_a_little_before_is_fresh() {
        for d in [0, 1, 30, 300, 899] {
            assert_eq!(check_window(CREATED - d, CREATED), Ok(()), "{d}");
        }
        for d in [1, 30, 299] {
            assert_eq!(check_window(CREATED + d, CREATED), Ok(()), "{d}");
        }
    }

    #[test]
    fn a_copy_of_an_old_message_is_too_old_by_the_time_since_signing() {
        // signed a day ago, posted again now
        assert_eq!(
            check_window(CREATED - 86_400, CREATED),
            Err(Replay::TooOld { by_secs: 86_400 })
        );
    }

    #[test]
    fn the_texts_name_the_limit_and_the_distance() {
        let old = Replay::TooOld { by_secs: 3_600 }.to_string();
        assert!(
            old.contains("60 min before") && old.contains("limit 15 min"),
            "{old}"
        );
        let fut = Replay::FromFuture { by_secs: 301 }.to_string();
        assert!(
            fut.contains("301 s after") && fut.contains("limit 5 min"),
            "{fut}"
        );
    }

    #[test]
    fn a_second_comment_with_the_same_sender_and_signature_is_a_copy() {
        let mut seen = Seen::default();
        assert_eq!(seen.first_use("a@h", "SIG1", 10), Ok(()));
        assert_eq!(
            seen.first_use("a@h", "SIG1", 14),
            Err(Replay::Copy { of: 10 })
        );
        // the same comment twice is not a copy of itself
        assert_eq!(seen.first_use("a@h", "SIG1", 10), Ok(()));
        // another signature or another sender is a different message
        assert_eq!(seen.first_use("a@h", "SIG2", 15), Ok(()));
        assert_eq!(seen.first_use("b@h", "SIG1", 16), Ok(()));
    }

    #[test]
    fn the_copy_text_names_the_earlier_comment() {
        let t = Replay::Copy { of: 101 }.to_string();
        assert!(t.contains("earlier comment 101"), "{t}");
    }

    #[test]
    fn the_lookback_covers_the_window_and_the_skew() {
        assert_eq!(COPY_LOOKBACK_SECS, 1200);
    }
}
