//! Guard test: crewboard stays project-neutral.
//!
//! No tracked file may name a private project, host, agent, account, person or personal tool.
//! The names live in a private file outside the tree (a hashed list of short names in a public
//! file can be guessed by brute force). The path is `CREWBOARD_NEUTRAL_NAMES_FILE`, default
//! `~/.config/crewboard-dev/neutral-names.txt`. One entry per line: `<byte length>:<FNV-1a 64
//! hash in hex>` of the lower-case name, or the plain word; `scoped:` before an entry marks a
//! name that only `tests/` and `src/` may hold; `#` starts a comment. Without the file the
//! list test prints one line and passes. The scanner lower-cases every line of every tracked
//! text file, hashes each window of a listed length and compares. A hit is reported as
//! `file:line: text`; the hit text is the leak being reported.
//!
//! The checks that need no list (the word check, the structure checks, the scanner tests) stay
//! in this file and always run.
//!
//! Like `md_links.rs`, this is a Rust test that reads `git ls-files`, so it runs on every OS in
//! CI and needs no extra tool.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A forbidden name as (length in bytes, FNV-1a 64 hash of the lower-case name).
type Entry = (usize, u64);

/// Path prefixes where a `scoped:` name is allowed. Docs and the skill say "the sandbox project
/// configured by the CREWBOARD_IT_* variables" instead. The test code and its fixtures live under
/// `tests/` and `src/` (unit tests).
const SCOPED_PATHS: &[&str] = &["tests/", "src/"];

/// FNV-1a, 64 bit.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The private lists: names that must not appear anywhere, and names that only
/// `SCOPED_PATHS` may hold.
struct Lists {
    forbidden: Vec<Entry>,
    scoped: Vec<Entry>,
}

/// Where the private list is read from.
fn list_path() -> PathBuf {
    if let Some(p) = std::env::var_os("CREWBOARD_NEUTRAL_NAMES_FILE") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/crewboard-dev/neutral-names.txt")
}

/// One entry: `<length>:<hex hash>` or a plain word.
fn parse_entry(text: &str) -> Entry {
    if let Some((len, hex)) = text.split_once(':')
        && let (Ok(len), Ok(hash)) = (len.parse::<usize>(), u64::from_str_radix(hex, 16))
    {
        return (len, hash);
    }
    entry(text)
}

/// Parse the text of a private list file.
fn parse_lists(text: &str) -> Lists {
    let mut lists = Lists {
        forbidden: Vec::new(),
        scoped: Vec::new(),
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        match line.strip_prefix("scoped:") {
            Some(rest) => lists.scoped.push(parse_entry(rest.trim())),
            None => lists.forbidden.push(parse_entry(line)),
        }
    }
    lists
}

/// The private lists, or `None` (after one line on stdout) when the file is absent.
fn load_lists() -> Option<Lists> {
    let path = list_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(parse_lists(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("neutral_names: no list at {}; skipped", path.display());
            None
        }
        Err(e) => panic!("cannot read the name list {}: {e}", path.display()),
    }
}

/// The list entry of a plain name. Only tests with planted names use it.
fn entry(name: &str) -> Entry {
    let lower = name.to_lowercase();
    (lower.len(), fnv1a64(lower.as_bytes()))
}

/// Every window of a listed length in `text` (lower-cased by the caller) whose hash is listed,
/// as (start offset, matching entry, window text). One pass: the hash of each start position is
/// extended byte by byte, and compared at each listed length.
fn scan_line(text: &str, entries: &[Entry]) -> Vec<(usize, Entry, String)> {
    let bytes = text.as_bytes();
    let max = entries.iter().map(|e| e.0).max().unwrap_or(0);
    // Bit n is set when some entry has length n (names are shorter than 64 bytes).
    let lens: u64 = entries.iter().fold(0, |m, e| m | 1u64 << e.0);
    let mut hits = Vec::new();
    for start in 0..bytes.len() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for (k, b) in bytes[start..].iter().take(max).enumerate() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
            if lens >> (k + 1) & 1 == 0 {
                continue;
            }
            for e in entries {
                if e.0 == k + 1 && e.1 == h {
                    let w = String::from_utf8_lossy(&bytes[start..start + k + 1]).into_owned();
                    hits.push((start, *e, w));
                }
            }
        }
    }
    hits
}

/// Every hit under `root` for the given files (paths relative to `root`), as `file:line: text`.
fn check(
    root: &Path,
    files: &[PathBuf],
    forbidden: &[Entry],
    scoped: &[Entry],
    scoped_paths: &[&str],
) -> Vec<String> {
    let mut hits = Vec::new();
    for file in files {
        let rel = file.to_string_lossy().to_string();
        let norm = rel.replace('\\', "/");
        // A file that is not UTF-8 text (an image, for example) cannot name anything.
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            for (_, _, w) in scan_line(&lower, forbidden) {
                hits.push(format!("{rel}:{}: {w}", i + 1));
            }
            if !scoped_paths.iter().any(|p| norm.starts_with(p)) {
                for (_, _, w) in scan_line(&lower, scoped) {
                    hits.push(format!("{rel}:{}: {w}", i + 1));
                }
            }
        }
    }
    hits
}

