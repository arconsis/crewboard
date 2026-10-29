//! Do the workflows of a repo run for crew branches? (spec/ci#ci-must-run-for-crew-branches)
//!
//! `done` needs a CI run for the head of the PR. If no workflow triggers on a push to a
//! `dev_branch_prefix` branch, and none on `pull_request`, no crew PR can reach Review, and
//! the agents find out only when the first PR waits. `init`, `start` and `selftest` read the
//! workflow files and warn.
//!
//! The YAML reader is minimal on purpose (the repo has no YAML crate). It reads the `on:` key
//! of a workflow in these forms:
//!
//! ```yaml
//! on: push
//! on: [push, pull_request]
//! on:
//!   - push
//! on:
//!   push:
//!     branches: [main, 'crew/**']      # or a block list
//!     branches-ignore: [...]
//!     tags: [...]
//!   pull_request:
//!     branches: [main]
//!     paths-ignore: ['docs/**']        # or `paths`, a flow or a block list
//! ```
//!
//! A form that it does not understand counts as "unknown", never as "no trigger": the warning
//! says that the file could not be read, so a parser gap cannot make a false alarm look sure.
//! A filter value of `push`, `pull_request` or `pull_request_target` that is not a scalar, a
//! one-line flow list or a block list (for example a flow list over several lines) makes the
//! whole file unknown.
//!
//! `judge_no_run` (in `services::ci_triggers`) uses the same reader for `done` and `verify`: can
//! a PR head have no CI run because every workflow ignores its files?
//! (spec/ci#files-that-every-workflow-ignores) This module is pure: it reads no workflow file.

use regex::Regex;
use serde_json::Value;

/// The branch filter of one event.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub branches: Option<Vec<String>>,
    pub branches_ignore: Option<Vec<String>>,
    /// A `tags` or `tags-ignore` filter (a push event with only a tag filter does not run
    /// for a branch push).
    pub has_tag_filter: bool,
    /// The `paths` allow-list (spec/ci#ci-and-crewboard-only-commits).
    pub paths: Option<Vec<String>>,
    /// The `paths-ignore` list (spec/ci#ci-and-crewboard-only-commits).
    pub paths_ignore: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Triggers {
    pub push: Option<Filter>,
    pub pull_request: Option<Filter>,
    /// `pull_request_target`: its runs count as CI too (spec/ci#which-ci-runs-count).
    pub pull_request_target: Option<Filter>,
    /// The names of all other events, in file order.
    pub other: Vec<String>,
}

/// What the reader found in one workflow file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum Parsed {
    Triggers(Triggers),
    /// The file has no `on:` key, or its form is not read.
    Unknown,
}

fn strip_comment(l: &str) -> String {
    let mut out = String::new();
    let (mut sq, mut dq) = (false, false);
    let mut prev = ' ';
    for c in l.chars() {
        match c {
            '\'' if !dq => sq = !sq,
            '"' if !sq => dq = !dq,
            '#' if !sq && !dq && (prev == ' ' || prev == '\t' || out.is_empty()) => break,
            _ => {}
        }
        out.push(c);
        prev = c;
    }
    out.trim_end().to_string()
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
        .unwrap_or(s)
        .to_string()
}

fn indent_of(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

/// The value of `key: value` as items (a scalar or a one-line flow list). `None`: the value
/// cannot be read (an open flow list, a map, a nested list).
fn inline_items(v: &str) -> Option<Vec<String>> {
    parse_flow(v).as_ref().and_then(filter_items)
}

/// The items of a filter value: a scalar or a list of scalars.
fn filter_items(v: &Val) -> Option<Vec<String>> {
    match v {
        Val::Str(s) if s.is_empty() => Some(vec![]),
        Val::Str(s) => Some(vec![s.clone()]),
        Val::List(l) => l
            .iter()
            .map(|x| match x {
                Val::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Val::Map(_) => None,
    }
}

/// A value of a YAML flow form: `a`, `[a, b]`, `{k: v, k2: [x]}`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    Str(String),
    List(Vec<Val>),
    Map(Vec<(String, Val)>),
}

struct Flow<'a> {
    c: &'a [char],
    i: usize,
}

impl Flow<'_> {
    fn ws(&mut self) {
        while self.i < self.c.len() && self.c[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn scalar(&mut self) -> Option<String> {
        self.ws();
        let q = *self.c.get(self.i)?;
        if q == '\'' || q == '"' {
            self.i += 1;
            let st = self.i;
            while self.i < self.c.len() && self.c[self.i] != q {
                self.i += 1;
            }
            let t: String = self.c[st..self.i].iter().collect();
            self.i += 1;
            return Some(t);
        }
        let st = self.i;
        while self.i < self.c.len() && !matches!(self.c[self.i], ',' | ']' | '}' | ':') {
            self.i += 1;
        }
        Some(
            self.c[st..self.i]
                .iter()
                .collect::<String>()
                .trim()
                .to_string(),
        )
    }

    fn value(&mut self) -> Option<Val> {
        self.ws();
        match self.c.get(self.i)? {
            '[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    match self.c.get(self.i)? {
                        ']' => {
                            self.i += 1;
                            return Some(Val::List(v));
                        }
                        ',' => self.i += 1,
                        _ => v.push(self.value()?),
                    }
                }
            }
            '{' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    match self.c.get(self.i)? {
                        '}' => {
                            self.i += 1;
                            return Some(Val::Map(v));
                        }
                        ',' => self.i += 1,
                        _ => {
                            let k = self.scalar()?;
                            self.ws();
                            let val = if self.c.get(self.i) == Some(&':') {
                                self.i += 1;
                                self.ws();
                                if matches!(self.c.get(self.i), Some(',' | '}')) {
                                    Val::Str(String::new())
                                } else {
                                    self.value()?
                                }
                            } else {
                                Val::Str(String::new())
                            };
                            v.push((k, val));
                        }
                    }
                }
            }
            _ => Some(Val::Str(self.scalar()?)),
        }
    }
}

/// Parse a whole flow value. Text after the value (an open list on the next line, a second
/// `:`) makes it unreadable.
fn parse_flow(s: &str) -> Option<Val> {
    let chars: Vec<char> = s.chars().collect();
    let mut f = Flow { c: &chars, i: 0 };
    let v = f.value()?;
    f.ws();
    (f.i >= chars.len()).then_some(v)
}

