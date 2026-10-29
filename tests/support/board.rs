//! A stateful fake of the GitHub board and comment endpoints.
//!
//! It holds one project (fields, items) and the comments and issues of the initiative repo,
//! and answers with the real GitHub shapes. One handler function per endpoint (see
//! `route`). A request that no handler knows gets a 501 and is listed in
//! [`FakeBoard::unhandled`], so a test fails loudly when the CLI calls something new.
//! Every link in an answer points at the fake itself (loopback), never at github.com.

#![allow(dead_code)]

use super::{FakeGithub, Req, Resp};
use crewboard::clock::{format_ts, now_epoch};
use crewboard::config::{AgentConfig, BoardConfig, ConfigDir, load_or_create_key};
use crewboard::protocol::canonical::IssueRef;
use crewboard::protocol::names::{AgentName, Kind, Re, Timestamp, To};
use crewboard::protocol::signature::{Outgoing, sign_message};
use crewboard::registry::{fingerprint, join_body};
use crewboard::rules::Role;
use serde_json::{Value, json};
use ssh_key::PrivateKey;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

mod project;
mod tickets;

#[allow(unused_imports)]
pub use tickets::Pull;

pub const OWNER: &str = "fake-owner";
pub const REPO: &str = "fake-owner/fake-repo";
pub const PROJECT: u64 = 15;
pub const REGISTRY: u64 = 1;
/// The issue that the tests use for messages.
pub const THREAD: u64 = 2;
pub const INITIATIVE: &str = "fake";

const BOARD_TOML: &str = "initiative = \"fake\"\nproject_owner = \"fake-owner\"\nproject_number = 15\nrepos = [\"fake-owner/fake-repo\"]\nowner_login = \"fake-owner\"\nregistry_issue = 1\ninsecure_one_account = true\n";

#[derive(Clone)]
struct Field {
    id: String,
    name: String,
    data_type: &'static str,
    options: Vec<(String, String)>,
}

struct Item {
    id: String,
    fields: BTreeMap<String, String>,
    updated: i64,
    /// `DRAFT_ISSUE` (an agent) or `ISSUE` (a ticket).
    kind: &'static str,
    /// The body of the draft issue behind an agent item.
    draft_body: String,
}

#[derive(Clone)]
struct Comment {
    id: u64,
    repo: String,
    issue: u64,
    login: String,
    body: String,
    created: i64,
    updated: i64,
    /// Logins of the edits, oldest first (what `userContentEdits` lists).
    editors: Vec<String>,
}

struct IssueRow {
    repo: String,
    number: u64,
    labels: Vec<String>,
    title: String,
    body: String,
    state: &'static str,
    /// The conversation is locked (`PUT .../lock`).
    locked: bool,
}

/// A pull request of the fake. The head moves with `FakeBoard::push`.
struct Pr {
    repo: String,
    number: u64,
    title: String,
    body: String,
    head_ref: String,
    head_sha: String,
    state: &'static str,
    merged: bool,
    /// `merged_by.login` of a merged PR.
    merged_by: Option<String>,
    mergeable_state: String,
    /// Commits that the head is behind the base (compare endpoint).
    behind_by: u64,
    files: Vec<String>,
    /// `previous_filename` of a renamed file: new path, old path.
    renamed_from: BTreeMap<String, String>,
}

struct State {
    base: String,
    viewer: String,
    fields: Vec<Field>,
    items: BTreeMap<String, Item>,
    comments: Vec<Comment>,
    issues: Vec<IssueRow>,
    next_comment: u64,
    unhandled: Vec<String>,
    /// Issue and PR numbers share one counter, as on GitHub.
    next_number: u64,
    prs: Vec<Pr>,
    /// `(repo, branch)` of the branches that exist.
    branches: Vec<(String, String)>,
    /// `(repo, path)` -> text of a file on the default branch.
    files: BTreeMap<(String, String), String>,
    /// `(path, ISO date)` of commits on the default branch (`GET commits?path=&since=`).
    base_commits: Vec<(String, String)>,
    /// `(repo, path)` -> HTTP status that `GET contents` answers instead of the file.
    file_errors: BTreeMap<(String, String), u16>,
    /// `(repo, label)` -> color.
    labels: BTreeMap<(String, String), String>,
    /// Counter for the commit ids of the fake.
    next_sha: u64,
    /// `(method, path part, body part)`: the next matching request gets a 500, once.
    fail_once: Vec<(String, String, String)>,
    /// Counter for the ids of options that `updateProjectV2Field` creates.
    next_option: u64,
    /// The answer of `GET repos/{r}/branches/{b}` (protection of the base branch), if set.
    branch_answer: Option<Value>,
    /// `GET repos/{r}/branches/{b}` fails with this status and message, if set.
    branch_error: Option<(u16, String)>,
    /// The answer of `GET repos/{r}/rules/branches/{b}` (rulesets): status and body.
    rules_answer: (u16, Value),
    /// The rulesets that `POST repos/{r}/rulesets` created (the request bodies).
    rulesets: Vec<Value>,
    /// `POST repos/{r}/rulesets` fails with this status and message, if set.
    ruleset_error: Option<(u16, String)>,
    /// The built-in project workflows `(name, enabled)`, or the GraphQL error that the
    /// `workflows` query gets. Default: none.
    workflows: Result<Vec<(String, String, bool)>, String>,
    /// The names of the workflows that `deleteProjectV2Workflow` removed, in order.
    workflows_deleted: Vec<String>,
    /// `deleteProjectV2Workflow` fails with this GraphQL error (a token without the permission).
    fail_workflow_delete: Option<String>,
    /// Issue number -> the events of `GET .../issues/{n}/timeline`, oldest first.
    timelines: BTreeMap<u64, Vec<Value>>,
    /// Workflow runs (`GET actions/runs`). Default: none, so a commit has no CI.
    runs: Vec<Value>,
    /// Check runs (jobs) of the commits, as `GET commits/{sha}/check-runs` answers them.
    checks: Vec<Value>,
    /// Project-level state of `init` and `report-status`.
    project: project::ProjectState,
    /// The `x-oauth-scopes` header of every answer (a classic token). Default: none, as for a
    /// fine-grained token.
    scopes: Option<String>,
    /// `permissions.push` of `GET repos/{r}`. Default: true.
    push: bool,
    /// The GraphQL error of the board query (`projectV2(number)`), if set.
    board_error: Option<String>,
    board_errors: Option<Value>,
    /// A single-select write of this value fails with a GraphQL error, if set.
    fail_select_value: Option<String>,
    /// How many writes of `fail_select_value` still pass before the failure starts.
    fail_select_pass: u32,
    /// Every clear of a field value fails with a GraphQL error.
    fail_clear: bool,
    /// Item id -> how many more item listings leave the item out (the listing lags).
    unlisted: BTreeMap<String, usize>,
    /// `(old name, new name)` of a repo that moved: REST answers for the old name too.
    moved_from: Vec<(String, String)>,
    /// `(item, field, value)`: set when the next option write happens, as a writer behind
    /// crewboard does.
    flip_on_options_write: Vec<(String, String, String)>,
    /// `(from, to)`: the next option write also renames the option `from` (a change GitHub
    /// made that crewboard did not plan).
    rename_on_options_write: Vec<(String, String)>,
    /// How many of the next option writes lose the option that they create, as if a concurrent
    /// writer replaced the list right after (the read-back must catch it).
    drop_new_option_on_write: u32,
    /// `(item, field, value, reads to let pass)`: before the read that follows that many reads of the item by id, the
    /// field is set, as a writer behind crewboard does between two reads of one command.
    flip_after_item_reads: Vec<(String, String, String, u32)>,
}

