//! `AUTHORITY.md` front matter (spec/board#authority). Pure: the caller reads the file from GitHub.
//!
//! The front matter is a small YAML subset: `key: value` lines with a bool, a string, a
//! flow list `[a, b]` or a block list (`- a`), and `#` comments. Anything else is refused,
//! so a typo cannot silently widen what an agent may do (an unknown key is only noted).
//!
//! The file has two kinds of keys (spec/board#authority). ENFORCED keys are read by crewboard,
//! and the command that a key limits refuses what the key forbids (level: loud,
//! [`ENFORCED_KEYS`]). ADVICE keys are not read by crewboard; the models read them as the
//! owner's rules ([`ADVICE_KEYS`]). What the enforced keys do:
//!
//! - `coordinator_may_create_tickets`: `assign` does not add an issue to the board if false.
//! - `dev_may_open_prs`: `done` is refused if false (the evidence is a PR).
//! - `dev_branch_prefix`: the branch of a ticket PR is `<prefix><agent>/<n>-<slug>`.
//! - `protected`: a PR whose head branch is protected is refused as evidence.
//! - `protected_paths`: path globs that join the built-in protected paths (additive, never a
//!   removal): the owner alone merges a PR that changes one ([`crate::rules::protected`]).
//! - `coordinator_may_merge`: `merge` is refused unless the owner allowed
//!   it. `merge` in `needs_owner` is deprecated but still refuses the merge, so an old file is
//!   not loosened silently. Use `coordinator_may_merge: false`.
//! - `ci_ignore`: CI jobs that the CI judgement does not count (spec/ci#jobs-that-ci_ignore-skips).
//! - `ci_mode`: `ci` (default) or `local` (spec/ci#local-run-ci_mode-local). Read from the
//!   default branch only, so a PR cannot switch it.
//! - `coordinator_may_split_tickets`: `ticket split` is refused if false (`authority-split-not-allowed`).
//! - `former_repos`: earlier names of the main repo, each with an exclusive cutoff. A message
//!   that fails under the current name also verifies under such a name if its signed `ts` is
//!   before the cutoff (spec/messages#repository-moves). A block list of
//!   `{ name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }` items.
//!
//! Whatever the keys say, the owner alone merges a PR that changes a protected path
//! ([`crate::rules::protected`]).

use crate::base::ids;
use crate::protocol::former::FormerRepo;
use crate::rules::{Refusal, Verdict};

/// The keys that crewboard reads and enforces (spec/board#authority).
pub const ENFORCED_KEYS: [&str; 12] = [
    "initiative",
    "owner",
    "coordinator_may_create_tickets",
    "coordinator_may_split_tickets",
    "coordinator_may_merge",
    "dev_may_open_prs",
    "dev_branch_prefix",
    "protected",
    "protected_paths",
    "ci_ignore",
    "ci_mode",
    "former_repos",
];

/// The keys that crewboard does not read: the models read them as the owner's rules. (`needs_owner`
/// holds the words of release, credentials and the like; `merge` in it is deprecated and still refuses.)
pub const ADVICE_KEYS: [&str; 1] = ["needs_owner"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    pub initiative: Option<String>,
    /// Deprecated: `owner_login` of the board config is the one source (`start` refuses a login
    /// that differs and prints a note when this key is set).
    pub owner: Option<String>,
    pub coordinator_may_create_tickets: bool,
    pub coordinator_may_split_tickets: bool,
    pub coordinator_may_merge: bool,
    pub dev_may_open_prs: bool,
    pub dev_branch_prefix: String,
    pub protected: Vec<String>,
    /// Path globs added to the built-in protected paths (spec/board#authority).
    pub protected_paths: Vec<String>,
    pub needs_owner: Vec<String>,
    /// Job names that the CI judgement does not count: exact, or a prefix with a final `*`.
    pub ci_ignore: Vec<String>,
    /// How the Gate of a ticket is judged: by CI, or by a local gate of the owner's decision.
    pub ci_mode: CiMode,
    /// Earlier names of the main repo with a cutoff (a transfer or a rename).
    pub former_repos: Vec<FormerRepo>,
    /// Keys that this version does not know (shown, never obeyed).
    pub unknown_keys: Vec<String>,
}