#[test]
fn tracked_files_name_no_private_names() {
    let Some(lists) = load_lists() else {
        return;
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let files: Vec<PathBuf> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect();
    assert!(files.len() > 20, "too few tracked files: {}", files.len());
    // The skill's reference files are loaded by every agent: they must be in the scan.
    assert!(
        files.iter().any(|f| f
            .to_string_lossy()
            .replace('\\', "/")
            .starts_with("skill/references/")),
        "skill/references/ is not tracked, so the guard cannot check it"
    );
    // The loop scripts' docs, the security policy and the GitHub templates are public text too.
    for want in ["skill/loops/", "SECURITY.md", ".github/"] {
        assert!(
            files
                .iter()
                .any(|f| f.to_string_lossy().replace('\\', "/").starts_with(want)),
            "{want} is not tracked, so the guard cannot check it"
        );
    }
    let hits = check(root, &files, &lists.forbidden, &lists.scoped, SCOPED_PATHS);
    assert!(
        hits.is_empty(),
        "forbidden names in maintained files (use placeholders, see CONTRIBUTING.md, Project-neutral rule):\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_private_list_is_not_empty() {
    // An emptied list would pass every scan.
    let Some(lists) = load_lists() else {
        return;
    };
    assert!(
        lists.forbidden.len() >= 10,
        "{} entries",
        lists.forbidden.len()
    );
    assert!(
        lists.forbidden.iter().all(|e| e.0 >= 4),
        "a name under 4 bytes"
    );
    assert!(!lists.scoped.is_empty());
    assert!(
        lists
            .forbidden
            .iter()
            .chain(&lists.scoped)
            .all(|e| e.0 < 64),
        "a name of 64 bytes or more"
    );
}

#[test]
fn the_list_file_format_is_parsed() {
    let text = "# comment

plain-word # note
9:ff
scoped:Box-9
scoped:7:1a
";
    let l = parse_lists(text);
    assert_eq!(l.forbidden, vec![entry("plain-word"), (9, 0xff)]);
    assert_eq!(l.scoped, vec![entry("box-9"), (7, 0x1a)]);
}

#[test]
fn hash_is_fnv1a_64() {
    // Published FNV-1a 64 test vectors.
    assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn guard_fires_on_every_planted_name_and_passes_a_clean_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    // A test-only list: planted names the real list does not hold. A name that contains a
    // shorter name (zebra-quux, zebra) is reported for both.
    let names = ["zebra-quux", "zebra", "Mango@Lab9", "x9y8"];
    let list: Vec<Entry> = names.iter().map(|n| entry(n)).collect();

    // Red: each name, in a different case, is found with file and line.
    for (i, name) in names.iter().enumerate() {
        let file = format!("docs/planted{i}.md");
        std::fs::write(
            root.join(&file),
            format!("clean line\nthe {} is here\n", name.to_uppercase()),
        )
        .unwrap();
        let hits = check(root, &[PathBuf::from(&file)], &list, &[], &[]);
        let lower = name.to_lowercase();
        let want: Vec<String> = names
            .iter()
            .map(|n| n.to_lowercase())
            .filter(|n| lower.contains(n.as_str()))
            .map(|n| format!("{file}:2: {n}"))
            .collect();
        assert!(want.contains(&format!("{file}:2: {lower}")), "name {name}");
        let mut got = hits.clone();
        got.sort();
        let mut want_sorted = want.clone();
        want_sorted.sort();
        assert_eq!(got, want_sorted, "name {name}");
    }

    // Green: near misses do not fire (one byte short, one byte changed, a separator changed,
    // a name split by a space).
    std::fs::write(
        root.join("docs/near.md"),
        "zebra-quu and zebra-quuy and zebra quux and zebr and zebr-a\nx9y and x9y7 and mango@lab\n",
    )
    .unwrap();
    let one = [entry("zebra-quux")];
    assert!(check(root, &[PathBuf::from("docs/near.md")], &one, &[], &[]).is_empty());
    // The same text with the real name added fires, so the empty result above is a real pass.
    std::fs::write(root.join("docs/near2.md"), "zebra-quu and zebra-quux\n").unwrap();
    assert_eq!(
        check(root, &[PathBuf::from("docs/near2.md")], &one, &[], &[]),
        vec!["docs/near2.md:1: zebra-quux"]
    );

    // Green: a clean tree passes.
    std::fs::write(
        root.join("docs/clean.md"),
        "Agents such as claude@host-a and hermes@host-b work on <owner>/<repo>.\n",
    )
    .unwrap();
    assert!(check(root, &[PathBuf::from("docs/clean.md")], &list, &[], &[]).is_empty());

    // A binary file is skipped, not an error.
    std::fs::write(root.join("docs/blob.bin"), [0xff_u8, 0xfe, 0x00, 0x80]).unwrap();
    assert!(check(root, &[PathBuf::from("docs/blob.bin")], &list, &[], &[]).is_empty());
}

#[test]
fn scanner_reports_the_position_of_each_hit_and_handles_non_ascii_text() {
    let list = [entry("needle")];
    let hits = scan_line("a needle, another needle", &list);
    assert_eq!(
        hits.iter().map(|h| h.0).collect::<Vec<_>>(),
        vec![2, 18],
        "{hits:?}"
    );
    // Text with multi-byte characters around the hit: the window is bytes, the hit is intact.
    let hits = scan_line("größe needle ünd", &list);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "needle");
    // A name with a non-ASCII letter is listed by its byte length.
    let umlaut = [entry("Grüße")];
    assert_eq!(umlaut[0].0, "grüße".len());
    assert_eq!(scan_line("das grüße ist da", &umlaut).len(), 1);
    // Empty list and empty text do nothing.
    assert!(scan_line("needle", &[]).is_empty());
    assert!(scan_line("", &list).is_empty());
}

#[test]
fn a_scoped_name_is_allowed_only_in_its_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for d in ["docs/design/spec", "skill", "tests/fixtures", "src"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let scoped = [entry("box-9-fixture")];
    let line = "repo org/Box-9-Fixture here\n";
    // Red: prose, the skill and the root README name it.
    for f in [
        "docs/design/spec/messages.md",
        "skill/SKILL.md",
        "README.md",
    ] {
        std::fs::write(root.join(f), line).unwrap();
        assert_eq!(
            check(root, &[PathBuf::from(f)], &[], &scoped, SCOPED_PATHS),
            vec![format!("{f}:1: box-9-fixture")],
            "{f}"
        );
    }
    // Green: tests and unit tests in src use it.
    for f in ["tests/it_sandbox.rs", "tests/fixtures/a.json", "src/ci.rs"] {
        std::fs::write(root.join(f), line).unwrap();
        assert!(
            check(root, &[PathBuf::from(f)], &[], &scoped, SCOPED_PATHS).is_empty(),
            "{f}"
        );
    }
    // A path that only looks like tests/ is still checked.
    std::fs::write(root.join("docs/tests-notes.md"), line).unwrap();
    assert_eq!(
        check(
            root,
            &[PathBuf::from("docs/tests-notes.md")],
            &[],
            &scoped,
            SCOPED_PATHS
        )
        .len(),
        1
    );
}

/// Generic words that tell a reader a private rollout exists. They are common words, so they
/// are checked as whole words (not inside "copilot") and kept as plain text, not as hashes.
/// The word is built from two parts so that this file does not hold it itself.
fn private_story_words() -> Vec<String> {
    vec![["pi", "lot"].concat()]
}

/// Lines of `text` that hold `word` as a whole word, ignoring case, as `line number: line`.
fn whole_word_lines(text: &str, word: &str) -> Vec<(usize, String)> {
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lower = line.to_lowercase();
        let mut from = 0;
        while let Some(pos) = lower[from..].find(word) {
            let start = from + pos;
            let end = start + word.len();
            let before = lower[..start].chars().next_back();
            let after = lower[end..].chars().next();
            if !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char) {
                out.push((i + 1, line.to_string()));
                break;
            }
            from = end;
        }
    }
    out
}

#[test]
fn tracked_files_do_not_tell_a_private_rollout_story() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let words = private_story_words();
    let mut hits = Vec::new();
    for file in String::from_utf8(out.stdout).unwrap().lines() {
        // The change log is release history and has its own review.
        if file == "CHANGELOG.md" {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for w in &words {
            for (n, line) in whole_word_lines(&text, w) {
                hits.push(format!("{file}:{n}: {}", line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "words that point at a private rollout; describe what the code does instead:\n{}",
        hits.join("\n")
    );
}

#[test]
fn the_whole_word_check_fires_and_spares_longer_words() {
    let w = private_story_words().remove(0);
    let red = format!("the {} case\nPrefix {}.\n", w.to_uppercase(), w);
    assert_eq!(whole_word_lines(&red, &w).len(), 2);
    // Green: the word inside a longer word does not count.
    let green = format!("co{w} and {w}s and my_{w}\n");
    assert!(whole_word_lines(&green, &w).is_empty());
    // A longer word first, then the real word on the same line, still fires.
    let mixed = format!("co{w} then {w}\n");
    assert_eq!(whole_word_lines(&mixed, &w).len(), 1);
}
