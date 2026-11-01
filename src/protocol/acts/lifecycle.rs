//! The notices of an agent: it stops (drains), it leaves, and the late report to the owner.

use super::{LATE_KEY, LATE_PREFIX, LEFT_MARKER, STOPPING_INFIX};

/// The body of the marker that an agent posts when it leaves.
pub fn left_body(name: &str) -> String {
    format!("{name} left\n{LEFT_MARKER}")
}

/// The body of the signed notice of an agent that starts to drain. `open` lists its open tickets
/// (the coordinator returns them).
pub fn stopping_body(name: &str, open: &str) -> String {
    format!("{name}{STOPPING_INFIX} Open tickets that the coordinator returns to Backlog: {open}")
}

/// The first two lines of a late report to the owner: `late: <wait> <subject>` and
/// `late-key: <key>`, each with its final newline. The writer of the report adds the text for a
/// person after them.
pub fn late_head(wait: &str, subject: &str, key: &str) -> String {
    format!("{LATE_PREFIX}{wait} {subject}\n{LATE_KEY}: {key}\n")
}
