//! Text helpers of the rules that other modules also need: the reference to an issue in a PR
//! text and the `ci:` value of a local gate. They live here so that `rules` imports no module
//! above it.

use regex::Regex;
use std::sync::OnceLock;

/// The `ci:` value of evidence that holds a local gate instead of a CI run.
pub const LOCAL_GATE_CI: &str = "local-gate";

/// A word for a POSIX shell: in single quotes; a single quote inside ends the quotes, adds
/// an escaped quote and opens the quotes again. Every command line that crewboard prints for a
/// person to run quotes text that an agent controls (a branch, a path, a name) with this.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Percent-encode text for a URL path or query value: every byte but `A-Za-z0-9-._~` (and `/`,
/// when `keep_slash`). `%` is encoded too, so `%2e%2e` in a name stays text: it cannot become a
/// dot segment, which the URL parser would fold. Every request path that holds a branch, a ref
/// or a file name from a PR goes through this.
pub fn url_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~')
            || (keep_slash && b == b'/')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The longest slug of a ticket branch (`<prefix><agent>/<n>-<slug>`).
pub const MAX_SLUG: usize = 100;

/// Is this the slug of a ticket branch? Only `A-Za-z0-9._-`, at most [`MAX_SLUG`] bytes, no
/// leading `-` or `.`, no `..`. So a branch name is one safe word for a shell and for a URL path.
pub fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG
        && !slug.starts_with(['-', '.'])
        && !slug.contains("..")
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Is this an agent name as a branch part (`name@host`)? The same letters as a slug, plus `@`.
pub fn branch_agent_ok(agent: &str) -> bool {
    !agent.is_empty()
        && agent.len() <= MAX_SLUG
        && !agent.starts_with(['-', '.'])
        && !agent.contains("..")
        && agent
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'@'))
}

/// Text after `Name:` on every ticket body line that has it, in order. Accepts `Gate: x`,
/// `**Gate:** x`, `**Gate**: x` and list markers. An empty value is skipped.
pub fn body_fields(body: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let t = line
            .trim_start_matches(|c: char| c == '-' || c == '*' || c == '>' || c.is_whitespace());
        if t.len() < name.len() || !t.is_char_boundary(name.len()) {
            continue;
        }
        let (head, rest) = t.split_at(name.len());
        if !head.eq_ignore_ascii_case(name) {
            continue;
        }
        let rest = rest.trim_start_matches('*');
        let Some(value) = rest.strip_prefix(':') else {
            continue;
        };
        let value = value.trim_start_matches('*').trim();
        if !value.is_empty() {
            out.push(value.to_string());
        }
    }
    out
}

/// Text after `Name:` on a ticket body line, for `Gate`, `CI`. First match wins; an empty
/// value is `None`.
pub fn body_field(body: &str, name: &str) -> Option<String> {
    body_fields(body, name).into_iter().next()
}

/// A reference to an issue: `#n`, `owner/repo#n` or the URL of the issue.
pub(crate) const REF: &str =
    r"(?:https?://github\.com/([\w.-]+/[\w.-]+)/issues/(\d+)|([\w.-]+/[\w.-]+)?#(\d+))\b";

#[allow(
    clippy::expect_used,
    reason = "a constant pattern; a unit test compiles it"
)]
fn ref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(REF).expect("reference pattern"))
}

/// Does this match (groups 1 to 4 of [`REF`], from `first`) name the ticket? A short `#n`
/// means the repo of the PR.
pub(crate) fn names_ticket(
    c: &regex::Captures<'_>,
    first: usize,
    pr_repo: &str,
    ticket_repo: &str,
    issue: u64,
) -> bool {
    let num = |i: usize| {
        c.get(first + i)
            .and_then(|m| m.as_str().parse::<u64>().ok())
    };
    let repo_is = |r: &str| r.eq_ignore_ascii_case(ticket_repo);
    if let (Some(r), Some(n)) = (c.get(first), num(1)) {
        return n == issue && repo_is(r.as_str());
    }
    match num(3) {
        Some(n) => {
            n == issue
                && match c.get(first + 2) {
                    Some(r) => repo_is(r.as_str()),
                    None => repo_is(pr_repo),
                }
        }
        None => false,
    }
}

/// Does the text name the ticket at all (`Refs #n`, `owner/repo#n`, the URL)?
pub fn mentions_ticket(text: &str, pr_repo: &str, ticket_repo: &str, issue: u64) -> bool {
    ref_re()
        .captures_iter(text)
        .any(|c| names_ticket(&c, 1, pr_repo, ticket_repo, issue))
}

