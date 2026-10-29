//! One row per agent as `status` and `status --queue` show it: the registry facts, the derived
//! liveness and busy flag, and the capabilities field read as tags and tier.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRow {
    pub name: String,
    pub approved: bool,
    pub rejected: bool,
    pub trusted: bool,
    pub role: Option<String>,
    pub state: Option<String>,
    pub heartbeat: Option<String>,
    /// Seconds since the heartbeat, if there is one.
    pub heartbeat_age: Option<i64>,
    pub runtime: Option<String>,
    pub host: Option<String>,
    pub item_id: Option<String>,
    /// The `Capabilities` field as stored (spec/profiles), if the board has it and it is set.
    pub capabilities: Option<String>,
    /// Derived (spec/board#fields): alive, stale, dead, or `-` for an agent that does not run.
    pub liveness: String,
    /// Derived, never stored: the keys of the tickets that are In Progress and assigned to this
    /// agent. The agent is busy when this is not empty (spec/board#lifecycle).
    pub busy: Vec<String>,
}

pub(crate) fn caps_of_row(r: &AgentRow) -> crate::capabilities::Caps {
    crate::capabilities::Caps::parse(r.capabilities.as_deref().unwrap_or(""))
}

/// The declared tags of an agent (facts are not tags).
pub fn declared_tags(r: &AgentRow) -> Vec<String> {
    caps_of_row(r).declared.into_iter().collect()
}

pub fn tier_of(r: &AgentRow) -> Option<String> {
    caps_of_row(r).tier
}