fn strings(v: &Val) -> Vec<String> {
    match v {
        Val::Str(s) if s.is_empty() => vec![],
        Val::Str(s) => vec![s.clone()],
        Val::List(l) => l.iter().flat_map(strings).collect(),
        Val::Map(_) => vec![],
    }
}

/// The events whose filters the reader must understand (their runs count as CI).
fn is_ci_event(event: &str) -> bool {
    matches!(event, "push" | "pull_request" | "pull_request_target")
}

/// Set one filter key. `false`: the value of a key of a CI event cannot be read.
fn set_key(f: &mut Filter, key: &str, items: Option<Vec<String>>) -> bool {
    let slot = match key {
        "branches" => &mut f.branches,
        "branches-ignore" => &mut f.branches_ignore,
        "paths" => &mut f.paths,
        "paths-ignore" => &mut f.paths_ignore,
        "tags" | "tags-ignore" => {
            f.has_tag_filter = true;
            return true;
        }
        _ => return true,
    };
    match items {
        Some(v) => {
            *slot = Some(v);
            true
        }
        None => false,
    }
}

/// The filter of an event in flow form (`push: {branches: [a]}`). `None`: a filter value of a
/// CI event cannot be read.
fn filter_of(event: &str, v: &Val) -> Option<Filter> {
    let mut f = Filter::default();
    if let Val::Map(m) = v {
        for (k, val) in m {
            if !set_key(&mut f, k, filter_items(val)) && is_ci_event(event) {
                return None;
            }
        }
    }
    Some(f)
}

fn set_event(t: &mut Triggers, event: &str, f: Filter) {
    match event {
        "push" => t.push = Some(f),
        "pull_request" => t.pull_request = Some(f),
        "pull_request_target" => t.pull_request_target = Some(f),
        other => {
            if !t.other.iter().any(|e| e == other) {
                t.other.push(other.to_string());
            }
        }
    }
}

fn triggers_of(v: &Val) -> Option<Triggers> {
    let mut t = Triggers::default();
    match v {
        Val::Map(m) => {
            for (k, val) in m {
                set_event(&mut t, k, filter_of(k, val)?);
            }
        }
        other => {
            for e in strings(other) {
                set_event(&mut t, &e, Filter::default());
            }
        }
    }
    Some(t)
}

/// Parse the `on:` key of a workflow file.
pub fn parse(text: &str) -> Parsed {
    let lines: Vec<String> = text
        .lines()
        .map(strip_comment)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let Some(start) = lines.iter().position(|l| {
        indent_of(l) == 0
            && matches!(
                l.split(':').next().map(str::trim),
                Some("on" | "'on'" | "\"on\"")
            )
            && l.contains(':')
    }) else {
        return Parsed::Unknown;
    };
    let head = &lines[start];
    let value = head.split_once(':').map_or("", |x| x.1).trim();
    let mut t = Triggers::default();
    if !value.is_empty() {
        return match parse_flow(value).as_ref().and_then(triggers_of) {
            Some(t) => Parsed::Triggers(t),
            None => Parsed::Unknown,
        };
    }
    // the block under `on:`
    let block: Vec<&String> = lines[start + 1..]
        .iter()
        .take_while(|l| indent_of(l) > 0)
        .collect();
    let Some(base) = block.first().map(|l| indent_of(l)) else {
        return Parsed::Unknown;
    };
    let mut i = 0;
    while i < block.len() {
        let l = block[i];
        if indent_of(l) != base {
            i += 1;
            continue;
        }
        let tl = l.trim();
        if let Some(item) = tl.strip_prefix("- ") {
            set_event(&mut t, &unquote(item), Filter::default());
            i += 1;
            continue;
        }
        let Some((ev, rest)) = tl.split_once(':') else {
            // neither an event key nor a list item: a form that the reader does not know
            return Parsed::Unknown;
        };
        let ev = unquote(ev);
        let mut f = Filter::default();
        if !rest.trim().is_empty() {
            // `push: {branches: [a]}`
            match parse_flow(rest).and_then(|v| filter_of(&ev, &v)) {
                Some(x) => f = x,
                None => return Parsed::Unknown,
            }
        }
        // the lines of this event: deeper than `base`
        let mut j = i + 1;
        let mut sub: Vec<&String> = Vec::new();
        while j < block.len() && indent_of(block[j]) > base {
            sub.push(block[j]);
            j += 1;
        }
        let mut k = 0;
        while k < sub.len() {
            let s = sub[k].trim();
            let ind = indent_of(sub[k]);
            if let Some((key, val)) = s.split_once(':') {
                let key = key.trim();
                let items = if val.trim().is_empty() {
                    // a block list: `- item` lines deeper than (or level with) the key
                    let mut items = Vec::new();
                    let mut m = k + 1;
                    while m < sub.len()
                        && sub[m].trim().starts_with("- ")
                        && indent_of(sub[m]) >= ind
                    {
                        items.push(unquote(sub[m].trim().trim_start_matches("- ")));
                        m += 1;
                    }
                    k = m - 1;
                    Some(items)
                } else {
                    inline_items(val)
                };
                if !set_key(&mut f, key, items) && is_ci_event(&ev) {
                    return Parsed::Unknown;
                }
            }
            k += 1;
        }
        set_event(&mut t, &ev, f);
        i = j;
    }
    Parsed::Triggers(t)
}

/// A GitHub branch pattern as a regex: `*` any characters except `/`, `**` any characters,
/// `?` zero or one of the preceding character, `+` one or more of it, `[...]` a class, `\`
/// makes the next character plain. `None`: the pattern cannot be read (for example `?` at the
/// start, with nothing to repeat).
fn pattern_regex(p: &str) -> Option<Regex> {
    glob_regex(p, false)
}

/// The same for file paths: `**/` also matches zero directories (`**/*.md` matches
/// `AUTHORITY.md`).
fn path_regex(p: &str) -> Option<Regex> {
    glob_regex(p, true)
}

