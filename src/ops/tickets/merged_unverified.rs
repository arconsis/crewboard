//! A ticket PR that was merged without a passed `verify` of crewboard.
//!
//! Anyone with write access can merge a crew PR outside crewboard. Crewboard does not refuse
//! that and does not undo it: `close` still ends the ticket. This module only
//! makes the gap visible.
//!
//! The gap: the PR of a ticket is MERGED and no `verify: pass` of a coordinator names the head of
//! that PR (`head_now:`). The coordinator then posts ONE signed `info` to the coordinator on the
//! ticket (lines `pr:`, `merge-commit:`, `merged-by:`, `head:`, `last-gate:`). It posts once per
//! (ticket, merge commit): an earlier note with the same `merge-commit:` line stops a repeat.
//! `crewboard status` marks a ticket with such a note, and a Review ticket whose
//! merged PR shows the gap, with [`MARKER`].
//!
//! The judgement is pure ([`gap`]); the reads are the PR (one GET, or none when the caller has
//! read it already) and the comments of the ticket (loaded anyway).

use super::post;
use super::pr_read::{MergedPr, read_pr};
use crate::error::CbResult;
use crate::ops::AgentCtx;
use crate::ops::Ctx;
use crate::protocol::names::{Kind, Re, To};
use crate::rules::evidence::parse_pr_url;
use crate::rules::status::Status;
use crate::services::ticket_loader::Ticket;
use crate::ticket::{Act, Sender, kv, parse_evidence};
use crate::ticket::{VMsg, is_local_gate, latest_done};

/// The short marker that `crewboard status` prints.
pub const MARKER: &str = "merged without verify";

/// The first line of the note (it starts with [`MARKER`]).
const NOTE_HEAD: &str =
    "merged without verify: no passed verify of crewboard names the head of the merged PR.";

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Does a `verify: pass` of a coordinator name `head` (its `head_now:` line)?
pub fn passed_verify_on_head(msgs: &[VMsg], head: &str) -> bool {
    msgs.iter()
        .filter(|m| m.sender.actor().is_coordinator() && m.kind == Some(Kind::Report))
        .any(|m| {
            m.act() == Act::Verify { pass: true }
                && kv(&m.body, "head_now").is_some_and(|h| same(&h, head))
        })
}

/// The latest gate result on the ticket, for any head: a verify result of a coordinator or a
/// local-gate line of a `done`, with its head. `none` when the ticket has neither.
pub fn last_gate(msgs: &[VMsg]) -> String {
    let mut last: Option<String> = None;
    for m in msgs {
        if m.sender.actor().is_coordinator() && m.kind == Some(Kind::Report) {
            let v = match m.act() {
                Act::Verify { pass: true } => "pass",
                Act::Verify { pass: false } => "fail",
                _ => continue,
            };
            let head = kv(&m.body, "head_now")
                .or_else(|| kv(&m.body, "sha"))
                .unwrap_or_else(|| "unknown".into());
            let mut s = format!("verify {v}, head {head}");
            if let Some(g) = kv(&m.body, "coordinator-local-gate") {
                s.push_str(&format!(", coordinator-local-gate {g}"));
            }
            last = Some(s);
        } else if m.kind == Some(Kind::Done)
            && matches!(m.sender, Sender::Agent { .. })
            && let Some(e) = parse_evidence(&m.body)
            && let Some(g) = e.local_gate.as_deref().filter(|_| is_local_gate(&e))
        {
            last = Some(format!("local-gate {g}, head {}", e.sha));
        }
    }
    last.unwrap_or_else(|| "none".into())
}

/// Did the coordinator note this merge commit already?
pub fn noted(msgs: &[VMsg], merge_commit: &str) -> bool {
    msgs.iter().any(|m| {
        m.sender.actor().is_coordinator()
            && m.kind == Some(Kind::Info)
            && m.body.starts_with(MARKER)
            && kv(&m.body, "merge-commit").is_some_and(|c| same(&c, merge_commit))
    })
}

/// Does the ticket carry a note of this kind (for any merge commit)?
pub fn has_note(msgs: &[VMsg]) -> bool {
    msgs.iter().any(|m| {
        m.sender.actor().is_coordinator()
            && m.kind == Some(Kind::Info)
            && m.body.starts_with(MARKER)
    })
}

