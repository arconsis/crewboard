//! Guard test: the tree does not point at history that a reader cannot follow.
//!
//! Numbers of the project that wrote the code mean nothing to a reader of another repository, so
//! a comment, a help text or a doc says the rule and the failure it prevents, not "issue 123".
//! The scan covers the tracked files under `src/`, `tests/`, `skill/` and `docs/` and refuses
//! `issue #N`, `PR #N`, `pull #N` (any case) and `(#N)`. The change log is history on purpose and
//! is not scanned. A number in test data (a fake ticket in a message under test) does not match
//! these forms; if a data file must hold one, list its path prefix in `ALLOWED`.
//!
//! A comment says what the code does now, not how it came to be. The comments of the Rust files
//! under `src/` and `tests/` must not use the phrases of `history_pattern`, and a script `<name>.sh`
//! that a Rust comment or a Markdown file of `skill/` or `docs/` names must be a tracked file (a
//! name with a `/` must end a tracked path). `SH_ALLOWED` lists the few names that are not files
//! of this repository, each with its reason.

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

/// Script names that are not files of this repository, with the reason.
const SH_ALLOWED: &[(&str, &str)] = &[
    (
        "poll.sh",
        "a hand-made wake script of a user that `wake install` finds",
    ),
    (
        "wake.sh",
        "a hand-made wake script of a user that `wake install` finds",
    ),
];

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

/// The tracked files of the repository.
fn tracked() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A script name: starts with a letter, a digit or `_`, and is not part of a longer word, a path
/// pattern (`ci-*.sh`), a placeholder (`<name>-wake.sh`, `{name}-wake.sh`) or a URL.
fn script_pattern() -> Regex {
    Regex::new(r"(?:^|[^A-Za-z0-9_./@{}<>*$-])([A-Za-z0-9_][A-Za-z0-9_./-]*\.sh)\b").unwrap()
}

fn history_pattern() -> Regex {
    Regex::new(r"(?i)\b(?:before this|used to|no longer)\b").unwrap()
}

/// The comment of a Rust line: a whole-line comment, or the part from a `//` that is not inside
/// a string literal and not the `//` of a URL.
fn rust_comment(line: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = line[from..].find("//").map(|i| i + from) {
        let before = &line[..i];
        if before.matches('"').count().is_multiple_of(2) && !before.ends_with(':') {
            return Some(&line[i..]);
        }
        from = i + 2;
    }
    None
}

/// The script names in `text` that are no tracked file and not in `SH_ALLOWED`.
fn missing_scripts<'a>(text: &'a str, files: &[String], re: &Regex) -> Vec<&'a str> {
    re.captures_iter(text)
        .map(|c| c.get(1).unwrap().as_str())
        .filter(|name| {
            let found = if name.contains('/') {
                files
                    .iter()
                    .any(|f| f == name || f.ends_with(&format!("/{name}")))
            } else {
                files.iter().any(|f| f.rsplit('/').next() == Some(*name))
            };
            !found && !SH_ALLOWED.iter().any(|(a, _)| a == name)
        })
        .collect()
}

/// The hits of one file: history phrases and missing scripts in the comments of a Rust file,
/// missing scripts in every line of a Markdown file. Other files give none.
fn comment_hits(file: &str, text: &str, files: &[String]) -> Vec<String> {
    let (sh, hist) = (script_pattern(), history_pattern());
    let rust = file.ends_with(".rs");
    if !rust && !file.ends_with(".md") {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let part = if rust { rust_comment(line) } else { Some(line) };
        let Some(part) = part else { continue };
        let at = format!("{file}:{}: {}", i + 1, line.trim());
        for name in missing_scripts(part, files, &sh) {
            hits.push(format!(
                "{at}\n    names `{name}`, which is not a file of this repository"
            ));
        }
        if rust && hist.is_match(part) {
            hits.push(format!(
                "{at}\n    tells history; say what the code does now"
            ));
        }
    }
    hits
}

#[test]
fn comments_name_no_missing_script_and_tell_no_history() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files = tracked();
    let mut scanned = 0;
    let mut hits = Vec::new();
    for file in &files {
        let rust =
            (file.starts_with("src/") || file.starts_with("tests/")) && file.ends_with(".rs");
        let md = (file.starts_with("skill/") || file.starts_with("docs/")) && file.ends_with(".md");
        if !rust && !md {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        scanned += 1;
        hits.extend(comment_hits(file, &text, &files));
    }
    assert!(scanned > 100, "too few scanned files: {scanned}");
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}

#[test]
fn the_comment_scan_fires_on_each_form_and_spares_code_and_data() {
    let files = vec![
        "skill/loops/tests/test-locks.sh".to_string(),
        "install.sh".to_string(),
    ];
    // the phrases are split so that this file does not hit itself
    let used = ["used", " to"].concat();
    let before = ["Before", " this"].concat();
    let longer = ["no", " longer"].concat();
    for bad in [
        "//! keyboard (`skill/loops/poll-and-wake.sh`).".to_string(),
        "/// what host-lock.sh wrote".to_string(),
        "x.ok(); // as gone.sh did".to_string(),
        format!("// {before}, the list was empty."),
        format!("    // it {used} read the board twice"),
        format!("/// the file is {longer} read"),
    ] {
        assert_eq!(comment_hits("src/a.rs", &bad, &files).len(), 1, "{bad}");
    }
    assert_eq!(
        comment_hits("docs/a.md", "run `scripts/gone.sh`", &files).len(),
        1
    );
    for good in [
        "//! the shell suite (`skill/loops/tests/test-locks.sh`)",
        "// `tests/test-locks.sh` and install.sh",
        "// a `<name>-wake.sh`, a `{name}-wake.sh` and `scripts/ci-*.sh`",
        "// https://example.com/gone.sh",
        "// a hand-made poll.sh",
        "let p = \"scripts/gone.sh\"; // a path of the test data",
        "let s = \"see https://x/gone.sh\";",
        "// a script.shim and a .sh suffix",
    ] {
        assert!(comment_hits("src/a.rs", good, &files).is_empty(), "{good}");
    }
    // a Markdown file is not scanned for the phrases; a script file is not scanned at all
    assert!(comment_hits("docs/a.md", &format!("it {used} be"), &files).is_empty());
    assert!(comment_hits("x.sh", "# gone.sh", &files).is_empty());
}

#[test]
fn no_issue_or_pr_numbers_in_code_tests_skill_and_docs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let re = pattern();
    let mut files = 0;
    let mut hits = Vec::new();
    for file in &tracked() {
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
