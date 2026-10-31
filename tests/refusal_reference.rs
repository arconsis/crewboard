//! The refusal ids: one catalog in the code (`src/base/ids.rs`), one page in the docs
//! (`docs/reference/refusals.md`), and no other id in the docs or the skill.
//!
//! - The page is generated from the catalog, so every id of the catalog is documented. The test
//!   fails when the page is out of date. To regenerate it:
//!   `CREWBOARD_UPDATE_DOCS=1 cargo test --test refusal_reference`.
//! - Every refusal id that the docs or the skill name is in the catalog. A refusal id is a
//!   kebab-case name in backticks in a paragraph, list item or table cell that says "refus" or
//!   "rule=". A few such names are not refusal ids (an audit finding, a label, a trust problem):
//!   [`NOT_REFUSALS`] lists them, each with what it is.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panic"
)]

use crewboard::rules::ids::{ALL, RuleId};
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const PAGE: &str = "docs/reference/refusals.md";

/// Names in backticks next to the word "refused" that are not refusal ids.
const NOT_REFUSALS: &[(&str, &str)] = &[
    ("aarch64-apple-darwin", "a release target"),
    ("agent-paused-ready", "an event of the coordinator"),
    ("adopts-pr", "a line of the take answer"),
    ("authority-contradicting-keys", "a `doctor` finding"),
    ("blocked-too-long", "an event of the coordinator"),
    (
        "branches-ignore",
        "a workflow trigger key of GitHub Actions",
    ),
    ("claude-code", "a runtime name"),
    ("crew-bot", "an example account name"),
    ("edited-message", "an event reason"),
    ("field-mismatch", "an `audit` finding"),
    (
        "late-and-stale-both-set",
        "a config error of the board config",
    ),
    ("lock-run-not-granted", "a `lock run` result"),
    ("needs-owner", "a label"),
    ("new-repo", "an example repo name"),
    ("new-scope", "an example token scope"),
    ("owner-merge", "an event"),
    ("paths-ignore", "a workflow trigger key of GitHub Actions"),
    ("replayed-message", "an event reason"),
    ("review-not-verified", "an event of the coordinator"),
    ("ssh-key", "a key type"),
    ("ssh-keygen", "a program"),
    ("tags-ignore", "a workflow trigger key of GitHub Actions"),
    ("unknown-sender", "an event reason"),
    ("you-agents", "an example account name"),
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The page, generated from the catalog: one section per area, one table row per id.
fn render(all: &[RuleId]) -> String {
    let mut by_area: Vec<(&str, Vec<RuleId>)> = Vec::new();
    for r in all {
        match by_area.iter_mut().find(|(a, _)| *a == r.area()) {
            Some((_, v)) => v.push(*r),
            None => by_area.push((r.area(), vec![*r])),
        }
    }
    let mut out = String::from(
        "# Refusal ids\n\n\
         This page is generated from the catalog of refusal ids in `src/base/ids.rs`. Do not edit it by hand. To regenerate it, run:\n\n\
         ```sh\nCREWBOARD_UPDATE_DOCS=1 cargo test --test refusal_reference\n```\n\n\
         A refused command exits 2 and prints `refused [rule=<id>]: <message>`. The message names the fix. \
         The id never changes, so a script or an agent can match on it. \
         The [exit codes reference](exit-codes.md) lists the exit codes.\n",
    );
    for (area, ids) in by_area {
        out.push_str(&format!("\n## {area}\n\n| Id | Meaning |\n|---|---|\n"));
        for r in ids {
            out.push_str(&format!("| `{}` | {} |\n", r.as_str(), r.meaning()));
        }
    }
    out
}

#[test]
fn the_refusal_page_is_generated_from_the_catalog() {
    let path = root().join(PAGE);
    let want = render(ALL);
    if std::env::var_os("CREWBOARD_UPDATE_DOCS").is_some() {
        fs::write(&path, &want).unwrap();
        return;
    }
    // a Windows checkout may turn LF into CRLF; the page is the same text either way
    let have = fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        have == want,
        "{PAGE} is out of date; run `CREWBOARD_UPDATE_DOCS=1 cargo test --test refusal_reference`"
    );
    // every id of the catalog is on the page
    for r in ALL {
        assert!(have.contains(&format!("| `{}` |", r.as_str())), "{r}");
    }
}

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            markdown_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

/// The kebab-case names in backticks that a text names in a refusal context: a paragraph, list
/// item or table cell that says "refus" or "rule=".
fn named_refusals(text: &str) -> BTreeSet<String> {
    let unit = Regex::new(r"\n\s*\n|\n(?:\s*[-*] |\s*\d+\. )|\|").unwrap();
    let name = Regex::new(r"`([a-z0-9]+(?:-[a-z0-9]+)+)`").unwrap();
    let context = Regex::new(r"(?i)refus|rule=").unwrap();
    unit.split(text)
        .filter(|u| context.is_match(u))
        .flat_map(|u| name.captures_iter(u).map(|c| c[1].to_string()))
        .collect()
}

/// The names that a refusal context of the docs or the skill names and that are neither in the
/// catalog nor in [`NOT_REFUSALS`], with the files that name them.
fn unknown_names(
    texts: &[(String, String)],
    catalog: &BTreeSet<&str>,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, text) in texts {
        for n in named_refusals(text) {
            if !catalog.contains(n.as_str()) && !NOT_REFUSALS.iter().any(|(x, _)| *x == n) {
                out.entry(n).or_default().push(file.clone());
            }
        }
    }
    out
}

#[test]
fn every_refusal_id_that_the_docs_or_the_skill_name_is_in_the_catalog() {
    let mut files = Vec::new();
    markdown_files(&root().join("docs"), &mut files);
    markdown_files(&root().join("skill"), &mut files);
    files.push(root().join("README.md"));
    let texts: Vec<(String, String)> = files
        .iter()
        .filter(|f| !f.ends_with(PAGE))
        .map(|f| {
            (
                f.strip_prefix(root()).unwrap().display().to_string(),
                fs::read_to_string(f).unwrap(),
            )
        })
        .collect();
    let catalog: BTreeSet<&str> = ALL.iter().map(|r| r.as_str()).collect();
    // the check must be able to fire: the docs name many ids of the catalog
    let named: BTreeSet<String> = texts.iter().flat_map(|(_, t)| named_refusals(t)).collect();
    let known = named
        .iter()
        .filter(|n| catalog.contains(n.as_str()))
        .count();
    assert!(known > 50, "only {known} catalog ids found in the docs");
    let unknown = unknown_names(&texts, &catalog);
    assert!(
        unknown.is_empty(),
        "named as refusal ids in the docs or the skill, but not in src/base/ids.rs: {unknown:#?}"
    );
    // an entry of NOT_REFUSALS that is a catalog id is a mistake
    for (n, what) in NOT_REFUSALS {
        assert!(!catalog.contains(n), "{n} is in the catalog, not {what}");
    }
}

#[test]
fn the_check_finds_an_unknown_id_and_ignores_text_without_a_refusal() {
    let catalog: BTreeSet<&str> = ["status-no-change"].into();
    let texts = vec![(
        "a.md".to_string(),
        "Refused with `status-no-change` or `status-made-up`.\n\nThe label `crew-made-up` is \
         a plain name.\n\n| `move` | refused: `move-made-up` |\n"
            .to_string(),
    )];
    let unknown = unknown_names(&texts, &catalog);
    assert_eq!(
        unknown.keys().map(String::as_str).collect::<Vec<_>>(),
        ["move-made-up", "status-made-up"]
    );
}
