//! `init` (spec/cli#commands, spec/board#fields, spec/board#views): create or adopt the board of an
//! initiative.
//!
//! Idempotent. Every step first looks at what exists and creates only what is missing:
//! a project (or `--project <n>` to adopt one), repo links, fields and single-select
//! options (complete-list merge with read-back, never a removal), the four views (REST),
//! labels (HTTP 422 means "reuse"), the registry issue, and its pin (3-pin limit: report,
//! never unpin). It opens one pull request in the main repo with `AUTHORITY.md` and
//! `.crewboard.toml`. It never merges.

use super::{Global, new_client};
use crate::board::{
    Board, OwnerInfo, OwnerKind, default_color, graphql_root, owner_info, repo_path, rest_base,
};
use crate::config::{BoardConfig, INITIATIVE_FILE, split_repo, valid_initiative_name};
use crate::ctx::decode_contents;
use crate::error::{CbError, CbResult};
use crate::github::projects;
use crate::github::{Client, GhError, is_already_exists};
use crate::rules::options::{is_noop, plan_options};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

pub struct InitArgs<'a> {
    pub initiative: &'a str,
    pub repos: &'a [String],
    pub project: Option<u64>,
    pub title: Option<&'a str>,
    pub owner_login: Option<&'a str>,
    /// GitHub login of the account all agents act as (machine account). It is written to
    /// `.crewboard.toml` as `crew_login` and added to the board with the Write role.
    pub crew_login: Option<&'a str>,
    /// The owner's consent to a board without a crew account (a one-person trial). It is
    /// written to `.crewboard.toml` as `insecure_one_account`.
    pub insecure_one_account: bool,
    pub no_pr: bool,
    /// Do not create the branch rule on the default branch of the main repo.
    pub no_branch_rule: bool,
    /// Add a minimal CI workflow to the init PR when the main repo has none.
    pub with_ci: bool,
}

/// The single-select fields of spec/board#fields: name, the options that must exist, and the
/// options a NEW field is created with. GitHub refuses a single-select field with no option, so
/// `Agent` (whose options are added at approval) is created with one placeholder,
/// `unassigned`; an existing `Agent` field is left alone.
// `ensure_fields` never removes an option: an option that an adopted board holds beyond the
// wanted ones stays.
pub const SELECT_FIELDS: &[(&str, &[&str], &[&str])] = &[
    ("Kind", &["Work", "Agent"], &["Work", "Agent"]),
    (
        "Status",
        &[
            "Backlog",
            "Ready",
            "In Progress",
            "Review",
            "Blocked",
            "Done",
        ],
        &[
            "Backlog",
            "Ready",
            "In Progress",
            "Review",
            "Blocked",
            "Done",
        ],
    ),
    ("Agent", &[], &["unassigned"]),
    ("Role", &["Coordinator", "Dev"], &["Coordinator", "Dev"]),
    (
        "State",
        &[
            "requested",
            "approved",
            "active",
            "paused",
            "draining",
            "left",
        ],
        &[
            "requested",
            "approved",
            "active",
            "paused",
            "draining",
            "left",
        ],
    ),
];

/// The number fields of spec/board#fields. `Order` is the queue position of a Ready ticket within
/// the queue of its agent; `init` adds it to existing boards.
pub const NUMBER_FIELDS: &[&str] = &["Order"];

/// The text fields of spec/board#fields.
pub const TEXT_FIELDS: &[&str] = &[
    "Gate",
    "Evidence",
    "Heartbeat",
    "Host",
    "Runtime",
    "Key",
    "Doing",
    "Cursor",
    // facts and declared tags of an agent (spec/profiles); `init` adds it to existing boards
    "Capabilities",
];

/// Built-in option names that GitHub gives a new project (renamed, so their ids stay).
const STATUS_RENAMES: &[(&str, &str)] = &[("Todo", "Backlog")];

pub const LABELS: &[(&str, &str, &str)] = &[
    ("crew:registry", "5319e7", "Crew registry issue"),
    ("crew:work", "0e8a16", "Crew work ticket"),
    (
        crate::labels::NEEDS_OWNER,
        crate::labels::NEEDS_OWNER_COLOR,
        crate::labels::NEEDS_OWNER_DESC,
    ),
];

pub fn authority_md(initiative: &str, owner: &str) -> String {
    format!(
        "---\n\
# ENFORCED: crewboard reads these keys and refuses what they forbid\n\
initiative: {initiative}\n\
owner: {owner}\n\
coordinator_may_create_tickets: true\n\
coordinator_may_split_tickets: true\n\
coordinator_may_merge: false        # true only if the owner allows it\n\
dev_may_open_prs: true\n\
dev_branch_prefix: crew/\n\
protected: [main]\n\
# protected_paths: [tests/**, scripts/ci-*.sh]  # more paths that only the owner merges; adds to the built-in seven\n\
# ci_ignore: [deploy-preview]       # CI jobs that are red for reasons outside a ticket; not counted\n\
# ci_mode: local                    # the owner switches CI off: agents deliver with --local-run\n\
# former_repos:                     # after a repo transfer or rename: the old name and an exclusive cutoff\n\
#   - {{ name: old-owner/old-repo, until: 2026-11-01T12:00:00Z }}\n\
# ADVICE: crewboard does not read these keys; the models read them as the owner's rules\n\
needs_owner: [release, credentials, delete, new-scope, new-repo]\n\
---\n\n\
# Authority of initiative {initiative}\n\n\
This file tells the agents what they may do. The owner ({owner}) edits it. Agents read it at\n\
each `crewboard start` and obey the front matter. Describe the scope of the initiative here in\n\
plain words: what the goal is, what is not in scope, which repos belong to it.\n\n\
A PR that changes this file, `.crewboard.toml`, `.github/workflows/**`, `.github/actions/**` or\n\
`.github/CODEOWNERS`, `CODEOWNERS` or `docs/CODEOWNERS` is merged by the owner only, whatever the keys say.\n"
    )
}

pub fn initiative_file(cfg: &BoardConfig) -> String {
    format!(
        "# crewboard initiative file (no secrets). Agents find the board through it.\n{}",
        cfg.to_toml()
    )
}

