use super::*;
use crate::comments::Comment;
use crate::config::BoardConfig;
use crate::protocol::names::{Kind, Re};
use crate::registry::{CREW_PREFIX, Trust};
use std::collections::BTreeMap;

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

/// The text of the `Evidence` board field.
pub fn evidence_field(e: &EvidenceMsg) -> String {
    if let (true, Some(t)) = (is_local_gate(e), &e.local_gate) {
        return format!("pr={} sha={} ci={} local-gate={t}", e.pr, e.sha, e.ci);
    }
    match &e.ci_sources {
        Some(s) => format!("pr={} sha={} ci={} ci-sources={s}", e.pr, e.sha, e.ci),
        None => format!("pr={} sha={} ci={}", e.pr, e.sha, e.ci),
    }
}

/// The evidence of the latest `done` message of `assignee`. A later `done` replaces an earlier
/// one (after a failed verify the dev delivers again), even when the later one has no parsable
/// evidence: then there is none (`None`), which `verify` refuses. That is fail closed on purpose.
pub fn latest_done<'a>(msgs: &'a [VMsg], assignee: &str) -> Option<(&'a VMsg, EvidenceMsg)> {
    msgs.iter()
        .rev()
        .filter(|m| m.kind == Some(Kind::Done))
        .find(|m| matches!(&m.sender, Sender::Agent { name, .. } if name == assignee))
        .and_then(|m| parse_evidence(&m.body).map(|e| (m, e)))
}

/// The latest assignment message that carries a signed `gate:` line: a coordinator assignment
/// (`assign` writes the line, `reassign` copies it forward; an assignment of an older binary has
/// none and is skipped). A message of another sender or kind, and a dev message with a `gate:`
/// line, are ignored (spec/tickets#ticket-content). Pure.
pub fn signed_assignment(msgs: &[VMsg]) -> Option<&VMsg> {
    msgs.iter().rev().find(|m| {
        m.sender.actor().is_coordinator()
            && matches!(m.act(), Act::Assignment { gate: Some(_), .. })
    })
}

/// The text that `assign` signs as the `gate:` line: every `Gate:` line of the body, joined with
/// ` | ` (one line gives that line). A body with a second Gate line is therefore not "CI only".
/// `None`: the body has no Gate line.
pub fn gate_line_of(body: &str) -> Option<String> {
    let gates = body_fields(body, "Gate");
    (!gates.is_empty()).then(|| gates.join(" | "))
}

/// The Gate that the coordinator signed in the latest assignment message. The issue body is
/// not signed: any holder of a crew token can edit it, so a judgement that must hold (the
/// headless "CI only") reads this line and not the body. `None`: no signed gate.
pub fn signed_gate(msgs: &[VMsg]) -> Option<String> {
    match signed_assignment(msgs)?.act() {
        Act::Assignment { gate, .. } => gate,
        _ => None,
    }
}

/// What the latest assignment message signed about CI: `Some(true)` for the line `ci: none`,
/// `Some(false)` for `ci: required`, `None` for an assignment of an older binary (no `ci:` line)
/// or no assignment at all.
pub fn signed_ci_none(msgs: &[VMsg]) -> Option<bool> {
    match signed_assignment(msgs)?.act() {
        Act::Assignment { ci_none, .. } => ci_none,
        _ => None,
    }
}

/// Does the ticket declare `CI: none`? The signed `ci:` line of the assignment decides (a body
/// edit after the assignment changes nothing). A ticket assigned before the signed line existed
/// has none: the live body decides then.
pub fn ci_none_declared(msgs: &[VMsg], body: &str) -> bool {
    signed_ci_none(msgs).unwrap_or_else(|| declares_ci_none(body))
}

/// The body of a verify result (`kind=report`, `re` = the `done` message).
pub fn verify_body(
    pass: bool,
    e: &EvidenceMsg,
    head_now: &str,
    failures: &[String],
    note: &str,
    case: Option<&str>,
    coordinator_local_gate: Option<&str>,
) -> String {
    let mut s = format!(
        "{}\npr: {}\nsha: {}\nhead_now: {}\nci: {}\n",
        crate::protocol::acts::verify_head(pass),
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
        s.push_str(&format!("failed: {f}\n"));
    }
    if let Some(c) = case {
        s.push_str(&format!("case: {c}\n"));
    }
    if !note.trim().is_empty() {
        s.push_str(&format!("note: {}\n", note.trim()));
    }
    s
}

