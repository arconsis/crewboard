//! An `init` rerun on an existing board config file: `init` sets the keys that it decides and
//! keeps every other key of the file (spec/cli#commands).
//!
//! The owner adds keys by hand (`heartbeat_interval`, `required_tools`, `[late]`, ...), and a
//! later build may add keys that this build does not know. A rerun of `init` (an upgrade note
//! asks for one) must not drop them. The lines of the file stay where they can, comments too:
//! only the line of a changed key is replaced, removed or added. A file whose changed keys are
//! not each on one line of their own is written anew from its keys, which keeps every key but
//! not the comments.

use super::board::BoardConfig;
use crate::error::{CbError, CbResult};

/// The keys that `init` decides, from its arguments and from GitHub. A rerun sets them (or
/// removes them when the rerun does not set them). `app_login` is the old name of `crew_login`.
pub const INIT_KEYS: [&str; 9] = [
    "initiative",
    "project_owner",
    "project_number",
    "repos",
    "owner_login",
    "registry_issue",
    "crew_login",
    "app_login",
    "insecure_one_account",
];

/// The text of an existing board config file (`existing`) after `init` ran again with `new`: the
/// keys of [`INIT_KEYS`] as `new` has them, `authority` only when the file has none, every other
/// key as the file has it. `header`: the first lines of a file that is written anew. Also returns
/// the keys that were kept (not keys of `init`), in file order. An `existing` that is not valid
/// TOML, or a result that is no valid board config, is an error: `init` does not overwrite what it
/// cannot read.
pub fn merge_init_keys(
    existing: &str,
    new: &BoardConfig,
    header: &str,
) -> CbResult<(String, Vec<String>)> {
    let old: toml::Table = toml::from_str(existing)
        .map_err(|e| CbError::config(format!("not valid TOML: {}", e.to_string().trim_end())))?;
    let new_t = toml::Table::try_from(new).map_err(|e| CbError::config(e.to_string()))?;
    let mut want = old.clone();
    for k in INIT_KEYS {
        match new_t.get(k) {
            Some(v) => want.insert(k.to_string(), v.clone()),
            None => want.remove(k),
        };
    }
    if !want.contains_key("authority")
        && let Some(v) = new_t.get("authority")
    {
        want.insert("authority".into(), v.clone());
    }
    let kept: Vec<String> = old
        .keys()
        .filter(|k| !INIT_KEYS.contains(&k.as_str()) && k.as_str() != "authority")
        .cloned()
        .collect();
    if want == old {
        return Ok((existing.to_string(), kept));
    }
    let text = edit_lines(existing, &old, &want)
        .filter(|t| toml::from_str::<toml::Table>(t).ok().as_ref() == Some(&want))
        .map_or_else(
            || {
                toml::to_string(&want)
                    .map(|t| format!("{header}{t}"))
                    .map_err(|e| CbError::config(e.to_string()))
            },
            Ok,
        )?;
    BoardConfig::parse(&text)?;
    Ok((text, kept))
}

