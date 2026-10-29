//! A fake GitHub for late work (spec/events#stale-work): one ticket, the branch list,
//! the open PR list and the CI of a head SHA, on top of the board fake.
//!
//! The front server answers the endpoints that the late waits read. Everything else goes to the
//! `FakeBoard` behind it. The front server records every request, so a test counts the late
//! reads. A test changes the scenario (branch pushed, head SHA, 500 answers) between polls.

#![allow(dead_code)]

use super::board::{Agent, FakeBoard, Out, REPO};
use super::{FakeGithub, Req, Resp};
use crewboard::clock::{format_ts, now_epoch};
use crewboard::config::{ConfigDir, LateConfig, StaleConfig};
use crewboard::protocol::names::{AgentName, Kind};
use crewboard::rules::Role;
use crewboard::state::StateCache;
use serde_json::{Value, json};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub const TICKET: u64 = 5;
pub const HEAD_REF: &str = "crew/hermes@fake-b/5-fix";
pub const SHA1: &str = "1111111111111111111111111111111111111111";
pub const SHA2: &str = "2222222222222222222222222222222222222222";

pub struct Scene {
    /// A branch `crew/<dev>/5-...` exists.
    pub branch: bool,
    /// The head SHA of the open PR; `None`: no open PR.
    pub pr_head: Option<String>,
    /// Unix seconds: when the CI of every head turned green.
    pub green_at: i64,
    /// Answer 500 on every read of the markers.
    pub fail: bool,
    /// The workflow runs of every head: `(status, conclusion)`. Empty: no CI at all.
    pub runs: Vec<(String, Option<String>)>,
}

impl Scene {
    /// Every run of the head has this state: `queued`, `in_progress` or `completed` with a
    /// conclusion.
    pub fn runs(&mut self, list: &[(&str, Option<&str>)]) {
        self.runs = list
            .iter()
            .map(|(s, c)| ((*s).to_string(), c.map(str::to_string)))
            .collect();
    }
}

pub struct LateBoard {
    pub board: FakeBoard,
    front: FakeGithub,
    scene: Arc<Mutex<Scene>>,
    pub coord: Agent,
    pub dev: Agent,
}