/// The gap: the PR is merged and no passed verify names its head. A PR whose head GitHub does not
/// name counts as a gap, too (nothing can have verified it).
pub fn gap(msgs: &[VMsg], pr: &MergedPr) -> bool {
    pr.merged
        && !pr
            .head_sha
            .as_deref()
            .is_some_and(|h| passed_verify_on_head(msgs, h))
}

/// The text of the note.
pub fn note_body(pr: &MergedPr, last_gate: &str) -> String {
    format!(
        "{NOTE_HEAD}\npr: {}\nmerge-commit: {}\nmerged-by: {}\nhead: {}\nlast-gate: {last_gate}\n",
        pr.url(),
        pr.merge_commit.as_deref().unwrap_or("unknown"),
        pr.merged_by.as_deref().unwrap_or("unknown"),
        pr.head_sha.as_deref().unwrap_or("unknown"),
    )
}

/// The note to post, if any: there is a gap and this merge commit is not noted yet.
pub fn note_to_post(msgs: &[VMsg], pr: &MergedPr) -> Option<String> {
    if !gap(msgs, pr) {
        return None;
    }
    let commit = pr.merge_commit.as_deref().unwrap_or("unknown");
    (!noted(msgs, commit)).then(|| note_body(pr, &last_gate(msgs)))
}

/// Post the note for a PR that the caller has read (the coordinator only). Returns one line, or
/// nothing when there is no gap or the note exists.
pub(super) fn note_loaded(ac: &AgentCtx, t: &Ticket, pr: &MergedPr) -> CbResult<String> {
    let Some(body) = note_to_post(&t.msgs, pr) else {
        return Ok(String::new());
    };
    let p = post(ac, t, To::Coordinator, Kind::Info, Re::None, &body)?;
    Ok(format!(
        "{}: {MARKER}, PR {} (merge commit {}); note {} ({})\n",
        t.key(),
        pr.url(),
        pr.merge_commit.as_deref().unwrap_or("unknown"),
        p.id,
        p.url
    ))
}

/// The PR of the latest signed `done` of the assignee, when it is in a repo of the initiative.
/// One GET.
pub(super) fn done_pr(ac: &AgentCtx, t: &Ticket) -> CbResult<Option<MergedPr>> {
    let Some((_, ev)) = t.agent().and_then(|a| latest_done(&t.msgs, a)) else {
        return Ok(None);
    };
    let Ok(u) = parse_pr_url(&ev.pr) else {
        return Ok(None);
    };
    if !ac
        .ctx
        .cfg
        .repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&u.repo))
    {
        return Ok(None);
    }
    read_pr(&ac.ctx, &u.repo, u.number)
}

/// The tick's check of one loaded ticket: read the PR of its latest `done` and post the note when
/// it is merged without a passed verify. A ticket without a merged PR costs the one GET.
pub(super) fn check_loaded(ac: &AgentCtx, t: &Ticket) -> CbResult<String> {
    match done_pr(ac, t)? {
        Some(pr) if pr.merged => note_loaded(ac, t, &pr),
        _ => Ok(String::new()),
    }
}

