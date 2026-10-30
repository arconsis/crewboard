//! The local gate of `ci_mode: local`: the text of `--local-run` and the checks of `done` and `verify` for it.

use crate::base::ids;
use crate::error::CbResult;
use crate::rules::Refusal;

/// The longest local-gate text (bytes): the text goes into the Evidence field (1024 bytes).
const MAX_LOCAL_GATE_BYTES: usize = 400;

/// The refusal of a local-gate flag while `ci_mode` is `ci`, and of a local-gate evidence that no
/// longer counts.
pub(crate) fn local_gate_not_enabled() -> Refusal {
    Refusal::new(
        ids::LOCAL_GATE_NOT_ENABLED,
        "--local-run needs `ci_mode: local` in AUTHORITY.md on the default branch (the owner sets it); it is `ci` now. Nothing was written",
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
    crate::ops::say::check_body_text(t)?;
    Ok(t.to_string())
}

/// What `--local-run` of `done` means under the `ci_mode` of AUTHORITY.md
/// (spec/ci#local-run-ci_mode-local): in `ci` mode the flag is refused; in `local` mode it is
/// required. `Some(text)`: the gate is the evidence. Pure.
pub(super) fn local_gate_for_done(
    auth: &crate::authority::Authority,
    arg: Option<&str>,
) -> CbResult<Option<String>> {
    match (auth.ci_mode.is_local(), arg) {
        (false, Some(_)) => Err(local_gate_not_enabled().into()),
        (false, None) => Ok(None),
        (true, None) => Err(Refusal::new(
            ids::LOCAL_GATE_MISSING,
            "AUTHORITY.md has `ci_mode: local`: done needs --local-run \"<host>: <command> rc=0, log <path>\" (the CI of the head is not judged)",
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
) -> CbResult<Option<String>> {
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
    fn done_follows_ci_mode_for_the_local_gate_flag() {
        // ci mode: no flag is fine, the flag is refused
        assert_eq!(local_gate_for_done(&auth(false), None).unwrap(), None);
        assert_eq!(
            rule_of(local_gate_for_done(&auth(false), Some(GATE))),
            "local-gate-not-enabled"
        );
        // local mode: the flag is required and is the evidence
        assert_eq!(
            rule_of(local_gate_for_done(&auth(true), None)),
            "local-gate-missing"
        );
        assert_eq!(
            local_gate_for_done(&auth(true), Some(&format!("  {GATE} \n"))).unwrap(),
            Some(GATE.to_string())
        );
        assert_eq!(
            rule_of(local_gate_for_done(&auth(true), Some("  "))),
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
        let v =
            |local, arg, ev_local, fail| local_gate_for_verify(&auth(local), arg, ev_local, fail);
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
