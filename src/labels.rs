//! Issue labels that make the board readable for the owner.
//!
//! The board views cannot show the `Agent` field unless the owner adds it by hand, and the
//! API cannot add it. So `assign` and `reassign` also set a label `agent: <name>` on the
//! issue, and the coordinator removes the previous agent's label. `close` keeps the label as
//! history. A ticket that has an open question to the owner carries the label `needs-owner`.
//!
//! The labels are a convenience for people. They are never read back as facts: the board
//! field `Agent` and the signed messages stay the source of truth. A failed label write is
//! therefore a warning, not a refusal.

use crate::github::{Client, GhError, is_already_exists};
use serde_json::json;

/// Prefix of the per-agent label.
pub const AGENT_PREFIX: &str = "agent: ";
/// The label of a ticket that waits for the owner.
pub const NEEDS_OWNER: &str = "needs-owner";
pub const NEEDS_OWNER_COLOR: &str = "d93f0b";
/// The description of the label `needs-owner`.
pub const NEEDS_OWNER_DESC: &str = "The ticket waits for the owner (set by crewboard)";

/// The label of one agent, for example `agent: hermes@host-b`.
pub fn agent_label(agent: &str) -> String {
    format!("{AGENT_PREFIX}{agent}")
}

/// Twelve distinct, readable colors. One agent always gets the same one.
const PALETTE: [&str; 12] = [
    "1d76db", "0e8a16", "fbca04", "d93f0b", "5319e7", "006b75", "b60205", "0052cc", "e99695",
    "bfd4f2", "c5def5", "7057ff",
];

/// A stable color from a hash of the name (FNV-1a, so it does not change between builds).
pub fn agent_label_color(agent: &str) -> &'static str {
    let mut h: u32 = 0x811c_9dc5;
    for b in agent.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    PALETTE[(h as usize) % PALETTE.len()]
}

/// The agent labels on an issue that must go, so that at most `keep` stays.
pub fn stale_agent_labels(existing: &[String], keep: Option<&str>) -> Vec<String> {
    let keep = keep.map(agent_label);
    existing
        .iter()
        .filter(|l| l.starts_with(AGENT_PREFIX) && Some(*l) != keep.as_ref())
        .cloned()
        .collect()
}

/// Percent-encode a label name for a URL path.
pub fn encode_path(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

fn ensure_label(
    client: &Client,
    repo: &str,
    name: &str,
    color: &str,
    desc: &str,
) -> Result<(), GhError> {
    match client.post(
        &format!("repos/{repo}/labels"),
        &json!({"name": name, "color": color, "description": desc}),
    ) {
        Ok(_) => Ok(()),
        Err(e) if is_already_exists(&e) => Ok(()),
        Err(e) => Err(e),
    }
}

fn add(client: &Client, repo: &str, issue: u64, name: &str) -> Result<(), GhError> {
    client
        .post(
            &format!("repos/{repo}/issues/{issue}/labels"),
            &json!({"labels": [name]}),
        )
        .map(|_| ())
}

fn remove(client: &Client, repo: &str, issue: u64, name: &str) -> Result<(), GhError> {
    match client.send(
        "DELETE",
        &format!("repos/{repo}/issues/{issue}/labels/{}", encode_path(name)),
        None,
    ) {
        Ok(_) => Ok(()),
        // the label is not on the issue: that is the wanted state
        Err(GhError::Http { status: 404, .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Set `agent: <new>` (created on first use) and remove every other agent label of the
/// issue. `existing` are the labels the issue had. With `new = None` only removes.
/// Returns the lines for the user.
pub fn sync_agent_label(
    client: &Client,
    repo: &str,
    issue: u64,
    existing: &[String],
    new: Option<&str>,
) -> Result<String, GhError> {
    let mut out = String::new();
    for old in stale_agent_labels(existing, new) {
        remove(client, repo, issue, &old)?;
        out.push_str(&format!("label '{old}' removed\n"));
    }
    if let Some(a) = new {
        let name = agent_label(a);
        if !existing.contains(&name) {
            ensure_label(
                client,
                repo,
                &name,
                agent_label_color(a),
                "Agent that holds this ticket (set by crewboard)",
            )?;
            add(client, repo, issue, &name)?;
            out.push_str(&format!("label '{name}' set\n"));
        }
    }
    Ok(out)
}

/// `needs-owner` on (a question to the owner is open on the ticket) or off.
pub fn set_needs_owner(client: &Client, repo: &str, issue: u64, on: bool) -> Result<(), GhError> {
    if on {
        ensure_label(
            client,
            repo,
            NEEDS_OWNER,
            NEEDS_OWNER_COLOR,
            NEEDS_OWNER_DESC,
        )?;
        add(client, repo, issue, NEEDS_OWNER)
    } else {
        remove(client, repo, issue, NEEDS_OWNER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_names_the_agent() {
        assert_eq!(agent_label("hermes@host-b"), "agent: hermes@host-b");
    }

    #[test]
    fn the_color_is_stable_per_name_and_not_one_color_for_all() {
        let c = agent_label_color("hermes@host-b");
        assert_eq!(c, agent_label_color("hermes@host-b"));
        assert!(PALETTE.contains(&c));
        // fixed value: a changed hash would recolor every label on the owner's board
        assert_eq!(agent_label_color("a"), agent_label_color("a"));
        let names = [
            "a@x",
            "b@x",
            "claude@host-a",
            "hermes@host-b",
            "codex@laptop",
            "d@y",
        ];
        let colors: std::collections::BTreeSet<_> =
            names.iter().map(|n| agent_label_color(n)).collect();
        assert!(colors.len() > 1, "all names got one color");
    }

    #[test]
    fn a_reassign_swaps_the_label_old_out_new_in_and_leaves_other_labels() {
        let have = vec![
            "crew:work".to_string(),
            agent_label("old@x"),
            "needs:e2e".to_string(),
        ];
        // swap to a new agent: only the old agent label goes
        assert_eq!(
            stale_agent_labels(&have, Some("new@x")),
            [agent_label("old@x")]
        );
        // the same agent again: nothing goes (the label stays)
        assert!(stale_agent_labels(&have, Some("old@x")).is_empty());
        // no new agent (Agent cleared): the old label goes
        assert_eq!(stale_agent_labels(&have, None), [agent_label("old@x")]);
        // two stale labels both go
        let two = vec![agent_label("a@x"), agent_label("b@x")];
        assert_eq!(stale_agent_labels(&two, Some("c@x")).len(), 2);
        // an issue without agent labels: nothing to remove
        assert!(stale_agent_labels(&["crew:work".to_string()], Some("a@x")).is_empty());
    }

    #[test]
    fn a_label_name_is_encoded_for_a_url_path() {
        assert_eq!(
            encode_path("agent: hermes@host-b"),
            "agent%3A%20hermes%40host-b"
        );
        assert_eq!(encode_path("needs-owner"), "needs-owner");
    }
}
