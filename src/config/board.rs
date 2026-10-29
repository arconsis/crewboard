//! The board config: `.crewboard.toml` in the initiative's main repo. No secrets.

use crate::error::{CbError, CbResult};
use serde::{Deserialize, Serialize};

pub const INITIATIVE_FILE: &str = ".crewboard.toml";

/// The keys of the board config that `start` never changes in the agent's copy.
/// See [`BoardConfig::identity_changes`] for the reason of each.
pub const IDENTITY_KEYS: [&str; 7] = [
    "initiative",
    "project_owner",
    "project_number",
    "owner_login",
    "registry_issue",
    "crew_login",
    "insecure_one_account",
];

/// The initiative file. Agents find the board through it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardConfig {
    /// Short name, `[a-z0-9-]`. Used in file names.
    pub initiative: String,
    /// Login of the organisation or user that owns the project.
    pub project_owner: String,
    pub project_number: u64,
    /// `owner/repo`. The first one is the main repo (registry issue, AUTHORITY.md).
    pub repos: Vec<String>,
    /// GitHub login of the human owner.
    pub owner_login: String,
    /// Number of the registry issue in the main repo.
    pub registry_issue: u64,
    /// Path of AUTHORITY.md in the main repo.
    #[serde(default = "default_authority")]
    pub authority: String,
    /// GitHub login of the shared crew account (a machine account such as `crew-bot`): the
    /// account that the agents of the owner act as. A contributor's agent may act as its own
    /// account instead; the owner's approval binds each agent to the account that wrote its
    /// join. Absent: the one-person trial, where the agents use the owner's token (one-account
    /// mode). When set:
    ///
    /// - a headerless comment is an owner message only if the OWNER wrote it;
    /// - every agent command refuses to run with the owner's token (rule
    ///   `agent-uses-owner-token`), and `approve` refuses a join that the owner's account wrote;
    /// - `start` refuses a token or a plain `gh` of another account than the approved one
    ///   (`agent-token-not-crew`, `agent-gh-not-crew`).
    ///
    /// It must differ from `owner_login`. The old name `app_login` is still read.
    #[serde(default, alias = "app_login", skip_serializing_if = "Option::is_none")]
    pub crew_login: Option<String>,
    /// The owner's consent to a board without `crew_login`: the agents then use the owner's
    /// token and nothing separates owner from agent (a one-person trial). Without a crew
    /// login and without this key, every agent command refuses (rule `one-account-needs-flag`).
    /// It must not be set together with `crew_login`.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub insecure_one_account: bool,
    /// Heartbeat interval of the initiative, for example `10m` or `20s` (spec/board#fields; default
    /// 10 minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval: Option<String>,
    /// Commands that must run for a dev agent that starts, each with a version flag, for
    /// example `["git --version", { cmd = "node --version", tags = ["web"] }]`. `start` runs them from the agent's
    /// work folder with the agent's PATH and refuses `active` (rule `toolchain-unreachable`)
    /// when one fails. Empty: no check (spec/agents#toolchain-check-of-start). An entry is a plain
    /// command (every agent) or a table `{ cmd = "...", tags = ["rust"] }` (only the agents
    /// that declare one of the tags).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tools: Vec<RequiredTool>,
    /// One command that proves the build works, for example `make test` or `npm run build`. `start`
    /// runs it after `required_tools`; the dev brief says to run it before the first `take`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smoke_cmd: Option<String>,
    /// Tool names that `join` and every `start` probe for a version, for example
    /// `["java", "gradle", "xcodebuild"]`, in addition to the default list (`crewboard::facts::
    /// DEFAULT_TOOLS`) and to the `tools` of the agent. The names are free: the stack of the
    /// project decides. A tool found becomes the fact `tool:<name>`; a tool that is not
    /// installed is left out, never an error. This is a probe, not a check: `required_tools` is
    /// the list that blocks `start`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detect_tools: Vec<String>,
    /// The optional `[stale]` table: after how long `status` and `poll` call work stale
    /// (spec/events#stale-work). Last in the struct, because TOML writes a table after the plain
    /// keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale: Option<StaleConfig>,
    /// The optional `[late]` table: the table of expected times (spec/events#stale-work). It
    /// replaces `[stale]`; both in one file is a config error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late: Option<LateConfig>,
    /// The former names of the main repo (`former_repos` of AUTHORITY.md), for the signature
    /// checks. Not a key of the initiative file: `ctx` reads AUTHORITY.md once where it loads the
    /// context and sets this. Empty until then. Use [`BoardConfig::former_for`].
    #[serde(skip)]
    pub former_repos: Vec<crate::protocol::former::FormerRepo>,
}

