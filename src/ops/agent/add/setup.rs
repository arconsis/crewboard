//! Steps 1 to 4 of `agent add`: the agent dir, `join`, the `gh` shim and the crew id.

use super::capture::{STEP_LIMIT, exit_of, run_captured};
use super::output::{Fail, Ok_, Run, env_fix};
use super::plan::{Plan, basename, seam_bin, strip_prefix};
use crate::agent_env;
use crate::base::ignore::best_effort_cleanup;
use crate::error::CbError;
use crate::fsutil::set_mode;
use crate::github::api::users;
use crate::ops::agent::absolute;
use crate::ops::agent::shim::{
    find_real_gh, is_current_stub_for, make_empty_gh_config, read_crew_id, scrub_tokens, sq,
    stub_text, valid_login, write_stub,
};
use crate::ops::{Global, join};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Step 1: the agent dir (mode 700) and the link `agent.env` to the env file. The env file is
/// checked by the loader of the agent env, the one place that knows the rules.
pub(super) fn step_dir(p: &Plan, run: &mut Run) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String| Fail {
        step: 1,
        name: "agent dir",
        what,
        fix,
        code: 4,
    };
    let env = &p.env_file;
    let loaded = agent_env::load(&p.agent_dir, Some(env)).map_err(|e| {
        let m = strip_prefix(&e);
        let fix = env_fix(&m, env);
        fail(m, fix)
    })?;
    run.secrets = loaded.token_values();

    let dir = &p.agent_dir;
    let link = dir.join("agent.env");
    if dir.is_dir() && is_linked(dir, env) {
        set_mode(dir, 0o700).map_err(|e| {
            fail(
                format!("cannot set the mode of {}: {e}", dir.display()),
                format!("chmod 700 {}", dir.display()),
            )
        })?;
        run.log = Some(dir.join("bootstrap.log"));
        return Ok(Ok_ {
            already: true,
            text: dir.display().to_string(),
        });
    }
    make_agent_dir(dir, env).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            fail(
                format!("{} exists and is not a link", link.display()),
                format!(
                    "move {} away (it may hold a token), then run this command again",
                    link.display()
                ),
            )
        } else {
            fail(
                format!(
                    "cannot make {} or link {}: {e}",
                    dir.display(),
                    link.display()
                ),
                format!(
                    "check the permissions of {}",
                    dir.parent().unwrap_or(dir).display()
                ),
            )
        }
    })?;
    run.log = Some(dir.join("bootstrap.log"));
    Ok(Ok_ {
        already: false,
        text: format!("{} -> {}", link.display(), env.display()),
    })
}

/// Does `<dir>/agent.env` link to the env file?
pub(in crate::ops::agent) fn is_linked(dir: &Path, env: &Path) -> bool {
    std::fs::read_link(dir.join("agent.env")).is_ok_and(|t| t == env)
}

/// Make the agent dir (mode 700) and link `agent.env` to the env file: a temp link in the dir,
/// renamed over the old link, so the link is never missing. A regular file at `agent.env` is not
/// replaced (it may be the only copy of a token): that is `AlreadyExists`.
pub(in crate::ops::agent) fn make_agent_dir(dir: &Path, env: &Path) -> std::io::Result<()> {
    let link = dir.join("agent.env");
    if let Ok(m) = link.symlink_metadata()
        && !m.file_type().is_symlink()
    {
        return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists));
    }
    std::fs::create_dir_all(dir)?;
    set_mode(dir, 0o700)?;
    let tmp = dir.join(format!(".agent.env.tmp.{}", std::process::id()));
    best_effort_cleanup(std::fs::remove_file(&tmp));
    std::os::unix::fs::symlink(env, &tmp)?;
    std::fs::rename(&tmp, &link).inspect_err(|_| {
        best_effort_cleanup(std::fs::remove_file(&tmp));
    })
}

