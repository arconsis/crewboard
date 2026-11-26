//! `verify --run-check <name>`: the tool runs named commands of AUTHORITY.md on a clean checkout
//! of the head that the `done` names, and writes the local-run line of the verify itself
//! (spec/ci#checks-that-the-tool-runs). A model types no result.
//!
//! What makes the result a machine result:
//!
//! - the command lines come from `checks` of AUTHORITY.md on the default branch, never from the
//!   PR, the ticket or the command line;
//! - they run without a shell, in a new work tree of exactly the commit of the evidence;
//! - the line starts with [`TOOL_RUN_PREFIX`], which a typed `--local-run` text must not use.
//!
//! What it is not: a sandbox. The commands are code of the PR and run as the user of this host.
//! The environment is cut down to a short list, so a token in a variable does not reach them;
//! the files of the user stay readable.

use super::local_gate::{TOOL_RUN_PREFIX, local_gate_not_enabled};
use super::verify::{latest_evidence, load_verify};
use crate::authority::Check;
use crate::base::ids;
use crate::error::{CbError, CbResult};
use crate::ops::AgentCtx;
use crate::protocol::acts::is_local_gate;
use crate::rules::Refusal;
use crate::rules::evidence::parse_pr_url;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The variables that a check gets. Everything else of the environment is removed.
const KEPT_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "TMPDIR",
    "TERM",
    "SHELL",
    // Windows
    "SystemRoot",
    "SystemDrive",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "PATHEXT",
    "ComSpec",
];

/// The longest text of the line (the limit of every local-run text).
const MAX_TEXT: usize = 400;

/// One check that ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ran {
    pub name: String,
    pub rc: i32,
    pub secs: u64,
}

/// The checks that `names` ask for, in that order. Pure.
pub(super) fn pick<'a>(checks: &'a [Check], names: &[String]) -> Result<Vec<&'a Check>, Refusal> {
    let mut out: Vec<&Check> = Vec::new();
    for n in names {
        let Some(c) = checks.iter().find(|c| &c.name == n) else {
            let known = if checks.is_empty() {
                "AUTHORITY.md on the default branch has no `checks` (the owner adds them)"
                    .to_string()
            } else {
                format!(
                    "AUTHORITY.md has: {}",
                    checks
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            return Err(Refusal::new(
                ids::CHECK_UNKNOWN,
                format!("--run-check {n}: no check has this name. {known}. Nothing was run"),
            ));
        };
        if !out.iter().any(|o| o.name == c.name) {
            out.push(c);
        }
    }
    Ok(out)
}

/// The local-run line of a tool run in which every check passed. Pure. It fits [`MAX_TEXT`]: a
/// line that is too long names the count of the checks, not each one.
pub(super) fn tool_text(host: &str, sha: &str, ran: &[Ran], log_dir: &Path) -> String {
    let short: String = sha.chars().take(12).collect();
    let each = ran
        .iter()
        .map(|r| format!("{} rc={} {}s", r.name, r.rc, r.secs))
        .collect::<Vec<_>>()
        .join(", ");
    let tail = format!("; clean checkout of {short}; logs {}", log_dir.display());
    let text = format!("{TOOL_RUN_PREFIX}{host}: {each}{tail}");
    if text.len() <= MAX_TEXT {
        return text;
    }
    let text = format!("{TOOL_RUN_PREFIX}{host}: {} checks rc=0{tail}", ran.len());
    if text.len() <= MAX_TEXT {
        return text;
    }
    format!(
        "{TOOL_RUN_PREFIX}{host}: {} checks rc=0; clean checkout of {short}",
        ran.len()
    )
}

/// Does the URL of `origin` name `repo` (`owner/name`)? Pure. Any letter case; a final `.git`
/// or `/` is ignored.
pub(super) fn origin_names_repo(url: &str, repo: &str) -> bool {
    let u = url.trim().trim_end_matches('/');
    let u = u.strip_suffix(".git").unwrap_or(u).to_ascii_lowercase();
    let r = repo.to_ascii_lowercase();
    u.strip_suffix(&r)
        .is_some_and(|before| before.ends_with('/') || before.ends_with(':'))
}

/// The full SHA of a commit: 40 hex digits. It goes to `git` as an argument.
fn is_full_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Run `git -C <repo> <args>`; the trimmed standard output, or the error text.
fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("git cannot start: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            err.trim().lines().last().unwrap_or("no message")
        ))
    }
}