/// The patch that a `done` message names with its line `owner-route: <patch>`: the first word
/// after the label, on the first such line. The ONE parser of that line (an indented line
/// counts). A line without a word names nothing.
pub fn owner_route_patch(body: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let rest = l.trim_start().strip_prefix("owner-route:")?;
        Some(rest.split_whitespace().next()?.to_string())
    })
}

/// A `done` that names the owner route (`owner-route:`) or a protected path (`protected-paths:`).
/// `verify` refuses such a ticket by design: the owner merges the PR.
pub fn is_owner_route_done(done: &VMsg) -> bool {
    owner_route_patch(&done.body).is_some()
        || done
            .body
            .lines()
            .any(|l| l.trim_start().starts_with("protected-paths:"))
}

/// `Some(true)` / `Some(false)`: the coordinator's verify result for exactly this `done`
/// message. `None`: no verify yet. A result for an earlier `done` does not count (`re`).
pub fn verify_result(msgs: &[VMsg], done: &VMsg, issue: u64) -> Option<bool> {
    verify_report(msgs, done, issue).map(|(_, pass)| pass)
}

/// The latest verify result of the coordinator for exactly this `done` message, with the
/// result (`true` = pass). `mine` reads its `head_now:` line.
pub fn verify_report<'a>(msgs: &'a [VMsg], done: &VMsg, issue: u64) -> Option<(&'a VMsg, bool)> {
    msgs.iter()
        .rev()
        .filter(|m| m.sender.actor().is_coordinator() && m.kind == Some(Kind::Report))
        .filter(|m| {
            m.re == Re::Comment {
                issue,
                comment: done.comment_id,
            }
        })
        .find_map(|m| match m.act() {
            Act::Verify { pass } => Some((m, pass)),
            _ => None,
        })
}

/// The coordinator's local-gate line of a PASSED verify of exactly this `done` message, when the
/// verify is on the head of the evidence (its `sha:` and `head_now:` lines are both
/// `ev.sha`). Merge relies on it (spec/ci#local-run-ci_mode-local). `None`: no such verify.
pub fn passed_coordinator_local_gate(
    msgs: &[VMsg],
    done: &VMsg,
    ev: &EvidenceMsg,
    issue: u64,
) -> Option<String> {
    let (m, pass) = verify_report(msgs, done, issue)?;
    let same = |k: &str| kv(&m.body, k).is_some_and(|v| v.eq_ignore_ascii_case(&ev.sha));
    (pass && same("sha") && same("head_now"))
        .then(|| kv(&m.body, "coordinator-local-gate"))
        .flatten()
}

/// The item ids that the signed `info` comments of the coordinator in `comments` announce,
/// by `owner/repo#n`. A new ticket is found this way (spec/board#item-ids-are-the-source-of-truth:
/// item ids are the source of truth), never through the item listing, which lags behind writes.
pub fn announced_item_ids(
    comments: &[Comment],
    cfg: &BoardConfig,
    trust: &Trust,
) -> BTreeMap<String, String> {
    let mut by_issue: BTreeMap<(String, u64), Vec<Comment>> = BTreeMap::new();
    for c in comments {
        if c.body.starts_with(CREW_PREFIX) && c.body.contains("item=PVTI_") {
            by_issue
                .entry((c.repo.clone(), c.issue))
                .or_default()
                .push(c.clone());
        }
    }
    by_issue
        .into_iter()
        .filter_map(|((repo, n), cs)| {
            find_item_id(&verified_messages(&cs, cfg, trust)).map(|id| (format!("{repo}#{n}"), id))
        })
        .collect()
}

/// True if `assignee` delivered evidence (the latest `done` with complete evidence) and the
/// coordinator has no verify result for exactly that message yet. `resume` lists these.
pub fn needs_verify(msgs: &[VMsg], assignee: &str, issue: u64) -> bool {
    latest_done(msgs, assignee).is_some_and(|(d, _)| verify_result(msgs, d, issue).is_none())
}

/// The marker that allows `ci: none` (an owner decision): a ticket body line
/// `CI: none` (also `**CI:** none`, a list item, any letter case, a trailing dot).
pub fn declares_ci_none(body: &str) -> bool {
    body_field(body, "CI")
        .is_some_and(|v| v.trim_end_matches('.').trim().eq_ignore_ascii_case("none"))
}
