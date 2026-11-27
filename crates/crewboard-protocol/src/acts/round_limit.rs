//! The round limit (spec/tickets#round-limit): the texts that `verify` writes when a ticket has
//! spent its rounds of repair, and their readers.
//!
//! A round is one failed verify that sends the work back: every fail but one whose only class is
//! `head-moved` ([`counts_as_round`]). At the limit `verify` writes two texts: the `case:` line
//! of the verify result ([`round_limit_case`]) and the question to the owner
//! ([`round_limit_question`]). The status log reads the first ([`is_round_limit_fail`]), `retro`
//! reads the second ([`round_limit_rounds`]).

use super::{FailClass, kv};

/// The start of the first line of the question to the owner. The number of rounds follows.
const QUESTION_PREFIX: &str = "round limit: ";
/// The end of the first line of the question to the owner.
const QUESTION_SUFFIX: &str = " failed verifies";
/// The start of the `case:` line of a verify result at the limit.
const CASE_PREFIX: &str = "round limit ";

/// Is a failed verify with these classes ([`super::fail_classes`]) a round? Every fail is one,
/// except a fail whose only class is `head-moved`: somebody updated the branch, and nobody has to
/// change the work.
pub fn counts_as_round(classes: &[FailClass]) -> bool {
    classes != [FailClass::HeadMoved]
}

/// The `case:` text of the verify result that spends the last round: `rounds` fails count, the
/// limit is `limit`.
pub fn round_limit_case(rounds: u32, limit: u32) -> String {
    format!("{CASE_PREFIX}{rounds} of {limit}; the owner decides")
}

/// Is `body` a verify result that stopped the ticket at the round limit (its `case:` line)?
pub fn is_round_limit_fail(body: &str) -> bool {
    kv(body, "case").is_some_and(|c| c.starts_with(CASE_PREFIX))
}

/// The rounds and the limit of the `case:` line that [`round_limit_case`] wrote; `None` for any
/// other verify result.
pub fn round_limit_case_of(body: &str) -> Option<(u32, u32)> {
    let case = kv(body, "case")?;
    let (numbers, _) = case.strip_prefix(CASE_PREFIX)?.split_once(';')?;
    let (rounds, limit) = numbers.split_once(" of ")?;
    Some((rounds.trim().parse().ok()?, limit.trim().parse().ok()?))
}

/// Does a line of `text` start with the key `case:`? A free text with such a line would read as
/// the case of a verify result, so `verify` takes no note that has one.
pub fn has_case_line(text: &str) -> bool {
    text.lines().any(|l| {
        l.split_once(':')
            .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case("case"))
    })
}

/// The body of the question to the owner at the round limit. The first line is fixed
/// ([`round_limit_rounds`] reads it); `report_url` is the comment of the verify result.
pub fn round_limit_question(rounds: u32, limit: u32, report_url: &str) -> String {
    format!(
        "{QUESTION_PREFIX}{rounds}{QUESTION_SUFFIX}\n\
         limit: max_rounds {limit} (AUTHORITY.md)\n\
         report: {report_url}\n\
         The ticket is Blocked and does not go back to its agent. You decide. A comment of yours on this ticket releases it:\n\
         - redirect: say what the agent must do differently. The agent then takes the ticket again.\n\
         - split: ask the coordinator to split the ticket.\n\
         - drop: ask the coordinator to drop the ticket.\n"
    )
}

/// The number of rounds in the first line of a round-limit question; `None` for any other text.
pub fn round_limit_rounds(body: &str) -> Option<u32> {
    body.lines()
        .next()?
        .trim()
        .strip_prefix(QUESTION_PREFIX)?
        .strip_suffix(QUESTION_SUFFIX)?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fail_is_a_round_but_one_that_is_head_moved_only() {
        assert!(!counts_as_round(&[FailClass::HeadMoved]));
        assert!(counts_as_round(&[FailClass::Review]));
        assert!(counts_as_round(&[FailClass::HeadMoved, FailClass::Review]));
        assert!(counts_as_round(&[FailClass::CiRed]));
        assert!(counts_as_round(&[FailClass::Other]));
    }

    #[test]
    fn the_case_line_and_the_question_read_back() {
        // the bytes are what boards hold: a change of wording fails here
        assert_eq!(
            round_limit_case(3, 3),
            "round limit 3 of 3; the owner decides"
        );
        let report = format!(
            "verify: fail\nfailed: x\ncase: {}\nnote: n\n",
            round_limit_case(4, 3)
        );
        assert!(is_round_limit_fail(&report));
        // another case line, a note that names the limit, and no case line at all
        assert!(!is_round_limit_fail(
            "verify: fail\ncase: verify failed; a works on b; queued as next\n"
        ));
        assert!(!is_round_limit_fail(
            "verify: fail\nnote: round limit 3 of 3\n"
        ));
        assert!(!is_round_limit_fail("verify: fail\n"));
        assert_eq!(round_limit_case_of(&report), Some((4, 3)));
        assert_eq!(
            round_limit_case_of("verify: fail\ncase: verify failed; a works on b\n"),
            None
        );
        assert_eq!(
            round_limit_case_of("verify: fail\ncase: round limit x of 3; y\n"),
            None
        );
        assert_eq!(round_limit_case_of("verify: fail\n"), None);
        // a note with a line of the key `case` would be read as the case
        for bad in ["case: round limit 9 of 9; x", "ok\n  Case : y", "a\ncase:"] {
            assert!(has_case_line(bad), "{bad:?}");
        }
        for good in [
            "in this case: fine",
            "the case is closed",
            "",
            "note\nuse case x: y",
        ] {
            assert!(!has_case_line(good), "{good:?}");
        }

        let q = round_limit_question(3, 2, "https://github.com/o/r/issues/5#issuecomment-9");
        assert_eq!(
            q,
            "round limit: 3 failed verifies\n\
             limit: max_rounds 2 (AUTHORITY.md)\n\
             report: https://github.com/o/r/issues/5#issuecomment-9\n\
             The ticket is Blocked and does not go back to its agent. You decide. A comment of yours on this ticket releases it:\n\
             - redirect: say what the agent must do differently. The agent then takes the ticket again.\n\
             - split: ask the coordinator to split the ticket.\n\
             - drop: ask the coordinator to drop the ticket.\n"
        );
        assert_eq!(round_limit_rounds(&q), Some(3));
        for other in [
            "",
            "Which layout?",
            "round limit: many failed verifies",
            "round limit: 3 failed verifies and more",
            "note\nround limit: 3 failed verifies",
        ] {
            assert_eq!(round_limit_rounds(other), None, "{other:?}");
        }
    }
}
