//! What a message means: the ONE parser of message acts (spec/messages#acts).
//!
//! The protocol has 10 `kind` values, but a ticket log carries more kinds of act than that: an
//! assignment, a reassignment, a close notice, a verify result, a late report, the marker of an
//! agent that leaves. The header has no field for them (protocol v1 stays as it is), so the act
//! exists only as the first line or as a body line of the message. Every reader asks
//! [`parse`], and every writer builds its text with the functions of this module. A change of
//! wording is then one edit, and the golden tests (`mod tests`) call the writers, so a writer
//! and the parser cannot drift apart.
//!
//! Pure. It judges the text and the kind only: who sent the message, and whether the sender may
//! do the act, is decided by the caller from the verified sender.

use crate::protocol::names::Kind;

/// Why a ticket was closed, as its notice says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseKind {
    /// `close --verified`: closed after a passed verify.
    Verified,
    /// `merge`: the coordinator merged the PR.
    Merged,
    /// `close`: the owner merged the PR by hand.
    OwnerMerged,
    /// `close --why`: the coordinator closed it by judgment.
    Judgment,
}

/// The meaning of one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// `info`: the ticket is on the board (the creation message that carries `item=`).
    BoardAnnounce,
    /// `request`: the ticket is assigned (`assign`, and `reassign` to a new agent). The signed
    /// `gate:` line and the signed `ci:` line (`Some(true)` for `ci: none`, `Some(false)` for
    /// `ci: required`) are what the assignment carries; a line that is missing or unreadable is
    /// `None`.
    Assignment {
        gate: Option<String>,
        ci_none: Option<bool>,
    },
    /// `info`: the ticket went back to Ready (`reassign`).
    Reassigned,
    /// `info`: the ticket was closed.
    Closed(CloseKind),
    /// `report`: the verify result of the coordinator.
    Verify { pass: bool },
    /// `answer`: takes an open question to the owner back.
    WithdrawsQuestion,
    /// `blocked`: the ticket is parked for the interrupting ticket `for_issue`.
    Parked { for_issue: u64 },
    /// `info`: the agent leaves (the lifecycle marker).
    Left,
    /// `info`: a late report to the owner. `key` is the `late-key:` line that makes one report
    /// per episode.
    Late { key: Option<String> },
    /// Every other message: its `kind` and its body lines are all it says.
    Other,
}

/// The act of a message with `kind` (`None`: a comment of the owner without a header) and
/// `body`. The first line decides the act, except for the acts that are a body line of their own
/// ([`WITHDRAWS_QUESTION_LINE`], [`LEFT_MARKER`]).
pub fn parse(kind: Option<Kind>, body: &str) -> Act {
    let Some(kind) = kind else {
        return Act::Other;
    };
    let first = body.lines().next().unwrap_or("").trim();
    match kind {
        Kind::Request if first.starts_with(ASSIGNED_PREFIX) => Act::Assignment {
            gate: kv(body, "gate"),
            ci_none: match kv(body, "ci").map(|v| v.to_ascii_lowercase()).as_deref() {
                Some("none") => Some(true),
                Some("required") => Some(false),
                _ => None,
            },
        },
        Kind::Info if first.starts_with(BOARD_ANNOUNCE_PREFIX) => Act::BoardAnnounce,
        Kind::Info if first.starts_with(REASSIGNED_PREFIX) => Act::Reassigned,
        Kind::Info if first.starts_with(CLOSED_VERIFIED) => Act::Closed(CloseKind::Verified),
        Kind::Info if first.starts_with(MERGED_PREFIX) => Act::Closed(CloseKind::Merged),
        Kind::Info if first.starts_with(OWNER_MERGED_PREFIX) => Act::Closed(CloseKind::OwnerMerged),
        Kind::Info if first.starts_with(CLOSED_JUDGMENT_PREFIX) => Act::Closed(CloseKind::Judgment),
        Kind::Info if has_line(body, LEFT_MARKER) => Act::Left,
        Kind::Info if body.starts_with(LATE_PREFIX) => Act::Late {
            key: kv(body, LATE_KEY),
        },
        Kind::Report => match first.strip_prefix(VERIFY_PREFIX).map(str::trim) {
            Some("pass") => Act::Verify { pass: true },
            Some("fail") => Act::Verify { pass: false },
            _ => Act::Other,
        },
        Kind::Answer if has_line(body, WITHDRAWS_QUESTION_LINE) => Act::WithdrawsQuestion,
        Kind::Blocked => parked_for(body).map_or(Act::Other, |for_issue| Act::Parked { for_issue }),
        _ => Act::Other,
    }
}