/// GitHub's limit of options per single-select field, and its error text (seen on the sandbox
/// on 2026-10-17).
pub const OPTION_LIMIT: usize = 100;
pub const OPTION_LIMIT_ERROR: &str = "Settings options cannot contain more than 100 entries";

fn field(name: &str, data_type: &'static str, options: &[&str]) -> Field {
    Field {
        id: format!("PVTF_{name}"),
        name: name.to_string(),
        data_type,
        options: options
            .iter()
            .map(|o| (format!("PVTSSO_{name}_{o}"), o.to_string()))
            .collect(),
    }
}

fn new_state(base: &str) -> State {
    let text = |n: &str| field(n, "TEXT", &[]);
    State {
        base: base.to_string(),
        viewer: OWNER.to_string(),
        fields: vec![
            text("Key"),
            text("Host"),
            text("Runtime"),
            text("Heartbeat"),
            text("Cursor"),
            text("Doing"),
            text("Gate"),
            text("Evidence"),
            text("Capabilities"),
            field(
                "State",
                "SINGLE_SELECT",
                &[
                    "requested",
                    "approved",
                    "active",
                    "working",
                    "paused",
                    "draining",
                    "left",
                ],
            ),
            field(
                "Status",
                "SINGLE_SELECT",
                &[
                    "Backlog",
                    "Ready",
                    "In Progress",
                    "Blocked",
                    "Review",
                    "Needs Human",
                    "Done",
                ],
            ),
            field("Agent", "SINGLE_SELECT", &[]),
            field("Kind", "SINGLE_SELECT", &["Work", "Agent"]),
            field("Role", "SINGLE_SELECT", &["Coordinator", "Dev"]),
            // the queue position; the number is stored as text and read as a number
            field("Order", "NUMBER", &[]),
        ],
        items: BTreeMap::new(),
        comments: Vec::new(),
        issues: Vec::new(),
        next_comment: 1000,
        unhandled: Vec::new(),
        next_number: 10,
        prs: Vec::new(),
        branches: Vec::new(),
        files: BTreeMap::new(),
        base_commits: Vec::new(),
        file_errors: BTreeMap::new(),
        labels: BTreeMap::new(),
        next_sha: 1,
        fail_once: Vec::new(),
        next_option: 1,
        branch_answer: None,
        branch_error: None,
        rules_answer: (200, json!([])),
        rulesets: Vec::new(),
        ruleset_error: None,
        workflows: Ok(Vec::new()),
        workflows_deleted: Vec::new(),
        fail_workflow_delete: None,
        timelines: BTreeMap::new(),
        runs: Vec::new(),
        checks: Vec::new(),
        project: project::ProjectState::default(),
        scopes: None,
        push: true,
        board_error: None,
        board_errors: None,
        fail_select_value: None,
        fail_select_pass: 0,
        fail_clear: false,
        unlisted: BTreeMap::new(),
        moved_from: Vec::new(),
        flip_on_options_write: Vec::new(),
        rename_on_options_write: Vec::new(),
        drop_new_option_on_write: 0,
        flip_after_item_reads: Vec::new(),
    }
}

/// One agent of a scenario: its name, key, board item and config dir.
pub struct Agent {
    pub name: String,
    pub role: Role,
    pub key: PrivateKey,
    pub item_id: String,
    pub dir: PathBuf,
    _tmp: tempfile::TempDir,
}

