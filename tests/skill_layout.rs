//! Guard test: `skill/` keeps the layout of the Agent Skills standard (agentskills.io).
//!
//! - The front matter of `SKILL.md` has only the portable keys, `name` equals the directory
//!   name, `description` has at most 1024 characters and `compatibility` at most 500.
//! - `SKILL.md` has fewer than 500 lines: every agent wake loads it.
//! - Each file in `skill/references/` is linked from `SKILL.md` (no orphan), sends the agent to
//!   no other reference (one level deep), and starts with a table of contents when it has more
//!   than 100 lines.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use regex::Regex;
use std::path::Path;

/// The top-level keys that the spec allows. Strict loaders reject any other key.
const PORTABLE_KEYS: &[&str] = &[
    "name",
    "description",
    "license",
    "compatibility",
    "metadata",
    "allowed-tools",
];

/// The top-level `key: value` pairs of the front matter, or `None` without front matter.
fn front_matter(text: &str) -> Option<Vec<(String, String)>> {
    let mut lines = text.lines();
    if lines.next()? != "---" {
        return None;
    }
    let mut keys = Vec::new();
    for line in lines {
        if line == "---" {
            return Some(keys);
        }
        if line.starts_with([' ', '\t']) || line.trim().is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':')?;
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(v);
        keys.push((k.trim().to_string(), v.to_string()));
    }
    None
}

/// Every problem of a SKILL.md text for a skill in directory `dir_name`.
fn skill_md_problems(text: &str, dir_name: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(keys) = front_matter(text) else {
        return vec!["no front matter".to_string()];
    };
    for (k, _) in &keys {
        if !PORTABLE_KEYS.contains(&k.as_str()) {
            problems.push(format!(
                "key `{k}` is not portable (put it under `metadata:`)"
            ));
        }
    }
    let get = |name: &str| {
        keys.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if get("name") != Some(dir_name) {
        problems.push(format!("`name` must be `{dir_name}`"));
    }
    match get("description") {
        None | Some("") => problems.push("no `description`".to_string()),
        Some(d) if d.chars().count() > 1024 => problems.push(format!(
            "`description` has {} characters (max 1024)",
            d.chars().count()
        )),
        _ => {}
    }
    if let Some(c) = get("compatibility")
        && c.chars().count() > 500
    {
        problems.push(format!(
            "`compatibility` has {} characters (max 500)",
            c.chars().count()
        ));
    }
    let lines = text.lines().count();
    if lines >= 500 {
        problems.push(format!("SKILL.md has {lines} lines (max 499)"));
    }
    problems
}

/// Every problem of one reference file `name` with `text`.
fn reference_problems(name: &str, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    // One level deep: no link to a sibling Markdown file, and no "references/<file>" text.
    let link = Regex::new(r"\]\(([^)\s#]+\.md)(#[^)]*)?\)").unwrap();
    for c in link.captures_iter(text) {
        let target = &c[1];
        if !target.contains('/') || target.contains("references/") {
            problems.push(format!("{name}: links to another reference ({target})"));
        }
    }
    if text.contains("references/") {
        problems.push(format!("{name}: names another reference (references/...)"));
    }
    let lines = text.lines().count();
    let head: Vec<&str> = text.lines().take(12).collect();
    if lines > 100 && !head.iter().any(|l| l.trim_start().starts_with("Contents")) {
        problems.push(format!(
            "{name}: {lines} lines and no table of contents (a `Contents:` list at the top)"
        ));
    }
    problems
}

/// Reference files that SKILL.md does not link (`references/<name>`).
fn orphans<'a>(skill_md: &str, names: &[&'a str]) -> Vec<&'a str> {
    names
        .iter()
        .copied()
        .filter(|n| !skill_md.contains(&format!("](references/{n})")))
        .collect()
}

#[test]
fn the_skill_keeps_the_standard_layout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("skill");
    let skill = std::fs::read_to_string(root.join("SKILL.md")).expect("read skill/SKILL.md");
    let problems = skill_md_problems(&skill, "crewboard");
    assert!(
        problems.is_empty(),
        "skill/SKILL.md:\n{}",
        problems.join("\n")
    );

    let mut names = Vec::new();
    for entry in std::fs::read_dir(root.join("references")).expect("read skill/references") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            names.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }
    assert!(names.len() >= 3, "too few reference files: {names:?}");
    let mut problems = Vec::new();
    for n in &names {
        let text = std::fs::read_to_string(root.join("references").join(n)).unwrap();
        problems.extend(reference_problems(n, &text));
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    for o in orphans(&skill, &refs) {
        problems.push(format!("{o}: SKILL.md does not link it"));
    }
    assert!(
        problems.is_empty(),
        "skill/references:\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_skill_md_check_fires_and_passes() {
    let good = "---\nname: crewboard\ndescription: \"Runs an agent.\"\ncompatibility: bash\nmetadata:\n  version: \"1.0.0\"\n---\n# x\n";
    assert!(skill_md_problems(good, "crewboard").is_empty());

    let top_version = good.replace("metadata:\n  version", "version: 1\nmetadata:\n  version");
    assert_eq!(
        skill_md_problems(&top_version, "crewboard"),
        vec!["key `version` is not portable (put it under `metadata:`)".to_string()]
    );
    assert_eq!(
        skill_md_problems(good, "other"),
        vec!["`name` must be `other`".to_string()]
    );
    let long = good.replace("Runs an agent.", &"x".repeat(1025));
    assert_eq!(
        skill_md_problems(&long, "crewboard"),
        vec!["`description` has 1025 characters (max 1024)".to_string()]
    );
    let no_desc = good.replace("description: \"Runs an agent.\"\n", "");
    assert_eq!(
        skill_md_problems(&no_desc, "crewboard"),
        vec!["no `description`".to_string()]
    );
    let big = format!("{good}{}", "line\n".repeat(500));
    assert_eq!(skill_md_problems(&big, "crewboard").len(), 1);
    assert_eq!(
        skill_md_problems("# no front matter\n", "crewboard"),
        vec!["no front matter".to_string()]
    );
}

#[test]
fn the_reference_check_fires_and_passes() {
    // Pass: links to the loop guides and the skill's own scripts, short file, no TOC needed.
    let ok = "# Dev\n\nSee [guide](../loops/codex.md) and `loops/gate-loop.sh`.\n";
    assert!(reference_problems("dev.md", ok).is_empty());
    // A long file with a table of contents passes.
    let toc = format!(
        "# Loops\n\nContents:\n\n- [A](#a)\n{}",
        "text\n".repeat(120)
    );
    assert!(reference_problems("loops.md", &toc).is_empty());

    // Refusals: a sibling link, a link into references/, the text "references/", no TOC.
    assert_eq!(
        reference_problems("a.md", "Read [b](b.md#x).\n"),
        vec!["a.md: links to another reference (b.md)".to_string()]
    );
    assert_eq!(
        reference_problems("a.md", "Read [b](../references/b.md).\n").len(),
        2
    );
    assert_eq!(
        reference_problems("a.md", "Read references/b.md first.\n"),
        vec!["a.md: names another reference (references/...)".to_string()]
    );
    let long = format!("# Long\n{}", "text\n".repeat(120));
    assert_eq!(reference_problems("long.md", &long).len(), 1);
}

#[test]
fn the_orphan_check_fires_and_passes() {
    let skill = "Read [references/dev.md](references/dev.md).\n";
    assert!(orphans(skill, &["dev.md"]).is_empty());
    assert_eq!(orphans(skill, &["dev.md", "extra.md"]), vec!["extra.md"]);
}
