//! How long a ticket in Review has waited for `verify`, for `status`. The wait starts at the
//! latest signed `done` of the assignee (GitHub's creation time of that comment).
//!
//! The limit is the `verify` time of the late-work table (`[late]` in `.crewboard.toml`,
//! default 30 min, see `late::Expect`). That time is the one the initiative already set for
//! "Review, not verified"; `status` does not add a second constant. `3 x heartbeat interval`
//! was the other candidate: it is 15 min by default and says how long an agent may be silent,
//! not how long a coordinator may take to verify; a fixed hour ignores the `[late]` table.

use crate::clock::{fmt_age, format_ts};

/// The wait of one Review ticket: the time of the latest `done`, and how long ago it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewWait {
    /// Unix seconds of the latest `done`.
    pub since: i64,
    /// Seconds from `since` to now; never negative.
    pub waited: i64,
}

impl ReviewWait {
    pub fn new(since: i64, now: i64) -> ReviewWait {
        ReviewWait {
            since,
            waited: (now - since).max(0),
        }
    }

    /// Longer than `limit` seconds.
    pub fn is_over(&self, limit: i64) -> bool {
        self.waited > limit
    }

    /// The text after the ticket line: the age of the `done`, and over the limit the mark
    /// `waiting for verify since <t>`.
    pub fn text(&self, limit: i64) -> String {
        if self.is_over(limit) {
            format!(
                "  waiting for verify since {} ({})",
                format_ts(self.since),
                fmt_age(self.waited)
            )
        } else {
            format!("  done {} ago", fmt_age(self.waited))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_marked_only_over_the_limit() {
        let t0 = 1_792_000_000;
        let w = ReviewWait::new(t0, t0 + 600);
        assert!(!w.is_over(1800));
        assert_eq!(w.text(1800), "  done 10m ago");
        // exactly at the limit is not over; one second more is
        assert!(!ReviewWait::new(t0, t0 + 1800).is_over(1800));
        let late = ReviewWait::new(t0, t0 + 1801);
        assert!(late.is_over(1800));
        assert!(
            late.text(1800).starts_with("  waiting for verify since 20"),
            "{}",
            late.text(1800)
        );
        assert!(late.text(1800).ends_with("Z (30m)"), "{}", late.text(1800));
        // the board clock may run ahead of this host: never negative
        assert_eq!(ReviewWait::new(t0, t0 - 5).waited, 0);
    }
}