/// The output of one CLI run.
pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn all(&self) -> String {
        format!(
            "exit {}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}

pub struct FakeBoard {
    pub gh: FakeGithub,
    st: Arc<Mutex<State>>,
}

impl FakeBoard {
    pub fn new() -> Self {
        let st: Arc<Mutex<State>> = Arc::new(Mutex::new(new_state("")));
        let st2 = st.clone();
        let gh = FakeGithub::start_with(move |base| {
            st2.lock().unwrap().base = base.to_string();
            let st3 = st2.clone();
            move |r: &Req| {
                let mut st = st3.lock().unwrap();
                let mut resp = route(&mut st, r);
                if let Some(s) = &st.scopes {
                    resp.headers.push(("x-oauth-scopes".into(), s.clone()));
                }
                resp
            }
        });
        FakeBoard { gh, st }
    }

    pub fn base(&self) -> String {
        self.gh.base()
    }

    pub fn requests(&self) -> Vec<Req> {
        self.gh.requests()
    }

    /// Requests that no handler knows. A test asserts that this is empty.
    pub fn unhandled(&self) -> Vec<String> {
        self.st.lock().unwrap().unhandled.clone()
    }

    // ---- seeding -------------------------------------------------------------------

    /// Add a comment by `login`. `created` and `updated` are Unix seconds; a comment with
    /// `updated != created` counts as edited. Returns the comment id.
    pub fn comment_at(
        &self,
        issue: u64,
        login: &str,
        body: &str,
        created: i64,
        updated: i64,
    ) -> u64 {
        let mut st = self.st.lock().unwrap();
        let id = st.next_comment;
        st.next_comment += 1;
        st.comments.push(Comment {
            id,
            repo: REPO.into(),
            issue,
            login: login.into(),
            body: body.into(),
            created,
            updated,
            editors: Vec::new(),
        });
        id
    }

    /// A comment by the owner's account, created 30 s ago.
    pub fn owner_comment(&self, issue: u64, body: &str) -> u64 {
        let t = now_epoch() - 30;
        self.comment_at(issue, OWNER, body, t, t)
    }

    /// A comment by the owner's account, created now: newer than any cursor that an earlier
    /// `poll` wrote.
    pub fn owner_comment_now(&self, issue: u64, body: &str) -> u64 {
        let t = now_epoch();
        self.comment_at(issue, OWNER, body, t, t)
    }

    /// Change the body of a comment and bump its `updated_at`, as an edit on GitHub does.
    pub fn edit_comment(&self, id: u64, body: &str) {
        let mut st = self.st.lock().unwrap();
        let c = st
            .comments
            .iter_mut()
            .find(|c| c.id == id)
            .expect("comment");
        c.body = body.into();
        c.updated = c.created + 10;
        c.editors.push(c.login.clone());
    }

    /// Like [`FakeBoard::edit_comment`], but `editor` made the edit (a user with Write can edit
    /// the comment of another user; the author stays the same).
    pub fn edit_comment_by(&self, id: u64, body: &str, editor: &str) {
        self.edit_comment(id, body);
        let mut st = self.st.lock().unwrap();
        let c = st
            .comments
            .iter_mut()
            .find(|c| c.id == id)
            .expect("comment");
        c.editors.pop();
        c.editors.push(editor.into());
    }

    pub fn comment_body(&self, id: u64) -> String {
        let st = self.st.lock().unwrap();
        st.comments
            .iter()
            .find(|c| c.id == id)
            .expect("comment")
            .body
            .clone()
    }

    /// All comments of an issue, oldest first: `(id, login, body)`.
    pub fn comments_of(&self, issue: u64) -> Vec<(u64, String, String)> {
        let st = self.st.lock().unwrap();
        st.comments
            .iter()
            .filter(|c| c.issue == issue)
            .map(|c| (c.id, c.login.clone(), c.body.clone()))
            .collect()
    }

    /// Add a board item (a draft issue) with field values. Returns its id.
    pub fn item(&self, id: &str, fields: &[(&str, &str)]) -> String {
        let mut st = self.st.lock().unwrap();
        st.items.insert(
            id.to_string(),
            Item {
                id: id.to_string(),
                fields: fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                updated: now_epoch() - 120,
                kind: "DRAFT_ISSUE",
                draft_body: String::new(),
            },
        );
        id.to_string()
    }

    /// Set a field value of an item directly (no mutation request).
    pub fn set_item(&self, id: &str, name: &str, value: &str) {
        let mut st = self.st.lock().unwrap();
        st.items
            .get_mut(id)
            .expect("item")
            .fields
            .insert(name.to_string(), value.to_string());
    }

    /// Mark an item as last changed `secs_ago` seconds ago.
    pub fn touch_item_ago(&self, id: &str, secs_ago: i64) {
        let mut st = self.st.lock().unwrap();
        st.items.get_mut(id).expect("item").updated = now_epoch() - secs_ago;
    }

    /// Mark an item as changed just now, as a board edit does.
    pub fn touch_item(&self, id: &str) {
        let mut st = self.st.lock().unwrap();
        st.items.get_mut(id).expect("item").updated = now_epoch();
    }

    /// A message that `from` signed `ago` seconds ago, posted as a comment created then (the
    /// signed `ts` and the comment agree, so the replay rule accepts it). Returns the id.
    #[allow(clippy::too_many_arguments)]
    pub fn signed_ago(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        body: &str,
        ago: i64,
    ) -> u64 {
        self.signed_at(key, from, to, kind, issue, body, now_epoch() - ago)
            .0
    }

    /// Make an item look as if it was last changed `secs` seconds ago.
    pub fn age_item(&self, id: &str, secs: i64) {
        let mut st = self.st.lock().unwrap();
        st.items.get_mut(id).expect("item").updated = now_epoch() - secs;
    }

    pub fn item_field(&self, id: &str, name: &str) -> Option<String> {
        let st = self.st.lock().unwrap();
        st.items.get(id)?.fields.get(name).cloned()
    }

    /// The body of the draft issue behind an agent item (the profile text, spec/profiles).
    pub fn draft_body(&self, id: &str) -> String {
        let st = self.st.lock().unwrap();
        st.items.get(id).expect("item").draft_body.clone()
    }

    /// Append options to the `Agent` field directly (no mutation request), as many earlier
    /// approvals would have left them.
    pub fn seed_agent_options(&self, names: &[String]) {
        let mut st = self.st.lock().unwrap();
        let f = st.fields.iter_mut().find(|f| f.name == "Agent").unwrap();
        for n in names {
            f.options.push((format!("PVTSSO_Agent_{n}"), n.clone()));
        }
    }

    /// Replace the options of a single-select field directly (no mutation request).
    pub fn set_field_options(&self, field: &str, names: &[String]) {
        let mut st = self.st.lock().unwrap();
        let f = st.fields.iter_mut().find(|f| f.name == field).unwrap();
        f.options = names
            .iter()
            .map(|n| (format!("PVTSSO_{field}_{n}"), n.clone()))
            .collect();
    }

    /// The option names of a single-select field, in order.
    pub fn field_options(&self, field: &str) -> Vec<String> {
        let st = self.st.lock().unwrap();
        let f = st.fields.iter().find(|f| f.name == field).unwrap();
        f.options.iter().map(|(_, n)| n.clone()).collect()
    }

    /// The option names of the `Agent` field, in order.
    pub fn agent_options(&self) -> Vec<String> {
        let st = self.st.lock().unwrap();
        let f = st.fields.iter().find(|f| f.name == "Agent").unwrap();
        f.options.iter().map(|(_, n)| n.clone()).collect()
    }

    /// The option ids of the `Agent` field, in order.
    pub fn agent_option_ids(&self) -> Vec<String> {
        let st = self.st.lock().unwrap();
        let f = st.fields.iter().find(|f| f.name == "Agent").unwrap();
        f.options.iter().map(|(id, _)| id.clone()).collect()
    }

    /// Add an open issue with labels to the issue list of the repo.
    pub fn issue(&self, number: u64, labels: &[&str]) {
        self.st.lock().unwrap().issues.push(IssueRow {
            repo: REPO.into(),
            number,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            title: format!("issue {number}"),
            body: String::new(),
            state: "open",
            locked: false,
        });
    }

    /// A joined and approved agent: a key, a board item, a signed join request and the
    /// owner's approval in the registry issue (both older than any cursor of the tests), and
    /// a config dir for the CLI. The item is `active`, with a fresh heartbeat and a cursor
    /// 10 minutes back.
    pub fn agent(&self, name: &str, role: Role) -> Agent {
        self.agent_with(name, role, true, OWNER)
    }

    /// [`FakeBoard::agent`], but the join is a comment of the GitHub account `login` (the
    /// agent's own account, or a shared crew account), and the approval names it.
    pub fn agent_as(&self, name: &str, role: Role, login: &str) -> Agent {
        self.agent_with(name, role, true, login)
    }

    /// [`FakeBoard::joined`], with the join posted by the GitHub account `login`.
    pub fn joined_as(&self, name: &str, login: &str) -> Agent {
        self.agent_with(name, Role::Dev, false, login)
    }

    /// An agent that has joined and waits for the owner: a key, a board item with
    /// `State = requested`, a signed join request an hour ago, no approval and no `Agent`
    /// option. `approve` is the next step.
    pub fn joined(&self, name: &str) -> Agent {
        self.agent_with(name, Role::Dev, false, OWNER)
    }

    fn agent_with(&self, name: &str, role: Role, approved: bool, login: &str) -> Agent {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let cd = ConfigDir(dir.clone());
        let an = AgentName::parse(name).unwrap();
        let (key, _) = load_or_create_key(&cd.key_path(&an)).unwrap();
        let item_id = format!("PVTI_{}", name.replace('@', "_"));
        let mut pubkey = key.public_key().clone();
        pubkey.set_comment(name);
        let key_line = pubkey.to_openssh().unwrap();
        let now = now_epoch();
        if approved {
            self.item(
                &item_id,
                &[
                    ("Key", &key_line),
                    (
                        "Role",
                        match role {
                            Role::Coordinator => "Coordinator",
                            Role::Dev => "Dev",
                        },
                    ),
                    ("State", "active"),
                    ("Heartbeat", &format_ts(now)),
                    ("Cursor", &format!("c1 ts={} seen=", format_ts(now - 600))),
                    ("Runtime", "fake/it"),
                    ("Host", "fake-host"),
                ],
            );
        } else {
            self.item(
                &item_id,
                &[
                    ("Key", &key_line),
                    ("State", "requested"),
                    ("Runtime", "fake/it"),
                    ("Host", "fake-host"),
                ],
            );
        }
        if role == Role::Dev && approved {
            // `approve` adds the agent as an option of the `Agent` field
            let mut st = self.st.lock().unwrap();
            let f = st.fields.iter_mut().find(|f| f.name == "Agent").unwrap();
            f.options
                .push((format!("PVTSSO_Agent_{name}"), name.to_string()));
        }
        let board = BoardConfig::parse(BOARD_TOML).unwrap();
        cd.save_agent(&AgentConfig {
            name: name.into(),
            runtime: "fake/it".into(),
            host: "fake-host".into(),
            item_id: item_id.clone(),
            board,
            tools: None,
            board_sha: None,
            loop_cfg: None,
            wake: None,
            insecure_one_account: false,
        })
        .unwrap();
        // the registry: a signed join, then the owner's approval, an hour ago
        let old = now - 3600;
        let join = sign_ts(
            &key,
            REGISTRY,
            name,
            "owner",
            Kind::Request,
            &join_body(&item_id, &key_line, "fake/it", "fake-host"),
            old,
        );
        self.comment_at(REGISTRY, login, &join, old, old);
        if approved {
            let approve = crewboard::owner_cmd::approve_text(
                &an,
                role,
                &fingerprint(key.public_key()),
                &item_id,
                login,
            );
            self.comment_at(REGISTRY, OWNER, &approve, old + 1, old + 1);
        }
        Agent {
            name: name.into(),
            role,
            key,
            item_id,
            dir,
            _tmp: tmp,
        }
    }

    /// The repo moves to another owner or name: GitHub reports `new` from now on, in every
    /// issue, comment, PR, branch, file and label. The signed messages stay as they are, so
    /// they still carry the old name inside their signature.
    pub fn rename_repo(&self, new: &str) {
        let mut st = self.st.lock().unwrap();
        let st = &mut *st;
        let old = st
            .issues
            .first()
            .map_or(REPO.to_string(), |i| i.repo.clone());
        st.moved_from.push((old.clone(), new.to_string()));
        st.comments.iter_mut().for_each(|c| c.repo = new.into());
        st.issues.iter_mut().for_each(|i| i.repo = new.into());
        st.prs.iter_mut().for_each(|p| p.repo = new.into());
        st.branches.iter_mut().for_each(|b| b.0 = new.into());
        st.files = std::mem::take(&mut st.files)
            .into_iter()
            .map(|((_, p), t)| ((new.to_string(), p), t))
            .collect();
        st.file_errors = std::mem::take(&mut st.file_errors)
            .into_iter()
            .map(|((_, p), c)| ((new.to_string(), p), c))
            .collect();
        for v in st.runs.iter_mut().chain(st.checks.iter_mut()) {
            *v = serde_json::from_str(&v.to_string().replace(&old, new)).unwrap();
        }
        st.labels = std::mem::take(&mut st.labels)
            .into_iter()
            .map(|((_, l), c)| ((new.to_string(), l), c))
            .collect();
    }

    /// A file on the default branch of `repo` (after [`FakeBoard::rename_repo`] the file
    /// helpers of the old name no longer apply).
    pub fn file_in(&self, repo: &str, path: &str, text: &str) {
        self.st
            .lock()
            .unwrap()
            .files
            .insert((repo.to_string(), path.to_string()), text.to_string());
    }

    /// The agent's config now names `repo` as its main repo (the host config change of the
    /// runbook).
    pub fn retarget_agent(&self, who: &Agent, repo: &str) {
        let cd = ConfigDir(who.dir.clone());
        let mut a = cd.load_agent_auto(None).unwrap();
        a.board.repos = vec![repo.to_string()];
        cd.save_agent(&a).unwrap();
    }

    /// A config dir with the board file of the owner for the main repo `repo`.
    pub fn owner_dir_for(&self, repo: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            ConfigDir(tmp.path().to_path_buf()).board_config_path(INITIATIVE),
            BOARD_TOML.replace(REPO, repo),
        )
        .unwrap();
        tmp
    }

    /// A config dir with the board file of the owner (for `audit` and `status`).
    pub fn owner_dir(&self) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            ConfigDir(tmp.path().to_path_buf()).board_config_path(INITIATIVE),
            BOARD_TOML,
        )
        .unwrap();
        tmp
    }

    /// A message that `from` signed with `key`, as a comment by the owner's account (agents
    /// share the owner's token in this setup), created 30 s ago. Returns `(id, text)`.
    pub fn signed(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        body: &str,
    ) -> (u64, String) {
        self.signed_at(key, from, to, kind, issue, body, now_epoch() - 30)
    }

    /// A message that was signed at `ts_epoch` and posted as a comment created 30 s ago: with an
    /// old `ts` it is a copy of an old message (the replay window refuses it).
    #[allow(clippy::too_many_arguments)]
    pub fn signed_with_ts(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        body: &str,
        ts_epoch: i64,
    ) -> u64 {
        let text = sign_ts(key, issue, from, to, kind, body, ts_epoch);
        let created = now_epoch() - 30;
        self.comment_at(issue, OWNER, &text, created, created)
    }

    /// Like [`FakeBoard::signed`], but created now: newer than any cursor that an earlier
    /// `poll` wrote.
    pub fn signed_now(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        body: &str,
    ) -> (u64, String) {
        self.signed_at(key, from, to, kind, issue, body, now_epoch())
    }

    #[allow(clippy::too_many_arguments)]
    fn signed_at(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        body: &str,
        created: i64,
    ) -> (u64, String) {
        let text = sign_ts(key, issue, from, to, kind, body, created);
        (self.comment_at(issue, OWNER, &text, created, created), text)
    }

    /// A message about the comment `re` of the same issue, signed `ago` seconds ago. Returns the id.
    #[allow(clippy::too_many_arguments)]
    pub fn signed_re_ago(
        &self,
        key: &PrivateKey,
        from: &str,
        to: &str,
        kind: Kind,
        issue: u64,
        re: u64,
        body: &str,
        ago: i64,
    ) -> u64 {
        let created = now_epoch() - ago;
        let re = Re::Comment { issue, comment: re };
        let text = sign_ts_re(key, issue, from, to, kind, re, body, created);
        self.comment_at(issue, OWNER, &text, created, created)
    }

    /// Put a file on the default branch of `repo` (`GET contents` serves it).
    pub fn set_file(&self, repo: &str, path: &str, text: &str) {
        let mut st = self.st.lock().unwrap();
        st.file_errors.remove(&(repo.into(), path.into()));
        st.files.insert((repo.into(), path.into()), text.into());
    }

    /// Make `GET contents` of a file answer with an HTTP error status.
    pub fn fail_file(&self, repo: &str, path: &str, status: u16) {
        self.st
            .lock()
            .unwrap()
            .file_errors
            .insert((repo.into(), path.into()), status);
    }

    /// The login of the token (`GET /user`). Default: the owner.
    pub fn set_viewer(&self, login: &str) {
        self.st.lock().unwrap().viewer = login.to_string();
    }

    /// The scopes header of every answer, as GitHub sends it for a classic token.
    pub fn set_scopes(&self, scopes: &str) {
        self.st.lock().unwrap().scopes = Some(scopes.to_string());
    }

    /// `permissions.push` of `GET repos/{r}`.
    pub fn set_push(&self, push: bool) {
        self.st.lock().unwrap().push = push;
    }

    /// The board query fails with this GraphQL error.
    pub fn fail_board(&self, message: &str) {
        self.st.lock().unwrap().board_error = Some(message.to_string());
    }

    /// The board query fails with this raw GraphQL `errors` array (to give an error `type`).
    pub fn fail_board_errors(&self, errors: Value) {
        self.st.lock().unwrap().board_errors = Some(errors);
    }

    /// A single-select write of `value` (for example `Done`) fails with a GraphQL error; `None`
    /// lets it pass again.
    pub fn fail_select_write(&self, value: Option<&str>) {
        self.fail_select_write_after(value, 0);
    }

    /// Every clear of a field value (for example the Agent) fails with a GraphQL error while `on`.
    pub fn fail_clear_write(&self, on: bool) {
        self.st.lock().unwrap().fail_clear = on;
    }

    /// Like [`Self::fail_select_write`], but the first `pass` writes of `value` succeed.
    pub fn fail_select_write_after(&self, value: Option<&str>, pass: u32) {
        let mut st = self.st.lock().unwrap();
        st.fail_select_value = value.map(str::to_string);
        st.fail_select_pass = pass;
    }

    /// The item listing leaves this item out for its next `listings` requests, as the lagging
    /// listing of GitHub does. Reading the item by id still works.
    pub fn unlist_item(&self, id: &str, listings: usize) {
        self.st
            .lock()
            .unwrap()
            .unlisted
            .insert(id.to_string(), listings);
    }

    /// The next option write (`updateProjectV2Field`) also sets `field` of `item` to `value`:
    /// a change behind crewboard in the middle of a run.
    pub fn flip_on_options_write(&self, item: &str, field: &str, value: &str) {
        self.st.lock().unwrap().flip_on_options_write.push((
            item.to_string(),
            field.to_string(),
            value.to_string(),
        ));
    }

    /// The next option write (`updateProjectV2Field`) also renames the option `from` to `to`
    /// behind the request, which the read-back must catch.
    pub fn rename_on_options_write(&self, from: &str, to: &str) {
        self.st
            .lock()
            .unwrap()
            .rename_on_options_write
            .push((from.to_string(), to.to_string()));
    }

    /// After `skip` reads of `item` by id, set `field` of it to `value`: a change behind the
    /// command between its first read and a later one.
    pub fn flip_after_item_reads(&self, item: &str, field: &str, value: &str, skip: u32) {
        self.st.lock().unwrap().flip_after_item_reads.push((
            item.to_string(),
            field.to_string(),
            value.to_string(),
            skip,
        ));
    }

    /// The next `times` option writes (`updateProjectV2Field`) lose the option that they create:
    /// a concurrent writer replaced the whole list right after the write.
    pub fn drop_new_option_on_write(&self, times: u32) {
        self.st.lock().unwrap().drop_new_option_on_write = times;
    }

    /// The built-in workflows of the project: `(name, enabled)`.
    pub fn set_workflows(&self, list: &[(&str, bool)]) {
        self.st.lock().unwrap().workflows = Ok(list
            .iter()
            .enumerate()
            .map(|(i, (n, e))| (format!("PVTW_{}", i + 1), n.to_string(), *e))
            .collect());
    }

    /// The six built-in workflows of a new project, all on, as GitHub makes them.
    pub fn set_default_workflows(&self) {
        self.set_workflows(&[
            ("Auto-add sub-issues to project", true),
            ("Auto-close issue", true),
            ("Item added to project", true),
            ("Item closed", true),
            ("Pull request linked to issue", true),
            ("Pull request merged", true),
        ]);
    }

    /// The names of the workflows that are on the project now, in order.
    pub fn workflow_names(&self) -> Vec<String> {
        match &self.st.lock().unwrap().workflows {
            Ok(l) => l.iter().map(|(_, n, _)| n.clone()).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The names of the workflows that were deleted by the mutation, in order.
    pub fn workflows_deleted(&self) -> Vec<String> {
        self.st.lock().unwrap().workflows_deleted.clone()
    }

    /// `deleteProjectV2Workflow` fails with this GraphQL error.
    pub fn fail_workflow_delete(&self, message: &str) {
        self.st.lock().unwrap().fail_workflow_delete = Some(message.to_string());
    }

    /// The `workflows` query fails with this GraphQL error, as for a token without the scope.
    pub fn fail_workflows(&self, message: &str) {
        self.st.lock().unwrap().workflows = Err(message.to_string());
    }

    /// Append a `project_v2_item_status_changed` event by `actor` to the timeline of an issue.
    pub fn status_changed_by(&self, issue: u64, actor: &str) {
        self.st
            .lock()
            .unwrap()
            .timelines
            .entry(issue)
            .or_default()
            .push(json!({
                "event": "project_v2_item_status_changed",
                "actor": {"login": actor, "type": "Bot"},
                "created_at": format_ts(now_epoch()),
            }));
    }

    /// The number of timeline reads so far.
    pub fn timeline_reads(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.method == "GET" && r.path.contains("/timeline"))
            .count()
    }

    /// The board file the agents of this fake were made with (`FakeBoard::agent`).
    pub fn board_toml() -> &'static str {
        BOARD_TOML
    }

    // ---- the CLI -------------------------------------------------------------------

    /// Run the CLI against this fake. `dir` is the config dir.
    pub fn run(&self, dir: &Path, args: &[&str]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(dir)
            .args(args)
            .env("CREWBOARD_API_BASE", self.base())
            .env("GH_TOKEN", "ghp_faketoken")
            // what the own `gh auth token` of the owner prints (owner commands)
            .env(crewboard::github::TEST_GH_AUTH_TOKEN_ENV, "ghp_faketoken")
            .env_remove("GITHUB_TOKEN")
            .output()
            .expect("run crewboard");
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        }
    }

    /// [`FakeBoard::run`] with extra environment variables and a working directory.
    pub fn run_env(&self, dir: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(dir)
            .args(args)
            .current_dir(cwd)
            .env("CREWBOARD_API_BASE", self.base())
            .env("GH_TOKEN", "ghp_faketoken")
            // what the own `gh auth token` of the owner prints (owner commands)
            .env(crewboard::github::TEST_GH_AUTH_TOKEN_ENV, "ghp_faketoken")
            .env_remove("GITHUB_TOKEN")
            .envs(env.iter().copied())
            .output()
            .expect("run crewboard");
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        }
    }

    /// `poll --json` of an agent. Returns the parsed last line and the exit code.
    pub fn poll_json(&self, who: &Agent) -> (Value, i32) {
        let o = self.run(&who.dir, &["poll", "--json"]);
        let last = o.stdout.lines().last().unwrap_or("").to_string();
        let v: Value = serde_json::from_str(&last)
            .unwrap_or_else(|e| panic!("poll --json is not JSON ({e}): {}", o.all()));
        (v, o.code)
    }
}

