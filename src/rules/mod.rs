//! Rules as data plus pure functions (spec/board#fields, spec/board#lifecycle,
//! spec/board#coordinator-role, spec/tickets#status-flow-and-writers).
//!
//! Nothing in this module calls GitHub. A caller collects the facts (who acts, what the
//! ticket looks like) and asks a function here. The answer is `Ok(())` or a [`Refusal`]
//! that names the rule. Every rule has a test that passes and a test that is refused
//! (spec/overview#tests): a rule that cannot fail is a defect.

pub mod evidence;
pub mod fields;
pub mod ids;
pub mod lifecycle;
pub mod limits;
pub mod options;
pub mod protected;
pub mod reassign;
pub mod status;
pub mod text;

// re-export; new code imports from base
pub use crate::base::{Actor, Refusal, Role, Verdict};

/// Turn a condition into a verdict.
pub(crate) fn require(cond: bool, rule: &'static str, message: &str) -> Verdict {
    if cond {
        Ok(())
    } else {
        Err(Refusal::new(rule, message))
    }
}