/// The dedicated command that writes the act of a free text, or `None` when the text is plain.
/// A writer of free text (`say`, `block --why`) refuses a body for which this is `Some`: the
/// reader would take the text for the act (a `say --kind info "closed: x"` of the coordinator
/// would close the ticket in the log). The text is judged as sent and with leading blank space
/// removed, because the receiver may see it that way.
pub fn reserved_for(kind: Kind, body: &str) -> Option<&'static str> {
    [body, body.trim_start()].into_iter().find_map(|text| {
        Some(match parse(Some(kind), text) {
            // the one act that `say --kind answer` may write on purpose (a coordinator answer
            // that takes a question back); `unblock` writes it too
            Act::Other | Act::WithdrawsQuestion => return None,
            Act::Assignment { .. } => "assign (or reassign --to)",
            Act::BoardAnnounce => "assign or ticket new",
            Act::Reassigned => "reassign",
            Act::Closed(_) => "close or merge",
            Act::Verify { .. } => "verify",
            Act::Parked { .. } => "take --interrupt",
            Act::Left => "stop",
            Act::Late { .. } => "no command: crewboard posts late reports itself",
        })
    })
}

/// Is a whole line of `body` (trimmed) `line`?
fn has_line(body: &str, line: &str) -> bool {
    body.lines().any(|l| l.trim() == line)
}

/// `key: value` of a message body line (first match, value not empty). Used for `pr`, `sha`,
/// `ci`, `verify`, `gate`, `late-key`.
pub fn kv(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key && !v.trim().is_empty()).then(|| v.trim().to_string())
    })
}

// ---- the texts that the writers post and the parser reads ------------------------------------

const ASSIGNED_PREFIX: &str = "Assigned to you";
const BOARD_ANNOUNCE_PREFIX: &str = "Ticket is on the board";
const REASSIGNED_PREFIX: &str = "Reassigned.";
const CLOSED_VERIFIED: &str = "Closed after a passed verify";
const MERGED_PREFIX: &str = "Merged by the coordinator.";
/// The first line of the notice of a close after an owner merge.
pub const OWNER_MERGED_HEAD: &str = "The ticket is closed: the owner merged the PR by hand.";
const OWNER_MERGED_PREFIX: &str = "The ticket is closed: the owner merged";
const CLOSED_JUDGMENT_PREFIX: &str = "closed:";
const VERIFY_PREFIX: &str = "verify:";
/// The line that a coordinator `answer` carries to take a question back.
pub const WITHDRAWS_QUESTION_LINE: &str = "withdraws: question";
/// The line of the signed `info` that an agent posts when it leaves.
pub const LEFT_MARKER: &str = "lifecycle: left";
/// Start of the `blocked` message that `take --interrupt` posts.
pub const PARK_PREFIX: &str = "parked for #";
/// Start of a late report to the owner (`late: <wait> <subject>`).
pub const LATE_PREFIX: &str = "late: ";
/// The key of a late report: the line `late-key: <key>`.
pub const LATE_KEY: &str = "late-key";

