//! Integration tests against a sandbox (spec/overview#tests): a GitHub project and two repos
//! that the owner of the run provides. Four environment variables name them:
//! `CREWBOARD_IT_OWNER` (login of the org or user that owns the project),
//! `CREWBOARD_IT_PROJECT` (project number), `CREWBOARD_IT_REPO` and `CREWBOARD_IT_REPO2`
//! (both `owner/name`; repo 1 holds the registry issue). Without all four the tests skip.
//!
//! They talk to GitHub, so each of them is `#[ignore]`d: a plain `cargo test` stays offline
//! and reports them as ignored, never as ok. Run them with
//! `CREWBOARD_IT=1 cargo test --test it_sandbox -- --ignored`. Without the env the test
//! still skips loudly (SKIPPED on stderr) and returns, so `-- --ignored` alone is no
//! false green that ran something. They use the token of `gh auth token` (the owner's) and
//! two agent identities, `claude@it-a` and `hermes@it-b`, with their own config dirs. The
//! identities persist in `target/tmp/crewboard-it` between runs (a name is bound to its
//! key). Tests run one after the other: they share the cursor of agent B.
//!
//! Comments and items that a test creates are removed at the end (also on a panic).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]

use crewboard::board::OptionInfo;
use crewboard::config::{BoardConfig, ConfigDir};
use crewboard::feed;
use crewboard::github::Client;
use crewboard::ops::selftest::ensure_selftest_issue;
use crewboard::ops::status::missing_from_listing;
use crewboard::ops::{AgentCtx, Ctx, Global, new_client};
use crewboard::protocol::names::{AgentName, Kind, Re, Timestamp, To};
use crewboard::protocol::signature::{Outgoing, sign_message};
use crewboard::rules::options::{OptionChange, merge_options};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// The sandbox of this run, read from the environment.
#[derive(Debug, Clone, PartialEq)]
struct Sandbox {
    owner: String,
    project: u64,
    repo: String,
    repo2: String,
}

const IT_VARS: [&str; 4] = [
    "CREWBOARD_IT_OWNER",
    "CREWBOARD_IT_PROJECT",
    "CREWBOARD_IT_REPO",
    "CREWBOARD_IT_REPO2",
];

impl Sandbox {
    /// Read the four variables through `get`. The error names every missing or bad one.
    fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Sandbox, String> {
        let val = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        let missing: Vec<&str> = IT_VARS
            .iter()
            .copied()
            .filter(|k| val(k).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "set {} to run the sandbox integration tests (see CONTRIBUTING.md)",
                missing.join(", ")
            ));
        }
        let project = val("CREWBOARD_IT_PROJECT")
            .unwrap()
            .trim()
            .parse::<u64>()
            .map_err(|_| "CREWBOARD_IT_PROJECT must be a project number".to_string())?;
        for k in ["CREWBOARD_IT_REPO", "CREWBOARD_IT_REPO2"] {
            let v = val(k).unwrap();
            if v.split('/').count() != 2 || v.split('/').any(str::is_empty) {
                return Err(format!("{k} must be owner/name"));
            }
        }
        Ok(Sandbox {
            owner: val("CREWBOARD_IT_OWNER").unwrap().trim().to_string(),
            project,
            repo: val("CREWBOARD_IT_REPO").unwrap().trim().to_string(),
            repo2: val("CREWBOARD_IT_REPO2").unwrap().trim().to_string(),
        })
    }
}

/// The sandbox of this process, or the reason there is none.
fn sandbox_result() -> &'static Result<Sandbox, String> {
    static SB: OnceLock<Result<Sandbox, String>> = OnceLock::new();
    SB.get_or_init(|| Sandbox::from_vars(|k| std::env::var(k).ok()))
}

/// The sandbox. Only called after `enabled()` said yes.
fn sandbox() -> &'static Sandbox {
    sandbox_result().as_ref().expect("sandbox variables")
}

/// Repo 1 of the sandbox. It prints and derefs as its `owner/name`.
struct Repo;
const REPO: Repo = Repo;
impl std::ops::Deref for Repo {
    type Target = str;
    fn deref(&self) -> &str {
        &sandbox().repo
    }
}
impl std::fmt::Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&sandbox().repo)
    }
}

/// The board config of the sandbox for the given owner login.
fn board_config(sb: &Sandbox, owner_login: &str) -> String {
    format!(
        "initiative = \"sandbox\"\nproject_owner = \"{}\"\nproject_number = {}\nrepos = [\"{}\", \"{}\"]\nowner_login = \"{owner_login}\"\nregistry_issue = 2\ninsecure_one_account = true\n",
        sb.owner, sb.project, sb.repo, sb.repo2
    )
}

/// The login of the token owner, read once per process (`GET /user`).
fn owner_login() -> &'static str {
    static LOGIN: OnceLock<String> = OnceLock::new();
    LOGIN.get_or_init(|| {
        new_client()
            .expect("client")
            .viewer_login()
            .expect("owner login from the token")
    })
}

/// The board config text for the token owner.
fn board() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| board_config(sandbox(), owner_login()))
}

/// An AUTHORITY file text that lets the coordinator merge.
fn merge_authority_text(owner_login: &str) -> String {
    format!(
        "---\ninitiative: sandbox\nowner: {owner_login}\ncoordinator_may_create_tickets: true\ncoordinator_may_split_tickets: true\ncoordinator_may_merge: true\ndev_may_open_prs: true\ndev_branch_prefix: crew/\nprotected: [main]\nneeds_owner: [release, ci-config, credentials, delete, new-scope, new-repo]\n---\n\nTest authority: the coordinator may merge.\n"
    )
}

fn placeholder() -> Sandbox {
    Sandbox {
        owner: "example-org".into(),
        project: 7,
        repo: "example-org/app".into(),
        repo2: "example-org/app-2".into(),
    }
}

#[test]
fn sandbox_variables_are_read_and_checked() {
    let all = |k: &str| {
        Some(
            match k {
                "CREWBOARD_IT_OWNER" => "example-org",
                "CREWBOARD_IT_PROJECT" => "7",
                "CREWBOARD_IT_REPO" => "example-org/app",
                _ => "example-org/app-2",
            }
            .to_string(),
        )
    };
    assert_eq!(Sandbox::from_vars(all), Ok(placeholder()));
    // Nothing set: every variable is named.
    let e = Sandbox::from_vars(|_| None).unwrap_err();
    for k in IT_VARS {
        assert!(e.contains(k), "{e}");
    }
    // One empty, one bad number, one bad repo.
    let e = Sandbox::from_vars(|k| {
        if k == "CREWBOARD_IT_REPO2" {
            Some(" ".into())
        } else {
            all(k)
        }
    })
    .unwrap_err();
    assert!(
        e.contains("CREWBOARD_IT_REPO2") && !e.contains("CREWBOARD_IT_OWNER"),
        "{e}"
    );
    let e = Sandbox::from_vars(|k| {
        if k == "CREWBOARD_IT_PROJECT" {
            Some("x".into())
        } else {
            all(k)
        }
    })
    .unwrap_err();
    assert!(e.contains("project number"), "{e}");
    let e = Sandbox::from_vars(|k| {
        if k == "CREWBOARD_IT_REPO" {
            Some("noslash".into())
        } else {
            all(k)
        }
    })
    .unwrap_err();
    assert!(e.contains("owner/name"), "{e}");
}

#[test]
fn board_and_authority_text_take_the_owner_login_as_given() {
    let sb = placeholder();
    let b = board_config(&sb, "some-owner");
    assert!(b.contains("owner_login = \"some-owner\"\n"));
    assert!(b.contains("project_owner = \"example-org\"\n"));
    assert!(b.contains("project_number = 7\n"));
    assert!(BoardConfig::parse(&b).is_ok());
    assert!(merge_authority_text("some-owner").contains("\nowner: some-owner\n"));
    assert!(!b.contains("{owner_login}"));
}

static SERIAL: Mutex<()> = Mutex::new(());
static SETUP: OnceLock<Result<(), String>> = OnceLock::new();

/// The three standing identities of the tests. A name is bound to its key and to the owner's
/// decisions (a gate run retires every agent of the sandbox with `/crew reject`, and a
/// `left` agent cannot join again), so the names carry a suffix that is kept in
/// `identity.txt` and renewed, with new keys, when one of them was rejected or has left.
struct Names {
    a: String,
    b: String,
    c: String,
}

static NAMES: OnceLock<Names> = OnceLock::new();

fn names() -> &'static Names {
    NAMES.get_or_init(compute_names)
}

fn a_name() -> &'static str {
    &names().a
}

fn b_name() -> &'static str {
    &names().b
}

fn c_name() -> &'static str {
    &names().c
}

fn compute_names() -> Names {
    let owner = dir("owner");
    std::fs::create_dir_all(&owner).expect("owner dir");
    std::fs::write(owner.join("sandbox.board.toml"), board()).expect("board file");
    let file = root().join("identity.txt");
    let make = |sfx: &str| Names {
        a: format!("claude@it-{sfx}"),
        b: format!("hermes@it-{sfx}"),
        c: format!("codex@it-{sfx}"),
    };
    let old = std::fs::read_to_string(&file)
        .ok()
        .map(|t| t.trim().to_string());
    // FIRST step: claim the record of a reused identity with our pid, before anything is read
    // from the board. The watcher of the previous run in this target dir retires the
    // identity only while the record still names ITS pid; after this write it does nothing.
    if let Some(sfx) = &old {
        write_run_record(sfx, &make(sfx));
    }
    let out = run(&owner, &["status", "--json", "--initiative", "sandbox"]);
    assert_eq!(out.code, 0, "status for the identity check: {}", out.stderr);
    let v: Value = serde_json::from_str(&out.stdout).expect("status json");
    let gone = |name: &str| {
        v["agents"].as_array().is_some_and(|a| {
            a.iter()
                .any(|x| x["name"] == name && (x["rejected"] == true || x["state"] == "left"))
        })
    };
    if let Some(sfx) = &old {
        let n = make(sfx);
        if ![&n.a, &n.b, &n.c].iter().any(|x| gone(x)) {
            return n;
        }
    }
    let sfx = crewboard::clock::now_epoch().to_string();
    for who in ["a", "b", "c"] {
        let _ = std::fs::remove_dir_all(dir(who));
    }
    std::fs::write(&file, &sfx).expect("identity file");
    let n = make(&sfx);
    write_run_record(&sfx, &n);
    n
}

