//! Words of a command line: split a string the way a POSIX shell does (single quotes, double
//! quotes, backslash) and quote a word so that a shell reads it back as that one word. Only what
//! `--runtime-flags` and `--print-cmd` need: no variables, no globs, no substitution.

/// Split `s` into words. Blanks separate words. `'...'` is literal. In `"..."` a backslash
/// escapes `"`, `\`, `$`, `` ` `` and a line feed; before another character it is kept. Outside
/// quotes a backslash makes the next character literal. An empty quoted string is an empty word.
/// An open quote or a trailing backslash is an error.
pub fn split(s: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut it = s.chars();
    while let Some(c) = it.next() {
        match c {
            ' ' | '\t' | '\n' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match it.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err("a single quote is not closed".to_string()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match it.next() {
                        Some('"') => break,
                        Some('\\') => match it.next() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => cur.push(n),
                            Some('\n') => {}
                            Some(n) => {
                                cur.push('\\');
                                cur.push(n);
                            }
                            None => return Err("a double quote is not closed".to_string()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err("a double quote is not closed".to_string()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match it.next() {
                    Some('\n') => {}
                    Some(n) => cur.push(n),
                    None => return Err("a backslash ends the text".to_string()),
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// One word for a shell: as it is when it has only plain characters, else in single quotes.
pub fn quote(w: &str) -> String {
    let plain = !w.is_empty()
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:=@%+,".contains(c));
    if plain {
        w.to_string()
    } else {
        format!("'{}'", w.replace('\'', "'\\''"))
    }
}

/// The words as one shell line.
pub fn join<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| quote(w.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        split(s).unwrap()
    }

    #[test]
    fn blanks_quotes_and_backslashes() {
        assert_eq!(v("  a  b\tc\nd "), ["a", "b", "c", "d"]);
        assert_eq!(
            v("--permission-mode acceptEdits"),
            ["--permission-mode", "acceptEdits"]
        );
        assert_eq!(v("'a b' \"c d\" e\\ f"), ["a b", "c d", "e f"]);
        assert_eq!(v("a'b c'd\"e f\"g"), ["ab cde fg"]);
        assert_eq!(v("'it'\\''s'"), ["it's"]);
        assert_eq!(v(r#""say \"hi\" \\ \$x \a""#), [r#"say "hi" \ $x \a"#]);
        assert_eq!(v("'' \"\" x"), ["", "", "x"]);
        assert_eq!(
            v("--allowedTools 'Bash(npm *),Read'"),
            ["--allowedTools", "Bash(npm *),Read"]
        );
        assert!(v("").is_empty() && v("   ").is_empty());
    }

    #[test]
    fn an_open_quote_or_a_trailing_backslash_is_an_error() {
        for bad in ["'a", "\"a", "a\\", "x 'y z"] {
            assert!(split(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn quote_gives_back_what_split_reads() {
        for w in [
            "plain",
            "a b",
            "it's",
            "",
            "Bash(git *)",
            "x\"y",
            "$HOME",
            "a\nb",
            "--model",
            "p/a-t_h.x:1=2,3@4%5+6",
        ] {
            assert_eq!(split(&quote(w)).unwrap(), [w], "{w:?}");
        }
        assert_eq!(quote("plain-1/x"), "plain-1/x");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(join(&["cd", "a b", "&&"]), "cd 'a b' '&&'");
    }
}
