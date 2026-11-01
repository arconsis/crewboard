//! Parse and print the hidden header, the visible line and the body (spec/messages#format).
//!
//! ```text
//! <!-- crew v=1 from=hermes@host-b to=claude@host-a kind=report re=#12/c-2361455 ts=... sig=... -->
//! **hermes@host-b -> claude@host-a** · report
//!
//! body
//! ```

use super::names::{AgentName, Kind, Re, Timestamp, To};
use super::{PROTOCOL_VERSION, ProtocolError};

/// The start of every crew comment.
pub const PREFIX: &str = "<!-- crew ";
const SUFFIX: &str = " -->";

/// The header fields. `sig` is the base64 `sshsig` blob (no line breaks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub from: AgentName,
    pub to: To,
    pub kind: Kind,
    pub re: Re,
    pub ts: Timestamp,
    pub sig: String,
}

/// A parsed message. `body` is trimmed and uses `\n` line ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub header: Header,
    pub body: String,
}

fn malformed<T>(m: impl Into<String>) -> Result<T, ProtocolError> {
    Err(ProtocolError::Malformed(m.into()))
}

impl Header {
    /// The visible second line, derived from the header.
    pub fn visible_line(&self) -> String {
        format!("**{} -> {}** \u{b7} {}", self.from, self.to, self.kind)
    }

    /// The hidden first line.
    pub fn to_line(&self) -> String {
        format!(
            "{PREFIX}v={PROTOCOL_VERSION} from={} to={} kind={} re={} ts={} sig={}{SUFFIX}",
            self.from, self.to, self.kind, self.re, self.ts, self.sig
        )
    }

    pub fn parse_line(line: &str) -> Result<Self, ProtocolError> {
        let inner = line
            .strip_prefix(PREFIX)
            .and_then(|l| l.strip_suffix(SUFFIX))
            .map_or_else(|| malformed("first line is not a crew header"), Ok)?;
        let (mut v, mut from, mut to, mut kind, mut re, mut ts, mut sig) =
            (None, None, None, None, None, None, None);
        for tok in inner.split(' ') {
            let Some((k, val)) = tok.split_once('=') else {
                return malformed(format!("header token '{tok}' is not key=value"));
            };
            let slot = match k {
                "v" => &mut v,
                "from" => &mut from,
                "to" => &mut to,
                "kind" => &mut kind,
                "re" => &mut re,
                "ts" => &mut ts,
                "sig" => &mut sig,
                other => return malformed(format!("unknown header field '{other}'")),
            };
            if slot.replace(val).is_some() {
                return malformed(format!("duplicate header field '{k}'"));
            }
        }
        fn need<'a>(o: Option<&'a str>, n: &str) -> Result<&'a str, ProtocolError> {
            o.map_or_else(|| malformed(format!("missing header field '{n}'")), Ok)
        }
        let version = need(v, "v")?;
        if version != PROTOCOL_VERSION {
            return malformed(format!("unsupported protocol version '{version}'"));
        }
        let sig = need(sig, "sig")?;
        if sig.is_empty()
            || !sig
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
        {
            return malformed("sig is not base64");
        }
        Ok(Header {
            from: AgentName::parse(need(from, "from")?)?,
            to: To::parse(need(to, "to")?)?,
            kind: Kind::parse(need(kind, "kind")?)?,
            re: Re::parse(need(re, "re")?)?,
            ts: Timestamp::parse(need(ts, "ts")?)?,
            sig: sig.to_string(),
        })
    }
}

impl Message {
    /// Print the full comment text.
    pub fn to_comment(&self) -> String {
        format!(
            "{}\n{}\n\n{}\n",
            self.header.to_line(),
            self.header.visible_line(),
            self.body.trim()
        )
    }

    /// Parse a comment. `\r\n` is normalised to `\n` first, because GitHub may return
    /// comments typed in the web UI with `\r\n`. The visible line must match the header,
    /// because it is not signed and a mismatch would show a false sender.
    pub fn parse(comment: &str) -> Result<Self, ProtocolError> {
        let text = comment.replace("\r\n", "\n");
        let mut parts = text.splitn(3, '\n');
        let first = parts.next().unwrap_or("");
        let header = Header::parse_line(first)?;
        let visible = match parts.next() {
            Some(v) => v,
            None => return malformed("missing visible line"),
        };
        if visible != header.visible_line() {
            return Err(ProtocolError::VisibleLineMismatch);
        }
        let body = parts.next().unwrap_or("").trim().to_string();
        Ok(Message { header, body })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample_header() -> Header {
        Header {
            from: AgentName::parse("hermes@host-b").unwrap(),
            to: To::parse("claude@host-a").unwrap(),
            kind: Kind::Report,
            re: Re::parse("#12/c-2361455").unwrap(),
            ts: Timestamp::parse("2026-10-15T14:05:00Z").unwrap(),
            sig: "QUJD".into(),
        }
    }

    const SPEC_EXAMPLE: &str = "<!-- crew v=1 from=hermes@host-b to=claude@host-a kind=report re=#12/c-2361455 ts=2026-10-15T14:05:00Z sig=QUJD -->\n**hermes@host-b -> claude@host-a** \u{b7} report\n\nCI on x86 is red only in `parser::tests::flake_7`.\nLog: <CI run URL>.\n";

    #[test]
    fn prints_the_spec_example_layout() {
        let m = Message {
            header: sample_header(),
            body: "CI on x86 is red only in `parser::tests::flake_7`.\nLog: <CI run URL>.".into(),
        };
        assert_eq!(m.to_comment(), SPEC_EXAMPLE);
    }

    #[test]
    fn parse_print_round_trip() {
        let m = Message::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(m.header, sample_header());
        assert_eq!(m.to_comment(), SPEC_EXAMPLE);
    }

    #[test]
    fn crlf_from_the_web_ui_is_normalised() {
        let crlf = SPEC_EXAMPLE.replace('\n', "\r\n");
        assert_eq!(
            Message::parse(&crlf).unwrap(),
            Message::parse(SPEC_EXAMPLE).unwrap()
        );
    }

    #[test]
    fn empty_body_is_allowed() {
        let text = format!(
            "{}\n{}\n",
            sample_header().to_line(),
            sample_header().visible_line()
        );
        assert_eq!(Message::parse(&text).unwrap().body, "");
    }

    #[test]
    fn mismatching_visible_line_is_refused() {
        let forged = SPEC_EXAMPLE.replace("**hermes@host-b ->", "**claude@host-a ->");
        let e = Message::parse(&forged).unwrap_err();
        assert_eq!(e, ProtocolError::VisibleLineMismatch);
    }

    #[test]
    fn broken_headers_are_refused() {
        let line = sample_header().to_line();
        let cases = [
            "plain owner comment".to_string(),
            line.replace("<!-- crew ", "<!-- other "),
            line.replace(" -->", ""),
            line.replace("v=1", "v=2"),
            line.replace(" kind=report", ""),
            line.replace("kind=report", "kind=shout"),
            line.replace("from=hermes@host-b", "from=Hermes"),
            line.replace("ts=2026-10-15T14:05:00Z", "ts=yesterday"),
            line.replace("sig=QUJD", "sig=not base64!"),
            line.replace("sig=QUJD", "sig=QUJD sig=QUJD"),
            line.replace("sig=QUJD", "sig=QUJD extra=1"),
        ];
        for c in cases {
            let text = format!("{c}\n{}\n\nbody", sample_header().visible_line());
            assert!(Message::parse(&text).is_err(), "should refuse: {c}");
        }
    }
}