/// True when `CREWBOARD_IT=1` and the sandbox variables are set. Otherwise it prints why not.
fn enabled() -> bool {
    if std::env::var("CREWBOARD_IT").as_deref() != Ok("1") {
        eprintln!(
            "SKIPPED: set CREWBOARD_IT=1 to run the sandbox integration tests \
             (cargo test --test it_sandbox -- --ignored)"
        );
        return false;
    }
    match sandbox_result() {
        Ok(_) => true,
        Err(why) => {
            eprintln!("SKIPPED: {why}");
            false
        }
    }
}

/// Skip a test unless the sandbox is enabled. Returns the lock that serialises the tests.
macro_rules! it {
    () => {{
        if !enabled() {
            return;
        }
        let guard = serial();
        setup();
        guard
    }};
}

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("crewboard-it")
}

fn dir(who: &str) -> PathBuf {
    root().join(who)
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_env(cfg_dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_crewboard"));
    cmd.arg("--config-dir").arg(cfg_dir).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o = cmd.output().expect("run crewboard");
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).to_string(),
        stderr: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}

fn run(cfg_dir: &Path, args: &[&str]) -> Out {
    run_env(cfg_dir, args, &[])
}

fn expect_ok(o: &Out, what: &str) {
    assert_eq!(
        o.code, 0,
        "{what}: exit {}\nstdout:\n{}\nstderr:\n{}",
        o.code, o.stdout, o.stderr
    );
}

fn check_ok(o: &Out, what: &str) -> Result<(), String> {
    if o.code == 0 {
        Ok(())
    } else {
        Err(format!(
            "{what}: exit {}\nstdout:\n{}\nstderr:\n{}",
            o.code, o.stdout, o.stderr
        ))
    }
}

/// What another run on this host can read about a run: who owns the identities and whether
/// that process still lives. Kept in `$HOME/.cache/crewboard-it/runs/<identity suffix>`, one
/// small `key=value` file per run. A file next to the sandbox registry entry would need a
/// product field, and the dead-pid rule only holds on the same host anyway, so a local file
/// is the simplest robust place. Every test process rewrites the record of its own suffix at
/// start (the suffix is reused by later processes of the same target dir), so the pid is
/// always the newest owner. There is no end hook for a test binary: a stale record is
/// harmless because its pid is dead, and that is exactly what marks the run as finished.
#[derive(Debug, Clone, PartialEq)]
struct RunRecord {
    pid: u32,
    host: String,
    started: i64,
    /// Start time of the process as `ps -o lstart=` prints it; empty when unknown. It guards
    /// against a reused pid.
    pid_start: String,
    agents: Vec<String>,
}

fn host_name() -> String {
    Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn runs_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache/crewboard-it/runs"))
}

/// The start time of process `pid`. `Ok(None)`: the process does not exist. `Err`: unknown.
#[cfg(unix)]
fn process_start(pid: u32) -> Result<Option<String>, String> {
    let o = Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
    Ok(if t.is_empty() { None } else { Some(t) })
}

#[cfg(not(unix))]
fn process_start(_pid: u32) -> Result<Option<String>, String> {
    Err("no pid check on this platform".to_string())
}

/// Does the process of the record still live? When we cannot tell, the answer is yes: the safe
/// direction, because it blocks a setup and never retires a live run. Residual risk: a reused
/// pid whose start time we could not compare (no `lstart` in the record) looks alive and blocks
/// until the heartbeat is 2 hours old.
fn record_pid_alive(r: &RunRecord) -> bool {
    match process_start(r.pid) {
        Err(_) => true,
        Ok(None) => false,
        Ok(Some(now_start)) => r.pid_start.is_empty() || r.pid_start == now_start,
    }
}

fn format_record(r: &RunRecord) -> String {
    format!(
        "pid={}\nhost={}\nstarted={}\npid_start={}\nagents={}\n",
        r.pid,
        r.host,
        r.started,
        r.pid_start,
        r.agents.join(",")
    )
}

fn parse_record(text: &str) -> Option<RunRecord> {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).and_then(|r| r.strip_prefix('=')))
    };
    Some(RunRecord {
        pid: get("pid")?.trim().parse().ok()?,
        host: get("host")?.trim().to_string(),
        started: get("started")?.trim().parse().ok()?,
        pid_start: get("pid_start").unwrap_or("").trim().to_string(),
        agents: get("agents")
            .unwrap_or("")
            .split(',')
            .filter(|x| !x.is_empty())
            .map(str::to_string)
            .collect(),
    })
}

/// Record this run for the other runs on this host. A failure is not fatal: without a record
/// other runs fall back to the heartbeat rule.
fn write_run_record(sfx: &str, n: &Names) {
    let Some(dir) = runs_dir() else { return };
    let pid = std::process::id();
    let rec = RunRecord {
        pid,
        host: host_name(),
        started: crewboard::clock::now_epoch(),
        pid_start: process_start(pid).ok().flatten().unwrap_or_default(),
        agents: vec![n.a.clone(), n.b.clone(), n.c.clone()],
    };
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(sfx), format_record(&rec));
    }
}

/// The record of the run that owns agent `name` (`<runtime>@it-<suffix>`), if any.
fn read_run_record(name: &str) -> Option<RunRecord> {
    let sfx = name.split_once("@it-")?.1;
    if sfx.is_empty() || !sfx.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    parse_record(&std::fs::read_to_string(runs_dir()?.join(sfx)).ok()?)
}

/// A run counts as finished when its agent has had no heartbeat for this long. Parallel runs
/// (other lanes, gate scripts) share the sandbox, and a run that is still going must never be
/// retired by another: it would fail with `agent-rejected`. A normal run takes minutes, so
/// 2 hours is far beyond any live run.
const FINISHED_AFTER_SECS: i64 = 2 * 3600;

/// Is the agent of `a` (one entry of `status --json` `agents`) provably left over from a
/// finished run? Two proofs count. (a) A heartbeat older than [`FINISHED_AFTER_SECS`].
/// (b) A run record from THIS host (`here`) whose process is dead (`alive` says so). A record
/// with a live pid, a record from another host, and no record with a fresh heartbeat are NOT
/// proof. An agent without any heartbeat (joined, not started yet) is not provably finished by
/// (a): `status --json` has no creation time, and it may belong to a run that is just
/// starting. Our own names and rejected agents are never retired.
fn is_finished_run(
    a: &Value,
    now: i64,
    ours: &[&str],
    rec: Option<&RunRecord>,
    here: &str,
    alive: &dyn Fn(&RunRecord) -> bool,
) -> bool {
    let name = a["name"].as_str().unwrap_or("");
    if ours.contains(&name) || a["rejected"] == true {
        return false;
    }
    let old_heartbeat = a["heartbeat"]
        .as_str()
        .and_then(crewboard::clock::parse_ts)
        .is_some_and(|hb| now - hb > FINISHED_AFTER_SECS);
    old_heartbeat || rec.is_some_and(|r| !here.is_empty() && r.host == here && !alive(r))
}

/// [`is_finished_run`] with the record of the agent and the real pid check.
fn is_finished_now(a: &Value, now: i64, ours: &[&str]) -> bool {
    let rec = read_run_record(a["name"].as_str().unwrap_or(""));
    is_finished_run(a, now, ours, rec.as_ref(), &host_name(), &record_pid_alive)
}

#[cfg(test)]
mod finished_run_tests {
    use super::*;

    const NOW: i64 = 1_792_100_000;
    const HERE: &str = "mac-a";

    fn agent(name: &str, hb_age: Option<i64>, rejected: bool) -> Value {
        json!({"name": name, "rejected": rejected,
               "heartbeat": hb_age.map(|s| crewboard::clock::format_ts(NOW - s))})
    }

    fn rec(host: &str, pid: u32) -> RunRecord {
        RunRecord {
            pid,
            host: host.to_string(),
            started: NOW - 600,
            pid_start: String::new(),
            agents: vec!["hermes@it-7".to_string()],
        }
    }

    fn dead(_: &RunRecord) -> bool {
        false
    }

    fn live(_: &RunRecord) -> bool {
        true
    }

    fn fin(a: &Value, r: Option<&RunRecord>, alive: &dyn Fn(&RunRecord) -> bool) -> bool {
        is_finished_run(a, NOW, &["claude@it-1"], r, HERE, alive)
    }

    #[test]
    fn a_stale_run_is_retired_and_a_fresh_one_is_kept() {
        // stale: retire
        assert!(fin(
            &agent("hermes@it-0", Some(FINISHED_AFTER_SECS + 60), false),
            None,
            &live
        ));
        // fresh, no record: keep (another host or an old harness)
        assert!(!fin(&agent("hermes@it-2", Some(300), false), None, &dead));
        // exactly at the threshold: keep
        assert!(!fin(
            &agent("hermes@it-3", Some(FINISHED_AFTER_SECS), false),
            None,
            &dead
        ));
        // no heartbeat, no record: not provably finished, keep
        assert!(!fin(&agent("hermes@it-4", None, false), None, &dead));
    }

    #[test]
    fn our_own_and_rejected_agents_are_never_retired() {
        let r = rec(HERE, 1);
        assert!(!fin(
            &agent("claude@it-1", Some(99_999), false),
            Some(&r),
            &dead
        ));
        assert!(!fin(
            &agent("hermes@it-0", Some(99_999), true),
            Some(&r),
            &dead
        ));
    }

    #[test]
    fn a_record_on_this_host_with_a_dead_pid_is_finished() {
        let r = rec(HERE, 4_000_000);
        assert!(fin(&agent("hermes@it-7", Some(60), false), Some(&r), &dead));
        // also without any heartbeat
        assert!(fin(&agent("hermes@it-7", None, false), Some(&r), &dead));
    }

    #[test]
    fn a_record_on_this_host_with_a_live_pid_is_not_finished() {
        let r = rec(HERE, std::process::id());
        assert!(!fin(
            &agent("hermes@it-7", Some(60), false),
            Some(&r),
            &live
        ));
        // the real check: the pid of this test process lives
        assert!(record_pid_alive(&rec(HERE, std::process::id())));
    }