/// One entry of `required_tools`: a command, and the agent tags that need it. No tags: every
/// agent needs it. In the file it is a plain string (no tags) or an inline table with the keys
/// `cmd` and `tags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredTool {
    pub cmd: String,
    pub tags: Vec<String>,
}

impl RequiredTool {
    /// An entry for every agent.
    pub fn all(cmd: impl Into<String>) -> Self {
        Self {
            cmd: cmd.into(),
            tags: vec![],
        }
    }

    /// An entry for the agents with one of `tags`.
    pub fn tagged(cmd: impl Into<String>, tags: &[&str]) -> Self {
        Self {
            cmd: cmd.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// True if an agent with these declared tags must have the tool.
    pub fn applies_to(&self, agent_tags: &std::collections::BTreeSet<String>) -> bool {
        self.tags.is_empty() || self.tags.iter().any(|t| agent_tags.contains(t))
    }
}

/// The table form, read by [`RequiredTool`].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequiredToolTable {
    cmd: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
}

/// A tag is written as in `crewboard agent profile`: 1 to 32 characters of a-z, 0-9 and `-`, no `-` at
/// the start or the end.
fn valid_tool_tag(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 32
        && t.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !t.starts_with('-')
        && !t.ends_with('-')
}

impl RequiredTool {
    fn from_parts<E: serde::de::Error>(cmd: String, tags: Option<Vec<String>>) -> Result<Self, E> {
        if cmd.trim().is_empty() {
            return Err(E::custom("required_tools: the command is empty"));
        }
        let tags = tags.unwrap_or_default();
        for t in &tags {
            if !valid_tool_tag(t) {
                return Err(E::custom(format!(
                    "required_tools: '{t}' (tags of `{cmd}`) is not a tag: use 1-32 characters of a-z, 0-9 and '-', no '-' at the start or end"
                )));
            }
        }
        Ok(Self { cmd, tags })
    }
}

impl<'de> Deserialize<'de> for RequiredTool {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{Error, MapAccess, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = RequiredTool;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(
                    "a command string (every agent) or a table { cmd = \"...\", tags = [\"...\"] } (agents with one of the tags)",
                )
            }
            fn visit_str<E: Error>(self, v: &str) -> Result<RequiredTool, E> {
                RequiredTool::from_parts(v.to_string(), None)
            }
            fn visit_map<M: MapAccess<'de>>(self, m: M) -> Result<RequiredTool, M::Error> {
                let t = RequiredToolTable::deserialize(
                    serde::de::value::MapAccessDeserializer::new(m),
                )?;
                if t.tags.as_ref().is_some_and(Vec::is_empty) {
                    return Err(M::Error::custom(format!(
                        "required_tools: `{}` has an empty tags list; leave `tags` out for a tool that every agent needs",
                        t.cmd
                    )));
                }
                RequiredTool::from_parts(t.cmd, t.tags)
            }
        }
        d.deserialize_any(V)
    }
}

impl Serialize for RequiredTool {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        if self.tags.is_empty() {
            return s.serialize_str(&self.cmd);
        }
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("cmd", &self.cmd)?;
        m.serialize_entry("tags", &self.tags)?;
        m.end()
    }
}

/// The value of `take`, `answer` and `owner` in `[late]` that turns the wait off.
pub const LATE_OFF: &str = "off";

/// `[late]` of `.crewboard.toml`. Every key is optional and takes a duration such as `90`,
/// `90s`, `30m` or `2h`. `take`, `answer` and `owner` also take `off`. `take` is off until the key
/// is set; `answer` (4h) and `owner` (1d) are on by default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LateConfig {
    /// In Progress without a pushed crew branch (default 2h).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<String>,
    /// Green CI on the head of an open PR without a `done` (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<String>,
    /// In Review without a `verify` (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    /// How long after the expected time of a dev wait the coordinator hears of it (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate: Option<String>,
    /// How long until the coordinator hears of the same wait again (default 2h).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<String>,
    /// Ready and assigned, not taken (default off; `off`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<String>,
    /// Blocked, the question has no answer (default 4h; `off` turns it off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// A question waits for the owner, or Review waits for an owner merge (default 1d; `off` turns
    /// it off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