/// The bare key of a line `key = value`, or `None`.
fn line_key(line: &str) -> Option<&str> {
    let (k, _) = line.split_once('=')?;
    let k = k.trim();
    (!k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
    .then_some(k)
}

/// `text` with the top-level keys that differ between `old` and `want` changed line by line: the
/// line of a changed key is replaced, the line of a removed key goes, a new key goes after the
/// last key line before the first table. `None` when a changed key that the file has is not on a
/// line before the first table. The caller checks the result by parsing it.
fn edit_lines(text: &str, old: &toml::Table, want: &toml::Table) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let first_table = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let at = |k: &str| {
        lines[..first_table]
            .iter()
            .position(|l| line_key(l) == Some(k))
    };
    let render = |k: &str, v: &toml::Value| {
        let mut one = toml::Table::new();
        one.insert(k.to_string(), v.clone());
        toml::to_string(&one).ok().map(|s| s.trim_end().to_string())
    };
    let mut replace: Vec<(usize, Option<String>)> = Vec::new();
    let mut add: Vec<String> = Vec::new();
    let mut keys: Vec<&String> = old.keys().chain(want.keys()).collect();
    keys.sort();
    keys.dedup();
    for k in keys.into_iter().filter(|k| old.get(*k) != want.get(*k)) {
        match (old.contains_key(k), want.get(k)) {
            (true, Some(v)) => replace.push((at(k)?, Some(render(k, v)?))),
            (true, None) => replace.push((at(k)?, None)),
            (false, Some(v)) => add.push(render(k, v)?),
            (false, None) => {}
        }
    }
    // a new key goes after the last key line before the first table (before a blank line or a
    // comment that leads into the table)
    let insert = lines[..first_table]
        .iter()
        .rposition(|l| line_key(l).is_some())
        .map_or(0, |i| i + 1);
    let mut out: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if i == insert {
            out.append(&mut add);
        }
        match replace.iter().find(|(j, _)| *j == i) {
            Some((_, Some(new))) => out.push(new.clone()),
            Some((_, None)) => {}
            None => out.push((*l).to_string()),
        }
    }
    out.append(&mut add);
    Some(out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "# header\n";

    const OWNER_FILE: &str = r#"# crewboard initiative file (no secrets). Agents find the board through it.
initiative = "demo"
project_owner = "example-org"
project_number = 15
repos = ["example-org/app"]
owner_login = "owner-login"
registry_issue = 2
authority = "docs/AUTHORITY.md"
# the owner's own comment: a slow host
heartbeat_interval = "30m"
required_tools = ["git --version", { cmd = "cargo --version", tags = ["rust"] }]
future_key = "a key of a later build"

[late]
push = "2h"
"#;

    fn new_cfg(registry: u64) -> BoardConfig {
        let mut c = BoardConfig::parse(OWNER_FILE).unwrap();
        c.registry_issue = registry;
        c.repos = vec!["example-org/app".into(), "example-org/other".into()];
        c.heartbeat_interval = None;
        c.required_tools = Vec::new();
        c.late = None;
        c.authority = "AUTHORITY.md".into();
        c
    }

    #[test]
    fn a_rerun_keeps_every_key_that_init_does_not_set_and_the_comments() {
        let (text, kept) = merge_init_keys(OWNER_FILE, &new_cfg(9), HEADER).unwrap();
        let c = BoardConfig::parse(&text).unwrap();
        assert_eq!(c.registry_issue, 9);
        assert_eq!(c.repos.len(), 2);
        assert_eq!(c.heartbeat_interval.as_deref(), Some("30m"));
        assert_eq!(c.required_tools.len(), 2);
        assert!(c.late.is_some());
        // the owner's path of AUTHORITY.md is not init's to change
        assert_eq!(c.authority, "docs/AUTHORITY.md");
        assert!(
            text.contains("# the owner's own comment: a slow host"),
            "{text}"
        );
        assert!(
            text.contains("future_key = \"a key of a later build\""),
            "{text}"
        );
        assert!(text.starts_with("# crewboard initiative file"), "{text}");
        assert_eq!(
            kept,
            ["future_key", "heartbeat_interval", "late", "required_tools"]
        );
        // nothing to change: the file stays byte for byte
        let (again, _) = merge_init_keys(&text, &new_cfg(9), HEADER).unwrap();
        assert_eq!(again, text);
    }

    #[test]
    fn a_key_that_init_sets_and_the_file_lacks_is_added_before_the_first_table() {
        let mut new = new_cfg(2);
        new.crew_login = Some("crew-bot".into());
        new.insecure_one_account = false;
        let (text, _) = merge_init_keys(OWNER_FILE, &new, HEADER).unwrap();
        let c = BoardConfig::parse(&text).unwrap();
        assert_eq!(c.crew_login.as_deref(), Some("crew-bot"));
        assert!(text.contains("# the owner's own comment"), "{text}");
        let crew = text.find("crew_login").unwrap();
        assert!(crew < text.find("[late]").unwrap(), "{text}");
        // and a rerun without it removes it again: the identity mode is init's key
        let (text, _) = merge_init_keys(&text, &new_cfg(2), HEADER).unwrap();
        assert!(!text.contains("crew_login"), "{text}");
        assert_eq!(
            BoardConfig::parse(&text)
                .unwrap()
                .heartbeat_interval
                .as_deref(),
            Some("30m")
        );
    }

    #[test]
    fn a_key_on_several_lines_makes_a_new_file_that_still_keeps_every_key() {
        let multi = OWNER_FILE.replace(
            "repos = [\"example-org/app\"]",
            "repos = [\n  \"example-org/app\",\n]",
        );
        let (text, _) = merge_init_keys(&multi, &new_cfg(2), HEADER).unwrap();
        assert!(text.starts_with(HEADER), "{text}");
        let c = BoardConfig::parse(&text).unwrap();
        assert_eq!(c.repos.len(), 2);
        assert_eq!(c.heartbeat_interval.as_deref(), Some("30m"));
        assert_eq!(c.required_tools.len(), 2);
        assert!(text.contains("future_key"), "{text}");
    }

    #[test]
    fn a_file_that_is_not_valid_toml_is_an_error_and_not_overwritten() {
        let e = merge_init_keys("initiative = ", &new_cfg(2), HEADER)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not valid TOML"), "{e}");
    }

    #[test]
    fn the_old_name_app_login_goes_when_init_writes_crew_login() {
        let old = OWNER_FILE.replace(
            "registry_issue = 2\n",
            "registry_issue = 2\napp_login = \"crew-bot\"\n",
        );
        let mut new = new_cfg(2);
        new.crew_login = Some("crew-bot".into());
        let (text, _) = merge_init_keys(&old, &new, HEADER).unwrap();
        assert!(!text.contains("app_login"), "{text}");
        assert!(text.contains("crew_login = \"crew-bot\""), "{text}");
    }
}
