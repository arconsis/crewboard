//! What a message means: the ONE parser of message acts (spec/messages#acts), and the ONE place
//! that writes their text.
//!
//! The protocol has 10 `kind` values, but a ticket log carries more kinds of act than that: an
//! assignment, a reassignment, a close notice, a verify result, a late report, the marker of an
//! agent that leaves. The header has no field for them (protocol v1 stays as it is), so the act
//! exists only as the first line or as a body line of the message. Every reader asks
//! [`parse`], and every writer builds its text with the functions of this module:
//!
//! - [`ticket`]: the creation info, the assignment, the reassign notice, the park and the answer
//!   that takes a question back.
//! - [`close`]: the notices of a close or a drop.
//! - [`evidence`]: the evidence of a `done` and the verify result.
//! - [`lifecycle`]: the stop and leave notices of an agent, and the late report to the owner.
//!
//! A change of wording is then one edit, and the golden tests (`golden_tests.rs`) pin the bytes
//! of every writer and read them back with the parser, so a writer and the parser cannot drift
//! apart.
//!
//! Pure. It judges the text and the kind only: who sent the message, and whether the sender may
//! do the act, is decided by the caller from the verified sender.

pub mod close;
pub mod evidence;
pub mod lifecycle;
pub mod ticket;

#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod unit_tests;

pub use close::*;
pub use evidence::*;
pub use lifecycle::*;
pub use ticket::*;

use crate::names::Kind;

/// Why a ticket was closed, as its notice says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    /// `close --verified`: closed after a passed verify.
    Verified,
    /// `merge`: the coordinator merged the PR.
    Merged,
    /// `close`: the owner merged the PR by hand.
    OwnerMerged,
    /// `closed: <reason>`: a close by judgment of a build before Dropped existed. It ended the
    /// ticket in Done, and the log of an existing board keeps that meaning. No writer posts it
    /// now.
    Judgment,
    /// `close --why`: the coordinator dropped the ticket by judgment (`dropped: <reason>`). The
    /// ticket ends in Dropped, not in Done.
    Dropped,
}

/// The meaning of one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// `info`: the ticket is on the board (the creation message that carries `item=`).
    BoardAnnounce,
    /// `request`: the ticket is assigned (`assign`, and `reassign` to a new agent). The signed
    /// `gate:` line and the signed `ci:` line (`Some(true)` for `ci: none`, `Some(false)` for
    /// `ci: required`) are what the assignment carries; a line that is missing or unreadable is
    /// `None`.
    Assignment {
        gate: Option<String>,
        ci_none: Option<bool>,
    },
    /// `info`: the ticket went back to Ready (`reassign`).
    Reassigned,
    /// `info`: the ticket was closed (Done) or dropped (Dropped).
    Closed(CloseKind),
    /// `report`: the verify result of the coordinator.
    Verify { pass: bool },
    /// `answer`: takes an open question to the owner back.
    WithdrawsQuestion,
    /// `blocked`: the ticket is parked for the interrupting ticket `for_issue`.
    Parked { for_issue: u64 },
    /// `info`: the agent leaves (the lifecycle marker).
    Left,
    /// `info`: the agent `name` is stopping (it drains). The caller checks that `name` is the
    /// sender.
    Stopping { name: String },
    /// `info`: a late report to the owner. `key` is the `late-key:` line that makes one report
    /// per episode.
    Late { key: Option<String> },
    /// Every other message: its `kind` and its body lines are all it says.
    Other,
}

/// The act of a message with `kind` (`None`: a comment of the owner without a header) and
/// `body`. The first line decides the act, except for the acts that are a body line of their own
/// ([`WITHDRAWS_QUESTION_LINE`], [`LEFT_MARKER`]).
pub fn parse(kind: Option<Kind>, body: &str) -> Act {
    let Some(kind) = kind else {
        return Act::Other;
    };
    let first = body.lines().next().unwrap_or("").trim();
    match kind {
        Kind::Request if first.starts_with(ASSIGNED_PREFIX) => Act::Assignment {
            gate: kv(body, "gate"),
            ci_none: match kv(body, "ci").map(|v| v.to_ascii_lowercase()).as_deref() {
                Some("none") => Some(true),
                Some("required") => Some(false),
                _ => None,
            },
        },
        Kind::Info if first.starts_with(BOARD_ANNOUNCE_PREFIX) => Act::BoardAnnounce,
        Kind::Info if first.starts_with(REASSIGNED_PREFIX) => Act::Reassigned,
        Kind::Info if first.starts_with(CLOSED_VERIFIED) => Act::Closed(CloseKind::Verified),
        Kind::Info if first.starts_with(MERGED_PREFIX) => Act::Closed(CloseKind::Merged),
        Kind::Info if first.starts_with(OWNER_MERGED_PREFIX) => Act::Closed(CloseKind::OwnerMerged),
        Kind::Info if first.starts_with(CLOSED_JUDGMENT_PREFIX) => Act::Closed(CloseKind::Judgment),
        Kind::Info if first.starts_with(DROPPED_PREFIX) => Act::Closed(CloseKind::Dropped),
        Kind::Info if has_line(body, LEFT_MARKER) => Act::Left,
        Kind::Info if stopping_name(first).is_some() => Act::Stopping {
            name: stopping_name(first).unwrap_or_default().to_string(),
        },
        Kind::Info if body.starts_with(LATE_PREFIX) => Act::Late {
            key: kv(body, LATE_KEY),
        },
        Kind::Report => match first.strip_prefix(VERIFY_PREFIX).map(str::trim) {
            Some("pass") => Act::Verify { pass: true },
            Some("fail") => Act::Verify { pass: false },
            _ => Act::Other,
        },
        Kind::Answer if has_line(body, WITHDRAWS_QUESTION_LINE) => Act::WithdrawsQuestion,
        Kind::Blocked => parked_for(body).map_or(Act::Other, |for_issue| Act::Parked { for_issue }),
        _ => Act::Other,
    }
}

