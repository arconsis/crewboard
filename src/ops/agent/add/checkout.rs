//! Steps 5 and 6 of `agent add`: the work dir and the git identity.

use super::capture::{CLONE_LIMIT, STEP_LIMIT, exit_of, run_captured};
use super::output::{Fail, Ok_, Run};
use super::plan::{Plan, origin_matches};
use crate::ops::agent::shim::{Identity, helper_value, is_checkout, scrub_tokens};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git_command(args: &[&str], cwd: Option<&Path>) -> Command {
    let mut c = Command::new("git");
    c.args(args);
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    c.env("GIT_TERMINAL_PROMPT", "0");
    scrub_tokens(&mut c);
    c
}

/// Step 5: the work dir is a checkout of the repo, or it is cloned through the stub.
pub(super) fn step_work_dir(
    p: &Plan,
    run: &mut Run,
    shim: &Path,
    login: &str,
) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String, code: u8| Fail {
        step: 5,
        name: "work dir",
        what,
        fix,
        code,
    };
    let wd = &p.work_dir;
    if is_checkout(wd) {
        let url = git_origin(wd);
        return if origin_matches(url.as_deref().unwrap_or(""), &p.repo) {
            Ok(Ok_ {
                already: true,
                text: wd.display().to_string(),
            })
        } else {
            Err(fail(
                format!(
                    "{} is a checkout of {}, not of {}",
                    wd.display(),
                    url.as_deref()
                        .filter(|u| !u.is_empty())
                        .unwrap_or("no origin"),
                    p.repo
                ),
                "pass another --work-dir".into(),
                4,
            ))
        };
    }
    if std::fs::read_dir(wd).is_ok_and(|mut d| d.next().is_some()) {
        return Err(fail(
            format!(
                "{} exists, is not empty and is not a git checkout",
                wd.display()
            ),
            format!("pass another --work-dir, or move {} away", wd.display()),
            4,
        ));
    }
    let made = match ensure_parent(wd) {
        Ok(m) => m,
        Err(what) => return Err(fail(what, "pass another --work-dir".into(), 4)),
    };
    let helper = format!("credential.helper={}", helper_value(shim));
    let url = format!("https://github.com/{}", p.repo);
    let wd_s = wd.display().to_string();
    let c = git_command(
        &[
            "-c",
            "credential.helper=",
            "-c",
            &helper,
            "clone",
            &url,
            &wd_s,
        ],
        None,
    );
    let cap = run_captured(c, CLONE_LIMIT);
    run.record(&format!("git clone {url} {wd_s}"), &cap.text);
    if cap.code != 0 || cap.timed_out {
        return Err(fail(
            format!("git clone exit {}", cap.code),
            format!(
                "check that {login} can read {} (the owner adds it with the Write role)",
                p.repo
            ),
            exit_of(cap.code),
        ));
    }
    let created = made.map_or(String::new(), |m| format!("created {}; ", m.display()));
    Ok(Ok_ {
        already: false,
        text: format!("{created}cloned {url} to {wd_s}"),
    })
}

/// The parent of the work dir: made when it is missing (like `mkdir -p`, mode 755), and then
/// returned as the top directory that was created. A path that is clearly wrong stops the step:
/// the nearest directory that exists is a file, or it is inside a git checkout (the repo of
/// another agent) other than a checkout of the home directory itself.
fn ensure_parent(wd: &Path) -> Result<Option<PathBuf>, String> {
    let Some(parent) = wd.parent() else {
        return Ok(None);
    };
    if parent.is_dir() {
        return Ok(None);
    }
    let Some(base) = parent.ancestors().find(|a| a.exists()) else {
        return Err(format!("no directory of {} exists", parent.display()));
    };
    if !base.is_dir() {
        return Err(format!(
            "{} is not a directory, so --work-dir cannot be created under it",
            base.display()
        ));
    }
    let top = git_command(&["rev-parse", "--show-toplevel"], Some(base));
    let cap = run_captured(top, STEP_LIMIT);
    let toplevel = cap.text.trim();
    if cap.code == 0 && !toplevel.is_empty() && !is_home(Path::new(toplevel)) {
        return Err(format!(
            "the parent of --work-dir does not exist and {} is inside the git checkout {toplevel}: that is probably the wrong path",
            base.display()
        ));
    }
    let first = parent
        .ancestors()
        .take_while(|a| !a.exists())
        .last()
        .unwrap_or(parent);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o755)
        .create(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    Ok(Some(first.to_path_buf()))
}

/// Is `p` the home directory of the user (a dotfiles checkout there is not "another agent")?
fn is_home(p: &Path) -> bool {
    let canon = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf());
    std::env::var_os("HOME").is_some_and(|h| canon(Path::new(&h)) == canon(p))
}

fn git_origin(wd: &Path) -> Option<String> {
    let c = git_command(&["remote", "get-url", "origin"], Some(wd));
    let cap = run_captured(c, STEP_LIMIT);
    (cap.code == 0).then(|| cap.text.trim().to_string())
}

/// Step 6: the git identity of the agent account in the work dir.
pub(super) fn step_git(
    p: &Plan,
    run: &mut Run,
    shim: &Path,
    login: &str,
    email: &str,
) -> Result<Ok_, Fail> {
    let id = Identity {
        repo: &p.work_dir,
        shim,
        login,
        email,
    };
    let text = format!("{login} <{email}>");
    if id.is_set() {
        return Ok(Ok_ {
            already: true,
            text,
        });
    }
    match id.set() {
        Ok(out) => {
            run.record("git config", &out);
            Ok(Ok_ {
                already: false,
                text,
            })
        }
        Err(e) => {
            run.record("git config", &e);
            Err(Fail {
                step: 6,
                name: "git config",
                what: "git config failed".into(),
                fix: "read the lines above and run this command again".into(),
                code: 4,
            })
        }
    }
}
