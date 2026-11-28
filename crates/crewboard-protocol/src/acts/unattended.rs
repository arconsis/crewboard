//! The line of a message that the unattended coordinator posts: `unattended: <n>/<budget>`
//! (spec/messages#acts). `n` is the number of verdicts of the UTC day up to and with this
//! message; `budget` is `unattended_budget` of AUTHORITY.md when the message was written.
//!
//! The line is the last body line. A reader counts the verdicts of a day from these lines, so
//! the readers are strict: a line that is not `<n>/<budget>` gives no number.

/// The key of the line.
pub const UNATTENDED_KEY: &str = "unattended";

/// The highest `unattended_budget` (the lowest is 1).
pub const UNATTENDED_BUDGET_MAX: u32 = 50;

/// The line `unattended: <n>/<budget>`, without a newline.
pub fn unattended_line(n: u32, budget: u32) -> String {
    format!("{UNATTENDED_KEY}: {n}/{budget}")
}

/// The value of a line with the key `unattended` (the key at the start of the line, no blank
/// space before it), or `None` for any other line.
fn value_of(line: &str) -> Option<&str> {
    let (key, value) = line.split_once(':')?;
    (key == UNATTENDED_KEY).then_some(value)
}

/// Does `text` have a line with the key `unattended`, whatever its value? A line with blank
/// space before the key counts too: a writer of free text refuses such a body, and a count
/// treats the message as stamped.
pub fn has_unattended_line(text: &str) -> bool {
    text.lines().any(|l| value_of(l.trim_start()).is_some())
}

/// `(n, budget)` of the LAST line of `body` that has the key `unattended`. Strict: the value is
/// one space, then `<n>/<budget>` in plain digits without a sign or a leading zero, `budget` from
/// 1 to [`UNATTENDED_BUDGET_MAX`] and `n` not above `budget`. Anything else is `None`, also when
/// an earlier line is valid.
pub fn unattended_of(body: &str) -> Option<(u32, u32)> {
    let value = body
        .lines()
        .rev()
        .find_map(|l| value_of(l.trim_start()))?
        .strip_prefix(' ')?;
    let (n, budget) = value.split_once('/')?;
    let (n, budget) = (plain_number(n)?, plain_number(budget)?);
    ((1..=UNATTENDED_BUDGET_MAX).contains(&budget) && n <= budget).then_some((n, budget))
}

/// The key of the line that a merge notice carries when the passing verify that the merge relied
/// on has the line `unattended: <n>/<budget>`.
pub const VERIFIED_BY_KEY: &str = "verified-by";

/// The line `verified-by: unattended`, without a newline. Its key is not [`UNATTENDED_KEY`]: a
/// merge notice is no stamped message and counts for no budget.
pub fn verified_by_unattended_line() -> String {
    format!("{VERIFIED_BY_KEY}: {UNATTENDED_KEY}")
}

/// Does `body` have the line `verified-by: unattended` (the key at the start of a line)?
pub fn is_verified_by_unattended(body: &str) -> bool {
    body.lines().any(|l| l == verified_by_unattended_line())
}

/// A whole number in plain digits: no sign, no blank space, no leading zero (`0` itself is one).
fn plain_number(s: &str) -> Option<u32> {
    let plain =
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    plain.then(|| s.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_writer_and_the_strict_reader_agree() {
        for (n, b) in [(0, 1), (1, 10), (10, 10), (7, 50), (50, 50)] {
            let line = unattended_line(n, b);
            assert_eq!(unattended_of(&line), Some((n, b)), "{line}");
            assert!(has_unattended_line(&line));
            // as the last line of a verify result
            let body = format!("verify: pass\nsha: abc\n{line}\n");
            assert_eq!(unattended_of(&body), Some((n, b)), "{body}");
        }
    }

    #[test]
    fn the_last_line_with_the_key_decides() {
        assert_eq!(
            unattended_of("unattended: 1/10\nnote\nunattended: 3/10\n"),
            Some((3, 10))
        );
        // a valid line does not save a later line that is not valid
        assert_eq!(unattended_of("unattended: 1/10\nunattended: many\n"), None);
        assert!(has_unattended_line("unattended: 1/10\nunattended: many\n"));
    }

    #[test]
    fn a_value_that_is_not_n_of_budget_gives_no_number() {
        for bad in [
            "unattended:",
            "unattended: ",
            "unattended:1/10",
            "unattended:  1/10",
            "unattended: 1/10 ",
            "unattended: 1 / 10",
            "unattended: +1/10",
            "unattended: -1/10",
            "unattended: 01/10",
            "unattended: 1/010",
            "unattended: 1",
            "unattended: 1/",
            "unattended: /10",
            "unattended: 1/10/2",
            "unattended: a/10",
            "unattended: 1.0/10",
            "unattended: 1/0",
            "unattended: 1/51",
            "unattended: 11/10",
            "unattended: 99999999999/10",
        ] {
            assert_eq!(unattended_of(bad), None, "{bad:?}");
            assert!(has_unattended_line(bad), "{bad:?}");
        }
    }

    #[test]
    fn the_line_of_a_merge_notice_is_no_stamp() {
        let line = verified_by_unattended_line();
        assert_eq!(line, "verified-by: unattended");
        let body = format!("Merged by the coordinator.\npr: p\n{line}");
        assert!(is_verified_by_unattended(&body));
        assert!(!has_unattended_line(&body));
        assert_eq!(unattended_of(&body), None);
        for other in [
            "Merged by the coordinator.\npr: p",
            "note: verified-by: unattended",
            " verified-by: unattended",
            "verified-by: unattended coordinator",
            "unattended: 1/10",
        ] {
            assert!(!is_verified_by_unattended(other), "{other:?}");
        }
    }

    #[test]
    fn another_key_is_no_unattended_line() {
        for other in [
            "",
            "note: unattended: 1/10",
            "unattended 1/10",
            "Unattended: 1/10",
            "unattended-budget: 1/10",
            "the unattended: 1/10",
        ] {
            assert!(!has_unattended_line(other), "{other:?}");
            assert_eq!(unattended_of(other), None, "{other:?}");
        }
        // blank space before the key does not hide the line
        assert!(has_unattended_line("  unattended: 2/10"));
        assert_eq!(unattended_of("  unattended: 2/10"), Some((2, 10)));
    }
}
