//! Guard test: the tickets spec and the status table name the same refusal codes.
//!
//! `src/rules/status/table.txt` is the golden matrix of every status move and its verdict. The
//! spec `docs/design/spec/tickets.md` explains each refusal code of that table once and points to
//! the file for the matrix. Two checks keep them in step:
//!
//! - every refusal code that occurs in `table.txt` appears in the spec, so a new code is
//!   explained;
//! - every `status-*` code that the spec names in backticks exists in `table.txt` or as a string
//!   literal under `src/`, so the spec names no code that the program does not have.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panic"
)]

use regex::Regex;
use std::collections::BTreeSet;
use std::path::Path;

const TABLE: &str = "src/rules/status/table.txt";
const SPEC: &str = "docs/design/spec/tickets.md";

/// The refusal codes of the table: every verdict that is not `ok`. A line is
/// `<profile>|<from>|<verdict> <verdict> ...`.
fn table_codes(table: &str) -> BTreeSet<String> {
    table
        .lines()
        .filter_map(|l| l.splitn(3, '|').nth(2))
        .flat_map(str::split_whitespace)
        .filter(|v| *v != "ok")
        .map(str::to_string)
        .collect()
}

/// The `status-*` codes that a Markdown text names in backticks.
fn spec_status_codes(spec: &str) -> BTreeSet<String> {
    let re = Regex::new(r"`(status-[a-z0-9]+(?:-[a-z0-9]+)*)`").unwrap();
    re.captures_iter(spec).map(|c| c[1].to_string()).collect()
}

/// The string literals `"status-..."` in the Rust files under `dir`.
fn source_status_codes(dir: &Path, re: &Regex, out: &mut BTreeSet<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            source_status_codes(&path, re, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            out.extend(re.captures_iter(&text).map(|c| c[1].to_string()));
        }
    }
}

/// The problems of a spec text against the table codes and the codes of the source.
fn problems(spec: &str, table: &BTreeSet<String>, source: &BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = table
        .iter()
        .filter(|c| !spec.contains(&format!("`{c}`")))
        .map(|c| format!("{c} is in {TABLE} but not explained in {SPEC}"))
        .collect();
    out.extend(
        spec_status_codes(spec)
            .into_iter()
            .filter(|c| !table.contains(c) && !source.contains(c))
            .map(|c| format!("{c} is named in {SPEC} but is neither in {TABLE} nor in src/")),
    );
    out
}

#[test]
fn the_tickets_spec_names_every_code_of_the_status_table_and_no_unknown_one() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let table = table_codes(&std::fs::read_to_string(root.join(TABLE)).unwrap());
    let spec = std::fs::read_to_string(root.join(SPEC)).unwrap();
    let mut source = BTreeSet::new();
    let literal = Regex::new(r#""(status-[a-z0-9]+(?:-[a-z0-9]+)*)""#).unwrap();
    source_status_codes(&root.join("src"), &literal, &mut source);
    // The check must be able to fire: the table has codes and the spec names some.
    assert!(table.len() >= 10, "too few codes in {TABLE}: {table:?}");
    assert!(table.contains("status-wrong-writer"), "{table:?}");
    assert!(
        !spec_status_codes(&spec).is_empty(),
        "no status code in {SPEC}"
    );
    let found = problems(&spec, &table, &source);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn the_check_flags_a_missing_and_an_unknown_code() {
    let table: BTreeSet<String> = ["status-no-change", "status-wrong-writer"]
        .map(str::to_string)
        .into();
    let source: BTreeSet<String> = ["status-from-code".to_string()].into();
    let good = "`status-no-change` and `status-wrong-writer`; `status-from-code`.";
    assert!(problems(good, &table, &source).is_empty());
    let missing = "`status-no-change` only.";
    let p = problems(missing, &table, &source);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("status-wrong-writer is in"), "{p:?}");
    let unknown = "`status-no-change`, `status-wrong-writer` and `status-made-up`.";
    let p = problems(unknown, &table, &source);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("status-made-up is named"), "{p:?}");
    assert_eq!(
        table_codes("p|new|ok status-a ok\nq|Ready|status-b status-a"),
        ["status-a", "status-b"].map(str::to_string).into()
    );
}
