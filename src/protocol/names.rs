//! Typed header fields: agent names, kinds, addressees, references, timestamps.
//!
//! The vocabulary of `header.rs`. A value that does not parse is refused before it is signed
//! or sent.

use super::ProtocolError;
use std::fmt;

fn bad<T>(m: impl Into<String>) -> Result<T, ProtocolError> {
    Err(ProtocolError::Malformed(m.into()))
}

/// Agent name `<runtime>@<host>` (spec/identity#agent-names): lowercase `[a-z0-9-]`, at most 40
/// characters in total, runtime and host both non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentName(String);

pub const MAX_AGENT_NAME: usize = 40;

impl AgentName {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        if s.chars().count() > MAX_AGENT_NAME {
            return bad(format!(
                "agent name longer than {MAX_AGENT_NAME} characters"
            ));
        }
        let Some((runtime, host)) = s.split_once('@') else {
            return bad("agent name must be <runtime>@<host>");
        };
        let ok = |p: &str| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        if !ok(runtime) || !ok(host) {
            return bad("agent name allows only lowercase a-z, 0-9 and '-' around one '@'");
        }
        Ok(AgentName(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The host part, after the `@`. [`AgentName::parse`] is the only way to make a name, and it
    /// refuses a name without one.
    #[allow(
        clippy::expect_used,
        reason = "parse makes sure that the name has one '@'"
    )]
    pub fn host(&self) -> &str {
        self.0
            .split_once('@')
            .map(|(_, h)| h)
            .expect("an agent name has one '@'")
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Message kind (spec/messages#format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Request,
    Ack,
    Report,
    Done,
    Blocked,
    Question,
    Answer,
    /// Retired as a liveness probe (liveness is the heartbeat age). Still a known kind: old
    /// comments parse and verify, and `selftest` posts one to itself. No other code sends it.
    Ping,
    /// Retired with `ping`. Parsed and verified, never sent by a liveness duty.
    Pong,
    Info,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Request,
        Kind::Ack,
        Kind::Report,
        Kind::Done,
        Kind::Blocked,
        Kind::Question,
        Kind::Answer,
        Kind::Ping,
        Kind::Pong,
        Kind::Info,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Request => "request",
            Kind::Ack => "ack",
            Kind::Report => "report",
            Kind::Done => "done",
            Kind::Blocked => "blocked",
            Kind::Question => "question",
            Kind::Answer => "answer",
            Kind::Ping => "ping",
            Kind::Pong => "pong",
            Kind::Info => "info",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s).map_or_else(
            || bad(format!("unknown kind '{s}'; valid kinds: {}", Kind::list())),
            Ok,
        )
    }

    /// The valid kinds, comma separated, in the order of [`Kind::ALL`].
    pub fn list() -> String {
        Kind::ALL.map(Kind::as_str).join(", ")
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Addressee (`to=`): an agent, `coordinator`, `owner` or `all` (spec/messages#format).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum To {
    Agent(AgentName),
    Coordinator,
    Owner,
    All,
}

impl To {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        match s {
            "coordinator" => Ok(To::Coordinator),
            "owner" => Ok(To::Owner),
            "all" => Ok(To::All),
            other => AgentName::parse(other).map(To::Agent),
        }
    }
}

impl fmt::Display for To {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            To::Agent(a) => write!(f, "{a}"),
            To::Coordinator => f.write_str("coordinator"),
            To::Owner => f.write_str("owner"),
            To::All => f.write_str("all"),
        }
    }
}

/// Reference to the message this one answers: `#<issue>/c-<comment id>` or `none`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Re {
    None,
    Comment { issue: u64, comment: u64 },
}

impl Re {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        if s == "none" {
            return Ok(Re::None);
        }
        let parsed = s
            .strip_prefix('#')
            .and_then(|r| r.split_once("/c-"))
            .and_then(|(i, c)| {
                let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
                if digits(i) && digits(c) {
                    Some((i.parse().ok()?, c.parse().ok()?))
                } else {
                    None
                }
            });
        match parsed {
            Some((issue, comment)) => Ok(Re::Comment { issue, comment }),
            None => bad(format!("bad re '{s}', expected #<issue>/c-<id> or none")),
        }
    }
}

impl fmt::Display for Re {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Re::None => f.write_str("none"),
            Re::Comment { issue, comment } => write!(f, "#{issue}/c-{comment}"),
        }
    }
}

/// UTC timestamp in the exact form `YYYY-MM-DDTHH:MM:SSZ`. Stored as text because
/// it is part of the signed canonical form and must round-trip byte for byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timestamp(String);