/// Sign a message of `issue` in the fake repo.
pub fn sign(key: &PrivateKey, issue: u64, from: &str, to: &str, kind: Kind, body: &str) -> String {
    sign_ts(key, issue, from, to, kind, body, now_epoch())
}

/// Like [`sign`], with the signed `ts` of `ts_epoch` (Unix seconds). A message is fresh when its
/// `ts` is inside the replay window of the creation time of its comment.
pub fn sign_ts(
    key: &PrivateKey,
    issue: u64,
    from: &str,
    to: &str,
    kind: Kind,
    body: &str,
    ts_epoch: i64,
) -> String {
    sign_ts_re(key, issue, from, to, kind, Re::None, body, ts_epoch)
}

/// [`sign_ts`] with a `re` that names the comment answered.
#[allow(clippy::too_many_arguments)]
pub fn sign_ts_re(
    key: &PrivateKey,
    issue: u64,
    from: &str,
    to: &str,
    kind: Kind,
    re: Re,
    body: &str,
    ts_epoch: i64,
) -> String {
    let issue_ref = IssueRef::parse(&format!("{REPO}#{issue}")).unwrap();
    sign_message(
        key,
        &issue_ref,
        Outgoing {
            from: AgentName::parse(from).unwrap(),
            to: To::parse(to).unwrap(),
            kind,
            re,
            ts: Timestamp::parse(&format_ts(ts_epoch)).unwrap(),
        },
        body,
    )
    .unwrap()
    .to_comment()
}

