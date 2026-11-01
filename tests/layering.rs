//! Layering freeze: builds the import graph of the top-level modules from `crate::<module>`
//! references in production code (the first inline `#[cfg(test)] mod x {` to the end of a file,
//! every `*_tests.rs` file and `testkit.rs`, the builders of the unit tests, are skipped;
//! `ops/tickets/tick.rs` counts as `ops`). An edge that is not in `ALLOWED` fails the test. `base` may be imported by every module and imports nothing itself. Remove an edge
//! from the list when a refactor removes it, never add one silently. `ignore` (the named reasons
//! to drop a result) lives in the crate `host-lock`; the binary reaches it as `base::ignore`.
//!
//! `domain` is the pure layer: it imports `base`, and `rules` and `authority` (pure modules that
//! have not moved under `domain` yet), never `github`, `services`, `ops`, `config` or `clock`.
//! `services` reads from `github` and hands the answers to `domain`.
//!
//! The scan reads every source tree of the workspace (`SOURCES`): `src/` of the binary and the
//! crates under `crates/`, each file under the module name that it has for the binary
//! (`crates/crewboard-protocol/src/names.rs` is `protocol::names`). `protocol` is a crate
//! of its own; Cargo stops it from importing the rest, and
//! `the_lower_crates_depend_on_no_workspace_crate` keeps its manifest free of workspace crates.
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
    ("audit", "base"),
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
    ("events", "base"),
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
    ("fsutil", "clock"),
    ("fsutil", "ignore"),
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
    ("lock", "clock"),
    ("lock", "fsutil"),
    ("lock", "ignore"),
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
    ("registry", "comments"),
    ("registry", "config"),
    ("registry", "items"),
    ("registry", "owner_cmd"),
    ("registry", "protocol"),
    ("registry", "rules"),
    ("rules", "base"),
    ("rules", "protocol"),
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
    ("spawn", "clock"),
    ("spawn", "ignore"),
    ("state", "base"),
    ("state", "clock"),
    ("state", "comments"),
    ("state", "error"),
    ("state", "events"),
    ("state", "fsutil"),
    ("ticket", "base"),
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

/// A source tree of the workspace: the directory, the module that the crate root stands for in
/// the binary (`None` for `src/` of the binary itself), and the top-level modules of the crate
/// that keep their own name.
struct Tree {
    dir: &'static str,
    root: Option<&'static str>,
    own: &'static [&'static str],
}

const SOURCES: &[Tree] = &[
    Tree {
        dir: "src",
        root: None,
        own: &[],
    },
    Tree {
        dir: "crates/crewboard-protocol/src",
        root: Some("protocol"),
        own: &[],
    },
    Tree {
        dir: "crates/host-lock/src",
        root: Some("lock"),
        own: &["clock", "fsutil", "ignore", "spawn"],
    },
];

impl Tree {
    /// The module path in the binary of the crate-relative path `segs` (what follows `crate::`).
    fn global(&self, segs: &[String]) -> Vec<String> {
        match self.root {
            Some(root) if !segs.first().is_some_and(|f| self.own.contains(&f.as_str())) => {
                std::iter::once(root.to_string())
                    .chain(segs.iter().cloned())
                    .collect()
            }
            _ => segs.to_vec(),
        }
    }
}

/// One Rust file of the workspace: its tree, its module path in the binary (`ops::tickets::new`,
/// `protocol::names`; a `mod.rs` is the module of its directory, the `lib.rs` of a crate the
/// module of the crate) and its text.
struct SrcFile {
    tree: &'static Tree,
    path: std::path::PathBuf,
    module: Vec<String>,
}

/// Every Rust file of every tree in `SOURCES`. Each tree must hold files: a scan of a moved or
/// misspelt directory would otherwise find nothing and pass.
fn source_files() -> Vec<SrcFile> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for tree in SOURCES {
        let root = base.join(tree.dir);
        let mut files = Vec::new();
        rs_files(&root, &mut files);
        assert!(
            files.len() >= 10,
            "{} holds {} Rust files: the scan is broken",
            tree.dir,
            files.len()
        );
        for path in files {
            let rel = path.strip_prefix(&root).unwrap().with_extension("");
            let mut segs: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if segs.last().is_some_and(|l| l == "mod") || (tree.root.is_some() && segs == ["lib"]) {
                segs.pop();
            }
            let module = tree.global(&segs);
            out.push(SrcFile { tree, path, module });
        }
    }
    out
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
    let mut found = BTreeSet::new();
    for file in source_files() {
        if test_only(&file.path) {
            continue;
        }
        let from = file.module[0].clone();
        if file.tree.root.is_none() && (from == "main" || from == "lib") {
            continue;
        }
        let code = production(&fs::read_to_string(&file.path).unwrap());
        for to in crate_targets(&code) {
            let to = file.tree.global(&[to]).swap_remove(0);
            if to != from {
                found.insert((from.clone(), to));
            }
        }
    }
    found
}

