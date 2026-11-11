//! The failures of a verify result: the `failed:` lines that `verify` writes, and the class of
//! each line.
//!
//! `verify` builds every `failed:` line from a [`VerifyFailure`], and `retro` reads the class of
//! a line back with [`fail_class_of`]. Both use the texts of this file, so the writer and the
//! reader cannot drift apart. A new failure is a new variant: [`VerifyFailure::class`] has no
//! catch-all arm, so it does not compile until the variant has a class, and the test
//! `every_failure_text_reads_back_as_its_class` fails when the text of the variant does not read
//! back as that class.

use std::fmt;

/// The class of one failure of a verify result. The classes other than `Review` and `Other` are
/// machine classes: the CLI found the failure. `Review` is the judgment of the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FailClass {
    /// The PR head is not the commit of the evidence any more.
    HeadMoved,
    /// The PR is closed without a merge.
    PrClosed,
    /// The PR is not in a repo of the initiative.
    PrRepo,
    /// The evidence says `ci: none`, the ticket does not.
    CiNoneUndeclared,
    /// A local gate without its line, or on a board that needs CI.
    LocalGate,
    /// A finding about a named CI run of the evidence.
    CiEvidence,
    /// No CI run counts for the commit.
    CiMissing,
    /// CI is not green.
    CiRed,
    /// CI cannot be read.
    CiUnreadable,
    /// The Gate check of the coordinator failed (`verify --fail`): free text.
    Review,
    /// A text that no class of this build knows (an older or a newer build wrote it).
    Other,
}

impl FailClass {
    /// Every class, in the order of a report.
    pub const ALL: [FailClass; 11] = [
        FailClass::Review,
        FailClass::HeadMoved,
        FailClass::CiRed,
        FailClass::CiMissing,
        FailClass::CiEvidence,
        FailClass::CiUnreadable,
        FailClass::CiNoneUndeclared,
        FailClass::LocalGate,
        FailClass::PrClosed,
        FailClass::PrRepo,
        FailClass::Other,
    ];

    /// The word of the class in a report.
    pub fn word(self) -> &'static str {
        match self {
            FailClass::HeadMoved => "head-moved",
            FailClass::PrClosed => "pr-closed",
            FailClass::PrRepo => "pr-repo",
            FailClass::CiNoneUndeclared => "ci-none-undeclared",
            FailClass::LocalGate => "local-gate",
            FailClass::CiEvidence => "ci-evidence",
            FailClass::CiMissing => "ci-missing",
            FailClass::CiRed => "ci-red",
            FailClass::CiUnreadable => "ci-unreadable",
            FailClass::Review => "review",
            FailClass::Other => "other",
        }
    }
}

const PR_REPO_SUFFIX: &str = " is not in a repo of the initiative";
const PR_CLOSED: &str = "the PR is closed without a merge";
const HEAD_MOVED_PREFIX: &str = "PR head moved: ";
const CI_NONE_UNDECLARED: &str =
    "the evidence says 'ci: none' but the ticket body does not declare 'CI: none'";
const LOCAL_GATE_MISSING: &str = "the done message has no local-gate line [local-gate-missing]";
const LOCAL_GATE_NOT_ENABLED: &str = "the evidence is a local gate, but AUTHORITY.md ci_mode is ci: it needs CI. The dev agent runs `crewboard done` again with a CI run of the head [local-gate-not-enabled]";
const NO_COUNTED_RUN_PREFIX: &str = "no counted CI run for ";
const NO_RUN_PREFIX: &str = "no CI run for ";
const CI_OF_PREFIX: &str = "CI of ";
const NOT_GREEN_INFIX: &str = " is not green: ";
/// The start of the text in every build. This build goes on with ` (<why>) and ...`; builds
/// before it wrote `CI cannot be read: <why>`, and their signed results are still on boards.
const CI_UNREADABLE_PREFIX: &str = "CI cannot be read";
const GATE_CHECK_PREFIX: &str = "Gate check by the coordinator: ";

