//! Canonical form that is signed (spec/messages#signature).
//!
//! `signature.rs` signs these bytes and checks them again. The sender and the reader build the
//! same bytes from the header and the body.

use super::CANONICAL_TAG;
use super::ProtocolError;
use super::header::Header;
use crate::base::TicketRef;
use sha2::{Digest, Sha256};
use std::fmt;

/// `<owner>/<repo>#<issue>`: binds a signature to one issue, so a message cannot be
/// replayed on another issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl IssueRef {
    pub fn new(owner: &str, repo: &str, number: u64) -> Self {
        IssueRef {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }

    /// Parse `owner/repo#123`.
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let err =
            || ProtocolError::Malformed(format!("bad issue ref '{s}', expected owner/repo#n"));
        let (repo_part, n) = s.split_once('#').ok_or_else(err)?;
        let (owner, repo) = repo_part.split_once('/').ok_or_else(err)?;
        let number: u64 = n.parse().map_err(|_| err())?;
        if owner.is_empty() || repo.is_empty() || repo.contains('/') {
            return Err(err());
        }
        Ok(IssueRef::new(owner, repo, number))
    }
}

/// The signed issue of a ticket. A repo without `/` gives an empty name, which no signature
/// matches.
impl From<&TicketRef> for IssueRef {
    fn from(t: &TicketRef) -> Self {
        let (owner, repo) = t.owner_and_name().unwrap_or((&t.repo, ""));
        IssueRef::new(owner, repo, t.issue)
    }
}

impl fmt::Display for IssueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// sha256 (lowercase hex) of the body after the visible line, trimmed.
pub fn body_hash(body: &str) -> String {
    let digest = Sha256::digest(body.trim().as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// The exact bytes that are signed: eight lines, each ended by `\n`, including the
/// last one. The `sig` field of the header is not part of it.
pub fn canonical(issue: &IssueRef, header: &Header, body: &str) -> String {
    format!(
        "{CANONICAL_TAG}\n{issue}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        header.from,
        header.to,
        header.kind,
        header.re,
        header.ts,
        body_hash(body)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::header::tests::sample_header;

    #[test]
    fn body_hash_known_vector_and_trim() {
        // sha256("hello")
        let h = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert_eq!(body_hash("hello"), h);
        assert_eq!(body_hash("\n\n  hello \n"), h);
        assert_ne!(body_hash("hello!"), h);
    }

    #[test]
    fn canonical_form_is_exactly_spec_9_2() {
        let issue = IssueRef::new("example-org", "app", 12);
        let header = sample_header();
        let got = canonical(&issue, &header, "hello\n");
        let expected = "crewboard-v1\n\
            example-org/app#12\n\
            hermes@host-b\n\
            claude@host-a\n\
            report\n\
            #12/c-2361455\n\
            2026-10-15T14:05:00Z\n\
            2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\n";
        assert_eq!(got, expected);
        assert_eq!(got.lines().count(), 8);
    }

    #[test]
    fn canonical_ignores_sig_but_not_the_issue_or_body() {
        let issue = IssueRef::new("example-org", "app", 12);
        let mut h = sample_header();
        let a = canonical(&issue, &h, "x");
        h.sig = "other".into();
        assert_eq!(a, canonical(&issue, &h, "x"));
        assert_ne!(
            a,
            canonical(&IssueRef::new("example-org", "app", 13), &h, "x")
        );
        assert_ne!(a, canonical(&issue, &h, "y"));
    }

    #[test]
    fn issue_ref_parse() {
        assert_eq!(
            IssueRef::parse("a/b#7").unwrap(),
            IssueRef::new("a", "b", 7)
        );
        for bad in ["", "a/b", "a#7", "/b#7", "a/#7", "a/b#x", "a/b/c#7"] {
            assert!(IssueRef::parse(bad).is_err(), "{bad}");
        }
    }
}
