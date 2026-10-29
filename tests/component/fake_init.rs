//! `init` against a fake GitHub.
//!
//! These tests run in plain `cargo test`: no token, no sandbox, no real GitHub. They replace
//! two tests of `it_sandbox.rs` (see `CONTRIBUTING.md`): `init` is idempotent and
//! keeps every option id, and `init` reports the 3-pin limit and unpins nothing.

use crate::support::board::{FakeBoard, Out, REPO};

const REPO2: &str = "fake-owner/fake-repo-2";

fn init(fake: &FakeBoard, dir: &std::path::Path, initiative: &str, extra: &[&str]) -> Out {
    let mut v = vec![
        "init",
        "--initiative",
        initiative,
        "--repo",
        REPO,
        "--repo",
        REPO2,
        "--project",
        "15",
        "--insecure-one-account",
    ];
    v.extend_from_slice(extra);
    fake.run(dir, &v)
}

fn mutations(fake: &FakeBoard) -> Vec<String> {
    fake.requests()
        .into_iter()
        .filter(|r| r.path == "/graphql" && r.body.contains("mutation"))
        .map(|r| r.body)
        .collect()
}

/// The REST writes: not a GET, not GraphQL.
fn rest_writes(fake: &FakeBoard) -> Vec<String> {
    fake.requests()
        .into_iter()
        .filter(|r| r.method != "GET" && r.path != "/graphql")
        .map(|r| format!("{} {}", r.method, r.path))
        .collect()
}