/// The value of `ci_mode` (spec/ci#local-run-ci_mode-local).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CiMode {
    /// CI judges the head of a PR (the default).
    #[default]
    Ci,
    /// The owner switched CI off: a local gate replaces the CI run.
    Local,
}

impl CiMode {
    pub fn is_local(self) -> bool {
        self == CiMode::Local
    }
}

/// The values that `crewboard init` writes (and that apply while a key is missing).
impl Default for Authority {
    fn default() -> Self {
        Authority {
            initiative: None,
            owner: None,
            coordinator_may_create_tickets: true,
            coordinator_may_split_tickets: true,
            coordinator_may_merge: false,
            dev_may_open_prs: true,
            dev_branch_prefix: "crew/".into(),
            protected: vec!["main".into()],
            protected_paths: Vec::new(),
            // `merge` has its own flag; `ci-config` is enforced now (workflows are a protected path).
            needs_owner: ["release", "credentials", "delete", "new-scope", "new-repo"]
                .into_iter()
                .map(String::from)
                .collect(),
            ci_ignore: Vec::new(),
            ci_mode: CiMode::Ci,
            former_repos: Vec::new(),
            unknown_keys: Vec::new(),
        }
    }
}

fn bad(m: impl Into<String>) -> Refusal {
    Refusal::new(ids::AUTHORITY_BAD_FRONT_MATTER, m)
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(v)
        .to_string()
}

fn parse_bool(key: &str, v: &str) -> Result<bool, Refusal> {
    match v {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(bad(format!("{key}: '{other}' is not true or false"))),
    }
}

fn parse_flow_list(key: &str, v: &str) -> Result<Vec<String>, Refusal> {
    let inner = v
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .ok_or_else(|| bad(format!("{key}: '{v}' is not a list like [a, b]")))?;
    Ok(inner
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect())
}

/// The `ci_ignore` entries: a name, or a prefix with ONE final `*`. A lone `*`, an empty
/// entry or a `*` anywhere else is refused, so that a typo cannot ignore more (or less) than
/// it says.
fn check_ci_ignore(items: Vec<String>) -> Result<Vec<String>, Refusal> {
    for i in &items {
        let body = i.strip_suffix('*').unwrap_or(i);
        if i.trim().is_empty() || body.contains('*') || body.is_empty() {
            return Err(bad(format!(
                "ci_ignore: '{i}' is not a job name or a prefix with a final * (a lone * and a * inside the name are not allowed)"
            )));
        }
    }
    Ok(items)
}

/// The `protected_paths` entries: relative path globs ([`crate::rules::protected::check_globs`]);
/// the refusal names the bad entry.
fn check_protected_paths(items: Vec<String>) -> Result<Vec<String>, Refusal> {
    crate::rules::protected::check_globs(&items)
        .map_err(|e| bad(format!("protected_paths: {e}")))?;
    Ok(items)
}

/// One `former_repos` item: `{ name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }`. Both
/// keys are required and nothing else is allowed, so a typo cannot widen what verifies.
fn parse_former_repo(item: &str) -> Result<FormerRepo, Refusal> {
    let err = |m: &str| bad(format!("former_repos: '{item}': {m}"));
    let inner = item
        .trim()
        .strip_prefix('{')
        .and_then(|x| x.strip_suffix('}'))
        .ok_or_else(|| err("expected { name: owner/repo, until: YYYY-MM-DDTHH:MM:SSZ }"))?;
    let (mut name, mut until) = (None, None);
    for part in inner.split(',') {
        let (k, v) = part
            .split_once(':')
            .ok_or_else(|| err("expected key: value pairs"))?;
        let slot = match k.trim() {
            "name" => &mut name,
            "until" => &mut until,
            other => return Err(err(&format!("unknown key '{other}'"))),
        };
        if slot.replace(unquote(v)).is_some() {
            return Err(err("a key is given twice"));
        }
    }
    let (Some(name), Some(until)) = (name, until) else {
        return Err(err("both name and until are required"));
    };
    FormerRepo::parse(&name, &until).map_err(|e| err(&e.to_string()))
}

