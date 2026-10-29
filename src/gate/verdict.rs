//! The verdict of one run of the gate. It depends on the exit code of the command, the text of
//! the log of THIS run, and two patterns. A missing result line is never green.

use regex::Regex;

/// A result line that reports a failure: the word `FAILED`, or a failed count that is not 0.
pub const DEFAULT_FAIL_RE: &str = r"FAILED|(^|[^0-9])[1-9][0-9]* failed";

/// The two patterns of the gate, compiled. Each applies to one line.
#[derive(Debug)]
pub struct Patterns {
    /// `--result-re`: which lines of the log are result lines. `None`: the exit code decides.
    pub result: Option<Regex>,
    /// `--fail-re`: which result lines report a failure.
    pub fail: Regex,
}

impl Patterns {
    /// Compile the patterns. `Err` is the usage error text (the compile error of the `regex`
    /// crate included).
    pub fn compile(result: Option<&str>, fail: Option<&str>) -> Result<Patterns, String> {
        let result = result
            .map(|re| {
                Regex::new(re)
                    .map_err(|e| format!("--result-re '{re}' is not a valid pattern: {e}"))
            })
            .transpose()?;
        let fail = fail.unwrap_or(DEFAULT_FAIL_RE);
        let fail = Regex::new(fail)
            .map_err(|e| format!("--fail-re '{fail}' is not a valid pattern: {e}"))?;
        Ok(Patterns { result, fail })
    }
}

/// The lines of `log` that match `re`, in order.
pub fn result_lines(log: &str, re: &Regex) -> Vec<String> {
    log.split_terminator('\n')
        .filter(|l| re.is_match(l))
        .map(str::to_string)
        .collect()
}

/// One finished run.
#[derive(Debug)]
pub struct Run<'a> {
    /// The number of the run, from 1.
    pub n: usize,
    /// The number of runs asked for.
    pub runs: usize,
    /// The exit code of the command.
    pub rc: i32,
    /// The path of the log of this run (named in the verdict of a missing result line).
    pub log: &'a str,
    /// The result lines of this run. Empty without `--result-re`.
    pub results: &'a [String],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Green,
    /// The verdict line, without the prefix `gate-loop: `.
    Red(String),
}

