//! A fake GitHub for the headless coordinator: one ticket in Review with a signed
//! `done`, its PR, its CI, AUTHORITY.md and the merge call, on top of the board fake.
//!
//! The front server answers the endpoints of the ticket, the PR and the merge. Everything
//! else (board, comments, registry) goes to the `FakeBoard` behind it. The front server
//! records every request, so a test counts the write requests.

#![allow(dead_code)]

use super::board::{Agent, FakeBoard, Out, REPO, THREAD};
use super::{FakeGithub, Req, Resp};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use crewboard::config::ConfigDir;
use crewboard::protocol::names::Kind;
use crewboard::rules::Role;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub const TICKET: u64 = 5;
pub const PR: u64 = 7;
pub const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
pub const HEAD_REF: &str = "crew/hermes@fake-b/5-fix";

/// What the scenario looks like.
pub struct Scenario<'a> {
    /// The text of the ticket body (it holds the `Gate:` line).
    pub body: &'a str,
    /// `coordinator_may_merge` in AUTHORITY.md.
    pub may_merge: bool,
    /// The CI run of the head SHA is green (else it failed).
    pub ci_green: bool,
}

#[derive(Default)]
struct Pr {
    merged: bool,
    /// The live issue body (a test edits it after the assignment).
    body: String,
    /// The files of the PR: the path, and the old path of a rename. Empty: `src/it.rs`.
    files: Vec<(String, Option<String>)>,
    /// More front matter lines of AUTHORITY.md (each ends in a newline).
    authority_extra: String,
}

pub struct Hub {
    pub board: FakeBoard,
    front: FakeGithub,
    pr: Arc<Mutex<Pr>>,
    /// The interactive coordinator: its config dir has no `headless` key.
    pub coord: Agent,
    pub dev: Agent,
    /// A config dir of the same coordinator with `headless = true` and a copy of the key.
    pub headless_dir: tempfile::TempDir,
}

fn forward(client: &reqwest::blocking::Client, board_base: &str, r: &Req) -> Resp {
    let url = format!("{board_base}{}", r.path);
    let m = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap();
    let mut b = client.request(m, url);
    if let Some(t) = &r.bearer {
        b = b.bearer_auth(t);
    }
    if !r.body.is_empty() {
        b = b.body(r.body.clone());
    }
    let resp = b.send().expect("forward to the board fake");
    let status = resp.status().as_u16();
    Resp {
        status,
        body: resp.text().unwrap_or_default(),
        headers: Vec::new(),
    }
}

