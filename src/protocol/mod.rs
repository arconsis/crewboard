//! Message protocol (spec/messages): header, canonical form, signature.
//!
//! Pure, without IO. `ops/say.rs` builds and signs a message here; `events.rs` parses and
//! verifies one. The typed fields are in `names.rs`.

pub mod acts;
pub mod canonical;
pub mod former;
pub mod header;
pub mod names;
pub mod replay;
pub mod signature;

#[cfg(test)]
mod interop_tests;

use std::fmt;

/// Namespace of every `sshsig` signature made by crewboard (spec/identity#author-and-signature).
pub const SIG_NAMESPACE: &str = "crewboard";

/// Protocol version written in `v=` (spec/messages#format).
pub const PROTOCOL_VERSION: &str = "1";

/// First line of the canonical form (spec/messages#signature).
pub const CANONICAL_TAG: &str = "crewboard-v1";

/// A message body has at most this many characters (spec/messages#format).
pub const MAX_BODY_CHARS: usize = 65_536;

/// Why a protocol operation failed. Each variant has a stable rule id (`CbError` maps it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// A header, name, kind, ref or timestamp is malformed.
    Malformed(String),
    /// The visible second line does not match the header.
    VisibleLineMismatch,
    /// The signature is missing, damaged, or made with another key or namespace.
    BadSignature(String),
    /// The key is not an Ed25519 key.
    BadKey(String),
    /// The body is longer than 65,536 characters (spec/messages#format).
    BodyTooLong(usize),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Malformed(m) => write!(f, "malformed message: {m}"),
            ProtocolError::VisibleLineMismatch => {
                write!(f, "visible sender line does not match the header")
            }
            ProtocolError::BadSignature(m) => write!(f, "bad signature: {m}"),
            ProtocolError::BadKey(m) => write!(f, "bad key: {m}"),
            ProtocolError::BodyTooLong(n) => write!(
                f,
                "message body is {n} characters; the limit is {MAX_BODY_CHARS}"
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}
