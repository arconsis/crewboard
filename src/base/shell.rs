//! Words for a POSIX shell. One implementation, so every command line that crewboard prints or
//! writes quotes the same way.

/// A word for a POSIX shell: in single quotes; a single quote inside ends the quotes, adds
/// an escaped quote and opens the quotes again (`'a'\''b'`). Every command line that crewboard
/// prints for a person to run quotes text that an agent controls (a branch, a path, a name)
/// with this.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// One word for a shell: as it is when it has only plain characters, else in single quotes
/// ([`quote`]). The empty word is quoted.
pub fn quote_if_needed(w: &str) -> String {
    let plain = !w.is_empty()
        && w.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-./:=@%+,".contains(&b));
    if plain { w.to_string() } else { quote(w) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_always_quotes_and_escapes_a_single_quote() {
        assert_eq!(quote("a"), "'a'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn quote_if_needed_leaves_a_plain_word_as_it_is() {
        assert_eq!(
            quote_if_needed("plain-1/x_y.z:a=b@c%d+e,f"),
            "plain-1/x_y.z:a=b@c%d+e,f"
        );
        assert_eq!(quote_if_needed(""), "''");
        assert_eq!(quote_if_needed("a b"), "'a b'");
        assert_eq!(quote_if_needed("it's"), r"'it'\''s'");
        // a letter outside ASCII is not plain
        assert_eq!(quote_if_needed("\u{e4}"), "'\u{e4}'");
    }
}
