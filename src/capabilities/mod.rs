//! Agent profiles and capability matching (spec/profiles). Everything here is pure.
//!
//! The `Capabilities` field of an agent item is one text line of space-separated tokens:
//!
//! ```text
//! e2e x86-builds tier:high arch:x86_64 os:macos-15 cpu:10 ram:32g tool:cargo tool:git
//! ```
//!
//! - a token without `:` is a **declared tag** (`[a-z0-9-]`, set by the agent or the owner);
//! - `tier:<name>` is the declared model/cost tier;
//! - `arch:`, `os:`, `cpu:`, `ram:`, `tool:` and `tools-cut:` are **facts**, detected at
//!   `join` and every `start` (`facts.rs`) and replaced there, so a hand edit of a fact does
//!   not outlive the next `start`.
//!
//! The field holds at most 1024 bytes. Declared tags, the tier, `arch:` and `os:` always stay;
//! `cpu:` and `ram:` come next; `tool:` tokens are cut from the end, and the cut is marked
//! with `tools-cut:<n>` so a missing tool tag is never mistaken for a missing tool.

use crate::base::ids;
use crate::rules::limits::MAX_TEXT_FIELD_BYTES;
use crate::rules::{Refusal, Verdict};
use std::collections::BTreeSet;

pub(crate) mod facts;

pub const MAX_TAG_LEN: usize = 32;
pub const MAX_DECLARED_TAGS: usize = 24;
/// Prefixes of the tokens that crewboard detects. `tier:` is not here: it is declared.
pub const FACT_KEYS: &[&str] = &["arch", "os", "cpu", "ram", "tool", "toolchain"];
pub const CUT_KEY: &str = "tools-cut";
const TIER_KEY: &str = "tier";

/// Declared tags: `[a-z0-9-]`, 1 to 32 characters, no hyphen at the start or the end.
pub fn validate_tag(s: &str) -> Result<String, Refusal> {
    let ok = !s.is_empty()
        && s.len() <= MAX_TAG_LEN
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
        && !s.ends_with('-');
    if ok {
        Ok(s.to_string())
    } else {
        Err(Refusal::new(
            ids::PROFILE_TAG_INVALID,
            format!(
                "'{s}' is not a tag: use 1-{MAX_TAG_LEN} characters of a-z, 0-9 and '-', no '-' at the start or end (for example x86-builds)"
            ),
        ))
    }
}

/// A tier is written like a tag (`low`, `mid`, `high`, `frontier-2`).
pub fn validate_tier(s: &str) -> Result<String, Refusal> {
    validate_tag(s).map_err(|_| {
        Refusal::new(
            ids::PROFILE_TIER_INVALID,
            format!("'{s}' is not a tier: write it like a tag, for example low, mid or high"),
        )
    })
}

/// Cost order of a tier, cheaper first: `low` < `mid` < `high`. Any other name sorts after
/// these, by name; no tier sorts last.
pub fn tier_rank(tier: Option<&str>) -> (u8, String) {
    match tier {
        Some("low") => (0, String::new()),
        Some("mid") => (1, String::new()),
        Some("high") => (2, String::new()),
        Some(other) => (3, other.to_string()),
        None => (4, String::new()),
    }
}

/// The parsed `Capabilities` field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Caps {
    pub declared: BTreeSet<String>,
    pub tier: Option<String>,
    /// Fact tokens in the order they were written (`arch:x86_64`, `tool:cargo`, ...),
    /// without the cut marker.
    pub facts: Vec<String>,
    /// How many tools were cut for space (`tools-cut:<n>`).
    pub tools_cut: usize,
    /// Tokens of the field that are neither valid tags nor known facts. They are dropped at the
    /// next write; `agent profile show` reports them.
    pub unknown: Vec<String>,
}

fn is_fact_token(t: &str) -> bool {
    t.split_once(':')
        .is_some_and(|(k, v)| FACT_KEYS.contains(&k) && !v.is_empty())
}

impl Caps {
    pub fn parse(text: &str) -> Caps {
        let mut c = Caps::default();
        for t in text.split_whitespace() {
            if let Some(v) = t.strip_prefix("tier:") {
                match validate_tier(v) {
                    Ok(v) => c.tier = Some(v),
                    Err(_) => c.unknown.push(t.to_string()),
                }
            } else if let Some(n) = t.strip_prefix("tools-cut:") {
                match n.parse::<usize>() {
                    Ok(n) => c.tools_cut = n,
                    Err(_) => c.unknown.push(t.to_string()),
                }
            } else if is_fact_token(t) {
                c.facts.push(t.to_string());
            } else if t.contains(':') {
                c.unknown.push(t.to_string());
            } else {
                match validate_tag(t) {
                    Ok(t) => {
                        c.declared.insert(t);
                    }
                    Err(_) => c.unknown.push(t.to_string()),
                }
            }
        }
        c
    }

    /// Replace the facts with freshly detected ones (a fact is never carried over).
    pub fn with_facts(mut self, tokens: Vec<String>) -> Caps {
        self.facts = tokens;
        self.tools_cut = 0;
        self
    }

