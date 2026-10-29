//! Guard test: the tree does not point at issue or PR numbers.
//!
//! Numbers of the project that wrote the code mean nothing to a reader of another repository, so
//! a comment, a help text or a doc says the rule and the failure it prevents, not "issue 123".
//! The scan covers the tracked files under `src/`, `tests/`, `skill/` and `docs/` and refuses
//! `issue #N`, `PR #N`, `pull #N` (any case) and `(#N)`. The change log is history on purpose and
//! is not scanned. A number in test data (a fake ticket in a message under test) does not match
//! these forms; if a data file must hold one, list its path prefix in `ALLOWED`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panic"
)]

use regex::Regex;
use std::path::Path;
use std::process::Command;

/// Directories whose tracked files are scanned.
const SCANNED: &[&str] = &["src/", "tests/", "skill/", "docs/"];

/// Path prefixes that may hold such a reference as data. Keep it empty if you can.
const ALLOWED: &[&str] = &[];

fn pattern() -> Regex {
    Regex::new(r"(?i)\b(?:issues?|pr|prs|pull) #[0-9]+|\(#[0-9]+\)").unwrap()
}

/// Hits of `text` as `file:line: line`.
fn scan(file: &str, text: &str, re: &Regex) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| re.is_match(l))
        .map(|(i, l)| format!("{file}:{}: {}", i + 1, l.trim()))
        .collect()
}

#[test]
fn no_issue_or_pr_numbers_in_code_tests_skill_and_docs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let re = pattern();
    let mut files = 0;
    let mut hits = Vec::new();
    for file in String::from_utf8(out.stdout).unwrap().lines() {
        if !SCANNED.iter().any(|d| file.starts_with(d))
            || ALLOWED.iter().any(|a| file.starts_with(a))
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        files += 1;
        hits.extend(scan(file, &text, &re));
    }
    assert!(files > 100, "too few scanned files: {files}");
    assert!(
        hits.is_empty(),
        "numbers of issues or PRs of another repository (say the rule and the failure instead):\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_pattern_fires_on_each_form_and_spares_test_data() {
    let re = pattern();
    let n = 123;
    for bad in [
        format!("// the lock (issue #{n}): text"),
        format!("// Issue #{n}: text"),
        format!("see PR #{n}"),
        format!("see pull #{n}"),
        format!("ISSUES #{n} and #4"),
        format!("done (#{n})"),
    ] {
        assert_eq!(scan("f", &bad, &re).len(), 1, "{bad}");
    }
    for good in [
        format!("parked for #{n}"),
        format!("Closes #{n}"),
        format!("demo/app#{n}"),
        format!("the tickets #{n}, #7 and #9"),
        format!("an issue of 12 (#{n}, part 2)"),
        "a PR of the crew and an issue number".to_string(),
        "reopen the issue #[0-9]+ pattern".to_string(),
    ] {
        assert!(scan("f", &good, &re).is_empty(), "{good}");
    }
}
