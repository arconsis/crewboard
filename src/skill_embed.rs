//! The skill files that `build.rs` embeds in the binary, and two pure helpers for them: the
//! SHA-256 of a file and the version line of a `SKILL.md`.
//!
//! The table has the files of `skill/` except `loops/tests/` and dot files.
//! `tests/skill_embed.rs` compares it with the directory.

use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/skill_files.rs"));

/// The text of an embedded file, by its path relative to `skill/`.
pub fn embedded(path: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .find(|(p, _, _)| *p == path)
        .map(|(_, b, _)| *b)
}

/// SHA-256 of `bytes` as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The `version:` of the `metadata:` map in the front matter of a `SKILL.md`. It equals the
/// version of the binary of the same commit (`tests/skill_version.rs`). A top-level `version:`
/// (the old form) and a `version:` in the body do not count. One pair of quotes around the value
/// is removed. `None` when there is no such line or its value is empty.
pub fn skill_version(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut in_metadata = false;
    for line in lines {
        if line == "---" {
            return None;
        }
        if line.trim().is_empty() {
            continue; // a blank line does not end the map
        }
        if !line.starts_with([' ', '\t']) {
            in_metadata = line
                .strip_prefix("metadata:")
                .is_some_and(|rest| rest.trim_matches([' ', '\t']).is_empty());
            continue;
        }
        if in_metadata
            && let Some(v) = line
                .trim_start_matches([' ', '\t'])
                .strip_prefix("version:")
        {
            return clean_value(v);
        }
    }
    None
}

/// The value after `version:`: leading blanks and one leading quote go, and a trailing quote with
/// the blanks behind it. (The same rule as the shell installer in `skill/`.)
fn clean_value(v: &str) -> Option<String> {
    let v = v.trim_start_matches([' ', '\t']);
    let v = v.strip_prefix(['"', '\'']).unwrap_or(v);
    let trimmed = v.trim_end_matches([' ', '\t']);
    let v = trimmed.strip_suffix(['"', '\'']).unwrap_or(v);
    (!v.is_empty()).then(|| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comes_from_the_metadata_map_only() {
        let ok = "---\nname: crewboard\nmetadata:\n  version: \"9.9.9\"\n---\n";
        assert_eq!(skill_version(ok).as_deref(), Some("9.9.9"));
        let single = "---\nmetadata:\n  version: '1.2.3'\n---\n";
        assert_eq!(skill_version(single).as_deref(), Some("1.2.3"));
        let bare = "---\nmetadata:\n\tversion: 4.5.6\n---\n";
        assert_eq!(skill_version(bare).as_deref(), Some("4.5.6"));
    }

    #[test]
    fn a_blank_line_inside_metadata_does_not_end_the_map() {
        let t = "---\nmetadata:\n  author: a\n\n  version: \"3.2.1\"\n---\n";
        assert_eq!(skill_version(t).as_deref(), Some("3.2.1"));
    }

    #[test]
    fn a_top_level_or_body_version_does_not_count() {
        let top = "---\nname: crewboard\nversion: 9.9.9\n---\n";
        assert_eq!(skill_version(top), None);
        let body = "---\nname: crewboard\n---\n\nversion: 9.9.9 in the body\n";
        assert_eq!(skill_version(body), None);
        let other_map = "---\nlicense: x\nmetadata_extra:\n  version: 1\nmetadata:\n  author: a\n---\n  version: 2\n";
        assert_eq!(skill_version(other_map), None);
        assert_eq!(
            skill_version("no front matter\nmetadata:\n  version: 1\n"),
            None
        );
        assert_eq!(
            skill_version("---\nmetadata:\n  version: \"\"\n---\n"),
            None
        );
    }

    #[test]
    fn the_embedded_skill_has_a_version_and_the_sha_is_hex() {
        let md = embedded("SKILL.md").expect("SKILL.md is embedded");
        let text = std::str::from_utf8(md).unwrap();
        assert!(skill_version(text).is_some());
        assert_eq!(
            sha256_hex(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
