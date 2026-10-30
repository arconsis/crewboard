//! Layering freeze: builds the import graph of the top-level modules from `crate::<module>`
//! references in production code (the first inline `#[cfg(test)] mod x {` to the end of a file,
//! every `*_tests.rs` file and `testkit.rs`, the builders of the unit tests, are skipped;
//! `ops/tickets/tick.rs` counts as `ops`). An edge that is not in `ALLOWED` fails the test. `base` may be imported by every module and imports nothing itself. Remove an edge
//! from the list when a refactor removes it, never add one silently.
//!
//! `domain` is the pure layer: it imports `base`, and `rules` and `authority` (pure modules that
//! have not moved under `domain` yet), never `github`, `services`, `ops`, `config` or `clock`.
//! `services` reads from `github` and hands the answers to `domain`.
//!
//! Inside `ops` and `lock` the same rule holds one level down: the import graph of the files has
//! no cycle (`the_files_of_ops_and_lock_have_no_cycle`). A helper that two commands share lives
//! below both: in `ctx`, or in a small module with its own name, never in one of the commands.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Allowed `from -> to` edges, sorted. Each line: (importer, imported).
const ALLOWED: &[(&str, &str)] = &[
    ("agent_env", "base"),
    ("agent_env", "error"),
    ("agent_env", "fsutil"),
    ("agent_gh", "base"),
    ("agent_gh", "config"),
    ("agent_gh", "error"),
    ("agent_gh", "spawn"),
    ("audit", "comments"),
    ("audit", "config"),
    ("audit", "protocol"),
    ("audit", "registry"),
    ("audit", "rules"),
    ("audit", "ticket"),
    ("authority", "base"),
    ("authority", "protocol"),
    ("authority", "rules"),
    ("board", "base"),
    ("board", "config"),
    ("board", "error"),
    ("board", "github"),
    ("board", "items"),
    ("board", "rules"),
    ("builtin_workflows", "board"),
    ("builtin_workflows", "github"),
    ("capabilities", "agent_env"),
    ("capabilities", "base"),
    ("capabilities", "clock"),
    ("capabilities", "error"),
    ("capabilities", "rules"),
    ("cli", "agent_env"),
    ("cli", "base"),
    ("cli", "capabilities"),
    ("cli", "clock"),
    ("cli", "config"),
    ("cli", "error"),
    ("cli", "exit"),
    ("cli", "headless"),
    ("cli", "ops"),
    ("cli", "owner_cmd"),
    ("cli", "protocol"),
    ("cli", "rules"),
    ("cli", "state"),
    ("cli", "wake"),
    ("comments", "base"),
    ("comments", "clock"),
    ("comments", "protocol"),
    ("config", "base"),
    ("config", "clock"),
    ("config", "error"),
    ("config", "fsutil"),
    ("config", "protocol"),
    ("ctx", "agent_env"),
    ("ctx", "authority"),
    ("ctx", "base"),
    ("ctx", "board"),
    ("ctx", "clock"),
    ("ctx", "comments"),
    ("ctx", "config"),
    ("ctx", "error"),
    ("ctx", "feed"),
    ("ctx", "github"),
    ("ctx", "items"),
    ("ctx", "late"),
    ("ctx", "protocol"),
    ("ctx", "registry"),
    ("ctx", "rules"),
    ("ctx", "services"),
    ("ctx", "state"),
    ("ctx", "ticket"),
    ("cursor", "clock"),
    ("cursor", "rules"),
    ("domain", "authority"),
    ("domain", "base"),
    ("domain", "rules"),
    ("error", "base"),
    ("error", "exit"),
    ("error", "protocol"),
    ("events", "comments"),
    ("events", "config"),
    ("events", "owner_cmd"),
    ("events", "protocol"),
    ("events", "registry"),
    ("events", "rules"),
    ("feed", "clock"),
    ("feed", "comments"),
    ("feed", "error"),
    ("feed", "github"),
    ("feed", "state"),
    ("fsutil", "base"),
    ("fsutil", "clock"),
    ("github", "agent_env"),
    ("github", "base"),
    ("github", "clock"),
    ("headless", "base"),
    ("headless", "config"),
    ("headless", "error"),
    ("headless", "rules"),
    ("items", "base"),
    ("labels", "github"),
    ("late", "base"),
    ("late", "clock"),
    ("late", "config"),
    ("late", "domain"),
    ("late", "error"),
    ("late", "protocol"),
    ("late", "rules"),
    ("late", "ticket"),
    ("lock", "base"),
    ("lock", "clock"),
    ("lock", "fsutil"),
    ("lock", "spawn"),
    ("ops", "agent_env"),
    ("ops", "agent_gh"),
    ("ops", "audit"),
    ("ops", "authority"),
    ("ops", "base"),
    ("ops", "board"),
    ("ops", "builtin_workflows"),
    ("ops", "capabilities"),
    ("ops", "clock"),
    ("ops", "comments"),
    ("ops", "config"),
    ("ops", "ctx"),
    ("ops", "cursor"),
    ("ops", "domain"),
    ("ops", "error"),
    ("ops", "events"),
    ("ops", "exit"),
    ("ops", "feed"),
    ("ops", "fsutil"),
    ("ops", "github"),
    ("ops", "headless"),
    ("ops", "items"),
    ("ops", "labels"),
    ("ops", "late"),
    ("ops", "lock"),
    ("ops", "owner_cmd"),
    ("ops", "owner_route"),
    ("ops", "profile"),
    ("ops", "protocol"),
    ("ops", "registry"),
    ("ops", "repeat"),
    ("ops", "rules"),
    ("ops", "secrets"),
    ("ops", "services"),
    ("ops", "skill_embed"),
    ("ops", "spawn"),
    ("ops", "state"),
    ("ops", "ticket"),
    ("ops", "toolchain"),
    ("ops", "unread"),
    ("owner_cmd", "base"),
    ("owner_cmd", "capabilities"),
    ("owner_cmd", "protocol"),
    ("owner_cmd", "rules"),
    ("owner_route", "base"),
    ("owner_route", "protocol"),
    ("owner_route", "rules"),
    ("owner_route", "ticket"),
    ("profile", "capabilities"),
    ("profile", "comments"),
    ("profile", "config"),
    ("profile", "owner_cmd"),
    ("profile", "protocol"),
    ("profile", "registry"),
    ("profile", "ticket"),
    ("protocol", "base"),
    ("protocol", "rules"),
    ("registry", "comments"),
    ("registry", "config"),
    ("registry", "items"),
    ("registry", "owner_cmd"),
    ("registry", "protocol"),
    ("registry", "rules"),
    ("rules", "base"),
    ("services", "base"),
    ("services", "board"),
    ("services", "comments"),
    ("services", "config"),
    ("services", "domain"),
    ("services", "error"),
    ("services", "feed"),
    ("services", "github"),
    ("services", "items"),
    ("services", "registry"),
    ("services", "rules"),
    ("services", "state"),
    ("services", "ticket"),
    ("spawn", "base"),
    ("spawn", "clock"),
    ("state", "base"),
    ("state", "clock"),
    ("state", "comments"),
    ("state", "error"),
    ("state", "events"),
    ("state", "fsutil"),
    ("ticket", "clock"),
    ("ticket", "comments"),
    ("ticket", "config"),
    ("ticket", "events"),
    ("ticket", "items"),
    ("ticket", "owner_cmd"),
    ("ticket", "protocol"),
    ("ticket", "registry"),
    ("ticket", "rules"),
    ("toolchain", "agent_env"),
    ("toolchain", "base"),
    ("toolchain", "clock"),
    ("toolchain", "config"),
    ("toolchain", "spawn"),
    ("unread", "base"),
    ("unread", "comments"),
    ("unread", "ctx"),
    ("unread", "cursor"),
    ("unread", "error"),
    ("unread", "events"),
    ("unread", "protocol"),
    ("unread", "state"),
    ("unread", "ticket"),
    ("wake", "agent_env"),
    ("wake", "base"),
    ("wake", "clock"),
    ("wake", "config"),
    ("wake", "ctx"),
    ("wake", "error"),
    ("wake", "fsutil"),
    ("wake", "headless"),
    ("wake", "lock"),
    ("wake", "ops"),
    ("wake", "protocol"),
    ("wake", "spawn"),
];

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A file that only test builds compile: `*_tests.rs` and `testkit.rs`.
fn test_only(file: &Path) -> bool {
    file.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.ends_with("_tests.rs") || n == "testkit.rs"
    })
}