#[test]
fn init_twice_creates_only_what_is_missing_and_never_touches_an_option_id() {
    let fake = FakeBoard::new();
    // a board from before two fields and one Status option existed, with the legacy options
    // `Needs Human` and `working`; one repo is linked
    fake.remove_field("Capabilities");
    let status_before = fake.field_option_ids("Status");
    let mut status = fake.field_options("Status");
    status.retain(|o| o != "Done");
    fake.set_field_options("Status", &status);
    let kept_status = fake.field_option_ids("Status");
    fake.seed_agent_options(&["claude@fake-a".to_string()]);
    let agent_ids = fake.agent_option_ids();
    fake.link_repo(REPO);
    let owner = tempfile::tempdir().unwrap();

    // first run: it creates and merges what is missing
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    for line in [
        "project 15 adopted",
        &format!("repo {REPO}: linked already"),
        &format!("repo {REPO2}: linked"),
        "field Capabilities: created",
        "field Status: options merged",
        "field Agent: ok",
        "view Board: created",
        "view Crew: created",
        "label crew:registry in",
        "registry pin: pinned",
        "registry lock: locked (only collaborators can comment)",
        "PR: opened",
    ] {
        assert!(o.stdout.contains(line), "{line}: {}", o.stdout);
    }
    let reg: u64 = o
        .stdout
        .split("registry issue #")
        .nth(1)
        .and_then(|r| r.split(':').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{}", o.stdout));
    assert!(
        o.stdout
            .contains(&format!("registry issue #{reg}: created"))
    );
    // the merge kept every earlier option id and added the missing option with a new id
    let ids = fake.field_option_ids("Status");
    assert_eq!(ids.len(), kept_status.len() + 1, "{ids:?}");
    assert!(kept_status.iter().all(|k| ids.contains(k)), "{ids:?}");
    // the wanted options are 6 and 6; a legacy option stays after them
    assert_eq!(
        fake.field_options("Status"),
        [
            "Backlog",
            "Ready",
            "In Progress",
            "Review",
            "Blocked",
            "Done",
            "Needs Human"
        ]
    );
    assert_eq!(
        fake.field_options("State"),
        [
            "requested",
            "approved",
            "active",
            "paused",
            "draining",
            "left",
            "working"
        ]
    );
    assert_ne!(ids, status_before, "a new option gets a new id");
    assert_eq!(fake.agent_option_ids(), agent_ids, "Agent is left alone");
    assert_eq!(fake.linked_repos(), [REPO, REPO2]);
    assert_eq!(fake.views(), ["Board", "By Agent", "Crew", "Needs Owner"]);
    assert_eq!(fake.pins(), [reg]);
    assert_eq!(
        fake.locked_issues(),
        [reg],
        "only the registry issue is locked"
    );
    assert_eq!(fake.open_titles("crew:registry").len(), 1);
    let prs = fake.open_prs();
    assert_eq!(prs.len(), 1, "{prs:?}");
    assert_eq!(prs[0].1, "crewboard/init-fake");
    let authority = fake
        .branch_file("crewboard/init-fake", "AUTHORITY.md")
        .expect("AUTHORITY.md on the branch");
    assert!(authority.contains("owner: fake-owner"), "{authority}");
    assert!(
        fake.branch_file("crewboard/init-fake", ".crewboard.toml")
            .is_some_and(|t| t.contains(&format!("registry_issue = {reg}")))
    );
    let cfg = std::fs::read_to_string(owner.path().join("fake.board.toml")).unwrap();
    assert!(cfg.contains(&format!("registry_issue = {reg}")), "{cfg}");

    // second run: everything is there; nothing changes
    let schema = fake.schema();
    let (m, w) = (mutations(&fake).len(), rest_writes(&fake).len());
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    for line in o.stdout.lines().filter(|l| l.starts_with("field ")) {
        assert!(line.ends_with(": ok"), "init changed a field: {line}");
    }
    for line in [
        &format!("repo {REPO2}: linked already"),
        &format!("registry issue #{reg}: exists"),
        "registry pin: ok",
        "registry lock: ok",
        "view Board: exists",
        "exists (reused)",
        "PR: open already",
    ] {
        assert!(o.stdout.contains(line), "{line}: {}", o.stdout);
    }
    assert_eq!(fake.schema(), schema, "no field, option or id changed");
    assert_eq!(mutations(&fake)[m..], [] as [String; 0], "no GraphQL write");
    let new_writes = rest_writes(&fake)[w..].to_vec();
    assert!(
        new_writes.iter().all(|x| x.ends_with("/labels")),
        "only the label creates, which answer 422: {new_writes:?}"
    );
    assert_eq!(fake.views().len(), 4);
    assert_eq!(fake.pins(), [reg]);
    assert_eq!(
        fake.open_titles("crew:registry").len(),
        1,
        "no second registry"
    );
    assert_eq!(fake.open_prs().len(), 1, "no second PR");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn init_reports_the_three_pin_limit_and_unpins_nothing() {
    let fake = FakeBoard::new();
    // two pinned issues: init pins the registry issue as the third
    let p1 = fake.ticket_issue("pin filler 1", "", &[]);
    let p2 = fake.ticket_issue("pin filler 2", "", &[]);
    fake.pin(p1);
    fake.pin(p2);
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "pin-one", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("registry pin: pinned"), "{}", o.stdout);
    assert!(o.stdout.contains("PR: skipped (--no-pr)"), "{}", o.stdout);
    let pins = fake.pins();
    assert_eq!(pins.len(), 3, "{pins:?}");
    assert_eq!(pins[..2], [p1, p2]);

    // three pinned issues now: a new initiative gets a registry issue that cannot be pinned
    let before = fake.pins();
    let m = mutations(&fake).len();
    let o = init(&fake, owner.path(), "pin-limit-test", &["--no-pr"]);
    assert_eq!(o.code, 0, "init with a full pin list: {}", o.all());
    assert!(
        o.stdout.contains("registry pin: NOT pinned"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("Nothing was unpinned"), "{}", o.stdout);
    assert_eq!(
        fake.pins(),
        before,
        "init must not change the pinned issues"
    );
    let pin_writes: Vec<String> = mutations(&fake)[m..]
        .iter()
        .filter(|b| b.contains("pinIssue") || b.contains("unpinIssue"))
        .cloned()
        .collect();
    assert!(pin_writes.is_empty(), "no pin and no unpin: {pin_writes:?}");
    assert_eq!(
        fake.open_titles("crew:registry").len(),
        2,
        "the registry issue is made, only not pinned"
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

/// Any write: a REST call that is not a GET, or a GraphQL mutation.
fn writes(fake: &FakeBoard) -> Vec<String> {
    let mut w = rest_writes(fake);
    w.extend(mutations(fake));
    w
}

#[test]
fn an_owner_token_without_the_project_scope_stops_init_before_any_write() {
    let fake = FakeBoard::new();
    fake.set_scopes("repo, read:org, gist");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("Run: gh auth refresh -h github.com -s project"),
        "{}",
        o.all()
    );
    assert!(
        o.stderr.contains("it has: repo, read:org, gist"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), [] as [String; 0], "nothing was written");
    assert!(!owner.path().join("fake.board.toml").exists());
    // with the scope the same run goes through, and says nothing about it
    fake.set_scopes("gist, project, repo");
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(!o.all().contains("gh auth refresh"), "{}", o.all());
    // a token that shows no scopes (fine-grained) is not judged here
    let fine = FakeBoard::new();
    let o = init(&fine, owner.path(), "fine", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn the_ticket_views_show_title_status_agent_and_order_sorted_by_order() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let bodies = fake.view_bodies();
    let by = |name: &str| bodies.iter().find(|b| b["name"] == name).unwrap().clone();
    // the ids of the fake's REST fields: Title is 1, the others follow the field order
    for view in ["Board", "By Agent", "Needs Owner"] {
        let cols = by(view)["visible_fields"].as_array().unwrap().len();
        assert_eq!(cols, 4, "{view}: {}", by(view));
    }
    assert!(by("Board")["vertical_group_by"].is_array());
    // the ticket views sort by Order, ascending (not by the deprecated Priority)
    let order_id = fake
        .rest_field_id("Order")
        .expect("the fake has an Order field");
    for view in ["Board", "By Agent", "Needs Owner"] {
        assert_eq!(
            by(view)["sort_by"],
            serde_json::json!([[order_id, "asc"]]),
            "{view}: {}",
            by(view)
        );
    }
    assert!(by("Crew").get("sort_by").is_none());
    // the manual step is gone from the checklist
    assert!(
        !o.stdout.contains("show Agent and Priority"),
        "{}",
        o.stdout
    );
    assert!(!o.stdout.contains("Views: open"), "{}", o.stdout);
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

const CI: &str = ".github/workflows/ci.yml";

#[test]
fn with_ci_adds_a_workflow_to_the_init_pr_of_a_repo_without_one() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, "Cargo.toml", "[package]\n");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--with-ci"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains(&format!("ci: {REPO} has no workflow")),
        "{}",
        o.stdout
    );
    let t = fake
        .branch_file("crewboard/init-fake", CI)
        .expect("ci.yml on the PR branch");
    assert!(
        t.contains("- run: |\n          cargo test --locked\n"),
        "{t}"
    );
    assert!(t.contains("branches: [main]"), "{t}");
    // one PR with the three files; the authority file and the config are there too
    assert_eq!(fake.open_prs().len(), 1);
    assert!(
        fake.branch_file("crewboard/init-fake", "AUTHORITY.md")
            .is_some()
    );
    // a second run changes nothing
    let w = rest_writes(&fake).len();
    let o = init(&fake, owner.path(), "fake", &["--with-ci"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(fake.open_prs().len(), 1, "no second PR");
    let again = rest_writes(&fake)[w..].to_vec();
    assert!(again.iter().all(|x| x.ends_with("/labels")), "{again:?}");
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn without_the_flag_no_workflow_is_added() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, "Cargo.toml", "[package]\n");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(fake.branch_file("crewboard/init-fake", CI).is_none());
}

#[test]
fn with_ci_leaves_a_repo_that_has_a_workflow_alone() {
    let fake = FakeBoard::new();
    fake.set_file(REPO, ".github/workflows/build.yml", "name: build\n");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--with-ci"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout
            .contains("has a workflow already; --with-ci adds none"),
        "{}",
        o.stdout
    );
    assert!(fake.branch_file("crewboard/init-fake", CI).is_none());
}

#[test]
fn with_ci_and_no_known_toolchain_adds_a_step_that_fails_until_the_owner_edits_it() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--with-ci"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(o.stdout.contains("WARNING: ci:"), "{}", o.stdout);
    let t = fake.branch_file("crewboard/init-fake", CI).expect("ci.yml");
    assert!(t.contains("exit 1"), "{t}");
}

#[test]
fn with_ci_needs_the_workflow_scope_and_checks_it_before_any_write() {
    let fake = FakeBoard::new();
    fake.set_scopes("project, repo");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--with-ci"]);
    assert_eq!(o.code, 4, "{}", o.all());
    assert!(
        o.stderr
            .contains("Run: gh auth refresh -h github.com -s workflow"),
        "{}",
        o.all()
    );
    assert_eq!(writes(&fake), [] as [String; 0], "nothing was written");
    // without the flag the same token is fine
    let o = init(&fake, owner.path(), "fake", &[]);
    assert_eq!(o.code, 0, "{}", o.all());
    // --with-ci and --no-pr do not go together
    let o = init(&fake, owner.path(), "fake", &["--with-ci", "--no-pr"]);
    assert_ne!(o.code, 0);
}

const DELETED: [&str; 4] = [
    "Item added to project",
    "Item closed",
    "Pull request linked to issue",
    "Pull request merged",
];

fn deleted_lines(o: &Out) -> Vec<String> {
    o.stdout
        .lines()
        .filter(|l| l.starts_with("deleted project workflow: "))
        .map(str::to_string)
        .collect()
}

#[test]
fn init_deletes_the_four_status_workflows_keeps_the_other_two_and_a_second_run_finds_none() {
    let fake = FakeBoard::new();
    fake.set_default_workflows();
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let lines = deleted_lines(&o);
    assert_eq!(lines.len(), 4, "{}", o.all());
    for n in DELETED {
        assert!(
            lines.contains(&format!("deleted project workflow: {n}")),
            "{n}: {}",
            o.all()
        );
    }
    assert!(
        o.stdout.contains("built-in project workflows: 4 deleted"),
        "{}",
        o.all()
    );
    // the two that stay are still on the project, and no warning is left for them
    assert_eq!(
        fake.workflow_names(),
        ["Auto-add sub-issues to project", "Auto-close issue"]
    );
    assert!(!o.stdout.contains("WARNING: built-in"), "{}", o.all());
    assert!(!o.stdout.contains("/projects/15/workflows"), "{}", o.all());
    assert_eq!(fake.workflows_deleted().len(), 4);

    // a second run: nothing to delete, and it says so
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(deleted_lines(&o).is_empty(), "{}", o.all());
    assert!(o.stdout.contains("already gone"), "{}", o.all());
    assert_eq!(fake.workflows_deleted().len(), 4);
}

#[test]
fn init_with_a_token_that_may_not_delete_fails_at_that_step_and_names_the_ui_steps() {
    let fake = FakeBoard::new();
    fake.set_default_workflows();
    fake.fail_workflow_delete("Resource not accessible by personal access token");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_ne!(o.code, 0, "{}", o.all());
    // the earlier steps stay visible, the later ones did not run
    assert!(o.stdout.contains("view Board:"), "{}", o.all());
    assert!(!o.stdout.contains("registry issue"), "{}", o.all());
    assert!(
        !o.stdout.contains("deleted project workflow"),
        "{}",
        o.all()
    );
    for want in [
        "could not delete the built-in project workflows",
        "Resource not accessible",
        "/workflows",
        "Item added to project",
        "run the command again",
    ] {
        assert!(o.stderr.contains(want), "{want}: {}", o.all());
    }
    assert_eq!(fake.workflow_names().len(), 6);
}

#[test]
fn init_matches_the_names_of_the_api_by_case_and_spacing() {
    let fake = FakeBoard::new();
    fake.set_workflows(&[("item CLOSED", true), ("Pull request merged ", false)]);
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(deleted_lines(&o).len(), 2, "{}", o.all());
    assert!(fake.workflow_names().is_empty());
}

/// GitHub's default `Status` options on a brand-new project.
fn github_default_status(fake: &FakeBoard) {
    let names: Vec<String> = ["Todo", "In Progress", "Done"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    fake.set_field_options("Status", &names);
}

#[test]
fn init_on_a_new_project_with_the_default_status_options_passes_in_one_run() {
    let fake = FakeBoard::new();
    github_default_status(&fake);
    let todo = fake.field_option_ids("Status")[0].clone();
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        o.stdout.contains("field Status: options merged"),
        "{}",
        o.stdout
    );
    // trial mode (one account): the next step skips `agent token`
    let next = o
        .stdout
        .lines()
        .find(|l| l.starts_with("next step:"))
        .unwrap_or_else(|| panic!("{}", o.stdout));
    assert!(next.contains("Trial mode, one account"), "{next}");
    assert!(!next.contains("crewboard agent token"), "{next}");
    // Todo became Backlog and kept its id
    let names = fake.field_options("Status");
    assert_eq!(names[0], "Backlog", "{names:?}");
    assert_eq!(fake.field_option_ids("Status")[0], todo);
    assert!(!names.iter().any(|n| n == "Todo"), "{names:?}");
    // a second run changes nothing
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
}

#[test]
fn init_still_refuses_a_rename_it_did_not_plan() {
    let fake = FakeBoard::new();
    github_default_status(&fake);
    // behind the request GitHub also renames 'Done'
    fake.rename_on_options_write("Done", "Finished");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 2, "{}", o.all());
    let all = o.all();
    assert!(all.contains("option-name-changed"), "{all}");
    assert!(all.contains("'Done' became 'Finished'"), "{all}");
    assert!(!all.contains("'Todo' became"), "{all}");
}