/// `[stale]` of `.crewboard.toml`. Every key is optional and takes a duration such as `90`,
/// `90s`, `30m` or `2h`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleConfig {
    /// In Progress without a pushed crew branch (default 2h).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_push: Option<String>,
    /// Green CI on the head of an open PR without a `done` (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub green: Option<String>,
    /// In Review without a `verify` (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    /// How long after the limit of `no-push` or `green-not-done` the coordinator hears of the
    /// marker (default 30m).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalate: Option<String>,
    /// How long until the coordinator hears of the same escalated marker again (default 2h).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<String>,
}

fn default_authority() -> String {
    "AUTHORITY.md".into()
}

/// Split `owner/repo`.
pub fn split_repo(r: &str) -> CbResult<(&str, &str)> {
    match r.split_once('/') {
        Some((o, n)) if !o.is_empty() && !n.is_empty() && !n.contains('/') => Ok((o, n)),
        _ => Err(CbError::config(format!("'{r}' is not owner/repo"))),
    }
}

pub fn valid_initiative_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

impl BoardConfig {
    pub fn parse(text: &str) -> CbResult<Self> {
        let c: BoardConfig =
            toml::from_str(text).map_err(|e| CbError::config(format!("{INITIATIVE_FILE}: {e}")))?;
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> CbResult<()> {
        if !valid_initiative_name(&self.initiative) {
            return Err(CbError::config(format!(
                "initiative '{}' must be 1-40 characters of a-z, 0-9, '-'",
                self.initiative
            )));
        }
        if self.repos.is_empty() {
            return Err(CbError::config("repos is empty"));
        }
        for r in &self.repos {
            split_repo(r)?;
        }
        if self.project_owner.is_empty() || self.owner_login.is_empty() {
            return Err(CbError::config("project_owner and owner_login must be set"));
        }
        if let Some(c) = &self.crew_login {
            if c.is_empty() {
                return Err(CbError::config("crew_login is empty"));
            }
            if c.eq_ignore_ascii_case(&self.owner_login) {
                return Err(CbError::config(
                    "crew_login must not be the owner_login: agents may not act as the owner",
                ));
            }
        }
        if self.crew_login.is_some() && self.insecure_one_account {
            return Err(CbError::config(
                "insecure_one_account must not be set together with crew_login: a board has one identity mode",
            ));
        }
        if let Some(i) = &self.heartbeat_interval {
            crate::clock::parse_interval(i).map_err(CbError::config)?;
        }
        for t in &self.detect_tools {
            if !crate::base::valid_tool_name(t) {
                return Err(CbError::config(format!(
                    "{INITIATIVE_FILE}: detect_tools: '{t}' is not a tool name ({})",
                    crate::base::TOOL_NAME_RULE
                )));
            }
        }
        if let Some(st) = &self.stale {
            for (k, v) in [
                ("no_push", &st.no_push),
                ("green", &st.green),
                ("review", &st.review),
                ("escalate", &st.escalate),
                ("repeat", &st.repeat),
            ] {
                if let Some(v) = v {
                    crate::clock::parse_interval(v)
                        .map_err(|e| CbError::config(format!("[stale] {k}: {e}")))?;
                }
            }
        }
        if let Some(l) = &self.late {
            if self.stale.is_some() {
                return Err(CbError::config(
                    "late-and-stale-both-set: .crewboard.toml has [late] and [stale]; keep [late] only (push, done and verify replace no_push, green and review)",
                ));
            }
            for (k, v) in [
                ("push", &l.push),
                ("done", &l.done),
                ("verify", &l.verify),
                ("escalate", &l.escalate),
                ("repeat", &l.repeat),
                ("take", &l.take),
                ("answer", &l.answer),
                ("owner", &l.owner),
            ] {
                let off = matches!(k, "take" | "answer" | "owner")
                    && v.as_deref()
                        .is_some_and(|t| t.eq_ignore_ascii_case(LATE_OFF));
                if let (Some(v), false) = (v, off) {
                    crate::clock::parse_interval(v)
                        .map_err(|e| CbError::config(format!("[late] {k}: {e}")))?;
                }
            }
        }
        if self.project_number == 0 || self.registry_issue == 0 {
            return Err(CbError::config(
                "project_number and registry_issue must be set (run `crewboard init`)",
            ));
        }
        Ok(())
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).unwrap_or_default()
    }

