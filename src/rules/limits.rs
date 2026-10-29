//! Size limits (spec/board#fields text fields, spec/messages#format message bodies). Pure checks.
//!
//! `board.rs` checks them before every text-field write, and `ops/say.rs` before every post.

use super::{Refusal, Verdict};

/// A project text field holds at most this many bytes of UTF-8 (spec/board#fields).
pub const MAX_TEXT_FIELD_BYTES: usize = 1024;

/// A message body has at most this many characters (spec/messages#format).
pub const MAX_BODY_CHARS: usize = 65_536;

/// Check a text field value before a write. Counts bytes, not characters: GitHub's limit
/// is in bytes, and `ä` is two bytes.
pub fn check_text_field(value: &str) -> Verdict {
    let n = value.len();
    if n > MAX_TEXT_FIELD_BYTES {
        return Err(Refusal::new(
            "text-field-too-long",
            format!("text field is {n} bytes (UTF-8); the limit is {MAX_TEXT_FIELD_BYTES}"),
        ));
    }
    Ok(())
}

/// Check a message body. Counts characters (Unicode scalar values).
pub fn check_body(body: &str) -> Verdict {
    let n = body.chars().count();
    if n > MAX_BODY_CHARS {
        return Err(Refusal::new(
            "body-too-long",
            format!("message body is {n} characters; the limit is {MAX_BODY_CHARS}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_field_of_exactly_1024_bytes_passes() {
        assert!(check_text_field(&"a".repeat(1024)).is_ok());
        assert!(check_text_field("").is_ok());
    }

    #[test]
    fn text_field_of_1025_bytes_is_refused() {
        let e = check_text_field(&"a".repeat(1025)).unwrap_err();
        assert_eq!(e.rule, "text-field-too-long");
    }

    #[test]
    fn text_field_counts_bytes_not_chars() {
        // 600 chars of 2 bytes each: under 1024 chars, 1200 bytes.
        let s = "ä".repeat(600);
        assert!(s.chars().count() < 1024 && s.len() > 1024);
        assert_eq!(
            check_text_field(&s).unwrap_err().rule,
            "text-field-too-long"
        );
        // 512 two-byte chars are exactly 1024 bytes and pass.
        assert!(check_text_field(&"ä".repeat(512)).is_ok());
    }

    #[test]
    fn body_of_exactly_65536_chars_passes() {
        assert!(check_body(&"a".repeat(MAX_BODY_CHARS)).is_ok());
        // Multibyte chars count once each.
        assert!(check_body(&"ä".repeat(MAX_BODY_CHARS)).is_ok());
    }

    #[test]
    fn body_of_65537_chars_is_refused() {
        assert_eq!(
            check_body(&"a".repeat(MAX_BODY_CHARS + 1))
                .unwrap_err()
                .rule,
            "body-too-long"
        );
    }
}
