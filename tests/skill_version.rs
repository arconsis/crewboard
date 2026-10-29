//! Guard test: the version of `skill/SKILL.md` equals the version of the binary.
//!
//! The skill and the binary come from one commit. `crewboard skill install` prints the skill version,
//! and `crewboard --version` prints the package version. The two must be the same, so a user
//! can compare them after an upgrade.
//!
//! The version lives in the `metadata:` map of the front matter (`metadata.version`), because
//! strict skill loaders accept only the keys of the Agent Skills spec at the top level.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use std::path::Path;

/// The `version:` value of the `metadata:` map of the front matter (the block between the first
/// two `---` lines). A top-level `version:` (the old form) and a `version:` in the body do not
/// count. Quotes around the value are removed.
fn front_matter_version(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut in_metadata = false;
    for line in lines {
        if line == "---" {
            return None;
        }
        if !line.starts_with([' ', '\t']) {
            in_metadata = line.trim_end() == "metadata:";
            continue;
        }
        if in_metadata && let Some(v) = line.trim_start().strip_prefix("version:") {
            let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
            return (!v.is_empty()).then(|| v.to_string());
        }
    }
    None
}

#[test]
fn skill_version_equals_the_package_version() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(root.join("skill/SKILL.md")).expect("read skill/SKILL.md");
    assert_eq!(
        front_matter_version(&text).as_deref(),
        Some(env!("CARGO_PKG_VERSION")),
        "skill/SKILL.md front matter `metadata.version` must equal the version in Cargo.toml"
    );
}

#[test]
fn the_parser_reads_only_the_metadata_version_of_the_front_matter() {
    // Pass: the line in the metadata map, quoted or not, after another metadata key.
    assert_eq!(
        front_matter_version("---\nname: x\nmetadata:\n  version: \"1.2.3\"\n---\nbody\n")
            .as_deref(),
        Some("1.2.3")
    );
    assert_eq!(
        front_matter_version("---\nmetadata:\n  author: a\n  version: 1.2.4\nlicense: MIT\n---\n")
            .as_deref(),
        Some("1.2.4")
    );
    // Refusals: no front matter, a version only in the body, an empty value, the old
    // top-level form, and a `version:` under another map.
    assert_eq!(front_matter_version("version: 1.2.3\n"), None);
    assert_eq!(
        front_matter_version("---\nname: x\n---\nmetadata:\n  version: 1.2.3\n"),
        None
    );
    assert_eq!(
        front_matter_version("---\nmetadata:\n  version: \"\"\n---\n"),
        None
    );
    assert_eq!(
        front_matter_version("---\nname: x\nversion: 1.2.3\n---\n"),
        None
    );
    assert_eq!(
        front_matter_version("---\nmetadata:\n  a: b\nother:\n  version: 1.2.3\n---\n"),
        None
    );
}