fn glob_regex(p: &str, path: bool) -> Option<Regex> {
    let mut out = String::from("^");
    let chars: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if path && chars.get(i + 1) == Some(&'*') && chars.get(i + 2) == Some(&'/') => {
                out.push_str("(?:.*/)?");
                i += 2;
            }
            '*' if chars.get(i + 1) == Some(&'*') => {
                out.push_str(".*");
                i += 1;
            }
            '*' => out.push_str("[^/]*"),
            '\\' => {
                let c = chars.get(i + 1)?;
                out.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
            '?' | '+' => out.push(chars[i]),
            '[' => {
                let end = chars[i..].iter().position(|c| *c == ']')? + i;
                let class: String = chars[i + 1..end].iter().collect();
                out.push('[');
                out.push_str(&class.replace('\\', "\\\\").replacen('!', "^", 1));
                out.push(']');
                i = end;
            }
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out.push('$');
    Regex::new(&out).ok()
}

/// Does the pattern list select the branch? Patterns run in order; a `!` pattern removes a
/// match; the last pattern that matches decides (GitHub's rule).
fn list_selects(list: &[String], branch: &str) -> bool {
    list_selects_with(list, branch, pattern_regex)
}

fn list_selects_with(list: &[String], branch: &str, mk: fn(&str) -> Option<Regex>) -> bool {
    let mut selected = false;
    for p in list {
        let (neg, pat) = match p.strip_prefix('!') {
            Some(x) => (true, x),
            None => (false, p.as_str()),
        };
        if mk(pat).is_some_and(|r| r.is_match(branch)) {
            selected = !neg;
        }
    }
    selected
}

impl Filter {
    /// Does an event with this filter run for `branch`?
    pub fn runs_for(&self, branch: &str) -> bool {
        match (&self.branches, &self.branches_ignore) {
            (Some(b), _) => list_selects(b, branch),
            (None, Some(ig)) => !list_selects(ig, branch),
            // only a tag filter: branch pushes do not run it
            (None, None) => !self.has_tag_filter,
        }
    }
}

/// A pattern reader: [`pattern_regex`] for branches, [`path_regex`] for files.
type PatternFn = fn(&str) -> Option<Regex>;

impl Filter {
    /// Does an event with this filter run for a change that touches only `file`?
    pub fn runs_for_file(&self, file: &str) -> bool {
        match (&self.paths, &self.paths_ignore) {
            (Some(p), _) => list_selects_with(p, file, path_regex),
            (None, Some(ig)) => !list_selects_with(ig, file, path_regex),
            (None, None) => true,
        }
    }

    /// For `judge_no_run`: the pattern that keeps `file` from starting this event, or `None`
    /// if the file starts it. A `paths-ignore` list gives the last pattern that matches; an
    /// allow-list that does not select the file gives `outside paths of <workflow>`.
    fn ignored_by(&self, file: &str, workflow: &str) -> Option<String> {
        match (&self.paths, &self.paths_ignore) {
            (Some(p), None) => (!list_selects_with(p, file, path_regex))
                .then(|| format!("outside paths of {workflow}")),
            (None, Some(ig)) => {
                let mut last: Option<&String> = None;
                for p in ig {
                    let (neg, pat) = match p.strip_prefix('!') {
                        Some(x) => (true, x),
                        None => (false, p.as_str()),
                    };
                    if path_regex(pat).is_some_and(|r| r.is_match(file)) {
                        last = (!neg).then_some(p);
                    }
                }
                last.cloned()
            }
            _ => None,
        }
    }

    /// For `judge_no_run`: why this filter cannot be judged, if it cannot. GitHub refuses a
    /// workflow with both `branches` and `branches-ignore` (or `paths` and `paths-ignore`) for
    /// one event; a pattern that does not make a regex is not judged either (an allow-list with
    /// such a pattern would otherwise exclude every file).
    fn unjudgeable(&self) -> Option<String> {
        if self.branches.is_some() && self.branches_ignore.is_some() {
            return Some("both branches and branches-ignore".into());
        }
        if self.paths.is_some() && self.paths_ignore.is_some() {
            return Some("both paths and paths-ignore".into());
        }
        let lists: [(&Option<Vec<String>>, PatternFn); 4] = [
            (&self.branches, pattern_regex),
            (&self.branches_ignore, pattern_regex),
            (&self.paths, path_regex),
            (&self.paths_ignore, path_regex),
        ];
        for (list, mk) in lists {
            for p in list.iter().flatten() {
                if mk(p.strip_prefix('!').unwrap_or(p)).is_none() {
                    return Some(format!("the pattern '{p}' cannot be read"));
                }
            }
        }
        None
    }
}

/// GitHub's events (the "Events that trigger workflows" page). A run of any of them except
/// `push`, `pull_request` and `pull_request_target` is information, never CI
/// (spec/ci#which-ci-runs-count), so `judge_no_run` does not need its filters. An event that
/// is NOT in this list is unknown: the judgement refuses.
pub const KNOWN_EVENTS: &[&str] = &[
    "branch_protection_rule",
    "check_run",
    "check_suite",
    "create",
    "delete",
    "deployment",
    "deployment_status",
    "discussion",
    "discussion_comment",
    "fork",
    "gollum",
    "image_version",
    "issue_comment",
    "issues",
    "label",
    "merge_group",
    "milestone",
    "page_build",
    "project",
    "project_card",
    "project_column",
    "public",
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "pull_request_target",
    "push",
    "registry_package",
    "release",
    "repository_dispatch",
    "schedule",
    "status",
    "watch",
    "workflow_call",
    "workflow_dispatch",
    "workflow_run",
];

/// GitHub evaluates `paths` filters on at most this many changed files; above it the result
/// is not sure (spec/ci#files-that-every-workflow-ignores).
pub const MAX_JUDGED_FILES: usize = 300;

/// At most this many patterns go into the evidence text; the rest is counted.
pub const MAX_SHOWN_PATTERNS: usize = 8;

/// Does every workflow that would run for the PR ignore every changed file? Pure.
/// `workflows` is `(name, text)`, `changed` the files of the PR (old names of renamed files
/// included), `head_ref` the PR branch (for `push`), `base` its base branch (for
/// `pull_request` and `pull_request_target`).
///
/// `Ok`: the patterns that ignore the files (no duplicates, file order). `Err`: the reason
/// that a run should exist or that the reader cannot be sure. Any doubt is an `Err`.
pub fn judge_ignored(
    workflows: &[(String, String)],
    changed: &[String],
    head_ref: &str,
    base: &str,
) -> Result<Vec<String>, String> {
    if workflows.is_empty() {
        return Err(
            "the repo has no workflow in .github/workflows, so CI is missing, not skipped".into(),
        );
    }
    if changed.is_empty() {
        return Err("the PR changes no file".into());
    }
    let mut patterns: Vec<String> = Vec::new();
    let mut matching = 0;
    for (name, text) in workflows {
        let Parsed::Triggers(t) = parse(text) else {
            return Err(format!(
                "workflow {name} cannot be read (its `on:` form is not known)"
            ));
        };
        if let Some(e) = t.other.iter().find(|e| !KNOWN_EVENTS.contains(&e.as_str())) {
            return Err(format!(
                "workflow {name} has the event '{e}', which crewboard does not know"
            ));
        }
        let events = [
            ("push", &t.push, head_ref),
            ("pull_request", &t.pull_request, base),
            ("pull_request_target", &t.pull_request_target, base),
        ];
        for (event, filter, branch) in events {
            let Some(f) = filter else { continue };
            if let Some(why) = f.unjudgeable() {
                return Err(format!("workflow {name} cannot be judged ({event}: {why})"));
            }
            if !f.runs_for(branch) {
                continue;
            }
            matching += 1;
            for file in changed {
                match f.ignored_by(file, name) {
                    Some(p) => {
                        if !patterns.contains(&p) {
                            patterns.push(p);
                        }
                    }
                    None => {
                        let filter = if f.paths.is_none() && f.paths_ignore.is_none() {
                            "has no paths filter"
                        } else {
                            "does not ignore it"
                        };
                        return Err(format!(
                            "file {file} is not ignored by workflow {name} ({event} {filter})"
                        ));
                    }
                }
            }
        }
    }
    if matching == 0 {
        return Err(format!(
            "no workflow runs on a push to {head_ref} or on a pull_request to {base}, so CI is missing, not skipped"
        ));
    }
    Ok(patterns)
}

/// Do required status checks (or rules that need a check) apply to `base`? Pure. `branch` is
/// the answer of `GET repos/{r}/branches/{base}` (its `protection` is readable with read
/// access); `rules` the answer of `GET repos/{r}/rules/branches/{base}` (rulesets), or its HTTP
/// error. GitHub answers 403 "Upgrade to GitHub Pro" when the plan of a private repo has no
/// rulesets: then no ruleset applies. `Err`: GitHub waits for a check, or the answers cannot
/// be read.
pub fn check_no_required_checks(
    base: &str,
    branch: &Value,
    rules: Result<&Value, (u16, String)>,
) -> Result<(), String> {
    let protected = branch
        .get("protected")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("the answer for branch {base} has no `protected`"))?;
    if protected {
        let rsc = branch
            .pointer("/protection/required_status_checks")
            .ok_or_else(|| {
                format!("branch {base} is protected and its required status checks cannot be read")
            })?;
        let level = rsc
            .get("enforcement_level")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let mut names: Vec<String> = Vec::new();
        for key in ["contexts", "checks"] {
            for c in rsc.get(key).and_then(Value::as_array).into_iter().flatten() {
                let n = c
                    .as_str()
                    .or_else(|| c.get("context").and_then(Value::as_str))
                    .unwrap_or("?")
                    .to_string();
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
        if level != "off" && !names.is_empty() {
            return Err(format!(
                "branch {base} requires the status checks {}; GitHub waits for them, so a PR without a run cannot merge",
                names.join(", ")
            ));
        }
    }
    match rules {
        Ok(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| format!("the rules answer for branch {base} is not a list"))?;
            const NEED_A_CHECK: [&str; 4] = [
                "required_status_checks",
                "workflows",
                "required_deployments",
                "code_scanning",
            ];
            if let Some(r) = arr.iter().find_map(|r| {
                r.get("type")
                    .and_then(Value::as_str)
                    .filter(|t| NEED_A_CHECK.contains(t))
            }) {
                return Err(format!(
                    "a ruleset of branch {base} has the rule {r}; GitHub waits for it, so a PR without a run cannot merge"
                ));
            }
            Ok(())
        }
        Err((403, m)) if m.contains("Upgrade to GitHub Pro") => Ok(()),
        Err((s, m)) => Err(format!(
            "the rules of branch {base} cannot be read (HTTP {s}: {m})"
        )),
    }
}

/// Is this evidence `ci` value one without a CI run (`none`, or `none (all files
/// paths-ignored: ...)`)? Such a ticket is not CI only for a headless merge
/// (spec/agents#headless-coordinator).
pub fn evidence_has_no_run(ci: &str) -> bool {
    ci == "none" || ci.starts_with("none (")
}

/// The verdict for one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerVerdict {
    /// At least one workflow runs for a crew branch push or a PR to the base branch.
    Covered,
    /// No workflow does. The text says what was missing.
    Missing(String),
    /// Some files could not be read and none of the others covers the case.
    Unsure(String),
}