    #[test]
    fn a_record_from_another_host_is_not_finished_unless_the_heartbeat_is_old() {
        let r = rec("mac-b", 4_000_000);
        assert!(!fin(
            &agent("hermes@it-7", Some(60), false),
            Some(&r),
            &dead
        ));
        assert!(fin(
            &agent("hermes@it-7", Some(FINISHED_AFTER_SECS + 1), false),
            Some(&r),
            &dead
        ));
        // an unknown own host never matches
        let same_empty = rec("", 4_000_000);
        assert!(!is_finished_run(
            &agent("hermes@it-7", Some(60), false),
            NOW,
            &[],
            Some(&same_empty),
            "",
            &dead
        ));
    }

    #[test]
    fn the_pid_check_sees_a_dead_process() {
        #[cfg(unix)]
        {
            let mut child = Command::new("true").spawn().expect("spawn true");
            let pid = child.id();
            child.wait().expect("wait");
            let mut r = rec(HERE, pid);
            assert!(!record_pid_alive(&r), "a reaped child is dead");
            // a live pid with another start time is a reused pid: dead
            r.pid = std::process::id();
            r.pid_start = "Thu Jan  1 00:00:00 1970".to_string();
            assert!(!record_pid_alive(&r));
        }
    }

    #[test]
    fn a_record_survives_the_file_round_trip() {
        let mut r = rec(HERE, 42);
        r.pid_start = "Fri Oct 16 10:00:00 2026".to_string();
        r.agents = vec!["a@it-7".into(), "b@it-7".into(), "c@it-7".into()];
        assert_eq!(parse_record(&format_record(&r)), Some(r));
        assert_eq!(parse_record("garbage"), None);
    }
}

/// The sandbox is one board for every gate and test run. An agent of an earlier run that is
/// still `active` but silent would be reported dead by a coordinator, which is
/// right, but noise for the tests. The owner retires it the way the gate script does: an owner
/// comment `/crew reject` and `State = left`. Only names that earlier runs made are touched,
/// and only runs that are provably finished ([`is_finished_run`]): a run that still beats
/// its heart belongs to a parallel lane and stays.
fn retire_leftovers(owner: &Path) -> Result<(), String> {
    let o = run(owner, &["status", "--json", "--initiative", "sandbox"]);
    check_ok(&o, "status")?;
    let v: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
    let ours = [a_name(), b_name(), c_name()];
    let ctx = owner_ctx();
    let now = crewboard::clock::now_epoch();
    for a in v["agents"].as_array().ok_or("no agents")? {
        let name = a["name"].as_str().unwrap_or("");
        let made_by_a_run = name.contains("@it") || name.contains("@gate") || {
            name.split_once("@m")
                .is_some_and(|(_, r)| r.starts_with(|c: char| c.is_ascii_digit()))
        };
        if made_by_a_run && is_finished_now(a, now, &ours) {
            feed::post_comment(&ctx.client, &REPO, 2, &format!("/crew reject {name}"))
                .map_err(|e| e.to_string())?;
            if let Some(item) = a["item_id"].as_str() {
                let _ = ctx.board.set_select(&ctx.client, item, "State", "left");
            }
        }
    }
    Ok(())
}

/// Retire OUR agents after the test process has ended, so a clean run leaves no fresh
/// coordinator. libtest has no "after all tests" hook. A `Drop` of a static never runs. A C
/// `atexit` handler does run, but only after Rust's thread-locals are gone, and then the HTTP
/// client and `eprintln!` panic ("use of std::thread::current() is not possible"). So the work
/// runs in a separate watcher process that starts while the process is healthy: `setup()`
/// starts this same test binary again with the hidden test [`it_exit_retire_watcher`] and the
/// env vars below. The watcher waits until the pid of the run is gone (this also covers a
/// crash or a kill) and then retires our three names. It is best-effort: a failure prints a
/// warning and nothing waits for it. It touches only our names.
const WATCH_PID: &str = "CREWBOARD_IT_WATCH_PID";
const WATCH_START: &str = "CREWBOARD_IT_WATCH_START";
const WATCH_NAMES: &str = "CREWBOARD_IT_WATCH_NAMES";

#[cfg(unix)]
fn register_exit_cleanup() {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::sync::Once;
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let Some(n) = NAMES.get() else { return };
        let pid = std::process::id();
        let start = process_start(pid).ok().flatten().unwrap_or_default();
        let spawned = std::env::current_exe().and_then(|exe| {
            Command::new(exe)
                .args(["--exact", "it_exit_retire_watcher", "--test-threads=1"])
                .env(WATCH_PID, pid.to_string())
                .env(WATCH_START, start)
                .env(WATCH_NAMES, format!("{},{},{}", n.a, n.b, n.c))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
        });
        if let Err(e) = spawned {
            eprintln!("warning: could not start the exit cleanup of the IT identities: {e}");
        }
    });
}

#[cfg(not(unix))]
fn register_exit_cleanup() {
    let _ = retire_names as fn(&[String], &dyn Fn() -> bool) -> Result<(), String>;
}

/// Block until process `pid` is gone (or is another process with the same pid: a start time
/// that differs), then run `then`. When the pid check fails, it keeps waiting: an unknown
/// state never retires anything.
fn wait_for_exit_then(pid: u32, start: &str, poll: std::time::Duration, then: impl FnOnce()) {
    loop {
        match process_start(pid) {
            Ok(None) => break,
            Ok(Some(now)) if !start.is_empty() && now != start => break,
            _ => std::thread::sleep(poll),
        }
    }
    then();
}

/// Retire the given agents the way the owner does: `/crew reject` and `State = left`.
fn retire_names(names: &[String], still_ours: &dyn Fn() -> bool) -> Result<(), String> {
    let owner = dir("owner");
    let o = run(&owner, &["status", "--json", "--initiative", "sandbox"]);
    check_ok(&o, "status")?;
    let v: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
    let ctx = owner_ctx();
    for a in v["agents"].as_array().ok_or("no agents")? {
        let name = a["name"].as_str().unwrap_or("");
        if names.iter().any(|n| n == name) && a["rejected"] != true && a["state"] != "left" {
            // read the record again right before each retire: a newer run may have claimed it
            if !still_ours() {
                return Ok(());
            }
            feed::post_comment(&ctx.client, &REPO, 2, &format!("/crew reject {name}"))
                .map_err(|e| e.to_string())?;
            if let Some(item) = a["item_id"].as_str() {
                let _ = ctx.board.set_select(&ctx.client, item, "State", "left");
            }
        }
    }
    Ok(())
}

/// Not a test. This is the entry point of the watcher process (see above). Without the env
/// vars it returns at once, so a normal run is not affected.
#[test]
fn it_exit_retire_watcher() {
    let (Ok(pid), Ok(names)) = (std::env::var(WATCH_PID), std::env::var(WATCH_NAMES)) else {
        return;
    };
    let Ok(pid) = pid.parse::<u32>() else { return };
    let start = std::env::var(WATCH_START).unwrap_or_default();
    let names: Vec<String> = names.split(',').map(str::to_string).collect();
    let Some(sfx) = names[0].split_once("@it-").map(|x| x.1.to_string()) else {
        return;
    };
    let still_ours = || watch_owns(read_record_file(&sfx).as_ref(), pid, &start);
    wait_for_exit_then(pid, &start, std::time::Duration::from_secs(2), || {
        if !still_ours() {
            eprintln!("exit cleanup skipped: the record of {sfx} no longer names pid {pid}");
            return;
        }
        // the standing names and every agent that a test made and noted in the record
        let all = with_recorded_agents(&names, read_record_file(&sfx).as_ref());
        if let Err(e) = retire_names(&all, &still_ours) {
            eprintln!("warning: exit cleanup of the IT identities failed: {e}");
        }
        if still_ours()
            && let Some(dir) = runs_dir()
        {
            let _ = std::fs::remove_file(dir.join(&sfx));
        }
    });
}

/// `names`, then every agent of the record that is not in `names` yet.
fn with_recorded_agents(names: &[String], rec: Option<&RunRecord>) -> Vec<String> {
    let mut all = names.to_vec();
    for a in rec.map(|r| r.agents.as_slice()).unwrap_or_default() {
        if !all.contains(a) {
            all.push(a.clone());
        }
    }
    all
}

fn read_record_file(sfx: &str) -> Option<RunRecord> {
    parse_record(&std::fs::read_to_string(runs_dir()?.join(sfx)).ok()?)
}

/// May the watcher of (`pid`, `start`) retire the identity of this record? Only when the record
/// still names that process. A newer run in the same target dir reuses the identity and
/// overwrites the record first thing in `setup()`: then the identity is in use and stays. A
/// missing record means someone else already cleaned up: do nothing (the safe direction).
fn watch_owns(rec: Option<&RunRecord>, pid: u32, start: &str) -> bool {
    rec.is_some_and(|r| r.pid == pid && r.pid_start == start)
}

#[cfg(all(test, unix))]
mod exit_watch_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn the_watcher_acts_only_after_the_process_has_ended() {
        let t = Instant::now();
        let mut child = Command::new("sleep").arg("2").spawn().expect("spawn sleep");
        let pid = child.id();
        let start = process_start(pid).unwrap().unwrap_or_default();
        let waiter = std::thread::spawn(move || {
            let mut ran_at = None;
            wait_for_exit_then(pid, &start, Duration::from_millis(50), || {
                ran_at = Some(Instant::now());
            });
            ran_at
        });
        // real wait: the waiter must still be blocked; nothing to wait for (an ignored sandbox test)
        std::thread::sleep(Duration::from_millis(500));
        assert!(!waiter.is_finished(), "must wait while the pid lives");
        child.wait().expect("wait");
        let ran_at = waiter.join().unwrap().expect("action ran");
        assert!(
            ran_at - t >= Duration::from_millis(1900),
            "ran before the exit"
        );
    }

    #[test]
    fn the_watcher_retires_only_while_the_record_names_its_process() {
        let mut r = RunRecord {
            pid: 100,
            host: "h".into(),
            started: 1,
            pid_start: "t1".into(),
            agents: vec![],
        };
        // record still ours: retire
        assert!(watch_owns(Some(&r), 100, "t1"));
        // a newer run overwrote the record: do nothing
        r.pid = 200;
        assert!(!watch_owns(Some(&r), 100, "t1"));
        // same pid reused with another start time: do nothing
        r.pid = 100;
        r.pid_start = "t2".into();
        assert!(!watch_owns(Some(&r), 100, "t1"));
        // record missing: do nothing
        assert!(!watch_owns(None, 100, "t1"));
    }

    #[test]
    fn the_watcher_also_retires_the_agents_that_tests_made() {
        let names: Vec<String> = vec!["a@it-7".into(), "b@it-7".into()];
        let r = RunRecord {
            pid: 1,
            host: "h".into(),
            started: 1,
            pid_start: String::new(),
            agents: vec!["a@it-7".into(), "dev@itx-9-0".into()],
        };
        assert_eq!(
            with_recorded_agents(&names, Some(&r)),
            ["a@it-7", "b@it-7", "dev@itx-9-0"]
        );
        // control: without a record only the standing names
        assert_eq!(with_recorded_agents(&names, None), names);
    }

    #[test]
    fn a_reused_pid_with_another_start_time_counts_as_gone() {
        let mut ran = false;
        wait_for_exit_then(
            std::process::id(),
            "Thu Jan  1 00:00:00 1970",
            Duration::from_millis(10),
            || ran = true,
        );
        assert!(ran);
    }
}