/// Does `crewboard status` mark this ticket? A note on the ticket marks it without any read.
/// Else a ticket that is not Done is judged from the PR of its latest `done` (one GET).
pub fn status_marked(ctx: &Ctx, status: Option<&str>, agent: Option<&str>, msgs: &[VMsg]) -> bool {
    if has_note(msgs) {
        return true;
    }
    if !Status::Review.is(status) {
        return false;
    }
    let Some((_, ev)) = agent.and_then(|a| latest_done(msgs, a)) else {
        return false;
    };
    let Ok(u) = parse_pr_url(&ev.pr) else {
        return false;
    };
    if !ctx
        .cfg
        .repos
        .iter()
        .any(|r| r.eq_ignore_ascii_case(&u.repo))
    {
        return false;
    }
    matches!(read_pr(ctx, &u.repo, u.number), Ok(Some(pr)) if gap(msgs, &pr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Role;

    fn msg(id: u64, sender: Sender, kind: Kind, body: &str) -> VMsg {
        crate::testkit::msg(id, sender, "coordinator", Some(kind), body)
    }

    fn coord() -> Sender {
        Sender::Agent {
            name: "c@h".into(),
            role: Role::Coordinator,
        }
    }

    fn dev() -> Sender {
        Sender::Agent {
            name: "d@h".into(),
            role: Role::Dev,
        }
    }

    fn verify(id: u64, res: &str, head: &str) -> VMsg {
        msg(
            id,
            coord(),
            Kind::Report,
            &format!("verify: {res}\npr: p\nsha: {head}\nhead_now: {head}\nci: x\n"),
        )
    }

    fn pr(head: Option<&str>) -> MergedPr {
        MergedPr {
            repo: "o/r".into(),
            number: 9,
            merged: true,
            body: String::new(),
            merged_by: Some("someone".into()),
            merge_commit: Some("mc1".into()),
            head_sha: head.map(str::to_string),
            base: None,
        }
    }

    #[test]
    fn a_passed_verify_on_the_head_is_no_gap_and_any_other_case_is() {
        let h = "aaa";
        assert!(gap(&[], &pr(Some(h))));
        assert!(!gap(&[verify(1, "pass", h)], &pr(Some(h))));
        // the head letter case does not matter
        assert!(!gap(&[verify(1, "pass", "AAA")], &pr(Some(h))));
        // a failed verify, a pass on another head
        assert!(gap(&[verify(1, "fail", h)], &pr(Some(h))));
        assert!(gap(&[verify(1, "pass", "bbb")], &pr(Some(h))));
        // a pass written by a dev agent or the owner is no verify of the coordinator
        let fake = msg(2, dev(), Kind::Report, "verify: pass\nhead_now: aaa\n");
        assert!(gap(&[fake], &pr(Some(h))));
        // an unknown head is a gap
        assert!(gap(&[verify(1, "pass", h)], &pr(None)));
        // a PR that is not merged is no gap
        let mut open = pr(Some(h));
        open.merged = false;
        assert!(!gap(&[], &open));
    }

    #[test]
    fn the_last_gate_is_the_latest_verify_or_local_gate_for_any_head() {
        assert_eq!(last_gate(&[]), "none");
        let done = |id, g: &str, sha: &str| {
            msg(
                id,
                dev(),
                Kind::Done,
                &format!("Done.\npr: p\nsha: {sha}\nci: local-gate\nlocal-gate: {g}\n"),
            )
        };
        let d = done(1, "host-a: make test rc=1", "s1");
        assert_eq!(
            last_gate(std::slice::from_ref(&d)),
            "local-gate host-a: make test rc=1, head s1"
        );
        let v = verify(2, "fail", "s2");
        assert_eq!(last_gate(&[d.clone(), v.clone()]), "verify fail, head s2");
        // the order of the messages decides
        let d2 = done(3, "host-a: make test rc=0", "s3");
        assert_eq!(
            last_gate(&[d, v, d2]),
            "local-gate host-a: make test rc=0, head s3"
        );
        // a done without a local gate is no gate line
        let plain = msg(4, dev(), Kind::Done, "Done.\npr: p\nsha: s\nci: success\n");
        assert_eq!(last_gate(&[plain]), "none");
    }

    #[test]
    fn the_note_has_the_five_lines_and_is_posted_once_per_merge_commit() {
        let p = pr(Some("aaa"));
        let body = note_to_post(&[], &p).expect("a gap");
        assert!(body.starts_with(MARKER), "{body}");
        for l in [
            "pr: https://github.com/o/r/pull/9\n",
            "merge-commit: mc1\n",
            "merged-by: someone\n",
            "head: aaa\n",
            "last-gate: none\n",
        ] {
            assert!(body.contains(l), "{l:?} in {body}");
        }
        // the posted note (by a coordinator) stops a repeat for the same merge commit
        let posted = msg(5, coord(), Kind::Info, &body);
        assert!(note_to_post(std::slice::from_ref(&posted), &p).is_none());
        assert!(has_note(std::slice::from_ref(&posted)));
        // another merge commit (the ticket was merged again) is noted again
        let mut p2 = p.clone();
        p2.merge_commit = Some("mc2".into());
        assert!(note_to_post(&[posted], &p2).is_some());
        // the same text from a dev agent is no note
        let fake = msg(6, dev(), Kind::Info, &body);
        assert!(note_to_post(std::slice::from_ref(&fake), &p).is_some());
        assert!(!has_note(&[fake]));
    }
}