/// Judge the workflow files of a repo. `files` is `(name, text)`. `prefix` is the branch
/// prefix with the slash (`crew/`), `base` the base branch of the PRs.
pub fn judge(files: &[(String, String)], prefix: &str, base: &str) -> TriggerVerdict {
    // a branch name of the agents: <prefix><agent>/<n>-<slug>
    let sample = format!("{prefix}agent/1-sample");
    let mut unknown: Vec<&str> = Vec::new();
    for (name, text) in files {
        match parse(text) {
            Parsed::Triggers(t) => {
                if t.push.as_ref().is_some_and(|f| f.runs_for(&sample))
                    || t.pull_request.as_ref().is_some_and(|f| f.runs_for(base))
                {
                    return TriggerVerdict::Covered;
                }
            }
            Parsed::Unknown => unknown.push(name),
        }
    }
    if !unknown.is_empty() {
        return TriggerVerdict::Unsure(format!(
            "could not read the triggers of {}",
            unknown.join(", ")
        ));
    }
    if files.is_empty() {
        return TriggerVerdict::Missing("the repo has no workflow in .github/workflows".into());
    }
    TriggerVerdict::Missing(format!(
        "no workflow triggers on a push to {prefix}** or on a pull_request to {base}"
    ))
}

/// The warning text for a verdict (`None`: covered).
pub fn warning(repo: &str, v: &TriggerVerdict, prefix: &str) -> Option<String> {
    match v {
        TriggerVerdict::Covered => None,
        TriggerVerdict::Missing(why) => Some(format!(
            "warning: CI of {repo} does not run for crew branches: {why}. `done` needs a CI run for the PR head, so no crew PR can reach Review. Add `pull_request` (or `push` with `branches: ['{prefix}**']`) to a workflow\n"
        )),
        TriggerVerdict::Unsure(why) => Some(format!(
            "warning: cannot confirm that CI of {repo} runs for crew branches ({why}). Check the triggers for {prefix}** and pull_request by hand\n"
        )),
    }
}

