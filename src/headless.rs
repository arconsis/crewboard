//! The headless scope (spec/agents#headless-coordinator).
//!
//! A headless coordinator is a coordinator that a scheduler wakes without a person at the
//! keyboard (`crewboard wake`). It may do two things alone: `verify`, and
//! `merge` of a ticket whose Gate is CI only. Every other command that changes the board or
//! a ticket is refused with the rule `headless-scope` (exit 2) before any config is loaded
//! and before any request is sent.
//!
//! The `watch` and `poll` cycle is allowed too. It is not read-only: it runs the coordinator
//! duties, which return the tickets of a dead agent to Backlog and post signed `info` notes.
//!
//! The scope comes from the setup, not from the prompt: the top-level key `headless = true` in
//! the agent config of the config dir that the loop uses. The key is a guardrail, not a
//! boundary: the headless user can write its own config dir. The boundary is the OS user and
//! its token.
//!
//! The model passes no flag, so it cannot leave the scope by leaving a flag out. The interactive
//! coordinator runs as another OS user, or uses another config dir without the key, and is not
//! restricted.

use crate::config::ConfigDir;
use crate::error::{CbError, CbResult};
use crate::rules::Refusal;

/// The rule name of every refusal in this module.
pub const RULE: &str = "headless-scope";

/// What a headless coordinator may do with one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeVerdict {
    /// Allowed. `merge` has more checks on the ticket (see [`check_merge_ticket`]).
    Allow,
    /// Refused, with the reason.
    Deny(&'static str),
}

pub(crate) const DENY_WRITE: &str =
    "it changes the board or a ticket; the interactive coordinator does it";
pub(crate) const DENY_OWNER: &str = "it is an owner or setup command";
pub(crate) const DENY_LIFECYCLE: &str =
    "it changes the availability of the agent; the interactive coordinator or the owner does it";
pub(crate) const DENY_GATE_FAIL: &str =
    "--fail records a failed Gate check, which is a judgment of the interactive coordinator";
pub(crate) const DENY_LOCAL_GATE: &str = "--local-run records the coordinator's own local run, which is a judgment of the interactive coordinator";
pub(crate) const DENY_CLOSE_JUDGMENT: &str =
    "close --why and close --verified are judgments of the interactive coordinator";
pub(crate) const DENY_RUN: &str =
    "--run names a CI run for a Gate that needs a by-hand run; such a Gate is not CI only";

/// The refusal for a command that the scope does not allow. `command` is the name as typed.
pub fn refusal(command: &str, why: &str) -> Refusal {
    Refusal::new(
        RULE,
        format!(
            "this config is headless: `{command}` is not allowed ({why}). A headless coordinator may only run verify, and merge for a ticket whose Gate is CI only. Leave this for the interactive coordinator and note it"
        ),
    )
}

/// Apply the verdict of a command: a denied command is refused when an agent config of the
/// config dir has `headless = true`. The CLI decides the verdict (`cli::scope`) and runs this
/// check first, before any config load or request.
pub(crate) fn enforce(
    command: &str,
    verdict: ScopeVerdict,
    config_dir: Option<&std::path::Path>,
) -> CbResult<()> {
    let ScopeVerdict::Deny(why) = verdict else {
        return Ok(());
    };
    // A config dir that cannot be found has no agent config; the command then fails by itself.
    let Ok(cd) = ConfigDir::resolve(config_dir) else {
        return Ok(());
    };
    if cd.has_headless_key()? {
        return Err(CbError::Refused(refusal(command, why)));
    }
    Ok(())
}

/// Does a scope bind this process: `headless = true` in the config dir? (A wake that the
/// scheduler started also binds it; see `lock_cmd`.)
pub fn in_scope(config_dir: Option<&std::path::Path>) -> CbResult<bool> {
    let Ok(cd) = ConfigDir::resolve(config_dir) else {
        return Ok(false);
    };
    cd.has_headless_key()
}

/// The value of the Gate line of a CI-only ticket: `Gate: ci`. `ticket new --gate-ci` writes it,
/// and the signed `gate:` line of the assignment carries it.
pub const GATE_CI: &str = "ci";

/// The Gate of a ticket as the headless coordinator reads it: the one success line of the
/// ticket (spec/agents#headless-coordinator). [`Gate::parse`] is the only parser of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// The typed CI-only Gate: the text is `ci` (any letter case, spaces around it ignored).
    Ci,
    /// Any other Gate (a command, a by-hand check, a second condition): never CI only.
    Text(String),
}

impl Gate {
    /// Parse the signed `gate:` line of the assignment (`ticket::signed_gate`; the body is never
    /// read). [`Gate::Ci`] when the trimmed text is `ci` in any letter case; any other text is
    /// [`Gate::Text`]. A signed gate that joins several `Gate:` lines of the body with ` | ` is
    /// a `Text`.
    pub fn parse(signed_gate_line: &str) -> Gate {
        if signed_gate_line.trim().eq_ignore_ascii_case(GATE_CI) {
            Gate::Ci
        } else {
            Gate::Text(signed_gate_line.to_string())
        }
    }

    /// Is the Gate CI only?
    pub fn is_ci_only(&self) -> bool {
        matches!(self, Gate::Ci)
    }
}