/// The body of the signed assignment message: the head, the signed `gate:` and `ci:` lines, then
/// `extra` (priority, order, reason and override lines, each with its final newline). `assign`
/// and `reassign` both write it.
pub fn assignment_body(
    title: &str,
    ticket_key: &str,
    item_id: &str,
    gate: Option<&str>,
    ci_none: Option<bool>,
    extra: &str,
) -> String {
    let gate = gate.map(|g| format!("gate: {g}\n")).unwrap_or_default();
    let ci = ci_none.map(ci_line).unwrap_or_default();
    format!("{ASSIGNED_PREFIX}: {title}\nticket: {ticket_key}\nitem={item_id}\n{gate}{ci}{extra}")
}

/// The `ci:` line that `assign` signs (`declared`: the body has `CI: none`) and that `reassign`
/// copies forward: `ci: none` or `ci: required`, with the final newline.
pub fn ci_line(declared: bool) -> &'static str {
    if declared {
        "ci: none\n"
    } else {
        "ci: required\n"
    }
}

/// The first line of the creation info that puts a ticket on the board.
pub fn board_announce_head() -> String {
    format!("{BOARD_ANNOUNCE_PREFIX}.")
}

/// The first line of a verify result: `verify: pass` or `verify: fail`.
pub fn verify_head(pass: bool) -> String {
    format!("{VERIFY_PREFIX} {}", if pass { "pass" } else { "fail" })
}

/// The notice to the old assignee of a reassignment.
pub fn reassigned_body(reason: &str) -> String {
    format!("{REASSIGNED_PREFIX} Reason: {}", reason.trim())
}

/// What a dead, draining or replaced agent left on GitHub for a ticket: its crew branch and, when
/// there is one, the open PR of that branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarlierWork {
    /// The URL of the open PR (`https://github.com/<repo>/pull/<n>`).
    pub pr: Option<String>,
    pub branch: String,
}

/// The line of the reassign notice that names the open PR: `open-pr: <url> branch: <branch>`.
const OPEN_PR_LINE: &str = "open-pr:";
/// The line of the reassign notice for a pushed branch without an open PR.
const EARLIER_BRANCH_LINE: &str = "earlier-branch:";

/// [`reassigned_body`] with the work that the old assignee left: the next line is
/// `open-pr: <url> branch: <branch>`, or `earlier-branch: <branch>` when there is no open PR.
pub fn reassigned_body_with(reason: &str, earlier: Option<&EarlierWork>) -> String {
    let mut s = reassigned_body(reason);
    match earlier {
        Some(EarlierWork {
            pr: Some(url),
            branch,
        }) => s.push_str(&format!("\n{OPEN_PR_LINE} {url} branch: {branch}\n")),
        Some(EarlierWork { pr: None, branch }) => {
            s.push_str(&format!("\n{EARLIER_BRANCH_LINE} {branch}\n"));
        }
        None => {}
    }
    s
}

/// The earlier work that a reassign notice names (`None`: it names none).
pub fn earlier_work_of(body: &str) -> Option<EarlierWork> {
    body.lines().find_map(|l| {
        let l = l.trim();
        if let Some(rest) = l.strip_prefix(OPEN_PR_LINE) {
            let (url, branch) = rest.trim().split_once(" branch: ")?;
            let (url, branch) = (url.trim(), branch.trim());
            return (!url.is_empty() && !branch.is_empty()).then(|| EarlierWork {
                pr: Some(url.to_string()),
                branch: branch.to_string(),
            });
        }
        let branch = l.strip_prefix(EARLIER_BRANCH_LINE)?.trim();
        (!branch.is_empty()).then(|| EarlierWork {
            pr: None,
            branch: branch.to_string(),
        })
    })
}

/// The notice of `close --verified`.
pub fn closed_verified_body() -> String {
    format!("{CLOSED_VERIFIED}.")
}

/// The start of the notice of `merge`: the lines up to `merge-commit: ` (the caller appends the
/// commit and the rest).
pub fn merged_head(pr: &str, sha: &str, method: impl std::fmt::Display) -> String {
    format!("{MERGED_PREFIX}\npr: {pr}\nsha: {sha}\nmethod: {method}\nmerge-commit: ")
}