    fn fact_of(&self, key: &str) -> Vec<&String> {
        let p = format!("{key}:");
        self.facts.iter().filter(|t| t.starts_with(&p)).collect()
    }

    /// The text of the field, at most 1024 bytes. Declared tags, the tier, `arch:` and `os:`
    /// are required: if they alone do not fit, the write is refused. `cpu:`/`ram:` and other
    /// facts follow while they fit, then the tools, cut from the end with `tools-cut:<n>`.
    pub fn compose(&self) -> Result<String, Refusal> {
        if self.declared.len() > MAX_DECLARED_TAGS {
            return Err(Refusal::new(
                ids::CAPABILITIES_TOO_LONG,
                format!(
                    "{} declared tags; at most {MAX_DECLARED_TAGS} fit a profile",
                    self.declared.len()
                ),
            ));
        }
        let mut required: Vec<String> = self.declared.iter().cloned().collect();
        if let Some(t) = &self.tier {
            required.push(format!("{TIER_KEY}:{t}"));
        }
        for k in ["arch", "os"] {
            required.extend(self.fact_of(k).into_iter().cloned());
        }
        let mut out = required.join(" ");
        if out.len() > MAX_TEXT_FIELD_BYTES {
            return Err(Refusal::new(
                ids::CAPABILITIES_TOO_LONG,
                format!(
                    "declared tags, tier, arch and os need {} bytes; the field holds {MAX_TEXT_FIELD_BYTES}",
                    out.len()
                ),
            ));
        }
        let push = |out: &mut String, t: &str, reserve: usize| -> bool {
            let add = t.len() + usize::from(!out.is_empty());
            if out.len() + add + reserve <= MAX_TEXT_FIELD_BYTES {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(t);
                true
            } else {
                false
            }
        };
        for t in self
            .facts
            .iter()
            .filter(|t| !t.starts_with("arch:") && !t.starts_with("os:") && !t.starts_with("tool:"))
        {
            push(&mut out, t, 0);
        }
        let tools: Vec<&String> = self.fact_of("tool");
        // Room for `tools-cut:9999` is kept free until we know that nothing is cut.
        let marker_room = 1 + CUT_KEY.len() + 1 + 4;
        let total: usize = tools.iter().map(|t| t.len() + 1).sum();
        let all_fit = out.len() + total + (if self.tools_cut > 0 { marker_room } else { 0 })
            <= MAX_TEXT_FIELD_BYTES;
        let mut kept = 0;
        if all_fit {
            for t in &tools {
                push(&mut out, t, 0);
                kept += 1;
            }
        } else {
            for t in &tools {
                if !push(&mut out, t, marker_room) {
                    break;
                }
                kept += 1;
            }
        }
        let cut = self.tools_cut + (tools.len() - kept);
        if cut > 0 {
            let marker = format!("{CUT_KEY}:{cut}");
            if !push(&mut out, &marker, 0) {
                return Err(Refusal::new(
                    ids::CAPABILITIES_TOO_LONG,
                    "no room for the tools-cut marker",
                ));
            }
        }
        check_len(&out)?;
        Ok(out)
    }
}