/// The production part of a source file: cut at the first inline test module.
fn production(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let cut = (0..lines.len().saturating_sub(1))
        .find(|&i| lines[i] == "#[cfg(test)]" && lines[i + 1].trim_end().ends_with('{'))
        .unwrap_or(lines.len());
    lines[..cut].join("\n")
}

/// The top-level modules that a piece of production code names with `crate::`: in `use`
/// statements (grouped, nested, aliased, over several lines: `use crate::{a::X, b::{Y, self}};`)
/// and in inline paths (`crate::a::f()`). Comment lines are skipped.
fn crate_targets(code: &str) -> BTreeSet<String> {
    let use_stmt = Regex::new(r"\buse\s+([^;]+);").unwrap();
    let inline = Regex::new(r"\bcrate::(\w+)").unwrap();
    let code: String = code
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = BTreeSet::new();
    for cap in use_stmt.captures_iter(&code) {
        for path in expand_use_tree(&cap[1]) {
            let mut segs = path.split("::").map(str::trim);
            if segs.next() == Some("crate") {
                let name: String = segs
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.insert(name);
                }
            }
        }
    }
    let rest = use_stmt.replace_all(&code, "");
    out.extend(inline.captures_iter(&rest).map(|c| c[1].to_string()));
    out
}

fn edges() -> BTreeSet<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    let mut found = BTreeSet::new();
    for file in files {
        let rel = file.strip_prefix(&root).unwrap();
        let first = rel
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_string_lossy();
        if test_only(&file) {
            continue;
        }
        let from = first.trim_end_matches(".rs").to_string();
        if from == "main" || from == "lib" {
            continue;
        }
        let code = production(&fs::read_to_string(&file).unwrap());
        for to in crate_targets(&code) {
            if to != from {
                found.insert((from.clone(), to));
            }
        }
    }
    found
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn the_scanner_reads_single_grouped_nested_and_multi_line_imports() {
    let src = "\
use crate::board;
use crate::config::Config as Cfg;
use crate::{feed, state::{A, B}};
pub use crate::{
    comments::{self, Comment},
    protocol::names::Kind as K,
    rules::{status::Status, *},
};
use std::collections::BTreeMap;
use super::sibling;
// use crate::github;
fn f() { crate::clock::now(); }
";
    assert_eq!(
        crate_targets(src),
        set(&[
            "board", "clock", "comments", "config", "feed", "protocol", "rules", "state"
        ])
    );
}