/// The `former_repos` items: no name twice.
fn check_former_repos(items: Vec<String>) -> Result<Vec<FormerRepo>, Refusal> {
    let mut out: Vec<FormerRepo> = Vec::new();
    for i in &items {
        let f = parse_former_repo(i)?;
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&f.name)) {
            return Err(bad(format!("former_repos: '{}' is listed twice", f.name)));
        }
        out.push(f);
    }
    Ok(out)
}

/// Does `ci_ignore` list this job name? An entry matches exactly, or as a prefix when it ends
/// with `*`. Nothing else (no regex, no glob inside the name).
pub fn ci_ignored(list: &[String], name: &str) -> bool {
    list.iter().any(|e| match e.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => e == name,
    })
}

/// Parse the front matter. `Ok(None)`: the text has no front matter (no `---` first line).
pub fn parse(text: &str) -> Result<Option<Authority>, Refusal> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Ok(None);
    }
    let mut a = Authority::default();
    let mut closed = false;
    // A key whose value is a block list: its name and the items so far.
    let mut block: Option<(String, Vec<String>)> = None;
    let flush = |a: &mut Authority, b: &mut Option<(String, Vec<String>)>| -> Result<(), Refusal> {
        if let Some((k, items)) = b.take() {
            match k.as_str() {
                "protected" => a.protected = items,
                "protected_paths" => a.protected_paths = check_protected_paths(items)?,
                "needs_owner" => a.needs_owner = items,
                "ci_ignore" => a.ci_ignore = check_ci_ignore(items)?,
                "former_repos" => a.former_repos = check_former_repos(items)?,
                other => return Err(bad(format!("{other}: a list is not allowed here"))),
            }
        }
        Ok(())
    };
    for raw in lines {
        let line = raw.split('#').next().unwrap_or("").trim_end();
        if raw.trim() == "---" {
            closed = true;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        if let Some(item) = line.trim_start().strip_prefix("- ") {
            match block.as_mut() {
                Some((_, items)) => items.push(unquote(item)),
                None => return Err(bad(format!("list item without a key: '{}'", line.trim()))),
            }
            continue;
        }
        flush(&mut a, &mut block)?;
        if line.starts_with(char::is_whitespace) {
            return Err(bad(format!(
                "nested value is not supported: '{}'",
                line.trim()
            )));
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| bad(format!("'{}' is not key: value", line.trim())))?;
        let (k, v) = (k.trim(), v.trim());
        if v.is_empty() {
            if matches!(
                k,
                "protected" | "protected_paths" | "needs_owner" | "ci_ignore" | "former_repos"
            ) {
                block = Some((k.to_string(), Vec::new()));
                continue;
            }
            return Err(bad(format!("{k} has no value")));
        }
        match k {
            "initiative" => a.initiative = Some(unquote(v)),
            "owner" => a.owner = Some(unquote(v)),
            "coordinator_may_create_tickets" => {
                a.coordinator_may_create_tickets = parse_bool(k, v)?
            }
            "coordinator_may_split_tickets" => a.coordinator_may_split_tickets = parse_bool(k, v)?,
            "coordinator_may_merge" => a.coordinator_may_merge = parse_bool(k, v)?,
            "dev_may_open_prs" => a.dev_may_open_prs = parse_bool(k, v)?,
            "dev_branch_prefix" => {
                let p = unquote(v);
                if p.is_empty() || p.contains(char::is_whitespace) {
                    return Err(bad("dev_branch_prefix must be a non-empty word like crew/"));
                }
                a.dev_branch_prefix = p;
            }
            "protected" => a.protected = parse_flow_list(k, v)?,
            "protected_paths" => a.protected_paths = check_protected_paths(parse_flow_list(k, v)?)?,
            "needs_owner" => a.needs_owner = parse_flow_list(k, v)?,
            "ci_ignore" => a.ci_ignore = check_ci_ignore(parse_flow_list(k, v)?)?,
            "former_repos" if v == "[]" => a.former_repos = Vec::new(),
            "former_repos" => {
                return Err(bad(
                    "former_repos: write one `- { name: owner/repo, until: ...Z }` item per line",
                ));
            }
            "ci_mode" => {
                a.ci_mode = match unquote(v).as_str() {
                    "ci" => CiMode::Ci,
                    "local" => CiMode::Local,
                    other => {
                        return Err(bad(format!("ci_mode: '{other}' is not ci or local")));
                    }
                }
            }
            other => a.unknown_keys.push(other.to_string()),
        }
    }
    flush(&mut a, &mut block)?;
    if !closed {
        return Err(bad("the front matter has no closing ---"));
    }
    Ok(Some(a))
}

