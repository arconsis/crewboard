//! The agent config: `<config-dir>/<initiative>.toml`, with its atomic write.

use super::board::BoardConfig;
use super::io;
use crate::error::{CbError, CbResult};
use crate::protocol::names::AgentName;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    pub runtime: String,
    pub host: String,
    /// Node id of the agent item on the board (spec/board#item-ids-are-the-source-of-truth).
    pub item_id: String,
    /// Blob SHA of the `.crewboard.toml` that `board` was read from. `start` writes
    /// it when it refreshes the copy; `status` compares it with the default branch. Absent: the
    /// copy has no known source version, and `status` compares the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board_sha: Option<String>,
    pub board: BoardConfig,
    /// The agent's consent to a board without `crew_login` (`start --insecure-one-account`
    /// writes it). It is not a `join` key: a re-join keeps it. A board with a crew login
    /// ignores it.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub insecure_one_account: bool,
    /// The tools whose versions `join` and `start` detect (spec/profiles#facts). Absent: the
    /// default list `crewboard::facts::DEFAULT_TOOLS`. The `detect_tools` of the board are added
    /// in both cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// The optional `[loop]` table: the arguments of the watch loop. `watch` reads
    /// `interval` and `timeout` when no flag is given; `resume` prints the re-arm command.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_cfg: Option<LoopConfig>,
    /// The optional `[wake]` table: the settings of `crewboard wake` for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeConfig>,
}

/// `[wake]` of the agent config. It has no key yet; an unknown key is a config error. A re-join
/// keeps the table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WakeConfig {}

/// `[loop]` of the agent config. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopConfig {
    /// Time between polls of `watch`, for example `60s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,
    /// `watch --timeout`, for example `30m`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// Log file of `crewboard monitor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

impl LoopConfig {
    /// Both durations must parse. An invalid value is a config error (exit 4), not a default.
    pub fn validate(&self) -> CbResult<()> {
        for (k, v) in [("interval", &self.interval), ("timeout", &self.timeout)] {
            if let Some(v) = v {
                crate::clock::parse_duration(v)
                    .map_err(|e| CbError::config(format!("[loop] {k}: {e}")))?;
            }
        }
        if self.log.as_deref().is_some_and(|l| l.trim().is_empty()) {
            return Err(CbError::config("[loop] log is empty"));
        }
        Ok(())
    }
}

/// The top-level key of the agent config that switches the headless scope on
/// (spec/agents#headless-coordinator).
pub const HEADLESS_KEY: &str = "headless";

/// The keys of the `[loop]` table.
pub const LOOP_KEYS: [&str; 3] = ["interval", "timeout", "log"];

/// The keys of the agent config that `join` writes. A re-join replaces these and keeps all others.
pub(super) const JOIN_KEYS: [&str; 7] = [
    "name",
    "runtime",
    "host",
    "item_id",
    "board_sha",
    "board",
    "tools",
];

/// Write a config table to a temp file next to `p` and rename it over `p`. `headless` is
/// written as the first line, so it stays a top-level key before any table
/// (spec/agents#headless-coordinator).
pub(super) fn write_table_atomic(p: &Path, t: &toml::Table) -> CbResult<()> {
    let mut rest = t.clone();
    let mut out = String::new();
    if let Some(h) = rest.remove(HEADLESS_KEY) {
        let mut one = toml::Table::new();
        one.insert(HEADLESS_KEY.into(), h);
        out = toml::to_string(&one).map_err(|e| CbError::config(e.to_string()))?;
    }
    out.push_str(&toml::to_string(&rest).map_err(|e| CbError::config(e.to_string()))?);
    // The config dir can hold identity material: a replaced file keeps the permissions of the
    // old one, a new file is private (0600 on unix). A failure to set them stops the write
    // before the rename, so the target never gets wider permissions than it had.
    crate::fsutil::write_atomic_private(p, out.as_bytes()).map_err(io("write agent config"))
}

/// A file is an agent config if it has the keys that only an agent config has. This is a loose
/// check on purpose: a config with a typo inside `[loop]` is still an agent config, and
/// `load_agent` then names the typo instead of "no agent config".
pub(super) fn looks_like_agent_config(text: &str) -> bool {
    toml::from_str::<toml::Table>(text).is_ok_and(|t| {
        ["name", "item_id", "board"]
            .iter()
            .all(|k| t.contains_key(*k))
    })
}

/// The error for a key in `[loop]` that is not allowed, naming the file, the table and the key.
pub(super) fn unknown_loop_key(path: &Path, text: &str) -> Option<CbError> {
    let t = toml::from_str::<toml::Table>(text).ok()?;
    let l = t.get("loop")?.as_table()?;
    let bad = l.keys().find(|k| !LOOP_KEYS.contains(&k.as_str()))?;
    Some(CbError::config(format!(
        "config error in {}: unknown key `{bad}` in [loop]; allowed: {}",
        path.display(),
        LOOP_KEYS.join(", ")
    )))
}

impl AgentConfig {
    pub fn agent_name(&self) -> CbResult<AgentName> {
        Ok(AgentName::parse(&self.name)?)
    }
}

#[cfg(all(test, unix))]
mod permission_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn table(v: i64) -> toml::Table {
        let mut t = toml::Table::new();
        t.insert("x".into(), toml::Value::Integer(v));
        t
    }

    #[test]
    fn a_new_agent_config_is_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        write_table_atomic(&p, &table(1)).unwrap();
        assert_eq!(mode(&p), 0o600);
    }

    #[test]
    fn a_replaced_agent_config_keeps_the_mode_of_the_old_file_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        std::fs::write(&p, "x = 0\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_table_atomic(&p, &table(2)).unwrap();
        assert_eq!(mode(&p), 0o640);
        assert!(std::fs::read_to_string(&p).unwrap().contains("x = 2"));
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_permission_step_is_an_error_and_keeps_the_old_file() {
        // the permission step runs inside the shared helper before the rename; a failure there
        // returns the error, removes the temp file and leaves the target as it was
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        std::fs::write(&p, "x = 0\n").unwrap();
        let e = crate::fsutil::write_atomic_with(&p, b"x = 9\n", |_| {
            Err(std::io::Error::other("chmod failed"))
        });
        assert!(e.is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "x = 0\n");
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 1);
    }
}
