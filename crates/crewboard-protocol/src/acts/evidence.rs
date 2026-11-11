//! The evidence of a `done` message and the verify result of the coordinator. A `done` carries no
//! act of its own: [`parse_evidence`] reads its lines.

use super::verify_fail::{FAILED_KEY, VerifyFailure};
use super::{VERIFY_PREFIX, kv};

/// The `ci:` value of evidence that holds a local gate instead of a CI run.
pub const LOCAL_GATE_CI: &str = "local-gate";

/// The evidence lines of a `done` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceMsg {
    pub pr: String,
    pub sha: String,
    pub ci: String,
    /// `workflow-runs, statuses; unreadable: check-runs`: which CI sources were read.
    pub ci_sources: Option<String>,
    /// `counted: 9002 (push, success); information only: 9010 (schedule, failure)`: which CI
    /// runs counted (spec/ci#which-ci-runs-count).
    pub ci_runs: Option<String>,
    /// `host-a: make test rc=0, log /tmp/gate.log`: the local gate of the dev, when `ci` is
    /// [`LOCAL_GATE_CI`] (spec/ci#local-run-ci_mode-local).
    pub local_gate: Option<String>,
}

/// The line of a local-gate `done` that names the owner decision.
pub const LOCAL_GATE_OWNER_LINE: &str = "CI off by owner decision (AUTHORITY.md ci_mode: local)";

/// Is this evidence a local gate (no CI run)?
pub fn is_local_gate(e: &EvidenceMsg) -> bool {
    e.ci == LOCAL_GATE_CI
}

pub fn done_body(e: &EvidenceMsg, note: &str) -> String {
    let sources = e
        .ci_sources
        .as_ref()
        .map(|s| format!("ci-sources: {s}\n"))
        .unwrap_or_default();
    let runs = e
        .ci_runs
        .as_ref()
        .map(|s| format!("ci-runs: {s}\n"))
        .unwrap_or_default();
    let local = match (&e.local_gate, is_local_gate(e)) {
        (Some(t), true) => format!("local-gate: {t}\n{LOCAL_GATE_OWNER_LINE}\n"),
        _ => String::new(),
    };
    format!(
        "Done. Evidence follows.\npr: {}\nsha: {}\nci: {}\n{sources}{runs}{local}{}",
        e.pr, e.sha, e.ci, note
    )
}

pub fn parse_evidence(body: &str) -> Option<EvidenceMsg> {
    Some(EvidenceMsg {
        pr: kv(body, "pr")?,
        sha: kv(body, "sha")?,
        ci: kv(body, "ci")?,
        ci_sources: kv(body, "ci-sources"),
        ci_runs: kv(body, "ci-runs"),
        local_gate: kv(body, "local-gate"),
    })
}

/// The first line of a verify result: `verify: pass` or `verify: fail`.
pub fn verify_head(pass: bool) -> String {
    format!("{VERIFY_PREFIX} {}", if pass { "pass" } else { "fail" })
}

/// The body of a verify result (`kind=report`, `re` = the `done` message).
pub fn verify_body(
    pass: bool,
    e: &EvidenceMsg,
    head_now: &str,
    failures: &[VerifyFailure],
    note: &str,
    case: Option<&str>,
    coordinator_local_gate: Option<&str>,
) -> String {
    let mut s = format!(
        "{}\npr: {}\nsha: {}\nhead_now: {}\nci: {}\n",
        verify_head(pass),
        e.pr,
        e.sha,
        head_now,
        e.ci
    );
    if let Some(src) = &e.ci_sources {
        s.push_str(&format!("ci-sources: {src}\n"));
    }
    if let Some(runs) = &e.ci_runs {
        s.push_str(&format!("ci-runs: {runs}\n"));
    }
    // spec/ci#local-run-ci_mode-local: the gate of the dev and the check of the coordinator.
    if let (true, Some(t)) = (is_local_gate(e), &e.local_gate) {
        s.push_str(&format!("local-gate: {t}\n"));
    }
    if let Some(t) = coordinator_local_gate {
        s.push_str(&format!("coordinator-local-gate: {t}\n"));
    }
    for f in failures {
        s.push_str(&format!("{FAILED_KEY}: {f}\n"));
    }
    if let Some(c) = case {
        s.push_str(&format!("case: {c}\n"));
    }
    if !note.trim().is_empty() {
        s.push_str(&format!("note: {}\n", note.trim()));
    }
    s
}
