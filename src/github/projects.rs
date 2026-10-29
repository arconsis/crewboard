//! The raw calls that `init` makes to find or create a GitHub Project (v2): the project itself,
//! its repositories, its fields and its views. The calls decide nothing: `ops::init` says what
//! is missing and what to create.

use super::Client;
use crate::base::GhError;
use serde_json::{Value, json};

/// The number of the project titled `title` under `login`. `root` is the GraphQL root of the
/// owner (`organization` or `user`).
pub(crate) fn find_project_number(
    client: &Client,
    root: &str,
    login: &str,
    title: &str,
) -> Result<Option<u64>, GhError> {
    let found = client.graphql(
        &format!("query($l:String!,$q:String!){{{root}(login:$l){{projectsV2(first:30,query:$q){{nodes{{number title}}}}}}}}"),
        json!({"l": login, "q": title}),
    )?;
    Ok(found
        .pointer(&format!("/{root}/projectsV2/nodes"))
        .and_then(Value::as_array)
        .and_then(|n| {
            n.iter()
                .find(|p| p.get("title").and_then(Value::as_str) == Some(title))
                .and_then(|p| p.get("number").and_then(Value::as_u64))
        }))
}

/// Create a project titled `title` for the owner with node id `owner_node_id`; its number.
pub(crate) fn create_project(
    client: &Client,
    owner_node_id: &str,
    title: &str,
) -> Result<u64, GhError> {
    let d = client.graphql(
        "mutation($o:ID!,$t:String!){createProjectV2(input:{ownerId:$o,title:$t}){projectV2{number}}}",
        json!({"o": owner_node_id, "t": title}),
    )?;
    d.pointer("/createProjectV2/projectV2/number")
        .and_then(Value::as_u64)
        .ok_or_else(|| GhError::Shape("createProjectV2 returned no number".into()))
}

/// The names (`nameWithOwner`, lower case) of the repositories that the project is linked to.
pub(crate) fn linked_repo_names(client: &Client, project_id: &str) -> Result<Vec<String>, GhError> {
    let linked = client.graphql(
        "query($p:ID!){node(id:$p){... on ProjectV2{repositories(first:50){nodes{nameWithOwner}}}}}",
        json!({"p": project_id}),
    )?;
    Ok(linked
        .pointer("/node/repositories/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| GhError::Shape("project has no repositories".into()))?
        .iter()
        .filter_map(|n| {
            n.get("nameWithOwner")
                .and_then(Value::as_str)
                .map(str::to_lowercase)
        })
        .collect())
}

/// Link the project to the repository with node id `repo_node_id`.
pub(crate) fn link_repo(
    client: &Client,
    project_id: &str,
    repo_node_id: &str,
) -> Result<(), GhError> {
    client.graphql(
        "mutation($p:ID!,$r:ID!){linkProjectV2ToRepository(input:{projectId:$p,repositoryId:$r}){repository{nameWithOwner}}}",
        json!({"p": project_id, "r": repo_node_id}),
    )?;
    Ok(())
}

/// Give the user with node id `user_node_id` the Write role on the project.
/// `updateProjectV2Collaborators` sets the role; calling it again is harmless.
pub(crate) fn grant_writer(
    client: &Client,
    project_id: &str,
    user_node_id: &str,
) -> Result<(), GhError> {
    let d = client.graphql(
        "mutation($p:ID!,$u:ID!){updateProjectV2Collaborators(input:{projectId:$p,collaborators:[{userId:$u,role:WRITER}]}){collaborators{totalCount}}}",
        json!({"p": project_id, "u": user_node_id}),
    )?;
    if d.pointer("/updateProjectV2Collaborators/collaborators")
        .is_none()
    {
        return Err(GhError::Shape(
            "updateProjectV2Collaborators returned no collaborators".into(),
        ));
    }
    Ok(())
}

/// The number of items on the project.
pub(crate) fn items_total(client: &Client, project_id: &str) -> Result<u64, GhError> {
    Ok(client
        .graphql(
            "query($p:ID!){node(id:$p){... on ProjectV2{items(first:1){totalCount}}}}",
            json!({"p": project_id}),
        )?
        .pointer("/node/items/totalCount")
        .and_then(Value::as_u64)
        .unwrap_or(0))
}

/// Create a single-select field. `options` are the `{name, color, description}` objects.
pub(crate) fn create_select_field(
    client: &Client,
    project_id: &str,
    name: &str,
    options: &[Value],
) -> Result<(), GhError> {
    client.graphql(
        "mutation($p:ID!,$n:String!,$o:[ProjectV2SingleSelectFieldOptionInput!]!){createProjectV2Field(input:{projectId:$p,dataType:SINGLE_SELECT,name:$n,singleSelectOptions:$o}){projectV2Field{... on ProjectV2SingleSelectField{id}}}}",
        json!({"p": project_id, "n": name, "o": options}),
    )?;
    Ok(())
}

pub(crate) fn create_text_field(
    client: &Client,
    project_id: &str,
    name: &str,
) -> Result<(), GhError> {
    client.graphql(
        "mutation($p:ID!,$n:String!){createProjectV2Field(input:{projectId:$p,dataType:TEXT,name:$n}){projectV2Field{... on ProjectV2Field{id}}}}",
        json!({"p": project_id, "n": name}),
    )?;
    Ok(())
}

pub(crate) fn create_number_field(
    client: &Client,
    project_id: &str,
    name: &str,
) -> Result<(), GhError> {
    client.graphql(
        "mutation($p:ID!,$n:String!){createProjectV2Field(input:{projectId:$p,dataType:NUMBER,name:$n}){projectV2Field{... on ProjectV2Field{id}}}}",
        json!({"p": project_id, "n": name}),
    )?;
    Ok(())
}

/// The names of the views of the project.
pub(crate) fn view_names(client: &Client, project_id: &str) -> Result<Vec<String>, GhError> {
    let existing = client.graphql(
        "query($p:ID!){node(id:$p){... on ProjectV2{views(first:50){nodes{name}}}}}",
        json!({"p": project_id}),
    )?;
    Ok(existing
        .pointer("/node/views/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| GhError::Shape("project has no views".into()))?
        .iter()
        .filter_map(|v| v.get("name").and_then(Value::as_str).map(str::to_string))
        .collect())
}

/// The fields of the project as the REST API lists them (they carry the numeric ids that the
/// views need). `base` is the REST path of the owner (see `board::rest_base`).
pub(crate) fn rest_fields(client: &Client, base: &str, number: u64) -> Result<Value, GhError> {
    Ok(client
        .get(&format!("{base}/projectsV2/{number}/fields?per_page=100"))?
        .body)
}

/// Create a view; `body` holds its name, layout, filter and columns.
pub(crate) fn create_view(
    client: &Client,
    base: &str,
    number: u64,
    body: &Value,
) -> Result<(), GhError> {
    client.post(&format!("{base}/projectsV2/{number}/views"), body)?;
    Ok(())
}