/// The notice of a close by judgment (`close --why`).
pub fn closed_judgment_body(reason: &str) -> String {
    format!("{CLOSED_JUDGMENT_PREFIX} {reason}\n")
}

/// The body of the answer that takes a question back.
pub fn withdraw_body(reason: &str) -> String {
    format!("{reason}\n{WITHDRAWS_QUESTION_LINE}\n")
}

/// The body of the park message: `parked for #<n>: <reason>`.
pub fn park_body(new_issue: u64, reason: &str) -> String {
    format!("{PARK_PREFIX}{new_issue}: {}", reason.trim())
}

/// The number in `parked for #<n>: <reason>`; `None` for any other text.
fn parked_for(body: &str) -> Option<u64> {
    let rest = body.strip_prefix(PARK_PREFIX)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(':') {
        return None;
    }
    digits.parse().ok()
}

/// The body of the marker that an agent posts when it leaves.
pub fn left_body(name: &str) -> String {
    format!("{name} left\n{LEFT_MARKER}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assigned(gate: Option<&str>, ci: Option<bool>) -> Act {
        Act::Assignment {
            gate: gate.map(str::to_string),
            ci_none: ci,
        }
    }

    // One golden test per act. Each text is made by the code that posts it.

    #[test]
    fn free_text_that_reads_as_an_act_is_reserved_for_its_command() {
        let cases: Vec<(Kind, String, &str)> = vec![
            (
                Kind::Request,
                assignment_body("T", "o/r#1", "x", None, None, ""),
                "assign",
            ),
            (
                Kind::Info,
                crate::ticket::info_body("PVTI_x", 15),
                "assign or ticket new",
            ),
            (Kind::Info, reassigned_body("late"), "reassign"),
            (Kind::Info, closed_verified_body(), "close or merge"),
            (
                Kind::Info,
                merged_head("p", "s", "squash"),
                "close or merge",
            ),
            (
                Kind::Info,
                format!("{OWNER_MERGED_HEAD}\n"),
                "close or merge",
            ),
            (
                Kind::Info,
                closed_judgment_body("not needed"),
                "close or merge",
            ),
            (Kind::Report, format!("{}\nx", verify_head(true)), "verify"),
            (Kind::Report, verify_head(false), "verify"),
            (Kind::Blocked, park_body(7, "r"), "take --interrupt"),
            (Kind::Info, left_body("a@b"), "stop"),
            (
                Kind::Info,
                "late: 1h waiting\nlate-key: k".to_string(),
                "no command",
            ),
        ];
        for (kind, body, cmd) in cases {
            for text in [body.clone(), format!("\n  {body}")] {
                let got = reserved_for(kind, &text);
                assert!(
                    got.is_some_and(|c| c.starts_with(cmd)),
                    "{kind:?} {text:?}: {got:?}"
                );
            }
        }
        // plain text, also text that only looks alike, is free
        for (kind, text) in [
            (Kind::Info, "Closed the loop with the owner"),
            (Kind::Info, "we closed: nothing"),
            (Kind::Report, "the verify: pass is next"),
            (Kind::Answer, "withdraws a question? no"),
            (Kind::Info, "Reassigned tickets are listed below"),
            (Kind::Request, "please assign this"),
            (Kind::Answer, "ok\nwithdraws: question"),
            (Kind::Question, "closed: or not?"),
        ] {
            assert_eq!(reserved_for(kind, text), None, "{text}");
        }
    }

    #[test]
    fn golden_assignment_as_assign_and_reassign_write_it() {
        let body = assignment_body(
            "Fix it",
            "o/r#4",
            "PVTI_x",
            Some("CI green"),
            Some(true),
            "priority: P1\norder: 2\n",
        );
        assert_eq!(
            parse(Some(Kind::Request), &body),
            assigned(Some("CI green"), Some(true))
        );
        let required = assignment_body(
            "Fix it",
            "o/r#4",
            "PVTI_x",
            Some("a | b"),
            Some(false),
            "reason: r\n",
        );
        assert_eq!(
            parse(Some(Kind::Request), &required),
            assigned(Some("a | b"), Some(false))
        );
        // an assignment that an old binary wrote has neither line
        let old = assignment_body("Fix it", "o/r#4", "PVTI_x", None, None, "reason: r\n");
        assert_eq!(parse(Some(Kind::Request), &old), assigned(None, None));
        // an unknown `ci:` value is no signed value
        let odd = format!(
            "{}ci: maybe\n",
            assignment_body("T", "o/r#1", "x", Some("g"), None, "")
        );
        assert_eq!(parse(Some(Kind::Request), &odd), assigned(Some("g"), None));
    }

    #[test]
    fn golden_assignment_needs_the_first_line_and_the_kind() {
        // a `gate:` line alone is no assignment
        assert_eq!(
            parse(Some(Kind::Request), "x\ngate: CI green\n"),
            Act::Other
        );
        let body = assignment_body("T", "o/r#1", "x", Some("g"), Some(true), "");
        assert_eq!(parse(Some(Kind::Info), &body), Act::Other);
        assert_eq!(parse(None, &body), Act::Other);
    }

    #[test]
    fn golden_board_announce_as_the_creation_info_writes_it() {
        let body = crate::ticket::info_body("PVTI_x", 15);
        assert_eq!(parse(Some(Kind::Info), &body), Act::BoardAnnounce);
        let adopting =
            crate::ticket::info_body_adopting("PVTI_x", 15, "https://github.com/o/r/pull/9", "b");
        assert_eq!(parse(Some(Kind::Info), &adopting), Act::BoardAnnounce);
    }

    #[test]
    fn golden_reassigned() {
        assert_eq!(
            parse(Some(Kind::Info), &reassigned_body(" stuck \n")),
            Act::Reassigned
        );
    }

    #[test]
    fn a_reassign_notice_carries_the_earlier_work_and_stays_a_reassign() {
        let pr = EarlierWork {
            pr: Some("https://github.com/o/r/pull/9".into()),
            branch: "crew/a@h/5-x".into(),
        };
        let body = reassigned_body_with("dead", Some(&pr));
        assert_eq!(parse(Some(Kind::Info), &body), Act::Reassigned);
        assert_eq!(earlier_work_of(&body), Some(pr));
        let branch = EarlierWork {
            pr: None,
            branch: "crew/a@h/5-x".into(),
        };
        let body = reassigned_body_with("dead", Some(&branch));
        assert_eq!(parse(Some(Kind::Info), &body), Act::Reassigned);
        assert_eq!(earlier_work_of(&body), Some(branch));
        // no work, or a line without its value: nothing
        assert_eq!(earlier_work_of(&reassigned_body_with("dead", None)), None);
        assert_eq!(earlier_work_of("open-pr: https://x branch: "), None);
        assert_eq!(earlier_work_of("open-pr: \nearlier-branch:"), None);
    }

    #[test]
    fn golden_every_close_notice() {
        use crate::ops::tickets::owner_merge::{judgment_body, notice_body};
        use crate::ops::tickets::pr_read::MergedPr;
        let close = |body: &str| parse(Some(Kind::Info), body);
        assert_eq!(
            close(&closed_verified_body()),
            Act::Closed(CloseKind::Verified)
        );
        let merged = format!(
            "{}abc123\nbranch: crew/x\n",
            merged_head("https://github.com/o/r/pull/1", "deadbeef", "squash")
        );
        assert_eq!(close(&merged), Act::Closed(CloseKind::Merged));
        let pr = MergedPr {
            repo: "o/r".into(),
            number: 9,
            merged: true,
            body: String::new(),
            merged_by: Some("owner-login".into()),
            merge_commit: Some("abc".into()),
            head_sha: Some("def".into()),
            base: None,
        };
        assert_eq!(
            close(&notice_body(&pr, None)),
            Act::Closed(CloseKind::OwnerMerged)
        );
        assert_eq!(
            close(&notice_body(&pr, Some("why"))),
            Act::Closed(CloseKind::OwnerMerged)
        );
        assert_eq!(
            close(&judgment_body("superseded by o/r#12")),
            Act::Closed(CloseKind::Judgment)
        );
        assert_eq!(
            close(OWNER_MERGED_HEAD),
            Act::Closed(CloseKind::OwnerMerged)
        );
    }

    #[test]
    fn golden_verify_result_as_verify_writes_it() {
        let ev = crate::ticket::EvidenceMsg {
            pr: "https://github.com/o/r/pull/1".into(),
            sha: "abc".into(),
            ci: "success".into(),
            ci_sources: None,
            ci_runs: None,
            local_gate: None,
        };
        let pass = crate::ticket::verify_body(true, &ev, "abc", &[], "", None, None);
        let fail =
            crate::ticket::verify_body(false, &ev, "def", &["ci red".into()], "n", Some("c"), None);
        assert_eq!(parse(Some(Kind::Report), &pass), Act::Verify { pass: true });
        assert_eq!(
            parse(Some(Kind::Report), &fail),
            Act::Verify { pass: false }
        );
        // a report without a result, and a result in another kind, is no verify
        assert_eq!(
            parse(Some(Kind::Report), "all green\nverify: pass\n"),
            Act::Other
        );
        assert_eq!(parse(Some(Kind::Info), &pass), Act::Other);
    }

    #[test]
    fn golden_withdraws_question() {
        let body = withdraw_body("reassigned: new agent asks again");
        assert_eq!(parse(Some(Kind::Answer), &body), Act::WithdrawsQuestion);
        assert_eq!(parse(Some(Kind::Answer), "a plain answer"), Act::Other);
        assert_eq!(parse(Some(Kind::Info), &body), Act::Other);
    }

    #[test]
    fn golden_parked_for() {
        assert_eq!(
            parse(Some(Kind::Blocked), &park_body(49, "P0 first")),
            Act::Parked { for_issue: 49 }
        );
        assert_eq!(
            parse(Some(Kind::Blocked), "parked for #7: x"),
            Act::Parked { for_issue: 7 }
        );
        for bad in [
            "parked for #: x",
            "parked for #12 x",
            "parked for #x: y",
            "waiting",
            "parked for #99999999999999999999999: z",
        ] {
            assert_eq!(parse(Some(Kind::Blocked), bad), Act::Other, "{bad}");
        }
        assert_eq!(parse(Some(Kind::Info), &park_body(3, "x")), Act::Other);
    }

    #[test]
    fn golden_left_marker() {
        assert_eq!(parse(Some(Kind::Info), &left_body("claude@a")), Act::Left);
        assert_eq!(
            parse(Some(Kind::Report), &left_body("claude@a")),
            Act::Other
        );
        assert_eq!(
            parse(Some(Kind::Info), "I left\nlifecycle: left now"),
            Act::Other
        );
    }

    #[test]
    fn golden_late_report_as_the_late_note_writes_it() {
        use crate::late::{Late, Subject, Wait};
        let l = Late {
            wait: Wait::Dead,
            subject: Subject::Agent {
                name: "c@h".into(),
                coordinator: false,
            },
            since: 100,
            age: 4000,
            expected: 600,
            sha: None,
            pr: None,
        };
        let body = l
            .owner_info("owner-login", "", None)
            .expect("a dead agent is told");
        match parse(Some(Kind::Info), &body) {
            Act::Late { key: Some(k) } => assert_eq!(k, l.report_key(None)),
            other => panic!("{other:?}"),
        }
        assert_eq!(parse(Some(Kind::Info), "no late: here"), Act::Other);
    }
}