/// The extra check of `merge` for a headless coordinator, on the signed Gate of the ticket
/// `key` (before any read of the PR). `signed`: [`crate::ticket::signed_gate`]. `ci_none`: the
/// ticket declares `CI: none`; it has no CI to rely on and is not CI only either. No signed
/// gate means "not CI only": the issue body is no source for this judgement.
pub fn check_merge_ticket(key: &str, signed: Option<&str>, ci_none: bool) -> Result<(), Refusal> {
    let Some(gate) = signed else {
        return Err(Refusal::new(
            RULE,
            format!(
                "no signed gate line on {key}: the Gate is not CI only. Leave the merge for the interactive coordinator and note it"
            ),
        ));
    };
    let ci_only = Gate::parse(gate).is_ci_only();
    if ci_only && !ci_none {
        return Ok(());
    }
    let why = if ci_none && ci_only {
        format!("the ticket {key} declares `CI: none`: it is not CI only")
    } else {
        format!("the signed gate of {key} is '{gate}': it is not CI only")
    };
    Err(Refusal::new(
        RULE,
        format!(
            "this config is headless: merge needs a Gate that is CI only (write `Gate: ci`); {why}. Leave the merge for the interactive coordinator and note it"
        ),
    ))
}

/// A headless coordinator never acts on a ticket with an open question to the owner, also after
/// the owner answered in a chat. `verify` and `merge` run this check before any other check of
/// the ticket and before any write; the interactive coordinator verifies and merges such a
/// ticket.
pub fn check_no_open_question_headless(key: &str, open: bool) -> Result<(), Refusal> {
    if !open {
        return Ok(());
    }
    Err(Refusal::new(
        RULE,
        format!(
            "this config is headless: {key} has an open question to the owner; only the interactive coordinator acts on it. Leave it for the interactive coordinator and note it"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci_only(g: &str) -> bool {
        Gate::parse(g).is_ci_only()
    }

    #[test]
    fn gate_ci_is_the_typed_form() {
        assert_eq!(GATE_CI, "ci");
        for g in ["ci", "CI", "Ci", "  ci  ", "ci\n"] {
            assert_eq!(Gate::parse(g), Gate::Ci, "{g:?}");
        }
    }

    #[test]
    fn the_old_phrases_are_not_ci_only_any_more() {
        for g in [
            "CI only",
            "CI green",
            "CI is green",
            "green CI",
            "**CI green**",
        ] {
            assert_eq!(Gate::parse(g), Gate::Text(g.to_string()), "{g}");
            assert!(!ci_only(g), "{g}");
        }
    }

    #[test]
    fn any_other_text_is_a_text_gate_and_never_ci_only() {
        for g in [
            "CI green | cargo test passes",
            "cargo test",
            "CI green and tests ran",
            "ci and a manual test",
            "ci,",
            "",
        ] {
            assert_eq!(Gate::parse(g), Gate::Text(g.to_string()), "{g}");
            assert!(!ci_only(g), "{g}");
        }
    }

    /// What `assign` signs for a ticket body: the `gate:` line of the assignment message.
    fn signed(body: &str) -> Option<String> {
        crate::ticket::gate_line_of(body)
    }

    fn ci_only_body(body: &str) -> bool {
        signed(body).is_some_and(|g| ci_only(&g))
    }

    #[test]
    fn ci_only_gates() {
        for g in ["Gate: ci", "Gate: CI", "- Gate: ci", "gate: Ci"] {
            let body = format!("Goal: x\n{g}\nMore text\n");
            assert!(ci_only_body(&body), "{g}");
        }
    }

    #[test]
    fn gates_that_are_not_ci_only() {
        for body in [
            "Goal: x\n",                                        // no Gate line
            "Gate:\n",                                          // empty Gate
            "Gate: `cargo test` passes\n",                      // a command
            "Gate: CI green and the owner has read the diff\n", // a second condition
            "Gate: CI green\nGate: run the demo by hand\n",     // a second Gate line
            "Gate: continuous integration\n",                   // not a listed phrase
            "Gatekeeper: CI\n",                                 // not a Gate line
            "Gate: CI green on all three OS and a manual test\n",
        ] {
            assert!(!ci_only_body(body), "{body:?}");
        }
    }

    #[test]
    fn a_ticket_that_declares_ci_none_is_not_ci_only() {
        assert!(check_merge_ticket("o/r#1", Some("ci"), false).is_ok());
        assert!(check_merge_ticket("o/r#1", Some("ci"), true).is_err());
        let r = check_merge_ticket("o/r#1", Some("ci"), true).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(r.message.contains("CI: none"), "{}", r.message);
    }

    #[test]
    fn merge_refusal_names_the_signed_gate_or_its_absence() {
        assert!(check_merge_ticket("o/r#1", Some("ci"), false).is_ok());
        let r = check_merge_ticket("o/r#1", Some("demo by hand"), false).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(
            r.message
                .contains("the signed gate of o/r#1 is 'demo by hand': it is not CI only"),
            "{}",
            r.message
        );
        let r = check_merge_ticket("o/r#1", None, false).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(
            r.message
                .contains("no signed gate line on o/r#1: the Gate is not CI only"),
            "{}",
            r.message
        );
    }

    #[test]
    fn a_ticket_with_an_open_question_is_refused_and_any_other_passes() {
        let r = check_no_open_question_headless("o/r#12", true).unwrap_err();
        assert_eq!(r.rule, RULE);
        assert!(
            r.message
                .contains("o/r#12 has an open question to the owner; only the interactive coordinator acts on it"),
            "{}",
            r.message
        );
        assert!(check_no_open_question_headless("o/r#12", false).is_ok());
    }
}