    pub fn main_repo(&self) -> &str {
        &self.repos[0]
    }

    /// The former names that apply to an issue of `repo`: those of the main repo, and only for
    /// it. A former name never applies to another repo of the initiative, so a message cannot be
    /// replayed from one repo to another with the same issue number.
    pub fn former_for(&self, repo: &str) -> &[crate::protocol::former::FormerRepo] {
        if self.main_repo().eq_ignore_ascii_case(repo) {
            &self.former_repos
        } else {
            &[]
        }
    }

    /// The keys whose value differs between two copies, in the order of the file.
    /// A key that one side leaves out counts as different. `repos` is one key; `stale` and `late` are
    /// one key each (not their sub-keys).
    pub fn changed_keys(&self, new: &BoardConfig) -> Vec<String> {
        let (a, b) = (toml::Table::try_from(self), toml::Table::try_from(new));
        let (Ok(a), Ok(b)) = (a, b) else {
            return vec!["(unreadable)".into()];
        };
        let mut keys: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
        keys.sort();
        keys.dedup();
        keys.retain(|k| a.get(k) != b.get(k));
        keys
    }

    /// The changed keys that a refresh must not apply (rule `board-identity-changed`).
    /// Identity: the board (`project_owner`, `project_number`), who the owner is
    /// (`owner_login`), the crew account (`crew_login`, it decides whether agents may act as the
    /// owner), the consent to a board without one (`insecure_one_account`), the registry
    /// (`registry_issue`), the file name of the config (`initiative`) and the main repo (the
    /// first entry of `repos`: registry and AUTHORITY.md live there). The other repos are not
    /// identity: a repo that joins or leaves the initiative is a normal edit. Everything else
    /// (`stale`, `required_tools`, ...) is applied.
    pub fn identity_changes(&self, new: &BoardConfig) -> Vec<String> {
        let mut v: Vec<String> = self
            .changed_keys(new)
            .into_iter()
            .filter(|k| IDENTITY_KEYS.contains(&k.as_str()))
            .collect();
        if self.repos.first() != new.repos.first() {
            v.push("repos (main repo)".into());
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
initiative = "demo"
project_owner = "example-org"
project_number = 15
repos = ["example-org/app", "example-org/other"]
owner_login = "owner-login"
registry_issue = 2
"#;

    #[test]
    fn former_names_apply_to_the_main_repo_only_and_are_not_a_key_of_the_file() {
        let mut cfg = BoardConfig::parse(GOOD).unwrap();
        assert!(cfg.former_repos.is_empty());
        let f = crate::protocol::former::FormerRepo::parse("old-org/app", "2030-01-01T00:00:00Z")
            .unwrap();
        cfg.former_repos = vec![f.clone()];
        assert_eq!(cfg.former_for("example-org/app"), std::slice::from_ref(&f));
        assert_eq!(cfg.former_for("Example-Org/App"), std::slice::from_ref(&f));
        // another repo of the initiative: a message cannot be replayed from the old name
        assert!(cfg.former_for("example-org/other").is_empty());
        // the list is runtime state: it is not written to the file
        assert!(!toml::to_string(&cfg).unwrap().contains("former"));
    }

    #[test]
    fn board_config_parses_and_round_trips() {
        let c = BoardConfig::parse(GOOD).unwrap();
        assert_eq!(c.main_repo(), "example-org/app");
        assert_eq!(c.authority, "AUTHORITY.md");
        assert_eq!(BoardConfig::parse(&c.to_toml()).unwrap(), c);
    }

    fn with_tools(v: &str) -> String {
        format!("{GOOD}required_tools = {v}\n")
    }

    #[test]
    fn required_tools_take_plain_strings_tables_and_a_mix() {
        let c =
            BoardConfig::parse(&with_tools(r#"["cargo --version", "node --version"]"#)).unwrap();
        assert_eq!(
            c.required_tools,
            [
                RequiredTool::all("cargo --version"),
                RequiredTool::all("node --version")
            ]
        );
        let c = BoardConfig::parse(&with_tools(
            r#"["git --version", { cmd = "cargo --version", tags = ["rust"] }, { cmd = "uv --version" }]"#,
        ))
        .unwrap();
        assert_eq!(
            c.required_tools,
            [
                RequiredTool::all("git --version"),
                RequiredTool::tagged("cargo --version", &["rust"]),
                RequiredTool::all("uv --version"),
            ]
        );
        // the saved copy of the agent reads back the same, for the plain and the mixed list
        assert_eq!(
            BoardConfig::parse(&c.to_toml()).unwrap().required_tools,
            c.required_tools
        );
        let only_tables = BoardConfig::parse(&with_tools(
            r#"[{ cmd = "a", tags = ["x", "y"] }, { cmd = "b", tags = ["z"] }]"#,
        ))
        .unwrap();
        assert_eq!(
            BoardConfig::parse(&only_tables.to_toml()).unwrap(),
            only_tables
        );
        // an agent tag that applies, one that does not, and none
        let t = RequiredTool::tagged("a", &["x", "y"]);
        let has = |v: &[&str]| v.iter().map(|x| x.to_string()).collect();
        assert!(t.applies_to(&has(&["y"])));
        assert!(!t.applies_to(&has(&["z"])) && !t.applies_to(&has(&[])));
        assert!(RequiredTool::all("a").applies_to(&has(&[])));
    }

    #[test]
    fn a_bad_required_tools_entry_is_a_config_error_with_a_clear_message() {
        let cases = [
            (r#"[5]"#, "a command string"),
            (r#"[{ tags = ["rust"] }]"#, "missing field `cmd`"),
            (
                r#"[{ cmd = "cargo --version", tag = ["rust"] }]"#,
                "unknown field `tag`",
            ),
            (
                r#"[{ cmd = "cargo --version", tags = "rust" }]"#,
                "invalid type",
            ),
            (
                r#"[{ cmd = "cargo --version", tags = [] }]"#,
                "empty tags list",
            ),
            (
                r#"[{ cmd = "cargo --version", tags = ["Rust"] }]"#,
                "'Rust'",
            ),
            (
                r#"[{ cmd = "cargo --version", tags = ["-x"] }]"#,
                "is not a tag",
            ),
            (r#"[{ cmd = " " }]"#, "the command is empty"),
            (r#"[""]"#, "the command is empty"),
        ];
        for (v, want) in cases {
            let e = BoardConfig::parse(&with_tools(v)).unwrap_err().to_string();
            assert!(e.contains(want), "{v}: {e}");
            assert!(e.contains(INITIATIVE_FILE), "{e}");
        }
    }

    #[test]
    fn detect_tools_is_an_optional_list_that_round_trips() {
        assert!(BoardConfig::parse(GOOD).unwrap().detect_tools.is_empty());
        let c = BoardConfig::parse(&format!(
            "{GOOD}detect_tools = [\"java\", \"gradle\", \"xcodebuild\"]\n"
        ))
        .unwrap();
        assert_eq!(c.detect_tools, ["java", "gradle", "xcodebuild"]);
        let text = c.to_toml();
        assert!(text.contains("detect_tools"), "{text}");
        assert_eq!(BoardConfig::parse(&text).unwrap(), c);
        // an empty list is not written back
        assert!(
            !BoardConfig::parse(GOOD)
                .unwrap()
                .to_toml()
                .contains("detect_tools")
        );
    }

    #[test]
    fn a_bad_detect_tools_entry_names_the_file_and_the_entry() {
        for (v, want) in [
            (r#"["Java"]"#, "'Java'"),
            (r#"["go", "a b"]"#, "'a b'"),
            (r#"["../x"]"#, "'../x'"),
            (r#"[""]"#, "''"),
        ] {
            let e = BoardConfig::parse(&format!("{GOOD}detect_tools = {v}\n"))
                .unwrap_err()
                .to_string();
            assert!(
                e.contains(INITIATIVE_FILE) && e.contains("detect_tools"),
                "{v}: {e}"
            );
            assert!(e.contains(want), "{v}: {e}");
        }
        let e = BoardConfig::parse(&format!("{GOOD}detect_tools = \"go\"\n"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(INITIATIVE_FILE), "{e}");
    }

    #[test]
    fn board_config_refusals() {
        let cases = [
            GOOD.replace("demo\"\nproject_owner", "Demo Init\"\nproject_owner"),
            GOOD.replace("[\"example-org/app\", \"example-org/other\"]", "[]"),
            GOOD.replace("example-org/other", "noslash"),
            GOOD.replace("project_number = 15", "project_number = 0"),
            GOOD.replace("registry_issue = 2", ""),
            GOOD.replace("owner_login = \"owner-login\"", "owner_login = \"\""),
            "not toml at all [".to_string(),
        ];
        for (i, c) in cases.iter().enumerate() {
            assert!(BoardConfig::parse(c).is_err(), "case {i} should be refused");
        }
    }

    #[test]
    fn heartbeat_interval_is_optional_and_checked() {
        assert_eq!(BoardConfig::parse(GOOD).unwrap().heartbeat_interval, None);
        let c = BoardConfig::parse(&format!("{GOOD}heartbeat_interval = \"20s\"\n")).unwrap();
        assert_eq!(c.heartbeat_interval.as_deref(), Some("20s"));
        assert_eq!(BoardConfig::parse(&c.to_toml()).unwrap(), c);
        assert!(BoardConfig::parse(&format!("{GOOD}heartbeat_interval = \"soon\"\n")).is_err());
        // an unset interval is not written out (the initiative file stays unchanged)
        assert!(
            !BoardConfig::parse(GOOD)
                .unwrap()
                .to_toml()
                .contains("heartbeat")
        );
    }

    #[test]
    fn crew_login_round_trips() {
        let c = BoardConfig::parse(&format!("{GOOD}crew_login = \"crew-bot\"\n")).unwrap();
        assert_eq!(c.crew_login.as_deref(), Some("crew-bot"));
        assert_eq!(BoardConfig::parse(&c.to_toml()).unwrap(), c);
        assert!(c.to_toml().contains("crew_login"));
        // without it the key is not written (control: the check can say no)
        assert!(
            !BoardConfig::parse(GOOD)
                .unwrap()
                .to_toml()
                .contains("crew_login")
        );
    }

    #[test]
    fn the_old_name_app_login_is_still_read_and_written_as_crew_login() {
        let c = BoardConfig::parse(&format!("{GOOD}app_login = \"crew-bot[bot]\"\n")).unwrap();
        assert_eq!(c.crew_login.as_deref(), Some("crew-bot[bot]"));
        assert!(c.to_toml().contains("crew_login") && !c.to_toml().contains("app_login"));
    }

    #[test]
    fn insecure_one_account_round_trips_and_is_written_before_the_tables() {
        let t = format!("{GOOD}insecure_one_account = true\n[stale]\nreview = \"1h\"\n");
        let c = BoardConfig::parse(&t).unwrap();
        assert!(c.insecure_one_account);
        let out = c.to_toml();
        assert!(out.contains("insecure_one_account = true"), "{out}");
        assert!(
            out.find("insecure_one_account") < out.find("[stale]"),
            "{out}"
        );
        assert_eq!(BoardConfig::parse(&out).unwrap(), c);
        // the default is false and is not written
        let d = BoardConfig::parse(GOOD).unwrap();
        assert!(!d.insecure_one_account && !d.to_toml().contains("insecure_one_account"));
    }

    #[test]
    fn insecure_one_account_with_a_crew_login_is_a_config_error() {
        let t = format!("{GOOD}crew_login = \"crew-bot\"\ninsecure_one_account = true\n");
        let e = BoardConfig::parse(&t).unwrap_err().to_string();
        assert!(e.contains("insecure_one_account"), "{e}");
        // control: each key alone parses
        assert!(BoardConfig::parse(&format!("{GOOD}crew_login = \"crew-bot\"\n")).is_ok());
        assert!(BoardConfig::parse(&format!("{GOOD}insecure_one_account = true\n")).is_ok());
    }

    #[test]
    fn insecure_one_account_is_an_identity_key() {
        assert!(IDENTITY_KEYS.contains(&"insecure_one_account"));
        let a = BoardConfig::parse(GOOD).unwrap();
        let b = BoardConfig::parse(&format!("{GOOD}insecure_one_account = true\n")).unwrap();
        assert_eq!(
            a.identity_changes(&b),
            vec!["insecure_one_account".to_string()]
        );
        assert_eq!(
            b.identity_changes(&a),
            vec!["insecure_one_account".to_string()]
        );
        assert!(a.identity_changes(&a).is_empty());
    }

    #[test]
    fn the_crew_login_may_not_be_the_owner() {
        for bad in ["owner-login", "Owner-Login", ""] {
            let t = format!("{GOOD}crew_login = \"{bad}\"\n");
            assert!(BoardConfig::parse(&t).is_err(), "{bad:?}");
        }
    }
}
