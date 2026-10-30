//! Behavioural check of the agent skill: a model that meets a situation must find its action in
//! the same rule of the skill text. No model call, no network.
//!
//! The cases are in `tests/fixtures/skill-eval/cases.toml`. For each case the test reads
//! `skill/SKILL.md` and `skill/references/<role>.md`, cuts them into rules and asserts that one
//! rule holds both the situation and the action.
//!
//! A rule is one list item (a line that starts with `- ` or `1. `, with its wrapped lines), or
//! one paragraph (lines up to the next blank line or heading), with the lines joined and the
//! white space collapsed. Backticks stay: they are part of the contract text.
//!
//! The test also proves that a case can fail: for three cases it removes the action (and, for
//! one, the situation) from the in-memory text and expects the check to report it. Last, it
//! counts the refusal rules that the CLI can print and that the skill names.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

struct Case {
    name: String,
    role: String,
    situation: String,
    action: String,
    rule: Option<String>,
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases() -> Vec<Case> {
    let text = fs::read_to_string(root().join("tests/fixtures/skill-eval/cases.toml")).unwrap();
    let table: toml::Table = text.parse().unwrap();
    let list = table["case"].as_array().unwrap();
    let get = |c: &toml::Table, k: &str| c.get(k).and_then(|v| v.as_str()).map(String::from);
    list.iter()
        .map(|c| {
            let c = c.as_table().unwrap();
            let name = get(c, "name").unwrap();
            let field = |k: &str| get(c, k).unwrap_or_else(|| panic!("case {name}: no `{k}`"));
            Case {
                role: field("role"),
                situation: field("situation"),
                action: field("action"),
                rule: get(c, "rule"),
                name,
            }
        })
        .collect()
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cut Markdown into rules (see the module comment).
fn rules_of(text: &str) -> Vec<String> {
    let item = Regex::new(r"^\s*(-|\d+\.) ").unwrap();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.trim().is_empty() {
            out.push(squash(cur));
        }
        cur.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            flush(&mut cur, &mut out);
        } else {
            if item.is_match(line) {
                flush(&mut cur, &mut out);
            }
            cur.push_str(line);
            cur.push(' ');
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn skill_text(role: &str) -> String {
    let dir = root().join("skill");
    let main = fs::read_to_string(dir.join("SKILL.md")).unwrap();
    let file = dir.join("references").join(format!("{role}.md"));
    let role_text = fs::read_to_string(&file).unwrap_or_else(|_| panic!("no role file {role}"));
    format!("{main}\n\n{role_text}")
}

/// `Ok` when one rule holds the situation and the action.
fn check(case: &Case, text: &str) -> Result<(), String> {
    let situation = squash(&case.situation);
    let action = squash(&case.action);
    let rules = rules_of(text);
    let with_situation: Vec<&String> = rules.iter().filter(|r| r.contains(&situation)).collect();
    if with_situation.is_empty() {
        return Err(format!("no rule holds the situation `{situation}`"));
    }
    if with_situation.iter().any(|r| r.contains(&action)) {
        Ok(())
    } else {
        Err(format!(
            "the rule of `{situation}` does not say `{action}`; it says: {}",
            with_situation[0]
        ))
    }
}

#[test]
fn every_situation_names_its_action() {
    let cases = cases();
    assert!(cases.len() >= 20, "keep at least 20 cases");
    let mut failures = Vec::new();
    for c in &cases {
        assert!(
            c.role == "dev" || c.role == "coordinator",
            "case {}: role",
            c.name
        );
        if let Err(e) = check(c, &skill_text(&c.role)) {
            failures.push(format!("{}: {e}", c.name));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_case_fails_when_the_skill_text_loses_the_action_or_the_situation() {
    let cases = cases();
    for name in [
        "dev: green CI",
        "coordinator: verify outside Review",
        "coordinator: status mismatch",
    ] {
        let c = cases.iter().find(|c| c.name == name).unwrap();
        let text = skill_text(&c.role);
        assert!(check(c, &text).is_ok(), "{name} passes on the real text");
        let broken = text.replacen(&c.action, "", 1);
        assert!(
            broken != text,
            "{name}: the action is not in one line of the file"
        );
        let err = check(c, &broken).expect_err(name);
        assert!(err.contains("does not say"), "{name}: {err}");
    }
    // The action in another rule does not count.
    let c = cases.iter().find(|c| c.name == "dev: green CI").unwrap();
    let moved = skill_text("dev").replacen(&c.action, "", 1) + "\n\n- Elsewhere: " + &c.action;
    assert!(
        check(c, &moved).is_err(),
        "an action in another rule must not count"
    );
    // The situation gone.
    let c = cases
        .iter()
        .find(|c| c.name == "dev: stuck on a tool")
        .unwrap();
    let text = skill_text("dev").replacen(&c.situation, "", 1);
    let err = check(c, &text).expect_err("situation");
    assert!(err.contains("no rule holds the situation"), "{err}");
}

#[test]
fn rules_split_on_list_items_and_paragraphs() {
    let md = "# H\n\nA wrapped\nparagraph.\n\n- one\n  wrapped\n- two\n1. three\n";
    assert_eq!(
        rules_of(md),
        ["A wrapped paragraph.", "- one wrapped", "- two", "1. three"]
    );
}

/// The refusal rules that the CLI can print: the catalog of refusal ids (`src/base/ids.rs`).
fn cli_rules() -> BTreeSet<String> {
    crewboard::rules::ids::ALL
        .iter()
        .map(|r| r.as_str().to_string())
        .collect()
}

/// `name` stands in `text` as a whole word (`-` and `a-z0-9` extend a name).
fn names(text: &str, name: &str) -> bool {
    let ok = |c: Option<char>| !c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-');
    text.match_indices(name).any(|(i, _)| {
        ok(text[..i].chars().next_back()) && ok(text[i + name.len()..].chars().next())
    })
}

#[test]
fn the_rules_of_the_cases_exist_and_the_skill_coverage_is_counted() {
    let rules = cli_rules();
    assert!(
        rules.len() > 50,
        "the pattern finds the rules of the CLI ({} found)",
        rules.len()
    );
    for c in cases() {
        if let Some(r) = &c.rule {
            assert!(rules.contains(r), "case {}: no CLI rule `{r}`", c.name);
        }
    }
    let dir = root().join("skill");
    let mut skill = fs::read_to_string(dir.join("SKILL.md")).unwrap();
    for e in fs::read_dir(dir.join("references")).unwrap() {
        skill.push_str(&fs::read_to_string(e.unwrap().path()).unwrap());
    }
    let mut with_docs = skill.clone();
    for e in fs::read_dir(root().join("docs/reference")).unwrap() {
        with_docs.push_str(&fs::read_to_string(e.unwrap().path()).unwrap());
    }
    let named: Vec<&String> = rules.iter().filter(|r| names(&skill, r)).collect();
    let documented = rules.iter().filter(|r| names(&with_docs, r)).count();
    println!(
        "refusal rules named in the skill: {} of {}; in skill or docs/reference: {} of {}",
        named.len(),
        rules.len(),
        documented,
        rules.len()
    );
    assert!(
        !named.is_empty(),
        "the skill names at least one refusal rule"
    );
    // The coverage is a report, not a gate: most rules print their own fix.
    let unnamed: Vec<&String> = rules.iter().filter(|r| !names(&skill, r)).collect();
    println!("not named in the skill: {unnamed:?}");
    assert!(names("a `x-y` b", "x-y") && !names("a x-yz b", "x-y") && !names("a ax-y b", "x-y"));
}