/// The checkout that the checks run in: a new work tree of `sha` under `log_dir`. Removed when
/// the value is dropped; the logs stay.
struct Tree<'a> {
    repo: &'a Path,
    dir: PathBuf,
}

impl Drop for Tree<'_> {
    fn drop(&mut self) {
        let dir = self.dir.to_string_lossy().into_owned();
        if let Err(e) = git(self.repo, &["worktree", "remove", "--force", &dir]) {
            eprintln!("the work tree {dir} was not removed: {e}");
        }
    }
}

/// Make the clean checkout of `sha` of PR `number` from `origin` of `repo`.
fn checkout<'a>(
    repo: &'a Path,
    number: u64,
    sha: &str,
    log_dir: &Path,
) -> Result<Tree<'a>, String> {
    // The commit of a PR from a branch of the repo is fetched by its SHA; the head ref of the PR
    // is the second way (a fork, or a server that does not give a commit by SHA).
    // Whether the commit is here is decided after the fetches, not by their exit codes.
    if git(repo, &["fetch", "--quiet", "--no-tags", "origin", sha]).is_err() {
        let pr_ref = format!("refs/pull/{number}/head");
        git(repo, &["fetch", "--quiet", "--no-tags", "origin", &pr_ref]).ok();
    }
    let commit = format!("{sha}^{{commit}}");
    git(repo, &["rev-parse", "--verify", "--quiet", &commit]).map_err(|_| {
        format!(
            "the commit {sha} of the evidence is not in origin of {}",
            repo.display()
        )
    })?;
    let dir = log_dir.join("tree");
    let text = dir.to_string_lossy().into_owned();
    git(
        repo,
        &["worktree", "add", "--detach", "--quiet", &text, sha],
    )?;
    Ok(Tree { repo, dir })
}

/// A new private directory for the logs of this run under `root`.
fn make_log_dir(root: &Path, key: &str, sha: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let safe: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let short: String = sha.chars().take(12).collect();
    let nanos = crate::clock::now_nanos();
    for attempt in 0..100u32 {
        let d = root.join(format!("{safe}-{short}-{}", nanos.wrapping_add(attempt)));
        match crate::ops::repeat_cmd::private_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("cannot create {}: {e}", d.display())),
        }
    }
    Err(format!(
        "cannot create a log directory in {}",
        root.display()
    ))
}

/// The words of the command with the program made absolute when it is a path in the tree
/// (`./check.sh`, `tools/x.py`): a relative program is then the one of the checkout on every
/// OS. A bare name (`cargo`) is looked up in `PATH`.
fn argv_in(tree: &Path, argv: &[String]) -> Vec<String> {
    let mut out = argv.to_vec();
    let p = Path::new(&argv[0]);
    if p.is_relative() && p.components().count() > 1 {
        out[0] = tree.join(p).to_string_lossy().into_owned();
    }
    out
}

/// Run one check in `tree`: its exit code and its seconds. Output goes to `log`.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
fn run_one(tree: &Path, c: &Check, log: &Path) -> Ran {
    let start = std::time::Instant::now();
    let rc = crate::ops::repeat_cmd::run_logged(&argv_in(tree, &c.argv), tree, log, &|cmd| {
        cmd.env_clear();
        for k in KEPT_ENV {
            if let Some(v) = std::env::var_os(k) {
                cmd.env(k, v);
            }
        }
    });
    Ran {
        name: c.name.clone(),
        rc,
        secs: start.elapsed().as_secs(),
    }
}

