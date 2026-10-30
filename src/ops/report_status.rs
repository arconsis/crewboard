//! `report-status` (spec/board#status-updates): the coordinator posts a project status update.
//!
//! GitHub shows the token user as the creator, so the first line of the text names the agent:
//! `from=<agent name>`. The update is read back by id and checked.

use super::AgentCtx;
use super::tickets::{load_world_cached, me_of};
use crate::base::ids;
use crate::error::{CbError, CbResult};
use crate::github::GhError;
use serde_json::{Value, json};

/// `on-track` -> `ON_TRACK`, and so on; `None` for anything else.
pub fn api_status(s: &str) -> Option<&'static str> {
    Some(match s {
        "on-track" => "ON_TRACK",
        "at-risk" => "AT_RISK",
        "off-track" => "OFF_TRACK",
        "inactive" => "INACTIVE",
        "complete" => "COMPLETE",
        _ => return None,
    })
}

/// The text that is stored: `from=<agent>` first, then the text.
pub fn body_of(agent: &str, text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        format!("from={agent}")
    } else {
        format!("from={agent}\n\n{t}")
    }
}

/// The agent named in the first line `from=<agent>` of a status update body.
pub fn from_line(body: &str) -> Option<&str> {
    body.lines().next()?.strip_prefix("from=").map(str::trim)
}

pub fn run(ac: &AgentCtx, state: &str, text: &str) -> CbResult<String> {
    let status = api_status(state).ok_or_else(|| {
        CbError::config(format!(
            "'{state}' is not a status: use on-track, at-risk, off-track, inactive or complete"
        ))
    })?;
    let world = load_world_cached(ac)?;
    let me = me_of(ac, &world)?;
    if !me.actor.is_coordinator() {
        return Err(CbError::refused(
            ids::REPORT_STATUS_COORDINATOR_ONLY,
            "report-status is a coordinator command (docs/design/spec/board.md#status-updates)",
        ));
    }
    let body = body_of(&me.name, text);
    super::say::check_body_text(&body)?;
    let d = ac.ctx.client.graphql(
        "mutation($p:ID!,$s:ProjectV2StatusUpdateStatus!,$b:String!){createProjectV2StatusUpdate(input:{projectId:$p,status:$s,body:$b}){statusUpdate{id status body createdAt}}}",
        json!({"p": ac.ctx.board.project_id, "s": status, "b": body}),
    )?;
    let id = d
        .pointer("/createProjectV2StatusUpdate/statusUpdate/id")
        .and_then(Value::as_str)
        .ok_or_else(|| GhError::Shape("createProjectV2StatusUpdate returned no id".into()))?
        .to_string();
    // Read it back: the newest update must be this one, with the from= line in its body.
    let r = ac.ctx.client.graphql(
        "query($p:ID!){node(id:$p){... on ProjectV2{statusUpdates(first:1,orderBy:{field:CREATED_AT,direction:DESC}){nodes{id status body createdAt}}}}}",
        json!({"p": ac.ctx.board.project_id}),
    )?;
    let node = r
        .pointer("/node/statusUpdates/nodes/0")
        .ok_or_else(|| GhError::Shape("project has no status update after the create".into()))?;
    let back_id = node.get("id").and_then(Value::as_str).unwrap_or("");
    let back_body = node.get("body").and_then(Value::as_str).unwrap_or("");
    if back_id != id || from_line(back_body) != Some(me.name.as_str()) {
        return Err(CbError::refused(
            ids::REPORT_STATUS_READBACK,
            format!(
                "the status update {id} was created, but the newest update reads back as {back_id} with first line '{}'",
                back_body.lines().next().unwrap_or("")
            ),
        ));
    }
    Ok(format!(
        "status update {id}: {status}; first line {}\n{}\n",
        back_body.lines().next().unwrap_or(""),
        node.get("createdAt").and_then(Value::as_str).unwrap_or("")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_states_map_to_the_github_enum_and_others_are_refused() {
        assert_eq!(api_status("on-track"), Some("ON_TRACK"));
        assert_eq!(api_status("at-risk"), Some("AT_RISK"));
        assert_eq!(api_status("off-track"), Some("OFF_TRACK"));
        assert_eq!(api_status("complete"), Some("COMPLETE"));
        assert_eq!(api_status("inactive"), Some("INACTIVE"));
        for bad in ["", "ON_TRACK", "green", "on track"] {
            assert_eq!(api_status(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_first_line_names_the_agent_and_reads_back() {
        let b = body_of("claude@host-a", "  M3 is half done.\nTwo tickets open.  ");
        assert_eq!(b.lines().next(), Some("from=claude@host-a"));
        assert!(b.ends_with("Two tickets open."));
        assert_eq!(from_line(&b), Some("claude@host-a"));
        // no text: the line alone; a body without the line names nobody
        assert_eq!(body_of("a@b", "   "), "from=a@b");
        assert_eq!(from_line("just text"), None);
        assert_eq!(
            from_line("\nfrom=a@b"),
            None,
            "the line must be the FIRST line"
        );
        assert_eq!(from_line(""), None);
    }
}
