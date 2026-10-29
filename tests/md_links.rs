//! Link check for the Markdown docs.
//!
//! Every relative link in every tracked `*.md` file must point to a file or directory that
//! exists. If the link has an `#anchor` and the target is a Markdown file, a heading with that
//! GitHub slug must exist. Links with a scheme (`https:`, `mailto:`) are not checked, and
//! neither are links inside fenced code blocks or inline code.
//!
//! This is a Rust test (not a shell script) so it runs on every OS in CI, needs no extra
//! tool except `git`, and uses the `regex` crate that the project already has.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The GitHub slug of a heading: lower case, keep letters, digits, `-` and `_`, spaces to `-`.
fn slug(heading: &str) -> String {
    heading
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() => Some(c),
            _ => None,
        })
        .collect()
}

/// All anchors of a Markdown text. A repeated heading gets `-1`, `-2`, ... like on GitHub.
fn anchors(text: &str) -> HashSet<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || !line.starts_with('#') {
            continue;
        }
        let title = line.trim_start_matches('#');
        if !title.starts_with(' ') {
            continue;
        }
        // Drop inline code marks and link syntax before the slug is made.
        let plain = title.replace('`', "");
        let base = slug(&plain);
        let n = seen.iter().filter(|s| **s == base).count();
        seen.push(base.clone());
        if n > 0 {
            seen.push(format!("{base}-{n}"));
        }
    }
    seen.into_iter().collect()
}

/// Every link problem under `root` for the given files (paths relative to `root`).
fn check(root: &Path, files: &[PathBuf]) -> Vec<String> {
    let inline = Regex::new(r"\]\(([^)\s]+)[^)]*\)").unwrap();
    let refdef = Regex::new(r"^\s{0,3}\[[^\]]+\]:\s*(\S+)").unwrap();
    let code_span = Regex::new(r"`[^`]*`").unwrap();
    let mut problems = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(root.join(file)).unwrap();
        let mut in_fence = false;
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            if in_fence {
                continue;
            }
            let bare = code_span.replace_all(line, "");
            let mut targets: Vec<String> = inline
                .captures_iter(&bare)
                .map(|c| c[1].to_string())
                .collect();
            if let Some(c) = refdef.captures(&bare) {
                targets.push(c[1].to_string());
            }
            for target in targets {
                if let Some(msg) = bad_link(root, file, &target) {
                    problems.push(format!(
                        "{}:{}: {} ({})",
                        file.display(),
                        i + 1,
                        target,
                        msg
                    ));
                }
            }
        }
    }
    problems
}

fn bad_link(root: &Path, file: &Path, target: &str) -> Option<String> {
    let target = target.trim_matches(|c| c == '<' || c == '>');
    if target.contains("://") || target.starts_with("mailto:") {
        return None;
    }
    let (path_part, anchor) = match target.split_once('#') {
        Some((p, a)) => (p, Some(a)),
        None => (target, None),
    };
    let dest = if path_part.is_empty() {
        root.join(file)
    } else {
        root.join(file.parent().unwrap_or(Path::new("")))
            .join(path_part)
    };
    if !dest.exists() {
        return Some("file not found".to_string());
    }
    if let Some(a) = anchor
        && !a.is_empty()
        && dest.extension().is_some_and(|e| e == "md")
    {
        let text = std::fs::read_to_string(&dest).unwrap();
        if !anchors(&text).contains(&a.to_lowercase()) {
            return Some("anchor not found".to_string());
        }
    }
    None
}

#[test]
fn relative_links_in_tracked_markdown_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = Command::new("git")
        .args(["ls-files", "*.md"])
        .current_dir(root)
        .output()
        .expect("git ls-files");
    assert!(out.status.success(), "git ls-files failed");
    let files: Vec<PathBuf> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect();
    assert!(!files.is_empty(), "no tracked Markdown files found");
    let problems = check(root, &files);
    assert!(
        problems.is_empty(),
        "broken links:\n{}",
        problems.join("\n")
    );
}

#[test]
fn link_check_flags_broken_file_and_anchor_and_passes_good_links() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/b.md"), "# Title\n\n## Part `x` two\n").unwrap();
    std::fs::write(
        root.join("a.md"),
        "[ok](sub/b.md) [ok2](sub/b.md#part-x-two) [ext](https://example.com/x.md)\n\
         [self](#own)\n\n# Own\n\n`[in code](nope.md)`\n\n```\n[fenced](nope.md)\n```\n",
    )
    .unwrap();
    let good = check(root, &[PathBuf::from("a.md")]);
    assert!(good.is_empty(), "unexpected: {good:?}");

    std::fs::write(
        root.join("bad.md"),
        "[gone](sub/missing.md)\n[badanchor](sub/b.md#nope)\n[x]: sub/none.md\n",
    )
    .unwrap();
    let bad = check(root, &[PathBuf::from("bad.md")]);
    assert_eq!(bad.len(), 3, "{bad:?}");
    assert!(bad[0].contains("file not found"));
    assert!(bad[1].contains("anchor not found"));
    assert!(bad[2].contains("file not found"));
}
