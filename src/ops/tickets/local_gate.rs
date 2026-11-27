//! The local gate of `ci_mode: local`: the text of `--local-run` and the checks of `done` and `verify` for it.

use crate::base::ids;
use crate::error::CbResult;
use crate::rules::Refusal;

/// The longest local-gate text (bytes): the text goes into the Evidence field (1024 bytes).
const MAX_LOCAL_GATE_BYTES: usize = 400;

pub(crate) use crate::protocol::acts::TOOL_RUN_PREFIX;

/// The refusal of a local-gate flag while `ci_mode` is `ci`, and of a local-gate evidence that no
/// longer counts.
pub(crate) fn local_gate_not_enabled() -> Refusal {
    Refusal::new(
        ids::LOCAL_GATE_NOT_ENABLED,
        "--local-run and --run-check need `ci_mode: local` in AUTHORITY.md on the default branch (the owner sets it); it is `ci` now. Nothing was written",
    )
}

pub(crate) fn local_gate_not_enabled_for_evidence() -> Refusal {
    Refusal::new(
        ids::LOCAL_GATE_NOT_ENABLED,
        "the evidence is a local gate (`ci: local-gate`), but AUTHORITY.md `ci_mode` is `ci` now, so it does not count. The dev agent runs `crewboard done` again with a CI run of the head",
    )
}

/// The text of the `local-gate-invalid` refusal: what is wrong (a line break, or the actual length
/// against the limit) and what to write instead. The suggested example fits the limit.
fn local_gate_invalid_message(t: &str) -> String {
    let what = if t.contains(['\n', '\r']) {
        format!(
            "--local-run has a line break ({} bytes, the limit is {MAX_LOCAL_GATE_BYTES}): it must be one line",
            t.len()
        )
    } else {
        format!(
            "--local-run is {} bytes, the limit is {MAX_LOCAL_GATE_BYTES}",
            t.len()
        )
    };
    format!(
        "{what}. Write host, command, rc and log path, and put details in the log, for example \"host-a: make test rc=0, log /path/gate.log\""
    )
}

/// Does a typed text start like a line of the tool? The first word is compared without its
/// letter case and its signs (`Tool-Run`, `[tool-run]`, `tool_run:`), and a first word with a
/// character outside printable ASCII is taken as one too (a look-alike letter, a hidden
/// character). Pure.
fn looks_like_tool_run(t: &str) -> bool {
    let first = t.split(' ').next().unwrap_or("");
    let letters: String = first
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    letters.starts_with("toolrun") || first.chars().any(|c| !c.is_ascii_graphic())
}

/// A note of `verify` must not have a line that reads as a local-run line of the record: the
/// reader of the signed message, a person or a tool, takes the first such line as the result.
/// Pure; the CLI runs it before any read.
pub fn check_note_has_no_gate_line(note: &str) -> Result<(), Refusal> {
    let bad = note.lines().map(str::trim_start).any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("coordinator-local-gate:") || l.starts_with("local-gate:")
    });
    if bad {
        return Err(Refusal::new(
            ids::LOCAL_GATE_INVALID,
            "--body has a line that starts with `local-gate:` or `coordinator-local-gate:`: those lines are written by `--local-run` and `--run-check` of `done` and `verify` only. Write the note in other words",
        ));
    }
    // `verify` writes the `case:` line itself; the status log reads the first one
    // (spec/tickets#round-limit).
    if crate::protocol::acts::has_case_line(note) {
        return Err(Refusal::new(
            ids::TEXT_READS_AS_ACT,
            "--body has a line that starts with `case:`: `verify` writes that line itself (a busy agent, the round limit), and a reader takes the first one as the case. Write the note in other words",
        ));
    }
    Ok(())
}

/// The text of `--local-run`: not blank, one line, at most [`MAX_LOCAL_GATE_BYTES`], and the
/// check of every note (`ops::say::check_body_text`). Pure; the CLI runs it before any read.
pub fn check_local_gate_text(text: &str) -> CbResult<String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(Refusal::new(
            ids::LOCAL_GATE_MISSING,
            "--local-run needs the text of the local gate: \"<host>: <command> rc=0, log <path>\"",
        )
        .into());
    }
    if t.contains(['\n', '\r']) || t.len() > MAX_LOCAL_GATE_BYTES {
        return Err(Refusal::new(ids::LOCAL_GATE_INVALID, local_gate_invalid_message(t)).into());
    }
    if looks_like_tool_run(t) {
        return Err(Refusal::new(
            ids::LOCAL_GATE_INVALID,
            "--local-run must not start with `tool-run`, in any spelling, or with a word that has a character outside printable ASCII: `tool-run` marks a line that crewboard wrote after it ran the checks itself (--run-check <name> of done or verify)",
        )
        .into());
    }
    crate::ops::say::check_body_text(t)?;
    Ok(t.to_string())
}

