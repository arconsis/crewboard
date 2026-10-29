//! Watch cursor (spec/events#watch-semantics), stored in the `Cursor` field of the agent item.
//!
//! The comment feed is read with `since=<updated_at>`. Two comments can share a second,
//! and a feed can lag a little, so the cursor is a timestamp plus the comments already
//! handled near it. A read starts `OVERLAP_SECS` before the timestamp and skips every
//! comment that is in the `seen` list. A comment that was edited has a new `updated_at`,
//! so it is a new entry (and is reported as edited).
//!
//! Text form (at most 1024 bytes, spec/board#fields):
//! `c1 ts=2026-10-15T14:05:00Z seen=123.1792073100,124.1792073161`
//!
//! A comment older than `ts - OVERLAP_SECS` that shows up late in a feed is not seen
//! again. That is the documented limit of a time-based cursor.

use crate::clock::{format_ts, parse_ts};
use crate::rules::limits::MAX_TEXT_FIELD_BYTES;
use std::fmt;

pub const OVERLAP_SECS: i64 = 60;
/// Entries are about 22 bytes each; 36 fit with room to spare under 1024 bytes.
pub const MAX_SEEN: usize = 36;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub id: u64,
    pub updated: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub ts: i64,
    pub seen: Vec<Seen>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorError(pub String);

impl fmt::Display for CursorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad cursor: {}", self.0)
    }
}

impl std::error::Error for CursorError {}

impl Cursor {
    /// A cursor that starts at `now`: no history before it.
    pub fn at(now: i64) -> Self {
        Cursor {
            ts: now,
            seen: Vec::new(),
        }
    }

    pub fn encode(&self) -> String {
        let seen: Vec<String> = self
            .seen
            .iter()
            .map(|s| format!("{}.{}", s.id, s.updated))
            .collect();
        format!("c1 ts={} seen={}", format_ts(self.ts), seen.join(","))
    }

    pub fn decode(s: &str) -> Result<Self, CursorError> {
        let err =
            |m: &str| CursorError(format!("{m}: '{}'", s.chars().take(80).collect::<String>()));
        let mut parts = s.split(' ');
        if parts.next() != Some("c1") {
            return Err(err("not a c1 cursor"));
        }
        let ts = parts
            .next()
            .and_then(|p| p.strip_prefix("ts="))
            .and_then(parse_ts)
            .ok_or_else(|| err("missing or bad ts"))?;
        let seen_raw = parts
            .next()
            .and_then(|p| p.strip_prefix("seen="))
            .ok_or_else(|| err("missing seen"))?;
        if parts.next().is_some() {
            return Err(err("trailing data"));
        }
        let mut seen = Vec::new();
        if !seen_raw.is_empty() {
            for e in seen_raw.split(',') {
                let (id, upd) = e.split_once('.').ok_or_else(|| err("bad seen entry"))?;
                seen.push(Seen {
                    id: id.parse().map_err(|_| err("bad seen id"))?,
                    updated: upd.parse().map_err(|_| err("bad seen time"))?,
                });
            }
        }
        Ok(Cursor { ts, seen })
    }

    /// The `since` of the next feed read.
    pub fn since(&self) -> i64 {
        self.ts - OVERLAP_SECS
    }

    /// Is this comment version new for this cursor?
    pub fn is_fresh(&self, id: u64, updated: i64) -> bool {
        updated >= self.since() && !self.seen.iter().any(|s| s.id == id && s.updated == updated)
    }

    /// The cursor after the given comment versions were handled (printed, dropped or
    /// refused). Never moves backwards.
    pub fn advance(&self, handled: &[(u64, i64)]) -> Cursor {
        let ts = handled
            .iter()
            .map(|h| h.1)
            .max()
            .map_or(self.ts, |m| m.max(self.ts));
        let mut seen = self.seen.clone();
        for &(id, updated) in handled {
            if !seen.iter().any(|s| s.id == id && s.updated == updated) {
                seen.push(Seen { id, updated });
            }
        }
        let floor = ts - 2 * OVERLAP_SECS;
        seen.retain(|s| s.updated >= floor);
        seen.sort_by_key(|s| (s.updated, s.id));
        if seen.len() > MAX_SEEN {
            let drop = seen.len() - MAX_SEEN;
            seen.drain(..drop);
        }
        let c = Cursor { ts, seen };
        debug_assert!(c.encode().len() <= MAX_TEXT_FIELD_BYTES);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_792_073_100;

    #[test]
    fn encode_decode_round_trip() {
        let c = Cursor {
            ts: T,
            seen: vec![
                Seen {
                    id: 5,
                    updated: T - 3,
                },
                Seen { id: 9, updated: T },
            ],
        };
        let text = c.encode();
        assert_eq!(
            text,
            format!("c1 ts=2026-10-15T14:05:00Z seen=5.{},9.{}", T - 3, T)
        );
        assert_eq!(Cursor::decode(&text).unwrap(), c);
        let empty = Cursor::at(T);
        assert_eq!(Cursor::decode(&empty.encode()).unwrap(), empty);
    }

    #[test]
    fn bad_cursors_are_refused() {
        for bad in [
            "",
            "c2 ts=2026-10-15T14:05:00Z seen=",
            "c1 ts=yesterday seen=",
            "c1 seen=",
            "c1 ts=2026-10-15T14:05:00Z",
            "c1 ts=2026-10-15T14:05:00Z seen=abc",
            "c1 ts=2026-10-15T14:05:00Z seen=1.x",
            "c1 ts=2026-10-15T14:05:00Z seen= extra",
        ] {
            assert!(Cursor::decode(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fresh_and_seen_comments() {
        let c = Cursor {
            ts: T,
            seen: vec![Seen {
                id: 7,
                updated: T - 5,
            }],
        };
        // new comment: fresh
        assert!(c.is_fresh(8, T + 1));
        // already handled: not fresh (the proof that the filter can say "seen")
        assert!(!c.is_fresh(7, T - 5));
        // the same comment edited later has a new version: fresh again
        assert!(c.is_fresh(7, T + 10));
        // inside the overlap window and not seen: fresh
        assert!(c.is_fresh(11, T - 30));
        // older than the window: not fresh
        assert!(!c.is_fresh(12, T - OVERLAP_SECS - 1));
    }

    #[test]
    fn advance_moves_forward_only_and_remembers_what_was_handled() {
        let c = Cursor::at(T);
        let n = c.advance(&[(1, T + 5), (2, T + 8)]);
        assert_eq!(n.ts, T + 8);
        assert!(!n.is_fresh(1, T + 5));
        assert!(!n.is_fresh(2, T + 8));
        // handling an older comment does not move the cursor back
        let m = n.advance(&[(3, T - 20)]);
        assert_eq!(m.ts, T + 8);
        // nothing handled: unchanged
        assert_eq!(n.advance(&[]), n);
    }

    #[test]
    fn seen_list_is_pruned_and_capped_and_stays_under_1024_bytes() {
        let mut c = Cursor::at(T);
        let many: Vec<(u64, i64)> = (0..200)
            .map(|i| (2_000_000_000 + i as u64, T + 1))
            .collect();
        c = c.advance(&many);
        assert_eq!(c.seen.len(), MAX_SEEN);
        assert!(
            c.encode().len() <= MAX_TEXT_FIELD_BYTES,
            "{}",
            c.encode().len()
        );
        // far in the future: old entries fall out of the window
        let later = c.advance(&[(1, T + 10_000)]);
        assert_eq!(later.seen.len(), 1);
        assert!(crate::rules::limits::check_text_field(&later.encode()).is_ok());
    }
}