#[allow(
    clippy::expect_used,
    reason = "a constant pattern; a unit test compiles it"
)]
fn closing_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(&format!(
            r"(?i)\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\b:?\s*{REF}"
        ))
        .expect("closing keyword pattern")
    })
}

/// Does the text (the body of a PR, or its title and body) hold a closing keyword for THIS
/// ticket? The one parser for both routes. GitHub's keywords: close, closes, closed, fix,
/// fixes, fixed, resolve, resolves, resolved; any letter case; an optional colon; then `#n`,
/// `owner/repo#n` or the URL of the issue. A short `#n` means the repo of the PR. A keyword for
/// another issue does not count.
pub fn closes_ticket(text: &str, pr_repo: &str, ticket_repo: &str, issue: u64) -> bool {
    closing_re()
        .captures_iter(text)
        .any(|c| names_ticket(&c, 1, pr_repo, ticket_repo, issue))
}

#[cfg(test)]
mod quoting_tests {
    use super::*;

    #[test]
    fn a_quoted_word_is_one_shell_word() {
        assert_eq!(shell_quote("crew/a@h/1-x"), "'crew/a@h/1-x'");
        assert_eq!(shell_quote("x;touch${IFS}p"), "'x;touch${IFS}p'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn url_encoding_keeps_only_unreserved_bytes_and_optionally_the_slash() {
        assert_eq!(url_encode("crew/a@h/1-x.y_z~", true), "crew/a%40h/1-x.y_z~");
        assert_eq!(url_encode("a/b", false), "a%2Fb");
        // `%` is encoded, so %2e%2e stays text and cannot fold as a dot segment
        assert_eq!(url_encode("x/%2e%2e/main", true), "x/%252e%252e/main");
        assert_eq!(url_encode("a b?c#d&e", true), "a%20b%3Fc%23d%26e");
    }

    #[test]
    fn a_slug_has_only_safe_letters() {
        for ok in ["x", "fix-it", "v1.2_x", "a.b"] {
            assert!(slug_ok(ok), "{ok}");
        }
        for bad in ["", "-x", ".x", "a..b", "a/b", "a b", "a;b", "a%2e", "é"] {
            assert!(!slug_ok(bad), "{bad}");
        }
        assert!(branch_agent_ok("claude@host-1"));
        assert!(!branch_agent_ok("a/b") && !branch_agent_ok("a;b") && !branch_agent_ok(""));
    }

    const R: &str = "a/b";

    #[test]
    fn a_closing_keyword_for_this_ticket_is_found_in_every_form() {
        for text in [
            "Closes #5",
            "closes: #5",
            "FIXED #5.",
            "This resolves a/b#5 and more",
            "Resolve https://github.com/a/b/issues/5",
            "Fix:#5",
            "Refs #1\n\nCloses\n#5",
            "close A/B#5",
            "(Fixes #5)",
        ] {
            assert!(closes_ticket(text, R, R, 5), "{text}");
        }
    }

    #[test]
    fn other_text_is_no_closing_keyword_for_this_ticket() {
        for text in [
            "Refs #5",
            "Closes #6",
            "Closes #55",
            "Closes #50",
            "it discloses #5",
            "Closes other/repo#5",
            "Closes https://github.com/other/repo/issues/5",
            "Closes https://github.com/a/b/issues/6",
            "closes the gap, see #5",
            "prefix #5",
            "",
        ] {
            assert!(!closes_ticket(text, R, R, 5), "{text}");
        }
        // `#5` in a PR of another repo is that repo's issue 5
        assert!(!closes_ticket("Closes #5", "x/y", R, 5));
        assert!(closes_ticket("Closes a/b#5", "x/y", R, 5));
    }

    #[test]
    fn a_link_to_the_ticket_is_found_without_a_keyword() {
        assert!(mentions_ticket("Refs #5", R, R, 5));
        assert!(mentions_ticket("Refs a/b#5", "x/y", R, 5));
        assert!(mentions_ticket(
            "see https://github.com/a/b/issues/5",
            R,
            R,
            5
        ));
        assert!(!mentions_ticket("Refs #6", R, R, 5));
        assert!(!mentions_ticket("Refs #5", "x/y", R, 5));
        assert!(!mentions_ticket("no link", R, R, 5));
    }
}