/// Join, approve and start the two identities. Idempotent; runs once per test process.
/// A failure is remembered, so every test reports the same cause (not a poisoned Once).
fn setup() {
    let r = SETUP.get_or_init(|| {
        let owner = dir("owner");
        names();
        register_exit_cleanup();
        std::fs::create_dir_all(&owner).map_err(|e| e.to_string())?;
        std::fs::write(owner.join("sandbox.board.toml"), board()).map_err(|e| e.to_string())?;
        retire_leftovers(&owner)?;
        // spec/profiles#storage: `init` adds the Capabilities field to an existing board
        // (idempotent)
        check_ok(
            &run(
                &owner,
                &[
                    "init",
                    "--initiative",
                    "sandbox",
                    "--repo",
                    &REPO,
                    "--repo",
                    &sandbox().repo2,
                    "--project",
                    &sandbox().project.to_string(),
                    "--insecure-one-account",
                    "--no-pr",
                    "--no-branch-rule",
                ],
            ),
            "init (Capabilities field)",
        )?;
        for (who, name, runtime) in [("a", a_name(), "claude"), ("b", b_name(), "hermes")] {
            let d = dir(who);
            std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            let board_file = d.join("initiative-file.cfg");
            std::fs::write(&board_file, board()).map_err(|e| e.to_string())?;
            let bf = board_file.to_str().unwrap();
            check_ok(
                &run(
                    &d,
                    &[
                        "join",
                        "--agent",
                        name,
                        "--runtime",
                        runtime,
                        "--model",
                        "it",
                        "--initiative-file",
                        bf,
                    ],
                ),
                "join",
            )?;
            // Approve through the owner path: dev role (the sandbox already has a coordinator).
            check_ok(
                &run(&owner, &["approve", name, "--initiative", "sandbox"]),
                "approve",
            )?;
            check_ok(&run(&d, &["start"]), "start")?;
        }
        Ok(())
    });
    if let Err(e) = r {
        panic!("sandbox setup failed: {e}");
    }
}

fn agent(who: &str) -> AgentCtx {
    AgentCtx::load(&Global {
        config_dir: Some(dir(who)),
        ..Global::default()
    })
    .expect("load agent ctx")
}

fn owner_ctx() -> Ctx {
    let cfg = BoardConfig::parse(board()).unwrap();
    Ctx::load(new_client().unwrap(), cfg, ConfigDir(dir("owner"))).unwrap()
}

/// Removes comments and items at the end of a test, also when it panics.
struct Cleanup {
    client: Client,
    comments: Vec<(String, u64)>,
    items: Vec<(String, String)>,
    fields: Vec<String>,
    /// (repo, issue number): closed at the end.
    issues: Vec<(String, u64)>,
    /// (repo, PR number): closed at the end.
    prs: Vec<(String, u64)>,
    /// (repo, branch): deleted at the end.
    branches: Vec<(String, String)>,
}

impl Cleanup {
    fn new() -> Self {
        Cleanup {
            client: new_client().unwrap(),
            comments: vec![],
            items: vec![],
            fields: vec![],
            issues: vec![],
            prs: vec![],
            branches: vec![],
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        for (repo, id) in &self.comments {
            let _ = feed::delete_comment(&self.client, repo, *id);
        }
        for (project, item) in &self.items {
            let _ = self.client.graphql(
                "mutation($p:ID!,$i:ID!){deleteProjectV2Item(input:{projectId:$p,itemId:$i}){deletedItemId}}",
                json!({"p": project, "i": item}),
            );
        }
        for (repo, n) in &self.prs {
            let _ = self.client.send(
                "PATCH",
                &format!("repos/{repo}/pulls/{n}"),
                Some(&json!({"state": "closed"})),
            );
        }
        for (repo, b) in &self.branches {
            let _ = self
                .client
                .send("DELETE", &format!("repos/{repo}/git/refs/heads/{b}"), None);
        }
        for (repo, n) in &self.issues {
            let _ = self.client.send(
                "PATCH",
                &format!("repos/{repo}/issues/{n}"),
                Some(&json!({"state": "closed"})),
            );
        }
        for f in &self.fields {
            let _ = self.client.graphql(
                "mutation($f:ID!){deleteProjectV2Field(input:{fieldId:$f}){clientMutationId}}",
                json!({"f": f}),
            );
        }
    }
}

fn ts() -> Timestamp {
    Timestamp::parse(&crewboard::clock::now_ts()).unwrap()
}

/// Post a message that `from` signed with `key`, as a comment of `issue`.
fn post_signed(
    c: &mut Cleanup,
    issue: u64,
    key: &ssh_key::PrivateKey,
    from: &str,
    to: &str,
    kind: Kind,
    body: &str,
) -> (u64, String) {
    let issue_ref =
        crewboard::protocol::canonical::IssueRef::parse(&format!("{REPO}#{issue}")).unwrap();
    let msg = sign_message(
        key,
        &issue_ref,
        Outgoing {
            from: AgentName::parse(from).unwrap(),
            to: To::parse(to).unwrap(),
            kind,
            re: Re::None,
            ts: ts(),
        },
        body,
    )
    .unwrap();
    let text = msg.to_comment();
    let (id, _) = feed::post_comment(&c.client, &REPO, issue, &text).unwrap();
    c.comments.push((REPO.to_string(), id));
    (id, text)
}

fn test_issue(ac: &AgentCtx) -> u64 {
    ensure_selftest_issue(&ac.ctx.client, &REPO, "sandbox").unwrap()
}

/// `poll --json` for B. Returns the parsed object and the exit code.
fn poll_json(who: &str) -> (Value, i32) {
    let o = run(&dir(who), &["poll", "--json"]);
    let last = o.stdout.lines().last().unwrap_or("").to_string();
    let v: Value = serde_json::from_str(&last).unwrap_or_else(|e| {
        panic!(
            "poll --json output is not JSON ({e}): {}\nstderr: {}",
            o.stdout, o.stderr
        )
    });
    (v, o.code)
}

fn drain(who: &str) {
    for _ in 0..3 {
        let (_, code) = poll_json(who);
        assert!(code == 0 || code == 1, "poll failed: {code}");
        if code == 1 {
            return;
        }
    }
}

fn ids(v: &Value, key: &str) -> Vec<u64> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array in {v}"))
        .iter()
        .map(|e| e["comment_id"].as_u64().unwrap())
        .collect()
}