// ---- endpoints ---------------------------------------------------------------------

fn route(st: &mut State, r: &Req) -> Resp {
    // REST follows the redirect of a repo that moved; GraphQL by owner and name does not
    let moved = st.moved_from.iter().find_map(|(old, new)| {
        r.path
            .strip_prefix(&format!("/repos/{old}"))
            .filter(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
            .map(|rest| format!("/repos/{new}{rest}"))
    });
    let redirected;
    let r = match moved {
        Some(path) => {
            redirected = Req { path, ..r.clone() };
            &redirected
        }
        None => r,
    };
    if let Some(k) = st.fail_once.iter().position(|(m, p, b)| {
        *m == r.method && r.path.contains(p.as_str()) && r.body.contains(b.as_str())
    }) {
        st.fail_once.remove(k);
        return Resp::json(500, r#"{"message":"fake: injected failure"}"#);
    }
    let (path, query) = r.path.split_once('?').unwrap_or((&r.path, ""));
    let seg: Vec<&str> = path.split('/').collect();
    match (r.method.as_str(), seg.as_slice()) {
        ("GET", ["", "user"]) => get_user(st),
        ("GET", ["", "users", login]) => get_owner(login),
        ("GET", ["", "rate_limit"]) => get_rate_limit(),
        ("POST", ["", "graphql"]) => graphql(st, r),
        ("GET", ["", "repos", o, n, "issues", "comments"]) => {
            get_comment_feed(st, &format!("{o}/{n}"), query)
        }
        ("GET", ["", "repos", o, n, "issues", num, "comments"]) => {
            get_issue_comments(st, &format!("{o}/{n}"), num)
        }
        ("GET", ["", "repos", _, _, "issues", num, "timeline"]) => {
            let n: u64 = num.parse().unwrap_or(0);
            ok(Value::Array(
                st.timelines.get(&n).cloned().unwrap_or_default(),
            ))
        }
        ("POST", ["", "repos", o, n, "issues", num, "comments"]) => {
            post_issue_comment(st, &format!("{o}/{n}"), num, &r.body)
        }
        ("GET", ["", "repos", o, n, "issues"]) => get_issue_list(st, &format!("{o}/{n}"), query),
        ("GET", ["", "repos", o, n]) => tickets::get_repo(st, &format!("{o}/{n}")),
        ("POST", ["", "repos", o, n, "issues"]) => {
            tickets::post_issue(st, &format!("{o}/{n}"), &r.body)
        }
        ("GET", ["", "repos", o, n, "issues", num]) => {
            tickets::get_issue(st, &format!("{o}/{n}"), num)
        }
        ("PATCH", ["", "repos", o, n, "issues", num]) => {
            tickets::patch_issue(st, &format!("{o}/{n}"), num, &r.body)
        }
        ("PUT", ["", "repos", _, _, "issues", n, "lock"]) => tickets::lock_issue(st, n),
        ("DELETE", ["", "repos", o, n, "issues", "comments", id]) => {
            tickets::delete_comment(st, &format!("{o}/{n}"), id)
        }
        ("POST", ["", "repos", o, n, "labels"]) => {
            tickets::post_label(st, &format!("{o}/{n}"), &r.body)
        }
        ("POST", ["", "repos", o, n, "issues", num, "labels"]) => {
            tickets::add_issue_label(st, &format!("{o}/{n}"), num, &r.body)
        }
        ("DELETE", ["", "repos", o, n, "issues", num, "labels", name]) => {
            tickets::remove_issue_label(st, &format!("{o}/{n}"), num, name)
        }
        ("GET", ["", "repos", o, n, "pulls"]) => {
            tickets::get_pull_list(st, &format!("{o}/{n}"), query)
        }
        ("POST", ["", "repos", o, n, "pulls"]) => {
            project::post_pull(st, &format!("{o}/{n}"), &r.body)
        }
        ("GET", ["", "orgs", _, "projectsV2", _, "fields"]) => project::get_rest_fields(st),
        ("POST", ["", "orgs", _, "projectsV2", _, "views"]) => project::post_view(st, &r.body),
        (
            "GET",
            [
                "",
                "repos",
                o,
                n,
                "git",
                "matching-refs",
                "heads",
                prefix @ ..,
            ],
        ) => tickets::get_matching_refs(st, &format!("{o}/{n}"), &prefix.join("/")),
        ("GET", ["", "repos", o, n, "git", "ref", "heads", branch @ ..]) => {
            project::get_ref(st, &format!("{o}/{n}"), &branch.join("/"))
        }
        ("POST", ["", "repos", o, n, "git", "refs"]) => {
            project::post_ref(st, &format!("{o}/{n}"), &r.body)
        }
        ("PUT", ["", "repos", o, n, "contents", file @ ..]) => {
            project::put_contents(st, &format!("{o}/{n}"), &file.join("/"), &r.body)
        }
        ("GET", ["", "repos", o, n, "pulls", num]) => {
            tickets::get_pull(st, &format!("{o}/{n}"), num)
        }
        ("GET", ["", "repos", o, n, "compare", spec @ ..]) => {
            tickets::get_compare(st, &format!("{o}/{n}"), &spec.join("/"))
        }
        ("GET", ["", "repos", o, n, "pulls", num, "files"]) => {
            tickets::get_pull_files(st, &format!("{o}/{n}"), num, query)
        }
        ("PUT", ["", "repos", o, n, "pulls", num, "merge"]) => {
            tickets::put_merge(st, &format!("{o}/{n}"), num, &r.body)
        }
        ("DELETE", ["", "repos", o, n, "git", "refs", "heads", branch @ ..]) => {
            tickets::delete_branch(st, &format!("{o}/{n}"), &branch.join("/"))
        }
        ("GET", ["", "repos", o, n, "contents", file @ ..]) => {
            tickets::get_contents(st, &format!("{o}/{n}"), &file.join("/"), query)
        }
        ("GET", ["", "repos", _, _, "commits"]) => tickets::get_commits(st, query),
        ("GET", ["", "repos", _, _, "actions", "runs"]) => tickets::get_workflow_runs(st, query),
        ("GET", ["", "repos", o, n, "actions", "runs", id]) => {
            tickets::get_workflow_run(st, &format!("{o}/{n}"), id)
        }
        ("GET", ["", "repos", _, _, "commits", sha, "check-runs"]) => {
            tickets::get_check_runs(st, sha)
        }
        ("GET", ["", "repos", _, _, "commits", sha, "status"]) => tickets::get_combined_status(sha),
        ("GET", ["", "repos", _, _, "commits", _, "statuses"]) => tickets::get_statuses(),
        (
            "GET",
            [
                "",
                "repos",
                _,
                _,
                "branches",
                _,
                "protection",
                "required_status_checks",
            ],
        ) => tickets::get_required_checks(),
        ("GET", ["", "repos", _, _, "branches", b]) => tickets::get_branch(st, b),
        ("GET", ["", "repos", _, _, "rules", "branches", _]) => tickets::get_branch_rules(st),
        ("GET", ["", "repos", _, _, "rulesets"]) => tickets::list_rulesets(st),
        ("POST", ["", "repos", _, _, "rulesets"]) => tickets::post_ruleset(st, &r.body),
        _ => {
            st.unhandled.push(format!("{} {}", r.method, r.path));
            Resp::json(
                501,
                &json!({"message": format!("fake: no handler for {} {}", r.method, r.path)})
                    .to_string(),
            )
        }
    }
}

fn ok(v: Value) -> Resp {
    Resp::json(200, &v.to_string())
}

/// `GET /user`
fn get_user(st: &State) -> Resp {
    ok(json!({"login": st.viewer, "id": 1, "type": "User", "node_id": "U_fake"}))
}

/// `GET /users/{login}`: the owner of the project (an organisation).
fn get_owner(login: &str) -> Resp {
    ok(json!({"login": login, "id": 2, "type": "Organization", "node_id": "O_fake"}))
}

/// `GET /rate_limit`
fn get_rate_limit() -> Resp {
    let res = json!({"limit": 5000, "remaining": 4999, "used": 1, "reset": now_epoch() + 3600});
    ok(json!({"resources": {"core": res, "graphql": res}, "rate": res}))
}

fn comment_json(st: &State, c: &Comment) -> Value {
    json!({
        "id": c.id,
        "node_id": format!("IC_{}", c.id),
        "url": format!("{}/repos/{}/issues/comments/{}", st.base, c.repo, c.id),
        "html_url": format!("{}/{}/issues/{}#issuecomment-{}", st.base, c.repo, c.issue, c.id),
        "issue_url": format!("{}/repos/{}/issues/{}", st.base, c.repo, c.issue),
        "user": {"login": c.login, "id": 1, "type": "User"},
        "created_at": format_ts(c.created),
        "updated_at": format_ts(c.updated),
        "author_association": "OWNER",
        "body": c.body,
    })
}

/// `GET /repos/{repo}/issues/{n}/comments`: oldest first.
fn get_issue_comments(st: &State, repo: &str, num: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    let list: Vec<Value> = st
        .comments
        .iter()
        .filter(|c| c.repo == repo && c.issue == n)
        .map(|c| comment_json(st, c))
        .collect();
    ok(Value::Array(list))
}

/// `POST /repos/{repo}/issues/{n}/comments`: the comment is by the token user.
fn post_issue_comment(st: &mut State, repo: &str, num: &str, body: &str) -> Resp {
    let n: u64 = num.parse().unwrap_or(0);
    let text = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("body").and_then(Value::as_str).map(str::to_string));
    let Some(text) = text else {
        return Resp::json(422, r#"{"message":"Validation Failed"}"#);
    };
    let now = now_epoch();
    let c = Comment {
        id: st.next_comment,
        repo: repo.into(),
        issue: n,
        login: st.viewer.clone(),
        body: text,
        created: now,
        updated: now,
        editors: Vec::new(),
    };
    st.next_comment += 1;
    let v = comment_json(st, &c);
    st.comments.push(c);
    Resp::json(201, &v.to_string())
}

/// `GET /repos/{repo}/issues/comments?since=...`: the feed, newest update first.
fn get_comment_feed(st: &State, repo: &str, query: &str) -> Resp {
    let since = query
        .split('&')
        .find_map(|p| p.strip_prefix("since="))
        .and_then(crewboard::clock::parse_ts)
        .unwrap_or(0);
    let mut list: Vec<&Comment> = st
        .comments
        .iter()
        .filter(|c| c.repo == repo && c.updated >= since)
        .collect();
    list.sort_by_key(|c| std::cmp::Reverse((c.updated, c.id)));
    ok(Value::Array(
        list.into_iter().map(|c| comment_json(st, c)).collect(),
    ))
}

/// `GET /repos/{repo}/issues?labels=...&state=open`
fn get_issue_list(st: &State, repo: &str, query: &str) -> Resp {
    let label = query
        .split('&')
        .find_map(|p| p.strip_prefix("labels="))
        .map(|l| l.replace("%3A", ":"));
    // GitHub lists open issues unless the query says `state=closed` or `state=all`.
    let state = query
        .split('&')
        .find_map(|p| p.strip_prefix("state="))
        .map(str::to_string);
    let list: Vec<Value> = st
        .issues
        .iter()
        .filter(|i| i.repo == repo)
        .filter(|i| label.as_ref().is_none_or(|l| i.labels.contains(l)))
        .filter(|i| match state.as_deref() {
            Some("open") | None => i.state == "open",
            Some("closed") => i.state == "closed",
            _ => true,
        })
        .map(|i| {
            json!({
                "number": i.number,
                "node_id": tickets::issue_node(&i.repo, i.number),
                "title": i.title,
                "body": i.body,
                "state": i.state,
                "html_url": format!("{}/{}/issues/{}", st.base, i.repo, i.number),
                "labels": i.labels.iter().map(|l| json!({"name": l})).collect::<Vec<_>>(),
            })
        })
        .collect();
    ok(Value::Array(list))
}

fn graphql(st: &mut State, r: &Req) -> Resp {
    let Ok(v) = serde_json::from_str::<Value>(&r.body) else {
        return Resp::json(400, r#"{"message":"Problems parsing JSON"}"#);
    };
    let q = v["query"].as_str().unwrap_or("");
    let vars = &v["variables"];
    let data = if q.contains("deleteProjectV2Workflow") {
        if let Some(m) = &st.fail_workflow_delete {
            return graphql_error(m);
        }
        let id = vars["w"].as_str().unwrap_or("");
        let Ok(list) = &mut st.workflows else {
            return graphql_error("workflows are not readable");
        };
        let Some(i) = list.iter().position(|(wid, _, _)| wid == id) else {
            return graphql_error(&format!(
                "Could not resolve to a node with the id of '{id}'"
            ));
        };
        let (_, name, _) = list.remove(i);
        st.workflows_deleted.push(name);
        json!({"deleteProjectV2Workflow": {"deletedWorkflowId": id}})
    } else if q.contains("workflows(") {
        // the built-in project workflows
        match &st.workflows {
            Ok(list) => json!({"organization": {"projectV2": {"workflows": {"nodes":
                list.iter().enumerate().map(|(i, (id, name, on))| json!({
                    "id": id, "number": i + 1, "name": name, "enabled": on
                })).collect::<Vec<_>>()
            }}}}),
            Err(m) => return graphql_error(m),
        }
    } else if let Some(d) = project::graphql(st, q, vars) {
        match d {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("projectV2(number") {
        if let Some(m) = &st.board_error {
            return graphql_error(m);
        }
        if let Some(e) = &st.board_errors {
            return ok(json!({"data": null, "errors": e}));
        }
        gql_board_load(st)
    } else if q.contains("addProjectV2DraftIssue") {
        // `join` makes the agent item of a new agent
        let id = format!("PVTI_new_{}", st.items.len() + 1);
        st.items.insert(
            id.clone(),
            Item {
                id: id.clone(),
                fields: BTreeMap::new(),
                updated: now_epoch(),
                kind: "DRAFT_ISSUE",
                draft_body: vars["b"].as_str().unwrap_or("").to_string(),
            },
        );
        json!({"addProjectV2DraftIssue": {"projectItem": {"id": id}}})
    } else if q.contains("DraftIssue{id body}") {
        tickets::gql_read_draft(st, vars)
    } else if q.contains("updateProjectV2DraftIssue") {
        match tickets::gql_set_draft_body(st, vars) {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("addProjectV2ItemById") {
        match tickets::gql_add_issue_item(st, vars) {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("clearProjectV2ItemFieldValue") {
        if st.fail_clear {
            return graphql_error("the clear of the field failed");
        }
        match tickets::gql_clear_field(st, vars) {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("on IssueComment") {
        gql_comment_editors(st, vars)
    } else if q.contains("nodes(ids:") {
        gql_items_by_id(st, vars)
    } else if q.contains("updateProjectV2ItemFieldValue") {
        match gql_set_field(st, vars, q.contains("singleSelectOptionId")) {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("updateProjectV2Field(") {
        match gql_update_field_options(st, vars) {
            Ok(d) => d,
            Err(m) => return graphql_error(&m),
        }
    } else if q.contains("items(first:") {
        gql_item_list(st, q, vars)
    } else {
        st.unhandled.push(format!("graphql {q}"));
        return Resp::json(
            501,
            &json!({"message": "fake: no handler for this GraphQL query"}).to_string(),
        );
    };
    ok(json!({"data": data}))
}

/// The editors query of `feed::resolve_owner_edits`: `editor` is the last editor,
/// `userContentEdits` lists every edit.
fn gql_comment_editors(st: &State, vars: &Value) -> Value {
    let nodes: Vec<Value> = vars["ids"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|id| {
            let id = id.as_str().unwrap_or("");
            match st
                .comments
                .iter()
                .find(|c| format!("IC_{}", c.id) == id)
            {
                None => Value::Null,
                Some(c) => json!({
                    "id": id,
                    "editor": c.editors.last().map(|l| json!({"login": l})),
                    "userContentEdits": {
                        "totalCount": c.editors.len(),
                        "nodes": c.editors.iter().map(|l| json!({"editor": {"login": l}})).collect::<Vec<_>>(),
                    },
                }),
            }
        })
        .collect();
    json!({ "nodes": nodes })
}

fn graphql_error(message: &str) -> Resp {
    ok(json!({"data": null, "errors": [{"message": message}]}))
}

/// `organization(login){projectV2(number)}`
fn gql_board_load(st: &State) -> Value {
    let nodes: Vec<Value> = st
        .fields
        .iter()
        .map(|f| {
            if f.data_type == "SINGLE_SELECT" {
                json!({
                    "__typename": "ProjectV2SingleSelectField",
                    "id": f.id, "name": f.name, "dataType": f.data_type,
                    "options": f.options.iter().map(|(id, name)| json!({
                        "id": id, "name": name, "color": "GRAY", "description": ""
                    })).collect::<Vec<_>>(),
                })
            } else {
                json!({"__typename": "ProjectV2Field", "id": f.id, "name": f.name,
                       "dataType": f.data_type})
            }
        })
        .collect();
    json!({"organization": {"projectV2": {
        "id": "PVT_fake", "number": PROJECT, "title": "fake board",
        "url": format!("{}/orgs/{OWNER}/projects/{PROJECT}", st.base), "public": false,
        "fields": {"nodes": nodes},
    }}})
}

fn item_json(st: &State, it: &Item) -> Value {
    let values: Vec<Value> = it
        .fields
        .iter()
        .filter_map(|(name, val)| {
            let f = st.fields.iter().find(|f| &f.name == name)?;
            Some(if f.data_type == "SINGLE_SELECT" {
                json!({"__typename": "ProjectV2ItemFieldSingleSelectValue", "name": val,
                       "field": {"name": name}})
            } else if f.data_type == "NUMBER" {
                json!({"__typename": "ProjectV2ItemFieldNumberValue",
                       "number": val.parse::<f64>().unwrap_or(0.0), "field": {"name": name}})
            } else {
                json!({"__typename": "ProjectV2ItemFieldTextValue", "text": val,
                       "field": {"name": name}})
            })
        })
        .collect();
    // The issue behind a ticket item (`PVTI_issue_<n>`) gives its state as `content.state`.
    let state = it
        .id
        .strip_prefix("PVTI_issue_")
        .and_then(|n| n.parse::<u64>().ok())
        .and_then(|n| st.issues.iter().find(|i| i.number == n))
        .map(|i| i.state.to_uppercase());
    let content = state.map_or(json!({}), |s| json!({"state": s}));
    json!({
        "id": it.id, "updatedAt": format_ts(it.updated), "type": it.kind,
        "isArchived": false, "project": {"number": PROJECT}, "content": content,
        "fieldValues": {"nodes": values},
    })
}

/// `nodes(ids:)`: an unknown id is `null`.
fn gql_items_by_id(st: &mut State, vars: &Value) -> Value {
    let ids: Vec<String> = vars["ids"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut hooks = std::mem::take(&mut st.flip_after_item_reads);
    hooks.retain_mut(|(item, field, value, skip)| {
        if !ids.contains(item) {
            return true;
        }
        if *skip > 0 {
            *skip -= 1;
            return true;
        }
        if let Some(it) = st.items.get_mut(item.as_str()) {
            it.fields.insert(field.clone(), value.clone());
        }
        false
    });
    st.flip_after_item_reads = hooks;
    let nodes: Vec<Value> = vars["ids"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|id| {
            id.as_str()
                .and_then(|id| st.items.get(id))
                .map_or(Value::Null, |it| item_json(st, it))
        })
        .collect();
    json!({"nodes": nodes})
}

/// `updateProjectV2ItemFieldValue` with a text or a single-select value.
fn gql_set_field(st: &mut State, vars: &Value, select: bool) -> Result<Value, String> {
    let (item, fid) = (
        vars["i"].as_str().unwrap_or(""),
        vars["f"].as_str().unwrap_or(""),
    );
    let f = st
        .fields
        .iter()
        .find(|f| f.id == fid)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{fid}'"))?
        .clone();
    let value = if let Some(n) = vars["n"].as_f64() {
        format!("{}", n as i64)
    } else if select {
        let oid = vars["o"].as_str().unwrap_or("");
        f.options
            .iter()
            .find(|(id, _)| id == oid)
            .map(|(_, n)| n.clone())
            .ok_or_else(|| format!("option {oid} is not an option of {}", f.name))?
    } else {
        vars["t"].as_str().unwrap_or("").to_string()
    };
    if select && st.fail_select_value.as_deref() == Some(value.as_str()) {
        if st.fail_select_pass == 0 {
            return Err(format!("the write of {value} failed"));
        }
        st.fail_select_pass -= 1;
    }
    let it = st
        .items
        .get_mut(item)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{item}'"))?;
    it.fields.insert(f.name, value);
    it.updated = now_epoch();
    Ok(json!({"updateProjectV2ItemFieldValue": {"projectV2Item": {"id": item}}}))
}

/// `updateProjectV2Field` with `singleSelectOptions`: GitHub replaces the whole list. An option
/// sent with a known id keeps it; one sent without an id gets a new id. Item values of an
/// option that is gone are cleared; a renamed option renames its values. More
/// than [`OPTION_LIMIT`] options fail with GitHub's error and change nothing.
fn gql_update_field_options(st: &mut State, vars: &Value) -> Result<Value, String> {
    let fid = vars["f"].as_str().unwrap_or("").to_string();
    let sent = vars["o"].as_array().cloned().unwrap_or_default();
    if sent.len() > OPTION_LIMIT {
        return Err(OPTION_LIMIT_ERROR.to_string());
    }
    let k = st
        .fields
        .iter()
        .position(|f| f.id == fid)
        .ok_or_else(|| format!("Could not resolve to a node with the global id of '{fid}'"))?;
    let old = st.fields[k].options.clone();
    let mut new = Vec::new();
    for o in &sent {
        let name = o["name"].as_str().unwrap_or("").to_string();
        let id = match o["id"].as_str() {
            Some(id) => id.to_string(),
            None => {
                st.next_option += 1;
                format!("PVTSSO_new_{}", st.next_option)
            }
        };
        new.push((id, name));
    }
    let fname = st.fields[k].name.clone();
    for it in st.items.values_mut() {
        let Some(v) = it.fields.get(&fname).cloned() else {
            continue;
        };
        let Some((oid, _)) = old.iter().find(|(_, n)| *n == v) else {
            continue;
        };
        match new.iter().find(|(id, _)| id == oid) {
            Some((_, n)) => {
                it.fields.insert(fname.clone(), n.clone());
            }
            None => {
                it.fields.remove(&fname);
            }
        }
    }
    st.fields[k].options = new;
    if st.drop_new_option_on_write > 0 {
        st.drop_new_option_on_write -= 1;
        st.fields[k]
            .options
            .retain(|(id, _)| !id.starts_with("PVTSSO_new_"));
    }
    for (from, to) in std::mem::take(&mut st.rename_on_options_write) {
        for (_, n) in st.fields[k].options.iter_mut().filter(|(_, n)| *n == from) {
            *n = to.clone();
        }
    }
    for (item, field, value) in std::mem::take(&mut st.flip_on_options_write) {
        if let Some(it) = st.items.get_mut(&item) {
            it.fields.insert(field, value);
        }
    }
    Ok(json!({"updateProjectV2Field": {"projectV2Field": {"id": fid}}}))
}

/// `node(id){... on ProjectV2{items(first:100)}}`: the item listing. With
/// `v:fieldValueByName(name:$f)` each node carries the single-select value of field `$f`.
fn gql_item_list(st: &mut State, q: &str, vars: &Value) -> Value {
    let by_name = q.contains("fieldValueByName");
    let fname = vars["f"].as_str().unwrap_or("");
    let hidden: Vec<String> = st.unlisted.keys().cloned().collect();
    for n in st.unlisted.values_mut() {
        *n -= 1;
    }
    st.unlisted.retain(|_, n| *n > 0);
    let nodes: Vec<Value> = st
        .items
        .values()
        .filter(|it| !hidden.contains(&it.id))
        .map(|it| {
            if !by_name {
                return json!({"id": it.id});
            }
            let f = st.fields.iter().find(|f| f.name == fname);
            let v = it.fields.get(fname).map(|name| {
                let oid = f.and_then(|f| f.options.iter().find(|(_, n)| n == name));
                json!({"name": name, "optionId": oid.map(|(id, _)| id.clone())})
            });
            json!({"id": it.id, "v": v})
        })
        .collect();
    json!({"node": {"items": {
        "pageInfo": {"hasNextPage": false, "endCursor": null},
        "nodes": nodes,
    }}})
}
