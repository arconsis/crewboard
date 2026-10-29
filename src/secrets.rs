//! Secret scan for `say` (spec/messages#secrets). Pure function: text in, findings out.
//!
//! A finding names the kind and the line. It never contains the secret itself, so a
//! refusal message cannot leak the token into a log.
//!
//! Hetzner tokens have no fixed prefix (64 alphanumeric characters), and a bare
//! 64-character match would flag every sha256 hex string. So that pattern only fires
//! when the word "hetzner" stands on the same line before the token.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    GithubToken,
    GithubFineGrainedPat,
    Anthropic,
    OpenAi,
    AwsAccessKey,
    Hetzner,
    PrivateKey,
}

impl SecretKind {
    pub fn label(self) -> &'static str {
        match self {
            SecretKind::GithubToken => "GitHub token",
            SecretKind::GithubFineGrainedPat => "GitHub fine-grained token",
            SecretKind::Anthropic => "Anthropic API key",
            SecretKind::OpenAi => "OpenAI API key",
            SecretKind::AwsAccessKey => "AWS access key id",
            SecretKind::Hetzner => "Hetzner-style token",
            SecretKind::PrivateKey => "private key block",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finding {
    pub kind: SecretKind,
    /// 1-based line number.
    pub line: usize,
}

#[allow(
    clippy::expect_used,
    reason = "constant patterns; a unit test compiles them"
)]
fn patterns() -> &'static [(SecretKind, Regex)] {
    static P: OnceLock<Vec<(SecretKind, Regex)>> = OnceLock::new();
    P.get_or_init(|| {
        let mk = |k, r: &str| (k, Regex::new(r).expect("secret pattern compiles"));
        vec![
            mk(SecretKind::GithubToken, r"\bgh[pousr]_[A-Za-z0-9]{36,}"),
            mk(
                SecretKind::GithubFineGrainedPat,
                r"\bgithub_pat_[A-Za-z0-9_]{22,}",
            ),
            mk(SecretKind::Anthropic, r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
            // `sk-ant-...` also matches this; scan() drops that duplicate.
            mk(SecretKind::OpenAi, r"\bsk-[A-Za-z0-9_-]{32,}"),
            mk(SecretKind::AwsAccessKey, r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
            mk(
                SecretKind::Hetzner,
                r#"(?i)hetzner[^\n]{0,60}?(?:^|[^A-Za-z0-9])[A-Za-z0-9]{64}(?:[^A-Za-z0-9]|$)"#,
            ),
            mk(
                SecretKind::PrivateKey,
                r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----",
            ),
        ]
    })
}

/// Scan `text`. Returns one finding per kind and line, ordered by line.
pub fn scan(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for (kind, re) in patterns() {
            let hit = re.find_iter(line).any(|m| {
                // An Anthropic key is not also an OpenAI key.
                !(*kind == SecretKind::OpenAi && m.as_str().starts_with("sk-ant-"))
            });
            if hit {
                out.push(Finding {
                    kind: *kind,
                    line: i + 1,
                });
            }
        }
    }
    out
}

/// One-line refusal text, or `None` if the text is clean.
pub fn refusal_message(findings: &[Finding]) -> Option<String> {
    if findings.is_empty() {
        return None;
    }
    let list: Vec<String> = findings
        .iter()
        .map(|f| format!("{} on line {}", f.kind.label(), f.line))
        .collect();
    Some(format!(
        "body looks like it contains a secret ({}); not posted",
        list.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fake values assembled at run time, so that this file itself holds no token-shaped literal.
    fn alnum(n: usize) -> String {
        "aB3dE5gH7jK9mN1pQ3sT5vW7yZ"
            .chars()
            .cycle()
            .take(n)
            .collect()
    }

    fn kinds(text: &str) -> Vec<SecretKind> {
        scan(text).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn planted_hit_per_pattern() {
        let cases: Vec<(SecretKind, String)> = vec![
            (SecretKind::GithubToken, format!("token ghp_{}", alnum(36))),
            (
                SecretKind::GithubToken,
                format!("oauth gho_{} end", alnum(40)),
            ),
            (
                SecretKind::GithubFineGrainedPat,
                format!("pat github_pat_{}_{}", alnum(22), alnum(40)),
            ),
            (
                SecretKind::Anthropic,
                format!("key=sk-ant-api03-{}", alnum(40)),
            ),
            (SecretKind::OpenAi, format!("OPENAI=sk-{}", alnum(48))),
            (SecretKind::OpenAi, format!("sk-proj-{}", alnum(48))),
            (
                SecretKind::AwsAccessKey,
                "id AKIAIOSFODNN7EXAMPLE here".to_string(),
            ),
            (
                SecretKind::Hetzner,
                format!("HCLOUD (hetzner) token: {}", alnum(64)),
            ),
            (SecretKind::Hetzner, format!("hetzner=\"{}\"", alnum(64))),
            (
                SecretKind::PrivateKey,
                "-----BEGIN OPENSSH PRIVATE KEY-----".to_string(),
            ),
            (
                SecretKind::PrivateKey,
                "-----BEGIN RSA PRIVATE KEY-----".to_string(),
            ),
            (
                SecretKind::PrivateKey,
                "-----BEGIN PRIVATE KEY-----".to_string(),
            ),
        ];
        for (kind, text) in cases {
            assert_eq!(kinds(&text), vec![kind], "text: {text}");
        }
    }

    #[test]
    fn anthropic_key_is_not_reported_as_openai_too() {
        assert_eq!(
            kinds(&format!("sk-ant-{}", alnum(50))),
            vec![SecretKind::Anthropic]
        );
    }

    #[test]
    fn clean_text_has_no_findings() {
        let sha256 = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let clean = format!(
            "CI on x86 is red only in `parser::tests::flake_7`.\n\
             Log: https://github.com/example-org/app/actions/runs/123456789\n\
             head sha 4f7be56a1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f, body hash {sha256}\n\
             task-queue-handling and risk-management-plan-for-the-quarterly-review-of-things\n\
             the hetzner server is named host-b, public key is ssh-ed25519 AAAAC3Nza...\n\
             ghp_ is the prefix, AKIA is too short here, BEGIN PRIVATE KEY is a phrase"
        );
        assert_eq!(scan(&clean), vec![]);
    }

    #[test]
    fn sha256_next_to_the_word_hetzner_on_another_line_is_clean() {
        let t = format!("hetzner server rebuilt\nbody hash {}", alnum(64));
        assert_eq!(scan(&t), vec![]);
    }

    #[test]
    fn too_short_tokens_are_clean() {
        assert_eq!(scan(&format!("ghp_{}", alnum(10))), vec![]);
        assert_eq!(scan("AKIA1234"), vec![]);
        assert_eq!(scan(&format!("sk-{}", alnum(10))), vec![]);
    }

    #[test]
    fn line_numbers_are_reported_and_secret_is_not_echoed() {
        let secret = format!("ghp_{}", alnum(36));
        let text = format!("fine\nstill fine\nleak: {secret}\n");
        let f = scan(&text);
        assert_eq!(
            f,
            vec![Finding {
                kind: SecretKind::GithubToken,
                line: 3
            }]
        );
        let msg = refusal_message(&f).unwrap();
        assert!(msg.contains("line 3"));
        assert!(!msg.contains(&secret));
        assert_eq!(refusal_message(&[]), None);
    }
}
