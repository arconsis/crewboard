//! GitHub's built-in project workflows (spec/board#built-in-project-workflows).
//!
//! A new GitHub Project has built-in workflows, and several of them are on by default. Some of
//! them write the `Status` field or add and archive board items. crewboard treats `Status` as
//! its own state, so such a workflow is a second writer behind crewboard. Example: "Pull request
//! linked to issue" moves a Blocked ticket to In Progress when a PR body names it.
//!
//! The GraphQL API reads the workflows (`ProjectV2.workflows`: `id number name enabled`). No
//! mutation turns one on or off, and none creates one; only `deleteProjectV2Workflow` exists.
//! `init` deletes the four built-ins of [`DELETED`] with it. The delete is
//! one way for the API. `selftest` and `doctor --board` only warn about a workflow that is
//! still on: for one of the four the fix is `crewboard init`, for any other writer
//! the owner turns it off in the project settings.
//!
//! The watch marks a `status-changed` event that GitHub automation made
//! (spec/events#status-changes-by-github-automation). The `creator` of a field value is not the
//! last writer, so only the issue timeline (`project_v2_item_status_changed`, `actor.login`)
//! names who changed `Status`.

use crate::board::{Board, graphql_root};
use crate::github::{Client, GhError};
use serde_json::{Value, json};

/// The four built-in workflows that write `Status` on every new project. `init` deletes them. The names are GitHub's names (checked on 2026-10-25); the match
/// ignores case and spacing ([`is_deleted`]).
pub const DELETED: [&str; 4] = [
    "Item added to project",
    "Item closed",
    "Pull request merged",
    "Pull request linked to issue",
];

/// The fix that a warning for one of [`DELETED`] names.
pub const DELETE_FIX: &str = "crewboard init";

/// The built-in workflows that write `Status` or the board items, with what they do: the four of
/// [`DELETED`], and the ones that exist only when the owner added them by hand. A warning names
/// the workflow that is on.
///
/// Two built-ins are decided on purpose and are NOT in the list:
///
/// - "Auto-add sub-issues to project" stays, and `init` does not delete it. It adds the
///   sub-issues of a board item as new items without `Kind`, `Gate` or a signed info comment,
///   so `status` does not list them; they are inert for crewboard.
/// - "Auto-close issue" stays too. It writes no field and no item: it closes the issue when
///   `Status` becomes Done. crewboard writes Done only for finished, verified work, so a
///   closed issue is the expected end state.
pub const STATUS_WRITERS: &[(&str, &str)] = &[
    ("Item added to project", "sets the Status of every new item"),
    (
        "Item reopened",
        "sets the Status when an issue or PR opens again",
    ),
    ("Item closed", "sets the Status when an issue or PR closes"),
    (
        "Pull request linked to issue",
        "sets the Status of an issue when a PR body links it",
    ),
    ("Pull request merged", "sets the Status when a PR merges"),
    (
        "Code changes requested",
        "sets the Status when a review requests changes",
    ),
    (
        "Code review approved",
        "sets the Status when a review approves",
    ),
    ("Auto-archive items", "archives board items"),
    (
        "Auto-add to project",
        "adds issues and PRs to the board that no crewboard command made",
    ),
];

/// The login of GitHub's project automation. The timeline shows it with the `[bot]` suffix.
pub const AUTOMATION_LOGIN: &str = "github-project-automation";

/// The text that the watch adds to a `status-changed` event that GitHub automation made.
pub const AUTOMATION_MARK: &str = "(by GitHub automation)";

/// One workflow of a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
}

