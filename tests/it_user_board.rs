//! Live test of a USER-owned project board. It talks to GitHub, so it runs only with
//! `CREWBOARD_IT_USER=1`. The test is `#[ignore]`d, so a plain `cargo test` shows it as
//! ignored, not ok. Run it with
//! `CREWBOARD_IT_USER=1 CREWBOARD_IT_CREW_LOGIN=<login> cargo test --test it_user_board -- --ignored`.
//!
//! It needs the owner's token (`gh auth token` of a user account; unset GH_TOKEN/GITHUB_TOKEN)
//! and creates ONE private project under that user, titled "crewboard user-owned IT (test)".
//! The project is reused on later runs and CLOSED (never deleted) at the end, also on a panic.
//! No repository is written: the test covers project, fields, views and the collaborator call.
//! The owner login comes from the token at run time. The crew account is a second user: set
//! `CREWBOARD_IT_CREW_LOGIN` to its login (the test is skipped without it).
//! No other board is touched.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use crewboard::board::{Board, OwnerKind, owner_info};
use crewboard::github::Client;
use crewboard::ops::init::{Report, add_collaborator, create_views, ensure_fields, ensure_project};
use crewboard::ops::new_client;
use serde_json::{Value, json};

const TITLE: &str = "crewboard user-owned IT (test)";

fn enabled() -> bool {
    if std::env::var("CREWBOARD_IT_USER").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("SKIPPED: set CREWBOARD_IT_USER=1 to run the user-owned board test");
    false
}

/// Closes the test project when dropped (also during a panic).
struct CloseOnDrop<'a> {
    client: &'a Client,
    project_id: String,
}

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        let r = set_closed(self.client, &self.project_id, true);
        if let Err(e) = r {
            eprintln!(
                "WARNING: could not close the test project {}: {e}",
                self.project_id
            );
        }
    }
}

fn set_closed(client: &Client, project_id: &str, closed: bool) -> Result<(), String> {
    client
        .graphql(
            "mutation($p:ID!,$c:Boolean!){updateProjectV2(input:{projectId:$p,closed:$c}){projectV2{closed}}}",
            json!({"p": project_id, "c": closed}),
        )
        .map_err(|e| e.to_string())
        .and_then(|d| {
            let now = d.pointer("/updateProjectV2/projectV2/closed").and_then(Value::as_bool);
            if now == Some(closed) {
                Ok(())
            } else {
                Err(format!("closed is {now:?}, wanted {closed}"))
            }
        })
}

fn view_names(client: &Client, board: &Board) -> Vec<String> {
    let d = client
        .graphql(
            "query($p:ID!){node(id:$p){... on ProjectV2{views(first:50){nodes{name layout}}}}}",
            json!({"p": board.project_id}),
        )
        .expect("views query");
    d.pointer("/node/views/nodes")
        .and_then(Value::as_array)
        .expect("views list")
        .iter()
        .filter_map(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

#[test]
#[ignore = "needs CREWBOARD_IT_USER=1 and a crew login"]
fn it_a_user_owned_board_gets_fields_views_and_a_collaborator_and_is_closed_after() {
    if !enabled() {
        return;
    }
    let Ok(crew_login) = std::env::var("CREWBOARD_IT_CREW_LOGIN") else {
        eprintln!("SKIPPED: set CREWBOARD_IT_CREW_LOGIN to the login of the crew account");
        return;
    };
    let crew_login = crew_login.as_str();
    let client = new_client().expect("client");
    // The owner is the user of the token (unset GH_TOKEN and GITHUB_TOKEN for the owner's token).
    let owner = client.viewer_login().expect("owner login from the token");
    let owner = owner.as_str();
    let info = owner_info(&client, owner).expect("owner info");
    assert_eq!(
        info.kind,
        OwnerKind::User,
        "{owner} is a user, not an organisation"
    );

    let mut rep = Report::default();
    let mut board = ensure_project(&client, owner, &info, TITLE, &mut rep).expect("project");
    let _guard = CloseOnDrop {
        client: &client,
        project_id: board.project_id.clone(),
    };
    set_closed(&client, &board.project_id, false).expect("reopen a reused project");
    assert_eq!(board.owner_kind, OwnerKind::User);
    assert!(!board.public, "the test project must be private");

    // fields: create, then a second run changes nothing
    ensure_fields(&client, &mut board, &mut rep).expect("fields");
    for f in [
        "Kind",
        "Status",
        "Agent",
        "Role",
        "State",
        "Gate",
        "Heartbeat",
        "Cursor",
    ] {
        assert!(board.fields.contains_key(f), "field {f} missing");
    }
    let status_ids: Vec<String> = board.fields["Status"]
        .options
        .iter()
        .map(|o| o.id.clone())
        .collect();
    ensure_fields(&client, &mut board, &mut rep).expect("fields again");
    let again: Vec<String> = board.fields["Status"]
        .options
        .iter()
        .map(|o| o.id.clone())
        .collect();
    assert_eq!(status_ids, again, "a second run keeps every option id");

    // views through the REST path of a user (by login)
    create_views(&client, &board, &mut rep).expect("views");
    let names = view_names(&client, &board);
    for v in ["Board", "By Agent", "Crew", "Needs Owner"] {
        assert!(
            names.iter().any(|n| n == v),
            "view {v} missing in {names:?}"
        );
    }
    let count = names.len();
    create_views(&client, &board, &mut rep).expect("views again");
    assert_eq!(
        view_names(&client, &board).len(),
        count,
        "a second run adds no view"
    );

    // the board loads again through the user root of GraphQL
    let reloaded = Board::load(&client, owner, board.number).expect("reload");
    assert_eq!(reloaded.project_id, board.project_id);

    // collaborator with the Write role (the crew account)
    let crew = client
        .get(&format!("users/{crew_login}"))
        .expect("crew user");
    let node = crew.body["node_id"]
        .as_str()
        .expect("crew node id")
        .to_string();
    add_collaborator(&client, &board, &node, crew_login, &mut rep).expect("collaborator");
    add_collaborator(&client, &board, &node, crew_login, &mut rep).expect("collaborator again");
}
