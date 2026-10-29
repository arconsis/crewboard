//! Ticket facts derived from the board and from signed comments
//! (spec/board#item-ids-are-the-source-of-truth, spec/tickets (ticket content to verification)).
//!
//! Pure: callers read comments and items; this module decides what they mean. The facts
//! that `rules::status::check_status_transition` needs (`answer_after_block`, `verify`,
//! `evidence_complete`) come from here, never from a flag that the caller sets.
//!
//! - [`messages`]: the verified messages of a ticket log, and what they say about questions,
//!   blocks and splits.
//! - [`evidence`]: the `done` evidence, the signed gate and CI lines, and the verify result.
//! - [`rows`]: the creation `info` of a ticket (item id, adopted PR), the board row and the
//!   events that a change of rows causes.
//! - [`queue_order`]: the queue of an agent: when `Order` alone decides, the end of a queue, the
//!   plan of the `migrate` step.
//! - [`log`]: the status that the signed log shows, against the board.

mod evidence;
pub(crate) mod log;
mod messages;
mod queue_order;
mod rows;

pub use crate::protocol::acts::{Act, kv};
pub use crate::rules::text::{LOCAL_GATE_CI, body_field, body_fields};
pub use evidence::*;
pub use messages::*;
pub use queue_order::*;
pub use rows::*;

/// The label of a ticket issue (spec/tickets#ticket-content).
pub const WORK_LABEL: &str = "crew:work";

#[cfg(test)]
mod signed_gate_tests;
#[cfg(test)]
mod split_proof_tests;
#[cfg(test)]
mod unit_tests;
