//! The config directory: where the agent config, board config, keys and state file live.

use super::agent::{
    AgentConfig, HEADLESS_KEY, JOIN_KEYS, looks_like_agent_config, unknown_loop_key,
    write_table_atomic,
};
use super::board::{BoardConfig, valid_initiative_name};
use super::io;
use crate::error::{CbError, CbResult};
use crate::protocol::names::AgentName;
use std::path::{Path, PathBuf};

/// The directory that holds the local config and keys.
#[derive(Debug, Clone)]
pub struct ConfigDir(pub PathBuf);

impl ConfigDir {
    /// `--config-dir`, else `$CREWBOARD_CONFIG_DIR`, else `~/.config/crewboard`.
    pub fn resolve(flag: Option<&Path>) -> CbResult<Self> {
        if let Some(p) = flag {
            return Ok(ConfigDir(p.to_path_buf()));
        }
        if let Ok(p) = std::env::var("CREWBOARD_CONFIG_DIR")
            && !p.is_empty()
        {
            return Ok(ConfigDir(PathBuf::from(p)));
        }
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map_err(|_| CbError::config("cannot find the home directory; use --config-dir"))?;
        Ok(ConfigDir(
            PathBuf::from(home).join(".config").join("crewboard"),
        ))
    }

    pub fn agent_config_path(&self, initiative: &str) -> PathBuf {
        self.0.join(format!("{initiative}.toml"))
    }

    pub fn board_config_path(&self, initiative: &str) -> PathBuf {
        self.0.join(format!("{initiative}.board.toml"))
    }

    pub fn key_path(&self, agent: &AgentName) -> PathBuf {
        self.0.join("keys").join(agent.as_str())
    }

    pub fn state_path(&self, initiative: &str, agent: &AgentName) -> PathBuf {
        self.0
            .join("state")
            .join(format!("{initiative}.{}.json", agent.as_str()))
    }

    /// Write the agent config. The keys that `join` owns are replaced. Every other key of an
    /// existing file stays as it is (`headless`, `[loop]`, any key a later version adds), so
    /// a re-join never drops a guardrail. The write is atomic.
    pub fn save_agent(&self, c: &AgentConfig) -> CbResult<()> {
        std::fs::create_dir_all(&self.0).map_err(io("create config dir"))?;
        let p = self.agent_config_path(&c.board.initiative);
        let toml::Value::Table(mut t) =
            toml::Value::try_from(c).map_err(|e| CbError::config(e.to_string()))?
        else {
            return Err(CbError::config("the agent config is not a table"));
        };
        // An existing file that does not parse has no keys to keep: it is replaced.
        if let Some(old) = self.read_table(&p) {
            for (k, v) in old {
                if !JOIN_KEYS.contains(&k.as_str()) && !t.contains_key(&k) {
                    t.insert(k, v);
                }
            }
        }
        write_table_atomic(&p, &t)
    }

    fn read_table(&self, p: &Path) -> Option<toml::Table> {
        toml::from_str(&std::fs::read_to_string(p).ok()?).ok()
    }

    /// The agent name that the agent config of `initiative` holds, read from the file itself
    /// (so a config that `load_agent` refuses still names its agent).
    pub fn agent_name_on_disk(&self, initiative: &str) -> Option<String> {
        let t = self.read_table(&self.agent_config_path(initiative))?;
        t.get("name")?.as_str().map(str::to_string)
    }

    /// Replace the board table (and its source SHA) in the agent config and keep every other
    /// key of the file (`headless`, `[loop]`, `tools`, ...). The file is written to a temp
    /// file in the same directory and renamed, so a crash leaves the old or the new file.
    pub fn update_board(&self, initiative: &str, board: &BoardConfig, sha: &str) -> CbResult<()> {
        let p = self.agent_config_path(initiative);
        let text = std::fs::read_to_string(&p).map_err(io("read agent config"))?;
        let mut t: toml::Table =
            toml::from_str(&text).map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
        let b = toml::Value::try_from(board).map_err(|e| CbError::config(e.to_string()))?;
        t.insert("board".into(), b);
        t.insert("board_sha".into(), toml::Value::String(sha.to_string()));
        write_table_atomic(&p, &t)
    }

    /// Write `insecure_one_account = true` into the agent config and keep every other key of the
    /// file. The write is atomic, as in [`ConfigDir::update_board`].
    pub fn set_agent_one_account(&self, initiative: &str) -> CbResult<()> {
        let p = self.agent_config_path(initiative);
        let text = std::fs::read_to_string(&p).map_err(io("read agent config"))?;
        let mut t: toml::Table =
            toml::from_str(&text).map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
        t.insert("insecure_one_account".into(), toml::Value::Boolean(true));
        write_table_atomic(&p, &t)
    }

