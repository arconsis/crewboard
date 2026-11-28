//! The headless scope (spec/agents#headless-coordinator).
//!
//! A headless coordinator is a coordinator that a scheduler wakes without a person at the
//! keyboard (`crewboard wake`). It may do two things alone: `verify`, and
//! `merge` of a ticket whose Gate is CI only. Every other command that changes the board or
//! a ticket is refused with the rule `headless-scope` (exit 2) before any config is loaded
//! and before any request is sent.
//!
//! With `unattended_coordinator: verify-and-merge` in AUTHORITY.md, `merge` also takes a ticket
//! whose Gate is prose, when the latest `done` has a passing verify result for the head of its
//! evidence ([`MergeJudged`], [`check_merge_ticket`]). Nothing else of the merge changes.
//!
//! A few commands are judgments that the unattended coordinator may make when AUTHORITY.md
//! allows it (spec/agents#unattended-coordinator): [`ScopeVerdict::Unattended`]. The scope lets
//! such a command pass, and the command decides after it loaded its context: the mode, the
//! budget of the day and the signed log of the ticket. With the mode `off` it is refused with
//! `unattended-off`, before any write.
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

use crate::base::ids;
use crate::config::ConfigDir;
use crate::error::{CbError, CbResult};
use crate::rules::Refusal;

/// What a headless coordinator may do with one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeVerdict {
    /// Allowed. `merge` has more checks on the ticket (see [`check_merge_ticket`]).
    Allow,
    /// Refused, with the reason.
    Deny(&'static str),
    /// A judgment of the unattended coordinator (spec/agents#unattended-coordinator). The scope
    /// lets it pass; the command itself decides after it loaded its context: the mode of
    /// AUTHORITY.md, the budget of the day and the signed log of the ticket
    /// (`ops::unattended::admit` and `check_ticket`).
    Unattended(UAct),
}

/// What the unattended coordinator may do with one command, when its mode allows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UAct {
    /// `verify`: a verify result on a ticket whose Gate is not the typed `ci`, and
    /// `verify --fail`. The command decides which of them counts: a plain `verify` of a ticket
    /// whose signed Gate is `ci` is no judgment and needs no mode.
    Verdict,
    /// `say --kind answer --to <agent>`.
    Answer,
    /// `unblock`.
    Unblock,
    /// `assign --to <agent>`.
    Assign,
    /// `ask`.
    Ask,
}

impl UAct {
    /// The words of the act in a refusal.
    pub fn words(self) -> &'static str {
        match self {
            UAct::Verdict => "a verify result on a Gate that is not `ci`",
            UAct::Answer => "an answer to the assignee",
            UAct::Unblock => "unblock",
            UAct::Assign => "assign",
            UAct::Ask => "ask",
        }
    }
}

pub(crate) const DENY_WRITE: &str =
    "it changes the board or a ticket; the interactive coordinator does it";
pub(crate) const DENY_OWNER: &str = "it is an owner or setup command";
pub(crate) const DENY_LIFECYCLE: &str =
    "it changes the availability of the agent; the interactive coordinator or the owner does it";
pub(crate) const DENY_LOCAL_GATE: &str = "--local-run records the coordinator's own local run, which is a judgment of the interactive coordinator";
pub(crate) const DENY_CLOSE_JUDGMENT: &str =
    "close --why and close --verified are judgments of the interactive coordinator";
pub(crate) const DENY_RUN: &str =
    "--run names a CI run for a Gate that needs a by-hand run; such a Gate is not CI only";
pub(crate) const DENY_RUN_CHECK: &str = "--run-check runs code of the PR on this host; the `tick` runs the checks of a ticket whose signed Gate is `checks: ...`, and the interactive coordinator runs any other";
pub(crate) const DENY_BODY_FILE: &str = "--body-file takes no file path in a headless config dir; pass the text with --body, or on standard input with `--body-file -`";
pub(crate) const DENY_ASSIGN_ARGS: &str = "the unattended coordinator assigns with `assign <n> --to <agent>` only; `--order` and `--override-capabilities` are for the interactive coordinator";