/// The dedicated command that writes the act of a free text, or `None` when the text is plain.
/// A writer of free text (`say`, `block --why`) refuses a body for which this is `Some`: the
/// reader would take the text for the act (a `say --kind info "closed: x"` of the coordinator
/// would close the ticket in the log). The text is judged as sent and with leading blank space
/// removed, because the receiver may see it that way.
pub fn reserved_for(kind: Kind, body: &str) -> Option<&'static str> {
    [body, body.trim_start()].into_iter().find_map(|text| {
        Some(match parse(Some(kind), text) {
            // the one act that `say --kind answer` may write on purpose (a coordinator answer
            // that takes a question back); `unblock` writes it too
            Act::Other | Act::WithdrawsQuestion => return None,
            Act::Assignment { .. } => "assign (or reassign --to)",
            Act::BoardAnnounce => "assign or ticket new",
            Act::Reassigned => "reassign",
            Act::Closed(_) => "close or merge",
            Act::Verify { .. } => "verify",
            Act::Parked { .. } => "take --interrupt",
            Act::Left | Act::Stopping { .. } => "stop",
            Act::Late { .. } => "no command: crewboard posts late reports itself",
        })
    })
}

/// Is a whole line of `body` (trimmed) `line`?
fn has_line(body: &str, line: &str) -> bool {
    body.lines().any(|l| l.trim() == line)
}

/// `key: value` of a message body line (first match, value not empty). Used for `pr`, `sha`,
/// `ci`, `verify`, `gate`, `late-key`.
pub fn kv(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key && !v.trim().is_empty()).then(|| v.trim().to_string())
    })
}

// ---- the texts that the writers post and the parser reads ------------------------------------

const ASSIGNED_PREFIX: &str = "Assigned to you";
const BOARD_ANNOUNCE_PREFIX: &str = "Ticket is on the board";
const REASSIGNED_PREFIX: &str = "Reassigned.";
const CLOSED_VERIFIED: &str = "Closed after a passed verify";
const MERGED_PREFIX: &str = "Merged by the coordinator.";
/// The first line of the notice of a close after an owner merge.
pub const OWNER_MERGED_HEAD: &str = "The ticket is closed: the owner merged the PR by hand.";
const OWNER_MERGED_PREFIX: &str = "The ticket is closed: the owner merged";
/// The first line of a close by judgment of an older build (Done). Read, never written.
const CLOSED_JUDGMENT_PREFIX: &str = "closed:";
/// The first line of a drop (`close --why`): `dropped: <reason>`.
const DROPPED_PREFIX: &str = "dropped:";
const VERIFY_PREFIX: &str = "verify:";
/// The line that a coordinator `answer` carries to take a question back.
pub const WITHDRAWS_QUESTION_LINE: &str = "withdraws: question";
/// The line of the signed `info` that an agent posts when it leaves.
pub const LEFT_MARKER: &str = "lifecycle: left";
/// What follows the agent name in the first line of the signed `info` of an agent that stops.
const STOPPING_INFIX: &str = " is stopping (draining).";
/// Start of the `blocked` message that `take --interrupt` posts.
pub const PARK_PREFIX: &str = "parked for #";
/// Start of a late report to the owner (`late: <wait> <subject>`).
const LATE_PREFIX: &str = "late: ";
/// The key of a late report: the line `late-key: <key>`.
const LATE_KEY: &str = "late-key";

/// The number in `parked for #<n>: <reason>`; `None` for any other text.
fn parked_for(body: &str) -> Option<u64> {
    let rest = body.strip_prefix(PARK_PREFIX)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(':') {
        return None;
    }
    digits.parse().ok()
}

/// The agent name of a first line that [`stopping_body`] wrote: one word before the infix.
fn stopping_name(first: &str) -> Option<&str> {
    let (name, _) = first.split_once(STOPPING_INFIX)?;
    (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(name)
}
