//! Rule `protected-path-owner-merge` (spec/board#authority, spec/tickets#merge).
//!
//! A PR that changes a file that holds the authority of the crew is merged by the owner only,
//! whatever AUTHORITY.md says. The list is a constant, not a setting: a setting in a file that
//! a PR can change would not protect that file. Pure: the caller reads the file list of the
//! PR and asks here.
//!
//! Protected: the authority file (`cfg.authority`, default `AUTHORITY.md`, root only),
//! `.crewboard.toml` (root only), CODEOWNERS (`.github/CODEOWNERS`, `CODEOWNERS` and
//! `docs/CODEOWNERS`: the three places that GitHub reads), and everything under
//! `.github/workflows/` and `.github/actions/`. The path match strips a leading `./` and
//! ignores ASCII letter case. A rename out of a protected path counts (the old name is checked).
//!
//! The owner may ADD paths with the key `protected_paths` of AUTHORITY.md (path globs, for
//! example test configs and CI scripts; [`check_globs`]). The key can only add: the seven built-in
//! places stay protected, whatever the key says. The file is read from the default branch, and
//! a PR that changes it is protected itself.

use super::{Refusal, Verdict, require};

/// The rule name.
pub const RULE: &str = "protected-path-owner-merge";

/// More files than this are not judged: the list may be cut, so the rule cannot prove that no
/// file is protected and fails closed.
pub const MAX_FILES: usize = 1000;

/// Protected files at the root (besides the authority file).
/// GitHub reads CODEOWNERS from `.github/`, the repo root and `docs/`, so all three are listed.
const PROTECTED_EXACT: [&str; 4] = [
    ".crewboard.toml",
    ".github/codeowners",
    "codeowners",
    "docs/codeowners",
];

/// Protected directories: the directory itself and everything below it.
const PROTECTED_DIR: [&str; 2] = [".github/workflows", ".github/actions"];

/// The files of a PR as the caller read them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFileList {
    /// The paths of the changed files.
    pub names: Vec<String>,
    /// The old paths of renamed files.
    pub previous: Vec<String>,
    /// True when the PR has more than [`MAX_FILES`] files (the list is not complete).
    pub truncated: bool,
}

impl PrFileList {
    /// Every path that the PR touches: the paths, then the old paths of renamed files.
    pub fn all(&self) -> Vec<String> {
        let mut v = self.names.clone();
        for p in &self.previous {
            if !v.contains(p) {
                v.push(p.clone());
            }
        }
        v
    }
}

/// What is protected: the built-in places and the globs that the owner added.
#[derive(Debug, Clone, Copy)]
pub struct Protect<'a> {
    /// The path of the authority file (`cfg.authority`).
    pub authority: &'a str,
    /// The globs of `protected_paths` in AUTHORITY.md (additive).
    pub extra: &'a [String],
}

impl<'a> Protect<'a> {
    pub fn new(authority: &'a str, extra: &'a [String]) -> Self {
        Protect { authority, extra }
    }
}

/// The entries of `protected_paths`: a relative path glob. `*` matches inside a name, `**`
/// across directories, `?` one character; a trailing `/` means the directory and all below it.
/// An empty entry, a leading `/`, a `\` and a `..` part are refused (the entry is named), so a
/// typo cannot protect nothing without notice.
pub fn check_globs(items: &[String]) -> Result<(), String> {
    for i in items {
        let t = i.trim();
        if t.is_empty() {
            return Err("an entry is empty".to_string());
        }
        if t.starts_with('/') || t.contains('\\') || t.split('/').any(|p| p == "..") {
            return Err(format!(
                "'{i}' is not a relative path glob (no leading /, no \\, no ..)"
            ));
        }
    }
    Ok(())
}

/// One step of a glob: a byte, `?`, `*`, `**`, or the `/` right after `**` (it may be absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Byte(u8),
    One,
    Star,
    Stars,
    OptSlash,
}

fn steps(pat: &[u8]) -> Vec<Step> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pat.len() {
        let run = pat[i..].iter().take_while(|c| **c == b'*').count();
        if run >= 2 {
            out.push(Step::Stars);
            i += run;
            // `**/` also matches no directory at all (`cfg/**/x` matches `cfg/x`).
            if pat.get(i) == Some(&b'/') {
                out.push(Step::OptSlash);
                i += 1;
            }
            continue;
        }
        out.push(match pat[i] {
            b'*' => Step::Star,
            b'?' => Step::One,
            c => Step::Byte(c),
        });
        i += 1;
    }
    out
}

