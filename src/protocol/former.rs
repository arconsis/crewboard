//! Former names of the main repo, with a cutoff (spec/messages#repository-moves).
//!
//! The signed canonical form starts with `<owner>/<repo>#<issue>`. When GitHub moves or renames
//! the repo, every issue is reported under the new name, and a message signed before the move no
//! longer verifies under it. The owner lists the old name with a cutoff in AUTHORITY.md
//! (`former_repos`); a message is then also checked under that name, and only if its signed `ts`
//! is before the cutoff.
//!
//! `ts` is part of the signed bytes, so a later message cannot claim an earlier `ts` without
//! breaking its signature under the former name. The cutoff is EXCLUSIVE: a message with
//! `ts == until` does not verify under the former name. A new message is always signed under
//! the current name.

use super::ProtocolError;
use super::canonical::IssueRef;
use super::header::Message;
use super::names::Timestamp;
use super::signature::verify_message;
use ssh_key::PublicKey;

/// One entry of `former_repos`: the name that the repo had, and the first moment at which a
/// message under it is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormerRepo {
    /// `owner/repo`, as the canonical form wrote it.
    pub name: String,
    /// Exclusive cutoff, in the exact form of `ts`: `YYYY-MM-DDTHH:MM:SSZ`.
    pub until: Timestamp,
}

impl FormerRepo {
    /// Check the shape of both values. A name must be `owner/repo` without further parts.
    pub fn parse(name: &str, until: &str) -> Result<Self, ProtocolError> {
        IssueRef::parse(&format!("{name}#1"))?;
        Ok(FormerRepo {
            name: name.to_string(),
            until: Timestamp::parse(until)?,
        })
    }

    /// Does a message with this `ts` count under the former name? Both are UTC with a fixed
    /// width, so the text order is the time order. Exclusive: `ts == until` does not count.
    pub fn covers(&self, ts: &Timestamp) -> bool {
        ts.as_str() < self.until.as_str()
    }
}

/// The issue ref under a former name: `old-owner/old-repo#n`.
pub fn under_name(name: &str, number: u64) -> Result<IssueRef, ProtocolError> {
    IssueRef::parse(&format!("{name}#{number}"))
}

/// Verify under the current name, and if that fails under each former name that covers the
/// signed `ts` of the message. The error is the one of the current name.
pub fn verify_with_former(
    msg: &Message,
    current: &IssueRef,
    former: &[FormerRepo],
    public: &PublicKey,
) -> Result<(), ProtocolError> {
    let first = match verify_message(msg, current, public) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    for f in former.iter().filter(|f| f.covers(&msg.header.ts)) {
        if let Ok(old) = under_name(&f.name, current.number)
            && verify_message(msg, &old, public).is_ok()
        {
            return Ok(());
        }
    }
    Err(first)
}