/// The warning for a check whose program is a file of the checkout that no protected path
/// covers: the PR can change what the check does.
fn unprotected_program(ac: &AgentCtx, extra: &[String], c: &Check) -> Option<String> {
    let p = Path::new(&c.argv[0]);
    if !(p.is_relative() && p.components().count() > 1) {
        return None;
    }
    let rel = c.argv[0].trim_start_matches("./").replace('\\', "/");
    let guard = crate::rules::protected::Protect::new(&ac.ctx.cfg.authority, extra);
    crate::rules::protected::protected_hits(std::slice::from_ref(&rel), guard)
        .is_empty()
        .then(|| {
            format!(
                "warning: check {}: {rel} is a file of the PR and is not in protected_paths of AUTHORITY.md, so a PR can change what this check does",
                c.name
            )
        })
}

/// `verify --run-check`: run the checks `names` for the latest `done` of the ticket. `Ok`: every
/// check exited 0, and the text is the local-run line for the verify. A check that fails is the
/// refusal `check-failed`; nothing is written then. `repo_dir`: a checkout of the repo of the PR.
/// `log_root`: where the logs of all runs are kept.
pub fn run_checks(
    ac: &AgentCtx,
    issue_arg: &str,
    names: &[String],
    repo_dir: &Path,
    log_root: &Path,
) -> CbResult<String> {
    let l = load_verify(ac, issue_arg)?;
    let key = l.t.key();
    let (_, _, ev) = latest_evidence(&key, l.t.agent(), &l.t.msgs)?;
    let auth = crate::ctx::load_authority(&ac.ctx)?;
    if !auth.ci_mode.is_local() {
        return Err(local_gate_not_enabled().into());
    }
    if !is_local_gate(&ev) {
        return Err(Refusal::new(
            ids::LOCAL_GATE_UNEXPECTED,
            "--run-check is for a local-gate done; the latest done of the dev has CI evidence. Run verify without it",
        )
        .into());
    }
    let checks = pick(&auth.checks, names)?;
    let pr = parse_pr_url(&ev.pr)?;
    if !is_full_sha(&ev.sha) {
        return Err(CbError::config(format!(
            "the evidence of {key} has no full commit SHA ({}), so no checkout is possible",
            ev.sha
        )));
    }
    let top = git(repo_dir, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        CbError::config(format!(
            "--run-check needs a git checkout of {}: {} is none (pass --work-dir)",
            pr.repo,
            repo_dir.display()
        ))
    })?;
    let top = PathBuf::from(top);
    let origin = git(&top, &["remote", "get-url", "origin"]).map_err(CbError::config)?;
    if !origin_names_repo(&origin, &pr.repo) {
        return Err(CbError::config(format!(
            "--run-check needs a checkout of {}: origin of {} is another repo",
            pr.repo,
            top.display()
        )));
    }
    for c in &checks {
        if let Some(w) = unprotected_program(ac, &auth.protected_paths, c) {
            eprintln!("{w}");
        }
    }
    let log_dir = make_log_dir(log_root, &key, &ev.sha).map_err(CbError::config)?;
    let tree = checkout(&top, pr.number, &ev.sha, &log_dir).map_err(CbError::config)?;
    let mut ran: Vec<Ran> = Vec::new();
    for c in &checks {
        let log = log_dir.join(format!("{}.log", c.name));
        eprintln!("check {}: running on {} ...", c.name, &ev.sha[..12]);
        let r = run_one(&tree.dir, c, &log);
        eprintln!("check {}: rc={} after {}s", r.name, r.rc, r.secs);
        if r.rc != 0 {
            let tail = log_tail(&log);
            return Err(Refusal::new(
                ids::CHECK_FAILED,
                format!(
                    "check {} exited {} on a clean checkout of {} ({}s). Log: {}{tail}\nNothing was written. To send the ticket back: crewboard verify {issue_arg} --fail --body \"<what failed>\"",
                    r.name,
                    r.rc,
                    ev.sha,
                    r.secs,
                    log.display()
                ),
            )
            .into());
        }
        ran.push(r);
    }
    drop(tree);
    Ok(tool_text(ac.name.host(), &ev.sha, &ran, &log_dir))
}

