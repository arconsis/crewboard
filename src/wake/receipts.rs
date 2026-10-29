//! The event receipts: the log lines of `acted --list` before a wake and of `acted --since`
//! after a wake that ended with exit 0.

use super::policy::clean_line;

/// The longest receipt in the log, in characters.
const CUT: usize = 200;

/// One miss that `acted --since` reported: `<repo>#<n> <kind> again|twice`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Miss {
    /// Not acted on; it goes to the next wake once.
    Again(String),
    /// Not acted on, and delivered again before; it is not delivered again.
    Twice(String),
}

impl Miss {
    /// The line for the log.
    pub fn log_text(&self) -> String {
        match self {
            Miss::Again(r) => format!("event not acted on: {r}; delivered again"),
            Miss::Twice(r) => format!("event not acted on twice: {r}; not delivered again"),
        }
    }
}

/// The log lines of the receipts from the output of `acted --list`: each line that is not empty.
pub fn delivered_lines(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| !l.is_empty())
        .map(|l| format!("event delivered to wake: {}", clean_line(l, CUT)))
        .collect()
}

/// The misses in the output of `acted --since`. A line whose last word is neither `again` nor
/// `twice` is ignored.
pub fn misses(out: &str) -> Vec<Miss> {
    out.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let (rec, how) = match l.rfind(' ') {
                Some(i) => (&l[..i], &l[i + 1..]),
                None => (l, l),
            };
            let rec = clean_line(rec, CUT);
            match how {
                "again" => Some(Miss::Again(rec)),
                "twice" => Some(Miss::Twice(rec)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ac1_every_listed_event_gets_a_receipt_line() {
        let l = delivered_lines("demo/app#12 assignment\n\ndemo/app#7 info\nowner-message\n");
        assert_eq!(
            l,
            vec![
                "event delivered to wake: demo/app#12 assignment",
                "event delivered to wake: demo/app#7 info",
                "event delivered to wake: owner-message",
            ]
        );
        assert!(delivered_lines("").is_empty());
        // control characters go and the line is cut
        let long = format!("a#1 x\x1b[0m{}", "z".repeat(300));
        assert_eq!(
            delivered_lines(&long)[0].chars().count(),
            "event delivered to wake: ".len() + 200
        );
        assert!(!delivered_lines(&long)[0].contains('\x1b'));
    }

    #[test]
    fn ac2_a_miss_is_again_or_twice_and_anything_else_is_ignored() {
        let m = misses(
            "demo/app#12 question again\ndemo/app#7 request twice\nnoise\ndemo/app#1 info maybe\n\n",
        );
        assert_eq!(
            m,
            vec![
                Miss::Again("demo/app#12 question".into()),
                Miss::Twice("demo/app#7 request".into())
            ]
        );
        assert_eq!(
            m[0].log_text(),
            "event not acted on: demo/app#12 question; delivered again"
        );
        assert_eq!(
            m[1].log_text(),
            "event not acted on twice: demo/app#7 request; not delivered again"
        );
        assert!(misses("").is_empty());
    }
}