/// Content of a file on a ref, or `None` on 404. Returns `(text, sha)`.
fn file_on(
    client: &Client,
    repo: &str,
    path: &str,
    git_ref: &str,
) -> CbResult<Option<(String, String)>> {
    match client.get(&format!("repos/{repo}/contents/{path}?ref={git_ref}")) {
        Ok(r) => Ok(Some((
            decode_contents(&r.body).unwrap_or_default(),
            r.body
                .get("sha")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ))),
        Err(GhError::Http { status: 404, .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[derive(Default)]
pub struct Report(String);

impl Report {
    /// Print at once: if a later step fails, the steps that were done stay visible.
    pub fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

/// The arguments that need no GitHub read: a valid name and valid repos. `--crew-login` and
/// `--insecure-one-account` together are refused by the CLI parser (one identity mode).
fn check_args(a: &InitArgs<'_>) -> CbResult<()> {
    if !valid_initiative_name(a.initiative) {
        return Err(CbError::config(format!(
            "initiative '{}' must be 1-40 characters of a-z, 0-9, '-'",
            a.initiative
        )));
    }
    for r in a.repos {
        split_repo(r)?;
    }
    Ok(())
}

/// The token must be the owner's, and it must have the `project` scope. Returns the owner login.
/// Nothing is written yet.
fn check_owner(client: &Client, a: &InitArgs<'_>, rep: &mut Report) -> CbResult<String> {
    let viewer = client.viewer_login()?;
    // before any write: the board needs the `project` scope of the owner token
    check_owner_scope(client, &viewer)?;
    let owner_login = a.owner_login.map_or(viewer.clone(), str::to_string);
    if !owner_login.eq_ignore_ascii_case(&viewer) {
        return Err(CbError::refused(
            "not-owner",
            format!(
                "init runs with the owner's token: this token is {viewer}, the owner is {owner_login}"
            ),
        ));
    }
    rep.line(format!("owner: {owner_login}"));
    if a.insecure_one_account {
        rep.line("WARNING: one-account board: the agents use the owner's token and nothing separates owner from agent. Use it for a one-person trial only; for a crew run init without --insecure-one-account.");
    } else if a.crew_login.is_none() {
        rep.line("identity: own accounts: each agent joins with its own GitHub account, never yours. Invite each contributor account as a collaborator with the Write role on the repos and on the Project; `approve` shows the account of each join. (A shared crew account: init --crew-login <account>.)");
    }
    Ok(owner_login)
}

/// The node ids of the repos, and the default branch of the first (main) repo.
fn read_repos(
    client: &Client,
    repos: &[String],
    rep: &mut Report,
) -> CbResult<(Vec<String>, String)> {
    let mut repo_ids = Vec::new();
    let mut default_branch = String::from("main");
    for (i, r) in repos.iter().enumerate() {
        let info = client.get(&repo_path(r)?)?;
        let id = info
            .body
            .get("node_id")
            .and_then(Value::as_str)
            .ok_or_else(|| GhError::Shape(format!("repo {r} has no node_id")))?;
        repo_ids.push(id.to_string());
        if i == 0 {
            default_branch = info
                .body
                .get("default_branch")
                .and_then(Value::as_str)
                .unwrap_or("main")
                .to_string();
        }
        if info.body.get("private").and_then(Value::as_bool) == Some(false) {
            rep.line(format!(
                "WARNING: {r} is PUBLIC. Messages, tickets and agent items would be world-readable (docs/design/spec/messages.md#untrusted-input)."
            ));
        }
    }
    Ok((repo_ids, default_branch))
}

/// Adopts the project `--project <n>` or finds or creates one.
fn adopt_or_create_project(
    client: &Client,
    a: &InitArgs<'_>,
    project_owner: &str,
    owner: &OwnerInfo,
    rep: &mut Report,
) -> CbResult<Board> {
    let board = match a.project {
        Some(n) => {
            let b = Board::load(client, project_owner, n)?;
            rep.line(format!("project {n} adopted: {} ({})", b.title, b.url));
            b
        }
        None => ensure_project(
            client,
            project_owner,
            owner,
            a.title.unwrap_or(a.initiative),
            rep,
        )?,
    };
    if board.public {
        rep.line("WARNING: the project is PUBLIC. init never changes visibility; set it private in the project settings.");
    }
    Ok(board)
}

fn link_repos(
    client: &Client,
    board: &Board,
    repos: &[String],
    repo_ids: &[String],
    rep: &mut Report,
) -> CbResult<()> {
    let linked_names = projects::linked_repo_names(client, &board.project_id)?;
    for (r, id) in repos.iter().zip(repo_ids) {
        if linked_names.contains(&r.to_lowercase()) {
            rep.line(format!("repo {r}: linked already"));
        } else {
            projects::link_repo(client, &board.project_id, id)?;
            rep.line(format!("repo {r}: linked"));
        }
    }
    Ok(())
}

fn ensure_labels(client: &Client, repos: &[String], rep: &mut Report) -> CbResult<()> {
    for r in repos {
        for (name, color, desc) in LABELS {
            match client.post(
                &format!("{}/labels", repo_path(r)?),
                &json!({"name": name, "color": color, "description": desc}),
            ) {
                Ok(_) => rep.line(format!("label {name} in {r}: created")),
                Err(e) if is_already_exists(&e) => {
                    rep.line(format!("label {name} in {r}: exists (reused)"));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

/// The open registry issue of this initiative, if there is one.
///
/// First the number from an earlier run, read directly: a label listing can lag behind a
/// just-created issue, and a lagging listing must not make a second registry issue. Else the
/// labelled open issues with exactly this title.
fn find_registry_issue(
    client: &Client,
    cd: &crate::config::ConfigDir,
    a: &InitArgs<'_>,
    main: &str,
    title: &str,
) -> CbResult<Option<Value>> {
    let remembered = std::fs::read_to_string(cd.board_config_path(a.initiative))
        .ok()
        .and_then(|t| BoardConfig::parse(&t).ok())
        .filter(|c| c.initiative == a.initiative && c.main_repo() == main)
        .map(|c| c.registry_issue);
    if let Some(n) = remembered
        && let Ok(r) = client.get(&format!("{}/issues/{n}", repo_path(main)?))
        && r.body.get("title").and_then(Value::as_str) == Some(title)
        && r.body.get("state").and_then(Value::as_str) == Some("open")
    {
        return Ok(Some(r.body));
    }
    let issues = client.get(&format!(
        "{}/issues?labels=crew:registry&state=open&per_page=100",
        repo_path(main)?
    ))?;
    Ok(issues
        .body
        .as_array()
        .ok_or_else(|| GhError::Shape("issue list is not an array".into()))?
        .iter()
        .find(|i| {
            i.get("pull_request").is_none() && i.get("title").and_then(Value::as_str) == Some(title)
        })
        .cloned())
}

/// Finds or creates the registry issue. Returns its number, its node id and whether it is locked.
fn ensure_registry_issue(
    client: &Client,
    cd: &crate::config::ConfigDir,
    a: &InitArgs<'_>,
    main: &str,
    rep: &mut Report,
) -> CbResult<(u64, String, bool)> {
    let title = format!("Crew registry - {}", a.initiative);
    let (number, node, locked) = match find_registry_issue(client, cd, a, main, &title)? {
        Some(i) => {
            rep.line(format!(
                "registry issue #{}: exists",
                i.get("number").and_then(Value::as_u64).unwrap_or(0)
            ));
            (
                i.get("number").and_then(Value::as_u64).unwrap_or(0),
                i.get("node_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                i.get("locked").and_then(Value::as_bool).unwrap_or(false),
            )
        }
        None => {
            let r = client.post(
                &format!("{}/issues", repo_path(main)?),
                &json!({"title": title,
                        "body": "Join requests, approvals, role changes and departures of this initiative are comments here (crewboard docs/design/spec/board.md#registry-issue). Agents post signed join requests; only the owner approves, with `/crew approve <name> [role=dev|coordinator]`.",
                        "labels": ["crew:registry"]}),
            )?;
            let n = r.body.get("number").and_then(Value::as_u64).unwrap_or(0);
            rep.line(format!("registry issue #{n}: created"));
            (
                n,
                r.body
                    .get("node_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                false,
            )
        }
    };
    if number == 0 || node.is_empty() {
        return Err(GhError::Shape("registry issue has no number or node id".into()).into());
    }
    Ok((number, node, locked))
}

/// Locks the conversation of the registry issue: only collaborators of the repo can comment
/// then. Any GitHub account could post a join otherwise, and the owner would see requests of
/// strangers (spec/security-details#trust-roots). The agents' accounts need the Write role
/// anyway. A failure is reported, never fatal: the owner can lock it by hand.
fn ensure_lock(client: &Client, main: &str, reg_number: u64, locked: bool, rep: &mut Report) {
    if locked {
        rep.line("registry lock: ok");
        return;
    }
    let path = match repo_path(main) {
        Ok(p) => format!("{p}/issues/{reg_number}/lock"),
        Err(e) => {
            rep.line(format!("registry lock: NOT locked ({e})"));
            return;
        }
    };
    match client.send("PUT", &path, Some(&json!({}))) {
        Ok(_) => rep.line("registry lock: locked (only collaborators can comment)"),
        Err(e) => rep.line(format!(
            "registry lock: NOT locked ({e}). Lock the conversation of #{reg_number} yourself (Lock conversation, on the issue page), so that only collaborators can post joins."
        )),
    }
}

/// Pins the registry issue. GitHub allows 3 pins: with 3 already, report and never unpin.
fn ensure_pin(
    client: &Client,
    main: &str,
    reg_number: u64,
    reg_node: &str,
    rep: &mut Report,
) -> CbResult<()> {
    let (o, n) = split_repo(main)?;
    let pinned = client.graphql(
        "query($o:String!,$n:String!){repository(owner:$o,name:$n){pinnedIssues(first:5){nodes{issue{number}}}}}",
        json!({"o": o, "n": n}),
    )?;
    let pins: Vec<u64> = pinned
        .pointer("/repository/pinnedIssues/nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| GhError::Shape("no pinnedIssues".into()))?
        .iter()
        .filter_map(|p| p.pointer("/issue/number").and_then(Value::as_u64))
        .collect();
    if pins.contains(&reg_number) {
        rep.line("registry pin: ok");
    } else if pins.len() >= 3 {
        rep.line(format!(
            "registry pin: NOT pinned. {main} already has 3 pinned issues ({}), the GitHub limit. Nothing was unpinned. Unpin one yourself, then pin #{reg_number}.",
            pins.iter().map(|p| format!("#{p}")).collect::<Vec<_>>().join(", ")
        ));
    } else {
        client.graphql(
            "mutation($i:ID!){pinIssue(input:{issueId:$i}){issue{number}}}",
            json!({"i": reg_node}),
        )?;
        rep.line("registry pin: pinned");
    }
    Ok(())
}

/// Writes the board config file of the initiative into the config directory.
fn write_board_config(
    cd: &crate::config::ConfigDir,
    cfg: &BoardConfig,
    rep: &mut Report,
) -> CbResult<()> {
    cfg.validate()?;
    std::fs::create_dir_all(&cd.0)
        .map_err(|e| CbError::config(format!("{}: {e}", cd.0.display())))?;
    std::fs::write(cd.board_config_path(&cfg.initiative), initiative_file(cfg))
        .map_err(|e| CbError::config(format!("write board config: {e}")))?;
    rep.line(format!(
        "board config written: {}",
        cd.board_config_path(&cfg.initiative).display()
    ));
    Ok(())
}

pub fn run(g: &Global, a: InitArgs<'_>) -> CbResult<String> {
    check_args(&a)?;
    let client = new_client()?;
    let mut rep = Report::default();
    let owner_login = check_owner(&client, &a, &mut rep)?;
    let (repo_ids, default_branch) = read_repos(&client, a.repos, &mut rep)?;
    let main = a.repos[0].clone();
    // `--with-ci` reads the repo and checks the token now, still before the first write
    let ci = if a.with_ci {
        let plan = super::init_ci::plan(&client, &main, &default_branch)?;
        for l in &plan.lines {
            rep.line(l);
        }
        plan.text
    } else {
        None
    };
    let project_owner = split_repo(&main)?.0.to_string();
    let owner = owner_info(&client, &project_owner)?;
    let crew = match a.crew_login {
        Some(c) => Some(check_crew_login(
            &client,
            c,
            &owner_login,
            a.repos,
            &mut rep,
        )?),
        None => None,
    };

    let mut board = adopt_or_create_project(&client, &a, &project_owner, &owner, &mut rep)?;
    link_repos(&client, &board, a.repos, &repo_ids, &mut rep)?;
    ensure_fields(&client, &mut board, &mut rep)?;
    if let Some((_, node)) = &crew {
        add_collaborator(&client, &board, node, a.crew_login.unwrap_or(""), &mut rep)?;
    }
    create_views(&client, &board, &mut rep)?;
    // Delete the four built-in project workflows that write Status behind crewboard
    // (spec/board#built-in-project-workflows). A failure stops init here, as every step does:
    // what came before stays, and a second run continues.
    let workflows = delete_status_writers(&client, &board, &mut rep)?;
    ensure_labels(&client, a.repos, &mut rep)?;
    let cd = g.dir()?;
    let (reg_number, reg_node, locked) = ensure_registry_issue(&client, &cd, &a, &main, &mut rep)?;
    ensure_pin(&client, &main, reg_number, &reg_node, &mut rep)?;
    ensure_lock(&client, &main, reg_number, locked, &mut rep);

    let cfg = BoardConfig {
        initiative: a.initiative.to_string(),
        project_owner,
        project_number: board.number,
        repos: a.repos.to_vec(),
        owner_login,
        registry_issue: reg_number,
        authority: "AUTHORITY.md".into(),
        crew_login: a.crew_login.map(str::to_string),
        insecure_one_account: a.insecure_one_account,
        heartbeat_interval: None,
        required_tools: Vec::new(),
        smoke_cmd: None,
        detect_tools: Vec::new(),
        stale: None,
        late: None,
        former_repos: Vec::new(),
    };
    write_board_config(&cd, &cfg, &mut rep)?;

    // CI must run for crew branches (spec/ci#ci-must-run-for-crew-branches): a warning, never
    // a failure
    let ci_warn = crate::services::ci_triggers::check_repos(
        &client,
        &cfg.repos,
        &crate::authority::Authority::default().branch_prefix(),
    );
    for l in ci_warn.lines() {
        rep.line(l);
    }

    // the one PR
    let pr = if a.no_pr {
        rep.line("PR: skipped (--no-pr)");
        None
    } else {
        open_pr(&client, &cfg, &default_branch, ci.as_deref(), &mut rep)?
    };
    if a.no_branch_rule {
        rep.line("branch rule: skipped (--no-branch-rule)");
    } else {
        rep.line(super::init_rule::ensure(&client, &main, &default_branch));
    }
    rep.line(format!("board: {}", board.url));
    let list = owner_checklist(&Checklist {
        board_url: &board.url,
        org: board.owner_kind == OwnerKind::Org,
        workflows,
        crew_login: a.crew_login,
        pr: pr.as_deref(),
        main_repo: &main,
    });
    for l in list.lines() {
        rep.line(l);
    }
    rep.line(next_step(
        a.initiative,
        &main,
        pr.is_some(),
        a.insecure_one_account,
    ));
    Ok(String::new())
}

/// The owner token must have the `project` scope: without it every board call fails half way
/// through. A classic or `gh` token shows its scopes (the header `x-oauth-scopes` of the first
/// answer); a fine-grained token shows none, and the first board call judges it. The error
/// names the exact `gh` line, and nothing was written.
fn check_owner_scope(client: &Client, owner: &str) -> CbResult<()> {
    let Some(scopes) = client.oauth_scopes() else {
        return Ok(());
    };
    if scopes.split(',').any(|s| s.trim() == "project") {
        return Ok(());
    }
    let has = if scopes.trim().is_empty() {
        "none"
    } else {
        scopes.as_str()
    };
    Err(CbError::config(format!(
        "the token of {owner} has no `project` scope (it has: {has}); nothing was written. Run: gh auth refresh -h github.com -s project, then run init again. For a token in GH_TOKEN or GITHUB_TOKEN: make a token with the scopes repo and project"
    )))
}

/// The step of `init` that deletes the built-in workflows that write
/// `Status`: one line `deleted project workflow: <name>` per workflow, and one summary line. A
/// workflow that is not there is already gone. A failure (a token without the permission, a
/// network error) is an error that names the manual fallback; the earlier lines are in `rep`.
/// Returns how many other writers of `Status` the board still has on (ones that the owner
/// added by hand): one warning line each.
pub fn delete_status_writers(client: &Client, board: &Board, rep: &mut Report) -> CbResult<usize> {
    let mut deleted = 0;
    let result = crate::builtin_workflows::delete_status_writers(client, board, |name| {
        deleted += 1;
        rep.line(format!("deleted project workflow: {name}"));
    });
    let remaining = match result {
        Ok(r) => r,
        Err(e) => {
            return Err(CbError::config(crate::builtin_workflows::manual_fallback(
                &board.url,
                &e.to_string(),
            )));
        }
    };
    rep.line(workflows_summary(deleted));
    let warn = crate::builtin_workflows::warnings(&remaining, &board.url);
    for l in warn.lines() {
        rep.line(l);
    }
    Ok(warn.lines().count())
}

/// The summary line of the delete step.
pub fn workflows_summary(deleted: usize) -> String {
    if deleted == 0 {
        "built-in project workflows: none of the four that write Status is there (already gone)"
            .into()
    } else {
        format!(
            "built-in project workflows: {deleted} deleted (the four that write Status; one way, the API cannot create them again)"
        )
    }
}

/// The facts of `init` for the owner checklist. No read: all of them are known already.
pub struct Checklist<'a> {
    pub board_url: &'a str,
    /// An organisation board (fine-grained token) or a user board (classic token).
    pub org: bool,
    /// Other workflows that write Status or add items and are on: the owner switches them off.
    pub workflows: usize,
    pub crew_login: Option<&'a str>,
    /// The URL of the init PR; `None`: no PR (not needed, or `--no-pr`).
    pub pr: Option<&'a str>,
    pub main_repo: &'a str,
}

/// The last lines of `init`: the steps that only the owner can do in the web UI, each with
/// its direct URL. Pure.
pub fn owner_checklist(c: &Checklist<'_>) -> String {
    let mut s = String::from("owner checklist (web UI, the API cannot do these):\n");
    let mut n = 0;
    let mut step = |s: &mut String, text: String| {
        n += 1;
        s.push_str(&format!("  {n}. {text}\n"));
    };
    if c.workflows > 0 {
        step(
            &mut s,
            format!(
                "Workflows: open {}/workflows and switch off the {} built-in workflow(s) of the WARNING lines above.",
                c.board_url, c.workflows
            ),
        );
    }
    if let Some(crew) = c.crew_login {
        step(
            &mut s,
            if c.org {
                format!(
                    "Crew token: as {crew}, open https://github.com/settings/personal-access-tokens/new (fine-grained; resource owner {}; only the initiative repos; permissions as in README.md, action 15).",
                    c.main_repo.split('/').next().unwrap_or("")
                )
            } else {
                format!(
                    "Crew token: as {crew}, open https://github.com/settings/tokens/new (classic; scopes repo and project, nothing else)."
                )
            },
        );
    }
    if let Some(pr) = c.pr {
        step(
            &mut s,
            format!(
                "Review AUTHORITY.md in {pr} and merge it. Agents can join only after the merge."
            ),
        );
    }
    if n == 0 { String::new() } else { s }
}

/// The last line of `init`: the next owner command. Pure.
/// In trial mode (`trial`: one account) there is no crew token to store: the agents use the
/// owner's token, so the join starts from the env file that holds it.
pub fn next_step(initiative: &str, repo: &str, pr: bool, trial: bool) -> String {
    let merge = if pr {
        "After the merge of the init PR, the agents "
    } else {
        "The agents "
    };
    let join = if trial {
        format!(
            "Trial mode, one account: there is no crew token, so skip `agent token`. On the agent host run `crewboard agent add {repo} --initiative {initiative} --env-file <the env file with your token> --wait-approval`"
        )
    } else {
        format!(
            "On each agent host run `crewboard agent token --initiative {initiative}` once, then `crewboard agent add {repo} --initiative {initiative} --wait-approval`"
        )
    };
    format!(
        "next step: {merge}join. {join}; it prints the approve command for you. As the owner run: crewboard approve <agent> [<agent>...] --initiative {initiative} (add --role coordinator for the coordinator)"
    )
}

/// Find the project of that title under the owner (organisation or user) or create it.
pub fn ensure_project(
    client: &Client,
    project_owner: &str,
    owner: &OwnerInfo,
    title: &str,
    rep: &mut Report,
) -> CbResult<Board> {
    let root = graphql_root(&owner.kind);
    let existing = projects::find_project_number(client, root, project_owner, title)?;
    match existing {
        Some(n) => {
            let b = Board::load(client, project_owner, n)?;
            rep.line(format!(
                "project {n} reused by title: {} ({})",
                b.title, b.url
            ));
            Ok(b)
        }
        None => {
            let n = projects::create_project(client, &owner.node_id, title)?;
            let b = Board::load(client, project_owner, n)?;
            rep.line(format!("project {n} created: {} ({})", b.title, b.url));
            Ok(b)
        }
    }
}

/// The crew account must exist, be a user and differ from the owner. Returns its
/// `(numeric id, node id)`. A missing write access to a repo is a warning, not a failure
/// (the owner may add it after `init`).
fn check_crew_login(
    client: &Client,
    crew: &str,
    owner_login: &str,
    repos: &[String],
    rep: &mut Report,
) -> CbResult<(u64, String)> {
    if crew.eq_ignore_ascii_case(owner_login) {
        return Err(CbError::refused(
            "crew-login-is-owner",
            format!("--crew-login {crew} is the owner: agents may not act as the owner"),
        ));
    }
    let r = client.get(&format!("users/{crew}")).map_err(|e| match e {
        GhError::Http { status: 404, .. } => {
            CbError::config(format!("--crew-login {crew}: no such GitHub user"))
        }
        other => other.into(),
    })?;
    if r.body.get("type").and_then(Value::as_str) != Some("User") {
        return Err(CbError::config(format!(
            "--crew-login {crew} is not a user account"
        )));
    }
    let id = r
        .body
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| GhError::Shape("crew user has no numeric id".into()))?;
    let node = r
        .body
        .get("node_id")
        .and_then(Value::as_str)
        .ok_or_else(|| GhError::Shape("crew user has no node_id".into()))?
        .to_string();
    rep.line(format!("crew account: {crew}"));
    for repo in repos {
        match client.get(&format!("{}/collaborators/{crew}/permission", repo_path(repo)?)) {
            Ok(p) => {
                let perm = p
                    .body
                    .get("permission")
                    .and_then(Value::as_str)
                    .unwrap_or("none");
                if matches!(perm, "write" | "admin" | "maintain") {
                    rep.line(format!("crew account on {repo}: {perm}"));
                } else {
                    rep.line(format!(
                        "WARNING: {crew} has permission '{perm}' on {repo}; agents need write access there."
                    ));
                }
            }
            Err(GhError::Http { status: 403 | 404, .. }) => rep.line(format!(
                "WARNING: could not read the permission of {crew} on {repo}; make sure it is a write collaborator."
            )),
            Err(e) => return Err(e.into()),
        }
    }
    Ok((id, node))
}

/// Give the crew account the Write role on the project (user-owned boards have no team
/// access, so this is the way in). `updateProjectV2Collaborators` sets the role; calling it
/// again is harmless.
pub fn add_collaborator(
    client: &Client,
    board: &Board,
    user_node_id: &str,
    login: &str,
    rep: &mut Report,
) -> CbResult<()> {
    projects::grant_writer(client, &board.project_id, user_node_id)?;
    rep.line(format!(
        "board access: {login} has role WRITER on project {}",
        board.number
    ));
    Ok(())
}

/// Create the single-select and text fields of spec/board#fields that are missing and merge the
/// options of the present ones. Idempotent.
pub fn ensure_fields(client: &Client, board: &mut Board, rep: &mut Report) -> CbResult<()> {
    let items_total = projects::items_total(client, &board.project_id)?;
    for (name, wanted, seed) in SELECT_FIELDS {
        match board.fields.get(*name) {
            None => {
                let opts: Vec<Value> = seed
                    .iter()
                    .map(|o| json!({"name": o, "color": default_color(o), "description": ""}))
                    .collect();
                projects::create_select_field(client, &board.project_id, name, &opts)?;
                rep.line(format!("field {name}: created ({} options)", seed.len()));
            }
            Some(f) => {
                if f.data_type != "SINGLE_SELECT" {
                    return Err(CbError::refused(
                        "field-type-mismatch",
                        format!(
                            "field {name} exists as {}, it must be a single select",
                            f.data_type
                        ),
                    ));
                }
                // Rename built-in names only on a project without items.
                let renames: &[(&str, &str)] = if *name == "Status" && items_total == 0 {
                    STATUS_RENAMES
                } else {
                    &[]
                };
                let cur = f.entries();
                let desired = plan_options(&cur, wanted, renames)?;
                if is_noop(&cur, &desired) {
                    rep.line(format!("field {name}: ok"));
                } else {
                    board.write_options_renaming(client, name, &desired, renames)?;
                    rep.line(format!(
                        "field {name}: options merged (every earlier option id kept and read back)"
                    ));
                }
            }
        }
    }
    for name in TEXT_FIELDS {
        match board.fields.get(*name) {
            None => {
                projects::create_text_field(client, &board.project_id, name)?;
                rep.line(format!("field {name}: created"));
            }
            Some(f) if f.data_type != "TEXT" => {
                return Err(CbError::refused(
                    "field-type-mismatch",
                    format!("field {name} exists as {}, it must be text", f.data_type),
                ));
            }
            Some(_) => rep.line(format!("field {name}: ok")),
        }
    }
    for name in NUMBER_FIELDS {
        match board.fields.get(*name) {
            None => {
                projects::create_number_field(client, &board.project_id, name)?;
                rep.line(format!("field {name}: created"));
            }
            Some(f) if f.data_type != "NUMBER" => {
                return Err(CbError::refused(
                    "field-type-mismatch",
                    format!(
                        "field {name} exists as {}, it must be a number",
                        f.data_type
                    ),
                ));
            }
            Some(_) => rep.line(format!("field {name}: ok")),
        }
    }
    board.reload(client)?;
    Ok(())
}

/// The four views of a board (spec/board#views); one that exists is left alone.
pub fn create_views(client: &Client, board: &Board, rep: &mut Report) -> CbResult<()> {
    ensure_views(client, board, rep, None)
}

/// The names of the views of the board.
pub fn view_names(client: &Client, board: &Board) -> CbResult<Vec<String>> {
    Ok(projects::view_names(client, &board.project_id)?)
}

/// Create the missing views; with `only`, just the view of that name.
pub fn ensure_views(
    client: &Client,
    board: &Board,
    rep: &mut Report,
    only: Option<&str>,
) -> CbResult<()> {
    let names = view_names(client, board)?;
    let base = rest_base(&board.owner_kind, &board.owner);
    let fields = projects::rest_fields(client, &base, board.number)?;
    let id_of = |n: &str| -> Option<u64> {
        fields
            .as_array()?
            .iter()
            .find(|f| f.get("name").and_then(Value::as_str) == Some(n))
            .and_then(|f| f.get("id").and_then(Value::as_u64))
    };
    let missing_field = |n: &str| GhError::Shape(format!("REST fields list has no '{n}'"));
    let status = id_of("Status").ok_or_else(|| missing_field("Status"))?;
    let agent = id_of("Agent").ok_or_else(|| missing_field("Agent"))?;
    // the columns of the ticket views: the title, the status, the agent and the queue order
    let ticket_cols: Vec<u64> = ["Title", "Status", "Agent", "Order"]
        .iter()
        .filter_map(|n| id_of(n))
        .collect();
    let crew_cols: Vec<u64> = [
        "Title",
        "Role",
        "State",
        "Heartbeat",
        "Host",
        "Runtime",
        "Capabilities",
        "Doing",
    ]
    .iter()
    .filter_map(|n| id_of(n))
    .collect();
    // the ticket views sort by `Order`, ascending: the queue position (spec/board#views)
    let by_order = match id_of("Order") {
        Some(order) => json!([[order, "asc"]]),
        None => json!([]),
    };
    let views = [
        (
            "Board",
            json!({"layout":"board","filter":"Kind:Work","vertical_group_by":[status],"visible_fields":ticket_cols,"sort_by":by_order}),
        ),
        (
            "By Agent",
            json!({"layout":"board","filter":"Kind:Work -Status:Done","vertical_group_by":[agent],"visible_fields":ticket_cols,"sort_by":by_order}),
        ),
        (
            "Crew",
            json!({"layout":"table","filter":"Kind:Agent","visible_fields":crew_cols}),
        ),
        (
            "Needs Owner",
            json!({"layout":"table","filter":"label:needs-owner -Status:Done","visible_fields":ticket_cols,"sort_by":by_order}),
        ),
    ];
    for (name, spec) in views {
        if only.is_some_and(|o| o != name) {
            continue;
        }
        if names.iter().any(|n| n == name) {
            rep.line(format!("view {name}: exists"));
            if name == "Crew" {
                rep.line("view Crew: an existing view keeps its columns (the REST API cannot change them). To show Capabilities, add the column once in the web UI, or delete the view and run init again.");
            } else if name == "Board" {
                rep.line("views: an existing view keeps its columns (the REST API cannot change them). The label of a ticket shows its agent, so a missing Agent or Order column is optional; to show them, or to sort by Order, delete the view and run init again.");
            }
            continue;
        }
        let mut body = spec.clone();
        body["name"] = json!(name);
        match projects::create_view(client, &base, board.number, &body) {
            Ok(_) => rep.line(format!("view {name}: created")),
            Err(GhError::Http { status: 422, .. }) if body.get("visible_fields").is_some() => {
                // Some projects refuse a column list or a sort; the view is still useful without.
                body.as_object_mut().map(|o| {
                    o.remove("visible_fields");
                    o.remove("sort_by")
                });
                projects::create_view(client, &base, board.number, &body)?;
                rep.line(format!(
                    "view {name}: created (without a column list and sort; the label of a ticket shows its agent, so the Agent and Order columns are optional)"
                ));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Open (or update) the init PR. Returns its URL; `None` when no PR is needed.
fn open_pr(
    client: &Client,
    cfg: &BoardConfig,
    default_branch: &str,
    ci: Option<&str>,
    rep: &mut Report,
) -> CbResult<Option<String>> {
    let repo = cfg.main_repo();
    let rp = repo_path(repo)?;
    let owner = &cfg.owner_login;
    let mut want = vec![
        (INITIATIVE_FILE, initiative_file(cfg), true),
        ("AUTHORITY.md", authority_md(&cfg.initiative, owner), false),
    ];
    // `--with-ci`: the workflow of a repo that has none (never overwritten)
    if let Some(text) = ci {
        want.push((super::init_ci::CI_PATH, text.to_string(), false));
    }

    // Which files differ from the default branch? AUTHORITY.md is never overwritten.
    let mut todo = Vec::new();
    for (path, text, overwrite) in &want {
        match file_on(client, repo, path, default_branch)? {
            Some((cur, _)) if cur == *text => {
                rep.line(format!("{path}: identical on {default_branch}"))
            }
            Some(_) if !overwrite => rep.line(format!(
                "{path}: exists on {default_branch}, left as the owner wrote it"
            )),
            _ => todo.push((*path, text.clone())),
        }
    }
    if todo.is_empty() {
        rep.line("PR: not needed");
        return Ok(None);
    }
    let branch = format!("crewboard/init-{}", cfg.initiative);
    let head = match client.get(&format!("{rp}/git/ref/heads/{default_branch}")) {
        Ok(h) => h,
        // GitHub answers 409 "Git Repository is empty" (or 404) when there is no commit.
        Err(GhError::Http {
            status: 409 | 404, ..
        }) => {
            return Err(CbError::refused(
                "repo-empty-no-pr",
                format!(
                    "{repo} has no commit, so no pull request can be opened. Add a first commit (for example a README), then run init again; it will only open the PR."
                ),
            ));
        }
        Err(e) => return Err(e.into()),
    };
    let base_sha = head
        .body
        .pointer("/object/sha")
        .and_then(Value::as_str)
        .ok_or_else(|| GhError::Shape("default branch ref has no sha".into()))?
        .to_string();
    match client.get(&format!("{rp}/git/ref/heads/{branch}")) {
        Ok(_) => {}
        Err(GhError::Http { status: 404, .. }) => {
            client.post(
                &format!("{rp}/git/refs"),
                &json!({"ref": format!("refs/heads/{branch}"), "sha": base_sha}),
            )?;
        }
        Err(e) => return Err(e.into()),
    }
    for (path, text) in &todo {
        let on_branch = file_on(client, repo, path, &branch)?;
        if on_branch.as_ref().is_some_and(|(t, _)| t == text) {
            continue;
        }
        let mut body = json!({
            "message": format!("crewboard init: {path} for initiative {}", cfg.initiative),
            "content": STANDARD.encode(text.as_bytes()),
            "branch": branch,
        });
        if let Some((_, sha)) = on_branch {
            body["sha"] = json!(sha);
        }
        match client.send("PUT", &format!("{rp}/contents/{path}"), Some(&body)) {
            Ok(_) => {}
            Err(GhError::Http {
                status: 403 | 404, ..
            }) if *path == super::init_ci::CI_PATH => {
                return Err(super::init_ci::workflow_scope_error("not shown"));
            }
            Err(e) => return Err(e.into()),
        }
    }
    let open = client.get(&format!(
        "{rp}/pulls?state=open&head={}:{branch}",
        split_repo(repo)?.0
    ))?;
    let url = match open
        .body
        .as_array()
        .and_then(|a| a.first())
        .and_then(|p| p.get("html_url").and_then(Value::as_str))
    {
        Some(url) => {
            rep.line(format!("PR: open already, branch updated: {url}"));
            url.to_string()
        }
        None => {
            let pr = client.post(
                &format!("{rp}/pulls"),
                &json!({
                    "title": format!("crewboard init: AUTHORITY.md and {INITIATIVE_FILE} for {}", cfg.initiative),
                    "head": branch,
                    "base": default_branch,
                    "body": if ci.is_some() {
                        "Adds the initiative file (no secrets), the authority file that crewboard agents read at start, and a minimal CI workflow (.github/workflows/ci.yml). Review AUTHORITY.md and check the test step of the workflow, then merge. crewboard does not merge it."
                    } else {
                        "Adds the initiative file (no secrets) and the authority file that crewboard agents read at start. Review AUTHORITY.md, then merge. crewboard does not merge it."
                    },
                }),
            )?;
            let url = pr
                .body
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            rep.line(format!("PR: opened {url} (not merged)"));
            url
        }
    };
    Ok(Some(url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::start::authority_owner;

    fn cfg() -> BoardConfig {
        BoardConfig::parse(
            "initiative=\"sandbox\"\nproject_owner=\"example-org\"\nproject_number=15\nrepos=[\"example-org/app\"]\nowner_login=\"owner-login\"\nregistry_issue=2\n",
        )
        .unwrap()
    }

    #[test]
    fn authority_md_has_machine_readable_front_matter_that_start_can_read() {
        let t = authority_md("sandbox", "owner-login");
        assert_eq!(authority_owner(&t).as_deref(), Some("owner-login"));
        assert!(t.contains("coordinator_may_merge: false"));
        assert!(t.contains("protected: [main]"));
        assert!(t.contains("# ENFORCED: crewboard reads these keys and refuses what they forbid"));
        assert!(t.contains(
            "# ADVICE: crewboard does not read these keys; the models read them as the owner's rules"
        ));
        // the enforced block comes first, the advice block holds needs_owner and nothing of the
        // enforced keys
        let (enforced, advice) = t.split_once("# ADVICE:").unwrap();
        for k in crate::authority::ENFORCED_KEYS {
            assert!(
                enforced.contains(&format!("{k}:")),
                "{k} is in the ENFORCED block"
            );
            assert!(
                !advice.contains(&format!("\n{k}:")),
                "{k} is not in the ADVICE block"
            );
        }
        for k in crate::authority::ADVICE_KEYS {
            assert!(advice.contains(&format!("{k}: [release,")));
            assert!(!enforced.contains(&format!("\n{k}:")));
        }
        // the file parses to the defaults of the code
        let a = crate::authority::parse(&t).unwrap().unwrap();
        assert!(a.unknown_keys.is_empty());
        assert_eq!(
            a.needs_owner,
            crate::authority::Authority::default().needs_owner
        );
    }

    #[test]
    fn the_initiative_file_parses_back_and_holds_no_secret_shaped_text() {
        let c = cfg();
        let f = initiative_file(&c);
        assert_eq!(BoardConfig::parse(&f).unwrap(), c);
        assert!(crate::secrets::scan(&f).is_empty());
        assert!(!f.to_lowercase().contains("token"));
    }

    #[test]
    fn the_field_tables_match_spec_5_1() {
        let sel: Vec<&str> = SELECT_FIELDS.iter().map(|f| f.0).collect();
        // GitHub refuses a single select without options: every field has a seed.
        assert!(SELECT_FIELDS.iter().all(|f| !f.2.is_empty()));
        assert_eq!(sel, ["Kind", "Status", "Agent", "Role", "State"]);
        assert!(!sel.contains(&"Type"), "Type is reserved by GitHub");
        assert_eq!(NUMBER_FIELDS, ["Order"]);
        assert_eq!(TEXT_FIELDS.len(), 9);
        assert!(TEXT_FIELDS.contains(&"Capabilities"));
        // every field the board layer writes exists in the tables
        for f in ["Key", "Host", "Runtime", "Cursor", "Heartbeat"] {
            assert!(TEXT_FIELDS.contains(&f), "{f}");
        }
        let status = SELECT_FIELDS.iter().find(|f| f.0 == "Status").unwrap().1;
        assert_eq!(status.len(), 6);
        let state = SELECT_FIELDS.iter().find(|f| f.0 == "State").unwrap().1;
        assert_eq!(state.len(), 6);
    }

    fn list(org: bool, workflows: usize, crew: Option<&str>, pr: Option<&str>) -> String {
        owner_checklist(&Checklist {
            board_url: "https://github.com/orgs/acme/projects/15",
            org,
            workflows,
            crew_login: crew,
            pr,
            main_repo: "acme/app",
        })
    }

    #[test]
    fn the_owner_checklist_has_a_direct_url_for_each_open_step() {
        let pr = "https://github.com/acme/app/pull/3";
        let t = list(true, 5, Some("acme-crew"), Some(pr));
        for want in [
            "https://github.com/orgs/acme/projects/15/workflows",
            "switch off the 5 built-in workflow(s)",
            "as acme-crew, open https://github.com/settings/personal-access-tokens/new",
            "resource owner acme",
            "Review AUTHORITY.md in https://github.com/acme/app/pull/3 and merge it",
        ] {
            assert!(t.contains(want), "{want}: {t}");
        }
        assert!(t.contains("  3. Review"), "numbered: {t}");
        // a user board gets the classic token page
        let u = list(false, 5, Some("acme-crew"), Some(pr));
        assert!(u.contains("https://github.com/settings/tokens/new"), "{u}");
        assert!(!u.contains("personal-access-tokens"), "{u}");
    }

    #[test]
    fn init_ends_with_the_next_owner_command() {
        let t = next_step("demo", "o/app", true, false);
        assert!(
            t.starts_with("next step: After the merge of the init PR"),
            "{t}"
        );
        assert!(
            t.contains("crewboard approve <agent> [<agent>...] --initiative demo"),
            "{t}"
        );
        assert!(t.contains("--role coordinator"), "{t}");
        assert!(
            t.contains("crewboard agent add o/app --initiative demo --wait-approval"),
            "{t}"
        );
        assert!(!t.contains('\n'), "one line: {t}");
        assert!(t.contains("crewboard agent token --initiative demo"), "{t}");
        // trial mode: no crew token to store, the owner's env file is used
        let tr = next_step("demo", "o/app", true, true);
        assert!(!tr.contains("crewboard agent token"), "{tr}");
        assert!(tr.contains("Trial mode, one account"), "{tr}");
        assert!(
            tr.contains("crewboard agent add o/app --initiative demo --env-file"),
            "{tr}"
        );
        assert!(tr.contains("crewboard approve <agent>"), "{tr}");
        assert!(!tr.contains('\n'), "one line: {tr}");
        // no PR: nothing to merge first
        assert!(next_step("demo", "o/app", false, false).starts_with("next step: The agents join"));
    }

    #[test]
    fn the_owner_checklist_leaves_out_what_is_done_or_unknown_to_init() {
        // no workflow on, no crew account (a trial), no PR (not needed or --no-pr)
        let t = list(true, 0, None, None);
        assert!(t.is_empty(), "no step, no header: {t}");
        let t = list(true, 0, None, Some("https://github.com/acme/app/pull/3"));
        assert!(!t.contains("/workflows"), "{t}");
        assert!(!t.contains("token"), "{t}");
        assert!(t.contains("  1. Review AUTHORITY.md"), "{t}");
        assert!(!t.contains("  2."), "only one step: {t}");
    }
}