/// One failure of a verify result. [`fmt::Display`] gives the text of its `failed:` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyFailure {
    /// The PR `repo#number` is not in a repo of the initiative.
    PrOutsideInitiative { repo: String, number: u64 },
    /// The PR is closed without a merge.
    PrClosed,
    /// The evidence names `evidence_sha`, the PR head is `head_now`.
    HeadMoved {
        evidence_sha: String,
        head_now: String,
    },
    /// The evidence says `ci: none`, the ticket body does not declare it.
    CiNoneUndeclared,
    /// The `done` of a local gate has no `local-gate:` line.
    LocalGateMissing,
    /// The evidence is a local gate, the board needs CI.
    LocalGateNotEnabled,
    /// A finding about a CI run that the evidence names: the message and the rule id.
    CiFinding { message: String, rule: String },
    /// Only runs that are information exist for `sha`. `cause` is the hint for the dev agent.
    NoCountedCiRun {
        sha: String,
        info: Vec<String>,
        cause: String,
    },
    /// No CI run exists for `sha`. `not_accepted`: why the head is not accepted as paths-ignored.
    NoCiRun {
        sha: String,
        not_accepted: Option<String>,
        cause: String,
    },
    /// The CI of `sha` is not green: the runs that are red.
    CiNotGreen { sha: String, red: Vec<String> },
    /// CI cannot be read, and the ticket does not declare `CI: none`.
    CiUnreadable { why: String },
    /// The Gate check of the coordinator failed: the note of `verify --fail`.
    GateCheck { note: String },
}

impl VerifyFailure {
    /// The class of this failure. No catch-all arm: a new variant needs a class.
    pub fn class(&self) -> FailClass {
        match self {
            VerifyFailure::PrOutsideInitiative { .. } => FailClass::PrRepo,
            VerifyFailure::PrClosed => FailClass::PrClosed,
            VerifyFailure::HeadMoved { .. } => FailClass::HeadMoved,
            VerifyFailure::CiNoneUndeclared => FailClass::CiNoneUndeclared,
            VerifyFailure::LocalGateMissing | VerifyFailure::LocalGateNotEnabled => {
                FailClass::LocalGate
            }
            VerifyFailure::CiFinding { .. } => FailClass::CiEvidence,
            VerifyFailure::NoCountedCiRun { .. } | VerifyFailure::NoCiRun { .. } => {
                FailClass::CiMissing
            }
            VerifyFailure::CiNotGreen { .. } => FailClass::CiRed,
            VerifyFailure::CiUnreadable { .. } => FailClass::CiUnreadable,
            VerifyFailure::GateCheck { .. } => FailClass::Review,
        }
    }
}

impl fmt::Display for VerifyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyFailure::PrOutsideInitiative { repo, number } => {
                write!(f, "PR {repo}#{number}{PR_REPO_SUFFIX}")
            }
            VerifyFailure::PrClosed => f.write_str(PR_CLOSED),
            VerifyFailure::HeadMoved {
                evidence_sha,
                head_now,
            } => write!(
                f,
                "{HEAD_MOVED_PREFIX}the evidence says {evidence_sha}, the PR head is {head_now} now"
            ),
            VerifyFailure::CiNoneUndeclared => f.write_str(CI_NONE_UNDECLARED),
            VerifyFailure::LocalGateMissing => f.write_str(LOCAL_GATE_MISSING),
            VerifyFailure::LocalGateNotEnabled => f.write_str(LOCAL_GATE_NOT_ENABLED),
            VerifyFailure::CiFinding { message, rule } => write!(f, "{message} [{rule}]"),
            VerifyFailure::NoCountedCiRun { sha, info, cause } => write!(
                f,
                "{NO_COUNTED_RUN_PREFIX}{sha}: only runs that are information (not push or pull_request): {}. Name a run that is part of the Gate with `--run <id>`. {cause}",
                info.join(", ")
            ),
            VerifyFailure::NoCiRun {
                sha,
                not_accepted,
                cause,
            } => write!(
                f,
                "{NO_RUN_PREFIX}{sha}.{} {cause}",
                match not_accepted {
                    Some(why) => format!(" Not accepted as paths-ignored: {why}."),
                    None => String::new(),
                }
            ),
            VerifyFailure::CiNotGreen { sha, red } => {
                write!(f, "{CI_OF_PREFIX}{sha}{NOT_GREEN_INFIX}{}", red.join("; "))
            }
            VerifyFailure::CiUnreadable { why } => write!(
                f,
                "{CI_UNREADABLE_PREFIX} ({why}) and the ticket does not declare 'CI: none' [evidence-ci-unreadable]"
            ),
            VerifyFailure::GateCheck { note } => write!(f, "{GATE_CHECK_PREFIX}{note}"),
        }
    }
}

