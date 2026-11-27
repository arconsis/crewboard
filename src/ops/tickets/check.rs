//! `crewboard check`: run checks of AUTHORITY.md on a clean checkout of a commit of the caller
//! (spec/ci#checks-before-a-delivery). It is the run of `verify --run-check` without a ticket:
//! the same command lines, without a shell, in a new work tree of exactly one commit, with the
//! same cut-down environment and the same time limit. It writes nothing: no comment, no board
//! field, no state. The logs stay in the state dir of crewboard.
//!
//! Two uses. A dev runs it before `done`, so that a fault of a machine check costs minutes and
//! not a round through Review. The owner runs it with `--authority-file` to try the `checks` of
//! a draft of AUTHORITY.md before that file is on the default branch.
//!
//! It is backpressure, not proof: the caller runs it on its own host, so a pass is no evidence
//! of a `done` or a `verify`.

use super::run_checks::{self, Job};
use crate::authority::{Authority, Check};
use crate::error::{CbError, CbResult};
use crate::ops::Global;
use crate::rules::Refusal;
use std::path::Path;

/// The name of the authority file when no board config is read (`--authority-file`).
const DEFAULT_AUTHORITY: &str = "AUTHORITY.md";

/// What `crewboard check` asks for.
pub struct CheckArgs<'a> {
    /// The names of the checks, as typed. None: every check of the file, in its order.
    pub names: &'a [String],
    /// The commit to test, as git names it (`HEAD`, a branch, a SHA).
    pub rev: &'a str,
    /// `--must-fail-at`: the commit on which each named check must fail first
    /// (spec/ci#a-check-that-must-be-red-first).
    pub must_fail_at: Option<&'a str>,
    /// A git checkout of the repo.
    pub repo_dir: &'a Path,
    /// Where the logs of all runs are kept.
    pub log_root: &'a Path,
    /// Read `checks` from this file and not from the default branch.
    pub authority_file: Option<&'a Path>,
    /// A scope binds the caller (a headless config dir, or a wake that a scheduler started):
    /// no check runs then whose command line names a file outside `protected_paths`, as for
    /// `tick`.
    pub unattended: bool,
}

/// The authority that the checks come from, the path of the authority file in the repo, and
/// the words that name the source in the report.
fn source(g: &Global, file: Option<&Path>) -> CbResult<(Authority, String, String)> {
    let Some(file) = file else {
        let ctx = crate::ops::open_reader(g)?;
        let auth = crate::ctx::load_authority(&ctx)?;
        let name = ctx.cfg.authority.clone();
        let from = format!("{name} on the default branch of {}", ctx.main_repo());
        return Ok((auth, name, from));
    };
    let text = std::fs::read_to_string(file)
        .map_err(|e| CbError::config(format!("cannot read {}: {e}", file.display())))?;
    let auth = crate::authority::parse(&text)?.ok_or_else(|| {
        CbError::config(format!(
            "{} has no front matter, so it has no `checks`",
            file.display()
        ))
    })?;
    Ok((
        auth,
        DEFAULT_AUTHORITY.to_string(),
        format!("the file {}", file.display()),
    ))
}

/// The checks to run: the named ones, or all. Pure.
fn wanted<'a>(
    checks: &'a [Check],
    names: &[String],
    from: &str,
) -> Result<Vec<&'a Check>, Refusal> {
    if checks.is_empty() {
        return Err(Refusal::new(
            crate::base::ids::CHECK_UNKNOWN,
            format!("{from} has no `checks` (the owner adds them). Nothing was run"),
        ));
    }
    if names.is_empty() {
        return Ok(checks.iter().collect());
    }
    // the words of `pick` name `--run-check` and the default branch: say what this caller typed
    run_checks::pick(checks, names).map_err(|_| {
        let unknown: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| !checks.iter().any(|c| c.name == *n))
            .collect();
        let known: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        Refusal::new(
            crate::base::ids::CHECK_UNKNOWN,
            format!(
                "no check has the name {}. {from} has: {}. Nothing was run",
                unknown.join(", "),
                known.join(", ")
            ),
        )
    })
}

/// The names of `check_env` that this process does not have. Pure but for the environment.
fn unset_env(names: &[String]) -> Vec<&str> {
    names
        .iter()
        .map(String::as_str)
        .filter(|k| std::env::var_os(k).is_none())
        .collect()
}

/// The report of a run in which every check passed, after the red run when there was one. Pure.
fn report(
    ran: &[run_checks::Ran],
    red: Option<&run_checks::Red>,
    sha: &str,
    from: &str,
    log_dir: &Path,
) -> String {
    let twelve = |s: &str| s.chars().take(12).collect::<String>();
    let mut out = String::new();
    let mut first = String::new();
    if let Some(red) = red {
        let at = twelve(&red.sha);
        for r in &red.ran {
            out.push_str(&format!(
                "red   {}: rc={} {}s at {at}\n",
                r.name, r.rc, r.secs
            ));
        }
        first = format!(" and failed first on {at}");
    }
    for r in ran {
        out.push_str(&format!("ok    {}: rc={} {}s\n", r.name, r.rc, r.secs));
    }
    let short = twelve(sha);
    let n = ran.len();
    out.push_str(&format!(
        "{n} check{} passed on a clean checkout of {short}{first}. Checks from {from}. Logs: {}. Nothing was written\n",
        if n == 1 { "" } else { "s" },
        log_dir.display()
    ));
    out
}

