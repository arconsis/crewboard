//! Ed25519 `sshsig` signatures, namespace `crewboard`, made in-process
//! (spec/identity#author-and-signature, spec/messages#signature).
//!
//! The `sig=` header value is the base64 body of the `sshsig` armor with the line
//! breaks removed. [`sig_to_armor`] rebuilds the armored file that
//! `ssh-keygen -Y verify -n crewboard -s <file>` reads.

use super::canonical::{IssueRef, canonical};
use super::header::{Header, Message};
use super::names::{AgentName, Kind, Re, Timestamp, To};
use super::{ProtocolError, SIG_NAMESPACE};
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey, SshSig, rand_core::OsRng};

const ARMOR_BEGIN: &str = "-----BEGIN SSH SIGNATURE-----";
const ARMOR_END: &str = "-----END SSH SIGNATURE-----";

fn bad_sig(m: impl Into<String>) -> ProtocolError {
    ProtocolError::BadSignature(m.into())
}

/// Create a new Ed25519 key.
pub fn generate_key() -> Result<PrivateKey, ProtocolError> {
    PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
        .map_err(|e| ProtocolError::BadKey(e.to_string()))
}

fn require_ed25519(alg: Algorithm) -> Result<(), ProtocolError> {
    if alg == Algorithm::Ed25519 {
        Ok(())
    } else {
        Err(ProtocolError::BadKey(format!(
            "expected ssh-ed25519, got {alg}"
        )))
    }
}

/// Sign `data` and return the header form (base64, no line breaks).
pub fn sign(key: &PrivateKey, data: &[u8]) -> Result<String, ProtocolError> {
    require_ed25519(key.algorithm())?;
    let sig = key
        .sign(SIG_NAMESPACE, HashAlg::Sha512, data)
        .map_err(|e| bad_sig(e.to_string()))?;
    let pem = sig
        .to_pem(LineEnding::LF)
        .map_err(|e| bad_sig(e.to_string()))?;
    Ok(pem.lines().filter(|l| !l.starts_with("-----")).collect())
}

/// Rebuild the armored `sshsig` text (70 columns) from the header form.
pub fn sig_to_armor(sig_b64: &str) -> String {
    let mut out = String::from(ARMOR_BEGIN);
    out.push('\n');
    for chunk in sig_b64.as_bytes().chunks(70) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    out.push_str(ARMOR_END);
    out.push('\n');
    out
}

/// Convert armored `sshsig` text (for example from `ssh-keygen -Y sign`) to the header form.
#[cfg(test)]
pub fn armor_to_sig(armor: &str) -> Result<String, ProtocolError> {
    let normalised = armor.replace("\r\n", "\n");
    let body: String = normalised
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("-----"))
        .collect();
    if body.is_empty() || !normalised.contains(ARMOR_BEGIN) {
        return Err(bad_sig("not an sshsig armor"));
    }
    Ok(body)
}

/// Verify `sig_b64` over `data` against `public`. Refuses a wrong namespace, a wrong
/// key, a changed message and any key that is not Ed25519.
pub fn verify(public: &PublicKey, data: &[u8], sig_b64: &str) -> Result<(), ProtocolError> {
    require_ed25519(public.algorithm())?;
    let sig = SshSig::from_pem(sig_to_armor(sig_b64)).map_err(|e| bad_sig(e.to_string()))?;
    if sig.namespace() != SIG_NAMESPACE {
        return Err(bad_sig(format!(
            "namespace is '{}', expected '{SIG_NAMESPACE}'",
            sig.namespace()
        )));
    }
    public
        .verify(SIG_NAMESPACE, data, &sig)
        .map_err(|e| bad_sig(e.to_string()))
}

/// The header fields of a message that is about to be signed.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub from: AgentName,
    pub to: To,
    pub kind: Kind,
    pub re: Re,
    /// Passed in so that callers and tests control the clock.
    pub ts: Timestamp,
}

/// Build and sign a message.
pub fn sign_message(
    key: &PrivateKey,
    issue: &IssueRef,
    out: Outgoing,
    body: &str,
) -> Result<Message, ProtocolError> {
    let mut header = Header {
        from: out.from,
        to: out.to,
        kind: out.kind,
        re: out.re,
        ts: out.ts,
        sig: String::new(),
    };
    let body = body.replace("\r\n", "\n").trim().to_string();
    let n = body.chars().count();
    if n > crate::rules::limits::MAX_BODY_CHARS {
        return Err(ProtocolError::BodyTooLong(n));
    }
    header.sig = sign(key, canonical(issue, &header, &body).as_bytes())?;
    Ok(Message { header, body })
}