// ---- the branch rule (docs/guides/owner.md#branch-rule) --------------------------------------

fn ruleset_posts(fake: &FakeBoard) -> Vec<String> {
    rest_writes(fake)
        .into_iter()
        .filter(|w| w.ends_with("/rulesets"))
        .collect()
}

fn rule_line(o: &Out) -> &str {
    o.stdout
        .lines()
        .find(|l| l.starts_with("branch rule:"))
        .unwrap_or_else(|| panic!("no branch rule line: {}", o.all()))
}

#[test]
fn init_creates_one_ruleset_on_the_default_branch_of_the_main_repo() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert!(
        rule_line(&o).starts_with(
            "branch rule: created ruleset \"crewboard: default branch\" on the default branch of fake-owner/fake-repo: pull request required, no force push, no deletion, 0 approvals"
        ),
        "{}",
        o.all()
    );
    // one POST, to the main repo only (not to REPO2)
    assert_eq!(
        ruleset_posts(&fake),
        ["POST /repos/fake-owner/fake-repo/rulesets"]
    );
    let sets = fake.rulesets();
    assert_eq!(sets.len(), 1);
    let b = &sets[0];
    assert_eq!(b["target"], "branch");
    assert_eq!(b["enforcement"], "active");
    assert_eq!(b["conditions"]["ref_name"]["include"][0], "~DEFAULT_BRANCH");
    let types: Vec<&str> = b["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["deletion", "non_fast_forward", "pull_request"]);
    assert_eq!(
        b["rules"][2]["parameters"]["required_approving_review_count"],
        0
    );
    assert!(fake.unhandled().is_empty(), "{:?}", fake.unhandled());
}