impl Authority {
    /// `crew/` always ends with a slash (`crew` and `crew/` mean the same).
    pub fn branch_prefix(&self) -> String {
        let p = &self.dev_branch_prefix;
        if p.ends_with('/') {
            p.clone()
        } else {
            format!("{p}/")
        }
    }

    fn owner_only(&self, what: &str) -> bool {
        self.needs_owner.iter().any(|n| n == what)
    }

    /// `assign` of an issue that is not yet on the board creates a board ticket.
    pub fn check_create_ticket(&self) -> Verdict {
        if self.coordinator_may_create_tickets {
            Ok(())
        } else {
            Err(Refusal::new(
                ids::AUTHORITY_NO_TICKET_CREATION,
                "AUTHORITY.md: coordinator_may_create_tickets is false; the owner adds the ticket to the board",
            ))
        }
    }

    /// `ticket split` moves the open part of a ticket to a new ticket.
    pub fn check_split_ticket(&self) -> Verdict {
        if self.coordinator_may_split_tickets {
            Ok(())
        } else {
            Err(Refusal::new(
                ids::AUTHORITY_SPLIT_NOT_ALLOWED,
                "AUTHORITY.md: coordinator_may_split_tickets is false; the owner splits the ticket",
            ))
        }
    }

    /// `done` delivers a PR.
    pub fn check_open_pr(&self) -> Verdict {
        if self.dev_may_open_prs {
            Ok(())
        } else {
            Err(Refusal::new(
                ids::AUTHORITY_NO_PRS,
                "AUTHORITY.md: dev_may_open_prs is false; ask the owner how to deliver",
            ))
        }
    }

    /// A PR from a protected branch is no evidence (spec/tickets#branches: no agent pushes there).
    pub fn check_head_branch(&self, head_ref: &str) -> Verdict {
        if self.protected.iter().any(|p| p == head_ref) {
            Err(Refusal::new(
                ids::EVIDENCE_BRANCH_PROTECTED,
                format!("the PR head '{head_ref}' is a protected branch (AUTHORITY.md: protected)"),
            ))
        } else {
            Ok(())
        }
    }

    /// `merge`: never without the owner's permission. Two keys decide:
    /// `merge` in `needs_owner` (deprecated advice that still wins) over
    /// `coordinator_may_merge: true`.
    pub fn check_merge(&self) -> Verdict {
        if self.owner_only("merge") {
            return Err(Refusal::new(
                ids::AUTHORITY_NEEDS_OWNER,
                "AUTHORITY.md: merge is in needs_owner; only the owner merges (needs_owner is advice from 0.4: set coordinator_may_merge: false instead)",
            ));
        }
        if !self.coordinator_may_merge {
            return Err(Refusal::new(
                ids::AUTHORITY_MERGE_NOT_ALLOWED,
                "AUTHORITY.md: coordinator_may_merge is false; the owner merges",
            ));
        }
        Ok(())
    }
}

/// Keys of AUTHORITY.md that contradict each other (spec/board#contradicting-keys-in-authoritymd).
/// The file is still obeyed; the loader and `start`/`selftest` warn, so the owner's ruling does not
/// look ignored.
impl Authority {
    pub fn contradictions(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.coordinator_may_merge && self.owner_only("merge") {
            v.push(
                "coordinator_may_merge is true, but `merge` is in needs_owner. Effective rule in this release: needs_owner wins, only the owner merges, and the coordinator may NOT merge (nothing is loosened). needs_owner is advice from 0.4, and a later release makes `start` refuse a file with both. Fix: remove `merge` from needs_owner to allow the coordinator, or set coordinator_may_merge: false to say that only the owner merges"
                    .to_string(),
            );
        }
        v
    }