#[test]
fn a_crate_path_inside_a_crate_counts_for_the_module_of_the_crate() {
    let tree = Tree {
        dir: "crates/host-lock/src",
        root: Some("lock"),
        own: &["clock"],
    };
    let planted = "use crate::{procs::Alive, clock::Clock};\nfn f() { crate::github::x(); }\n";
    let found: BTreeSet<String> = crate_targets(planted)
        .into_iter()
        .map(|t| tree.global(&[t]).swap_remove(0))
        .collect();
    // `crate::github` inside the lock crate is a module of that crate (`lock::github`); only the
    // own modules (`clock`) keep their name. What a crate may import from other crates is
    // decided by its manifest (`the_lower_crates_depend_on_no_workspace_crate`).
    assert_eq!(found, set(&["clock", "lock"]));
    let deep: Vec<String> = ["procs", "Alive"].map(String::from).into();
    assert_eq!(tree.global(&deep).join("::"), "lock::procs::Alive");
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

/// The paths in the code of the binary (`src/`) that name a lower crate (`host_lock`,
/// `crewboard_protocol`) directly. The edge check reads `crate::<module>` only, so such a path
/// would pass by the layer rules. The re-exports in `src/lib.rs` and `src/base.rs` are the only
/// places that may name these crates; everything else goes through `crate::lock`,
/// `crate::clock`, `crate::protocol` and so on. Comment lines are skipped. A field or variable
/// named `host_lock` is no path (no `::` follows it).
fn lower_crate_paths(file: &str, text: &str) -> Vec<String> {
    let path = Regex::new(
        r"(?:^|[^.\w])(?:host_lock|crewboard_protocol)\s*::|\buse\s+(?:::)?(?:host_lock|crewboard_protocol)\b",
    )
    .unwrap();
    let reexport = ["src/lib.rs", "src/base.rs"].contains(&file);
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .filter(|(_, l)| path.is_match(l))
        .filter(|(_, l)| {
            let l = l.trim_start();
            !(reexport && (l.starts_with("pub use ") || l.starts_with("pub(crate) use ")))
        })
        .map(|(i, l)| format!("{file}:{}: {}", i + 1, l.trim()))
        .collect()
}

#[test]
fn the_binary_names_the_lower_crates_only_through_the_re_exports() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut scanned = 0;
    let mut bad = Vec::new();
    for file in source_files().into_iter().filter(|f| f.tree.root.is_none()) {
        let rel = file
            .path
            .strip_prefix(base)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        scanned += 1;
        bad.extend(lower_crate_paths(
            &rel,
            &fs::read_to_string(&file.path).unwrap(),
        ));
    }
    assert!(scanned > 100, "too few files of src/ scanned: {scanned}");
    assert!(
        bad.is_empty(),
        "name the module through the re-export (crate::clock, crate::lock, crate::protocol, ...):\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_crate_name_check_sees_a_planted_path() {
    let planted = "use crewboard_protocol::names::Kind;\n\
                   fn f() -> i64 { host_lock::clock::now_epoch() }\n\
                   use ::host_lock as hl;\n\
                   // host_lock::clock in a comment\n\
                   pub host_lock: Vec<PathBuf>,\n\
                   let n = a.host_lock.iter().count();\n";
    let hits = lower_crate_paths("src/domain/planted.rs", planted);
    assert_eq!(hits.len(), 3, "{hits:?}");
    // The re-exports of lib.rs and base.rs pass; the same line elsewhere does not.
    let reexport = "pub use host_lock::clock;\npub(crate) use host_lock as lock;\n";
    assert!(lower_crate_paths("src/lib.rs", reexport).is_empty());
    assert_eq!(lower_crate_paths("src/ops/x.rs", reexport).len(), 2);
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

/// The crates below the binary: (manifest, why it must not depend on a workspace crate).
const LOWER_CRATES: &[(&str, &str)] = &[
    (
        "crates/crewboard-protocol/Cargo.toml",
        "protocol is the wire: it imports nothing from the rest of crewboard",
    ),
    (
        "crates/host-lock/Cargo.toml",
        "the host lock and its seams sit below everything else",
    ),
];

/// The names of the workspace crates (the root package and every member) that a manifest lists
/// in a dependency table: `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` and the
/// same tables under `[target.'cfg(..)']`.
fn workspace_deps(manifest: &str, workspace: &BTreeSet<String>) -> Vec<String> {
    let m: toml::Table = manifest.parse().unwrap();
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables: Vec<&toml::Table> = kinds
        .iter()
        .filter_map(|k| m.get(*k).and_then(|t| t.as_table()))
        .collect();
    if let Some(targets) = m.get("target").and_then(|t| t.as_table()) {
        for t in targets.values().filter_map(|t| t.as_table()) {
            tables.extend(
                kinds
                    .iter()
                    .filter_map(|k| t.get(*k).and_then(|d| d.as_table())),
            );
        }
    }
    tables
        .iter()
        .flat_map(|t| t.keys())
        .filter(|k| workspace.contains(k.as_str()))
        .cloned()
        .collect()
}

/// The package names of the workspace: the root package and each member.
fn workspace_crates() -> BTreeSet<String> {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root: toml::Table = fs::read_to_string(base.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    root["workspace"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let path = base.join(m.as_str().unwrap()).join("Cargo.toml");
            let t: toml::Table = fs::read_to_string(path).unwrap().parse().unwrap();
            t["package"]["name"].as_str().unwrap().to_string()
        })
        .collect()
}

/// Cargo stops a crate from importing a crate that it does not depend on, so the crates below
/// the binary stay below as long as their manifests name no workspace crate.
#[test]
fn the_lower_crates_depend_on_no_workspace_crate() {
    let workspace = workspace_crates();
    assert!(
        workspace.contains("crewboard") && workspace.len() > LOWER_CRATES.len(),
        "the workspace members are not found: {workspace:?}"
    );
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (manifest, why) in LOWER_CRATES {
        let text = fs::read_to_string(base.join(manifest)).unwrap();
        let bad = workspace_deps(&text, &workspace);
        assert!(bad.is_empty(), "{manifest} depends on {bad:?}: {why}");
    }
}

#[test]
fn the_manifest_check_sees_a_planted_workspace_dependency() {
    let workspace = set(&["crewboard", "crewboard-protocol", "host-lock"]);
    let planted = "[package]\nname = \"x\"\n\n[dependencies]\nsha2 = \"1\"\n\n\
                   [target.'cfg(unix)'.dev-dependencies]\ncrewboard = { path = \"../..\" }\n";
    assert_eq!(workspace_deps(planted, &workspace), ["crewboard"]);
    assert!(workspace_deps("[dependencies]\nsha2 = \"1\"\n", &workspace).is_empty());
}

/// The production files of `ops` and `lock`, by module path (`ops::tickets::new`, `lock::procs`;
/// a `mod.rs` is the module of its directory), with their tree and the production part of their
/// text.
fn scoped_files() -> BTreeMap<String, (&'static Tree, String)> {
    let mut out = BTreeMap::new();
    for file in source_files() {
        if test_only(&file.path)
            || !matches!(
                file.module.first().map(String::as_str),
                Some("ops" | "lock")
            )
        {
            continue;
        }
        out.insert(
            file.module.join("::"),
            (
                file.tree,
                production(&fs::read_to_string(&file.path).unwrap()),
            ),
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
    for (module, (tree, text)) in &files {
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
            let Some(mut resolved) = resolve_path(&current, path) else {
                continue;
            };
            if path.trim_start().starts_with("crate::") {
                resolved = tree.global(&resolved);
            }
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
        .filter(|(_, (_, text))| {
            text.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .any(|l| l.contains("super::super::"))
        })
        .map(|(m, _)| m)
        .collect();
    assert!(bad.is_empty(), "`super::super::` in: {bad:?}");
}