/// The class of the text of one `failed:` line (without the key). The fixed texts decide first;
/// a text that only ends with a rule id in brackets is a CI finding; any other text is
/// [`FailClass::Other`].
pub fn fail_class_of(text: &str) -> FailClass {
    let t = text.trim();
    if t.starts_with(GATE_CHECK_PREFIX.trim_end()) {
        FailClass::Review
    } else if t.starts_with(HEAD_MOVED_PREFIX.trim_end()) {
        FailClass::HeadMoved
    } else if t == PR_CLOSED {
        FailClass::PrClosed
    } else if t.starts_with("PR ") && t.ends_with(PR_REPO_SUFFIX) {
        FailClass::PrRepo
    } else if t == CI_NONE_UNDECLARED {
        FailClass::CiNoneUndeclared
    } else if t == LOCAL_GATE_MISSING || t == LOCAL_GATE_NOT_ENABLED {
        FailClass::LocalGate
    } else if t.starts_with(NO_COUNTED_RUN_PREFIX) || t.starts_with(NO_RUN_PREFIX) {
        FailClass::CiMissing
    } else if t.starts_with(CI_OF_PREFIX) && t.contains(NOT_GREEN_INFIX) {
        FailClass::CiRed
    } else if t.starts_with(CI_UNREADABLE_PREFIX) {
        FailClass::CiUnreadable
    } else if ends_with_rule_id(t) {
        FailClass::CiEvidence
    } else {
        FailClass::Other
    }
}

/// Does the text end with ` [<rule id>]`, a rule id in kebab case?
fn ends_with_rule_id(t: &str) -> bool {
    t.strip_suffix(']')
        .and_then(|rest| rest.rsplit_once(" ["))
        .is_some_and(|(_, id)| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// The key of a failure line of a verify result.
const FAILED_KEY: &str = "failed";

/// The line `failed: <text>` of one failure, with its line end: the one writer of the line, for
/// the signed result and for the answer that `verify` prints.
pub fn failed_line(f: &VerifyFailure) -> String {
    format!("{FAILED_KEY}: {f}\n")
}

/// The text of every `failed:` line of a verify result, in order.
pub fn failed_lines(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k == FAILED_KEY).then(|| v.trim())
        })
        .collect()
}