#[test]
fn the_scanner_sees_a_planted_grouped_import() {
    // A forbidden edge `domain -> github` written as a grouped, multi-line import in memory.
    let planted = "use crate::{\n    base::Id,\n    github::{Client, self},\n};\n";
    let found: BTreeSet<(String, String)> = crate_targets(planted)
        .into_iter()
        .map(|to| ("domain".to_string(), to))
        .collect();
    let allowed: BTreeSet<(String, String)> = ALLOWED
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let new: Vec<_> = found.difference(&allowed).collect();
    assert_eq!(new, [&("domain".to_string(), "github".to_string())]);
}

#[test]
fn allow_list_is_sorted() {
    assert!(
        ALLOWED.windows(2).all(|w| w[0] < w[1]),
        "ALLOWED must be sorted and unique"
    );
}

#[test]
fn no_new_module_edges() {
    let allowed: BTreeSet<(String, String)> = ALLOWED
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let found = edges();
    let new: Vec<_> = found.difference(&allowed).collect();
    assert!(
        new.is_empty(),
        "new module import edges (add to ALLOWED only with review): {new:#?}"
    );
    let gone: Vec<_> = allowed.difference(&found).collect();
    assert!(
        gone.is_empty(),
        "edges in ALLOWED that no longer exist (remove them): {gone:#?}"
    );
}

/// `services` sits between `ops` and the GitHub layer: only `ops` and `ctx` (the contexts of the
/// commands, which hand a `Reader` out) import it, and it imports
/// no command, no CLI, no policy and no local-machine helper (`spawn`, `toolchain`, `facts`).
#[test]
fn services_sit_between_ops_and_github() {
    let found = edges();
    let importers: Vec<_> = found
        .iter()
        .filter(|(_, to)| to == "services")
        .map(|(from, _)| from.as_str())
        .collect();
    assert_eq!(
        importers,
        ["ctx", "ops"],
        "only ops and its contexts (ctx) import services"
    );
    let below = [
        "base", "board", "comments", "config", "domain", "error", "feed", "github", "items",
        "registry", "rules", "state", "ticket",
    ];
    let bad: Vec<_> = found
        .iter()
        .filter(|(from, to)| from == "services" && !below.contains(&to.as_str()))
        .collect();
    assert!(
        bad.is_empty(),
        "services import something above them: {bad:?}"
    );
}