impl Timestamp {
    pub fn parse(s: &str) -> Result<Self, ProtocolError> {
        let b = s.as_bytes();
        let shape = b"dddd-dd-ddTdd:dd:ddZ";
        let shape_ok = b.len() == shape.len()
            && b.iter().zip(shape).all(|(c, p)| match p {
                b'd' => c.is_ascii_digit(),
                other => c == other,
            });
        if !shape_ok {
            return bad(format!(
                "bad timestamp '{s}', expected YYYY-MM-DDTHH:MM:SSZ"
            ));
        }
        let n = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(99);
        let (y, mo, d, h, mi, se) = (n(0..4), n(5..7), n(8..10), n(11..13), n(14..16), n(17..19));
        if !crate::base::valid_civil_time(y, mo, d, h, mi, se) {
            return bad(format!("timestamp '{s}' is out of range"));
        }
        Ok(Timestamp(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_names_accepted() {
        for ok in [
            "claude@host-a",
            "hermes@host-b",
            "codex@host-c",
            "claude-2@host-c",
        ] {
            assert!(AgentName::parse(ok).is_ok(), "{ok}");
        }
        // Exactly 40 characters is allowed.
        let name = format!("{}@h", "a".repeat(38));
        assert_eq!(name.len(), 40);
        assert!(AgentName::parse(&name).is_ok());
    }

    #[test]
    fn the_host_of_an_agent_name_is_the_part_after_the_at() {
        assert_eq!(
            AgentName::parse("claude-2@host-c").unwrap().host(),
            "host-c"
        );
    }

    #[test]
    fn agent_names_refused() {
        let too_long = format!("{}@h", "a".repeat(39));
        assert_eq!(too_long.len(), 41);
        for bad in [
            "",
            "claude",
            "@host-a",
            "claude@",
            "Claude@host-a",
            "claude@HOST-A",
            "claude@host_a",
            "claude@host-a.local",
            "a@b@c",
            "claude @host-a",
            "cläude@host-a",
            too_long.as_str(),
        ] {
            let e = AgentName::parse(bad).expect_err(bad);
            assert_eq!(e.rule(), "malformed-message");
        }
    }

    #[test]
    fn kinds_round_trip_and_unknown_is_refused() {
        assert_eq!(Kind::ALL.len(), 10);
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()).unwrap(), k);
        }
        assert!(Kind::parse("shout").is_err());
        assert!(Kind::parse("Report").is_err());
    }

    #[test]
    fn to_parses_roles_and_agents() {
        assert_eq!(To::parse("owner").unwrap(), To::Owner);
        assert_eq!(To::parse("all").unwrap(), To::All);
        assert_eq!(To::parse("coordinator").unwrap(), To::Coordinator);
        assert_eq!(
            To::parse("claude@host-a").unwrap().to_string(),
            "claude@host-a"
        );
        assert!(To::parse("everyone").is_err());
    }

    #[test]
    fn re_round_trip_and_refusal() {
        assert_eq!(Re::parse("none").unwrap(), Re::None);
        let r = Re::parse("#12/c-2361455").unwrap();
        assert_eq!(
            r,
            Re::Comment {
                issue: 12,
                comment: 2361455
            }
        );
        assert_eq!(r.to_string(), "#12/c-2361455");
        for bad in [
            "", "#12", "12/c-3", "#12/c-", "#/c-3", "#1/c-x", "None", "#12/3",
        ] {
            assert!(Re::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn timestamp_accepted_and_refused() {
        assert!(Timestamp::parse("2026-10-15T14:05:00Z").is_ok());
        for bad in [
            "2026-10-15 14:05:00Z",
            "2026-10-15T14:05:00",
            "2026-10-15T14:05:00+02:00",
            "2026-13-15T14:05:00Z",
            "2026-10-15T24:05:00Z",
            "26-10-15T14:05:00Z",
            "2026-02-31T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-02-29T00:00:00Z",
            "2026-10-15T14:05:60Z",
            "2026-00-10T00:00:00Z",
            "2026-10-00T00:00:00Z",
        ] {
            assert!(Timestamp::parse(bad).is_err(), "{bad}");
        }
        // a leap day of a leap year exists, and so do the last second and the last day
        for ok in [
            "2028-02-29T00:00:00Z",
            "2000-02-29T00:00:00Z",
            "2026-12-31T23:59:59Z",
        ] {
            assert!(Timestamp::parse(ok).is_ok(), "{ok}");
        }
    }
}