    /// The warning lines of [`Authority::contradictions`], each ending with a newline.
    pub fn contradiction_warnings(&self) -> String {
        self.contradictions()
            .iter()
            .map(|c| format!("warning: AUTHORITY.md contradicts itself: {c}\n"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INIT_TEMPLATE: &str = "---\n\
initiative: sandbox\n\
owner: owner-login\n\
coordinator_may_create_tickets: true\n\
coordinator_may_split_tickets: true\n\
coordinator_may_merge: false        # true only if the owner allows it\n\
dev_may_open_prs: true\n\
dev_branch_prefix: crew/\n\
protected: [main]\n\
needs_owner: [release, credentials, delete, new-scope, new-repo]\n\
---\n\n# text\n";

    fn parsed(t: &str) -> Authority {
        parse(t).unwrap().unwrap()
    }

    #[test]
    fn the_template_of_init_parses_to_its_own_defaults() {
        let a = parsed(INIT_TEMPLATE);
        assert_eq!(a.owner.as_deref(), Some("owner-login"));
        assert_eq!(a.initiative.as_deref(), Some("sandbox"));
        let d = Authority {
            initiative: a.initiative.clone(),
            owner: a.owner.clone(),
            ..Authority::default()
        };
        assert_eq!(a, d);
        assert!(a.unknown_keys.is_empty());
    }

    #[test]
    fn values_are_read_not_defaulted() {
        let t = INIT_TEMPLATE
            .replace(
                "coordinator_may_create_tickets: true",
                "coordinator_may_create_tickets: false",
            )
            .replace("dev_may_open_prs: true", "dev_may_open_prs: false")
            .replace("dev_branch_prefix: crew/", "dev_branch_prefix: team/x")
            .replace("protected: [main]", "protected: [main, release]")
            .replace(
                "coordinator_may_merge: false",
                "coordinator_may_merge: true",
            );
        let a = parsed(&t);
        assert!(!a.coordinator_may_create_tickets && !a.dev_may_open_prs);
        assert!(a.coordinator_may_merge);
        assert_eq!(a.branch_prefix(), "team/x/");
        assert_eq!(a.protected, vec!["main".to_string(), "release".into()]);
    }

    #[test]
    fn protected_paths_is_a_flow_or_block_list_and_defaults_to_empty() {
        assert!(parsed(INIT_TEMPLATE).protected_paths.is_empty());
        let a = parsed("---\nprotected_paths: [tests/**, \"scripts/ci-*.sh\"]\n---\n");
        assert_eq!(a.protected_paths, vec!["tests/**", "scripts/ci-*.sh"]);
        let b = parsed("---\nprotected_paths:\n  - Makefile\n  - ci/\n---\n");
        assert_eq!(b.protected_paths, vec!["Makefile", "ci/"]);
        // `protected` (branch names) is a different key and stays separate
        assert_eq!(b.protected, vec!["main".to_string()]);
    }

    #[test]
    fn a_bad_protected_paths_entry_is_named_in_the_refusal() {
        for (list, entry) in [
            ("[tests/**, /etc/x]", "'/etc/x'"),
            ("[a/../b]", "'a/../b'"),
            ("[ok, ]x", ""),
        ] {
            let r = parse(&format!("---\nprotected_paths: {list}\n---\n")).unwrap_err();
            assert_eq!(r.rule, "authority-bad-front-matter");
            assert!(r.message.contains("protected_paths"), "{}", r.message);
            assert!(r.message.contains(entry), "{}", r.message);
        }
        let r = parse("---\nprotected_paths:\n  - ok\n  - \"\"\n---\n").unwrap_err();
        assert!(
            r.message.contains("protected_paths: an entry is empty"),
            "{}",
            r.message
        );
    }

    #[test]
    fn block_lists_comments_quotes_and_unknown_keys() {
        let a = parsed(
            "---\nowner: \"w\"  # me\nprotected:\n  - main\n  - 'rel'\nneeds_owner: []\nfuture_flag: yes\n---\n",
        );
        assert_eq!(a.owner.as_deref(), Some("w"));
        assert_eq!(a.protected, vec!["main".to_string(), "rel".into()]);
        assert!(a.needs_owner.is_empty());
        assert_eq!(a.unknown_keys, vec!["future_flag".to_string()]);
    }

    #[test]
    fn no_front_matter_is_none_and_a_broken_one_is_refused() {
        assert_eq!(parse("# just text\n").unwrap(), None);
        assert_eq!(parse("").unwrap(), None);
        for bad in [
            "---\ncoordinator_may_merge: maybe\n---\n",
            "---\ndev_may_open_prs: yes\n---\n",
            "---\nprotected: main\n---\n",
            "---\nowner: x\n",
            "---\n- orphan\n---\n",
            "---\n  nested: 1\n---\n",
            "---\nno colon here\n---\n",
            "---\ndev_branch_prefix: a b\n---\n",
            "---\ndev_branch_prefix:\n---\n",
        ] {
            let e = parse(bad).expect_err(bad);
            assert_eq!(e.rule, "authority-bad-front-matter", "{bad}");
        }
    }

    #[test]
    fn ci_ignore_is_read_as_a_block_or_flow_list_and_is_empty_by_default() {
        assert!(Authority::default().ci_ignore.is_empty());
        assert!(parsed("---\nowner: o\n---\n").ci_ignore.is_empty());
        let block = parsed("---\nci_ignore:\n  - deploy-preview\n  - \"lint*\"\n---\n");
        assert_eq!(block.ci_ignore, vec!["deploy-preview", "lint*"]);
        let flow = parsed("---\nci_ignore: [deploy-preview, 'lint*']\n---\n");
        assert_eq!(flow.ci_ignore, block.ci_ignore);
        assert!(flow.unknown_keys.is_empty());
    }

    #[test]
    fn ci_mode_is_ci_by_default_and_reads_ci_and_local() {
        assert_eq!(Authority::default().ci_mode, CiMode::Ci);
        assert_eq!(parsed("---\nowner: o\n---\n").ci_mode, CiMode::Ci);
        assert_eq!(parsed("---\nci_mode: ci\n---\n").ci_mode, CiMode::Ci);
        let local = parsed("---\nci_mode: local   # CI is down\n---\n");
        assert_eq!(local.ci_mode, CiMode::Local);
        assert!(local.ci_mode.is_local() && local.unknown_keys.is_empty());
        assert_eq!(
            parsed("---\nci_mode: \"local\"\n---\n").ci_mode,
            CiMode::Local
        );
        // a commented line is no setting
        assert_eq!(parsed("---\n# ci_mode: local\n---\n").ci_mode, CiMode::Ci);
    }

    #[test]
    fn ci_mode_refuses_any_other_value() {
        for bad in [
            "---\nci_mode: off\n---\n",
            "---\nci_mode: Local\n---\n",
            "---\nci_mode: true\n---\n",
            "---\nci_mode: [local]\n---\n",
            "---\nci_mode:\n---\n",
            "---\nci_mode:\n  - local\n---\n",
        ] {
            let e = parse(bad).expect_err(bad);
            assert_eq!(e.rule, "authority-bad-front-matter", "{bad}");
        }
    }

    #[test]
    fn ci_ignore_refuses_a_lone_star_and_a_star_inside_the_name() {
        for bad in [
            "---\nci_ignore: [\"*\"]\n---\n",
            "---\nci_ignore:\n  - \"*\"\n---\n",
            "---\nci_ignore: [\"a*b\"]\n---\n",
            "---\nci_ignore: [\"*a\"]\n---\n",
            "---\nci_ignore: [\"a**\"]\n---\n",
            "---\nci_ignore: true\n---\n",
        ] {
            let e = parse(bad).expect_err(bad);
            assert_eq!(e.rule, "authority-bad-front-matter", "{bad}");
        }
    }

    #[test]
    fn ci_ignored_matches_exactly_or_by_prefix_and_nothing_else() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let list = l(&["deploy-preview", "lint*"]);
        assert!(ci_ignored(&list, "deploy-preview"));
        assert!(ci_ignored(&list, "lint"));
        assert!(ci_ignored(&list, "lint (ubuntu)"));
        assert!(!ci_ignored(&list, "deploy-preview-2"));
        assert!(!ci_ignored(&list, "Deploy-Preview"));
        assert!(!ci_ignored(&list, "my-lint"));
        assert!(!ci_ignored(&[], "deploy-preview"));
    }

    #[test]
    fn ticket_creation_follows_the_flag_in_both_directions() {
        assert!(Authority::default().check_create_ticket().is_ok());
        let off = Authority {
            coordinator_may_create_tickets: false,
            ..Authority::default()
        };
        assert_eq!(
            off.check_create_ticket().unwrap_err().rule,
            "authority-no-ticket-creation"
        );
    }

    #[test]
    fn splitting_follows_the_flag_in_both_directions() {
        assert!(Authority::default().check_split_ticket().is_ok());
        let off = Authority {
            coordinator_may_split_tickets: false,
            ..Authority::default()
        };
        assert_eq!(
            off.check_split_ticket().unwrap_err().rule,
            "authority-split-not-allowed"
        );
    }

    #[test]
    fn prs_follow_the_flag_in_both_directions() {
        assert!(Authority::default().check_open_pr().is_ok());
        let off = Authority {
            dev_may_open_prs: false,
            ..Authority::default()
        };
        assert_eq!(off.check_open_pr().unwrap_err().rule, "authority-no-prs");
    }

    #[test]
    fn a_protected_head_branch_is_refused_and_another_passes() {
        let a = Authority::default();
        assert_eq!(
            a.check_head_branch("main").unwrap_err().rule,
            "evidence-branch-protected"
        );
        assert!(a.check_head_branch("crew/x@y/3-work").is_ok());
        assert!(a.check_head_branch("main-fix").is_ok());
    }

    #[test]
    fn the_branch_prefix_always_ends_with_a_slash() {
        let mut a = Authority::default();
        assert_eq!(a.branch_prefix(), "crew/");
        a.dev_branch_prefix = "crew".into();
        assert_eq!(a.branch_prefix(), "crew/");
    }

    fn with_merge_advice() -> Authority {
        Authority {
            needs_owner: vec!["merge".into(), "release".into()],
            ..Authority::default()
        }
    }

    #[test]
    fn merge_is_refused_by_the_flag_and_by_a_merge_entry_of_needs_owner_and_allowed_only_by_both_keys()
     {
        // default: the flag is false, and `merge` is not in the default advice list
        assert!(
            !Authority::default()
                .needs_owner
                .iter()
                .any(|n| n == "merge")
        );
        assert_eq!(
            Authority::default().check_merge().unwrap_err().rule,
            "authority-merge-not-allowed"
        );
        // flag false and `merge` listed: the flag refuses first, the file still holds both
        let a = Authority {
            coordinator_may_merge: false,
            ..with_merge_advice()
        };
        assert_eq!(a.check_merge().unwrap_err().rule, "authority-needs-owner");
        // flag true and no `merge` entry: allowed
        let b = Authority {
            coordinator_may_merge: true,
            ..Authority::default()
        };
        assert!(b.check_merge().is_ok());
        // flag true but `merge` still listed: the old file keeps its stricter meaning
        let c = Authority {
            coordinator_may_merge: true,
            ..with_merge_advice()
        };
        let r = c.check_merge().unwrap_err();
        assert_eq!(r.rule, "authority-needs-owner");
        assert!(
            r.message
                .contains("set coordinator_may_merge: false instead"),
            "{}",
            r.message
        );
    }

    #[test]
    fn every_enforced_key_is_read_and_needs_owner_is_advice_only() {
        for k in ENFORCED_KEYS {
            let value = match k {
                "coordinator_may_create_tickets"
                | "coordinator_may_split_tickets"
                | "coordinator_may_merge"
                | "dev_may_open_prs" => "true",
                "protected" | "ci_ignore" | "protected_paths" => "[a]",
                "former_repos" => "[]",
                "ci_mode" => "ci",
                _ => "x",
            };
            let a = parsed(&format!("---\n{k}: {value}\n---\n"));
            assert!(
                a.unknown_keys.is_empty(),
                "{k} is unknown: {:?}",
                a.unknown_keys
            );
        }
        assert_eq!(ADVICE_KEYS, ["needs_owner"]);
        assert!(!ENFORCED_KEYS.contains(&"needs_owner"));
        // control: a key that is in neither list is noted as unknown
        assert_eq!(
            parsed("---\nmade_up: 1\n---\n").unknown_keys,
            vec!["made_up".to_string()]
        );
    }

    #[test]
    fn contradicting_merge_keys_warn_and_a_consistent_file_is_silent() {
        // a file that contradicts itself: coordinator_may_merge true and merge still in needs_owner
        let c = Authority {
            coordinator_may_merge: true,
            ..with_merge_advice()
        };
        let w = c.contradiction_warnings();
        assert!(
            w.contains("coordinator_may_merge") && w.contains("needs_owner") && w.contains("wins"),
            "{w}"
        );
        // control: the defaults, and a file that allows the merge cleanly, are silent
        assert_eq!(Authority::default().contradiction_warnings(), "");
        let ok = Authority {
            coordinator_may_merge: true,
            needs_owner: vec!["release".into()],
            ..Authority::default()
        };
        assert_eq!(ok.contradiction_warnings(), "");
        // the file text parses to the same verdict
        let t = "---\ncoordinator_may_merge: true\nneeds_owner: [merge, release]\n---\n";
        assert_eq!(parse(t).unwrap().unwrap().contradictions().len(), 1);
        // the note names the effective rule, and the behaviour is unchanged: merge stays refused
        assert!(w.contains("Effective rule"), "{w}");
        assert!(w.contains("coordinator may NOT merge"), "{w}");
        assert!(w.contains("nothing is loosened"), "{w}");
        assert_eq!(c.check_merge().unwrap_err().rule, "authority-needs-owner");
    }

    #[test]
    fn former_repos_is_a_block_list_of_name_and_cutoff() {
        let t = "---\nowner: o\nformer_repos:\n  - { name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }  # moved\n  - {name: \"first/one\", until: 2026-06-01T00:00:00Z}\n---\n";
        let a = parsed(t);
        assert_eq!(a.former_repos.len(), 2);
        assert_eq!(a.former_repos[0].name, "old-owner/old-repo");
        assert_eq!(a.former_repos[0].until.as_str(), "2026-11-01T12:00:00Z");
        assert_eq!(a.former_repos[1].name, "first/one");
        assert!(a.unknown_keys.is_empty());
        // control: absent or empty means none
        assert!(parsed("---\nowner: o\n---\n").former_repos.is_empty());
        assert!(
            parsed("---\nowner: o\nformer_repos: []\n---\n")
                .former_repos
                .is_empty()
        );
    }

    #[test]
    fn a_bad_former_repos_entry_is_refused_not_ignored() {
        for item in [
            "{ name: old-owner/old-repo }",
            "{ until: 2026-11-01T12:00:00Z }",
            "{ name: old, until: 2026-11-01T12:00:00Z }",
            "{ name: a/b/c, until: 2026-11-01T12:00:00Z }",
            "{ name: a/b, until: 2026-11-01 }",
            "{ name: a/b, until: 2026-11-01T12:00:00+01:00 }",
            "{ name: a/b, until: 2026-11-01T12:00:00Z, extra: 1 }",
            "{ name: a/b, name: c/d, until: 2026-11-01T12:00:00Z }",
            "name: a/b",
            "old-owner/old-repo",
        ] {
            let t = format!("---\nformer_repos:\n  - {item}\n---\n");
            let e = parse(&t).unwrap_err();
            assert_eq!(e.rule, "authority-bad-front-matter", "{item}");
        }
        // the same name twice, and a flow list of maps (not supported)
        let twice = "---\nformer_repos:\n  - { name: a/b, until: 2026-11-01T12:00:00Z }\n  - { name: A/B, until: 2026-12-01T12:00:00Z }\n---\n";
        assert!(parse(twice).is_err());
        let flow = "---\nformer_repos: [{ name: a/b, until: 2026-11-01T12:00:00Z }]\n---\n";
        assert!(parse(flow).is_err());
    }
}