/// The pure layer imports no module that does I/O: only `base` and the pure `rules` and
/// `authority`.
#[test]
fn domain_imports_only_pure_modules() {
    let bad: Vec<_> = edges()
        .into_iter()
        .filter(|(from, to)| {
            from == "domain" && !["base", "rules", "authority"].contains(&to.as_str())
        })
        .collect();
    assert!(bad.is_empty(), "domain imports an impure module: {bad:#?}");
}

/// The command layer has one direction: `cli` runs `ops` and `wake`, `wake` calls `ops`, and
/// `ops` imports neither `cli` nor `wake`. `ctx`, `headless` and `unread` sit below all three.
#[test]
fn the_command_layer_has_no_cycle() {
    let found = edges();
    let bad: Vec<_> = found
        .iter()
        .filter(|(from, to)| {
            let top = ["cli", "ops", "wake"];
            (from == "ops" && ["cli", "wake"].contains(&to.as_str()))
                || (from == "wake" && to == "cli")
                || (["ctx", "headless", "unread"].contains(&from.as_str())
                    && top.contains(&to.as_str()))
        })
        .collect();
    assert!(bad.is_empty(), "the command layer has a cycle: {bad:#?}");
}

/// No cycle in the import graph of the top-level modules: peel off every module that imports
/// nothing left, until none remains. What is left on a failure is the cycle, with every module
/// that leads into it. The core (`agent_env`, `comments`, `config`, `items`, `facts`, `github`,
/// `capabilities`, `owner_cmd`, `registry`, `events`, `state`, `ticket`) was one cycle of 12
/// modules until the plain data types and helpers moved down.
#[test]
fn the_module_graph_has_no_cycle() {
    let mut left = edges();
    let mut nodes: BTreeSet<String> = left
        .iter()
        .flat_map(|(a, b)| [a.clone(), b.clone()])
        .collect();
    loop {
        let sinks: Vec<String> = nodes
            .iter()
            .filter(|n| !left.iter().any(|(from, _)| from == *n))
            .cloned()
            .collect();
        if sinks.is_empty() {
            break;
        }
        for n in &sinks {
            nodes.remove(n);
        }
        left.retain(|(_, to)| !sinks.contains(to));
    }
    assert!(nodes.is_empty(), "import cycle among: {nodes:?}");
}

/// The order inside the former cycle, lowest first. A module imports only modules that stand
/// before it in this list (or modules outside the list).
#[test]
fn the_core_is_layered() {
    let order = [
        "agent_env",
        "comments",
        "config",
        "items",
        "facts",
        "github",
        "capabilities",
        "owner_cmd",
        "registry",
        "events",
        "state",
        "ticket",
    ];
    let bad: Vec<_> = edges()
        .into_iter()
        .filter(|(from, to)| {
            match (
                order.iter().position(|m| m == from),
                order.iter().position(|m| m == to),
            ) {
                (Some(f), Some(t)) => t >= f,
                _ => false,
            }
        })
        .collect();
    assert!(bad.is_empty(), "the core imports upward: {bad:#?}");
}

/// The production files under `src/ops` and `src/lock`, by module path (`ops::tickets::new`; a
/// `mod.rs` is the module of its directory), with the production part of their text.
fn scoped_files() -> BTreeMap<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    let mut out = BTreeMap::new();
    for file in files {
        if test_only(&file) {
            continue;
        }
        let rel = file.strip_prefix(&root).unwrap().with_extension("");
        let mut parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if parts.last().is_some_and(|l| l == "mod") {
            parts.pop();
        }
        if !matches!(parts.first().map(String::as_str), Some("ops" | "lock")) {
            continue;
        }
        out.insert(
            parts.join("::"),
            production(&fs::read_to_string(&file).unwrap()),
        );
    }
    out
}