/// The last lines of a log, for the refusal text (the log may hold text of the PR: it is shown
/// as data, indented, and cut).
fn log_tail(log: &Path) -> String {
    let Ok(text) = std::fs::read(log) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text.lines().rev().take(8).collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("\nLast lines of the log:");
    for l in lines.iter().rev() {
        let cut: String = l.chars().take(200).collect();
        out.push_str("\n  | ");
        out.push_str(&cut);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, line: &str) -> Check {
        Check {
            name: name.into(),
            argv: line.split(' ').map(str::to_string).collect(),
        }
    }

    #[test]
    fn pick_keeps_the_order_of_the_names_and_refuses_an_unknown_name() {
        let all = [check("app", "./check.sh all"), check("lint", "cargo fmt")];
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = pick(&all, &names(&["lint", "app", "lint"])).unwrap();
        assert_eq!(
            got.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["lint", "app"]
        );
        let r = pick(&all, &names(&["app", "tests"])).unwrap_err();
        assert_eq!(r.rule, "check-unknown");
        assert!(
            r.message.contains("AUTHORITY.md has: app, lint"),
            "{}",
            r.message
        );
        let r = pick(&[], &names(&["app"])).unwrap_err();
        assert!(r.message.contains("has no `checks`"), "{}", r.message);
    }

    #[test]
    fn the_text_names_each_check_and_fits_the_limit() {
        let ran = |n: usize| -> Vec<Ran> {
            (0..n)
                .map(|i| Ran {
                    name: format!("check-number-{i}"),
                    rc: 0,
                    secs: 7,
                })
                .collect()
        };
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let t = tool_text("host-a", sha, &ran(2), Path::new("/s/runs/x"));
        assert!(t.starts_with("tool-run host-a: check-number-0 rc=0 7s, check-number-1 rc=0 7s; clean checkout of 0123456789ab; logs "), "{t}");
        let long = tool_text("host-a", sha, &ran(40), Path::new("/s/runs/x"));
        assert!(long.len() <= MAX_TEXT, "{}", long.len());
        assert!(long.contains("40 checks rc=0"), "{long}");
        let deep = "/x".repeat(300);
        let no_logs = tool_text("host-a", sha, &ran(40), Path::new(&deep));
        assert!(
            no_logs.len() <= MAX_TEXT && !no_logs.contains("logs"),
            "{no_logs}"
        );
    }

    #[test]
    fn origin_must_name_the_repo_of_the_pr() {
        for url in [
            "https://github.com/Acme/App.git",
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app/",
            "/tmp/x/acme/app",
        ] {
            assert!(origin_names_repo(url, "acme/app"), "{url}");
        }
        for url in [
            "https://github.com/acme/app2.git",
            "https://github.com/other/app.git",
            "https://github.com/xacme/app",
            "acme/app",
        ] {
            assert!(!origin_names_repo(url, "acme/app"), "{url}");
        }
    }

    #[test]
    fn a_program_that_is_a_path_runs_from_the_checkout_and_a_bare_name_from_path() {
        let tree = Path::new("/t/tree");
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            argv_in(tree, &v(&["./check.sh", "all"]))[0],
            tree.join("./check.sh").to_string_lossy()
        );
        assert_eq!(
            argv_in(tree, &v(&["tools/x.py"]))[0],
            tree.join("tools/x.py").to_string_lossy()
        );
        assert_eq!(argv_in(tree, &v(&["cargo", "test"])), v(&["cargo", "test"]));
        assert!(is_full_sha("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_full_sha("--upload-pack=x") && !is_full_sha("0123abc"));
    }
}