// ---------------------------------------------------------------------------------------

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_selftest_passes_for_both_agents() {
    let _g = it!();
    for who in ["a", "b"] {
        let o = run(&dir(who), &["agent", "selftest"]);
        expect_ok(&o, "selftest");
        assert!(o.stdout.contains("selftest PASS"), "{}", o.stdout);
        assert!(
            o.stdout.contains("forged message refused as bad-signature"),
            "{}",
            o.stdout
        );
    }
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_message_round_trip_between_two_agents_with_verified_sender() {
    let _g = it!();
    drain("b");
    let issue = test_issue(&agent("a"));
    let mut c = Cleanup::new();
    let o = run(
        &dir("a"),
        &[
            "say",
            &issue.to_string(),
            "--to",
            b_name(),
            "--kind",
            "request",
            "--body",
            "round trip body",
        ],
    );
    expect_ok(&o, "say");
    let id: u64 = o
        .stdout
        .split("comment ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    c.comments.push((REPO.to_string(), id));
    let (v, code) = poll_json("b");
    assert_eq!(code, 0, "{v}");
    let ev = v["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["comment_id"].as_u64() == Some(id))
        .unwrap_or_else(|| panic!("B did not get the message: {v}"));
    assert_eq!(ev["from"], a_name());
    assert_eq!(ev["kind"], "request");
    assert_eq!(ev["verified"], true);
    assert_eq!(ev["body"], "round trip body");
    assert_eq!(ev["class"], "message");
    // delivered once: the cursor moved after printing, so a second poll is quiet
    let (v2, code2) = poll_json("b");
    assert!(!ids(&v2, "events").contains(&id), "delivered twice: {v2}");
    assert!(code2 == 1 || code2 == 0);
    // A is not addressed and does not get its own message
    let (va, _) = poll_json("a");
    assert!(
        !ids(&va, "events").contains(&id),
        "A got its own message back: {va}"
    );
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_quiet_poll_leaves_events_pending_and_a_later_read_still_sees_them() {
    let _g = it!();
    drain("b");
    let a = agent("a");
    let issue = test_issue(&a);
    let mut c = Cleanup::new();
    let (id, _) = post_signed(
        &mut c,
        issue,
        &a.key,
        a_name(),
        b_name(),
        Kind::Info,
        "pending",
    );
    for _ in 0..2 {
        let o = run(&dir("b"), &["poll", "--quiet"]);
        assert_eq!(
            o.code, 0,
            "quiet poll must report pending events every time: {} {}",
            o.stdout, o.stderr
        );
        assert!(o.stdout.contains("events pending"), "{}", o.stdout);
    }
    let (v, code) = poll_json("b");
    assert_eq!(code, 0, "{v}");
    assert!(
        ids(&v, "events").contains(&id),
        "a quiet poll must not hide the event: {v}"
    );
    let (_, code) = poll_json("b");
    assert_eq!(code, 1, "after the read there is nothing new");
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_new_item_is_readable_by_id_whether_or_not_the_listing_shows_it() {
    let _g = it!();
    let ctx = owner_ctx();
    let mut c = Cleanup::new();
    let id = ctx
        .board
        .add_draft(&ctx.client, "it-by-id", "integration test item")
        .unwrap();
    c.items.push((ctx.board.project_id.clone(), id.clone()));
    ctx.board
        .set_text(&ctx.client, &id, "Doing", "read me by id")
        .unwrap();
    // by id: immediately and always
    let it = ctx.board.read_item(&ctx.client, &id).unwrap();
    assert_eq!(it.field("Doing"), Some("read me by id"));
    assert_eq!(it.project_number, Some(sandbox().project));
    // the listing may or may not have it; crewboard must not depend on it
    let listed = ctx.board.list_item_ids(&ctx.client).unwrap();
    let missing = missing_from_listing(std::slice::from_ref(&id), &listed);
    assert_eq!(missing.is_empty(), listed.contains(&id));
    eprintln!(
        "item {id}: listed={} (either is fine)",
        listed.contains(&id)
    );
    // an unknown id is None, not an error
    let r = ctx
        .board
        .read_items(&ctx.client, &["PVTI_doesnotexist".to_string()]);
    assert!(r.is_err() || r.unwrap()[0].is_none());
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_adding_an_option_keeps_item_values_and_a_list_without_ids_is_refused() {
    let _g = it!();
    let mut ctx = owner_ctx();
    let mut c = Cleanup::new();
    let name = format!("it-scratch-{}", crewboard::clock::now_epoch());
    // a scratch single select, so that no real field is ever damaged
    let d = ctx
        .client
        .graphql(
            "mutation($p:ID!,$n:String!){createProjectV2Field(input:{projectId:$p,dataType:SINGLE_SELECT,name:$n,singleSelectOptions:[{name:\"one\",color:BLUE,description:\"\"},{name:\"two\",color:GREEN,description:\"\"}]}){projectV2Field{... on ProjectV2SingleSelectField{id}}}}",
            json!({"p": ctx.board.project_id, "n": name}),
        )
        .unwrap();
    let field_id = d
        .pointer("/createProjectV2Field/projectV2Field/id")
        .and_then(Value::as_str)
        .unwrap()
        .to_string();
    c.fields.push(field_id.clone());
    let item = ctx
        .board
        .add_draft(&ctx.client, "it-option-survival", "x")
        .unwrap();
    c.items.push((ctx.board.project_id.clone(), item.clone()));
    ctx.board.reload(&ctx.client).unwrap();
    ctx.board
        .set_select(&ctx.client, &item, &name, "two")
        .unwrap();
    assert_eq!(
        ctx.board
            .read_item(&ctx.client, &item)
            .unwrap()
            .field(&name),
        Some("two")
    );
    let before: Vec<OptionInfo> = ctx.board.field(&name).unwrap().options.clone();

    // 1. add an option through the one writer: ids kept, value kept
    let merged = merge_options(
        &ctx.board.field(&name).unwrap().entries(),
        &OptionChange::Add {
            name: "three".into(),
        },
    )
    .unwrap();
    let after = ctx
        .board
        .write_options(&ctx.client, &name, &merged)
        .unwrap();
    assert_eq!(after.len(), 3);
    for b in &before {
        assert!(
            after.iter().any(|a| a.id == b.id),
            "option id {} lost",
            b.id
        );
    }
    assert_eq!(
        ctx.board
            .read_item(&ctx.client, &item)
            .unwrap()
            .field(&name),
        Some("two"),
        "item value must survive"
    );

    // 2. a list that drops an earlier id is refused BEFORE any write
    let lossy: Vec<_> = merged.iter().filter(|d| d.name != "one").cloned().collect();
    let e = ctx
        .board
        .write_options(&ctx.client, &name, &lossy)
        .unwrap_err();
    assert!(e.to_string().contains("option-id-lost"), "{e}");
    let still = ctx.board.read_options(&ctx.client, &name).unwrap();
    assert_eq!(
        still.len(),
        3,
        "the refused write must not have changed anything"
    );

    // 3. control: the raw write without ids DOES clear the value, so the
    //    check above guards against something real
    let f = ctx.board.field(&name).unwrap().clone();
    ctx.client
        .graphql(
            "mutation($f:ID!){updateProjectV2Field(input:{fieldId:$f,singleSelectOptions:[{name:\"one\",color:BLUE,description:\"\"},{name:\"two\",color:GREEN,description:\"\"},{name:\"three\",color:GRAY,description:\"\"},{name:\"four\",color:GRAY,description:\"\"}]}){projectV2Field{... on ProjectV2SingleSelectField{id}}}}",
            json!({"f": f.id}),
        )
        .unwrap();
    assert_eq!(
        ctx.board
            .read_item(&ctx.client, &item)
            .unwrap()
            .field(&name),
        None,
        "without ids the item value is cleared (M0 2c)"
    );
}

// ---------------------------------------------------------------------------------------
// Tickets. A coordinator `codex@it-c` is made the single coordinator of the sandbox (an
// earlier coordinator, for example a gate agent, is retired the way the owner does it: an
// owner comment and State=left). The dev agent is B (`hermes@it-b`).

fn setup_coordinator() {
    static ONCE: OnceLock<Result<(), String>> = OnceLock::new();
    let r = ONCE.get_or_init(|| {
        let owner = dir("owner");
        let o = run(&owner, &["status", "--json", "--initiative", "sandbox"]);
        check_ok(&o, "status")?;
        let v: Value = serde_json::from_str(&o.stdout).map_err(|e| e.to_string())?;
        let ctx = owner_ctx();
        let now = crewboard::clock::now_epoch();
        let ours = [a_name(), b_name(), c_name()];
        for a in v["agents"].as_array().ok_or("no agents")? {
            let name = a["name"].as_str().unwrap_or("");
            if a["role"] == "coordinator"
                && a["rejected"] == false
                && a["state"] != "left"
                && name != c_name()
                && !is_finished_now(a, now, &ours)
            {
                // The sandbox has one coordinator. A foreign one that is not provably finished
                // may belong to a parallel run: do not reject it, fail with a clear cause.
                return Err(format!(
                    "sandbox busy: fresh coordinator {name} (heartbeat {}); wait for that run, retire it by hand, or run on the host that owns it",
                    a["heartbeat"].as_str().unwrap_or("none")
                ));
            }
            if a["role"] == "coordinator" && a["rejected"] == false && name != c_name() {
                feed::post_comment(&ctx.client, &REPO, 2, &format!("/crew reject {name}"))
                    .map_err(|e| e.to_string())?;
                if let Some(item) = a["item_id"].as_str() {
                    let _ = ctx.board.set_select(&ctx.client, item, "State", "left");
                }
            }
        }
        let d = dir("c");
        std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
        let bf = d.join("initiative-file.cfg");
        std::fs::write(&bf, board()).map_err(|e| e.to_string())?;
        check_ok(
            &run(
                &d,
                &[
                    "join",
                    "--agent",
                    c_name(),
                    "--runtime",
                    "codex",
                    "--model",
                    "it",
                    "--initiative-file",
                    bf.to_str().unwrap(),
                ],
            ),
            "join coordinator",
        )?;
        check_ok(
            &run(
                &owner,
                &[
                    "approve",
                    c_name(),
                    "--role",
                    "coordinator",
                    "--initiative",
                    "sandbox",
                ],
            ),
            "approve coordinator",
        )?;
        check_ok(&run(&d, &["start"]), "start coordinator")
    });
    if let Err(e) = r {
        panic!("coordinator setup failed: {e}");
    }
}

/// Create a ticket issue as the owner. `ci_none` adds the `CI: none` line.
fn new_ticket(c: &mut Cleanup, title: &str, ci_none: bool, gate: bool) -> u64 {
    let mut body = String::from("Goal: integration test ticket.\n\n");
    if gate {
        body.push_str("**Gate:** the PR of this ticket is open and closes it.\n");
    }
    if ci_none {
        body.push_str("CI: none\n");
    }
    let r = c
        .client
        .post(
            &format!("repos/{REPO}/issues"),
            &json!({"title": title, "body": body, "labels": ["crew:work"]}),
        )
        .unwrap();
    let n = r.body["number"].as_u64().unwrap();
    c.issues.push((REPO.to_string(), n));
    n
}

/// Item id from the output of `assign`; registered for removal.
fn item_of(c: &mut Cleanup, o: &Out) -> String {
    let id = o
        .stdout
        .split("added to the board as ")
        .nth(1)
        .unwrap_or_else(|| panic!("no item id in: {}", o.stdout))
        .split(';')
        .next()
        .unwrap()
        .to_string();
    c.items.push((owner_ctx().board.project_id, id.clone()));
    id
}

fn field_of(item: &str, field: &str) -> Option<String> {
    let ctx = owner_ctx();
    ctx.board
        .read_item(&ctx.client, item)
        .unwrap()
        .field(field)
        .map(str::to_string)
}

/// Branch + one commit + PR "Closes #n" through the REST API. Returns (pr url, number, head sha).
fn make_pr(c: &mut Cleanup, issue: u64, agent: &str) -> (String, u64, String, String) {
    let branch = format!("crew/{agent}/{issue}-it");
    let base = c
        .client
        .get(&format!("repos/{REPO}/git/ref/heads/main"))
        .unwrap()
        .body["object"]["sha"]
        .as_str()
        .unwrap()
        .to_string();
    c.client
        .post(
            &format!("repos/{REPO}/git/refs"),
            &json!({"ref": format!("refs/heads/{branch}"), "sha": base}),
        )
        .unwrap();
    c.branches.push((REPO.to_string(), branch.clone()));
    push_commit(c, &branch, "first");
    let pr = c
        .client
        .post(
            &format!("repos/{REPO}/pulls"),
            &json!({"title": format!("IT work for #{issue}"), "head": branch, "base": "main", "body": format!("IT.\n\nCloses #{issue}")}),
        )
        .unwrap()
        .body;
    let n = pr["number"].as_u64().unwrap();
    c.prs.push((REPO.to_string(), n));
    (
        pr["html_url"].as_str().unwrap().to_string(),
        n,
        pr["head"]["sha"].as_str().unwrap().to_string(),
        branch,
    )
}

fn push_commit(c: &Cleanup, branch: &str, label: &str) -> String {
    use base64::Engine;
    let nonce = crewboard::clock::now_epoch();
    let content = base64::engine::general_purpose::STANDARD.encode(format!("{label} {nonce}"));
    c.client
        .send(
            "PUT",
            &format!("repos/{REPO}/contents/crew-it/{label}-{nonce}.txt"),
            Some(&json!({"message": format!("it {label}"), "content": content, "branch": branch})),
        )
        .unwrap()
        .body["commit"]["sha"]
        .as_str()
        .unwrap()
        .to_string()
}

fn argv<'a>(v: &'a [&'a str]) -> Vec<&'a str> {
    v.to_vec()
}

// ---------------------------------------------------------------------------------------
// Lifecycle. Agents that a test stops or silences are made for the test (`fresh_dev`): a
// `left` agent never comes back, and the standing identities are used by the other tests.

use std::sync::atomic::{AtomicU64, Ordering};

static FRESH: AtomicU64 = AtomicU64::new(0);

/// Join, approve and start a new dev agent with its own config dir. Returns (dir, name).
fn fresh_dev(tag: &str) -> (PathBuf, String) {
    let n = FRESH.fetch_add(1, Ordering::SeqCst);
    let nonce = crewboard::clock::now_epoch();
    let name = format!("{tag}@itx-{nonce}-{n}");
    let d = root().join(format!("x-{nonce}-{n}"));
    std::fs::create_dir_all(&d).unwrap();
    let bf = d.join("initiative-file.cfg");
    std::fs::write(&bf, board()).unwrap();
    expect_ok(
        &run(
            &d,
            &[
                "join",
                "--agent",
                &name,
                "--runtime",
                // a tag that is no runtime (kimi) joins as codex; the name keeps the tag
                if matches!(tag, "claude" | "hermes") {
                    tag
                } else {
                    "codex"
                },
                "--model",
                "it",
                "--initiative-file",
                bf.to_str().unwrap(),
            ],
        ),
        "join fresh",
    );
    expect_ok(
        &run(
            &dir("owner"),
            &["approve", &name, "--initiative", "sandbox"],
        ),
        "approve fresh",
    );
    note_run_agent(&name);
    expect_ok(&run(&d, &["start"]), "start fresh");
    (d, name)
}

/// Add an agent that a test made to the run record of this process, so the exit watcher
/// retires it with the three standing names. Without this, every run left its
/// `@itx` agents that were not stopped in a running state. The cap prune of `approve`
/// (spec/board#option-limit) can only remove the `Agent` option of an agent that has left.
/// Best-effort: no record (another run claimed the identity) means no note.
fn note_run_agent(name: &str) {
    let Some(sfx) = a_name().split_once("@it-").map(|x| x.1.to_string()) else {
        return;
    };
    let Some(mut rec) = read_record_file(&sfx) else {
        return;
    };
    if rec.pid != std::process::id() {
        return;
    }
    if !rec.agents.iter().any(|a| a == name) {
        rec.agents.push(name.to_string());
    }
    if let Some(dir) = runs_dir() {
        let _ = std::fs::write(dir.join(&sfx), format_record(&rec));
    }
}

fn item_id_of(d: &Path) -> String {
    AgentCtx::load(&Global {
        config_dir: Some(d.to_path_buf()),
        ..Global::default()
    })
    .expect("agent ctx")
    .agent
    .item_id
}

fn poll_in(d: &Path, env: &[(&str, &str)]) -> (Value, i32) {
    let o = run_env(d, &["poll", "--json"], env);
    let last = o.stdout.lines().last().unwrap_or("").to_string();
    let v: Value = serde_json::from_str(&last)
        .unwrap_or_else(|e| panic!("poll --json is not JSON ({e}): {}\n{}", o.stdout, o.stderr));
    (v, o.code)
}

fn registry_comments(c: &Cleanup) -> Vec<crewboard::comments::Comment> {
    feed::issue_comments(&c.client, &REPO, 2, None, owner_login())
        .unwrap()
        .0
}

/// The `late-key:` lines of the heartbeat notes about the coordinator to the owner in the registry,
/// one entry per note, in comment order.
fn outage_keys(c: &Cleanup) -> Vec<String> {
    let prefix = format!("late-key: heartbeat:{}:", c_name());
    registry_comments(c)
        .iter()
        .filter(|x| x.body.contains("to=owner kind=info") && x.body.contains("late: heartbeat "))
        .filter_map(|x| {
            x.body
                .lines()
                .find(|l| l.trim_start().starts_with(&prefix))
                .map(|l| l.trim().to_string())
        })
        .collect()
}

fn count_registry(c: &Cleanup, pred: impl Fn(&str) -> bool) -> usize {
    registry_comments(c)
        .iter()
        .filter(|x| pred(&x.body))
        .count()
}

fn liveness_events<'a>(v: &'a Value, kind: &str) -> Vec<&'a Value> {
    v["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["class"] == "liveness" && e["kind"] == kind)
                .collect()
        })
        .unwrap_or_default()
}

/// The events of poll output `v` whose body names agent `name`. Events about other agents
/// (a standing identity or a leftover of an earlier test process that went dead meanwhile)
/// do not belong to the test that polls.
fn events_naming<'a>(v: &'a Value, name: &str) -> Vec<&'a Value> {
    v["events"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["body"].as_str().is_some_and(|b| b.contains(name)))
                .collect()
        })
        .unwrap_or_default()
}

