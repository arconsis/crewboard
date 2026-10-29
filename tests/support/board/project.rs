//! Project-level endpoints of the fake for `init` and `report-status`.
//!
//! A child of `board.rs`: it shares the private `State`. It holds what `init` creates or
//! adopts: repo links, views, pinned issues, the field schema writes, the files on a branch,
//! and the status updates of the project. One handler function per endpoint. The GraphQL
//! operations are matched by a text of their query (see [`graphql`]).

use super::*;
use base64::Engine;

/// The project-level state. Default: no linked repo, no view, no pin, no status update.
#[derive(Default)]
pub(super) struct ProjectState {
    /// `nameWithOwner` of the repos linked to the project.
    linked: Vec<String>,
    /// Names of the project views.
    views: Vec<String>,
    /// The bodies of the `POST .../views` calls, in order.
    view_bodies: Vec<Value>,
    /// Numbers of the pinned issues of the main repo, in pin order.
    pins: Vec<u64>,
    /// `(id, status, body, created)`, oldest first.
    status_updates: Vec<(String, String, String, i64)>,
    /// The next `createProjectV2StatusUpdate` is followed by an update of someone else.
    foreign_next: bool,
    /// `(repo, branch, path)` -> text of a file that a `PUT contents` wrote on a branch.
    pub(super) branch_files: BTreeMap<(String, String, String), String>,
    /// The base SHA of each branch, `(repo, branch)` -> sha.
    refs: BTreeMap<(String, String), String>,
}

/// GitHub's limit of pinned issues per repo.
pub const PIN_LIMIT: usize = 3;

impl FakeBoard {
    /// Remove a field from the project (as a board made before the field existed).
    pub fn remove_field(&self, name: &str) {
        let mut st = self.st.lock().unwrap();
        st.fields.retain(|f| f.name != name);
    }

    /// `(name, data type, option ids)` of every field, in order: what `init` must not change.
    pub fn schema(&self) -> Vec<(String, String, Vec<String>)> {
        let st = self.st.lock().unwrap();
        st.fields
            .iter()
            .map(|f| {
                (
                    f.name.clone(),
                    f.data_type.to_string(),
                    f.options.iter().map(|(id, _)| id.clone()).collect(),
                )
            })
            .collect()
    }