/// The classes of a failed verify result, sorted and without duplicates. A result without a
/// `failed:` line carries only the free text of the coordinator: [`FailClass::Review`].
pub fn fail_classes(body: &str) -> Vec<FailClass> {
    let mut classes: Vec<FailClass> = failed_lines(body).into_iter().map(fail_class_of).collect();
    if classes.is_empty() {
        classes.push(FailClass::Review);
    }
    classes.sort();
    classes.dedup();
    classes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acts::{EvidenceMsg, verify_body};

    /// One failure of every variant. A new variant goes into `listed` (which does not compile
    /// without it) and into this list.
    fn samples() -> Vec<VerifyFailure> {
        let s = |t: &str| t.to_string();
        vec![
            VerifyFailure::PrOutsideInitiative {
                repo: s("a/c"),
                number: 7,
            },
            VerifyFailure::PrClosed,
            VerifyFailure::HeadMoved {
                evidence_sha: s("abc"),
                head_now: s("def"),
            },
            VerifyFailure::CiNoneUndeclared,
            VerifyFailure::LocalGateMissing,
            VerifyFailure::LocalGateNotEnabled,
            VerifyFailure::CiFinding {
                message: s("run 9 is gone"),
                rule: s("evidence-ci-missing"),
            },
            VerifyFailure::NoCountedCiRun {
                sha: s("abc"),
                info: vec![s("9 (schedule)")],
                cause: s("Likely cause: no workflow ran."),
            },
            VerifyFailure::NoCiRun {
                sha: s("abc"),
                not_accepted: Some(s("a workflow file changed")),
                cause: s("Likely cause: no workflow ran."),
            },
            VerifyFailure::CiNotGreen {
                sha: s("abc"),
                red: vec![s("build"), s("test")],
            },
            VerifyFailure::CiUnreadable { why: s("HTTP 403") },
            VerifyFailure::GateCheck {
                note: s("Gate part 2 is missing"),
            },
        ]
    }

    /// The position of a variant in `samples`. No catch-all arm.
    fn listed(v: &VerifyFailure) -> usize {
        match v {
            VerifyFailure::PrOutsideInitiative { .. } => 0,
            VerifyFailure::PrClosed => 1,
            VerifyFailure::HeadMoved { .. } => 2,
            VerifyFailure::CiNoneUndeclared => 3,
            VerifyFailure::LocalGateMissing => 4,
            VerifyFailure::LocalGateNotEnabled => 5,
            VerifyFailure::CiFinding { .. } => 6,
            VerifyFailure::NoCountedCiRun { .. } => 7,
            VerifyFailure::NoCiRun { .. } => 8,
            VerifyFailure::CiNotGreen { .. } => 9,
            VerifyFailure::CiUnreadable { .. } => 10,
            VerifyFailure::GateCheck { .. } => 11,
        }
    }

    #[test]
    fn every_failure_text_reads_back_as_its_class() {
        let all = samples();
        for (at, v) in all.iter().enumerate() {
            assert_eq!(listed(v), at, "samples() is out of order at {v:?}");
            let text = v.to_string();
            assert_ne!(v.class(), FailClass::Other, "{v:?} has no class");
            assert_eq!(fail_class_of(&text), v.class(), "{text}");
            // through the body of a verify result, as `retro` reads it
            let e = EvidenceMsg {
                pr: "https://github.com/a/b/pull/7".into(),
                sha: "abc".into(),
                ci: "none".into(),
                ci_sources: None,
                ci_runs: None,
                local_gate: None,
            };
            let body = verify_body(false, &e, "def", std::slice::from_ref(v), "", None, None);
            assert_eq!(failed_lines(&body), [text.as_str()]);
            assert_eq!(fail_classes(&body), [v.class()]);
        }
    }

    #[test]
    fn every_class_has_its_own_word_and_is_in_the_report_order() {
        let mut words: Vec<&str> = FailClass::ALL.iter().map(|c| c.word()).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), FailClass::ALL.len());
        for v in samples() {
            assert!(FailClass::ALL.contains(&v.class()));
        }
    }

    #[test]
    fn the_texts_of_the_failures_are_pinned() {
        let texts: Vec<String> = samples().iter().map(ToString::to_string).collect();
        assert_eq!(texts[0], "PR a/c#7 is not in a repo of the initiative");
        assert_eq!(texts[1], "the PR is closed without a merge");
        assert_eq!(
            texts[2],
            "PR head moved: the evidence says abc, the PR head is def now"
        );
        assert_eq!(texts[6], "run 9 is gone [evidence-ci-missing]");
        assert_eq!(
            texts[8],
            "no CI run for abc. Not accepted as paths-ignored: a workflow file changed. Likely cause: no workflow ran."
        );
        assert_eq!(texts[9], "CI of abc is not green: build; test");
        assert_eq!(
            texts[10],
            "CI cannot be read (HTTP 403) and the ticket does not declare 'CI: none' [evidence-ci-unreadable]"
        );
        assert_eq!(
            texts[11],
            "Gate check by the coordinator: Gate part 2 is missing"
        );
    }

    #[test]
    fn the_unreadable_ci_text_of_an_older_build_has_the_same_class() {
        // builds of October wrote this text; their signed results are still on boards
        assert_eq!(
            fail_class_of("CI cannot be read: HTTP 403"),
            FailClass::CiUnreadable
        );
        assert_eq!(
            fail_classes("verify: fail\nfailed: CI cannot be read: HTTP 403\n"),
            [FailClass::CiUnreadable]
        );
        // the writer of the line and its reader agree
        let f = VerifyFailure::CiUnreadable {
            why: "HTTP 403".into(),
        };
        assert_eq!(failed_lines(&failed_line(&f)), [f.to_string().as_str()]);
    }

    #[test]
    fn a_free_text_an_unknown_text_and_a_result_without_a_line_have_a_class() {
        assert_eq!(
            fail_class_of("something a newer build says"),
            FailClass::Other
        );
        assert_eq!(
            fail_class_of("the run is too old [some-new-rule]"),
            FailClass::CiEvidence
        );
        // brackets that hold no rule id are plain text
        assert_eq!(fail_class_of("see the log [Build 7]"), FailClass::Other);
        // a Gate check that quotes a machine text stays a review
        assert_eq!(
            fail_class_of("Gate check by the coordinator: PR head moved: no"),
            FailClass::Review
        );
        // an empty note
        assert_eq!(
            fail_class_of("Gate check by the coordinator:"),
            FailClass::Review
        );
        assert_eq!(
            fail_classes("verify: fail\nnote: wrong\n"),
            [FailClass::Review]
        );
        // two lines of one class count once; the classes come sorted
        let body = "verify: fail\nfailed: CI of a is not green: x\nfailed: PR head moved: a\nfailed: CI of b is not green: y\n";
        assert_eq!(fail_classes(body), [FailClass::HeadMoved, FailClass::CiRed]);
        // only a line that starts with the key counts
        assert!(failed_lines("verify: fail\nnote: failed: no\n").is_empty());
    }
}
