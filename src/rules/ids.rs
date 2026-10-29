//! The rule ids that callers match on.
//!
//! A [`Refusal`](super::Refusal) names its rule with a `&'static str`. Most rule ids are only
//! text for a person or a test. These four also steer control flow in a caller (a command
//! turns the refusal into another message, or `tick` treats it as "leave for the interactive
//! coordinator"). A typo in a literal would compile and never match, so the producer and the
//! matcher use one constant.

/// The ticket has no `Gate` (a move to Ready needs one).
pub const STATUS_GATE_MISSING: &str = "status-gate-missing";

/// A Blocked ticket cannot be taken: no answer after the block.
pub const STATUS_ANSWER_MISSING: &str = "status-answer-missing";

/// An owner-merged or closed ticket needs a reason for `close`.
pub const CLOSE_NEEDS_REASON: &str = "close-needs-reason";

/// The ticket has an agent already (`assign` of a ticket that is assigned).
pub const TICKET_ALREADY_ASSIGNED: &str = "ticket-already-assigned";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_keep_their_wire_text() {
        // Scripts and the spec name these ids: the text must not change by accident.
        assert_eq!(STATUS_GATE_MISSING, "status-gate-missing");
        assert_eq!(STATUS_ANSWER_MISSING, "status-answer-missing");
        assert_eq!(CLOSE_NEEDS_REASON, "close-needs-reason");
        assert_eq!(TICKET_ALREADY_ASSIGNED, "ticket-already-assigned");
    }
}