    /// The option ids of a single-select field, in order.
    pub fn field_option_ids(&self, field: &str) -> Vec<String> {
        let st = self.st.lock().unwrap();
        let f = st.fields.iter().find(|f| f.name == field).unwrap();
        f.options.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Link a repo to the project directly.
    pub fn link_repo(&self, repo: &str) {
        self.st
            .lock()
            .unwrap()
            .project
            .linked
            .push(repo.to_string());
    }

    pub fn linked_repos(&self) -> Vec<String> {
        self.st.lock().unwrap().project.linked.clone()
    }

    pub fn views(&self) -> Vec<String> {
        self.st.lock().unwrap().project.views.clone()
    }

    /// The request bodies of the view creations, in order.
    pub fn view_bodies(&self) -> Vec<Value> {
        self.st.lock().unwrap().project.view_bodies.clone()
    }

    /// The numeric REST id of a field, as `GET .../fields` lists it.
    pub fn rest_field_id(&self, name: &str) -> Option<u64> {
        let st = self.st.lock().unwrap();
        st.fields
            .iter()
            .position(|f| f.name == name)
            .map(|i| 100 + i as u64)
    }

    /// Add a view directly (a board that was made by an older `init`).
    pub fn add_view(&self, name: &str) {
        self.st.lock().unwrap().project.views.push(name.to_string());
    }

    /// Pin an issue of the main repo directly (no mutation request).
    pub fn pin(&self, issue: u64) {
        self.st.lock().unwrap().project.pins.push(issue);
    }

    pub fn pins(&self) -> Vec<u64> {
        self.st.lock().unwrap().project.pins.clone()
    }

    /// Clear a field value of an item directly (no mutation request).
    pub fn clear_item(&self, id: &str, name: &str) {
        let mut st = self.st.lock().unwrap();
        st.items.get_mut(id).expect("item").fields.remove(name);
    }

    /// `(id, status, body)` of the status updates, oldest first.
    pub fn status_updates(&self) -> Vec<(String, String, String)> {
        let st = self.st.lock().unwrap();
        st.project
            .status_updates
            .iter()
            .map(|(i, s, b, _)| (i.clone(), s.clone(), b.clone()))
            .collect()
    }

    /// Someone else posts a status update right after the next one of crewboard, so the
    /// read-back of `report-status` finds a newer update than its own.
    pub fn foreign_status_update_next(&self) {
        self.st.lock().unwrap().project.foreign_next = true;
    }

    /// The text of a file on a branch, as a `PUT contents` wrote it.
    pub fn branch_file(&self, branch: &str, path: &str) -> Option<String> {
        let st = self.st.lock().unwrap();
        st.project
            .branch_files
            .get(&(REPO.to_string(), branch.to_string(), path.to_string()))
            .cloned()
    }

    /// `(number, head branch, title)` of the open PRs.
    pub fn open_prs(&self) -> Vec<(u64, String, String)> {
        let st = self.st.lock().unwrap();
        st.prs
            .iter()
            .filter(|p| p.state == "open")
            .map(|p| (p.number, p.head_ref.clone(), p.title.clone()))
            .collect()
    }
}

/// The GraphQL operations of this module, or `None` if the query is not one of them.
pub(super) fn graphql(st: &mut State, q: &str, vars: &Value) -> Option<Result<Value, String>> {
    Some(if q.contains("createProjectV2StatusUpdate") {
        Ok(create_status_update(st, vars))
    } else if q.contains("statusUpdates(first:") {
        Ok(read_status_updates(st))
    } else if q.contains("repositories(first:") {
        Ok(json!({"node": {"repositories": {"nodes":
            st.project.linked.iter().map(|r| json!({"nameWithOwner": r})).collect::<Vec<_>>()
        }}}))
    } else if q.contains("linkProjectV2ToRepository") {
        link_repo(st, vars)
    } else if q.contains("items(first:1){totalCount}") {
        let n = st.items.len();
        Ok(json!({"node": {"items": {"totalCount": n}}}))
    } else if q.contains("createProjectV2Field") {
        create_field(st, q, vars)
    } else if q.contains("views(first:") {
        Ok(json!({"node": {"views": {"nodes":
            st.project.views.iter().map(|v| json!({"name": v})).collect::<Vec<_>>()
        }}}))
    } else if q.contains("pinnedIssues(first:") {
        Ok(json!({"repository": {"pinnedIssues": {"nodes":
            st.project.pins.iter().map(|n| json!({"issue": {"number": n}})).collect::<Vec<_>>()
        }}}))
    } else if q.contains("pinIssue(") && !q.contains("unpinIssue(") {
        pin_issue(st, vars)
    } else {
        return None;
    })
}

fn create_status_update(st: &mut State, vars: &Value) -> Value {
    let now = now_epoch();
    let n = st.project.status_updates.len() + 1;
    let (id, status, body) = (
        format!("PVTSU_{n}"),
        vars["s"].as_str().unwrap_or("").to_string(),
        vars["b"].as_str().unwrap_or("").to_string(),
    );
    st.project
        .status_updates
        .push((id.clone(), status.clone(), body.clone(), now));
    if std::mem::take(&mut st.project.foreign_next) {
        st.project.status_updates.push((
            format!("PVTSU_{}", n + 1),
            "AT_RISK".into(),
            "from=someone@else\n\nnewer".into(),
            now,
        ));
    }
    json!({"createProjectV2StatusUpdate": {"statusUpdate": {
        "id": id, "status": status, "body": body, "createdAt": format_ts(now)}}})
}

/// `statusUpdates(first:1, orderBy CREATED_AT DESC)`: the newest update.
fn read_status_updates(st: &State) -> Value {
    let nodes: Vec<Value> = st
        .project
        .status_updates
        .iter()
        .rev()
        .take(1)
        .map(|(id, s, b, t)| json!({"id": id, "status": s, "body": b, "createdAt": format_ts(*t)}))
        .collect();
    json!({"node": {"statusUpdates": {"nodes": nodes}}})
}

fn link_repo(st: &mut State, vars: &Value) -> Result<Value, String> {
    let id = vars["r"].as_str().unwrap_or("");
    let repo = id
        .strip_prefix("R_")
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{id}'"))?
        .to_string();
    if !st.project.linked.contains(&repo) {
        st.project.linked.push(repo.clone());
    }
    Ok(json!({"linkProjectV2ToRepository": {"repository": {"nameWithOwner": repo}}}))
}

/// `createProjectV2Field`: the data type is a literal in the query. GitHub refuses a single
/// select without options and a name that exists.
fn create_field(st: &mut State, q: &str, vars: &Value) -> Result<Value, String> {
    let name = vars["n"].as_str().unwrap_or("").to_string();
    if st.fields.iter().any(|f| f.name == name) {
        return Err(format!("Name has already been taken: {name}"));
    }
    let data_type: &'static str = if q.contains("dataType:SINGLE_SELECT") {
        "SINGLE_SELECT"
    } else if q.contains("dataType:NUMBER") {
        "NUMBER"
    } else {
        "TEXT"
    };
    let options: Vec<String> = vars["o"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|o| o["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if data_type == "SINGLE_SELECT" && options.is_empty() {
        return Err("Single select fields must have at least one option".into());
    }
    let refs: Vec<&str> = options.iter().map(String::as_str).collect();
    let f = field(&name, data_type, &refs);
    let id = f.id.clone();
    st.fields.push(f);
    Ok(json!({"createProjectV2Field": {"projectV2Field": {"id": id}}}))
}

/// `pinIssue`: GitHub refuses a fourth pin.
fn pin_issue(st: &mut State, vars: &Value) -> Result<Value, String> {
    let node = vars["i"].as_str().unwrap_or("");
    let n = st
        .issues
        .iter()
        .find(|i| tickets::issue_node(&i.repo, i.number) == node)
        .map(|i| i.number)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{node}'"))?;
    if st.project.pins.len() >= PIN_LIMIT {
        return Err("You can only pin 3 issues per repository".into());
    }
    if !st.project.pins.contains(&n) {
        st.project.pins.push(n);
    }
    Ok(json!({"pinIssue": {"issue": {"number": n}}}))
}

// ---- REST ------------------------------------------------------------------------------

/// `GET /orgs/{owner}/projectsV2/{n}/fields`: the numeric ids of the fields.
pub(super) fn get_rest_fields(st: &State) -> Resp {
    let mut list = vec![json!({"id": 1, "name": "Title", "data_type": "title"})];
    list.extend(st.fields.iter().enumerate().map(|(i, f)| {
        json!({"id": 100 + i, "node_id": f.id, "name": f.name,
               "data_type": f.data_type.to_ascii_lowercase()})
    }));
    ok(Value::Array(list))
}

/// `POST /orgs/{owner}/projectsV2/{n}/views`: a new view by name.
pub(super) fn post_view(st: &mut State, body: &str) -> Resp {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(name) = v["name"].as_str() else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    st.project.views.push(name.to_string());
    st.project.view_bodies.push(v.clone());
    let id = st.project.views.len();
    Resp::json(
        201,
        &json!({"id": id, "number": id, "name": name, "layout": v["layout"]}).to_string(),
    )
}

/// `GET /repos/{repo}/git/ref/heads/{branch}`: the default branch always exists; another
/// branch exists after `POST git/refs`.
pub(super) fn get_ref(st: &State, repo: &str, branch: &str) -> Resp {
    let sha = if branch == "main" {
        Some(format!("{:040x}", 0xba5e_u64))
    } else {
        st.project
            .refs
            .get(&(repo.to_string(), branch.to_string()))
            .cloned()
    };
    match sha {
        Some(sha) => ok(json!({"ref": format!("refs/heads/{branch}"),
                               "object": {"type": "commit", "sha": sha}})),
        None => Resp::json(404, r#"{"message":"Not Found","status":"404"}"#),
    }
}

/// `POST /repos/{repo}/git/refs`: a new branch. 422 if it exists.
pub(super) fn post_ref(st: &mut State, repo: &str, body: &str) -> Resp {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (Some(r), Some(sha)) = (v["ref"].as_str(), v["sha"].as_str()) else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    let branch = r.strip_prefix("refs/heads/").unwrap_or(r).to_string();
    let key = (repo.to_string(), branch.clone());
    if st.project.refs.contains_key(&key) {
        return Resp::json(422, r#"{"message":"Reference already exists"}"#);
    }
    st.project.refs.insert(key, sha.to_string());
    st.branches.push((repo.to_string(), branch));
    Resp::json(201, &json!({"ref": r, "object": {"sha": sha}}).to_string())
}

/// `PUT /repos/{repo}/contents/{path}`: write a file on a branch (never on the default branch
/// in these tests).
pub(super) fn put_contents(st: &mut State, repo: &str, path: &str, body: &str) -> Resp {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (Some(branch), Some(b64)) = (v["branch"].as_str(), v["content"].as_str()) else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    let Some(text) = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
    else {
        return Resp::json(422, r#"{"message":"content is not valid Base64"}"#);
    };
    let key = (repo.to_string(), branch.to_string(), path.to_string());
    let exists = st.project.branch_files.contains_key(&key);
    if exists && v["sha"].as_str().is_none() {
        return Resp::json(422, r#"{"message":"\"sha\" wasn't supplied."}"#);
    }
    st.project.branch_files.insert(key, text);
    Resp::json(
        if exists { 200 } else { 201 },
        &json!({"content": {"path": path}, "commit": {"sha": format!("{:040x}", 0xf11e_u64)}})
            .to_string(),
    )
}

/// The text of a file on a branch that `PUT contents` wrote, if any (for `GET contents` with
/// `ref=<branch>`).
pub(super) fn branch_file_of(st: &State, repo: &str, path: &str, query: &str) -> Option<String> {
    let branch = query.split('&').find_map(|kv| kv.strip_prefix("ref="))?;
    st.project
        .branch_files
        .get(&(repo.to_string(), branch.to_string(), path.to_string()))
        .cloned()
}

/// `POST /repos/{repo}/pulls`: a new PR from a branch.
pub(super) fn post_pull(st: &mut State, repo: &str, body: &str) -> Resp {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (Some(title), Some(head)) = (v["title"].as_str(), v["head"].as_str()) else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    let number = st.next_number;
    st.next_number += 1;
    st.prs.push(Pr {
        repo: repo.to_string(),
        number,
        title: title.to_string(),
        body: v["body"].as_str().unwrap_or("").to_string(),
        head_ref: head.to_string(),
        head_sha: format!("{:040x}", 0xf11e_u64),
        state: "open",
        merged: false,
        merged_by: None,
        mergeable_state: "clean".to_string(),
        behind_by: 0,
        files: Vec::new(),
        renamed_from: BTreeMap::new(),
    });
    Resp::json(
        201,
        &json!({"number": number,
                "html_url": format!("{}/{repo}/pull/{number}", st.base)})
        .to_string(),
    )
}