fn check_len(s: &str) -> Verdict {
    if s.len() > MAX_TEXT_FIELD_BYTES {
        return Err(Refusal::new(
            ids::CAPABILITIES_TOO_LONG,
            format!("{} bytes; the field holds {MAX_TEXT_FIELD_BYTES}", s.len()),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Requirements of a ticket

/// Normalise one requirement of a ticket (`Needs:` line or `needs:` label). The result is
/// one of:
///
/// - `arch:x86_64` or `arch:arm64` (a bare `x86_64`, `amd64`, `arm64`, `aarch64` too);
/// - `os:<name>[-<major>]`, `tool:<name>`, `cpu:<n>`, `ram:<n>g`, `tier:<name>`;
/// - a declared tag.
pub fn parse_requirement(raw: &str) -> Result<String, Refusal> {
    let r = raw.trim().to_ascii_lowercase();
    let bad = |why: &str| {
        Refusal::new(
            ids::NEEDS_INVALID,
            format!("requirement '{raw}' is not valid: {why}"),
        )
    };
    if r.is_empty() {
        return Err(bad("empty"));
    }
    if let Some((k, v)) = r.split_once(':') {
        if v.is_empty() {
            return Err(bad("nothing after ':'"));
        }
        return match k {
            "arch" => Ok(format!(
                "arch:{}",
                crate::capabilities::facts::normalize_arch(v)
            )),
            "os" | "tool" | "cpu" | "ram" => {
                if v.chars().all(|c| {
                    c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || matches!(c, '-' | '_' | '.' | '+')
                }) {
                    Ok(format!("{k}:{v}"))
                } else {
                    Err(bad("a-z, 0-9, '-', '_', '.', '+' only after ':'"))
                }
            }
            "tier" => validate_tier(v)
                .map(|t| format!("tier:{t}"))
                .map_err(|e| bad(&e.message)),
            other => Err(bad(&format!(
                "unknown key '{other}:' (use arch, os, tool, cpu, ram or tier)"
            ))),
        };
    }
    // a bare architecture name
    if matches!(r.as_str(), "x86_64" | "amd64" | "x64" | "arm64" | "aarch64") {
        return Ok(format!(
            "arch:{}",
            crate::capabilities::facts::normalize_arch(&r)
        ));
    }
    validate_tag(&r)
        .map_err(|e| bad(&e.message))
        .map(|t| t.to_string())
}

/// Split the value of a `Needs:` line at commas and white space.
fn split_needs(v: &str) -> impl Iterator<Item = &str> {
    v.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
}

/// The requirements of a ticket: every `Needs:` line of the body (also `**Needs:**`, a list
/// item, any letter case) and every label `needs:<requirement>`. Sorted, without duplicates.
/// A requirement that does not parse is a refusal (rule `needs-invalid`): a requirement that
/// is dropped silently would let an agent without it take the ticket.
pub fn needs_of_ticket(body: &str, labels: &[String]) -> Result<Vec<String>, Refusal> {
    let mut out = BTreeSet::new();
    for line in crate::rules::text::body_fields(body, "Needs") {
        for tok in split_needs(&line) {
            out.insert(parse_requirement(tok)?);
        }
    }
    for l in labels {
        let low = l.to_ascii_lowercase();
        if let Some(rest) = low.strip_prefix("needs:") {
            out.insert(parse_requirement(rest)?);
        }
    }
    Ok(out.into_iter().collect())
}

/// Is the (normalised) requirement met by the agent?
///
/// - `arch:`, `cpu:`, `ram:`, `tool:`: the agent has exactly this fact token;
/// - `os:<name>`: the agent has `os:<name>` or `os:<name>-<more>` (so `os:macos` meets
///   `os:macos-15`, `os:linux` meets `os:linux-ubuntu-24`, and `os:macos-15` needs macOS 15);
/// - `tier:<t>`: the declared tier is exactly `<t>`;
/// - anything else is a declared tag, and the agent must have declared exactly this tag.
pub fn is_met(req: &str, caps: &Caps) -> bool {
    match req.split_once(':') {
        Some(("tier", t)) => caps.tier.as_deref() == Some(t),
        Some(("os", want)) => caps.facts.iter().any(|f| {
            f.strip_prefix("os:").is_some_and(|have| {
                have == want
                    || have
                        .strip_prefix(want)
                        .is_some_and(|rest| rest.starts_with('-'))
            })
        }),
        Some(_) => caps.facts.iter().any(|f| f == req),
        None => caps.declared.contains(req),
    }
}

/// The requirements that the agent does not meet, in the order of `needs`.
pub fn missing(needs: &[String], caps: &Caps) -> Vec<String> {
    needs.iter().filter(|n| !is_met(n, caps)).cloned().collect()
}

/// What an accepted override leaves behind: the missing tags and the reason, for the signed
/// assignment message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub missing: Vec<String>,
    pub reason: String,
}

/// An override needs a reason. Checked before anything is read, so the flag never works
/// "by accident": rule `capability-override-needs-reason`.
pub fn check_override_args(override_flag: bool, reason: Option<&str>) -> Verdict {
    if override_flag && reason.map(str::trim).is_none_or(str::is_empty) {
        return Err(Refusal::new(
            ids::CAPABILITY_OVERRIDE_NEEDS_REASON,
            "--override-capabilities needs --why \"...\": the reason is recorded in the signed assignment message",
        ));
    }
    Ok(())
}

/// The one rule of `assign`: the agent meets every requirement, or the coordinator overrides
/// with a reason.
///
/// - requirements met: `Ok(None)`;
/// - something missing, no override: refused, rule `capability-mismatch`, naming the tags;
/// - something missing, override with a non-empty reason: `Ok(Some(Override))`;
/// - an override without a reason is refused whatever the requirements are, rule
///   `capability-override-needs-reason` (checked first, so the flag never works "by accident").
pub fn check_assign(
    agent: &str,
    needs: &[String],
    caps: &Caps,
    override_flag: bool,
    reason: Option<&str>,
) -> Result<Option<Override>, Refusal> {
    check_override_args(override_flag, reason)?;
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    let miss = missing(needs, caps);
    if miss.is_empty() {
        return Ok(None);
    }
    match (override_flag, reason) {
        (true, Some(r)) => Ok(Some(Override {
            missing: miss,
            reason: r.to_string(),
        })),
        _ => {
            let cut = if caps.tools_cut > 0 && miss.iter().any(|m| m.starts_with("tool:")) {
                format!(
                    " (the tool list of {agent} was cut for space: tools-cut:{})",
                    caps.tools_cut
                )
            } else {
                String::new()
            };
            Err(Refusal::new(
                ids::CAPABILITY_MISMATCH,
                format!(
                    "{agent} lacks: {}{cut}. Pick an agent from `crewboard status --agents --can ...`, or override with --override-capabilities --why \"...\"",
                    miss.join(", ")
                ),
            ))
        }
    }
}

// ---------------------------------------------------------------------------------------
// Profile changes

/// What the user asked for in `agent profile set`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub tags: Vec<String>,
    pub untags: Vec<String>,
    pub tier: Option<String>,
    pub clear_tier: bool,
    /// New background text; `Some("")` clears it.
    pub about: Option<String>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.untags.is_empty()
            && self.tier.is_none()
            && !self.clear_tier
            && self.about.is_none()
    }

    /// Split `a,b c` into tags and validate each one.
    pub fn parse_tags(values: &[String]) -> Result<Vec<String>, Refusal> {
        let mut out = Vec::new();
        for v in values {
            for t in v.split(|c: char| c == ',' || c.is_whitespace()) {
                if t.is_empty() {
                    continue;
                }
                let t = validate_tag(t)?;
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        Ok(out)
    }
}

/// The declared part of a profile after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub caps: Caps,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub tier_changed: bool,
    pub about_changed: bool,
}

impl Applied {
    pub fn changed(&self) -> bool {
        !self.added.is_empty()
            || !self.removed.is_empty()
            || self.tier_changed
            || self.about_changed
    }
}

/// Apply a change to the stored profile. A tag in both `tags` and `untags` is a usage
/// error. Removing a tag that is not there changes nothing (it is not an error: the aim is met).
pub fn apply_change(old: &Caps, old_about: &str, c: &Change) -> Result<Applied, Refusal> {
    if let Some(t) = c.tags.iter().find(|t| c.untags.contains(t)) {
        return Err(Refusal::new(
            ids::PROFILE_TAG_CONFLICT,
            format!("'{t}' is in --tag and in --untag"),
        ));
    }
    if c.tier.is_some() && c.clear_tier {
        return Err(Refusal::new(
            ids::PROFILE_TAG_CONFLICT,
            "--tier and --clear-tier together",
        ));
    }
    let mut caps = old.clone();
    caps.unknown.clear();
    let mut added = Vec::new();
    let mut removed = Vec::new();
    for t in &c.tags {
        if caps.declared.insert(t.clone()) {
            added.push(t.clone());
        }
    }
    for t in &c.untags {
        if caps.declared.remove(t) {
            removed.push(t.clone());
        }
    }
    let mut tier_changed = false;
    if let Some(t) = &c.tier {
        let t = validate_tier(t)?;
        if caps.tier.as_deref() != Some(t.as_str()) {
            tier_changed = true;
        }
        caps.tier = Some(t);
    }
    if c.clear_tier && caps.tier.take().is_some() {
        tier_changed = true;
    }
    let about_changed = c
        .about
        .as_ref()
        .is_some_and(|a| a.trim() != old_about.trim());
    Ok(Applied {
        caps,
        added,
        removed,
        tier_changed,
        about_changed,
    })
}

// ---------------------------------------------------------------------------------------
// The agent item body (markdown)

/// Text of the agent item body, parsed back: tier, tags, background and the facts section.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Body {
    pub about: String,
    /// The text from `## Facts` to the end (without the heading), kept as it is.
    pub facts_md: String,
}

const PROFILE_H: &str = "## Profile";
const ABOUT_H: &str = "### Background";
const FACTS_H: &str = "## Facts";
const NO_ABOUT: &str = "(none yet: set it with `crewboard agent profile set --about-file <file>`)";
/// The placeholder that older versions wrote to the board: still read as "no background text".
const NO_ABOUT_OLD: &str = "(none yet: set it with `crewboard profile set --about-file <file>`)";

impl Body {
    pub fn parse(text: &str) -> Body {
        let after_about = text.split_once(&format!("\n{ABOUT_H}\n")).map(|x| x.1);
        let (about_part, facts_part) = match after_about {
            Some(rest) => match rest.split_once(&format!("\n{FACTS_H}\n")) {
                Some((a, f)) => (a, f),
                None => (rest, ""),
            },
            None => (
                "",
                text.split_once(&format!("\n{FACTS_H}\n"))
                    .map_or("", |x| x.1),
            ),
        };
        let about = about_part.trim();
        let about = if about == NO_ABOUT || about == NO_ABOUT_OLD {
            ""
        } else {
            about
        };
        Body {
            about: about.to_string(),
            facts_md: facts_part.trim_end().to_string(),
        }
    }

    /// The item body: the profile (tier, tags, background) and the detected facts.
    pub fn render(name: &str, caps: &Caps, about: &str, facts_md: &str) -> String {
        let tags = if caps.declared.is_empty() {
            "(none)".to_string()
        } else {
            caps.declared.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let about = if about.trim().is_empty() {
            NO_ABOUT
        } else {
            about.trim()
        };
        let facts = if facts_md.trim().is_empty() {
            "(not detected yet: `crewboard start` detects them)"
        } else {
            facts_md.trim_end()
        };
        format!(
            "crewboard agent item of {name}\n\n{PROFILE_H}\n\nTier: {}\nTags: {tags}\n\n{ABOUT_H}\n{about}\n\n{FACTS_H}\n{facts}\n",
            caps.tier.as_deref().unwrap_or("(none)")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(declared: &[&str], facts: &[&str]) -> Caps {
        Caps {
            declared: declared.iter().map(|s| (*s).to_string()).collect(),
            facts: facts.iter().map(|s| (*s).to_string()).collect(),
            ..Caps::default()
        }
    }

    fn rule<T: std::fmt::Debug>(r: Result<T, Refusal>) -> &'static str {
        r.expect_err("must be refused").rule.as_str()
    }

    // -- tag validation -------------------------------------------------------------

    #[test]
    fn valid_tags_pass_and_each_kind_of_bad_tag_is_refused() {
        for ok in [
            "e2e",
            "x86-builds",
            "gpu",
            "ui-review",
            "a",
            "0",
            &"a".repeat(32),
        ] {
            assert_eq!(validate_tag(ok).unwrap(), ok);
        }
        for bad in [
            "",
            "E2E",
            "x86_64",
            "arch:x86",
            "has space",
            "-lead",
            "trail-",
            "ä",
            "a.b",
            &"a".repeat(33),
        ] {
            assert_eq!(rule(validate_tag(bad)), "profile-tag-invalid", "{bad:?}");
        }
    }

    #[test]
    fn a_tier_is_written_like_a_tag_and_sorts_cheap_first() {
        assert!(validate_tier("high").is_ok());
        assert_eq!(rule(validate_tier("High")), "profile-tier-invalid");
        let mut v = [
            tier_rank(None),
            tier_rank(Some("high")),
            tier_rank(Some("zeta")),
            tier_rank(Some("low")),
            tier_rank(Some("alpha")),
            tier_rank(Some("mid")),
        ];
        v.sort();
        let names: Vec<_> = v.iter().map(|(r, n)| (*r, n.as_str())).collect();
        assert_eq!(
            names,
            [
                (0, ""),
                (1, ""),
                (2, ""),
                (3, "alpha"),
                (3, "zeta"),
                (4, "")
            ]
        );
    }

    // -- the field: parse and compose -------------------------------------------------

    #[test]
    fn the_field_round_trips_and_separates_declared_tags_from_facts() {
        let text =
            "e2e x86-builds tier:high arch:x86_64 os:macos-15 cpu:10 ram:32g tool:cargo tool:git";
        let c = Caps::parse(text);
        assert_eq!(
            c.declared.iter().cloned().collect::<Vec<_>>(),
            ["e2e", "x86-builds"]
        );
        assert_eq!(c.tier.as_deref(), Some("high"));
        assert_eq!(c.facts.len(), 6);
        assert!(c.unknown.is_empty());
        assert_eq!(c.compose().unwrap(), text);
        // control: a tag is never mistaken for a fact and a fact never for a tag
        assert!(!c.declared.contains("tool:cargo"));
    }

    #[test]
    fn garbage_tokens_are_reported_not_trusted_and_dropped_at_the_next_write() {
        let c = Caps::parse("e2e Bad_Tag foo:bar tier:HIGH tools-cut:x arch:arm64");
        assert_eq!(
            c.unknown,
            ["Bad_Tag", "foo:bar", "tier:HIGH", "tools-cut:x"]
        );
        assert_eq!(c.compose().unwrap(), "e2e arch:arm64");
    }

    #[test]
    fn facts_are_replaced_by_detection_and_declared_tags_survive() {
        let old = Caps::parse("e2e tier:low arch:x86_64 os:macos-14 tool:cargo tools-cut:3");
        let new = old.with_facts(vec!["arch:arm64".into(), "os:macos-15".into()]);
        assert_eq!(
            new.compose().unwrap(),
            "e2e tier:low arch:arm64 os:macos-15"
        );
        assert_eq!(
            new.tools_cut, 0,
            "a re-detection starts the cut count again"
        );
    }

    fn with_tools(last_len: usize) -> Caps {
        let mut facts = vec!["arch:arm64".to_string(), "os:macos-15".to_string()];
        // 82 tokens of 11 bytes (+1 separator each), then one of `5 + last_len` bytes
        for i in 0..82 {
            facts.push(format!("tool:t{i:05}"));
        }
        facts.push(format!("tool:{}", "z".repeat(last_len)));
        Caps {
            facts,
            ..Caps::default()
        }
    }

    #[test]
    fn a_field_of_exactly_1024_bytes_passes_and_one_byte_more_cuts_with_a_marker() {
        // 22 (arch, os) + 82 * 12 + (1 + 5 + 12) = 1024
        let exact = with_tools(12).compose().unwrap();
        assert_eq!(exact.len(), 1024);
        assert!(
            !exact.contains("tools-cut"),
            "nothing is cut at exactly 1024"
        );
        assert_eq!(exact.matches("tool:").count(), 83);
        // one byte more: the tools are cut from the end and the cut is counted
        let over = with_tools(13).compose().unwrap();
        assert!(over.len() <= 1024, "{}", over.len());
        assert!(over.starts_with("arch:arm64 os:macos-15 tool:t00000"));
        let n: usize = over.rsplit("tools-cut:").next().unwrap().parse().unwrap();
        assert!(n >= 1);
        assert_eq!(over.matches("tool:").count() + n, 83, "kept + cut = all");
        assert!(!over.contains("zzzz"), "the last tool is the one that goes");
        // re-parsing the cut field keeps the count, and composing it again is stable
        let again = Caps::parse(&over);
        assert_eq!(again.tools_cut, n);
        assert_eq!(again.compose().unwrap(), over);
    }

    #[test]
    fn the_cut_never_touches_declared_tags_tier_arch_or_os() {
        let declared: BTreeSet<String> = (0..24)
            .map(|i| format!("capability-tag-number-{i:02}"))
            .collect();
        let c = Caps {
            declared: declared.clone(),
            tier: Some("high".into()),
            facts: {
                let mut f = vec!["arch:x86_64".to_string(), "os:linux-ubuntu-24".to_string()];
                f.extend((0..40).map(|i| format!("tool:tool-number-{i:02}")));
                f
            },
            ..Caps::default()
        };
        let s = c.compose().unwrap();
        assert!(s.len() <= 1024, "{}", s.len());
        for t in &declared {
            assert!(s.split(' ').any(|x| x == t), "{t} lost");
        }
        assert!(
            s.contains("tier:high")
                && s.contains("arch:x86_64")
                && s.contains("os:linux-ubuntu-24")
        );
        assert!(s.contains("tools-cut:"), "the tools do not fit: {s}");
    }

    #[test]
    fn declared_tags_that_cannot_fit_are_refused_not_truncated() {
        let c = Caps {
            declared: (0..25).map(|i| format!("t{i}")).collect(),
            ..Caps::default()
        };
        assert_eq!(rule(c.compose()), "capabilities-too-long");
        // a field of exactly 1024 bytes of required tokens is still accepted
        let tag = "a".repeat(32);
        let mut declared = BTreeSet::new();
        // 24 distinct 32-byte tags = 24*32 + 23 = 791 bytes: fits
        for i in 0..24 {
            declared.insert(format!("{}{i:02}", &tag[..30]));
        }
        let ok = Caps {
            declared,
            facts: vec!["arch:arm64".into(), "os:macos-15".into()],
            ..Caps::default()
        };
        assert!(ok.compose().unwrap().len() <= 1024);
    }

    // -- Needs parsing -----------------------------------------------------------------

    fn labels(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn needs_come_from_the_body_line_in_every_spelling() {
        for body in [
            "Goal: x\nNeeds: x86_64, x86-builds\n",
            "**Needs:** x86_64 x86-builds",
            "- needs: AMD64, x86-builds",
            "> **Needs**: x86_64,x86-builds",
        ] {
            assert_eq!(
                needs_of_ticket(body, &[]).unwrap(),
                ["arch:x86_64", "x86-builds"],
                "{body}"
            );
        }
        // two lines are one union
        assert_eq!(
            needs_of_ticket("Needs: e2e\nGate: x\nNeeds: gpu, e2e", &[]).unwrap(),
            ["e2e", "gpu"]
        );
        // no line: no requirement (control: the parser can say "none")
        assert!(needs_of_ticket("Goal: x\nGate: y", &[]).unwrap().is_empty());
        assert!(needs_of_ticket("Needs:", &[]).unwrap().is_empty());
        // `Needs` inside a sentence is not a line
        assert!(
            needs_of_ticket("The ticket needs: nothing", &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn needs_come_from_labels_and_both_sources_are_merged() {
        let l = labels(&[
            "crew:work",
            "needs:gpu",
            "needs:x86_64",
            "needs:tool:docker",
            "bug",
        ]);
        assert_eq!(
            needs_of_ticket("", &l).unwrap(),
            ["arch:x86_64", "gpu", "tool:docker"]
        );
        assert_eq!(
            needs_of_ticket("Needs: e2e", &labels(&["needs:e2e", "needs:gpu"])).unwrap(),
            ["e2e", "gpu"],
            "a tag in both places counts once"
        );
        // a label that only looks like it is no requirement
        assert!(
            needs_of_ticket("", &labels(&["needs", "needs-gpu", "crew:needs:gpu"]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_requirement_that_does_not_parse_is_refused_never_dropped() {
        for body in [
            "Needs: Fast CPU!",
            "Needs: x86_64, ??",
            "Needs: foo:bar",
            "Needs: tool:",
            "Needs: ram:16 gb!",
        ] {
            assert_eq!(rule(needs_of_ticket(body, &[])), "needs-invalid", "{body}");
        }
        assert_eq!(
            rule(needs_of_ticket("", &labels(&["needs:BAD TAG"]))),
            "needs-invalid"
        );
    }

    #[test]
    fn requirement_forms_are_normalised() {
        for (raw, want) in [
            ("x86_64", "arch:x86_64"),
            ("AMD64", "arch:x86_64"),
            ("aarch64", "arch:arm64"),
            ("arm64", "arch:arm64"),
            ("arch:aarch64", "arch:arm64"),
            ("OS:macos-15", "os:macos-15"),
            ("tool:Cargo", "tool:cargo"),
            ("tier:high", "tier:high"),
            ("e2e", "e2e"),
        ] {
            assert_eq!(parse_requirement(raw).unwrap(), want, "{raw}");
        }
    }

    // -- matching ----------------------------------------------------------------------

    fn dev() -> Caps {
        Caps::parse(
            "e2e x86-builds tier:mid arch:x86_64 os:linux-ubuntu-24 cpu:8 ram:16g tool:cargo tool:docker",
        )
    }

    #[test]
    fn matching_rules_each_kind_met_and_each_kind_missing() {
        let c = dev();
        // declared tag
        assert!(is_met("e2e", &c));
        assert!(!is_met("gpu", &c));
        // a declared tag is not a fact and a fact name is not a tag
        assert!(!is_met("cargo", &c), "a bare tool name is not a tag");
        // arch
        assert!(is_met("arch:x86_64", &c));
        assert!(!is_met("arch:arm64", &c));
        // os, by name or by name and major, on a hyphen boundary only
        assert!(is_met("os:linux", &c));
        assert!(is_met("os:linux-ubuntu", &c));
        assert!(is_met("os:linux-ubuntu-24", &c));
        assert!(!is_met("os:linux-ubuntu-22", &c));
        assert!(!is_met("os:lin", &c), "no prefix inside a word");
        assert!(!is_met("os:macos", &c));
        assert!(!is_met("os:linux-ubuntu-2", &c), "24 is not 2");
        // tool, cpu, ram: exact
        assert!(is_met("tool:docker", &c));
        assert!(!is_met("tool:xcodebuild", &c));
        assert!(is_met("cpu:8", &c));
        assert!(!is_met("cpu:4", &c));
        assert!(is_met("ram:16g", &c));
        // tier
        assert!(is_met("tier:mid", &c));
        assert!(!is_met("tier:high", &c));
        assert!(!is_met("tier:mid", &Caps::default()));
    }

    #[test]
    fn a_bare_arch_matches_the_arch_fact_and_nothing_else() {
        let need = needs_of_ticket("Needs: x86_64", &[]).unwrap();
        assert!(missing(&need, &dev()).is_empty());
        let arm = Caps::parse("arch:arm64 os:macos-15");
        assert_eq!(missing(&need, &arm), ["arch:x86_64"]);
        // an agent that declares a tag called like an arch is not an arch match
        assert!(!is_met("arch:x86_64", &Caps::parse("x86-64")));
    }

    #[test]
    fn missing_names_every_unmet_requirement_in_order() {
        let need = needs_of_ticket("Needs: x86_64, gpu, e2e, ui-review", &[]).unwrap();
        assert_eq!(missing(&need, &dev()), ["gpu", "ui-review"]);
        assert!(missing(&[], &dev()).is_empty());
        assert_eq!(missing(&need, &Caps::default()).len(), 4);
    }

    // -- the assign rule -----------------------------------------------------------------

    #[test]
    fn assign_passes_when_the_agent_meets_every_requirement() {
        let need = needs_of_ticket("Needs: x86_64, e2e", &[]).unwrap();
        assert_eq!(
            check_assign("hermes@b", &need, &dev(), false, None).unwrap(),
            None
        );
        assert_eq!(
            check_assign("hermes@b", &[], &Caps::default(), false, None).unwrap(),
            None
        );
    }

    #[test]
    fn assign_is_refused_with_the_missing_tags_named() {
        let need = needs_of_ticket("Needs: x86_64, gpu, ui-review", &[]).unwrap();
        let e = check_assign("hermes@b", &need, &dev(), false, None).unwrap_err();
        assert_eq!(e.rule, "capability-mismatch");
        assert!(e.message.contains("gpu, ui-review"), "{}", e.message);
        assert!(!e.message.contains("x86_64"), "the met tag is not named");
        assert!(
            e.message.contains("crewboard status --agents --can"),
            "{}",
            e.message
        );
        // an agent with no profile at all lacks everything
        let e = check_assign("a@b", &need, &Caps::default(), false, None).unwrap_err();
        assert!(
            e.message.contains("arch:x86_64, gpu, ui-review"),
            "{}",
            e.message
        );
    }

    #[test]
    fn a_cut_tool_list_is_named_when_a_tool_is_missing() {
        let mut c = dev();
        c.tools_cut = 5;
        let e = check_assign("a@b", &["tool:go".to_string()], &c, false, None).unwrap_err();
        assert!(e.message.contains("tools-cut:5"), "{}", e.message);
        let e = check_assign("a@b", &["gpu".to_string()], &c, false, None).unwrap_err();
        assert!(!e.message.contains("tools-cut"), "only for a missing tool");
    }

    #[test]
    fn override_with_a_reason_is_accepted_and_returns_what_to_record() {
        let need = needs_of_ticket("Needs: gpu, e2e", &[]).unwrap();
        let o = check_assign(
            "a@b",
            &need,
            &dev(),
            true,
            Some("  owner agreed, CPU is enough  "),
        )
        .unwrap()
        .expect("an override");
        assert_eq!(o.missing, ["gpu"]);
        assert_eq!(o.reason, "owner agreed, CPU is enough");
        // an override that is not needed records nothing
        assert_eq!(
            check_assign(
                "a@b",
                &["e2e".to_string()],
                &dev(),
                true,
                Some("just in case")
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn override_without_a_reason_is_refused_also_when_nothing_is_missing() {
        let need = needs_of_ticket("Needs: gpu", &[]).unwrap();
        for reason in [None, Some(""), Some("   ")] {
            let e = check_assign("a@b", &need, &dev(), true, reason).unwrap_err();
            assert_eq!(e.rule, "capability-override-needs-reason", "{reason:?}");
        }
        let e = check_assign("a@b", &["e2e".to_string()], &dev(), true, None).unwrap_err();
        assert_eq!(e.rule, "capability-override-needs-reason");
        // a reason without the flag does not override
        let e = check_assign("a@b", &need, &dev(), false, Some("please")).unwrap_err();
        assert_eq!(e.rule, "capability-mismatch");
    }

    // -- profile changes -----------------------------------------------------------------

    #[test]
    fn a_change_adds_removes_and_reports_exactly_what_moved() {
        let old = caps(&["e2e", "gpu"], &["arch:arm64", "os:macos-15", "tool:git"]);
        let c = Change {
            tags: vec!["x86-builds".into(), "e2e".into()],
            untags: vec!["gpu".into(), "never-had".into()],
            tier: Some("high".into()),
            ..Change::default()
        };
        let a = apply_change(&old, "", &c).unwrap();
        assert_eq!(a.added, ["x86-builds"], "e2e was there already");
        assert_eq!(a.removed, ["gpu"], "an unknown tag is no removal");
        assert!(a.tier_changed && a.changed());
        assert_eq!(
            a.caps.compose().unwrap(),
            "e2e x86-builds tier:high arch:arm64 os:macos-15 tool:git",
            "facts are untouched"
        );
        // control: the same change again changes nothing
        let again = apply_change(&a.caps, "", &c).unwrap();
        assert!(!again.changed(), "{again:?}");
    }

    #[test]
    fn a_change_cannot_edit_facts_and_conflicts_are_refused() {
        // facts have a ':' and are no tags, so they cannot be passed as one
        assert_eq!(
            rule(Change::parse_tags(&["arch:x86_64".to_string()])),
            "profile-tag-invalid"
        );
        assert_eq!(
            rule(Change::parse_tags(&["tool:cargo".to_string()])),
            "profile-tag-invalid"
        );
        assert_eq!(
            Change::parse_tags(&["a,b c".to_string(), "b".to_string()]).unwrap(),
            ["a", "b", "c"]
        );
        let old = caps(&["e2e"], &[]);
        let both = Change {
            tags: vec!["gpu".into()],
            untags: vec!["gpu".into()],
            ..Change::default()
        };
        assert_eq!(rule(apply_change(&old, "", &both)), "profile-tag-conflict");
        let tier = Change {
            tier: Some("low".into()),
            clear_tier: true,
            ..Change::default()
        };
        assert_eq!(rule(apply_change(&old, "", &tier)), "profile-tag-conflict");
        assert_eq!(
            rule(apply_change(
                &old,
                "",
                &Change {
                    tier: Some("Low!".into()),
                    ..Change::default()
                }
            )),
            "profile-tier-invalid"
        );
    }

    #[test]
    fn clearing_the_tier_and_changing_the_background_are_changes() {
        let mut old = caps(&["e2e"], &[]);
        old.tier = Some("high".into());
        let a = apply_change(
            &old,
            "old text",
            &Change {
                clear_tier: true,
                about: Some("new text".into()),
                ..Change::default()
            },
        )
        .unwrap();
        assert_eq!(a.caps.tier, None);
        assert!(a.tier_changed && a.about_changed);
        // the same text again is no change; an empty text clears
        let same = apply_change(
            &old,
            "x",
            &Change {
                about: Some(" x ".into()),
                ..Change::default()
            },
        )
        .unwrap();
        assert!(!same.about_changed);
        let clear = apply_change(
            &old,
            "x",
            &Change {
                about: Some(String::new()),
                ..Change::default()
            },
        )
        .unwrap();
        assert!(clear.about_changed);
        assert!(Change::default().is_empty());
    }

    // -- the body --------------------------------------------------------------------------

    #[test]
    fn the_body_round_trips_background_and_facts() {
        let c = caps(&["e2e", "gpu"], &["arch:arm64"]);
        c.compose().unwrap();
        let facts = "Detected by crewboard at T.\n\n- arch: arm64\n- tools:\n  - cargo 1.91.0";
        let text = Body::render(
            "claude@host-a",
            &c,
            "Runs the UI review.\n\nTwo paragraphs.",
            facts,
        );
        assert!(text.contains("Tags: e2e, gpu") && text.contains("Tier: (none)"));
        let b = Body::parse(&text);
        assert_eq!(b.about, "Runs the UI review.\n\nTwo paragraphs.");
        assert_eq!(b.facts_md, facts);
        // render(parse(x)) is stable
        assert_eq!(
            Body::render("claude@host-a", &c, &b.about, &b.facts_md),
            text
        );
        // the placeholder text is not read back as a background
        let empty = Body::render("a@b", &c, "", "");
        assert_eq!(Body::parse(&empty).about, "");
        assert_eq!(
            Body::parse(&empty).facts_md,
            "(not detected yet: `crewboard start` detects them)"
        );
        // the placeholder of an older version, on a board that is not rewritten yet, is no
        // background either
        let old = format!("{PROFILE_H}\n\n{ABOUT_H}\n{NO_ABOUT_OLD}\n\n{FACTS_H}\nfacts\n");
        assert_eq!(Body::parse(&old).about, "");
        // a plain first-join body has neither
        let first = Body::parse("crewboard agent item of a@b");
        assert_eq!((first.about.as_str(), first.facts_md.as_str()), ("", ""));
    }
}
