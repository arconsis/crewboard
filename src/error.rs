//! One error type for the commands, mapped to the exit codes of spec/cli#exit-codes.
//!
//! Every layer returns it. `main.rs` prints it and turns it into the exit code (see `exit.rs`).

use crate::base::{GhError, Refusal};
use crate::exit::Exit;
use crate::protocol::ProtocolError;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CbError {
    /// A rule said no (exit 2).
    Refused(Refusal),
    /// GitHub or network (exit 3).
    Network(GhError),
    /// Configuration, key or credential problem (exit 4).
    Config(String),
}

impl CbError {
    pub fn config(m: impl Into<String>) -> Self {
        CbError::Config(m.into())
    }

    pub fn refused(rule: &'static str, m: impl Into<String>) -> Self {
        CbError::Refused(Refusal::new(rule, m))
    }

    pub fn exit(&self) -> Exit {
        match self {
            CbError::Refused(_) => Exit::Refused,
            CbError::Network(GhError::Auth(_) | GhError::InsufficientScopes { .. }) => Exit::Config,
            CbError::Network(_) => Exit::Network,
            CbError::Config(_) => Exit::Config,
        }
    }
}

impl fmt::Display for CbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CbError::Refused(r) => write!(f, "{r}"),
            CbError::Network(e) => write!(f, "{e}"),
            CbError::Config(m) => write!(f, "configuration error: {m}"),
        }
    }
}

impl std::error::Error for CbError {}

impl From<Refusal> for CbError {
    fn from(r: Refusal) -> Self {
        CbError::Refused(r)
    }
}

impl From<GhError> for CbError {
    fn from(e: GhError) -> Self {
        CbError::Network(e)
    }
}

impl From<ProtocolError> for CbError {
    fn from(e: ProtocolError) -> Self {
        match e {
            ProtocolError::BadKey(m) => CbError::Config(format!("bad key: {m}")),
            other => CbError::Refused(Refusal::new(other.rule(), other.to_string())),
        }
    }
}

pub type CbResult<T> = Result<T, CbError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_distinct_exit_codes() {
        assert_eq!(CbError::refused("x", "m").exit(), Exit::Refused);
        assert_eq!(
            CbError::Network(GhError::Network("x".into())).exit(),
            Exit::Network
        );
        assert_eq!(
            CbError::Network(GhError::GraphQl("x".into())).exit(),
            Exit::Network
        );
        assert_eq!(
            CbError::Network(GhError::Auth("x".into())).exit(),
            Exit::Config
        );
        assert_eq!(
            CbError::Network(GhError::InsufficientScopes {
                needed: vec!["read:project".into()],
                granted: vec![],
                other: vec![],
            })
            .exit(),
            Exit::Config
        );
        assert_eq!(CbError::config("x").exit(), Exit::Config);
        // None of the error paths may yield the "no events" code.
        for e in [
            CbError::refused("x", "m"),
            CbError::Network(GhError::Network("x".into())),
            CbError::config("x"),
        ] {
            assert_ne!(e.exit(), Exit::NoEvents);
        }
    }

    #[test]
    fn protocol_errors_map_by_kind() {
        let e: CbError = ProtocolError::BadKey("x".into()).into();
        assert_eq!(e.exit(), Exit::Config);
        let e: CbError = ProtocolError::BodyTooLong(70_000).into();
        assert_eq!(e.exit(), Exit::Refused);
    }
}