fn route(s: &Scene, base: &str, r: &Req) -> Option<Resp> {
    let (path, query) = r.path.split_once('?').unwrap_or((&r.path, ""));
    let seg: Vec<&str> = path.split('/').collect();
    let ok = |v: Value| Some(Resp::json(200, &v.to_string()));
    let stale_read = matches!(
        seg.as_slice(),
        ["", "repos", _, _, "git", "matching-refs", ..]
            | ["", "repos", _, _, "pulls"]
            | ["", "repos", _, _, "actions", "runs"]
            | ["", "repos", _, _, "commits", ..]
            | ["", "repos", _, _, "branches", ..]
    );
    if s.fail && stale_read && r.method == "GET" {
        return Some(Resp::json(500, r#"{"message":"boom"}"#));
    }
    match (r.method.as_str(), seg.as_slice()) {
        (
            "GET",
            [
                "",
                "repos",
                _,
                _,
                "git",
                "matching-refs",
                "heads",
                rest @ ..,
            ],
        ) => {
            // the request must name the prefix, the agent and the ticket
            let want = format!("crew/hermes@fake-b/{TICKET}-");
            if s.branch && rest.join("/") == want {
                ok(json!([{"ref": format!("refs/heads/{HEAD_REF}")}]))
            } else {
                ok(json!([]))
            }
        }
        ("GET", ["", "repos", _, _, "pulls"]) => {
            let list: Vec<Value> = s
                .pr_head
                .iter()
                .map(|sha| {
                    json!({"number": 7, "head": {"ref": HEAD_REF, "sha": sha},
                           "base": {"ref": "main"}})
                })
                .collect();
            ok(json!(list))
        }
        ("GET", ["", "repos", _, _, "contents", "AUTHORITY.md"]) => {
            Some(Resp::json(404, r#"{"message":"Not Found"}"#))
        }
        ("GET", ["", "repos", _, _, "actions", "runs"]) => {
            let sha = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("head_sha="))
                .unwrap_or("");
            let runs: Vec<Value> = s
                .runs
                .iter()
                .enumerate()
                .map(|(i, (status, conclusion))| {
                    json!({
                        "id": 9002 + i, "name": format!("CI {i}"), "head_sha": sha,
                        "status": status, "conclusion": conclusion, "run_attempt": 1,
                        "event": "pull_request", "updated_at": format_ts(s.green_at),
                        "html_url": format!("{base}/{REPO}/actions/runs/{}", 9002 + i),
                    })
                })
                .collect();
            ok(json!({"total_count": runs.len(), "workflow_runs": runs}))
        }
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
        _ => None,
    }
}

impl LateBoard {
    /// The signed assignment of the ticket to the dev, in the log (`take`, `done` and the other
    /// agent commands trust it, not the board `Agent`). Not in `new`: a poll shows it as an event.
    pub fn sign_assignment(&self) {
        self.board.signed(
            &self.coord.key,
            &self.coord.name,
            &self.dev.name,
            Kind::Request,
            TICKET,
            &crewboard::protocol::acts::assignment_body(
                "late ticket",
                &format!("{REPO}#{TICKET}"),
                "PVTI_ticket",
                None,
                None,
                "",
            ),
        );
    }

    /// A ticket with `status` assigned to the dev; its board item is 120 s old.
    pub fn new(status: &str) -> LateBoard {
        let board = FakeBoard::new();
        let coord = board.agent("claude@fake-a", Role::Coordinator);
        let dev = board.agent("hermes@fake-b", Role::Dev);
        board.item(
            "PVTI_ticket",
            &[
                ("Status", status),
                ("Agent", "hermes@fake-b"),
                ("Gate", "from the body"),
            ],
        );
        board.issue(TICKET, &["crew:work"]);
        board.signed(
            &coord.key,
            &coord.name,
            "owner",
            Kind::Info,
            TICKET,
            "Ticket is on the board.\nitem=PVTI_ticket\nproject=15\n",
        );
        let scene = Arc::new(Mutex::new(Scene {
            branch: false,
            pr_head: None,
            green_at: now_epoch() - 3600,
            fail: false,
            runs: vec![("completed".into(), Some("success".into()))],
        }));
        let (board_base, sc) = (board.base(), scene.clone());
        let client = reqwest::blocking::Client::new();
        let front = FakeGithub::start_with(move |base| {
            let base = base.to_string();
            move |r: &Req| {
                route(&sc.lock().unwrap(), &base, r).unwrap_or_else(|| {
                    let m = reqwest::Method::from_bytes(r.method.as_bytes()).unwrap();
                    let mut b = client.request(m, format!("{board_base}{}", r.path));
                    if let Some(t) = &r.bearer {
                        b = b.bearer_auth(t);
                    }
                    if !r.body.is_empty() {
                        b = b.body(r.body.clone());
                    }
                    let resp = b.send().expect("forward to the board fake");
                    Resp {
                        status: resp.status().as_u16(),
                        body: resp.text().unwrap_or_default(),
                        headers: Vec::new(),
                    }
                })
            }
        });
        LateBoard {
            board,
            front,
            scene,
            coord,
            dev,
        }
    }

    pub fn scene(&self, f: impl FnOnce(&mut Scene)) {
        f(&mut self.scene.lock().unwrap());
    }

    /// Put a `[stale]` table with these limits into the config of `who`.
    pub fn limits(&self, who: &Agent, no_push: &str, green: &str, review: &str) {
        let cd = ConfigDir(who.dir.clone());
        let mut a = cd.load_agent("fake").unwrap();
        a.board.stale = Some(StaleConfig {
            no_push: Some(no_push.into()),
            green: Some(green.into()),
            review: Some(review.into()),
            ..StaleConfig::default()
        });
        cd.save_agent(&a).unwrap();
    }

    /// Put a `[late]` table into the config of `who`, from `(key, value)` pairs such as
    /// `("push", "60s")`. The `[stale]` table goes.
    pub fn late_table(&self, who: &Agent, pairs: &[(&str, &str)]) {
        let cd = ConfigDir(who.dir.clone());
        let mut a = cd.load_agent("fake").unwrap();
        let mut l = LateConfig::default();
        for (k, v) in pairs {
            let v = Some((*v).to_string());
            match *k {
                "push" => l.push = v,
                "done" => l.done = v,
                "verify" => l.verify = v,
                "escalate" => l.escalate = v,
                "repeat" => l.repeat = v,
                "take" => l.take = v,
                "answer" => l.answer = v,
                "owner" => l.owner = v,
                other => panic!("no [late] key {other}"),
            }
        }
        a.board.stale = None;
        a.board.late = Some(l);
        cd.save_agent(&a).unwrap();
    }

    /// Set `escalate` and `repeat` in the `[stale]` table of `who`.
    pub fn escalation(&self, who: &Agent, escalate: &str, repeat: &str) {
        let cd = ConfigDir(who.dir.clone());
        let mut a = cd.load_agent("fake").unwrap();
        let st = a.board.stale.get_or_insert_with(StaleConfig::default);
        st.escalate = Some(escalate.into());
        st.repeat = Some(repeat.into());
        cd.save_agent(&a).unwrap();
    }

    /// Add In Progress tickets `numbers` for the dev. A ticket with `young` was changed just now.
    pub fn more_tickets(&self, numbers: &[u64], young: bool) {
        for n in numbers {
            let id = format!("PVTI_t{n}");
            self.board.item(
                &id,
                &[
                    ("Status", "In Progress"),
                    ("Agent", "hermes@fake-b"),
                    ("Gate", "from the body"),
                ],
            );
            if young {
                self.board.touch_item(&id);
            }
            self.board.issue(*n, &["crew:work"]);
            self.board.signed(
                &self.coord.key,
                &self.coord.name,
                "owner",
                Kind::Info,
                *n,
                &format!("Ticket is on the board.\nitem={id}\nproject=15\n"),
            );
        }
    }

    /// The seen entries of `who` whose key ends with `suffix`.
    pub fn seen_entries(&self, who: &Agent, suffix: &str) -> Vec<(String, String)> {
        let cd = ConfigDir(who.dir.clone());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        StateCache::load(&p)
            .late
            .late_seen
            .into_iter()
            .filter(|(k, _)| k.ends_with(suffix))
            .collect()
    }

    /// The state file of `who`.
    pub fn state_file(&self, who: &Agent) -> std::path::PathBuf {
        let cd = ConfigDir(who.dir.clone());
        cd.state_path("fake", &AgentName::parse(&who.name).unwrap())
    }

    /// Make the escalations that `who` gave `secs` older (as if `secs` had passed).
    pub fn age_escalations(&self, who: &Agent, secs: i64) {
        let cd = ConfigDir(who.dir.clone());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        let mut c = StateCache::load(&p);
        let mut changed = 0;
        for (k, v) in c.late.late_seen.iter_mut() {
            if k.ends_with("|escalated") {
                let (val, t) = v.rsplit_once('|').expect("value|time");
                *v = format!("{val}|{}", t.parse::<i64>().unwrap() - secs);
                changed += 1;
            }
        }
        assert!(changed > 0, "no escalation entry to age");
        c.save(&p).unwrap();
    }

    /// A headless copy of the config of `who` (`headless = true` and the key).
    pub fn headless_copy(&self, who: &Agent) -> tempfile::TempDir {
        let to = tempfile::tempdir().unwrap();
        let from = ConfigDir(who.dir.clone());
        let into = ConfigDir(to.path().to_path_buf());
        let text = std::fs::read_to_string(from.agent_config_path("fake")).unwrap();
        std::fs::write(
            into.agent_config_path("fake"),
            format!("headless = true\n{text}"),
        )
        .unwrap();
        let name = AgentName::parse(&who.name).unwrap();
        let key_to = into.key_path(&name);
        std::fs::create_dir_all(key_to.parent().unwrap()).unwrap();
        std::fs::copy(from.key_path(&name), &key_to).unwrap();
        to
    }

    /// Forget when the headless `tick` of `dir` read GitHub last.
    pub fn lift_tick_throttle(&self, dir: &std::path::Path, who: &Agent) {
        let cd = ConfigDir(dir.to_path_buf());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        let mut c = StateCache::load(&p);
        c.late.late_tick_checked = None;
        c.save(&p).unwrap();
    }

    /// Run a command with another config dir against the front server.
    pub fn run_dir(&self, dir: &std::path::Path, args: &[&str]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(dir)
            .args(args)
            .env("CREWBOARD_API_BASE", self.front.base())
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

    /// Forget when `who` read GitHub for the markers last (the 5 minute throttle of `poll`).
    /// Lift only the throttle of `ci-finished` (60 s); the one of the waits (5 min) stays.
    pub fn lift_ci_throttle(&self, who: &Agent) {
        let cd = ConfigDir(who.dir.clone());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        let mut c = StateCache::load(&p);
        c.late.late_ci_checked = None;
        c.save(&p).unwrap();
    }

    pub fn lift_throttle(&self, who: &Agent) {
        let cd = ConfigDir(who.dir.clone());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        let mut c = StateCache::load(&p);
        c.late.late_checked = None;
        c.late.late_ci_checked = None;
        c.save(&p).unwrap();
    }

    pub fn requests(&self) -> Vec<Req> {
        self.front.requests()
    }

    /// Requests whose path contains `part`.
    pub fn count(&self, part: &str) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.path.contains(part))
            .count()
    }

    /// The reads that only the markers make: branch list, PR list, CI runs, AUTHORITY.md.
    pub fn stale_reads(&self) -> usize {
        [
            "/git/matching-refs/",
            "/pulls?",
            "/actions/runs?",
            "/contents/AUTHORITY.md",
        ]
        .iter()
        .map(|p| self.count(p))
        .sum()
    }

    /// The reads of the CI of a head: workflow runs, check runs, statuses, branch protection.
    pub fn ci_reads(&self) -> usize {
        ["/actions/runs?", "/check-runs", "/status", "/branches/"]
            .iter()
            .map(|p| self.count(p))
            .sum()
    }

    /// Forget the heads that `poll` gave `ci-finished` for (and the "no CI" answers).
    pub fn forget_ci_events(&self, who: &Agent) {
        let cd = ConfigDir(who.dir.clone());
        let p = cd.state_path("fake", &AgentName::parse(&who.name).unwrap());
        let mut c = StateCache::load(&p);
        c.late
            .late_seen
            .retain(|k, _| !k.ends_with("|ci-finished") && !k.ends_with("|ci-none-seen"));
        c.save(&p).unwrap();
    }

    /// The `ci-finished` events of a poll answer: the first line of each body, e.g.
    /// `ci-finished: green`.
    pub fn ci_events(v: &Value) -> Vec<String> {
        Self::ci_bodies(v)
            .iter()
            .map(|b| b.lines().next().unwrap_or("").to_string())
            .collect()
    }

    pub fn ci_bodies(v: &Value) -> Vec<String> {
        v["events"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|e| e["kind"] == "ci-finished")
                    .map(|e| e["body"].as_str().unwrap_or("").to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Run any command of the CLI against the front server (the markers and CI are the scene).
    pub fn run(&self, who: &Agent, args: &[&str]) -> Out {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(&who.dir)
            .args(args)
            .env("CREWBOARD_API_BASE", self.front.base())
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

    pub fn poll(&self, who: &Agent) -> (Value, Out) {
        self.poll_env(who, &[])
    }

    /// [`LateBoard::poll`] with extra environment variables.
    pub fn poll_env(&self, who: &Agent, env: &[(&str, &str)]) -> (Value, Out) {
        let o = Command::new(env!("CARGO_BIN_EXE_crewboard"))
            .arg("--config-dir")
            .arg(&who.dir)
            .args(["poll", "--json"])
            .env("CREWBOARD_API_BASE", self.front.base())
            .env("GH_TOKEN", "ghp_faketoken")
            // what the own `gh auth token` of the owner prints (owner commands)
            .env(crewboard::github::TEST_GH_AUTH_TOKEN_ENV, "ghp_faketoken")
            .env_remove("GITHUB_TOKEN")
            .env_remove("CREWBOARD_HEARTBEAT_INTERVAL")
            .env_remove("CREWBOARD_TEST_HEARTBEAT_INTERVAL")
            .envs(env.iter().copied())
            .output()
            .expect("run crewboard");
        let out = Out {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).to_string(),
            stderr: String::from_utf8_lossy(&o.stderr).to_string(),
        };
        let last = out.stdout.lines().last().unwrap_or("").to_string();
        let v: Value = serde_json::from_str(&last)
            .unwrap_or_else(|e| panic!("poll --json is not JSON ({e}): {}", out.all()));
        (v, out)
    }

    /// The body of the one `stale-work` event of a poll answer.
    pub fn stale_body(v: &Value) -> String {
        v["events"]
            .as_array()
            .and_then(|a| a.iter().find(|e| e["kind"] == "stale-work"))
            .and_then(|e| e["body"].as_str())
            .expect("a stale-work event")
            .to_string()
    }

    /// The `stale-work` events of a poll answer: the first line of each body, e.g.
    /// `stale-work: no-push`.
    pub fn stale_events(v: &Value) -> Vec<String> {
        v["events"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|e| e["kind"] == "stale-work")
                    .map(|e| {
                        e["body"]
                            .as_str()
                            .unwrap_or("")
                            .lines()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