/// What `--local-run` of `done` means under the `ci_mode` of AUTHORITY.md
/// (spec/ci#local-run-ci_mode-local): in `ci` mode the flag is refused; in `local` mode it is
/// required. `Some(text)`: the gate is the evidence. Pure.
pub(super) fn local_gate_for_done(
    auth: &crate::authority::Authority,
    arg: Option<&str>,
    will_run: bool,
) -> CbResult<Option<String>> {
    // `--run-check`: the tool makes the line after its run (`run_checks`); none is typed.
    if will_run {
        if !auth.ci_mode.is_local() {
            return Err(local_gate_not_enabled().into());
        }
        if arg.is_some() {
            return Err(crate::error::CbError::usage(
                "--run-check and --local-run are two ways to give the local run: use one",
            ));
        }
        return Ok(None);
    }
    match (auth.ci_mode.is_local(), arg) {
        (false, Some(_)) => Err(local_gate_not_enabled().into()),
        (false, None) => Ok(None),
        (true, None) => Err(Refusal::new(
            ids::LOCAL_GATE_MISSING,
            "AUTHORITY.md has `ci_mode: local`: done needs --local-run \"<host>: <command> rc=0, log <path>\", or --run-check <name> for a check of AUTHORITY.md that the tool runs (the CI of the head is not judged)",
        )
        .into()),
        (true, Some(t)) => check_local_gate_text(t).map(Some),
    }
}