/// The refusal for a ticket or a text that the unattended coordinator must leave alone. `what`
/// is a full clause, for example "o/r#5 has an open question to the owner".
pub fn unattended_refusal(what: &str) -> Refusal {
    Refusal::new(
        ids::HEADLESS_SCOPE,
        format!(
            "this config is headless: {what}. Leave this for the interactive coordinator and note it; nothing was written"
        ),
    )
}

/// The refusal for a command that the scope does not allow. `command` is the name as typed.
pub fn refusal(command: &str, why: &str) -> Refusal {
    Refusal::new(
        ids::HEADLESS_SCOPE,
        format!(
            "this config is headless: `{command}` is not allowed here ({why}). Leave this for the interactive coordinator and note it; nothing was written"
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
    // `Allow` passes, and so does `Unattended`: the command decides after its context load.
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

/// Is this process a child of a wake: `CREWBOARD_WAKE` is set and not empty? The wake sets it
/// for the model of `--runtime` (a command of the test seam `--wake-cmd` does not get it). A
/// runtime that gives the model a full shell can clear the variable, so a check on it is a
/// guardrail against a mistake, not a boundary.
///
/// A unit test never is a wake child: the test suite can run inside a wake (a dev agent runs
/// the gate in its wake), and the tests of this rule pass the fact as a value.
pub fn in_wake_child() -> bool {
    !cfg!(test) && std::env::var_os("CREWBOARD_WAKE").is_some_and(|v| !v.is_empty())
}

/// What a wake child must not run (spec/agents#the-model-wake). `wake` is the fact of
/// [`in_wake_child`], `headless` the one of [`in_scope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeChild {
    /// The run form of `wake`: a second wake with its own count file, flags and time budget.
    Wake,
    /// `agent exec` and `agent with-env` in a headless config dir: they run any command.
    Shell,
    /// `heartbeat` and `start` in a headless config dir: they write the coordinator heartbeat.
    Heartbeat,
    /// `wake install` and `wake uninstall` for a headless config dir: they change the units of
    /// the scheduler.
    Units,
}

/// The refusal for a wake child, or `None` when the command may run.
pub fn wake_child_refusal(what: WakeChild, wake: bool, headless: bool) -> Option<Refusal> {
    if !wake {
        return None;
    }
    match what {
        WakeChild::Wake => Some(Refusal::new(
            ids::WAKE_INSIDE_WAKE,
            "`crewboard wake` does not run inside a wake (CREWBOARD_WAKE is set): only the scheduler starts a wake, with the options of its unit. Nothing was started",
        )),
        WakeChild::Shell if headless => Some(unattended_refusal(
            "`agent exec` and `agent with-env` run any command, and the model of a wake does not get a shell through crewboard",
        )),
        WakeChild::Heartbeat if headless => Some(unattended_refusal(
            "the model of a wake writes no heartbeat of the coordinator: the heartbeat says that the interactive coordinator is there",
        )),
        WakeChild::Units if headless => Some(unattended_refusal(
            "`wake install` and `wake uninstall` change the units of the scheduler, and the model of a wake does not do that",
        )),
        WakeChild::Shell | WakeChild::Heartbeat | WakeChild::Units => None,
    }
}

/// The value of the Gate line of a CI-only ticket: `Gate: ci`. `ticket new --gate-ci` writes it,
/// and the signed `gate:` line of the assignment carries it.
pub const GATE_CI: &str = "ci";

/// The key of the Gate line of a ticket whose Gate is a list of checks: `Gate: checks: a, b`.
pub const GATE_CHECKS: &str = "checks";

/// The Gate of a ticket as the headless coordinator reads it: the one success line of the
/// ticket (spec/agents#headless-coordinator). [`Gate::parse`] is the only parser of the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// The typed CI-only Gate: the text is `ci` (any letter case, spaces around it ignored).
    Ci,
    /// The typed Gate of named checks: `checks: <name>, <name>`. The names are checks of
    /// AUTHORITY.md that `verify --run-check` runs (spec/ci#checks-that-the-tool-runs). The
    /// headless coordinator verifies such a ticket; it does not merge it.
    Checks(Vec<String>),
    /// Any other Gate (a command, a by-hand check, a second condition): never CI only.
    Text(String),
}

impl Gate {
    /// Parse the signed `gate:` line of the assignment (`ticket::signed_gate`; the body is never
    /// read). [`Gate::Ci`] when the trimmed text is `ci` in any letter case; any other text is
    /// [`Gate::Text`]. A signed gate that joins several `Gate:` lines of the body with ` | ` is
    /// a `Text`.
    pub fn parse(signed_gate_line: &str) -> Gate {
        let text = signed_gate_line.trim();
        if text.eq_ignore_ascii_case(GATE_CI) {
            return Gate::Ci;
        }
        match Self::check_names(text) {
            Some(names) => Gate::Checks(names),
            None => Gate::Text(signed_gate_line.to_string()),
        }
    }

    /// The names of `checks: a, b`: the key in any letter case, then one name or more, each of
    /// lower-case letters, digits and `-`, no name twice. Any other text is `None`: a Gate with
    /// one more word is prose, and a person reads it.
    fn check_names(text: &str) -> Option<Vec<String>> {
        let (key, rest) = text.split_once(':')?;
        if !key.trim().eq_ignore_ascii_case(GATE_CHECKS) {
            return None;
        }
        let mut names: Vec<String> = Vec::new();
        for n in rest.split(',').map(str::trim) {
            let ok = !n.is_empty()
                && n.len() <= 32
                && !n.starts_with('-')
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !ok || names.iter().any(|x| x == n) {
                return None;
            }
            names.push(n.to_string());
        }
        Some(names)
    }

    /// Is the Gate CI only?
    pub fn is_ci_only(&self) -> bool {
        matches!(self, Gate::Ci)
    }
}

/// What the merge mode of the unattended coordinator says about one ticket
/// (spec/agents#unattended-coordinator). The caller decides it from AUTHORITY.md and the signed
/// log at merge time (`unattended::pass_on_head`), never from a list or a board field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeJudged {
    /// `unattended_coordinator` is not `verify-and-merge`: only a Gate `ci` is merged.
    ModeOff,
    /// The mode is `verify-and-merge`, and the latest `done` has no passing verify result of
    /// the unattended coordinator for the head of its evidence. A passing result of an attended
    /// coordinator is no such result: that coordinator or a person merges after it.
    NoPass,
    /// The mode is `verify-and-merge` and the signed log has such a result, but the log may miss
    /// a message that ends it. The clause says why (`unattended::Held`).
    Held(String),
    /// The mode is `verify-and-merge`, and the latest `done` has such a result.
    Pass,
}

