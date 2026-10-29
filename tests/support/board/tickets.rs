//! Ticket, pull request, label and CI endpoints of the fake.
//!
//! A child of `board.rs`: it shares the private `State`. One handler function per endpoint
//! (see `route` in the parent). The seeding helpers at the top make the things that the dev
//! agent does outside crewboard (open an issue, push a branch, open a PR).
//!
//! Every link in an answer points at the fake (loopback). The one exception is input, not
//! answer: `Pull::url` has the `https://github.com/...` form that `done --pr` parses. It is
//! text for the CLI and is never requested.

use super::*;
use base64::Engine;

/// A PR that `FakeBoard::pr_for` opened.
pub struct Pull {
    /// The form that `done --pr` accepts. The fake never receives a request for it.
    pub url: String,
    pub number: u64,
    pub head: String,
    pub branch: String,
}

fn sha(st: &mut State) -> String {
    st.next_sha += 1;
    format!(
        "{:040x}",
        0x00c0_ffee_0000_0000_u128 + u128::from(st.next_sha)
    )
}

pub(super) fn issue_node(repo: &str, n: u64) -> String {
    format!("I_kwDO_{}_{n}", repo.replace('/', "_"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn not_found() -> Resp {
    Resp::json(404, r#"{"message":"Not Found","status":"404"}"#)
}

impl FakeBoard {
    // ---- seeding: what a person or the dev agent does outside crewboard ---------------

    /// An open issue with a body and labels, as `gh issue create` makes it. Returns its
    /// number.
    pub fn ticket_issue(&self, title: &str, body: &str, labels: &[&str]) -> u64 {
        let mut st = self.st.lock().unwrap();
        new_issue(&mut st, REPO, title, body, labels)
    }

    /// The next request with this method, a path containing `path` and a body containing
    /// `body` gets a 500 (once). It is the fault injection of `fake_split.rs`.
    pub fn fail_once(&self, method: &str, path: &str, body: &str) {
        self.st
            .lock()
            .unwrap()
            .fail_once
            .push((method.into(), path.into(), body.into()));
    }

    /// The title and body of an issue.
    pub fn issue_text(&self, issue: u64) -> Option<(String, String)> {
        let st = self.st.lock().unwrap();
        st.issues
            .iter()
            .find(|i| i.repo == REPO && i.number == issue)
            .map(|i| (i.title.clone(), i.body.clone()))
    }

    /// Replace the body of an issue, as a person who edits the issue does (the fake does not
    /// check who edits).
    pub fn set_issue_body(&self, issue: u64, body: &str) {
        let mut st = self.st.lock().unwrap();
        let i = st
            .issues
            .iter_mut()
            .find(|i| i.repo == REPO && i.number == issue)
            .expect("issue");
        i.body = body.to_string();
    }

    /// The label names of an issue.
    pub fn labels_of(&self, issue: u64) -> Vec<String> {
        let st = self.st.lock().unwrap();
        st.issues
            .iter()
            .find(|i| i.repo == REPO && i.number == issue)
            .map(|i| i.labels.clone())
            .unwrap_or_default()
    }

    /// Put a label on an issue, as a person or an old binary does.
    pub fn add_label(&self, issue: u64, label: &str) {
        let mut st = self.st.lock().unwrap();
        st.issues
            .iter_mut()
            .find(|i| i.repo == REPO && i.number == issue)
            .expect("issue")
            .labels
            .push(label.to_string());
    }

    /// Take a label off an issue.
    pub fn remove_label(&self, issue: u64, label: &str) {
        let mut st = self.st.lock().unwrap();
        st.issues
            .iter_mut()
            .find(|i| i.repo == REPO && i.number == issue)
            .expect("issue")
            .labels
            .retain(|l| l != label);
    }

    /// The color of a label of the repo, if it exists.
    pub fn label_color(&self, name: &str) -> Option<String> {
        let st = self.st.lock().unwrap();
        st.labels
            .get(&(REPO.to_string(), name.to_string()))
            .cloned()
    }

    /// Titles of the open issues that carry `label`.
    /// The numbers of the locked issues.
    pub fn locked_issues(&self) -> Vec<u64> {
        let st = self.st.lock().unwrap();
        st.issues
            .iter()
            .filter(|i| i.locked)
            .map(|i| i.number)
            .collect()
    }

    pub fn open_titles(&self, label: &str) -> Vec<String> {
        let st = self.st.lock().unwrap();
        st.issues
            .iter()
            .filter(|i| i.state == "open" && i.labels.iter().any(|l| l == label))
            .map(|i| i.title.clone())
            .collect()
    }

    /// The board item of an issue, once `addProjectV2ItemById` made it.
    pub fn item_of_issue(&self, issue: u64) -> Option<String> {
        let id = format!("PVTI_issue_{issue}");
        self.st
            .lock()
            .unwrap()
            .items
            .contains_key(&id)
            .then_some(id)
    }

    /// A file on the default branch (for example `AUTHORITY.md`).
    pub fn file(&self, path: &str, text: &str) {
        self.st
            .lock()
            .unwrap()
            .files
            .insert((REPO.to_string(), path.to_string()), text.to_string());
    }

    /// A commit on the default branch that changed `path`, dated `date` (ISO 8601, UTC).
    pub fn base_commit(&self, path: &str, date: &str) {
        self.st
            .lock()
            .unwrap()
            .base_commits
            .push((path.to_string(), date.to_string()));
    }

    /// Branch `crew/<agent>/<issue>-it` with one commit and a PR "Closes #<issue>".
    pub fn pr_for(&self, issue: u64, agent: &str) -> Pull {
        let mut st = self.st.lock().unwrap();
        let branch = format!("crew/{agent}/{issue}-it");
        let head = sha(&mut st);
        st.branches.push((REPO.to_string(), branch.clone()));
        let number = st.next_number;
        st.next_number += 1;
        st.prs.push(Pr {
            repo: REPO.to_string(),
            number,
            title: format!("IT work for #{issue}"),
            body: format!("IT.\n\nCloses #{issue}"),
            head_ref: branch.clone(),
            head_sha: head.clone(),
            state: "open",
            merged: false,
            merged_by: None,
            mergeable_state: "clean".to_string(),
            behind_by: 0,
            files: vec!["src/it.rs".to_string()],
            renamed_from: BTreeMap::new(),
        });
        Pull {
            url: format!("https://github.com/{REPO}/pull/{number}"),
            number,
            head,
            branch,
        }
    }

    /// A branch on GitHub without a PR.
    pub fn push_branch(&self, branch: &str) {
        let mut st = self.st.lock().unwrap();
        st.branches.push((REPO.to_string(), branch.to_string()));
    }

    /// An open PR on any branch name, with the given body.
    pub fn pr_on_branch(&self, branch: &str, body: &str) -> Pull {
        let mut st = self.st.lock().unwrap();
        let head = sha(&mut st);
        st.branches.push((REPO.to_string(), branch.to_string()));
        let number = st.next_number;
        st.next_number += 1;
        st.prs.push(Pr {
            repo: REPO.to_string(),
            number,
            title: format!("IT work on {branch}"),
            body: body.to_string(),
            head_ref: branch.to_string(),
            head_sha: head.clone(),
            state: "open",
            merged: false,
            merged_by: None,
            mergeable_state: "clean".to_string(),
            behind_by: 0,
            files: vec!["src/it.rs".to_string()],
            renamed_from: BTreeMap::new(),
        });
        Pull {
            url: format!("https://github.com/{REPO}/pull/{number}"),
            number,
            head,
            branch: branch.to_string(),
        }
    }

    /// A new commit on a branch: the head of its PR moves. Returns the new head.
    pub fn push(&self, branch: &str) -> String {
        let mut st = self.st.lock().unwrap();
        let new = sha(&mut st);
        let pr = st
            .prs
            .iter_mut()
            .find(|p| p.head_ref == branch)
            .expect("PR of the branch");
        pr.head_sha = new.clone();
        new
    }

    /// The `mergeable_state` that GitHub reports for a PR (`clean` by default; for example
    /// `dirty` after a conflict with the base branch).
    pub fn set_mergeable_state(&self, number: u64, state: &str) {
        let mut st = self.st.lock().unwrap();
        st.prs
            .iter_mut()
            .find(|p| p.number == number)
            .expect("PR")
            .mergeable_state = state.to_string();
    }

    /// How many commits the head of a PR is behind its base (`behind_by` of the compare
    /// endpoint; 0 by default).
    pub fn set_behind(&self, number: u64, n: u64) {
        let mut st = self.st.lock().unwrap();
        st.prs
            .iter_mut()
            .find(|p| p.number == number)
            .expect("PR")
            .behind_by = n;
    }

    /// A file that the PR changes.
    pub fn pr_file(&self, number: u64, path: &str) {
        let mut st = self.st.lock().unwrap();
        st.prs
            .iter_mut()
            .find(|p| p.number == number)
            .expect("PR")
            .files
            .push(path.to_string());
    }

    /// A file of the PR that was renamed: `new` is the path now, `old` the path before.
    pub fn pr_rename(&self, number: u64, new: &str, old: &str) {
        let mut st = self.st.lock().unwrap();
        let p = st.prs.iter_mut().find(|p| p.number == number).expect("PR");
        p.files.push(new.to_string());
        p.renamed_from.insert(new.to_string(), old.to_string());
    }

    /// Replace the files that the PR changes.
    pub fn set_pr_files(&self, number: u64, paths: &[&str]) {
        let mut st = self.st.lock().unwrap();
        st.prs
            .iter_mut()
            .find(|p| p.number == number)
            .expect("PR")
            .files = paths.iter().map(|p| p.to_string()).collect();
    }

    /// The answer of `GET repos/{r}/branches/{b}` (the base branch and its protection).
    pub fn set_branch_answer(&self, v: Value) {
        self.st.lock().unwrap().branch_answer = Some(v);
    }

    /// `GET repos/{r}/branches/{b}` fails with this HTTP status and message (a token that cannot
    /// read the branch).
    pub fn set_branch_error(&self, status: u16, message: &str) {
        self.st.lock().unwrap().branch_error = Some((status, message.to_string()));
    }

    /// The answer of `GET repos/{r}/rules/branches/{b}` (the rulesets of the base branch).
    pub fn set_rules_answer(&self, status: u16, v: Value) {
        self.st.lock().unwrap().rules_answer = (status, v);
    }

    /// `POST repos/{r}/rulesets` fails with this status and message (a token that is not admin,
    /// a plan without rulesets on private repos).
    pub fn set_ruleset_error(&self, status: u16, message: &str) {
        self.st.lock().unwrap().ruleset_error = Some((status, message.to_string()));
    }

    /// The request bodies of the rulesets that the fake accepted, in order.
    pub fn rulesets(&self) -> Vec<Value> {
        self.st.lock().unwrap().rulesets.clone()
    }

    /// A ruleset exists already, with this `enforcement` (`active`, `disabled`).
    pub fn add_ruleset(&self, name: &str, enforcement: &str) {
        self.st
            .lock()
            .unwrap()
            .rulesets
            .push(json!({"name": name, "enforcement": enforcement}));
    }

    /// Set the title and the body of a PR (a person edits them).
    pub fn set_pr_text(&self, number: u64, title: &str, body: &str) {
        let mut st = self.st.lock().unwrap();
        let p = st.prs.iter_mut().find(|p| p.number == number).expect("PR");
        p.title = title.to_string();
        p.body = body.to_string();
    }

    /// The owner merges the PR in the browser: merged and closed. The fake does not close the
    /// issue that a closing keyword names; a test calls `close_issue` for that.
    pub fn merge_pr(&self, number: u64) {
        self.merge_pr_by(number, OWNER);
    }

    /// `login` merges the PR (`merged_by` of the answer).
    pub fn merge_pr_by(&self, number: u64, login: &str) {
        let mut st = self.st.lock().unwrap();
        let p = st.prs.iter_mut().find(|p| p.number == number).expect("PR");
        p.merged = true;
        p.merged_by = Some(login.to_string());
        p.state = "closed";
    }

    /// The PR is closed without a merge.
    pub fn close_pr(&self, number: u64) {
        let mut st = self.st.lock().unwrap();
        let p = st.prs.iter_mut().find(|p| p.number == number).expect("PR");
        p.state = "closed";
    }

    /// The timeline of `issue` shows that PR `number` refers to it (a `cross-referenced`
    /// event, as GitHub records it when a PR body names the issue).
    pub fn cross_reference(&self, issue: u64, number: u64) {
        let mut st = self.st.lock().unwrap();
        st.timelines.entry(issue).or_default().push(json!({
            "event": "cross-referenced",
            "source": {"type": "issue", "issue": {
                "number": number, "pull_request": {}, "repository": {"full_name": REPO}}},
        }));
    }

    /// An issue is closed (GitHub does it when a PR with `Closes #n` is merged).
    pub fn close_issue(&self, issue: u64) {
        let mut st = self.st.lock().unwrap();
        st.issues
            .iter_mut()
            .find(|i| i.number == issue)
            .expect("issue")
            .state = "closed";
    }

    /// Is the issue open?
    pub fn issue_is_open(&self, issue: u64) -> bool {
        let st = self.st.lock().unwrap();
        st.issues
            .iter()
            .find(|i| i.number == issue)
            .expect("issue")
            .state
            == "open"
    }

    /// `(merged, head sha)` of a PR.
    pub fn pr_now(&self, number: u64) -> (bool, String) {
        let st = self.st.lock().unwrap();
        let p = st.prs.iter().find(|p| p.number == number).expect("PR");
        (p.merged, p.head_sha.clone())
    }

    /// A completed workflow run of `sha` in the repo of the fake. `event` is for
    /// example `workflow_dispatch`, `conclusion` `success` or `failure`.
    pub fn workflow_run(&self, id: u64, sha: &str, event: &str, conclusion: &str) {
        let mut st = self.st.lock().unwrap();
        let url = format!("{}/{REPO}/actions/runs/{id}", st.base);
        st.runs.push(json!({
            "id": id, "name": "ci", "head_sha": sha, "event": event, "status": "completed",
            "conclusion": conclusion, "run_attempt": 1, "check_suite_id": id * 10,
            "html_url": url, "repository": {"full_name": REPO},
        }));
    }

    /// A completed check run (a job) `name` of `sha`, in the suite of the workflow run `run`.
    pub fn check_run(&self, id: u64, run: u64, sha: &str, name: &str, conclusion: &str) {
        let mut st = self.st.lock().unwrap();
        let url = format!("{}/{REPO}/runs/{id}", st.base);
        st.checks.push(json!({
            "id": id, "name": name, "head_sha": sha, "status": "completed",
            "conclusion": conclusion, "html_url": url, "check_suite": {"id": run * 10},
        }));
    }

    /// A file on a branch of the repo (`GET contents?ref=<branch>` serves it). The default
    /// branch is `file`.
    pub fn branch_file_set(&self, branch: &str, path: &str, text: &str) {
        let mut st = self.st.lock().unwrap();
        st.project.branch_files.insert(
            (REPO.to_string(), branch.to_string(), path.to_string()),
            text.into(),
        );
    }

    /// A `push` workflow run of `sha` that still runs (`status` `in_progress`, no conclusion).
    pub fn workflow_run_running(&self, id: u64, sha: &str) {
        let mut st = self.st.lock().unwrap();
        let url = format!("{}/{REPO}/actions/runs/{id}", st.base);
        st.runs.push(json!({
            "id": id, "name": "ci", "head_sha": sha, "event": "push", "status": "in_progress",
            "conclusion": null, "run_attempt": 1, "check_suite_id": id * 10,
            "html_url": url, "repository": {"full_name": REPO},
        }));
    }

    /// A running workflow run completes with `conclusion`.
    pub fn complete_run(&self, id: u64, conclusion: &str) {
        let mut st = self.st.lock().unwrap();
        let r = st
            .runs
            .iter_mut()
            .find(|r| r["id"].as_u64() == Some(id))
            .expect("run");
        r["status"] = json!("completed");
        r["conclusion"] = json!(conclusion);
    }

    /// A rerun of a workflow run ends with another conclusion.
    pub fn set_run_conclusion(&self, id: u64, conclusion: &str) {
        let mut st = self.st.lock().unwrap();
        let r = st
            .runs
            .iter_mut()
            .find(|r| r["id"].as_u64() == Some(id))
            .expect("run");
        r["conclusion"] = json!(conclusion);
    }

    /// A rerun of a check run (a job) ends with another conclusion.
    pub fn set_check_conclusion(&self, id: u64, conclusion: &str) {
        let mut st = self.st.lock().unwrap();
        let c = st
            .checks
            .iter_mut()
            .find(|c| c["id"].as_u64() == Some(id))
            .expect("check run");
        c["conclusion"] = json!(conclusion);
    }

    pub fn branch_exists(&self, branch: &str) -> bool {
        let st = self.st.lock().unwrap();
        st.branches.iter().any(|(_, b)| b == branch)
    }
}

fn new_issue(st: &mut State, repo: &str, title: &str, body: &str, labels: &[&str]) -> u64 {
    let n = st.next_number;
    st.next_number += 1;
    for l in labels {
        st.labels
            .entry((repo.to_string(), l.to_string()))
            .or_insert_with(|| "ededed".to_string());
    }
    st.issues.push(IssueRow {
        repo: repo.to_string(),
        number: n,
        labels: labels.iter().map(|s| s.to_string()).collect(),
        title: title.to_string(),
        body: body.to_string(),
        state: "open",
        locked: false,
    });
    n
}

/// `PUT /repos/{repo}/issues/{n}/lock`: 204, also for an issue that is locked already. Issue
/// numbers are unique in the fake (one counter).
pub(super) fn lock_issue(st: &mut State, num: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    match st.issues.iter_mut().find(|i| i.number == n) {
        Some(i) => {
            i.locked = true;
            Resp::json(204, "")
        }
        None => not_found(),
    }
}

/// `GET /repos/{repo}`: the repo and its default branch.
pub(super) fn get_repo(st: &State, repo: &str) -> Resp {
    let mut v = repo_json(st, repo);
    v["default_branch"] = json!("main");
    v["node_id"] = json!(format!("R_{repo}"));
    ok(v)
}

// ---- issues ----------------------------------------------------------------------------

fn label_json(st: &State, repo: &str, name: &str) -> Value {
    let color = st
        .labels
        .get(&(repo.to_string(), name.to_string()))
        .cloned()
        .unwrap_or_else(|| "ededed".to_string());
    json!({"id": 1, "node_id": format!("LA_{name}"), "name": name, "color": color,
           "default": false, "description": null,
           "url": format!("{}/repos/{repo}/labels/{name}", st.base)})
}

fn issue_json(st: &State, i: &IssueRow) -> Value {
    json!({
        "url": format!("{}/repos/{}/issues/{}", st.base, i.repo, i.number),
        "html_url": format!("{}/{}/issues/{}", st.base, i.repo, i.number),
        "id": 5000 + i.number,
        "node_id": issue_node(&i.repo, i.number),
        "number": i.number,
        "title": i.title,
        "user": {"login": st.viewer, "id": 1, "type": "User"},
        "labels": i.labels.iter().map(|l| label_json(st, &i.repo, l)).collect::<Vec<_>>(),
        "state": i.state,
        "locked": i.locked,
        "assignees": [],
        "comments": st.comments.iter().filter(|c| c.repo == i.repo && c.issue == i.number).count(),
        "author_association": "OWNER",
        "body": i.body,
    })
}

/// `GET /repos/{repo}/issues/{n}`
pub(super) fn get_issue(st: &State, repo: &str, num: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    match st.issues.iter().find(|i| i.repo == repo && i.number == n) {
        Some(i) => ok(issue_json(st, i)),
        None => not_found(),
    }
}

/// `POST /repos/{repo}/issues`: title, body, labels. A new label is created.
pub(super) fn post_issue(st: &mut State, repo: &str, body: &str) -> Resp {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Resp::json(400, r#"{"message":"Problems parsing JSON"}"#);
    };
    let Some(title) = v["title"].as_str().filter(|t| !t.is_empty()) else {
        return Resp::json(
            422,
            r#"{"message":"Validation Failed","errors":[{"resource":"Issue","code":"missing_field","field":"title"}]}"#,
        );
    };
    let labels: Vec<String> = v["labels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let n = new_issue(st, repo, title, v["body"].as_str().unwrap_or(""), &refs);
    let i = st.issues.iter().find(|i| i.number == n).unwrap();
    Resp::json(201, &issue_json(st, i).to_string())
}

/// `PATCH /repos/{repo}/issues/{n}`: the body and the title (`ticket split` appends a link).
pub(super) fn patch_issue(st: &mut State, repo: &str, num: &str, body: &str) -> Resp {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return Resp::json(400, r#"{"message":"Problems parsing JSON"}"#);
    };
    let n: u64 = num.parse().unwrap_or(0);
    let Some(i) = st
        .issues
        .iter_mut()
        .find(|i| i.repo == repo && i.number == n)
    else {
        return not_found();
    };
    if let Some(b) = v["body"].as_str() {
        i.body = b.to_string();
    }
    if let Some(t) = v["title"].as_str() {
        i.title = t.to_string();
    }
    match v["state"].as_str() {
        Some("open") => i.state = "open",
        Some("closed") => i.state = "closed",
        _ => {}
    }
    let i = st
        .issues
        .iter()
        .find(|i| i.repo == repo && i.number == n)
        .unwrap();
    ok(issue_json(st, i))
}

/// `DELETE /repos/{repo}/issues/comments/{id}`
pub(super) fn delete_comment(st: &mut State, repo: &str, id: &str) -> Resp {
    let id: u64 = id.parse().unwrap_or(0);
    let before = st.comments.len();
    st.comments.retain(|c| !(c.repo == repo && c.id == id));
    if st.comments.len() == before {
        return not_found();
    }
    Resp::json(204, "")
}

// ---- labels ----------------------------------------------------------------------------

/// `POST /repos/{repo}/labels`: 422 `already_exists` for a known name.
pub(super) fn post_label(st: &mut State, repo: &str, body: &str) -> Resp {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let (Some(name), color) = (v["name"].as_str(), v["color"].as_str().unwrap_or("ededed")) else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    let key = (repo.to_string(), name.to_string());
    if st.labels.contains_key(&key) {
        return Resp::json(
            422,
            r#"{"message":"Validation Failed","errors":[{"resource":"Label","code":"already_exists","field":"name"}]}"#,
        );
    }
    st.labels.insert(key, color.to_string());
    Resp::json(201, &label_json(st, repo, name).to_string())
}

/// `POST /repos/{repo}/issues/{n}/labels`: adds the names; an unknown name is created.
pub(super) fn add_issue_label(st: &mut State, repo: &str, num: &str, body: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let names: Vec<String> = v["labels"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for name in &names {
        st.labels
            .entry((repo.to_string(), name.clone()))
            .or_insert_with(|| "ededed".to_string());
    }
    let Some(i) = st
        .issues
        .iter_mut()
        .find(|i| i.repo == repo && i.number == n)
    else {
        return not_found();
    };
    for name in names {
        if !i.labels.contains(&name) {
            i.labels.push(name);
        }
    }
    let all = i.labels.clone();
    ok(Value::Array(
        all.iter().map(|l| label_json(st, repo, l)).collect(),
    ))
}

/// `DELETE /repos/{repo}/issues/{n}/labels/{name}`: 404 if the issue has no such label.
pub(super) fn remove_issue_label(st: &mut State, repo: &str, num: &str, name: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    let name = percent_decode(name);
    let Some(i) = st
        .issues
        .iter_mut()
        .find(|i| i.repo == repo && i.number == n)
    else {
        return not_found();
    };
    if !i.labels.contains(&name) {
        return Resp::json(404, r#"{"message":"Label does not exist","status":"404"}"#);
    }
    i.labels.retain(|l| *l != name);
    let all = i.labels.clone();
    ok(Value::Array(
        all.iter().map(|l| label_json(st, repo, l)).collect(),
    ))
}

// ---- pull requests ---------------------------------------------------------------------

fn repo_json(st: &State, repo: &str) -> Value {
    json!({"id": 7, "name": repo.rsplit('/').next().unwrap_or(""), "full_name": repo,
           "private": true, "owner": {"login": OWNER, "id": 2, "type": "Organization"},
           "permissions": {"admin": false, "push": st.push, "pull": true},
           "url": format!("{}/repos/{repo}", st.base)})
}

fn pull_json(st: &State, p: &Pr) -> Value {
    let merge_sha = format!("{:040x}", 0xabc0_u128 + u128::from(p.number));
    json!({
        "url": format!("{}/repos/{}/pulls/{}", st.base, p.repo, p.number),
        "id": 9000 + p.number,
        "node_id": format!("PR_kwDO_{}", p.number),
        "html_url": format!("{}/{}/pull/{}", st.base, p.repo, p.number),
        "number": p.number,
        "state": p.state,
        "locked": false,
        "title": p.title,
        "user": {"login": st.viewer, "id": 1, "type": "User"},
        "body": p.body,
        "merged_at": if p.merged { json!(format_ts(now_epoch())) } else { Value::Null },
        "merge_commit_sha": if p.merged { json!(merge_sha) } else { Value::Null },
        "draft": false,
        "head": {"label": format!("{OWNER}:{}", p.head_ref), "ref": p.head_ref,
                 "sha": p.head_sha, "repo": repo_json(st, &p.repo)},
        "base": {"label": format!("{OWNER}:main"), "ref": "main",
                 "sha": format!("{:040x}", 0xba5e_u64), "repo": repo_json(st, &p.repo)},
        "merged": p.merged,
        "merged_by": p.merged_by.as_ref().map(|l| json!({"login": l, "id": 3, "type": "User"})),
        "mergeable": !p.merged,
        "mergeable_state": p.mergeable_state,
        "comments": 0,
        "commits": 1,
        "changed_files": p.files.len(),
    })
}

fn find_pr<'a>(st: &'a State, repo: &str, num: &str) -> Option<&'a Pr> {
    let n: u64 = num.parse().unwrap_or(0);
    st.prs.iter().find(|p| p.repo == repo && p.number == n)
}

/// `GET /repos/{repo}/pulls?state=open`, with `head=<owner>:<branch>` only the PRs of that branch.
pub(super) fn get_pull_list(st: &State, repo: &str, query: &str) -> Resp {
    let head = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("head="))
        .map(|h| h.split_once(':').map_or(h, |(_, b)| b).to_string());
    ok(Value::Array(
        st.prs
            .iter()
            .filter(|p| p.repo == repo && p.state == "open")
            .filter(|p| head.as_ref().is_none_or(|h| *h == p.head_ref))
            .map(|p| pull_json(st, p))
            .collect(),
    ))
}

/// `GET /repos/{repo}/git/matching-refs/heads/{prefix}`: the branches that start with `prefix`.
pub(super) fn get_matching_refs(st: &State, repo: &str, prefix: &str) -> Resp {
    ok(Value::Array(
        st.branches
            .iter()
            .filter(|(r, b)| r == repo && b.starts_with(prefix))
            .map(|(_, b)| json!({"ref": format!("refs/heads/{b}")}))
            .collect(),
    ))
}

/// `GET /repos/{repo}/compare/{base}...{head}`: only `behind_by`, for the PR whose head is `head`
/// (a SHA or a branch name). 404 for a head that no PR has.
pub(super) fn get_compare(st: &State, repo: &str, spec: &str) -> Resp {
    let Some((_, head)) = spec.split_once("...") else {
        return not_found();
    };
    match st
        .prs
        .iter()
        .find(|p| p.repo == repo && (p.head_sha == head || p.head_ref == head))
    {
        Some(p) => ok(
            json!({"status": if p.behind_by > 0 { "diverged" } else { "ahead" },
                             "ahead_by": 1, "behind_by": p.behind_by, "total_commits": 1,
                             "commits": [], "files": []}),
        ),
        None => not_found(),
    }
}

/// `GET /repos/{repo}/pulls/{n}`
pub(super) fn get_pull(st: &State, repo: &str, num: &str) -> Resp {
    match find_pr(st, repo, num) {
        Some(p) => ok(pull_json(st, p)),
        None => not_found(),
    }
}

/// `GET /repos/{repo}/pulls/{n}/files`, 100 per page like GitHub (`per_page`, `page`).
pub(super) fn get_pull_files(st: &State, repo: &str, num: &str, query: &str) -> Resp {
    let Some(p) = find_pr(st, repo, num) else {
        return not_found();
    };
    let arg = |name: &str, default: usize| -> usize {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let per_page = arg("per_page", 30).max(1);
    let page = arg("page", 1).max(1);
    ok(Value::Array(
        p.files
            .iter()
            .skip((page - 1) * per_page)
            .take(per_page)
            .map(|f| match p.renamed_from.get(f) {
                Some(old) => json!({"sha": format!("{:040x}", f.len()), "filename": f,
                       "status": "renamed", "previous_filename": old,
                       "additions": 0, "deletions": 0, "changes": 0}),
                None => {
                    json!({"sha": format!("{:040x}", f.len()), "filename": f, "status": "added",
                       "additions": 1, "deletions": 0, "changes": 1,
                       "patch": "@@ -0,0 +1 @@\n+x"})
                }
            })
            .collect(),
    ))
}

/// `PUT /repos/{repo}/pulls/{n}/merge`: 409 if `sha` is not the head; 405 if the PR is not
/// open.
pub(super) fn put_merge(st: &mut State, repo: &str, num: &str, body: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let Some(p) = st.prs.iter_mut().find(|p| p.repo == repo && p.number == n) else {
        return not_found();
    };
    if p.merged || p.state != "open" {
        return Resp::json(
            405,
            r#"{"message":"Pull Request is not mergeable","status":"405"}"#,
        );
    }
    if v["sha"].as_str() != Some(p.head_sha.as_str()) {
        return Resp::json(
            409,
            r#"{"message":"Head branch was modified. Review and try the merge again.","status":"409"}"#,
        );
    }
    p.merged = true;
    p.merged_by = Some("fake-crew".to_string());
    p.state = "closed";
    ok(
        json!({"sha": format!("{:040x}", 0xabc0_u128 + u128::from(n)),
              "merged": true, "message": "Pull Request successfully merged"}),
    )
}

/// `DELETE /repos/{repo}/git/refs/heads/{branch}`: 422 if the ref does not exist.
pub(super) fn delete_branch(st: &mut State, repo: &str, branch: &str) -> Resp {
    // GitHub decodes the path: `crew/a%40h/1-x` names the branch `crew/a@h/1-x`
    let branch = &percent_decode(branch);
    let before = st.branches.len();
    st.branches.retain(|(r, b)| !(r == repo && b == branch));
    if st.branches.len() == before {
        return Resp::json(
            422,
            r#"{"message":"Reference does not exist","status":"422"}"#,
        );
    }
    Resp::json(204, "")
}

/// `GET /repos/{repo}/commits?sha=&path=&since=&per_page=`: the commits of the default branch
/// that changed `path` (or a file below it) after `since`.
pub(super) fn get_commits(st: &State, query: &str) -> Resp {
    let arg = |name: &str| -> Option<&str> {
        query
            .split('&')
            .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
    };
    let path = arg("path").unwrap_or("");
    let since = arg("since").unwrap_or("");
    let list: Vec<Value> = st
        .base_commits
        .iter()
        .filter(|(p, d)| {
            (path.is_empty() || p == path || p.starts_with(&format!("{path}/")))
                && d.as_str() > since
        })
        .map(|(p, d)| json!({"sha": "c0ffee", "commit": {"committer": {"date": d}, "message": p}}))
        .collect();
    ok(Value::Array(list))
}

// ---- files -----------------------------------------------------------------------------

/// `GET /repos/{repo}/contents/{path}`: a file of the default branch, base64 in 60-character
/// lines as GitHub sends it. A path with files below it is a directory: the answer lists them.
/// The `ref` query names a branch: a file that `PUT contents` wrote there wins; else the file of
/// the default branch is served (a new branch starts as a copy of it).
pub(super) fn get_contents(st: &State, repo: &str, path: &str, query: &str) -> Resp {
    if let Some(text) = super::project::branch_file_of(st, repo, path, query) {
        return file_json(st, repo, path, &text);
    }
    if let Some(code) = st.file_errors.get(&(repo.to_string(), path.to_string())) {
        return Resp::json(*code, r#"{"message":"Server Error","status":"500"}"#);
    }
    let Some(text) = st.files.get(&(repo.to_string(), path.to_string())) else {
        // an empty path is the root of the repo
        let dir = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let entries: Vec<Value> = st
            .files
            .iter()
            .filter(|((r, p), _)| r == repo && p.starts_with(&dir) && !p[dir.len()..].contains('/'))
            .map(|((_, p), t)| {
                json!({"type": "file", "name": &p[dir.len()..], "path": p, "size": t.len(),
                       "sha": blob_sha(t)})
            })
            .collect();
        if entries.is_empty() {
            return not_found();
        }
        return ok(Value::Array(entries));
    };
    file_json(st, repo, path, text)
}

fn file_json(st: &State, repo: &str, path: &str, text: &str) -> Resp {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    let wrapped: Vec<&str> = b64
        .as_bytes()
        .chunks(60)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    ok(json!({
        "type": "file", "encoding": "base64", "size": text.len(),
        "name": path.rsplit('/').next().unwrap_or(path), "path": path,
        "sha": blob_sha(text),
        "content": format!("{}\n", wrapped.join("\n")),
        "url": format!("{}/repos/{repo}/contents/{path}", st.base),
    }))
}

/// A stand-in for the git blob SHA: 40 hex characters that depend on the content.
fn blob_sha(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(text.as_bytes());
    d.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

// ---- CI: a commit without any CI (recorded from the sandbox, see tests/fixtures/ci) ------
// A test can add workflow runs with `FakeBoard::workflow_run`.

/// `GET /repos/{repo}/actions/runs?head_sha=`: the runs of that SHA (none by default).
pub(super) fn get_workflow_runs(st: &State, query: &str) -> Resp {
    let sha = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("head_sha="))
        .unwrap_or("");
    let runs: Vec<Value> = st
        .runs
        .iter()
        .filter(|r| r["head_sha"].as_str() == Some(sha))
        .cloned()
        .collect();
    ok(json!({"total_count": runs.len(), "workflow_runs": runs}))
}

/// `GET /repos/{repo}/actions/runs/{id}`: one run, or 404.
pub(super) fn get_workflow_run(st: &State, repo: &str, id: &str) -> Resp {
    st.runs
        .iter()
        .find(|r| {
            r["id"].as_u64().is_some_and(|v| Some(v) == id.parse().ok())
                && r["repository"]["full_name"].as_str() == Some(repo)
        })
        .map(|r| ok(r.clone()))
        .unwrap_or_else(not_found)
}

/// `GET /repos/{repo}/commits/{sha}/check-runs`
pub(super) fn get_check_runs(st: &State, sha: &str) -> Resp {
    let checks: Vec<Value> = st
        .checks
        .iter()
        .filter(|c| c["head_sha"].as_str() == Some(sha))
        .cloned()
        .collect();
    ok(json!({"total_count": checks.len(), "check_runs": checks}))
}

/// `GET /repos/{repo}/commits/{sha}/status`: GitHub says `pending` for a commit with no
/// status, and `total_count` 0.
pub(super) fn get_combined_status(sha: &str) -> Resp {
    ok(json!({"state": "pending", "statuses": [], "sha": sha, "total_count": 0}))
}

/// `GET /repos/{repo}/commits/{sha}/statuses`
pub(super) fn get_statuses() -> Resp {
    ok(json!([]))
}

/// `GET /repos/{repo}/branches/{b}`: not protected, unless a test set the answer.
pub(super) fn get_branch(st: &State, b: &str) -> Resp {
    if let Some((status, message)) = &st.branch_error {
        return Resp::json(*status, &json!({"message": message}).to_string());
    }
    ok(st.branch_answer.clone().unwrap_or_else(|| {
        json!({"name": b, "protected": active_rulesets(st).next().is_some(), "protection": {"enabled": false,
               "required_status_checks": {"enforcement_level": "off", "contexts": [], "checks": []}}})
    }))
}

/// `GET /repos/{repo}/rules/branches/{b}`: no ruleset, unless a test set the answer.
pub(super) fn get_branch_rules(st: &State) -> Resp {
    let (status, body) = &st.rules_answer;
    let rules: Vec<Value> = active_rulesets(st)
        .flat_map(|r| r["rules"].as_array().cloned().unwrap_or_default())
        .collect();
    if *status == 200 && body == &json!([]) && !rules.is_empty() {
        return ok(Value::Array(rules));
    }
    Resp::json(*status, &body.to_string())
}

/// The rulesets that apply: the ones with `enforcement: active`.
fn active_rulesets(st: &State) -> impl Iterator<Item = &Value> {
    st.rulesets.iter().filter(|r| r["enforcement"] == "active")
}

/// `GET /repos/{repo}/rulesets`: name and enforcement of each ruleset.
pub(super) fn list_rulesets(st: &State) -> Resp {
    ok(Value::Array(st.rulesets.clone()))
}

/// `POST /repos/{repo}/rulesets`: stores the body, or fails as a test said.
pub(super) fn post_ruleset(st: &mut State, body: &str) -> Resp {
    if let Some((status, message)) = &st.ruleset_error {
        return Resp::json(*status, &json!({"message": message}).to_string());
    }
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if v["name"].as_str().is_none() {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    }
    st.rulesets.push(v.clone());
    Resp::json(201, &v.to_string())
}

/// `GET /repos/{repo}/branches/{b}/protection/required_status_checks`: not protected.
pub(super) fn get_required_checks() -> Resp {
    Resp::json(404, r#"{"message":"Branch not protected","status":"404"}"#)
}

// ---- GraphQL ---------------------------------------------------------------------------

/// `addProjectV2ItemById`: the issue becomes an item. An issue on the board already returns
/// its item.
pub(super) fn gql_add_issue_item(st: &mut State, vars: &Value) -> Result<Value, String> {
    let node = vars["c"].as_str().unwrap_or("");
    let Some((repo, n)) = st
        .issues
        .iter()
        .find(|i| issue_node(&i.repo, i.number) == node)
        .map(|i| (i.repo.clone(), i.number))
    else {
        return Err(format!(
            "Could not resolve to a node with the global id of '{node}'"
        ));
    };
    let _ = repo;
    let id = format!("PVTI_issue_{n}");
    st.items.entry(id.clone()).or_insert_with(|| Item {
        id: id.clone(),
        fields: BTreeMap::new(),
        updated: now_epoch(),
        kind: "ISSUE",
        draft_body: String::new(),
    });
    Ok(json!({"addProjectV2ItemById": {"item": {"id": id}}}))
}

/// `clearProjectV2ItemFieldValue`
pub(super) fn gql_clear_field(st: &mut State, vars: &Value) -> Result<Value, String> {
    let (item, fid) = (
        vars["i"].as_str().unwrap_or(""),
        vars["f"].as_str().unwrap_or(""),
    );
    let name = st
        .fields
        .iter()
        .find(|f| f.id == fid)
        .map(|f| f.name.clone())
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{fid}'"))?;
    let it = st
        .items
        .get_mut(item)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{item}'"))?;
    it.fields.remove(&name);
    it.updated = now_epoch();
    Ok(json!({"clearProjectV2ItemFieldValue": {"projectV2Item": {"id": item}}}))
}

/// `node(id){... on ProjectV2Item{content{__typename ... on DraftIssue{id body}}}}`
pub(super) fn gql_read_draft(st: &State, vars: &Value) -> Value {
    let item = vars["i"].as_str().unwrap_or("");
    match st.items.get(item) {
        Some(it) if it.kind == "DRAFT_ISSUE" => json!({"node": {"content": {
            "__typename": "DraftIssue", "id": format!("DI_{item}"), "body": it.draft_body}}}),
        Some(_) => json!({"node": {"content": {"__typename": "Issue"}}}),
        None => json!({"node": null}),
    }
}

/// `updateProjectV2DraftIssue`: the body of an agent item.
pub(super) fn gql_set_draft_body(st: &mut State, vars: &Value) -> Result<Value, String> {
    let d = vars["d"].as_str().unwrap_or("");
    let it = st
        .items
        .values_mut()
        .find(|it| format!("DI_{}", it.id) == d)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{d}'"))?;
    it.draft_body = vars["b"].as_str().unwrap_or("").to_string();
    Ok(json!({"updateProjectV2DraftIssue": {"draftIssue": {"id": d}}}))
}