fn epoch_of(item: &str, field: &str) -> i64 {
    crewboard::clock::parse_ts(&field_of(item, field).unwrap_or_default()).unwrap_or(0)
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_heartbeat_pause_resume_stop_and_the_owner_commands() {
    let _g = it!();
    setup_coordinator();
    drain("c");
    let (c_dir, owner) = (dir("c"), dir("owner"));
    let (x_dir, x) = fresh_dev("kimi");
    let xi = item_id_of(&x_dir);
    let mut c = Cleanup::new();

    // heartbeat: one command, a fresh time on the board
    let o = run(&x_dir, &["heartbeat"]);
    expect_ok(&o, "heartbeat");
    assert!(o.stdout.contains("heartbeat 20"), "{}", o.stdout);
    let age = crewboard::clock::now_epoch() - epoch_of(&xi, "Heartbeat");
    assert!((0..120).contains(&age), "heartbeat age {age}");

    // pause: assign refuses, the agent stays alive (control: resume lets assign pass)
    expect_ok(&run(&x_dir, &["pause"]), "pause");
    assert_eq!(field_of(&xi, "State").as_deref(), Some("paused"));
    let t = new_ticket(&mut c, "IT pause", true, true);
    let ts = t.to_string();
    let o = run(&c_dir, &["assign", &ts, "--to", &x]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("status-agent-not-active-dev") && o.stderr.contains("paused"),
        "{}",
        o.stderr
    );
    expect_ok(&run(&x_dir, &["resume"]), "resume");
    assert_eq!(field_of(&xi, "State").as_deref(), Some("active"));
    let o = run(&c_dir, &["assign", &ts, "--to", &x]);
    expect_ok(&o, "assign after the resume");
    let item = item_of(&mut c, &o);
    // a paused agent takes nothing new; after the resume it does
    expect_ok(&run(&x_dir, &["pause"]), "pause again");
    let o = run(&x_dir, &["take", &ts]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("agent-paused"), "{}", o.stderr);
    expect_ok(&run(&x_dir, &["resume"]), "resume again");
    expect_ok(&run(&x_dir, &["take", &ts]), "take");
    assert_eq!(field_of(&xi, "State").as_deref(), Some("active"));

    // owner command: pause. Applied once (one ack), and the agent cannot undo it itself.
    let o = run(&owner, &["pause", "--agent", &x, "--initiative", "sandbox"]);
    expect_ok(&o, "owner pause");
    let pause_id: u64 = o
        .stdout
        .split("(comment ")
        .nth(1)
        .and_then(|r| r.split(',').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no comment id in {}", o.stdout));
    c.comments.push((REPO.to_string(), pause_id));
    let (v, code) = poll_in(&x_dir, &[]);
    assert_eq!(code, 0, "{v}");
    assert!(
        liveness_events(&v, "owner-command")
            .iter()
            .any(|e| e["body"].as_str().unwrap().contains("/crew pause applied")),
        "{v}"
    );
    assert_eq!(field_of(&xi, "State").as_deref(), Some("paused"));
    let acks = |c: &Cleanup, id: u64| {
        count_registry(c, |b| {
            b.contains(&format!("kind=ack re=#2/c-{id} ")) && b.contains(&format!("from={x} "))
        })
    };
    assert_eq!(acks(&c, pause_id), 1);
    let _ = poll_in(&x_dir, &[]);
    assert_eq!(acks(&c, pause_id), 1, "a second poll applies nothing again");
    let o = run(&x_dir, &["resume"]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("resume-owner-paused"), "{}", o.stderr);
    assert_eq!(field_of(&xi, "State").as_deref(), Some("paused"));
    let o = run(
        &owner,
        &["resume", "--agent", &x, "--initiative", "sandbox"],
    );
    expect_ok(&o, "owner resume");
    let (v, _) = poll_in(&x_dir, &[]);
    assert!(!liveness_events(&v, "owner-command").is_empty(), "{v}");
    assert_eq!(field_of(&xi, "State").as_deref(), Some("active"));

    // owner command: stop, with a ticket In Progress. draining -> coordinator returns it -> left
    let o = run(&owner, &["stop", "--agent", &x, "--initiative", "sandbox"]);
    expect_ok(&o, "owner stop");
    let (v, _) = poll_in(&x_dir, &[]);
    assert_eq!(field_of(&xi, "State").as_deref(), Some("draining"), "{v}");
    assert_eq!(field_of(&item, "Status").as_deref(), Some("In Progress"));
    assert_eq!(field_of(&item, "Agent").as_deref(), Some(x.as_str()));
    let (vc, code) = poll_json("c");
    assert_eq!(code, 0, "{vc}");
    assert!(
        liveness_events(&vc, "agent-stopped")
            .iter()
            .any(|e| e["body"].as_str().unwrap().contains(&format!("#{t}"))),
        "{vc}"
    );
    assert_eq!(field_of(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(field_of(&item, "Agent"), None, "the Agent field is cleared");
    let (v, _) = poll_in(&x_dir, &[]);
    assert!(!liveness_events(&v, "left").is_empty(), "{v}");
    assert_eq!(field_of(&xi, "State").as_deref(), Some("left"));
    let o = run(&x_dir, &["poll"]);
    assert_eq!(o.code, 2, "a left agent's loop must stop: {}", o.stderr);
    assert!(o.stderr.contains("agent-left"), "{}", o.stderr);
    // and nothing is assigned to a departed agent
    let o = run(&c_dir, &["assign", &ts, "--to", &x]);
    assert_eq!(o.code, 2, "{}", o.stderr);
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_a_dead_agent_loses_its_ticket_and_the_owner_is_told_once() {
    let _g = it!();
    setup_coordinator();
    // The interval is 10 s for every process of this test: late after 30 s, dead after 50 s.
    let env: &[(&str, &str)] = &[("CREWBOARD_TEST_HEARTBEAT_INTERVAL", "10s")];
    let (c_dir, i) = (dir("c"), 10i64);
    let (y_dir, y) = fresh_dev("claude");
    let (z_dir, z) = fresh_dev("hermes");
    let (yi, _zi) = (item_id_of(&y_dir), item_id_of(&z_dir));
    let mut c = Cleanup::new();
    // The interval is the same for every agent of the initiative, so the standing identities
    // would be judged by it too: they keep their heartbeat fresh like the control agent z.
    let keep_alive = |extra: &Path| {
        for d in [dir("a"), dir("b"), extra.to_path_buf()] {
            expect_ok(&run_env(&d, &["heartbeat"], env), "heartbeat");
        }
    };
    keep_alive(&z_dir);
    let t = new_ticket(&mut c, "IT dead agent", true, true);
    let ts = t.to_string();
    // the commands are slow against a 3 s interval: y heartbeats right before the assign
    keep_alive(&z_dir);
    expect_ok(&run_env(&y_dir, &["heartbeat"], env), "heartbeat of y");
    let _ = poll_in(&c_dir, env);
    let o = run_env(&c_dir, &["assign", &ts, "--to", &y], env);
    expect_ok(&o, "assign to the live agent");
    let item = item_of(&mut c, &o);
    expect_ok(&run_env(&y_dir, &["take", &ts], env), "take");
    let hb = epoch_of(&yi, "Heartbeat");
    assert!(hb > 0);
    // y is silent from here on; z (the control) keeps polling; c polls and looks
    let (mut dead_events, mut dead_at, mut z_events, mut late_events) = (0, None, 0, 0);
    let start = crewboard::clock::now_epoch();
    while crewboard::clock::now_epoch() - start < 150 && dead_at.is_none() {
        // real wait: a poll of the sandbox for a heartbeat age that only real time makes
        std::thread::sleep(std::time::Duration::from_millis(1500));
        keep_alive(&z_dir);
        let (v, code) = poll_in(&c_dir, env);
        assert!(code <= 1, "{v}");
        let now = crewboard::clock::now_epoch();
        // A late agent is silent. The model hears of an agent only when
        // it is dead, and then once.
        late_events += liveness_events(&v, "agent-late").len();
        for e in liveness_events(&v, "agent-dead") {
            let body = e["body"].as_str().unwrap();
            assert!(
                !body.contains(&z),
                "the control agent must never be reported"
            );
            if body.contains(&y) {
                dead_events += 1;
                dead_at = dead_at.or(Some(now));
            }
        }
        z_events +=
            v["events"].as_array().map_or(0, |a| a.len()) - liveness_events(&v, "agent-dead").len();
        let _ = z_events;
    }
    assert_eq!(late_events, 0, "no event for a late agent");
    assert_eq!(dead_events, 1, "exactly one event for the dead agent");
    let dead_at = dead_at.expect("y never dead");
    assert!(
        dead_at - hb >= 5 * i,
        "dead only after 5 intervals without a heartbeat"
    );
    assert_eq!(
        count_registry(&c, |b| b.contains("kind=ping") || b.contains("kind=pong")),
        0,
        "no ping or pong on the way to dead"
    );
    // the ticket is back in Ready without an Agent, with a signed notice and its derived reason
    assert_eq!(field_of(&item, "Status").as_deref(), Some("Ready"));
    assert_eq!(field_of(&item, "Agent"), None);
    let (tc, _) = feed::issue_comments(&c.client, &REPO, t, None, owner_login()).unwrap();
    let notes = tc
        .iter()
        .filter(|x| {
            x.body.contains(&format!("to={y} kind=info"))
                && x.body
                    .contains(&format!("Reassigned. Reason: agent {y} is dead"))
        })
        .count();
    assert_eq!(notes, 1);
    // one report, one message to the owner; two more polls add nothing
    let counts = |c: &Cleanup| {
        (
            count_registry(c, |b| {
                b.contains("to=all kind=report") && b.contains(&format!("agent-dead: {y}"))
            }),
            count_registry(c, |b| {
                b.contains("to=owner kind=info") && b.contains(&format!("agent-dead: {y}"))
            }),
        )
    };
    assert_eq!(counts(&c), (1, 1));
    for _ in 0..2 {
        let (v, code) = poll_in(&c_dir, env);
        assert!(code <= 1, "{v}");
        // Judge only the events of this test's own agent. Other agents of the initiative
        // (the standing identities, leftovers of an earlier test process) may turn dead while
        // this test runs, and the coordinator rightly reports them.
        assert!(
            events_naming(&v, &y).is_empty(),
            "a second poll repeats nothing about {y}: {v}"
        );
    }
    assert_eq!(counts(&c), (1, 1), "nothing is posted twice");
    // a dead agent is not given work
    let o = run_env(&c_dir, &["assign", &ts, "--to", &y], env);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("status-agent-not-alive"), "{}", o.stderr);
    // y comes back: its poll tells it that the ticket is gone, and the coordinator sees it
    // alive again
    let (vy, code) = poll_in(&y_dir, env);
    assert_eq!(code, 0, "{vy}");
    assert!(
        vy["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "unassigned"),
        "{vy}"
    );
    assert_eq!(
        count_registry(&c, |b| b.contains(&format!("from={y} "))
            && b.contains("kind=pong")),
        0,
        "the returning agent sends no pong"
    );
    // the coordinator says nothing about the return (silent bookkeeping)
    // (the standing identities stopped heartbeating long ago: their own dead reports are fine)
    let (vc, _) = poll_in(&c_dir, env);
    assert!(
        liveness_events(&vc, "agent-alive").is_empty()
            && !vc["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["body"].as_str().is_some_and(|b| b.contains(&y))),
        "{vc}"
    );
    // the returning agent was told once that it was reported dead (the signed report to all)
    assert!(
        vy["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "report" && e["body"].as_str().unwrap().contains("agent-dead:")),
        "{vy}"
    );
    // leave no leftovers that go stale under the next test's short interval
    for d in [&y_dir, &z_dir] {
        let _ = run_env(d, &["stop"], env);
    }
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_a_late_agent_causes_no_traffic_and_wakes_nobody_but_a_real_message_does() {
    let _g = it!();
    setup_coordinator();
    let env: &[(&str, &str)] = &[("CREWBOARD_TEST_HEARTBEAT_INTERVAL", "10s")];
    let c_dir = dir("c");
    let (x_dir, x) = fresh_dev("kimi");
    let c = Cleanup::new();
    let keep = || {
        for d in [dir("a"), dir("b")] {
            expect_ok(&run_env(&d, &["heartbeat"], env), "heartbeat");
        }
    };
    // Leftover agents of earlier tests go stale and dead on the 3 s interval. Let the
    // coordinator report them first (each once), so that only this test's traffic is left.
    for _ in 0..3 {
        keep();
        let _ = poll_in(&x_dir, env); // x reads its backlog (approve etc.) and heartbeats
        let _ = poll_in(&c_dir, env);
        // real wait: stale heartbeats age on the board's own clock (an ignored sandbox test)
        std::thread::sleep(std::time::Duration::from_secs(8));
    }
    keep();
    let _ = poll_in(&x_dir, env);
    let (v, code) = poll_in(&c_dir, env);
    assert_eq!(code, 1, "the drain left events: {v}");
    // x is silent for 3 intervals and a little more: late (dead only after 5). The coordinator
    // sends nothing and says nothing.
    // real wait: lateness is judged by the board's own timestamps (an ignored sandbox test)
    std::thread::sleep(std::time::Duration::from_secs(32));
    keep();
    let o = run_env(&c_dir, &["poll", "--quiet"], env);
    if o.code != 1 {
        let (v, _) = poll_in(&c_dir, env);
        panic!("a late agent is no event for the coordinator: {v}");
    }
    let traffic = || count_registry(&c, |b| b.contains("kind=ping") || b.contains("kind=pong"));
    assert_eq!(traffic(), 0, "a late agent is not pinged");
    // x's watch heartbeats; it prints no event block and exits 1
    let w = run_env(
        &x_dir,
        &["watch", "--timeout", "4s", "--interval", "1s"],
        env,
    );
    assert_eq!(w.code, 1, "{}{}", w.stdout, w.stderr);
    assert!(!w.stdout.contains("== ["), "no event block: {}", w.stdout);
    assert_eq!(traffic(), 0, "no pong either");
    // the coordinator: still no event, quiet or not
    // (the commands are slow against a 3 s interval: x heartbeats again, as its loop would)
    keep();
    expect_ok(&run_env(&x_dir, &["heartbeat"], env), "heartbeat of x");
    let o = run_env(&c_dir, &["poll", "--quiet"], env);
    assert_eq!(o.code, 1, "{}{}", o.stdout, o.stderr);
    let (v, code) = poll_in(&c_dir, env);
    assert_eq!(code, 1, "{v}");
    assert_eq!(traffic(), 0);
    // the same period plus ONE real message: exit 0, exactly that event
    expect_ok(
        &run_env(
            &c_dir,
            &[
                "say",
                &format!("{REPO}#2"),
                "--to",
                &x,
                "--kind",
                "info",
                "--body",
                "real message",
            ],
            env,
        ),
        "say",
    );
    let (v, code) = poll_in(&x_dir, env);
    assert_eq!(code, 0, "{v}");
    let evs = v["events"].as_array().unwrap();
    assert_eq!(evs.len(), 1, "exactly one event: {v}");
    assert_eq!(evs[0]["body"], "real message");
    let _ = run_env(&x_dir, &["stop"], env);
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_done_judges_commit_statuses_both_ways() {
    let _g = it!();
    setup_coordinator();
    let (c_dir, b_dir) = (dir("c"), dir("b"));
    drain("b");
    let mut c = Cleanup::new();
    // a ticket that does NOT declare 'CI: none'
    let t = new_ticket(&mut c, "IT statuses", false, true);
    let ts = t.to_string();
    let o = run(&c_dir, &["assign", &ts, "--to", b_name()]);
    expect_ok(&o, "assign");
    let item = item_of(&mut c, &o);
    expect_ok(&run(&b_dir, &["take", &ts]), "take");
    let (pr, _n, sha, _branch) = make_pr(&mut c, t, b_name());
    let done = || run(&b_dir, &["done", &ts, "--pr", &pr, "--sha", &sha]);
    let post = |state: &str| {
        c.client
            .post(
                &format!("repos/{REPO}/statuses/{sha}"),
                &json!({"state": state, "context": "it/ci", "description": "integration test"}),
            )
            .unwrap();
    };
    let o = done();
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("evidence-ci-missing"),
        "no CI at all: {}",
        o.stderr
    );
    post("pending");
    let o = done();
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("pending"), "{}", o.stderr);
    post("failure");
    let o = done();
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(
        o.stderr.contains("evidence-ci-not-green") && o.stderr.contains("it/ci"),
        "{}",
        o.stderr
    );
    assert_eq!(
        field_of(&item, "Status").as_deref(),
        Some("In Progress"),
        "a refusal changes nothing"
    );
    post("success");
    let o = done();
    expect_ok(&o, "the latest status is success");
    assert!(
        o.stdout
            .contains("ci-sources: workflow-runs, check-runs, statuses"),
        "{}",
        o.stdout
    );
    assert_eq!(field_of(&item, "Status").as_deref(), Some("Review"));
    assert!(field_of(&item, "Evidence").unwrap().contains("ci-sources="));
    // verify judges it again, by the same statuses
    let o = run(&c_dir, &["verify", &ts]);
    expect_ok(&o, "verify");
    assert!(o.stdout.contains("verify PASS"), "{}", o.stdout);
    // a failing status after the done makes the verify fail (control: it can say no)
    post("failure");
    let o = run(&c_dir, &["verify", &ts]);
    expect_ok(&o, "verify runs");
    assert!(
        o.stdout.contains("verify FAIL") && o.stdout.contains("it/ci"),
        "{}",
        o.stdout
    );
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_mine_lists_owned_work_and_take_say_refresh_a_due_heartbeat_only() {
    let _g = it!();
    setup_coordinator();
    let (c_dir, a_dir, b_dir) = (dir("c"), dir("a"), dir("b"));
    drain("b");
    let mut c = Cleanup::new();
    let t = new_ticket(&mut c, "IT mine and heartbeat", true, true);
    let ts = t.to_string();
    let o = run(&c_dir, &argv(&["assign", &ts, "--to", b_name()]));
    expect_ok(&o, "assign");
    item_of(&mut c, &o);

    // mine: B owns a Ready ticket -> exit 0, listed; --quiet prints nothing
    let o = run(&b_dir, &argv(&["mine"]));
    assert_eq!(o.code, 0, "{}{}", o.stdout, o.stderr);
    assert!(
        o.stdout.contains(&format!("#{t}")) && o.stdout.contains("Ready"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("order"), "{}", o.stdout);
    let q = run(&b_dir, &argv(&["mine", "--quiet"]));
    assert_eq!((q.code, q.stdout.as_str()), (0, ""), "{}", q.stderr);
    // control: the coordinator owns no ticket -> exit 1, silent with --quiet
    let q = run(&c_dir, &argv(&["mine", "--quiet"]));
    assert_eq!((q.code, q.stdout.as_str()), (1, ""), "{}", q.stderr);
    let _ = &a_dir;

    // heartbeat: with a 2 s interval, a heartbeat that is 3 s old is due, and `take` writes one
    let b_item = item_id_of(&b_dir);
    expect_ok(&run(&b_dir, &argv(&["heartbeat"])), "heartbeat");
    let hb0 = epoch_of(&b_item, "Heartbeat");
    // real wait: the heartbeat age is judged by the board's own timestamps (an ignored sandbox test)
    std::thread::sleep(std::time::Duration::from_secs(3));
    let env: &[(&str, &str)] = &[("CREWBOARD_TEST_HEARTBEAT_INTERVAL", "2s")];
    expect_ok(&run_env(&b_dir, &argv(&["take", &ts]), env), "take");
    let hb1 = epoch_of(&b_item, "Heartbeat");
    assert!(
        hb1 > hb0,
        "take refreshed the due heartbeat: {hb0} -> {hb1}"
    );
    // not due: with the default 10 minute interval `say` writes no heartbeat
    expect_ok(
        &run(
            &b_dir,
            &argv(&[
                "say",
                &ts,
                "--to",
                "coordinator",
                "--kind",
                "info",
                "--body",
                "working",
            ]),
        ),
        "say",
    );
    assert_eq!(
        epoch_of(&b_item, "Heartbeat"),
        hb1,
        "a fresh heartbeat is not rewritten"
    );
    // due again: say after 3 s with the 2 s interval refreshes it
    // real wait: the heartbeat age is judged by the board's own timestamps (an ignored sandbox test)
    std::thread::sleep(std::time::Duration::from_secs(3));
    expect_ok(
        &run_env(
            &b_dir,
            &argv(&[
                "say",
                &ts,
                "--to",
                "coordinator",
                "--kind",
                "info",
                "--body",
                "still working",
            ]),
            env,
        ),
        "say when due",
    );
    assert!(epoch_of(&b_item, "Heartbeat") > hb1);
    // B is In Progress: mine lists it first
    let o = run(&b_dir, &argv(&["mine"]));
    assert!(o.stdout.contains("In Progress"), "{}", o.stdout);
    // release B for the other tests
    expect_ok(&run(&b_dir, &argv(&["pause"])), "pause");
    expect_ok(&run(&b_dir, &argv(&["resume"])), "resume");
}

#[test]
#[ignore = "needs CREWBOARD_IT=1 and a sandbox board"]
fn it_a_stale_coordinator_gets_one_owner_alert_that_wakes_nobody() {
    // spec/ci#ci-and-crewboard-only-commits. Interval 4 s: the coordinator is stale 12 s
    // after its last heartbeat.
    let _g = it!();
    setup_coordinator();
    let env: &[(&str, &str)] = &[("CREWBOARD_TEST_HEARTBEAT_INTERVAL", "4s")];
    let c_dir = dir("c");
    let (x_dir, _x) = fresh_dev("kimi");
    let (y_dir, _y) = fresh_dev("hermes");
    let c = Cleanup::new();
    let alerts = |c: &Cleanup| {
        count_registry(c, |b| {
            b.contains("to=owner kind=info")
                && b.contains("late: heartbeat ")
                && b.contains(&format!("late-key: heartbeat:{}:", c_name()))
        })
    };
    let keep = || {
        for d in [&x_dir, &y_dir, &dir("a"), &dir("b")] {
            expect_ok(&run_env(d, &["heartbeat"], env), "heartbeat");
        }
    };
    // a live coordinator: no alert, whoever polls
    expect_ok(&run_env(&c_dir, &["heartbeat"], env), "heartbeat of c");
    keep();
    let _ = poll_in(&x_dir, env);
    let _ = poll_in(&y_dir, env);
    // The calls above can take longer than 3 intervals (12 s) on a slow or busy host. The
    // coordinator is then stale and a dev poll has already raised the alert for that outage
    // (key = coordinator + last heartbeat). The dedup is right to send no second alert for
    // it. So count first, then write a fresh heartbeat: the silence below is a NEW outage
    // with its own key, and exactly its alerts are counted in `after`.
    let before = alerts(&c);
    expect_ok(
        &run_env(&c_dir, &["heartbeat"], env),
        "heartbeat of c, start of the outage",
    );
    // the coordinator goes silent; the dev agents keep their heartbeats and poll
    // real wait: the heartbeat age is judged by the board's own timestamps (an ignored sandbox test)
    std::thread::sleep(std::time::Duration::from_secs(16));
    keep();
    // the alert is no event: the quiet poll has the same exit code as without a coordinator problem
    let quiet_x = run_env(&x_dir, &["poll", "--quiet"], env);
    let (vx, code_x) = poll_in(&x_dir, env);
    assert_eq!(
        quiet_x.code, code_x,
        "the alert does not change the exit code: {vx}"
    );
    assert!(
        !vx["events"].as_array().unwrap().iter().any(|e| e["body"]
            .as_str()
            .is_some_and(|b| b.contains("late: heartbeat "))),
        "the alert is no event: {vx}"
    );
    let (_vy, _) = poll_in(&y_dir, env);
    let after = alerts(&c);
    assert!(
        after > before && after <= before + 2,
        "one alert (a race may add one): {before} -> {after}"
    );
    // more polls of every dev agent add nothing
    for _ in 0..2 {
        keep();
        let _ = poll_in(&x_dir, env);
        let _ = poll_in(&y_dir, env);
    }
    assert_eq!(alerts(&c), after, "no alert per poll");
    // the coordinator comes back and dies again: a second alert
    // The rule: no second alert for an outage that has one. The poll below can start more than
    // 3 intervals (12 s) after this heartbeat on a slow host or a slow GitHub; then the
    // coordinator is stale again, and that NEW outage (new key) gets its own alert, which is
    // right. So the check is per outage key, not a total count.
    let keys_before = outage_keys(&c);
    assert!(
        !keys_before.is_empty(),
        "the alert above is found by its outage key (the check can fire)"
    );
    expect_ok(&run_env(&c_dir, &["heartbeat"], env), "c is back");
    let _ = poll_in(&x_dir, env);
    let keys_after = outage_keys(&c);
    for k in &keys_before {
        let n = |v: &[String]| v.iter().filter(|x| *x == k).count();
        assert_eq!(
            n(&keys_after),
            n(&keys_before),
            "a live coordinator clears the state, no second alert for outage {k}: {keys_after:?}"
        );
    }
    // real wait: the heartbeat age is judged by the board's own timestamps (an ignored sandbox test)
    std::thread::sleep(std::time::Duration::from_secs(16));
    keep();
    let _ = poll_in(&x_dir, env);
    let _ = poll_in(&y_dir, env);
    assert!(alerts(&c) > after, "a later stale period gets a new alert");
    expect_ok(
        &run_env(&c_dir, &["heartbeat"], env),
        "c is back at the end",
    );
    for d in [&x_dir, &y_dir] {
        let _ = run_env(d, &["stop"], env);
    }
}