/// The crewboard files whose change alone must not start CI
/// (spec/ci#ci-and-crewboard-only-commits).
pub const CREW_FILES: [&str; 2] = [".crewboard.toml", "AUTHORITY.md"];

/// TriggerVerdict of the `paths-ignore` check for one repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsVerdict {
    /// No workflow on push to the base branch runs for a change of only the crewboard files.
    Quiet,
    /// These workflows run for it: `(workflow, crewboard files that start it)`.
    Runs(Vec<(String, Vec<String>)>),
    /// A workflow could not be read and no readable one gives a sure warning.
    Unsure(String),
}

/// Judge the `paths` / `paths-ignore` filters of the push trigger on the base branch.
pub fn judge_paths(files: &[(String, String)], base: &str) -> PathsVerdict {
    let mut runs = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    for (name, text) in files {
        match parse(text) {
            Parsed::Triggers(t) => {
                let Some(f) = t.push.as_ref().filter(|f| f.runs_for(base)) else {
                    continue;
                };
                let started: Vec<String> = CREW_FILES
                    .iter()
                    .filter(|c| f.runs_for_file(c))
                    .map(|c| c.to_string())
                    .collect();
                if !started.is_empty() {
                    runs.push((name.clone(), started));
                }
            }
            Parsed::Unknown => unknown.push(name),
        }
    }
    if !runs.is_empty() {
        PathsVerdict::Runs(runs)
    } else if !unknown.is_empty() {
        PathsVerdict::Unsure(format!(
            "could not read the triggers of {}",
            unknown.join(", ")
        ))
    } else {
        PathsVerdict::Quiet
    }
}