/// The extra check of `merge` for a headless coordinator, on the signed Gate of the ticket
/// `key` (before any read of the PR). `signed`: [`crate::ticket::signed_gate`]. `ci_none`: the
/// ticket declares `CI: none`; it has no CI to rely on and is not CI only either. No signed
/// gate means "not CI only": the issue body is no source for this judgement.
///
/// `judged` widens the check for one kind of Gate only: a Gate in prose ([`Gate::Text`]) passes
/// with [`MergeJudged::Pass`]. A Gate `checks: ...` is as it was (a person merges it), and so
/// are a ticket without a signed gate and a ticket that declares `CI: none`.
pub fn check_merge_ticket(
    key: &str,
    signed: Option<&str>,
    ci_none: bool,
    judged: &MergeJudged,
) -> Result<(), Refusal> {
    let Some(gate) = signed else {
        return Err(Refusal::new(
            ids::HEADLESS_SCOPE,
            format!(
                "no signed gate line on {key}: the Gate is not CI only. Leave the merge for the interactive coordinator and note it"
            ),
        ));
    };
    let parsed = Gate::parse(gate);
    let ci_only = parsed.is_ci_only();
    if ci_only && !ci_none {
        return Ok(());
    }
    if matches!(parsed, Gate::Text(_)) && *judged != MergeJudged::ModeOff {
        if ci_none {
            return Err(Refusal::new(
                ids::HEADLESS_SCOPE,
                format!(
                    "this config is headless: the ticket {key} declares `CI: none`, so no CI run proves the change. Leave the merge for the interactive coordinator and note it"
                ),
            ));
        }
        return match judged {
            MergeJudged::Pass => Ok(()),
            MergeJudged::Held(why) => Err(Refusal::new(
                ids::HEADLESS_SCOPE,
                format!(
                    "this config is headless: the Gate of {key} is not CI only, and {why}. The interactive coordinator reads the ticket and merges it; leave the merge and note it"
                ),
            )),
            _ => Err(Refusal::new(
                ids::HEADLESS_SCOPE,
                format!(
                    "this config is headless: the Gate of {key} is not CI only, and its latest done has no passing verify result of the unattended coordinator for the SHA of its evidence (a later done, a failed result, a stop at the round limit, a result for another head or a result without the line `unattended:` leaves none). `unattended_coordinator: verify-and-merge` merges such a ticket only after that result; after a verify of an attended coordinator, that coordinator merges. Leave the merge for the interactive coordinator and note it"
                ),
            )),
        };
    }
    let why = if ci_none && ci_only {
        format!("the ticket {key} declares `CI: none`: it is not CI only")
    } else {
        format!("the signed gate of {key} is '{gate}': it is not CI only")
    };
    Err(Refusal::new(
        ids::HEADLESS_SCOPE,
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
        ids::HEADLESS_SCOPE,
        format!(
            "this config is headless: {key} has an open question to the owner; only the interactive coordinator acts on it. Leave it for the interactive coordinator and note it"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wake_child_runs_no_wake_and_in_a_headless_dir_no_shell_and_no_heartbeat() {
        let rule = |w, wake, headless| wake_child_refusal(w, wake, headless).map(|r| r.rule);
        // outside a wake nothing is refused here
        for w in [
            WakeChild::Wake,
            WakeChild::Shell,
            WakeChild::Heartbeat,
            WakeChild::Units,
        ] {
            for headless in [false, true] {
                assert_eq!(rule(w, false, headless), None, "{w:?}");
            }
        }
        // `wake` inside a wake: in every config dir
        assert_eq!(
            rule(WakeChild::Wake, true, false),
            Some(ids::WAKE_INSIDE_WAKE)
        );
        assert_eq!(
            rule(WakeChild::Wake, true, true),
            Some(ids::WAKE_INSIDE_WAKE)
        );
        // the shell commands and the heartbeat: in a headless config dir only (the wake of a
        // dev agent uses the gh shim, which runs `agent with-env`, and writes its heartbeat)
        for w in [WakeChild::Shell, WakeChild::Heartbeat, WakeChild::Units] {
            assert_eq!(rule(w, true, false), None, "{w:?}");
            assert_eq!(rule(w, true, true), Some(ids::HEADLESS_SCOPE), "{w:?}");
        }
    }

    fn ci_only(g: &str) -> bool {
        Gate::parse(g).is_ci_only()
    }

    const OFF: MergeJudged = MergeJudged::ModeOff;

    #[test]
    fn gate_ci_is_the_typed_form() {
        assert_eq!(GATE_CI, "ci");
        for g in ["ci", "CI", "Ci", "  ci  ", "ci\n"] {
            assert_eq!(Gate::parse(g), Gate::Ci, "{g:?}");
        }
    }

    #[test]
    fn a_gate_of_named_checks_is_typed_and_any_other_word_makes_it_prose() {
        let names = |v: &[&str]| Gate::Checks(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(
            Gate::parse("checks: ara, private"),
            names(&["ara", "private"])
        );
        assert_eq!(Gate::parse("  Checks:tests \n"), names(&["tests"]));
        for g in [
            "checks:",
            "checks: ara,",
            "checks: ara, ara",
            "checks: ara and the page is unchanged",
            "checks: Ara",
            "checks: ara; private",
            "the checks: ara",
            "check: ara",
        ] {
            assert_eq!(Gate::parse(g), Gate::Text(g.to_string()), "{g}");
        }
        assert!(!Gate::parse("checks: ara").is_ci_only());
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
        assert!(check_merge_ticket("o/r#1", Some("ci"), false, &OFF).is_ok());
        assert!(check_merge_ticket("o/r#1", Some("ci"), true, &OFF).is_err());
        let r = check_merge_ticket("o/r#1", Some("ci"), true, &OFF).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(r.message.contains("CI: none"), "{}", r.message);
    }

    #[test]
    fn merge_refusal_names_the_signed_gate_or_its_absence() {
        assert!(check_merge_ticket("o/r#1", Some("ci"), false, &OFF).is_ok());
        let r = check_merge_ticket("o/r#1", Some("demo by hand"), false, &OFF).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(
            r.message
                .contains("the signed gate of o/r#1 is 'demo by hand': it is not CI only"),
            "{}",
            r.message
        );
        let r = check_merge_ticket("o/r#1", None, false, &OFF).unwrap_err();
        assert_eq!(r.rule, "headless-scope");
        assert!(
            r.message
                .contains("no signed gate line on o/r#1: the Gate is not CI only"),
            "{}",
            r.message
        );
    }

    #[test]
    fn the_merge_mode_widens_the_check_for_a_prose_gate_with_a_pass_and_for_nothing_else() {
        use MergeJudged::{Held, NoPass, Pass};
        let rule = |gate: Option<&str>, ci_none: bool, j: MergeJudged| {
            check_merge_ticket("o/r#1", gate, ci_none, &j).map_err(|r| {
                assert_eq!(r.rule, ids::HEADLESS_SCOPE);
                r.message
            })
        };
        // the one case that the mode adds
        assert_eq!(rule(Some("the demo runs"), false, Pass), Ok(()));
        // no pass, and the mode off with a pass that the caller did not even look for
        let e = rule(Some("the demo runs"), false, NoPass).unwrap_err();
        assert!(
            e.contains("no passing verify result of the unattended coordinator"),
            "{e}"
        );
        // a log that may miss a message: the clause of the caller, and the way out
        let e = rule(
            Some("the demo runs"),
            false,
            MergeJudged::Held("comment 9 is odd".into()),
        )
        .unwrap_err();
        assert!(e.contains("comment 9 is odd"), "{e}");
        assert!(
            e.contains("The interactive coordinator reads the ticket"),
            "{e}"
        );
        assert!(rule(Some("checks: ara"), false, MergeJudged::Held("x".into())).is_err());
        let e = rule(Some("the demo runs"), false, OFF).unwrap_err();
        assert!(e.contains("it is not CI only"), "{e}");
        // `CI: none`, a Gate of checks and no signed gate stay refused with a pass
        let e = rule(Some("the demo runs"), true, Pass).unwrap_err();
        assert!(e.contains("declares `CI: none`"), "{e}");
        let e = rule(Some("checks: ara"), false, Pass).unwrap_err();
        assert!(e.contains("it is not CI only"), "{e}");
        let e = rule(None, false, Pass).unwrap_err();
        assert!(e.contains("no signed gate line"), "{e}");
        // a Gate `ci` needs no pass, and `CI: none` refuses it in every mode
        for j in [OFF, NoPass, Pass, Held("x".into())] {
            assert_eq!(rule(Some("ci"), false, j.clone()), Ok(()));
            assert!(rule(Some("ci"), true, j).is_err());
        }
    }

    #[test]
    fn a_ticket_with_an_open_question_is_refused_and_any_other_passes() {
        let r = check_no_open_question_headless("o/r#12", true).unwrap_err();
        assert_eq!(r.rule, ids::HEADLESS_SCOPE);
        assert!(
            r.message
                .contains("o/r#12 has an open question to the owner; only the interactive coordinator acts on it"),
            "{}",
            r.message
        );
        assert!(check_no_open_question_headless("o/r#12", false).is_ok());
    }
}
