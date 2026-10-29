//! Guard test: the skill files embedded in the binary are the files of `skill/` on disk.
//!
//! `build.rs` embeds every file of `skill/` except `loops/tests/` and dot files and directories.
//! The table must hold exactly those files with equal bytes. The skill has no script.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use crewboard::skill_embed::{FILES, embedded};
use std::collections::BTreeMap;
use std::path::Path;

fn excluded(rel: &str) -> bool {
    rel.starts_with("loops/tests/") || rel.split('/').any(|c| c.starts_with('.'))
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(root, &path, out);
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !excluded(&rel) {
            out.insert(rel, std::fs::read(&path).unwrap());
        }
    }
}

fn on_disk() -> BTreeMap<String, Vec<u8>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("skill");
    let mut files = BTreeMap::new();
    walk(&root, &root, &mut files);
    files
}

#[test]
fn the_embedded_table_has_exactly_the_files_of_skill_with_equal_bytes() {
    let disk = on_disk();
    let table: BTreeMap<&str, &[u8]> = FILES.iter().map(|(p, b, _)| (*p, *b)).collect();
    assert_eq!(table.len(), FILES.len(), "a path is in the table twice");
    let disk_names: Vec<&str> = disk.keys().map(String::as_str).collect();
    let table_names: Vec<&str> = table.keys().copied().collect();
    assert_eq!(table_names, disk_names, "the table and skill/ differ");
    for (path, bytes) in &disk {
        assert_eq!(table[path.as_str()], bytes.as_slice(), "bytes of {path}");
    }
}

#[test]
fn what_is_left_out_is_not_in_the_table() {
    // The red direction of the test above: the files that must be absent exist on disk.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("skill");
    assert!(root.join("loops/tests/test-loops.sh").is_file());
    for (path, _, _) in FILES {
        assert!(!excluded(path), "{path} must not be embedded");
    }
    assert!(embedded("SKILL.md").is_some());
    assert!(embedded("loops/tests/test-loops.sh").is_none());
}

#[test]
fn the_skill_has_no_script_and_no_file_needs_the_executable_flag() {
    assert!(
        !FILES.iter().any(|(p, _, _)| p.ends_with(".sh")),
        "the skill has a shell script"
    );
    for (path, _, exec) in FILES {
        assert!(!exec, "{path} is marked executable");
    }
}