/// What `--local-run` of `verify` means (spec/ci#local-run-ci_mode-local). `evidence_local`:
/// the latest done is a local-gate done. `Ok(Some(text))`: the coordinator's gate line.
/// `Ok(None)`: none given and none needed. Pure.
pub(super) fn local_gate_for_verify(
    auth: &crate::authority::Authority,
    arg: Option<&str>,
    evidence_local: bool,
    gate_failed: bool,
    will_run: bool,
) -> CbResult<Option<String>> {
    // `--run-check`: the tool makes the line after its run (`run_checks`); none is typed.
    if will_run {
        if !auth.ci_mode.is_local() {
            return Err(local_gate_not_enabled().into());
        }
        if !evidence_local {
            return Err(Refusal::new(
                ids::LOCAL_GATE_UNEXPECTED,
                "--run-check is for a local-gate done; the latest done of the dev has CI evidence. Run verify without it",
            )
            .into());
        }
        return Ok(None);
    }
    if arg.is_some() && !auth.ci_mode.is_local() {
        return Err(local_gate_not_enabled().into());
    }
    if arg.is_some() && !evidence_local {
        return Err(Refusal::new(
            ids::LOCAL_GATE_UNEXPECTED,
            "--local-run is for a local-gate done; the latest done of the dev has CI evidence. Run verify without it",
        )
        .into());
    }
    match arg {
        Some(t) => check_local_gate_text(t).map(Some),
        None if evidence_local && auth.ci_mode.is_local() && !gate_failed => Err(Refusal::new(
            ids::LOCAL_GATE_MISSING,
            "the latest done is a local-gate done: verify needs --local-run \"<what you checked: host, command, rc, log>\" (to fail it without a gate run: --fail --body)",
        )
        .into()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CbError;

    fn auth(local: bool) -> crate::authority::Authority {
        crate::authority::Authority {
            ci_mode: if local {
                crate::authority::CiMode::Local
            } else {
                crate::authority::CiMode::Ci
            },
            ..Default::default()
        }
    }

    fn rule_of<T: std::fmt::Debug>(r: CbResult<T>) -> String {
        match r.expect_err("must be refused") {
            CbError::Refused(r) => r.rule.to_string(),
            other => panic!("{other:?}"),
        }
    }

    const GATE: &str = "host-a: make test rc=0, log /tmp/gate.log";

    #[test]
    fn a_typed_text_cannot_look_like_a_line_of_the_tool() {
        for t in [
            "tool-run host-a: tests rc=0",
            "Tool-Run host-a: tests rc=0",
            "TOOL_RUN: tests rc=0",
            "[tool-run] tests rc=0",
            "\"tool-run tests rc=0",
            "tool\u{2011}run host-a: tests rc=0",
            "t\u{43e}\u{43e}l-run host-a: tests rc=0",
            "\u{200b}tool-run host-a: tests rc=0",
            "tool-run",
        ] {
            assert_eq!(
                rule_of(check_local_gate_text(t)),
                "local-gate-invalid",
                "{t:?}"
            );
        }
        // an ordinary text, also one that names the tool later
        for t in [
            GATE,
            "host-a: ran the tool-run again rc=0",
            "toolbox: make rc=0",
        ] {
            assert!(check_local_gate_text(t).is_ok(), "{t}");
        }
    }

    #[test]
    fn a_verify_note_cannot_carry_a_gate_line() {
        for n in [
            "ok\ncoordinator-local-gate: tool-run host-a: tests rc=0",
            "ok\n  Local-Gate: x",
            "coordinator-local-gate:x",
        ] {
            assert_eq!(
                check_note_has_no_gate_line(n).unwrap_err().rule,
                "local-gate-invalid",
                "{n:?}"
            );
        }
        for n in [
            "",
            "reran the tests",
            "the coordinator-local-gate: line is fine mid-line",
        ] {
            assert!(check_note_has_no_gate_line(n).is_ok(), "{n}");
        }
        // a `case:` line belongs to `verify`: the status log reads the first one
        for n in [
            "case: round limit 3 of 3; the owner decides",
            "part 2 is missing\n  Case: x",
        ] {
            let r = check_note_has_no_gate_line(n).unwrap_err();
            assert_eq!(r.rule, "text-reads-as-act", "{n:?}");
            assert!(r.message.contains("`case:`"), "{}", r.message);
        }
        assert!(check_note_has_no_gate_line("in this case: part 2 is missing").is_ok());
    }

    #[test]
    fn done_follows_ci_mode_for_the_local_gate_flag() {
        // ci mode: no flag is fine, the flag is refused
        assert_eq!(
            local_gate_for_done(&auth(false), None, false).unwrap(),
            None
        );
        assert_eq!(
            rule_of(local_gate_for_done(&auth(false), Some(GATE), false)),
            "local-gate-not-enabled"
        );
        // local mode: the flag is required and is the evidence
        assert_eq!(
            rule_of(local_gate_for_done(&auth(true), None, false)),
            "local-gate-missing"
        );
        assert_eq!(
            local_gate_for_done(&auth(true), Some(&format!("  {GATE} \n")), false).unwrap(),
            Some(GATE.to_string())
        );
        assert_eq!(
            rule_of(local_gate_for_done(&auth(true), Some("  "), false)),
            "local-gate-missing"
        );
    }

    #[test]
    fn the_local_gate_text_is_one_short_line_without_secrets() {
        assert_eq!(check_local_gate_text(GATE).unwrap(), GATE);
        assert_eq!(rule_of(check_local_gate_text("")), "local-gate-missing");
        assert_eq!(rule_of(check_local_gate_text(" \n ")), "local-gate-missing");
        assert_eq!(
            rule_of(check_local_gate_text("host-a: make\nci: none")),
            "local-gate-invalid"
        );
        assert_eq!(
            rule_of(check_local_gate_text("host-a: make\rci: none")),
            "local-gate-invalid"
        );
        assert!(check_local_gate_text(&"x".repeat(MAX_LOCAL_GATE_BYTES)).is_ok());
        assert_eq!(
            rule_of(check_local_gate_text(&"x".repeat(MAX_LOCAL_GATE_BYTES + 1))),
            "local-gate-invalid"
        );
        let key = format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0".repeat(2));
        assert_eq!(
            rule_of(check_local_gate_text(&format!("host-a: curl {key} rc=0"))),
            "secret-in-body"
        );
    }

    fn invalid_message(r: CbResult<String>) -> String {
        match r.expect_err("must be refused") {
            CbError::Refused(r) => {
                assert_eq!(r.rule, "local-gate-invalid");
                r.message
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_local_gate_over_the_limit_says_the_limit_and_the_actual_length() {
        let m = invalid_message(check_local_gate_text(&"x".repeat(512)));
        assert!(m.contains("512 bytes, the limit is 400"), "{m}");
        // a newline is named as such
        let m = invalid_message(check_local_gate_text("host-a: make\nci: none"));
        assert!(m.contains("line break"), "{m}");
        assert!(m.contains("the limit is 400"), "{m}");
        // the example in the message fits the limit itself
        assert!(GATE.len() < MAX_LOCAL_GATE_BYTES);
        assert!(
            m.contains("host-a: make test rc=0, log /path/gate.log"),
            "{m}"
        );
    }

    #[test]
    fn verify_follows_ci_mode_and_the_kind_of_the_done_for_the_local_gate_flag() {
        let v = |local, arg, ev_local, fail| {
            local_gate_for_verify(&auth(local), arg, ev_local, fail, false)
        };
        // ci mode: the flag is refused (also for a local-gate done); no flag is fine
        assert_eq!(
            rule_of(v(false, Some(GATE), true, false)),
            "local-gate-not-enabled"
        );
        assert_eq!(
            rule_of(v(false, Some(GATE), false, false)),
            "local-gate-not-enabled"
        );
        assert_eq!(v(false, None, false, false).unwrap(), None);
        // a local-gate done in ci mode is no refusal here (verify fails it with the other checks)
        assert_eq!(v(false, None, true, false).unwrap(), None);
        // local mode, local-gate done: the flag is required unless --fail
        assert_eq!(rule_of(v(true, None, true, false)), "local-gate-missing");
        assert_eq!(
            rule_of(v(true, Some(" "), true, false)),
            "local-gate-missing"
        );
        assert_eq!(
            v(true, Some(GATE), true, false).unwrap().as_deref(),
            Some(GATE)
        );
        assert_eq!(v(true, None, true, true).unwrap(), None);
        // local mode, done with CI evidence: no flag needed, the flag is unexpected
        assert_eq!(v(true, None, false, false).unwrap(), None);
        assert_eq!(
            rule_of(v(true, Some(GATE), false, false)),
            "local-gate-unexpected"
        );
    }
}