/// `crewboard check [NAME]...`: the report of the run. A check that fails is the refusal
/// `check-failed`.
pub fn check(g: &Global, a: &CheckArgs<'_>) -> CbResult<String> {
    let top = run_checks::top_of(a.repo_dir).map_err(|_| {
        CbError::config(format!(
            "check needs a git checkout, and {} is none (pass --work-dir)",
            a.repo_dir.display()
        ))
    })?;
    // the commit first: a typo in `--rev` costs no request
    let sha = run_checks::commit_of(&top, a.rev).map_err(CbError::usage)?;
    // `--must-fail-at`: also before any request. The run resolves the same commit again.
    let red = match a.must_fail_at {
        Some(_) if a.names.is_empty() => {
            return Err(CbError::usage(
                "--must-fail-at: name the checks that must be red (crewboard check <name> --must-fail-at <rev>)",
            ));
        }
        Some(rev) => Some(run_checks::red_commit(&top, rev, &sha)?),
        None => None,
    };
    let (auth, authority, from) = source(g, a.authority_file)?;
    let checks = wanted(&auth.checks, a.names, &from)?;
    // only for the commit of HEAD: for another commit the open changes say nothing
    let head = run_checks::commit_of(&top, "HEAD").ok();
    if head.as_deref() == Some(sha.as_str()) && run_checks::has_open_changes(&top) {
        eprintln!(
            "note: {} has changes that no commit holds. The checks run on the commit {} without them",
            top.display(),
            &sha[..12]
        );
    }
    let unset = unset_env(&auth.check_env);
    if !unset.is_empty() {
        eprintln!(
            "note: check_env names {}, and this process does not have {}",
            unset.join(", "),
            if unset.len() == 1 { "it" } else { "them" }
        );
    }
    let job = Job {
        key: "check",
        sha: &sha,
        authority: &authority,
        protected_paths: &auth.protected_paths,
        env: &auth.check_env,
        timeout: auth.check_timeout,
        unattended: a.unattended,
        fetch_pr: None,
        after_fail: "",
        must_fail_at: red.as_deref(),
    };
    let passed = run_checks::run_job(&top, &job, &checks, a.log_root)?;
    Ok(report(
        &passed.ran,
        passed.red.as_ref(),
        &sha,
        &from,
        &passed.log_dir,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authority::Check;

    fn c(name: &str) -> Check {
        Check {
            name: name.to_string(),
            argv: vec!["true".to_string()],
        }
    }

    #[test]
    fn no_name_means_every_check_in_the_order_of_the_file() {
        let all = [c("b"), c("a")];
        let got = wanted(&all, &[], "the file x").unwrap();
        assert_eq!(
            got.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        let one = wanted(&all, &["a".to_string()], "the file x").unwrap();
        assert_eq!(one.len(), 1);
    }

    #[test]
    fn a_file_without_checks_and_an_unknown_name_are_refused() {
        let r = wanted(&[], &[], "the file x").unwrap_err();
        assert_eq!(r.rule, crate::base::ids::CHECK_UNKNOWN);
        assert!(
            r.message.starts_with("the file x has no `checks`"),
            "{}",
            r.message
        );
        let r = wanted(&[c("a")], &["z".to_string()], "the file x").unwrap_err();
        assert_eq!(r.rule, crate::base::ids::CHECK_UNKNOWN);
        assert_eq!(
            r.message,
            "no check has the name z. the file x has: a. Nothing was run"
        );
        let r = wanted(&[], &["z".to_string()], "the file x").unwrap_err();
        assert!(
            r.message.starts_with("the file x has no `checks`"),
            "{}",
            r.message
        );
    }

    #[test]
    fn the_report_names_each_check_the_commit_the_source_and_the_logs() {
        let ran = [run_checks::Ran {
            name: "fmt".to_string(),
            rc: 0,
            secs: 2,
            end: run_checks::End::Exited,
        }];
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let t = report(&ran, None, sha, "the file A.md", Path::new("/s/logs"));
        assert_eq!(
            t,
            "ok    fmt: rc=0 2s\n1 check passed on a clean checkout of 0123456789ab. Checks from the file A.md. Logs: /s/logs. Nothing was written\n"
        );
        // with a red run: its lines come first, and the last line names both commits
        let red = run_checks::Red {
            sha: "fedcba9876543210fedcba9876543210fedcba98".to_string(),
            ran: vec![run_checks::Ran {
                name: "fmt".to_string(),
                rc: 1,
                secs: 3,
                end: run_checks::End::Exited,
            }],
        };
        let t = report(&ran, Some(&red), sha, "the file A.md", Path::new("/s/logs"));
        assert_eq!(
            t,
            "red   fmt: rc=1 3s at fedcba987654\nok    fmt: rc=0 2s\n1 check passed on a clean checkout of 0123456789ab and failed first on fedcba987654. Checks from the file A.md. Logs: /s/logs. Nothing was written\n"
        );
    }
}