    pub fn load_agent(&self, initiative: &str) -> CbResult<AgentConfig> {
        let p = self.agent_config_path(initiative);
        let text = std::fs::read_to_string(&p).map_err(|e| {
            CbError::config(format!(
                "cannot read {} ({e}); run `crewboard join` first",
                p.display()
            ))
        })?;
        if let Some(e) = unknown_loop_key(&p, &text) {
            return Err(e);
        }
        let c: AgentConfig =
            toml::from_str(&text).map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
        c.board.validate()?;
        c.agent_name()?;
        if let Some(l) = &c.loop_cfg {
            l.validate()?;
        }
        Ok(c)
    }

    /// The initiative names that have an agent config here.
    pub fn agent_initiatives(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                // Only files that really are agent configs count: a stray `notes.toml` in
                // the directory must not turn "the only initiative" into "several".
                if let Some(stem) = n.strip_suffix(".toml")
                    && !stem.ends_with(".board")
                    && valid_initiative_name(stem)
                    && std::fs::read_to_string(e.path()).is_ok_and(|t| looks_like_agent_config(&t))
                {
                    v.push(stem.to_string());
                }
            }
        }
        v.sort();
        v
    }

    /// True if this run is headless (spec/agents#headless-coordinator): an agent config in this
    /// directory has `headless = true` (a guardrail).
    pub fn is_headless(&self) -> CbResult<bool> {
        self.has_headless_key()
    }

    /// True if an agent config in this directory has `headless = true`
    /// (spec/agents#headless-coordinator). The key is a top-level key that the owner or the setup
    /// writes once. The binary reads it from the file itself, so no flag and no prompt can switch
    /// it off. Every agent config of the directory counts (the strict answer wins). A value that is
    /// not a boolean is a config error (exit 4): the scope is never dropped because of a typo in
    /// the value.
    pub fn has_headless_key(&self) -> CbResult<bool> {
        for i in self.agent_initiatives() {
            let p = self.agent_config_path(&i);
            let text = std::fs::read_to_string(&p)
                .map_err(|e| CbError::config(format!("cannot read {} ({e})", p.display())))?;
            let t: toml::Table = toml::from_str(&text)
                .map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
            match t.get(HEADLESS_KEY) {
                None => {}
                Some(toml::Value::Boolean(true)) => return Ok(true),
                Some(toml::Value::Boolean(false)) => {}
                Some(_) => {
                    return Err(CbError::config(format!(
                        "config error in {}: `{HEADLESS_KEY}` must be true or false",
                        p.display()
                    )));
                }
            }
        }
        Ok(false)
    }

    /// The initiative names that have a `<initiative>.board.toml` here (written by `init`).
    pub fn board_initiatives(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if let Some(stem) = n.strip_suffix(".board.toml")
                    && valid_initiative_name(stem)
                {
                    v.push(stem.to_string());
                }
            }
        }
        v.sort();
        v
    }

    /// Pick the agent config: the named initiative, or the only one in the directory.
    pub fn load_agent_auto(&self, initiative: Option<&str>) -> CbResult<AgentConfig> {
        match initiative {
            Some(i) => self.load_agent(i),
            None => {
                let all = self.agent_initiatives();
                match all.as_slice() {
                    [one] => self.load_agent(one),
                    [] => Err(CbError::config(format!(
                        "no agent config in {}; run `crewboard join` first",
                        self.0.display()
                    ))),
                    many => Err(CbError::config(format!(
                        "several initiatives in {} ({}); pass --initiative",
                        self.0.display(),
                        many.join(", ")
                    ))),
                }
            }
        }
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
    fn the_agent_consent_is_written_by_the_helper_and_kept_by_a_rejoin() {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let a = AgentConfig {
            name: "claude@t1".into(),
            runtime: "claude-code".into(),
            host: "t1".into(),
            item_id: "PVTI_x".into(),
            board: BoardConfig::parse(GOOD).unwrap(),
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        };
        cd.save_agent(&a).unwrap();
        assert!(!cd.load_agent("demo").unwrap().insecure_one_account);
        cd.set_agent_one_account("demo").unwrap();
        assert!(cd.load_agent("demo").unwrap().insecure_one_account);
        // a re-join writes `false` (the field is skipped) and the key of the file stays
        cd.save_agent(&a).unwrap();
        assert!(cd.load_agent("demo").unwrap().insecure_one_account);
    }

    #[test]
    fn agent_config_round_trip_and_auto_pick() {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        assert!(cd.load_agent_auto(None).is_err(), "empty dir is refused");
        let board = BoardConfig::parse(GOOD).unwrap();
        let a = AgentConfig {
            name: "claude@t1".into(),
            runtime: "claude-code".into(),
            host: "t1".into(),
            item_id: "PVTI_x".into(),
            board: board.clone(),
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        };
        cd.save_agent(&a).unwrap();
        assert_eq!(cd.load_agent_auto(None).unwrap(), a);
        let mut b = a.clone();
        b.board.initiative = "other".into();
        cd.save_agent(&b).unwrap();
        assert!(
            cd.load_agent_auto(None).is_err(),
            "two initiatives need --initiative"
        );
        assert_eq!(cd.load_agent_auto(Some("other")).unwrap(), b);
    }

    #[test]
    fn save_agent_keeps_headless_first_and_every_key_it_does_not_own() {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let board = BoardConfig::parse(
            "initiative = \"x\"\nproject_owner = \"o\"\nproject_number = 1\nrepos = [\"o/r\"]\nowner_login = \"o\"\nregistry_issue = 1\n",
        )
        .unwrap();
        let a = AgentConfig {
            name: "claude@t1".into(),
            runtime: "claude-code".into(),
            host: "t1".into(),
            item_id: "PVTI_x".into(),
            board,
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        };
        cd.save_agent(&a).unwrap();
        let p = cd.agent_config_path("x");
        let fresh = std::fs::read_to_string(&p).unwrap();
        assert!(
            !fresh.contains("headless") && !fresh.contains("[loop]"),
            "{fresh}"
        );
        std::fs::write(
            &p,
            format!("headless = true\nfuture = 7\n{fresh}\n[loop]\ninterval = \"5s\"\n"),
        )
        .unwrap();
        let mut b = a.clone();
        b.host = "t2".into();
        cd.save_agent(&b).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("headless = true\n"), "{text}");
        assert!(text.contains("future = 7"), "{text}");
        assert!(cd.is_headless().unwrap());
        let c = cd.load_agent("x").unwrap();
        assert_eq!(c.host, "t2");
        assert_eq!(c.loop_cfg.unwrap().interval.as_deref(), Some("5s"));
        // update_board keeps the first line, too
        cd.update_board("x", &b.board, "abc").unwrap();
        assert!(
            std::fs::read_to_string(&p)
                .unwrap()
                .starts_with("headless = true\n")
        );
    }

    #[test]
    fn the_tool_list_of_the_agent_config_is_optional_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let mut a = AgentConfig {
            name: "claude@t1".into(),
            runtime: "claude-code".into(),
            host: "t1".into(),
            item_id: "PVTI_x".into(),
            board: BoardConfig::parse(GOOD).unwrap(),
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        };
        // an old config file without `tools` still loads, and writes none back
        cd.save_agent(&a).unwrap();
        let text = std::fs::read_to_string(cd.agent_config_path(&a.board.initiative)).unwrap();
        assert!(!text.contains("tools"), "{text}");
        assert_eq!(cd.load_agent_auto(None).unwrap().tools, None);
        // a configured list survives the round trip
        a.tools = Some(vec!["cargo".into(), "go".into()]);
        cd.save_agent(&a).unwrap();
        assert_eq!(
            cd.load_agent_auto(None).unwrap().tools,
            Some(vec!["cargo".to_string(), "go".to_string()])
        );
    }

    fn config_text_with(extra: &str) -> (tempfile::TempDir, ConfigDir) {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let a = AgentConfig {
            name: "claude@t1".into(),
            runtime: "claude-code".into(),
            host: "t1".into(),
            item_id: "PVTI_x".into(),
            board: BoardConfig::parse(GOOD).unwrap(),
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        };
        cd.save_agent(&a).unwrap();
        let p = cd.agent_config_path(&a.board.initiative);
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, format!("{text}\n{extra}\n")).unwrap();
        (dir, cd)
    }

    #[test]
    fn the_loop_table_is_optional_read_when_present_and_kept_on_save() {
        // absent: None, and nothing is written back
        let (_d, cd) = config_text_with("");
        assert_eq!(cd.load_agent_auto(None).unwrap().loop_cfg, None);
        // present: every key is read
        let (_d, cd) =
            config_text_with("[loop]\ninterval = \"30s\"\ntimeout = \"10m\"\nlog = \"/x/w.log\"");
        let a = cd.load_agent_auto(None).unwrap();
        let l = a.loop_cfg.clone().unwrap();
        assert_eq!(l.interval.as_deref(), Some("30s"));
        assert_eq!(l.timeout.as_deref(), Some("10m"));
        assert_eq!(l.log.as_deref(), Some("/x/w.log"));
        // control: a save of the loaded config keeps the table
        cd.save_agent(&a).unwrap();
        assert_eq!(cd.load_agent_auto(None).unwrap().loop_cfg, a.loop_cfg);
        // a partial table is fine
        let (_d, cd) = config_text_with("[loop]\ntimeout = \"5m\"");
        let l = cd.load_agent_auto(None).unwrap().loop_cfg.unwrap();
        assert_eq!((l.interval, l.timeout.as_deref()), (None, Some("5m")));
    }

    #[test]
    fn the_wake_table_is_optional_and_kept_on_save_and_rejoin() {
        let (_d, cd) = config_text_with("");
        assert_eq!(cd.load_agent_auto(None).unwrap().wake, None);
        let (_d, cd) = config_text_with("[wake]");
        let a = cd.load_agent_auto(None).unwrap();
        assert_eq!(a.wake, Some(crate::config::WakeConfig {}));
        // a save of the loaded config keeps the table
        cd.save_agent(&a).unwrap();
        assert_eq!(cd.load_agent_auto(None).unwrap().wake, a.wake);
        // control: a config saved WITHOUT the table (a join that did not carry it) still keeps
        // the table that the file holds
        let mut bare = a.clone();
        bare.wake = None;
        cd.save_agent(&bare).unwrap();
        assert_eq!(cd.load_agent_auto(None).unwrap().wake, a.wake);
    }

    #[test]
    fn an_unknown_key_in_the_wake_table_is_a_config_error() {
        let (_d, cd) = config_text_with("[wake]\nwork_dir = \"/w\"");
        let e = cd.load_agent_auto(None).unwrap_err().to_string();
        assert!(e.contains("work_dir"), "{e}");
    }

    #[test]
    fn an_invalid_loop_table_is_a_config_error() {
        for bad in [
            "[loop]\ninterval = \"soon\"",
            "[loop]\ntimeout = \"0m\"",
            "[loop]\nlog = \" \"",
        ] {
            let (_d, cd) = config_text_with(bad);
            let e = cd.load_agent_auto(None).unwrap_err();
            assert!(matches!(e, CbError::Config(_)), "{bad}: {e:?}");
        }
    }

    #[test]
    fn a_typo_in_the_loop_table_names_the_file_the_table_and_the_key() {
        let (_d, cd) = config_text_with("[loop]\nintervall = \"30s\"");
        let e = cd.load_agent_auto(None).unwrap_err();
        assert!(matches!(e, CbError::Config(_)), "{e:?}");
        let m = e.to_string();
        let file = cd.agent_config_path("demo").display().to_string();
        assert!(m.contains(&file), "{m}");
        assert!(m.contains("unknown key `intervall` in [loop]"), "{m}");
        assert!(m.contains("allowed: interval, timeout, log"), "{m}");
        assert!(!m.contains("run `crewboard join`"), "{m}");
        // with --initiative the same message
        let m2 = cd.load_agent("demo").unwrap_err().to_string();
        assert!(m2.contains("`intervall`"), "{m2}");
    }

    #[test]
    fn a_missing_file_still_says_run_join_and_a_valid_table_loads() {
        let dir = tempfile::tempdir().unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let e = cd.load_agent_auto(None).unwrap_err().to_string();
        assert!(e.contains("run `crewboard join` first"), "{e}");
        let e = cd.load_agent("demo").unwrap_err().to_string();
        assert!(e.contains("run `crewboard join` first"), "{e}");
        // control: a stray file that is no agent config does not count as one
        std::fs::write(dir.path().join("notes.toml"), "[loop]\nx = 1\n").unwrap();
        assert!(cd.agent_initiatives().is_empty());
        let (_d, cd) = config_text_with("[loop]\ninterval = \"30s\"\nlog = \"/x.log\"");
        assert!(cd.load_agent_auto(None).is_ok());
    }

    #[test]
    fn headless_is_a_top_level_key_that_only_true_switches_on() {
        let headless = |first_line: &str| {
            let (_d, cd) = config_text_with("");
            let p = cd.agent_config_path("demo");
            let text = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, format!("{first_line}\n{text}")).unwrap();
            // the config still loads: the key does not disturb the agent config
            assert!(cd.load_agent_auto(None).is_ok(), "{first_line}");
            cd.is_headless()
        };
        assert!(headless("headless = true").unwrap());
        // control: the check can say no
        assert!(!headless("headless = false").unwrap());
        assert!(!headless("# nothing").unwrap());
        // a value that is not a boolean is a config error, never a silent "not headless"
        for bad in ["headless = \"yes\"", "headless = 1"] {
            let e = headless(bad).unwrap_err();
            assert!(matches!(e, CbError::Config(_)), "{bad}: {e:?}");
        }
        // an empty directory is not headless
        let d = tempfile::tempdir().unwrap();
        assert!(!ConfigDir(d.path().to_path_buf()).is_headless().unwrap());
    }
}