/// The paths of a `use` tree: `super::{a, b::{c, self}}` gives `super::a`, `super::b::c`,
/// `super::b`.
fn expand_use_tree(tree: &str) -> Vec<String> {
    let tree = tree.trim();
    let Some(open) = tree.find('{') else {
        return vec![tree.to_string()];
    };
    let prefix = &tree[..open];
    let mut parts = Vec::new();
    let (mut depth, mut cur) = (0, String::new());
    for c in tree[open + 1..].chars() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => break,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    parts.push(cur);
    let mut out = Vec::new();
    for part in parts.iter().filter(|p| !p.trim().is_empty()) {
        for e in expand_use_tree(part) {
            out.push(if e == "self" {
                prefix.trim_end_matches(':').to_string()
            } else {
                format!("{prefix}{e}")
            });
        }
    }
    out
}

/// The module that a `crate::`, `super::` or `self::` path starts in, as segments without the
/// leading `crate`. `current` is the module of the importing file.
fn resolve_path(current: &[&str], path: &str) -> Option<Vec<String>> {
    let mut segs = path.split("::").map(str::trim).peekable();
    let mut out: Vec<String> = match segs.next()? {
        "crate" => Vec::new(),
        "self" => current.iter().map(|s| s.to_string()).collect(),
        "super" => {
            let mut m: Vec<String> = current.iter().map(|s| s.to_string()).collect();
            m.pop();
            while segs.peek() == Some(&"super") {
                segs.next();
                m.pop();
            }
            m
        }
        _ => return None,
    };
    out.extend(segs.map(str::to_string));
    Some(out)
}

/// `from -> to` between the files of `ops` and of `lock` (a path into a file or into an item of
/// a file counts as an edge to that file; `ops::tickets` is the file `ops/tickets/mod.rs`). A
/// parent that declares and re-exports its children with a relative `use` does not count as
/// importing them.
fn file_edges() -> BTreeSet<(String, String)> {
    let files = scoped_files();
    let use_stmt = Regex::new(r"\buse\s+([^;]+);").unwrap();
    let inline = Regex::new(r"\b(?:crate|super|self)(?:::\w+)+").unwrap();
    let rename = Regex::new(r"\s+as\s+\w+\s*$").unwrap();
    let mut found = BTreeSet::new();
    for (module, text) in &files {
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let current: Vec<&str> = module.split("::").collect();
        let mut paths: Vec<String> = use_stmt
            .captures_iter(&code)
            .flat_map(|c| expand_use_tree(&c[1]))
            .collect();
        let rest = use_stmt.replace_all(&code, "");
        paths.extend(inline.find_iter(&rest).map(|m| m.as_str().to_string()));
        for path in paths {
            let path = rename.replace(path.trim(), "");
            let path = path.trim_end_matches("::*");
            let Some(resolved) = resolve_path(&current, path) else {
                continue;
            };
            // The longest prefix that is a file.
            for n in (1..=resolved.len()).rev() {
                let target = resolved[..n].join("::");
                if files.contains_key(&target) {
                    if &target != module {
                        found.insert((module.clone(), target));
                    }
                    break;
                }
            }
        }
    }
    found
}

/// No file of `ops` or `lock` imports, directly or through others, a file that imports it back.
/// The helper that two commands share goes below both (`ctx`, or a module like `assign_checks`);
/// a command never calls a command that calls it. Peels the files without imports and without
/// importers until none is left; what is left on a failure are the edges between the files of
/// the cycles. Seven cycles stood here when the check was written: `start`, `late`, `duties`,
/// `selftest` and `watch` through `load_authority` and `is_coordinator`, and six pairs or
/// triples.
#[test]
fn the_files_of_ops_and_lock_have_no_cycle() {
    let mut left = file_edges();
    assert!(
        left.len() > 50,
        "the file graph looks empty ({} edges): the path scan is broken",
        left.len()
    );
    // Peel the files that import nothing left, and the files that nothing left imports.
    loop {
        let before = left.len();
        let from: BTreeSet<String> = left.iter().map(|(a, _)| a.clone()).collect();
        let to: BTreeSet<String> = left.iter().map(|(_, b)| b.clone()).collect();
        left.retain(|(a, b)| to.contains(a) && from.contains(b));
        if left.len() == before {
            break;
        }
    }
    assert!(
        left.is_empty(),
        "import cycle among files (move the shared function down): {left:#?}"
    );
}

#[test]
fn no_command_reaches_two_levels_up_into_another_command() {
    let bad: Vec<_> = scoped_files()
        .into_iter()
        .filter(|(_, text)| {
            text.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains("super::super::"))
        })
        .map(|(m, _)| m)
        .collect();
    assert!(bad.is_empty(), "`super::super::` in: {bad:?}");
}