/// The warning text for a `paths` verdict (`None`: quiet).
pub fn paths_warning(repo: &str, v: &PathsVerdict) -> Option<String> {
    match v {
        PathsVerdict::Quiet => None,
        PathsVerdict::Runs(r) => {
            let list: Vec<String> = r
                .iter()
                .map(|(w, f)| format!("{w} (runs for {})", f.join(", ")))
                .collect();
            Some(format!(
                "warning: CI of {repo} runs on a push to the base branch when only crewboard files change: {}. Every merge of `.crewboard.toml` or `AUTHORITY.md` then starts a full run. The owner adds them to `paths-ignore` of the push trigger, next to the docs paths (`on: push: paths-ignore: ['.crewboard.toml', 'AUTHORITY.md']`). The workflow change is the owner's; crewboard never edits workflows\n",
                list.join("; ")
            ))
        }
        PathsVerdict::Unsure(why) => Some(format!(
            "warning: cannot confirm that CI of {repo} ignores crewboard-only changes ({why}). Check `paths-ignore` for .crewboard.toml and AUTHORITY.md by hand\n"
        )),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn covered(yaml: &str) -> bool {
        judge(&[("ci.yml".into(), yaml.into())], "crew/", "main") == TriggerVerdict::Covered
    }

    fn missing(yaml: &str) -> bool {
        matches!(
            judge(&[("ci.yml".into(), yaml.into())], "crew/", "main"),
            TriggerVerdict::Missing(_)
        )
    }

    #[test]
    fn a_flow_list_of_events_covers() {
        assert!(covered("name: ci\non: [push, pull_request]\njobs: {}\n"));
        assert!(covered("on: [workflow_dispatch, pull_request]\n"));
        // control: events that do not run for branches do not cover
        assert!(missing("on: [workflow_dispatch, schedule]\n"));
    }

    #[test]
    fn a_scalar_event_covers_or_not() {
        assert!(covered("on: pull_request\n"));
        assert!(covered("on: push\n"));
        assert!(missing("on: workflow_dispatch\n"));
        assert!(covered("'on': pull_request\n"));
    }

    #[test]
    fn inline_maps_are_read() {
        assert!(covered("on: {push: {branches: [main, 'crew/**']}}\n"));
        assert!(missing(
            "on: {push: {branches: [main]}, workflow_dispatch: {}}\n"
        ));
        assert!(covered("on: {pull_request: {branches: [main]}}\n"));
        assert!(missing("on:\n  push: {branches: [main]}\n"));
        assert!(covered("on:\n  push: {branches: ['crew/**']}\n"));
    }

    #[test]
    fn a_block_list_of_events() {
        assert!(covered("on:\n  - push\n  - workflow_dispatch\n"));
        assert!(missing("on:\n  - workflow_dispatch\n  - schedule\n"));
    }

    #[test]
    fn push_with_a_branches_filter_matches_the_prefix_glob_or_not() {
        // a common form: push to main and feat/** only, no pull_request
        assert!(missing("on:\n  push:\n    branches: [main, 'feat/**']\n"));
        assert!(covered("on:\n  push:\n    branches: [main, 'crew/**']\n"));
        // a block list of branches
        assert!(covered(
            "on:\n  push:\n    branches:\n      - main\n      - 'crew/**'\n"
        ));
        assert!(missing(
            "on:\n  push:\n    branches:\n      - main\n      - 'feat/**'\n"
        ));
        // `*` does not cross a slash: crew/* does not match crew/agent/1-sample
        assert!(missing("on:\n  push:\n    branches: ['crew/*']\n"));
        assert!(covered("on:\n  push:\n    branches: ['**']\n"));
    }

    #[test]
    fn push_without_a_filter_covers_and_with_only_tags_does_not() {
        assert!(covered("on:\n  push:\n  workflow_dispatch:\n"));
        assert!(covered("on:\n  push: {}\n"));
        assert!(missing("on:\n  push:\n    tags: ['v*']\n"));
    }

    #[test]
    fn branches_ignore_and_negated_patterns() {
        assert!(missing("on:\n  push:\n    branches-ignore: ['crew/**']\n"));
        assert!(covered("on:\n  push:\n    branches-ignore: ['feat/**']\n"));
        assert!(missing("on:\n  push:\n    branches: ['**', '!crew/**']\n"));
    }

    #[test]
    fn pull_request_counts_only_for_the_base_branch() {
        assert!(covered("on:\n  pull_request:\n    branches: [main]\n"));
        assert!(missing("on:\n  pull_request:\n    branches: [develop]\n"));
        assert!(covered("on:\n  pull_request:\n    types: [opened]\n"));
        assert!(covered(
            "on:\n  pull_request:\n    branches:\n      - main\n"
        ));
    }

    #[test]
    fn comments_and_other_keys_do_not_confuse_the_reader() {
        let y = "# the CI\nname: ci   # name\non:   # triggers\n  push:\n    branches: [main] # only main\n  pull_request:   # and PRs\njobs:\n  build:\n    on: nothing\n";
        assert!(covered(y));
        let y2 = "name: ci\non:\n  push:\n    branches: [main]\njobs:\n  build:\n    steps:\n      - uses: x\n";
        assert!(missing(y2));
    }

    #[test]
    fn an_unreadable_form_is_unsure_and_no_workflows_is_missing() {
        // an inline map is not read: say so instead of claiming "no trigger"
        // an unbalanced flow map cannot be read
        let v = judge(
            &[("a.yml".into(), "on: {push: {branches: [main]\n".into())],
            "crew/",
            "main",
        );
        assert!(matches!(v, TriggerVerdict::Unsure(_)), "{v:?}");
        let v = judge(&[("a.yml".into(), "name: x\n".into())], "crew/", "main");
        assert!(matches!(v, TriggerVerdict::Unsure(_)), "{v:?}");
        assert!(matches!(
            judge(&[], "crew/", "main"),
            TriggerVerdict::Missing(_)
        ));
        // one covering file wins over an unreadable one
        let v = judge(
            &[
                ("a.yml".into(), "name: x\n".into()),
                ("b.yml".into(), "on: pull_request\n".into()),
            ],
            "crew/",
            "main",
        );
        assert_eq!(v, TriggerVerdict::Covered);
    }

    #[test]
    fn the_warning_names_the_repo_and_the_prefix_and_covered_is_silent() {
        let m = warning(
            "o/r",
            &TriggerVerdict::Missing("no workflow".into()),
            "crew/",
        )
        .unwrap();
        assert!(m.contains("o/r") && m.contains("crew/**"), "{m}");
        assert!(warning("o/r", &TriggerVerdict::Covered, "crew/").is_none());
    }

    fn pj(yaml: &str) -> PathsVerdict {
        judge_paths(&[("ci.yml".into(), yaml.into())], "main")
    }

    fn runs_for(yaml: &str) -> Vec<String> {
        match pj(yaml) {
            PathsVerdict::Runs(r) => r[0].1.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn paths_push_without_paths_ignore_warns_for_both_files() {
        // refusal direction
        assert_eq!(
            runs_for("on:\n  push:\n    branches: [main]\n"),
            vec![".crewboard.toml", "AUTHORITY.md"]
        );
        assert_eq!(
            runs_for("on: push\n"),
            vec![".crewboard.toml", "AUTHORITY.md"]
        );
        assert_eq!(
            runs_for("on:\n  push:\n    paths-ignore: ['docs/**']\n"),
            vec![".crewboard.toml", "AUTHORITY.md"]
        );
        let w = paths_warning("o/r", &pj("on: push\n")).unwrap();
        assert!(
            w.contains("o/r") && w.contains("ci.yml") && w.contains("paths-ignore"),
            "{w}"
        );
    }

    #[test]
    fn paths_ignored_by_exact_name_or_glob_is_quiet() {
        // pass direction
        let exact =
            "on:\n  push:\n    paths-ignore:\n      - '.crewboard.toml'\n      - AUTHORITY.md\n";
        assert_eq!(pj(exact), PathsVerdict::Quiet);
        assert_eq!(
            pj("on:\n  push:\n    paths-ignore: ['*.toml', '**/*.md']\n"),
            PathsVerdict::Quiet
        );
        assert_eq!(
            pj("on: {push: {paths-ignore: ['.crewboard.toml', 'AUTHORITY.md']}}\n"),
            PathsVerdict::Quiet
        );
        assert!(paths_warning("o/r", &PathsVerdict::Quiet).is_none());
    }

    #[test]
    fn paths_partial_cover_names_the_missing_file() {
        // refusal direction
        assert_eq!(
            runs_for("on:\n  push:\n    paths-ignore: ['*.toml']\n"),
            vec!["AUTHORITY.md"]
        );
        assert_eq!(
            runs_for("on:\n  push:\n    paths-ignore: ['AUTHORITY.md']\n"),
            vec![".crewboard.toml"]
        );
        let w = paths_warning("o/r", &pj("on:\n  push:\n    paths-ignore: ['*.toml']\n")).unwrap();
        assert!(
            w.contains("runs for AUTHORITY.md") && !w.contains("runs for .crewboard"),
            "{w}"
        );
    }

    #[test]
    fn paths_allow_list_without_the_files_is_quiet() {
        // pass direction
        assert_eq!(
            pj("on:\n  push:\n    paths: ['src/**', 'Cargo.toml']\n"),
            PathsVerdict::Quiet
        );
        // control: an allow-list that includes one of them warns for that one
        assert_eq!(
            runs_for("on:\n  push:\n    paths: ['src/**', '*.toml']\n"),
            vec![".crewboard.toml"]
        );
        // a negated pattern removes a file again
        assert_eq!(
            pj("on:\n  push:\n    paths: ['*', '!.crewboard.toml', '!AUTHORITY.md']\n"),
            PathsVerdict::Quiet
        );
    }

    #[test]
    fn paths_workflow_that_does_not_push_to_base_is_quiet() {
        // pass direction: pull_request only, other branches only, tags only
        assert_eq!(pj("on: pull_request\n"), PathsVerdict::Quiet);
        assert_eq!(
            pj("on:\n  push:\n    branches: ['feat/**']\n"),
            PathsVerdict::Quiet
        );
        assert_eq!(pj("on:\n  push:\n    tags: ['v*']\n"), PathsVerdict::Quiet);
        assert_eq!(pj("on: workflow_dispatch\n"), PathsVerdict::Quiet);
        // control: the same file with the base branch warns
        assert!(matches!(
            pj("on:\n  push:\n    branches: [main]\n"),
            PathsVerdict::Runs(_)
        ));
    }

    #[test]
    fn paths_unreadable_form_is_cannot_confirm_and_a_sure_warning_wins() {
        assert!(matches!(pj("name: x\n"), PathsVerdict::Unsure(_)));
        assert!(matches!(
            pj("on: {push: {branches: [main]\n"),
            PathsVerdict::Unsure(_)
        ));
        let w = paths_warning("o/r", &pj("name: x\n")).unwrap();
        assert!(w.contains("cannot confirm"), "{w}");
        // a sure warning from a readable file is not hidden by an unreadable one
        let v = judge_paths(
            &[
                ("a.yml".into(), "name: x\n".into()),
                ("b.yml".into(), "on: push\n".into()),
            ],
            "main",
        );
        assert!(matches!(v, PathsVerdict::Runs(_)), "{v:?}");
        // all files ignored by a readable one and no unreadable: quiet
        assert_eq!(judge_paths(&[], "main"), PathsVerdict::Quiet);
    }

    // ---- files that every workflow ignores (spec/ci#files-that-every-workflow-ignores)

    fn path_sel(list: &[&str], file: &str) -> bool {
        let l: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        list_selects_with(&l, file, path_regex)
    }

    fn branch_sel(list: &[&str], b: &str) -> bool {
        let l: Vec<String> = list.iter().map(|s| s.to_string()).collect();
        list_selects(&l, b)
    }

    /// The path rows of GitHub's "Filter pattern cheat sheet" (workflow syntax page), both
    /// directions per row.
    #[test]
    fn globs_paths_follow_the_github_cheat_sheet() {
        let rows: &[(&[&str], &[&str], &[&str])] = &[
            (&["*"], &["README.md", "server.rb"], &["docs/README.md"]),
            (
                &["*.jsx?"],
                &["page.js", "page.jsx"],
                &["page.jsxx", "a/page.js"],
            ),
            (&["**"], &["all/the/files.md", "x"], &[]),
            (&["*.js"], &["app.js", "index.js"], &["js/index.js"]),
            (
                &["**.js"],
                &["index.js", "js/index.js", "src/js/app.js"],
                &["app.jsx"],
            ),
            (
                &["docs/*"],
                &["docs/README.md", "docs/file.txt"],
                &["docs/a/b.md"],
            ),
            (
                &["docs/**"],
                &["docs/README.md", "docs/mona/octocat.txt"],
                &["README.md"],
            ),
            (
                &["docs/**/*.md"],
                &[
                    "docs/README.md",
                    "docs/mona/hello-world.md",
                    "docs/a/markdown/file.md",
                ],
                &["docs/a/file.txt"],
            ),
            (
                &["**/docs/**"],
                &[
                    "docs/hello.md",
                    "dir/docs/my-file.txt",
                    "space/docs/plan/space.doc",
                ],
                &["dir/doc/x.md"],
            ),
            (
                &["**/README.md"],
                &["README.md", "js/README.md"],
                &["README.txt"],
            ),
            (
                &["**/*src/**"],
                &["a/src/app.js", "my-src/code/js/app.js"],
                &["src.js"],
            ),
            (
                &["**/*-post.md"],
                &["my-post.md", "path/their-post.md"],
                &["post.md"],
            ),
            (
                &["**/migrate-*.sql"],
                &[
                    "migrate-10909.sql",
                    "db/migrate-v1.0.sql",
                    "db/sept/migrate-v1.sql",
                ],
                &["db/migrate.sql"],
            ),
            (
                &["*.md", "!README.md"],
                &["hello.md"],
                &["README.md", "docs/hello.md"],
            ),
            (
                &["*.md", "!README.md", "README*"],
                &["hello.md", "README.md", "README.doc"],
                &["docs/hello.md"],
            ),
        ];
        for (pats, yes, no) in rows {
            for f in *yes {
                assert!(path_sel(pats, f), "{pats:?} must match {f}");
            }
            for f in *no {
                assert!(!path_sel(pats, f), "{pats:?} must not match {f}");
            }
        }
    }

    /// The branch rows of the cheat sheet, and `\` that makes a special character plain.
    #[test]
    fn globs_branches_follow_the_github_cheat_sheet() {
        let rows: &[(&[&str], &[&str], &[&str])] = &[
            (
                &["feature/*"],
                &["feature/my-branch", "feature/your-branch"],
                &["feature/a/b"],
            ),
            (
                &["feature/**"],
                &[
                    "feature/beta-a/my-branch",
                    "feature/your-branch",
                    "feature/mona/the/octocat",
                ],
                &["feat/x"],
            ),
            (
                &["main", "releases/mona-the-octocat"],
                &["main", "releases/mona-the-octocat"],
                &["mainx"],
            ),
            (&["*"], &["main", "releases"], &["releases/x"]),
            (&["**"], &["all/the/branches", "every/tag"], &[]),
            (&["v2*"], &["v2", "v2.0", "v2.9"], &["v1"]),
            (
                &["v[12].[0-9]+.[0-9]+"],
                &["v1.10.1", "v2.0.0"],
                &["v3.0.0", "v1.x.1"],
            ),
            (
                &["releases/**", "!releases/**-alpha"],
                &["releases/1.0"],
                &["releases/1.0-alpha"],
            ),
            (&["a\\*b"], &["a*b"], &["axb"]),
        ];
        for (pats, yes, no) in rows {
            for b in *yes {
                assert!(branch_sel(pats, b), "{pats:?} must match {b}");
            }
            for b in *no {
                assert!(!branch_sel(pats, b), "{pats:?} must not match {b}");
            }
        }
    }

    fn ig(yaml: &str, files: &[&str]) -> Result<Vec<String>, String> {
        let changed: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        judge_ignored(
            &[("ci.yml".into(), yaml.into())],
            &changed,
            "crew/agent/1-docs",
            "main",
        )
    }

    const CREW_PUSH: &str =
        "on:\n  push:\n    branches: [main, 'crew/**']\n    paths-ignore: ['docs/**', README.md]\n";

    #[test]
    fn ignored_files_pass_and_name_the_patterns() {
        // pass direction
        assert_eq!(
            ig(CREW_PUSH, &["docs/a.md", "README.md", "docs/b/c.md"]),
            Ok(vec!["docs/**".to_string(), "README.md".to_string()])
        );
        // an allow-list that does not select the file
        assert_eq!(
            ig(
                "on:\n  pull_request:\n    paths: ['src/**']\n",
                &["docs/a.md"]
            ),
            Ok(vec!["outside paths of ci.yml".to_string()])
        );
        // a push filter for other branches does not run; the pull_request one ignores docs
        assert_eq!(
            ig(
                "on:\n  push:\n    branches: [main]\n  pull_request:\n    paths-ignore:\n      - 'docs/**'\n",
                &["docs/a.md"]
            ),
            Ok(vec!["docs/**".to_string()])
        );
        // events that are information only do not matter
        assert!(
            ig(
                &format!("{CREW_PUSH}  workflow_dispatch:\n  schedule:\n    - cron: '0 4 * * *'\n"),
                &["docs/a.md"]
            )
            .is_ok()
        );
    }

    #[test]
    fn a_file_that_runs_a_workflow_is_named_with_the_workflow() {
        // refusal direction
        let e = ig(CREW_PUSH, &["docs/a.md", "src/lib.rs"]).unwrap_err();
        assert_eq!(
            e,
            "file src/lib.rs is not ignored by workflow ci.yml (push does not ignore it)"
        );
        let e = ig("on: pull_request\n", &["docs/a.md"]).unwrap_err();
        assert!(e.contains("pull_request has no paths filter"), "{e}");
        // a `!` pattern in paths-ignore takes a file back
        let e = ig(
            "on:\n  push:\n    paths-ignore: ['docs/**', '!docs/api/**']\n",
            &["docs/api/x.md"],
        )
        .unwrap_err();
        assert!(e.contains("docs/api/x.md"), "{e}");
        // the allow-list selects the file
        assert!(ig("on:\n  push:\n    paths: ['docs/**']\n", &["docs/a.md"]).is_err());
        // pull_request_target is judged like pull_request
        assert!(ig("on: pull_request_target\n", &["docs/a.md"]).is_err());
    }

    #[test]
    fn any_doubt_refuses() {
        // an unreadable form, also a flow list over two lines
        for yaml in [
            "name: x\n",
            "on:\n  push:\n    paths-ignore: ['docs/**',\n      'README.md']\n",
            "on:\n  push:\n    paths-ignore: {a: b}\n",
            "on:\n  push\n  pull_request\n",
        ] {
            let e = ig(yaml, &["docs/a.md"]).unwrap_err();
            assert!(e.contains("cannot be read"), "{yaml:?}: {e}");
        }
        // an unknown event
        let e = ig(
            &format!("{CREW_PUSH}  pull_request_comment:\n"),
            &["docs/a.md"],
        )
        .unwrap_err();
        assert!(e.contains("'pull_request_comment'"), "{e}");
        // both filters of one kind, a pattern without a regex
        let e = ig(
            "on:\n  push:\n    paths: [a]\n    paths-ignore: [b]\n",
            &["b"],
        )
        .unwrap_err();
        assert!(e.contains("both paths and paths-ignore"), "{e}");
        let e = ig("on:\n  push:\n    paths: ['[x']\n", &["docs/a.md"]).unwrap_err();
        assert!(e.contains("the pattern '[x' cannot be read"), "{e}");
        // no workflow runs at all: missing CI, not skipped
        let e = ig("on:\n  push:\n    branches: [main]\n", &["docs/a.md"]).unwrap_err();
        assert!(e.contains("no workflow runs"), "{e}");
        assert!(judge_ignored(&[], &["a".into()], "crew/x", "main").is_err());
        assert!(ig(CREW_PUSH, &[]).is_err());
        // control: the same file passes
        assert!(ig(CREW_PUSH, &["docs/a.md"]).is_ok());
    }

    #[test]
    fn the_strict_filter_reader_keeps_the_init_warning_unsure() {
        // the open flow list is "cannot confirm", never a sure "no trigger"
        let v = judge(
            &[(
                "a.yml".into(),
                "on:\n  push:\n    branches: [main,\n      'crew/**']\n".into(),
            )],
            "crew/",
            "main",
        );
        assert!(matches!(v, TriggerVerdict::Unsure(_)), "{v:?}");
        // a colon in an unquoted input description of another event is fine
        assert!(covered(
            "on:\n  workflow_dispatch:\n    inputs:\n      x:\n        description: Run: now\n  pull_request:\n"
        ));
    }

    #[test]
    fn required_checks_refuse_and_none_pass() {
        let off = serde_json::json!({"protected": false, "protection": {"enabled": false,
            "required_status_checks": {"enforcement_level": "off", "contexts": [], "checks": []}}});
        let empty = serde_json::json!([]);
        // pass direction
        assert_eq!(check_no_required_checks("main", &off, Ok(&empty)), Ok(()));
        let upgrade =
            "Upgrade to GitHub Pro or make this repository public to enable this feature.";
        assert_eq!(
            check_no_required_checks("main", &off, Err((403, upgrade.into()))),
            Ok(())
        );
        let other_rules = serde_json::json!([{"type": "pull_request"}, {"type": "deletion"}]);
        assert_eq!(
            check_no_required_checks("main", &off, Ok(&other_rules)),
            Ok(())
        );
        // refusal direction
        let req = serde_json::json!({"protected": true, "protection": {"enabled": true,
            "required_status_checks": {"enforcement_level": "everyone", "contexts": ["ci"], "checks": []}}});
        let e = check_no_required_checks("main", &req, Ok(&empty)).unwrap_err();
        assert!(e.contains("requires the status checks ci"), "{e}");
        let hidden = serde_json::json!({"protected": true});
        assert!(check_no_required_checks("main", &hidden, Ok(&empty)).is_err());
        assert!(check_no_required_checks("main", &serde_json::json!({}), Ok(&empty)).is_err());
        for t in [
            "required_status_checks",
            "workflows",
            "required_deployments",
            "code_scanning",
        ] {
            let r = serde_json::json!([{"type": t}]);
            let e = check_no_required_checks("main", &off, Ok(&r)).unwrap_err();
            assert!(e.contains(t), "{e}");
        }
        assert!(check_no_required_checks("main", &off, Err((404, "Not Found".into()))).is_err());
        assert!(check_no_required_checks("main", &off, Err((403, "Forbidden".into()))).is_err());
    }

    #[test]
    fn evidence_without_a_run_is_told_apart() {
        assert!(evidence_has_no_run("none"));
        assert!(evidence_has_no_run(
            "none (all files paths-ignored: docs/**)"
        ));
        assert!(!evidence_has_no_run(
            "https://github.com/o/r/actions/runs/1"
        ));
        assert!(!evidence_has_no_run("nonesuch"));
    }
}