/// Glob match on normalized text: `**` any run, `*` any run without `/`, `?` one char but `/`.
/// It walks the text once and keeps the set of glob positions that match so far, so the cost
/// is at most (glob length) x (text length), whatever the stars.
fn glob_match(pat: &[u8], s: &[u8]) -> bool {
    let steps = steps(pat);
    // `at[k]`: the text read so far matches the first `k` steps
    let mut at = vec![false; steps.len() + 1];
    at[0] = true;
    let close = |at: &mut [bool]| {
        for k in 0..steps.len() {
            if at[k] && matches!(steps[k], Step::Star | Step::Stars | Step::OptSlash) {
                at[k + 1] = true;
            }
        }
    };
    close(&mut at);
    for &c in s {
        let mut next = vec![false; steps.len() + 1];
        for (k, step) in steps.iter().enumerate() {
            if !at[k] {
                continue;
            }
            match *step {
                Step::Byte(b) if b == c => next[k + 1] = true,
                Step::One if c != b'/' => next[k + 1] = true,
                Step::Star if c != b'/' => next[k] = true,
                Step::Stars => next[k] = true,
                Step::OptSlash if c == b'/' => next[k + 1] = true,
                _ => {}
            }
        }
        close(&mut next);
        if !next.contains(&true) {
            return false;
        }
        at = next;
    }
    at[steps.len()]
}

/// Does the glob protect this path? A glob that ends with `/` also covers the directory itself.
fn glob_hits(glob: &str, path: &str) -> bool {
    let g = norm(glob);
    if let Some(dir) = g.strip_suffix('/') {
        return path == dir || glob_match(format!("{dir}/**").as_bytes(), path.as_bytes());
    }
    glob_match(g.as_bytes(), path.as_bytes())
}

/// A path as compared: no leading `./`, ASCII lower case.
fn norm(p: &str) -> String {
    let mut s = p.trim();
    while let Some(rest) = s.strip_prefix("./") {
        s = rest;
    }
    s.to_ascii_lowercase()
}

/// Is this one path protected?
fn is_protected(path: &str, pr: Protect<'_>) -> bool {
    let p = norm(path);
    if p == norm(pr.authority) || PROTECTED_EXACT.contains(&p.as_str()) {
        return true;
    }
    PROTECTED_DIR
        .iter()
        .any(|d| p == *d || p.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
        || pr.extra.iter().any(|g| glob_hits(g, &p))
}

/// The protected paths among `files` (as given, each once), in the order of `files`.
pub fn protected_hits(files: &[String], pr: Protect<'_>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in files {
        if is_protected(f, pr) && !out.contains(f) {
            out.push(f.clone());
        }
    }
    out
}

/// The names of the protected places, for the refusal text.
fn protected_list(pr: Protect<'_>) -> String {
    let base = format!(
        "{}, .crewboard.toml, .github/workflows/**, .github/actions/** or CODEOWNERS (.github/CODEOWNERS, CODEOWNERS, docs/CODEOWNERS)",
        pr.authority
    );
    if pr.extra.is_empty() {
        base
    } else {
        format!(
            "{base}, or a path of protected_paths in {} ({})",
            pr.authority,
            pr.extra.join(", ")
        )
    }
}

/// The refusal for hits that are known already (`merge` reads the list once and passes the hits
/// and the cut flag on). `ticket` is the argument of the ticket for the command in the text.
pub fn check_hits(
    pr_url: &str,
    ticket: &str,
    hits: &[String],
    truncated: bool,
    pr: Protect<'_>,
) -> Verdict {
    if truncated {
        return Err(Refusal::new(
            RULE,
            format!(
                "PR {pr_url} has more than {MAX_FILES} files; crewboard cannot prove that none is protected: the owner merges it by hand (its body closes the ticket), then run crewboard close {ticket}; without a closing line: crewboard close {ticket} --why \"<text>\""
            ),
        ));
    }
    require(
        hits.is_empty(),
        RULE,
        &format!(
            "PR {pr_url} changes {}: only the owner merges a PR that changes {}; review it, merge it by hand (its body closes the ticket), then run crewboard close {ticket}; without a closing line: crewboard close {ticket} --why \"<text>\"",
            hits.join(", "),
            protected_list(pr)
        ),
    )
}