/// The agent config in the dir that names this agent. A file that `load` would refuse counts too.
fn find_conf(dir: &Path, name: &str) -> Option<PathBuf> {
    let want = format!("name = \"{name}\"");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|f| {
            let n = basename(f);
            n.ends_with(".toml") && !n.ends_with(".board.toml") && f.is_file()
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .find(|f| std::fs::read_to_string(f).is_ok_and(|t| t.lines().any(|l| l == want)))
}

/// A child process that has the env file of the agent (`apply` removes the token variables of the
/// caller and sets the keys of the file).
fn agent_command(p: &Plan, bin: &Path) -> Result<Command, String> {
    let env = agent_env::load(&p.agent_dir, None).map_err(|e| strip_prefix(&e))?;
    let mut c = Command::new(bin);
    env.apply(&mut c);
    Ok(c)
}

/// Step 2: `join`. An agent config in the dir that names this agent means joined.
pub(super) fn step_join(p: &Plan, run: &mut Run) -> Result<(Ok_, Option<PathBuf>, bool), Fail> {
    if let Some(c) = find_conf(&p.agent_dir, &p.name) {
        let text = basename(&c);
        return Ok((
            Ok_ {
                already: true,
                text,
            },
            Some(c),
            false,
        ));
    }
    let fail = |what: String, code: u8| {
        Fail {
        step: 2,
        name: "join",
        what,
        fix: "read the lines above. Before the init pull request is merged, the repo has no .crewboard.toml; then pass --board-file <initiative>.board.toml (the board file of the owner)".into(),
        code,
    }
    };
    let mut title = format!("join --agent {} --runtime {}", p.name, p.runtime.bin());
    if let Some(m) = &p.model {
        title.push_str(&format!(" --model {m}"));
    }
    let result: Result<String, (String, u8)> = if let Some(bin) = seam_bin(p) {
        let mut c = agent_command(p, Path::new(bin)).map_err(|m| fail(m, 4))?;
        // the old name on purpose: the seam binary may be an older one, and every version accepts it
        c.arg("--config-dir").arg(&p.agent_dir).args([
            "join",
            "--name",
            &p.name,
            "--runtime",
            p.runtime.bin(),
        ]);
        if let Some(m) = &p.model {
            c.args(["--model", m]);
        }
        match &p.board_file {
            Some(f) => c.arg("--initiative-file").arg(f),
            None => c.args(["--repo", &p.repo]),
        };
        let cap = run_captured(c, STEP_LIMIT);
        if cap.code == 0 && !cap.timed_out {
            Ok(cap.text)
        } else {
            run.record(&title, &cap.text);
            Err((
                format!("crewboard join exit {}", cap.code),
                exit_of(cap.code),
            ))
        }
    } else {
        let g = Global {
            config_dir: Some(p.agent_dir.clone()),
            initiative: None,
            initiative_file: p.board_file.clone(),
            insecure_one_account: false,
        };
        let repo = p.board_file.is_none().then_some(p.repo.as_str());
        match join::run(
            &g,
            join::JoinArgs {
                name: &p.name,
                runtime: p.runtime,
                model: p.model.as_deref(),
                host: None,
                repo,
                tools: &[],
            },
        ) {
            Ok(t) => Ok(t),
            Err(e) => {
                run.record(&title, &e.to_string());
                Err((
                    format!("crewboard join exit {}", e.exit().code()),
                    e.exit().code(),
                ))
            }
        }
    };
    match result {
        Ok(text) => {
            run.record(&title, &text);
            Ok((
                Ok_ {
                    already: false,
                    text: format!("join request posted for {}", p.name),
                },
                find_conf(&p.agent_dir, &p.name),
                true,
            ))
        }
        Err((what, code)) => Err(fail(what, code)),
    }
}

/// Step 3: the `gh` stub and the empty `gh` config dir.
pub(super) fn step_shim(p: &Plan, run: &mut Run) -> Result<Ok_, Fail> {
    let fail = |what: String, fix: String| Fail {
        step: 3,
        name: "gh shim",
        what,
        fix,
        code: 4,
    };
    let bin_dir = p.agent_dir.join("bin");
    let shim = bin_dir.join("gh");
    let empty = p.agent_dir.join("gh-empty-config");
    let env_link = p.agent_dir.join("agent.env");
    let is_exec = std::fs::metadata(&shim).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    if is_exec
        && empty.is_dir()
        && std::fs::read_to_string(&shim).is_ok_and(|t| is_current_stub_for(&t, &env_link))
    {
        return Ok(Ok_ {
            already: true,
            text: shim.display().to_string(),
        });
    }
    let real =
        find_real_gh(&std::env::var_os("PATH").unwrap_or_default(), &bin_dir).ok_or_else(|| {
            fail(
                "no real gh found on the PATH".into(),
                "install gh (https://cli.github.com) and run this command again".into(),
            )
        })?;
    let me = std::env::current_exe().map_err(|e| {
        fail(
            format!("cannot find the crewboard binary: {e}"),
            "run crewboard by its full path".into(),
        )
    })?;
    let text = stub_text(&p.agent_dir, &absolute(&me), &env_link, &real);
    write_stub(&bin_dir, &text).map_err(|e| {
        fail(
            strip_prefix(&e),
            format!("move {} away and run this command again", shim.display()),
        )
    })?;
    make_empty_gh_config(&empty).map_err(|e| {
        fail(
            strip_prefix(&e),
            format!("check the permissions of {}", p.agent_dir.display()),
        )
    })?;
    run.record(
        "gh shim",
        &format!("wrote {} (real gh: {})", shim.display(), real.display()),
    );
    Ok(Ok_ {
        already: false,
        text: shim.display().to_string(),
    })
}

/// Ask GitHub through the stub (the test seam): `gh api <path> --jq <jq>`, the output trimmed.
fn gh_api(run: &mut Run, shim: &Path, path: &str, jq: &str) -> Result<String, (String, u8)> {
    let mut c = Command::new(shim);
    c.args(["api", path, "--jq", jq]);
    scrub_tokens(&mut c);
    let cap = run_captured(c, STEP_LIMIT);
    run.record(&format!("gh api {path}"), &cap.text);
    if cap.code == 0 && !cap.timed_out {
        Ok(cap.text.split_whitespace().collect())
    } else {
        Err((
            format!("gh api {path} exit {}", cap.code),
            exit_of(cap.code),
        ))
    }
}

/// Step 4: the crew id, for the noreply e-mail `<id>+<login>@users.noreply.github.com`.
pub(super) fn step_crew_id(
    p: &Plan,
    run: &mut Run,
    shim: &Path,
) -> Result<(Ok_, String, String), Fail> {
    let fail = |what: String, fix: String, code: u8| Fail {
        step: 4,
        name: "crew id",
        what,
        fix,
        code,
    };
    let cached = read_crew_id(&p.agent_dir);
    if let Some((l, i)) = &cached
        && p.crew_login.as_ref().is_none_or(|w| w == l)
    {
        return Ok((
            Ok_ {
                already: true,
                text: format!("{l}, id {i}"),
            },
            l.clone(),
            i.clone(),
        ));
    }
    let env = p.env_file.display();
    let via_seam = seam_bin(p).is_some();
    let client = if via_seam {
        None
    } else {
        Some(crate::ops::new_client().map_err(|e| {
            fail(
                strip_prefix(&e),
                format!("check the token in {env}"),
                e.exit().code(),
            )
        })?)
    };
    let login = match &p.crew_login {
        Some(l) => l.clone(),
        None => {
            let got = match &client {
                Some(c) => c.viewer_login().map_err(|e| {
                    let e = CbError::from(e);
                    run.record("GET /user", &e.to_string());
                    (format!("GET /user: {e}"), e.exit().code())
                }),
                None => gh_api(run, shim, "user", ".login"),
            };
            got.map_err(|(what, code)| {
                fail(
                    what,
                    format!(
                        "check the token in {env}: crewboard agent exec --agent-dir {} -- gh auth status",
                        sq(&p.agent_dir.display().to_string())
                    ),
                    code,
                )
            })?
        }
    };
    if !valid_login(&login) {
        return Err(fail(
            format!("no numeric id for {login}: it is not a GitHub login"),
            format!("check --crew-login and the token in {env}"),
            4,
        ));
    }
    let id_path = format!("users/{login}");
    let got = match &client {
        Some(c) => users::get(c, &login)
            .map_err(|e| {
                let e = CbError::from(e);
                run.record(&format!("GET /{id_path}"), &e.to_string());
                (format!("GET /{id_path}: {e}"), e.exit().code())
            })
            .map(|u| u.id.map(|n| n.to_string()).unwrap_or_default()),
        None => gh_api(run, shim, &id_path, ".id"),
    };
    let id = got.map_err(|(what, code)| {
        fail(
            what,
            format!("check --crew-login and the token in {env}"),
            code,
        )
    })?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(fail(
            format!("no numeric id for {login}"),
            format!("check --crew-login and the token in {env}"),
            4,
        ));
    }
    std::fs::write(p.agent_dir.join("crew-id"), format!("{login} {id}\n")).map_err(|e| {
        fail(
            format!("cannot write {}/crew-id: {e}", p.agent_dir.display()),
            format!("check the permissions of {}", p.agent_dir.display()),
            4,
        )
    })?;
    Ok((
        Ok_ {
            already: false,
            text: format!("{login}, id {id}"),
        },
        login,
        id,
    ))
}
