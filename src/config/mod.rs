//! Configuration (spec/overview#where-things-live, spec/overview#build-and-distribution).
//!
//! - `.crewboard.toml` in the initiative's main repo: [`BoardConfig`]. No secrets.
//! - `<config-dir>/<initiative>.toml`: [`AgentConfig`], one agent per initiative per
//!   config dir (a copy of the board config plus the agent's name and item id).
//! - `<config-dir>/keys/<agent>`: the Ed25519 private key, OpenSSH format, mode 600.
//! - `<config-dir>/state/<initiative>.<agent>.json`: see `crate::state`.

//!
//! Split by concern: `board` (the board TOML), `agent` (the agent TOML and its atomic
//! write), `dir` (the directory layout) and `keys` (the key store and file modes). Every
//! public item is re-exported here, so callers use `crate::config::<Item>`.

mod agent;
mod board;
mod dir;
mod keys;

pub use agent::{AgentConfig, HEADLESS_KEY, LOOP_KEYS, LoopConfig, WakeConfig};
pub use board::{
    BoardConfig, IDENTITY_KEYS, INITIATIVE_FILE, LATE_OFF, LateConfig, RequiredTool, StaleConfig,
    split_repo, valid_initiative_name,
};
pub use dir::ConfigDir;
pub use keys::{load_key, load_or_create_key, public_line};

use crate::error::CbError;

/// `skip_serializing_if` of a boolean key that is written only when it is set.
fn is_false(b: &bool) -> bool {
    !*b
}

fn io(what: &'static str) -> impl Fn(std::io::Error) -> CbError {
    move |e| CbError::config(format!("{what}: {e}"))
}