#[test]
fn a_second_init_finds_the_ruleset_and_creates_no_second_one() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    assert_eq!(init(&fake, owner.path(), "fake", &["--no-pr"]).code, 0);
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        rule_line(&o),
        "branch rule: main of fake-owner/fake-repo is protected already, left as it is"
    );
    assert_eq!(fake.rulesets().len(), 1);
    assert_eq!(ruleset_posts(&fake).len(), 1);
}

#[test]
fn a_rule_that_exists_already_is_left_alone_and_said() {
    // a classic rule
    let fake = FakeBoard::new();
    fake.set_branch_answer(serde_json::json!({"name": "main", "protected": true}));
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(
        rule_line(&o),
        "branch rule: main of fake-owner/fake-repo is protected already, left as it is"
    );
    assert!(fake.rulesets().is_empty());
    // a ruleset that only the rules endpoint shows
    let fake = FakeBoard::new();
    fake.set_rules_answer(200, serde_json::json!([{"type": "pull_request"}]));
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(
        rule_line(&o),
        "branch rule: a ruleset applies to main of fake-owner/fake-repo already, left as it is"
    );
    assert!(fake.rulesets().is_empty());
    // a ruleset of the owner that is not active: it is named, and no second one is made
    let fake = FakeBoard::new();
    fake.add_ruleset("crewboard: default branch", "disabled");
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert!(
        rule_line(&o).contains("exists (enforcement: disabled), left as it is"),
        "{}",
        o.all()
    );
    assert_eq!(fake.rulesets().len(), 1);
    assert!(ruleset_posts(&fake).is_empty());
}