/// Judge one run. The checks come in this order: the exit code, a missing result line, a failing
/// result line.
pub fn judge(p: &Patterns, run: &Run) -> Verdict {
    let at = format!("RED at run {} of {}", run.n, run.runs);
    if run.rc != 0 {
        return Verdict::Red(format!(
            "{at}: the command exited {} ({} green runs before it)",
            run.rc,
            run.n - 1
        ));
    }
    let Some(re) = &p.result else {
        return Verdict::Green;
    };
    if run.results.is_empty() {
        return Verdict::Red(format!(
            "{at}: no result line in {} matches '{}' (a missing line is not green)",
            run.log,
            re.as_str()
        ));
    }
    if run.results.iter().any(|l| p.fail.is_match(l)) {
        return Verdict::Red(format!("{at}: a result line of this run reports a failure"));
    }
    Verdict::Green
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO: &str = "^test result: ";
    const JEST: &str = "^Tests: ";
    const PYTEST: &str = "^=+ .*(passed|failed)";

    fn judged(result: Option<&str>, rc: i32, log: &str) -> Verdict {
        let p = Patterns::compile(result, None).unwrap();
        let results = result
            .map(|_| result_lines(log, p.result.as_ref().unwrap()))
            .unwrap_or_default();
        judge(
            &p,
            &Run {
                n: 2,
                runs: 4,
                rc,
                log: "/l/run-2.log",
                results: &results,
            },
        )
    }

    fn red(v: Verdict) -> String {
        match v {
            Verdict::Red(t) => t,
            Verdict::Green => panic!("expected red"),
        }
    }

    #[test]
    fn without_result_re_the_exit_code_decides() {
        assert_eq!(judged(None, 0, "Tests: 1 failed\n"), Verdict::Green);
        assert_eq!(
            red(judged(None, 1, "all fine\n")),
            "RED at run 2 of 4: the command exited 1 (1 green runs before it)"
        );
    }

    #[test]
    fn exit_0_without_result_line_is_red() {
        let t = red(judged(Some(CARGO), 0, "building\n"));
        assert_eq!(
            t,
            "RED at run 2 of 4: no result line in /l/run-2.log matches '^test result: ' (a missing line is not green)"
        );
        // the other direction: a result line makes the same log green
        assert_eq!(
            judged(
                Some(CARGO),
                0,
                "building\ntest result: ok. 5 passed; 0 failed\n"
            ),
            Verdict::Green
        );
    }

    #[test]
    fn failed_count_is_red() {
        for line in [
            "test result: ok. 3 passed; 2 failed",
            "test result: FAILED. 4 passed; 1 failed",
            "test result: ok. 1 failed; 4 passed",
            "test result: ok. 12 failed",
        ] {
            assert_eq!(
                red(judged(Some(CARGO), 0, &format!("{line}\n"))),
                "RED at run 2 of 4: a result line of this run reports a failure",
                "{line}"
            );
        }
        // zero failures, and a count that only ends in a failed digit run, are green
        for line in [
            "test result: ok. 5 passed; 0 failed; 0 ignored",
            "test result: ok. 10 passed; 0 failed",
        ] {
            assert_eq!(
                judged(Some(CARGO), 0, &format!("{line}\n")),
                Verdict::Green,
                "{line}"
            );
        }
    }

    #[test]
    fn the_exit_code_is_judged_before_the_result_lines() {
        let t = red(judged(
            Some(CARGO),
            101,
            "test result: ok. 5 passed; 0 failed\n",
        ));
        assert!(t.contains("the command exited 101"), "{t}");
    }

    #[test]
    fn old_result_line_never_counts() {
        // only the text of this run's log reaches the judge: a line of an earlier run is not in it
        let old = "test result: ok. 99 passed; 0 failed\n";
        let new = "building\n";
        assert_eq!(judged(Some(CARGO), 0, old), Verdict::Green);
        assert!(matches!(judged(Some(CARGO), 0, new), Verdict::Red(_)));
        let re = Regex::new(CARGO).unwrap();
        assert!(result_lines(new, &re).is_empty());
    }

    #[test]
    fn a_result_line_must_match_from_the_start_of_the_line() {
        let re = Regex::new(CARGO).unwrap();
        assert!(result_lines("  test result: ok. 1 passed\n", &re).is_empty());
        assert_eq!(
            result_lines("a\ntest result: ok.\nb\n", &re),
            ["test result: ok."]
        );
        // a last line without a newline counts too
        assert_eq!(
            result_lines("a\ntest result: ok.", &re),
            ["test result: ok."]
        );
    }

    #[test]
    fn the_cargo_example_of_the_help_text() {
        assert_eq!(
            judged(
                Some(CARGO),
                0,
                "test result: ok. 5 passed; 0 failed; 0 ignored\n"
            ),
            Verdict::Green
        );
        assert!(matches!(
            judged(Some(CARGO), 0, "test result: FAILED. 4 passed; 1 failed\n"),
            Verdict::Red(_)
        ));
    }

    #[test]
    fn the_jest_example_of_the_help_text() {
        let ok = "Tests:       5 passed, 5 total\n";
        let bad = "Tests:       1 failed, 4 passed, 5 total\n";
        assert_eq!(judged(Some(JEST), 0, ok), Verdict::Green);
        assert!(matches!(judged(Some(JEST), 0, bad), Verdict::Red(_)));
        assert!(matches!(
            judged(Some(JEST), 0, "Test Suites: 1 passed\n"),
            Verdict::Red(_)
        ));
    }

    #[test]
    fn the_pytest_example_of_the_help_text() {
        let ok = "============ 5 passed in 0.12s ============\n";
        let bad = "== 1 failed, 46 passed in 1.2s ==\n";
        assert_eq!(judged(Some(PYTEST), 0, ok), Verdict::Green);
        assert!(matches!(judged(Some(PYTEST), 0, bad), Verdict::Red(_)));
        assert!(matches!(
            judged(Some(PYTEST), 0, "collecting\n"),
            Verdict::Red(_)
        ));
    }

    /// The other result lines of the help text: a green log passes, and a log without the result
    /// line is red (the exit code alone would pass it).
    #[test]
    fn the_go_vitest_maven_gradle_and_xcodebuild_examples_of_the_help_text() {
        let cases = [
            (r"^\s*Tests\s", "      Tests  5 passed (5)\n"),
            (r"^(ok|FAIL)\s", "ok  \texample.org/app\t0.12s\n"),
            (r"^Tests run:", "Tests run: 12, Failures: 0, Errors: 0\n"),
            (r"^BUILD (SUCCESSFUL|FAILED)", "BUILD SUCCESSFUL in 4s\n"),
            (
                r"^\*\* TEST (SUCCEEDED|FAILED) \*\*",
                "** TEST SUCCEEDED **\n",
            ),
        ];
        for (re, ok) in cases {
            assert_eq!(judged(Some(re), 0, ok), Verdict::Green, "{re}");
            assert!(
                matches!(judged(Some(re), 0, "compiling\n"), Verdict::Red(_)),
                "{re}"
            );
            assert!(matches!(judged(Some(re), 1, ok), Verdict::Red(_)), "{re}");
        }
        // a failing line that carries its own word is red even with exit 0
        assert!(matches!(
            judged(
                Some(r"^BUILD (SUCCESSFUL|FAILED)"),
                0,
                "BUILD FAILED in 4s\n"
            ),
            Verdict::Red(_)
        ));
        assert!(matches!(
            judged(
                Some(r"^\*\* TEST (SUCCEEDED|FAILED) \*\*"),
                0,
                "** TEST FAILED **\n"
            ),
            Verdict::Red(_)
        ));
    }

    #[test]
    fn a_custom_fail_re_replaces_the_default() {
        let p = Patterns::compile(Some("^done"), Some("ERROR")).unwrap();
        let run = |lines: &[&str]| {
            let results: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
            judge(
                &p,
                &Run {
                    n: 1,
                    runs: 1,
                    rc: 0,
                    log: "l",
                    results: &results,
                },
            )
        };
        // "FAILED" is not a failure for this pattern, "ERROR" is
        assert_eq!(run(&["done: 3 FAILED"]), Verdict::Green);
        assert!(matches!(run(&["done: ERROR"]), Verdict::Red(_)));
    }

    #[test]
    fn posix_classes_of_the_old_grep_work() {
        let p = Patterns::compile(Some("^[[:digit:]]+ passed"), Some("[[:space:]]failed")).unwrap();
        let re = p.result.as_ref().unwrap();
        assert_eq!(result_lines("12 passed\nx passed\n", re), ["12 passed"]);
        assert!(p.fail.is_match("1 passed 2 failed"));
    }

    #[test]
    fn a_bad_pattern_is_an_error_with_the_compile_error() {
        let e = Patterns::compile(Some("(unclosed"), None).unwrap_err();
        assert!(
            e.starts_with("--result-re '(unclosed' is not a valid pattern: "),
            "{e}"
        );
        let e = Patterns::compile(None, Some("a{2,1}")).unwrap_err();
        assert!(
            e.starts_with("--fail-re 'a{2,1}' is not a valid pattern: "),
            "{e}"
        );
        // the default fail pattern compiles, and a good pair compiles
        assert!(Patterns::compile(Some(CARGO), None).is_ok());
    }
}