/// A verify PASS, `close --verified` and `merge`: refuse a PR that changes a protected path, or whose file list is cut.
pub fn check_unprotected(
    pr_url: &str,
    ticket: &str,
    files: &PrFileList,
    pr: Protect<'_>,
) -> Verdict {
    check_hits(
        pr_url,
        ticket,
        &protected_hits(&files.all(), pr),
        files.truncated,
        pr,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth() -> Protect<'static> {
        Protect::new("AUTHORITY.md", &[])
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn protected_paths_are_hits() {
        for p in [
            "AUTHORITY.md",
            "./authority.md",
            ".crewboard.toml",
            ".CrewBoard.toml",
            ".github/workflows/ci.yml",
            ".github/workflows",
            "./.github/Workflows/deep/x.yaml",
            ".github/actions/setup/action.yml",
            ".github/CODEOWNERS",
            ".github/codeowners",
        ] {
            assert_eq!(
                protected_hits(&v(&[p]), auth()),
                v(&[p]),
                "{p} must be protected"
            );
        }
    }

    #[test]
    fn other_paths_pass() {
        for p in [
            "docs/AUTHORITY.md",
            "sub/.crewboard.toml",
            ".github/workflows-x/a",
            ".github/actionsx/a",
            ".github/ISSUE_TEMPLATE/bug.md",
            ".github/CODEOWNERS.md",
            "src/main.rs",
            "AUTHORITY.md.bak",
        ] {
            assert!(protected_hits(&v(&[p]), auth()).is_empty(), "{p} must pass");
        }
    }

    #[test]
    fn a_custom_authority_path_is_protected_and_the_default_name_is_not() {
        let custom = "docs/crew/AUTHORITY.md";
        assert_eq!(
            protected_hits(&v(&[custom, "AUTHORITY.md"]), Protect::new(custom, &[])),
            v(&[custom])
        );
    }

    #[test]
    fn hits_keep_the_order_and_have_no_duplicate() {
        let files = v(&[
            "src/a.rs",
            ".crewboard.toml",
            "AUTHORITY.md",
            ".crewboard.toml",
        ]);
        assert_eq!(
            protected_hits(&files, auth()),
            v(&[".crewboard.toml", "AUTHORITY.md"])
        );
    }

    #[test]
    fn a_rename_out_of_a_protected_path_is_refused() {
        let files = PrFileList {
            names: v(&["docs/old-authority.md"]),
            previous: v(&["AUTHORITY.md"]),
            truncated: false,
        };
        let r = check_unprotected("https://github.com/o/r/pull/3", "o/r#7", &files, auth())
            .unwrap_err();
        assert_eq!(r.rule, RULE);
        assert!(r.message.contains("changes AUTHORITY.md"), "{}", r.message);
    }

    #[test]
    fn the_refusal_names_the_paths_and_ends_with_the_command() {
        let files = PrFileList {
            names: v(&["src/a.rs", ".github/workflows/ci.yml"]),
            ..PrFileList::default()
        };
        let r =
            check_unprotected("https://github.com/o/r/pull/3", "7", &files, auth()).unwrap_err();
        assert_eq!(r.rule, "protected-path-owner-merge");
        assert!(r.message.contains(".github/workflows/ci.yml"));
        assert!(!r.message.contains("src/a.rs"));
        assert!(
            r.message
                .contains("(its body closes the ticket), then run crewboard close 7;")
                && r.message.ends_with("crewboard close 7 --why \"<text>\""),
            "{}",
            r.message
        );
    }

    #[test]
    fn a_clean_list_passes_and_a_cut_list_is_refused() {
        let clean = PrFileList {
            names: v(&["src/a.rs", "README.md"]),
            ..PrFileList::default()
        };
        assert!(check_unprotected("u", "7", &clean, auth()).is_ok());
        let cut = PrFileList {
            truncated: true,
            ..clean
        };
        let r = check_unprotected("https://github.com/o/r/pull/3", "7", &cut, auth()).unwrap_err();
        assert_eq!(r.rule, RULE);
        assert!(r.message.contains("more than 1000 files"), "{}", r.message);
        assert!(r.message.ends_with("crewboard close 7 --why \"<text>\""));
    }

    #[test]
    fn the_codeowners_files_that_github_reads_are_all_protected() {
        for p in [
            ".github/CODEOWNERS",
            "CODEOWNERS",
            "docs/CODEOWNERS",
            "./docs/codeowners",
        ] {
            assert_eq!(protected_hits(&v(&[p]), auth()), v(&[p]), "{p}");
        }
        for p in [
            "src/CODEOWNERS",
            "docs/CODEOWNERS.md",
            "docs/sub/CODEOWNERS",
        ] {
            assert!(protected_hits(&v(&[p]), auth()).is_empty(), "{p}");
        }
    }

    fn with_extra<'a>(extra: &'a [String]) -> Protect<'a> {
        Protect::new("AUTHORITY.md", extra)
    }

    #[test]
    fn protected_paths_add_to_the_built_in_places() {
        let extra = v(&["tests/**", "scripts/ci-*.sh", "Makefile", "gate/"]);
        for p in [
            "tests/a/b.rs",
            "TESTS/x",
            "scripts/ci-lint.sh",
            "Makefile",
            "gate",
            "gate/run.sh",
            "./tests/x",
        ] {
            assert_eq!(protected_hits(&v(&[p]), with_extra(&extra)), v(&[p]), "{p}");
        }
        for p in [
            "src/tests/x",
            "scripts/ci/lint.sh",
            "scripts/other.sh",
            "Makefile.am",
            "gates/x",
            "testsuite/x",
        ] {
            assert!(
                protected_hits(&v(&[p]), with_extra(&extra)).is_empty(),
                "{p}"
            );
        }
        // The built-in places stay protected whatever the list holds.
        assert_eq!(
            protected_hits(
                &v(&["AUTHORITY.md", ".github/workflows/a.yml"]),
                with_extra(&extra)
            )
            .len(),
            2
        );
    }

    #[test]
    fn a_star_stays_inside_a_directory_and_two_stars_do_not() {
        let extra = v(&["ci/*.sh", "cfg/**/*.toml"]);
        let hit = |p: &str| !protected_hits(&v(&[p]), with_extra(&extra)).is_empty();
        assert!(hit("ci/a.sh") && !hit("ci/sub/a.sh"));
        assert!(hit("cfg/a/b/c.toml") && hit("cfg/x.toml") && !hit("cfg/x.json"));
    }

    #[test]
    fn the_refusal_names_the_added_paths() {
        let extra = v(&["tests/**"]);
        let files = PrFileList {
            names: v(&["tests/a.rs"]),
            ..PrFileList::default()
        };
        let r = check_unprotected("u", "7", &files, with_extra(&extra)).unwrap_err();
        assert_eq!(r.rule, RULE);
        assert!(r.message.contains("changes tests/a.rs"), "{}", r.message);
        assert!(
            r.message
                .contains("or a path of protected_paths in AUTHORITY.md (tests/**)"),
            "{}",
            r.message
        );
    }

    /// A plain recursive matcher: the reference for the semantics.
    fn reference(pat: &[u8], s: &[u8]) -> bool {
        match pat.split_first() {
            None => s.is_empty(),
            Some((b'*', rest)) if rest.first() == Some(&b'*') => {
                let rest = rest
                    .iter()
                    .skip_while(|c| **c == b'*')
                    .copied()
                    .collect::<Vec<u8>>();
                // `**/` also matches no directory at all (`cfg/**/x` matches `cfg/x`).
                let skip = rest.strip_prefix(b"/").map(<[u8]>::to_vec);
                (0..=s.len()).any(|i| {
                    reference(&rest, &s[i..])
                        || skip.as_deref().is_some_and(|r| reference(r, &s[i..]))
                })
            }
            Some((b'*', rest)) => {
                for i in 0..=s.len() {
                    if reference(rest, &s[i..]) {
                        return true;
                    }
                    if s.get(i) == Some(&b'/') {
                        break;
                    }
                }
                false
            }
            Some((b'?', rest)) => s.first().is_some_and(|c| *c != b'/') && reference(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && reference(rest, &s[1..]),
        }
    }

    #[test]
    fn the_glob_matcher_keeps_the_meaning_of_the_recursive_one() {
        let globs = [
            "",
            "a",
            "?",
            "*",
            "**",
            "***",
            "ci/*.sh",
            "cfg/**/*.toml",
            "a/**",
            "**/b",
            "a**/b",
            "a/**/**/b",
            "*/*",
            "a?c",
            "**/",
            "a*b*c",
            "x/**/y/*",
        ];
        let paths = [
            "",
            "a",
            "b",
            "ab",
            "a/b",
            "abc",
            "a/c",
            "ci/a.sh",
            "ci/sub/a.sh",
            "cfg/x.toml",
            "cfg/a/b/c.toml",
            "cfg/x.json",
            "a/x/b",
            "axb",
            "a/b/c",
            "x/y/z",
            "x/q/y/z",
            "/",
            "a/",
        ];
        for g in globs {
            for p in paths {
                assert_eq!(
                    glob_match(g.as_bytes(), p.as_bytes()),
                    reference(g.as_bytes(), p.as_bytes()),
                    "{g:?} on {p:?}"
                );
            }
        }
    }

    #[test]
    fn a_glob_with_many_stars_is_fast_on_a_long_path() {
        let glob = "*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*a*b".repeat(2);
        let path = "a".repeat(5_000);
        let start = std::time::Instant::now();
        assert!(!glob_match(glob.as_bytes(), path.as_bytes()));
        let deep = format!("{}x", "a/".repeat(2_000));
        assert!(!glob_match(
            "**/**/**/**/**/**/**/**/y".as_bytes(),
            deep.as_bytes()
        ));
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn globs_are_checked() {
        assert!(check_globs(&v(&["tests/**", "a/b.sh"])).is_ok());
        for bad in ["", "  ", "/abs", "a/../b", "a\\b"] {
            assert!(check_globs(&v(&[bad])).is_err(), "{bad:?}");
        }
    }
}