/// The workflows in a `workflows(first:30){nodes{id number name enabled}}` answer.
pub fn parse_workflows(v: &Value) -> Result<Vec<Workflow>, GhError> {
    let nodes = v
        .pointer("/workflows/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| GhError::Shape("project has no workflows.nodes".into()))?;
    Ok(nodes
        .iter()
        .filter_map(|n| {
            Some(Workflow {
                id: n.get("id")?.as_str()?.to_string(),
                name: n.get("name")?.as_str()?.to_string(),
                enabled: n.get("enabled")?.as_bool()?,
            })
        })
        .collect())
}

/// Read the workflows of the board. The owner type picks the GraphQL root, as `Board::load`
/// does.
pub fn read(client: &Client, board: &Board) -> Result<Vec<Workflow>, GhError> {
    let root = graphql_root(&board.owner_kind);
    let q = format!(
        "query($l:String!,$n:Int!){{{root}(login:$l){{projectV2(number:$n){{workflows(first:30){{nodes{{id number name enabled}}}}}}}}}}"
    );
    let d = client.graphql(&q, json!({"l": board.owner, "n": board.number}))?;
    let p = d
        .pointer(&format!("/{root}/projectV2"))
        .filter(|p| p.is_object())
        .ok_or_else(|| GhError::Shape("project not found in the workflows answer".into()))?;
    parse_workflows(p)
}

/// Is this workflow (by its name) one of [`DELETED`]? The match ignores case, and trims and
/// collapses spaces, so that a small change of GitHub's spelling still finds it.
pub fn is_deleted(name: &str) -> bool {
    let n = squash(name);
    DELETED.iter().any(|d| squash(d) == n)
}

fn squash(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The workflows of the list that `init` deletes.
pub fn to_delete(workflows: &[Workflow]) -> Vec<&Workflow> {
    workflows.iter().filter(|w| is_deleted(&w.name)).collect()
}

/// Delete one workflow (`deleteProjectV2Workflow`). One way: the API cannot create it again.
pub fn delete(client: &Client, workflow: &Workflow) -> Result<(), GhError> {
    client.graphql(
        "mutation($w:ID!){deleteProjectV2Workflow(input:{workflowId:$w}){deletedWorkflowId}}",
        json!({"w": workflow.id}),
    )?;
    Ok(())
}

/// Delete every workflow of [`DELETED`] that the board has, and call `on_deleted` with the name
/// of each one after its delete worked. A workflow that is not there is already gone. Returns
/// the workflows that remain. On the first failure it stops: the ones before it stay deleted.
pub fn delete_status_writers(
    client: &Client,
    board: &Board,
    mut on_deleted: impl FnMut(&str),
) -> Result<Vec<Workflow>, GhError> {
    let all = read(client, board)?;
    for w in to_delete(&all) {
        delete(client, w)?;
        on_deleted(&w.name);
    }
    Ok(all.into_iter().filter(|w| !is_deleted(&w.name)).collect())
}

/// The one-line explanation for a failed delete step, with the manual fallback.
pub fn manual_fallback(board_url: &str, why: &str) -> String {
    format!(
        "could not delete the built-in project workflows that write Status ({why}). Do it by hand: open {board_url}/workflows, then for each of {} select it, open its menu, and delete it (or switch it off); then run the command again",
        DELETED.map(|n| format!("\"{n}\"")).join(", ")
    )
}

/// One warning line per enabled workflow of [`STATUS_WRITERS`]. Empty when none is enabled. A
/// workflow of [`DELETED`] names `crewboard init` as the fix; any other writer names
/// the page where the owner turns it off.
pub fn warnings(workflows: &[Workflow], board_url: &str) -> String {
    let mut out = String::new();
    for w in workflows.iter().filter(|w| w.enabled) {
        let Some((name, does)) = STATUS_WRITERS
            .iter()
            .find(|(n, _)| squash(n) == squash(&w.name))
        else {
            continue;
        };
        let fix = if is_deleted(name) {
            format!("Delete it: {DELETE_FIX}")
        } else {
            format!("Turn it off: {board_url}/workflows, select \"{name}\", switch it off")
        };
        out.push_str(&format!(
            "WARNING: built-in project workflow \"{name}\" is on: it {does}, behind crewboard. {fix} (docs/design/spec/board.md#built-in-project-workflows)\n"
        ));
    }
    out
}

/// The check of `selftest` and `doctor --board` (and of `init` for what it does not delete). It never fails: a read error gives one
/// "could not check" warning.
pub fn check(client: &Client, board: &Board) -> String {
    match read(client, board) {
        Ok(w) => warnings(&w, &board.url),
        Err(e) => could_not_check(&format!("{}/workflows", board.url), &e.to_string()),
    }
}

/// The warning for a check that could not run (a token without the scope, an old token, a
/// board that is not readable). `place` says where the owner checks by hand.
pub fn could_not_check(place: &str, why: &str) -> String {
    format!(
        "WARNING: crewboard could not check the built-in project workflows ({why}). Check them by hand in {place}: delete or switch off every workflow that writes Status or adds items (docs/design/spec/board.md#built-in-project-workflows)\n"
    )
}

/// Is this login GitHub's project automation (`github-project-automation` or
/// `github-project-automation[bot]`)?
pub fn is_automation(login: &str) -> bool {
    login
        .strip_suffix("[bot]")
        .unwrap_or(login)
        .eq_ignore_ascii_case(AUTOMATION_LOGIN)
}

/// The actor of the last `project_v2_item_status_changed` event in a timeline page. `None`: the
/// page has no such event.
pub fn last_status_actor(timeline: &Value) -> Option<String> {
    timeline
        .as_array()?
        .iter()
        .rev()
        .find(|e| e.get("event").and_then(Value::as_str) == Some("project_v2_item_status_changed"))
        .and_then(|e| e.pointer("/actor/login").and_then(Value::as_str))
        .map(str::to_string)
}

/// Did GitHub automation make the last `Status` change of this issue? One REST call
/// (`GET /repos/{r}/issues/{n}/timeline`, 100 events). `Ok(None)`: it cannot tell, because the
/// page has no status event or the timeline has more than 100 events (the newest are on a later
/// page, and crewboard reads one page only).
pub fn automation_changed_status(
    client: &Client,
    repo: &str,
    issue: u64,
) -> Result<Option<bool>, GhError> {
    let r = client.get(&format!(
        "{}/issues/{issue}/timeline?per_page=100",
        crate::board::repo_path(repo).map_err(|e| GhError::Shape(e.to_string()))?
    ))?;
    if !r.body.is_array() {
        return Err(GhError::Shape("issue timeline is not an array".into()));
    }
    if r.link_next.is_some() {
        return Ok(None);
    }
    Ok(last_status_actor(&r.body).map(|a| is_automation(&a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf(name: &str, enabled: bool) -> Workflow {
        Workflow {
            id: format!("PVTW_{}", name.replace(' ', "_")),
            name: name.into(),
            enabled,
        }
    }

    #[test]
    fn an_enabled_status_writer_warns_once_and_a_disabled_one_does_not() {
        let url = "https://github.com/orgs/o/projects/1";
        let w = warnings(
            &[
                wf("Item closed", true),
                wf("Pull request linked to issue", true),
                wf("Pull request merged", false),
                wf("Auto-close issue", true),
            ],
            url,
        );
        assert_eq!(w.lines().count(), 2, "{w}");
        assert!(w.contains("\"Item closed\" is on"), "{w}");
        assert!(w.contains("\"Pull request linked to issue\" is on"), "{w}");
        // the four that init deletes name the command as the fix, not the UI
        assert!(w.lines().all(|l| l.contains("crewboard init")), "{w}");
        assert!(!w.contains("/workflows"), "{w}");
        assert!(!w.contains("Pull request merged"), "{w}");
        // Auto-close issue writes no Status (see STATUS_WRITERS)
        assert!(!w.contains("Auto-close"), "{w}");
        // all of them off: no warning
        let off: Vec<Workflow> = STATUS_WRITERS.iter().map(|(n, _)| wf(n, false)).collect();
        assert_eq!(warnings(&off, url), "");
        // all of them on: one line each
        let on: Vec<Workflow> = STATUS_WRITERS.iter().map(|(n, _)| wf(n, true)).collect();
        assert_eq!(warnings(&on, url).lines().count(), STATUS_WRITERS.len());
    }

    #[test]
    fn the_six_defaults_of_a_new_board_give_four_warnings() {
        let defaults = [
            "Item added to project",
            "Item closed",
            "Pull request linked to issue",
            "Pull request merged",
            "Auto-close issue",
            "Auto-add sub-issues to project",
        ];
        let w: Vec<Workflow> = defaults.iter().map(|n| wf(n, true)).collect();
        let text = warnings(&w, "u");
        assert_eq!(text.lines().count(), 4, "{text}");
        // the two that stay never warn
        assert!(!text.contains("Auto-close"), "{text}");
        assert!(!text.contains("sub-issues"), "{text}");
    }

    #[test]
    fn a_writer_that_is_not_deleted_names_the_ui_page() {
        let w = warnings(&[wf("Item reopened", true)], "https://x/projects/1");
        assert!(w.contains("https://x/projects/1/workflows"), "{w}");
        assert!(!w.contains("crewboard init"), "{w}");
    }

    #[test]
    fn the_name_match_ignores_case_and_spacing_and_knows_exactly_four() {
        for n in DELETED {
            assert!(is_deleted(n), "{n}");
        }
        assert!(is_deleted("  item   CLOSED "));
        assert!(is_deleted("pull request linked to issue"));
        // kept on purpose, or not a status writer of the four
        for n in [
            "Auto-close issue",
            "Auto-add sub-issues to project",
            "Item reopened",
            "Item closed by someone",
            "Pull request",
            "",
        ] {
            assert!(!is_deleted(n), "{n}");
        }
        let all = [
            wf("Auto-close issue", true),
            wf("Item closed", false),
            wf("Pull request merged", true),
            wf("Auto-add sub-issues to project", true),
        ];
        let names: Vec<&str> = to_delete(&all).iter().map(|w| w.name.as_str()).collect();
        // an off workflow is deleted too: it still exists
        assert_eq!(names, ["Item closed", "Pull request merged"]);
        assert!(to_delete(&[]).is_empty());
    }

    #[test]
    fn the_manual_fallback_names_the_page_and_all_four() {
        let t = manual_fallback("https://x/projects/1", "no scope");
        assert!(t.contains("https://x/projects/1/workflows"), "{t}");
        assert!(t.contains("no scope"), "{t}");
        for n in DELETED {
            assert!(t.contains(n), "{t}");
        }
    }

    #[test]
    fn the_workflows_answer_is_parsed_and_a_wrong_shape_is_an_error() {
        let v = json!({"workflows": {"nodes": [
            {"id": "W1", "number": 1, "name": "Item closed", "enabled": true},
            {"id": "W2", "number": 2, "name": "Item reopened", "enabled": false},
        ]}});
        let got = parse_workflows(&v).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].id.as_str(), got[0].name.as_str()),
            ("W1", "Item closed")
        );
        assert!(got[0].enabled && !got[1].enabled);
        assert!(parse_workflows(&json!({"workflows": null})).is_err());
    }

    #[test]
    fn the_automation_login_is_known_with_and_without_the_bot_suffix() {
        assert!(is_automation("github-project-automation[bot]"));
        assert!(is_automation("github-project-automation"));
        assert!(!is_automation("crew-account"));
        assert!(!is_automation("github-actions[bot]"));
    }

    #[test]
    fn the_last_status_change_names_the_actor() {
        let t = json!([
            {"event": "project_v2_item_status_changed", "actor": {"login": "crew-account"}},
            {"event": "commented", "actor": {"login": "someone"}},
            {"event": "project_v2_item_status_changed", "actor": {"login": "github-project-automation[bot]"}},
            {"event": "labeled", "actor": {"login": "crew-account"}},
        ]);
        assert_eq!(
            last_status_actor(&t).as_deref(),
            Some("github-project-automation[bot]")
        );
        assert_eq!(last_status_actor(&json!([{"event": "commented"}])), None);
        assert_eq!(last_status_actor(&json!({"message": "x"})), None);
    }
}
