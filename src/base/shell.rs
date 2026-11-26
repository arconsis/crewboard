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

/// Split a command line into words like a POSIX shell: spaces separate, single quotes keep
/// everything, double quotes keep everything but `\"` and `\\`, a backslash outside quotes
/// keeps the next character. Nothing else of a shell is done (no variable, no glob, no pipe).
/// The error names a quote that does not end.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut open = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if open {
                    words.push(std::mem::take(&mut cur));
                    open = false;
                }
            }
            '\'' => {
                open = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("a single quote does not end".to_string()),
                    }
                }
            }
            '"' => {
                open = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\')) => cur.push(x),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err("a double quote does not end".to_string()),
                        },
                        Some(x) => cur.push(x),
                        None => return Err("a double quote does not end".to_string()),
                    }
                }
            }
            '\\' => {
                open = true;
                match chars.next() {
                    Some(x) => cur.push(x),
                    None => return Err("a backslash at the end".to_string()),
                }
            }
            c => {
                open = true;
                cur.push(c);
            }
        }
    }
    if open {
        words.push(cur);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_words_splits_like_a_shell_and_runs_nothing() {
        let w = |s: &str| split_words(s).unwrap();
        assert_eq!(w("  ./check.sh   all "), ["./check.sh", "all"]);
        assert_eq!(w("a 'b c' \"d e\" f\\ g"), ["a", "b c", "d e", "f g"]);
        assert_eq!(w("x '' \"\""), ["x", "", ""]);
        assert_eq!(w("say \"a \\\"b\\\" $HOME\""), ["say", "a \"b\" $HOME"]);
        assert_eq!(w("a|b >c *.rs"), ["a|b", ">c", "*.rs"]);
        assert!(w("").is_empty());
        assert!(split_words("a 'b").unwrap_err().contains("single quote"));
        assert!(split_words("a \"b").unwrap_err().contains("double quote"));
        assert!(split_words("a \\").unwrap_err().contains("backslash"));
    }

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
