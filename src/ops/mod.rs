//! Command implementations. Each file is one command (or a small family).
//!
//! `cli.rs` dispatches here. An operation reads facts (board, comments), asks the pure rules,
//! and only then writes. The shared contexts and the token checks live in `crate::ctx`.

pub mod acted;
pub mod agent;
pub(crate) mod agent_row;
pub(crate) mod agents;
pub(crate) mod approve;
pub(crate) mod audit;
pub(crate) mod behind;
pub(crate) mod board_refresh;
pub(crate) mod crew_token;
pub(crate) mod doctor;
pub(crate) mod duties;
pub(crate) mod force_stop;
pub mod gate_cmd;
pub mod init;
pub(crate) mod init_ci;
pub(crate) mod init_rule;
pub(crate) mod join;
pub(crate) mod late;
pub(crate) mod lifecycle;
pub mod lock_cmd;
pub(crate) mod log_check;
pub(crate) mod mine;
pub(crate) mod monitor;
pub(crate) mod needs_owner_sync;
pub(crate) mod pr_state;
pub(crate) mod profile;
pub(crate) mod prune;
pub(crate) mod queue;
mod registry_post;
pub(crate) mod repo_migration;
pub(crate) mod report_status;
pub(crate) mod restart;
pub(crate) mod review_wait;
pub(crate) mod say;
pub mod selftest;
pub(crate) mod skill_cmd;
pub(crate) mod start;
pub mod status;
pub(crate) mod tickets;
pub mod update_cmd;
pub(crate) mod watch;

use crate::config::{BoardConfig, INITIATIVE_FILE, split_repo};
use crate::error::{CbError, CbResult};

// The contexts live in `crate::ctx` (below the commands); the commands keep the short paths.
pub use crate::ctx::{
    AgentCtx, Ctx, Global, IdentityMode, World, judge_agent, judge_agent_account, judge_owner,
    load_world, new_client, new_owner_client, require_agent, require_owner,
};

/// Find the board config for a command that is not an agent command (owner approve):
/// `--initiative-file`, else the agent config or board config in the config dir, else
/// `./.crewboard.toml`.
pub fn resolve_board_config(g: &Global) -> CbResult<BoardConfig> {
    if let Some(p) = &g.initiative_file {
        let t = std::fs::read_to_string(p)
            .map_err(|e| CbError::config(format!("{}: {e}", p.display())))?;
        return BoardConfig::parse(&t);
    }
    let cd = g.dir()?;
    if let Some(i) = &g.initiative {
        if let Ok(a) = cd.load_agent(i) {
            return Ok(a.board);
        }
        if let Ok(t) = std::fs::read_to_string(cd.board_config_path(i)) {
            return BoardConfig::parse(&t);
        }
    } else if let Ok(a) = cd.load_agent_auto(None) {
        return Ok(a.board);
    } else if let [one] = cd.board_initiatives().as_slice()
        && let Ok(t) = std::fs::read_to_string(cd.board_config_path(one))
    {
        return BoardConfig::parse(&t);
    }
    let local = std::path::Path::new(INITIATIVE_FILE);
    if let Ok(t) = std::fs::read_to_string(local) {
        return BoardConfig::parse(&t);
    }
    Err(CbError::config(format!(
        "no board config: pass --initiative-file, or --initiative with a config in {}, or run in a checkout that has {INITIATIVE_FILE}",
        cd.0.display()
    )))
}

/// `12` (main repo) or `owner/repo#12`.
pub fn parse_issue_arg(s: &str, main_repo: &str) -> CbResult<(String, u64)> {
    if let Some((repo, n)) = s.split_once('#') {
        split_repo(repo)?;
        let n: u64 = n
            .parse()
            .map_err(|_| CbError::config(format!("bad issue number in '{s}'")))?;
        return Ok((repo.to_string(), n));
    }
    let n: u64 = s
        .parse()
        .map_err(|_| CbError::config(format!("'{s}' is not an issue number or owner/repo#n")))?;
    Ok((main_repo.to_string(), n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_args() {
        assert_eq!(parse_issue_arg("12", "a/b").unwrap(), ("a/b".into(), 12));
        assert_eq!(parse_issue_arg("c/d#7", "a/b").unwrap(), ("c/d".into(), 7));
        for bad in ["", "x", "c#7", "c/d#x", "c/d#"] {
            assert!(parse_issue_arg(bad, "a/b").is_err(), "{bad}");
        }
    }
}