#[test]
fn a_token_without_admin_gets_one_line_and_init_still_succeeds() {
    let fake = FakeBoard::new();
    fake.set_ruleset_error(403, "Upgrade to GitHub Pro or make this repository public");
    let owner = tempfile::tempdir().unwrap();
    let o = init(&fake, owner.path(), "fake", &["--no-pr"]);
    assert_eq!(o.code, 0, "{}", o.all());
    let l = rule_line(&o);
    assert!(
        l.starts_with(
            "branch rule: not created for fake-owner/fake-repo (HTTP 403: Upgrade to GitHub Pro"
        ),
        "{l}"
    );
    assert!(l.contains("Fix: in the repo settings"), "{l}");
    assert_eq!(o.stdout.matches("branch rule:").count(), 1, "{}", o.all());
    assert!(fake.rulesets().is_empty());
    // the rest of init ran: the board line and the next step come after
    assert!(o.stdout.contains("board: "), "{}", o.all());
}

#[test]
fn no_branch_rule_skips_the_step_and_makes_no_request_for_it() {
    let fake = FakeBoard::new();
    let owner = tempfile::tempdir().unwrap();
    let o = init(
        &fake,
        owner.path(),
        "fake",
        &["--no-pr", "--no-branch-rule"],
    );
    assert_eq!(o.code, 0, "{}", o.all());
    assert_eq!(rule_line(&o), "branch rule: skipped (--no-branch-rule)");
    assert!(ruleset_posts(&fake).is_empty());
    assert!(
        !fake
            .requests()
            .iter()
            .any(|r| r.path.contains("/rulesets") || r.path.contains("/rules/branches")),
        "{:?}",
        fake.requests()
    );
}