/// Verify a parsed message against the issue it was read from and the sender's key.
pub fn verify_message(
    msg: &Message,
    issue: &IssueRef,
    public: &PublicKey,
) -> Result<(), ProtocolError> {
    let data = canonical(issue, &msg.header, &msg.body);
    verify(public, data.as_bytes(), &msg.header.sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue() -> IssueRef {
        IssueRef::new("example-org", "app", 12)
    }

    fn make(key: &PrivateKey, body: &str) -> Message {
        let out = Outgoing {
            from: AgentName::parse("hermes@host-b").unwrap(),
            to: To::parse("claude@host-a").unwrap(),
            kind: Kind::Report,
            re: Re::parse("#12/c-2361455").unwrap(),
            ts: Timestamp::parse("2026-10-15T14:05:00Z").unwrap(),
        };
        sign_message(key, &issue(), out, body).unwrap()
    }

    fn sign_body(body: &str) -> Result<Message, ProtocolError> {
        let key = generate_key().unwrap();
        let out = Outgoing {
            from: AgentName::parse("hermes@host-b").unwrap(),
            to: To::parse("claude@host-a").unwrap(),
            kind: Kind::Report,
            re: Re::parse("#12/c-2361455").unwrap(),
            ts: Timestamp::parse("2026-10-15T14:05:00Z").unwrap(),
        };
        sign_message(&key, &issue(), out, body)
    }

    #[test]
    fn body_of_exactly_65536_chars_is_signed() {
        // Multibyte: the limit counts characters, not bytes.
        assert!(sign_body(&"ä".repeat(65_536)).is_ok());
    }

    #[test]
    fn body_of_65537_chars_is_refused() {
        let e = sign_body(&"a".repeat(65_537)).unwrap_err();
        assert_eq!(e, ProtocolError::BodyTooLong(65_537));
        assert_eq!(e.rule(), "body-too-long");
    }

    #[test]
    fn round_trip_through_comment_text() {
        let key = generate_key().unwrap();
        let msg = make(&key, "all green\nsecond line");
        let parsed = Message::parse(&msg.to_comment()).unwrap();
        assert_eq!(parsed, msg);
        verify_message(&parsed, &issue(), key.public_key()).unwrap();
    }

    #[test]
    fn edited_body_fails() {
        let key = generate_key().unwrap();
        let text = make(&key, "all green").to_comment();
        let edited = Message::parse(&text.replace("all green", "all red")).unwrap();
        assert_eq!(
            verify_message(&edited, &issue(), key.public_key())
                .unwrap_err()
                .rule(),
            "bad-signature"
        );
    }

    #[test]
    fn changed_header_field_fails() {
        let key = generate_key().unwrap();
        let mut msg = make(&key, "x");
        msg.header.kind = Kind::Done;
        assert!(verify_message(&msg, &issue(), key.public_key()).is_err());
    }

    #[test]
    fn replay_on_another_issue_fails() {
        let key = generate_key().unwrap();
        let msg = make(&key, "x");
        let other = IssueRef::new("example-org", "app", 13);
        assert!(verify_message(&msg, &other, key.public_key()).is_err());
    }

    #[test]
    fn wrong_key_fails_right_key_passes() {
        let a = generate_key().unwrap();
        let b = generate_key().unwrap();
        let msg = make(&a, "x");
        assert!(verify_message(&msg, &issue(), a.public_key()).is_ok());
        assert!(verify_message(&msg, &issue(), b.public_key()).is_err());
    }

    #[test]
    fn wrong_namespace_fails() {
        let key = generate_key().unwrap();
        let sig = key.sign("other-ns", HashAlg::Sha512, b"data").unwrap();
        let b64: String = sig
            .to_pem(LineEnding::LF)
            .unwrap()
            .lines()
            .filter(|l| !l.starts_with("-----"))
            .collect();
        assert!(verify(key.public_key(), b"data", &b64).is_err());
        // Same data, right namespace: passes.
        let ok = sign(&key, b"data").unwrap();
        assert!(verify(key.public_key(), b"data", &ok).is_ok());
    }

    #[test]
    fn garbage_signature_fails() {
        let key = generate_key().unwrap();
        assert!(verify(key.public_key(), b"x", "QUJD").is_err());
        assert!(armor_to_sig("hello").is_err());
    }

    #[test]
    fn non_ed25519_key_is_refused() {
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
        let sig = sign(&key, b"x").unwrap();
        // An RSA public key line cannot be used. Parse a known ecdsa key instead of generating one.
        let ecdsa = "ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg= test";
        let public = PublicKey::from_openssh(ecdsa).unwrap();
        assert_eq!(verify(&public, b"x", &sig).unwrap_err().rule(), "bad-key");
    }
}
