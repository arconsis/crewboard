//! Build script: embed the skill files (`skill/`) in the binary.
//!
//! It walks `skill/` and writes `$OUT_DIR/skill_files.rs`, a table
//! `pub static FILES: &[(&str, &[u8], bool)]` of (path relative to `skill/`, file bytes, executable).
//! `src/skill_embed.rs` includes it. Left out: `loops/tests/` (the shell tests stay in the clone),
//! and dot files and directories. A file is executable when its mode says so (Unix).

#![allow(clippy::expect_used, reason = "a build script fails by panic")]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn skipped(rel: &str) -> bool {
    rel.starts_with("loops/tests/") || rel.split('/').any(|c| c.starts_with('.'))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("read_dir entry").path();
        let rel = path
            .strip_prefix(root)
            .expect("path under skill/")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if path.is_dir() {
            if !skipped(&format!("{rel}/")) {
                walk(root, &path, out);
            }
        } else if !skipped(&rel) {
            out.push((rel, path));
        }
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.join("skill");
    let mut files = Vec::new();
    walk(&root, &root, &mut files);
    files.sort();
    let mut table = String::from("pub static FILES: &[(&str, &[u8], bool)] = &[\n");
    for (rel, path) in &files {
        let abs = path.to_string_lossy();
        let exec = is_executable(path);
        // writing to a String cannot fail
        writeln!(table, "    ({rel:?}, include_bytes!({abs:?}), {exec}),").ok();
    }
    table.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("skill_files.rs");
    std::fs::write(&out, table).unwrap_or_else(|e| panic!("write {}: {e}", out.display()));
    println!("cargo:rerun-if-changed=skill");
    println!("cargo:rerun-if-changed=build.rs");
}