/// The `Gate:` lines of a ticket body, joined with ` | ` (what `assign` signs). The bodies of
/// these tests use the plain `Gate: text` form.
fn gate_lines(body: &str) -> String {
    body.lines()
        .filter_map(|l| l.trim_start_matches("- ").strip_prefix("Gate:"))
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

/// The assignment message of `assign`.
fn assignment(gate: &str) -> String {
    format!("Assigned to you: Fix it\nticket: {REPO}#{TICKET}\nitem=PVTI_ticket\ngate: {gate}\n")
}

fn authority_text(may_merge: bool, extra: &str) -> String {
    format!(
        "---\ninitiative: fake\nowner: fake-owner\ncoordinator_may_merge: {may_merge}\nneeds_owner: []\n{extra}---\n\n# Authority\n"
    )
}

fn route(s: &Scenario<'_>, pr: &Mutex<Pr>, base: &str, r: &Req) -> Option<Resp> {
    let path = r.path.split('?').next().unwrap_or("");
    let seg: Vec<&str> = path.split('/').collect();
    let ok = |v: Value| Some(Resp::json(200, &v.to_string()));
    match (r.method.as_str(), seg.as_slice()) {
        ("GET", ["", "repos", o, n, "issues", num]) if *num == TICKET.to_string() => ok(json!({
            "node_id": "I_ticket", "number": TICKET, "title": "Fix it", "body": pr.lock().unwrap().body,
            "state": "open", "labels": [{"name": "crew:work"}],
            "html_url": format!("{base}/{o}/{n}/issues/{TICKET}"),
        })),
        ("GET", ["", "repos", _, _, "contents", "AUTHORITY.md"]) => ok(json!({
            "content": STANDARD.encode(authority_text(s.may_merge, &pr.lock().unwrap().authority_extra)),
            "encoding": "base64",
        })),
        ("GET", ["", "repos", _, _, "pulls", num]) if *num == PR.to_string() => {
            let merged = pr.lock().unwrap().merged;
            ok(json!({
                "number": PR, "state": if merged { "closed" } else { "open" }, "merged": merged,
                "mergeable_state": "clean", "body": format!("Closes #{TICKET}"),
                "head": {"sha": SHA, "ref": HEAD_REF, "repo": {"full_name": REPO}},
                "base": {"ref": "main"},
            }))
        }
        ("GET", ["", "repos", _, _, "pulls", num, "files"]) if *num == PR.to_string() => {
            // 100 per page, like GitHub
            let q = r.path.split_once('?').map_or("", |x| x.1);
            let arg = |name: &str, default: usize| -> usize {
                q.split('&')
                    .find_map(|kv| kv.strip_prefix(&format!("{name}=")))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(default)
            };
            let (per_page, page) = (arg("per_page", 30).max(1), arg("page", 1).max(1));
            let files = pr.lock().unwrap().files.clone();
            let files = if files.is_empty() {
                vec![("src/it.rs".to_string(), None)]
            } else {
                files
            };
            ok(Value::Array(
                files
                    .iter()
                    .skip((page - 1) * per_page)
                    .take(per_page)
                    .map(|(f, old)| match old {
                        Some(o) => {
                            json!({"filename": f, "status": "renamed", "previous_filename": o})
                        }
                        None => json!({"filename": f, "status": "added"}),
                    })
                    .collect(),
            ))
        }
        ("GET", ["", "repos", _, _, "actions", "runs"]) => ok(json!({
            "total_count": 1,
            "workflow_runs": [{
                "id": 9002, "name": "CI", "head_sha": SHA, "status": "completed",
                "conclusion": if s.ci_green { "success" } else { "failure" },
                "run_attempt": 1, "event": "pull_request",
                "html_url": format!("{base}/{REPO}/actions/runs/9002"),
            }],
        })),
        ("GET", ["", "repos", _, _, "commits", _, "check-runs"]) => {
            ok(json!({"total_count": 0, "check_runs": []}))
        }
        ("GET", ["", "repos", _, _, "commits", sha, "status"]) => {
            ok(json!({"state": "pending", "statuses": [], "sha": sha, "total_count": 0}))
        }
        ("GET", ["", "repos", _, _, "commits", _, "statuses"]) => ok(json!([])),
        ("GET", ["", "repos", _, _, "branches", _, "protection", ..]) => {
            Some(Resp::json(404, r#"{"message":"Branch not protected"}"#))
        }
        ("PUT", ["", "repos", _, _, "pulls", num, "merge"]) if *num == PR.to_string() => {
            pr.lock().unwrap().merged = true;
            ok(json!({"sha": "fedcba9876543210fedcba9876543210fedcba98", "merged": true}))
        }
        ("DELETE", ["", "repos", _, _, "git", "refs", ..]) => Some(Resp::json(200, "{}")),
        _ => None,
    }
}

impl Hub {
    pub fn new(s: Scenario<'static>) -> Hub {
        let board = FakeBoard::new();
        let coord = board.agent("claude@fake-a", Role::Coordinator);
        let dev = board.agent("hermes@fake-b", Role::Dev);
        // the ticket: on the board, in Review, assigned to the dev
        board.item(
            "PVTI_ticket",
            &[
                ("Status", "Review"),
                ("Agent", "hermes@fake-b"),
                ("Gate", "from the body"),
            ],
        );
        board.issue(TICKET, &["crew:work"]);
        // the signed info comment that names the item, and the signed done of the dev
        board.signed(
            &coord.key,
            &coord.name,
            "owner",
            Kind::Info,
            TICKET,
            "Ticket is on the board.\nitem=PVTI_ticket\nproject=15\n",
        );
        // the signed assignment: the `gate:` line that `assign` writes from the body
        board.signed(
            &coord.key,
            &coord.name,
            &dev.name,
            Kind::Request,
            TICKET,
            &assignment(&gate_lines(s.body)),
        );
        board.signed(
            &dev.key,
            &dev.name,
            &coord.name,
            Kind::Done,
            TICKET,
            &format!(
                "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\n"
            ),
        );
        let headless_dir = tempfile::tempdir().unwrap();
        copy_headless(&coord, headless_dir.path());
        let pr = Arc::new(Mutex::new(Pr {
            body: s.body.to_string(),
            ..Pr::default()
        }));
        let board_base = board.base();
        let pr2 = pr.clone();
        let client = reqwest::blocking::Client::new();
        let front = FakeGithub::start_with(move |base| {
            let base = base.to_string();
            move |r: &Req| {
                route(&s, &pr2, &base, r).unwrap_or_else(|| forward(&client, &board_base, r))
            }
        });
        let _ = THREAD;
        Hub {
            board,
            front,
            pr,
            coord,
            dev,
            headless_dir,
        }
    }

    /// Edit the live issue body (nobody signs the edit).
    pub fn set_body(&self, body: &str) {
        self.pr.lock().unwrap().body = body.to_string();
    }

    /// More front matter lines for AUTHORITY.md, for example `protected_paths: [tests/**]\n`.
    pub fn set_authority_extra(&self, lines: &str) {
        self.pr.lock().unwrap().authority_extra = lines.to_string();
    }

    /// The files that the PR changes now.
    pub fn set_files(&self, files: &[&str]) {
        self.pr.lock().unwrap().files = files.iter().map(|f| (f.to_string(), None)).collect();
    }

    /// The same, with one file renamed from `old` to `new`.
    pub fn set_renamed_file(&self, new: &str, old: &str) {
        self.pr.lock().unwrap().files = vec![(new.to_string(), Some(old.to_string()))];
    }

    /// Post another signed assignment message of the coordinator (a `reassign`, or an assignment
    /// with a gate that the body does not have). `lines` follow the `item=` line.
    pub fn sign_assignment(&self, lines: &str) {
        self.board.signed(
            &self.coord.key,
            &self.coord.name,
            &self.dev.name,
            Kind::Request,
            TICKET,
            &format!("Assigned to you: Fix it\nticket: {REPO}#{TICKET}\nitem=PVTI_ticket\n{lines}"),
        );
    }

    /// A newer signed `done` of the dev with the same evidence and `extra` lines after it (for
    /// example `protected-paths: ...`).
    pub fn sign_done(&self, extra: &str) {
        self.board.signed(
            &self.dev.key,
            &self.dev.name,
            &self.coord.name,
            Kind::Done,
            TICKET,
            &format!(
                "Done. Evidence follows.\npr: https://github.com/{REPO}/pull/{PR}\nsha: {SHA}\nci: https://github.com/{REPO}/actions/runs/9002\n{extra}"
            ),
        );
    }

    pub fn requests(&self) -> Vec<Req> {
        self.front.requests()
    }

    /// Requests that change something: every non-GET REST call and every GraphQL mutation.
    pub fn writes(&self) -> Vec<Req> {
        self.requests()
            .into_iter()
            .filter(|r| {
                r.method != "GET"
                    && !(r.path.starts_with("/graphql") && !r.body.contains("mutation"))
            })
            .collect()
    }

    pub fn merged(&self) -> bool {
        self.pr.lock().unwrap().merged
    }

    /// Run the CLI against the front server with a config dir.
    pub fn run(&self, dir: &Path, args: &[&str]) -> Out {
        self.run_env(dir, args, &[])
    }

    /// [`Hub::run`] with extra environment variables (for example the scope dir of the test
    /// hook).
    pub fn run_env(&self, dir: &Path, args: &[&str], env: &[(&str, &std::ffi::OsStr)]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(dir)
            .args(args)
            .envs(env.iter().copied())
            .env("CREWBOARD_API_BASE", self.front.base())
            .env("GH_TOKEN", "ghp_faketoken")
            .env_remove("GITHUB_TOKEN")
            .output()
            .expect("run crewboard");
        Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        }
    }

    pub fn headless(&self, args: &[&str]) -> Out {
        self.run(self.headless_dir.path(), args)
    }

    pub fn interactive(&self, args: &[&str]) -> Out {
        self.run(&self.coord.dir, args)
    }

    pub fn status_of_ticket(&self) -> Option<String> {
        self.board.item_field("PVTI_ticket", "Status")
    }
}

/// Copy the agent config and the key of `a` into `to`; put `headless = true` on the first line.
pub fn copy_headless(a: &Agent, to: &Path) {
    let from = ConfigDir(a.dir.clone());
    let into = ConfigDir(to.to_path_buf());
    let src = from.agent_config_path("fake");
    let text = std::fs::read_to_string(&src).unwrap();
    std::fs::write(
        into.agent_config_path("fake"),
        format!("headless = true\n{text}"),
    )
    .unwrap();
    let name = crewboard::protocol::names::AgentName::parse(&a.name).unwrap();
    let key_to: PathBuf = into.key_path(&name);
    std::fs::create_dir_all(key_to.parent().unwrap()).unwrap();
    std::fs::copy(from.key_path(&name), &key_to).unwrap();
}