/// The name that a repo of the evidence has now. A signed message with `ts` may name its PR under
/// a former name of the main repo; if that name covers `ts` (the cutoff is later), the PR is the
/// one of the main repo (GitHub redirects the old name). Any other repo, and any `ts` at or after
/// the cutoff, give `None`: the name stays as written and the repo check refuses it.
pub fn current_name_of<'a>(
    repo: &str,
    ts: &str,
    main_repo: &'a str,
    former: &[FormerRepo],
) -> Option<&'a str> {
    let ts = Timestamp::parse(ts).ok()?;
    former
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case(repo) && f.covers(&ts))
        .then_some(main_repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::header::Header;
    use crate::protocol::names::{AgentName, Kind, Re, To};
    use crate::protocol::signature::{Outgoing, generate_key, sign_message};

    const OLD: &str = "old-owner/old-repo";
    const CUT: &str = "2026-11-01T12:00:00Z";

    fn old_ref() -> IssueRef {
        IssueRef::new("old-owner", "old-repo", 7)
    }
    fn new_ref() -> IssueRef {
        IssueRef::new("example-org", "example-repo", 7)
    }
    fn former() -> Vec<FormerRepo> {
        vec![FormerRepo::parse(OLD, CUT).unwrap()]
    }
    fn signed(key: &ssh_key::PrivateKey, issue: &IssueRef, ts: &str, body: &str) -> Message {
        sign_message(
            key,
            issue,
            Outgoing {
                from: AgentName::parse("claude@host-a").unwrap(),
                to: To::Owner,
                kind: Kind::Info,
                re: Re::None,
                ts: Timestamp::parse(ts).unwrap(),
            },
            body,
        )
        .unwrap()
    }

    #[test]
    fn parse_checks_name_and_timestamp() {
        assert!(FormerRepo::parse(OLD, CUT).is_ok());
        for bad in [
            ("old", CUT),
            ("a/b/c", CUT),
            ("", CUT),
            (OLD, "2026-11-01"),
            (OLD, ""),
        ] {
            assert!(FormerRepo::parse(bad.0, bad.1).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_message_signed_before_the_cutoff_verifies_under_the_former_name() {
        let k = generate_key().unwrap();
        let m = signed(&k, &old_ref(), "2026-11-01T11:59:59Z", "hello");
        assert!(verify_message(&m, &new_ref(), k.public_key()).is_err());
        assert!(verify_with_former(&m, &new_ref(), &former(), k.public_key()).is_ok());
        // control: without the list it fails
        assert!(verify_with_former(&m, &new_ref(), &[], k.public_key()).is_err());
    }

    #[test]
    fn a_message_signed_after_the_cutoff_never_verifies_under_the_former_name() {
        let k = generate_key().unwrap();
        let m = signed(&k, &old_ref(), "2026-11-01T12:00:01Z", "late");
        assert!(verify_with_former(&m, &new_ref(), &former(), k.public_key()).is_err());
    }

    #[test]
    fn the_cutoff_is_exclusive() {
        let k = generate_key().unwrap();
        let at = signed(&k, &old_ref(), CUT, "at the cutoff");
        assert!(verify_with_former(&at, &new_ref(), &former(), k.public_key()).is_err());
        let before = signed(&k, &old_ref(), "2026-11-01T11:59:59Z", "one second before");
        assert!(verify_with_former(&before, &new_ref(), &former(), k.public_key()).is_ok());
    }

    #[test]
    fn a_message_under_the_current_name_needs_no_list() {
        let k = generate_key().unwrap();
        let m = signed(&k, &new_ref(), "2027-01-01T00:00:00Z", "new");
        assert!(verify_with_former(&m, &new_ref(), &[], k.public_key()).is_ok());
        assert!(verify_with_former(&m, &new_ref(), &former(), k.public_key()).is_ok());
    }

    #[test]
    fn a_forged_ts_breaks_the_signature() {
        // Signed after the cutoff under the old name. An attacker who holds the text changes the
        // visible ts to a time before the cutoff: ts is inside the signed bytes, so the
        // signature fails under the former name too.
        let k = generate_key().unwrap();
        let late = signed(&k, &old_ref(), "2026-11-02T00:00:00Z", "late");
        let mut forged = late.clone();
        forged.header = Header {
            ts: Timestamp::parse("2026-10-01T00:00:00Z").unwrap(),
            ..late.header.clone()
        };
        assert!(forged.header.ts.as_str() < CUT);
        assert!(verify_with_former(&forged, &new_ref(), &former(), k.public_key()).is_err());
        // control: the untouched message fails only because of the cutoff, the forged one even
        // when the cutoff is moved far into the future
        let wide = vec![FormerRepo::parse(OLD, "2099-01-01T00:00:00Z").unwrap()];
        assert!(verify_with_former(&late, &new_ref(), &wide, k.public_key()).is_ok());
        assert!(verify_with_former(&forged, &new_ref(), &wide, k.public_key()).is_err());
    }

    #[test]
    fn a_wrong_key_or_another_issue_number_still_fails() {
        let k = generate_key().unwrap();
        let other = generate_key().unwrap();
        let m = signed(&k, &old_ref(), "2026-10-01T00:00:00Z", "x");
        assert!(verify_with_former(&m, &new_ref(), &former(), other.public_key()).is_err());
        let elsewhere = IssueRef::new("example-org", "example-repo", 8);
        assert!(verify_with_former(&m, &elsewhere, &former(), k.public_key()).is_err());
    }

    #[test]
    fn two_former_names_each_with_its_own_cutoff() {
        let k = generate_key().unwrap();
        let list = vec![
            FormerRepo::parse("first/one", "2026-06-01T00:00:00Z").unwrap(),
            FormerRepo::parse(OLD, CUT).unwrap(),
        ];
        let a = signed(
            &k,
            &IssueRef::new("first", "one", 7),
            "2026-05-01T00:00:00Z",
            "a",
        );
        let b = signed(&k, &old_ref(), "2026-09-01T00:00:00Z", "b");
        let c = signed(
            &k,
            &IssueRef::new("first", "one", 7),
            "2026-07-01T00:00:00Z",
            "c",
        );
        assert!(verify_with_former(&a, &new_ref(), &list, k.public_key()).is_ok());
        assert!(verify_with_former(&b, &new_ref(), &list, k.public_key()).is_ok());
        assert!(verify_with_former(&c, &new_ref(), &list, k.public_key()).is_err());
    }

    #[test]
    fn evidence_names_a_former_repo_only_before_the_cutoff() {
        let main = "example-org/example-repo";
        let f = former();
        let at = |ts| current_name_of(OLD, ts, main, &f);
        assert_eq!(at("2026-11-01T11:59:59Z"), Some(main));
        assert_eq!(at(CUT), None, "exclusive");
        assert_eq!(at("2026-11-02T00:00:00Z"), None);
        assert_eq!(at(""), None, "no signed ts, no mapping");
        // case of the name does not matter, another repo is never mapped
        assert_eq!(
            current_name_of("OLD-owner/Old-Repo", "2026-10-01T00:00:00Z", main, &f),
            Some(main)
        );
        assert_eq!(
            current_name_of("other/repo", "2026-10-01T00:00:00Z", main, &f),
            None
        );
        assert_eq!(
            current_name_of(OLD, "2026-10-01T00:00:00Z", main, &[]),
            None
        );
    }
}
