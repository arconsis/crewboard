//! `crewboard agent shim`, and the parts that `agent add` shares with it: the generated `gh`
//! stub, the real `gh`, and the git identity of a work dir.
//!
//! The stub is a three-line `sh` file. It runs the real `gh` through `crewboard agent with-env`,
//! so the crew token is loaded for that one call and the model never sees it:
//!
//! ```sh
//! #!/bin/sh
//! # crewboard gh shim for <agent dir>: runs the real gh as the crew account.
//! exec '<crewboard>' agent with-env --env-file '<env file>' -- '<real gh>' "$@"
//! ```

use super::{absolute, missing_dir};
use crate::error::{CbError, CbResult};
use crate::fsutil::{Mode, is_executable, set_mode, write_atomic};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The start of the second line of every stub that crewboard wrote, in this and in the older
/// form. A file whose second line starts like this may be replaced.
const STUB_MARK: &str = "# crewboard gh shim";

/// Quote one word for a POSIX shell: `'a'\''b'`.
pub(super) fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The text of the stub. Pure.
pub fn stub_text(agent_dir: &Path, crewboard: &Path, env_file: &Path, real_gh: &Path) -> String {
    format!(
        "#!/bin/sh\n{STUB_MARK} for {}: runs the real gh as the crew account.\nexec {} agent with-env --env-file {} -- {} \"$@\"\n",
        agent_dir.display().to_string().replace('\n', " "),
        sq(&crewboard.display().to_string()),
        sq(&env_file.display().to_string()),
        sq(&real_gh.display().to_string()),
    )
}

/// Did crewboard write this file? The second line decides.
pub fn is_crewboard_stub(text: &str) -> bool {
    text.lines()
        .nth(1)
        .is_some_and(|l| l.starts_with(STUB_MARK))
}

/// Is this a stub of the current form that loads `env_file`? The older form is
/// a crewboard stub, but not this one: `agent add` writes it again.
pub fn is_current_stub_for(text: &str, env_file: &Path) -> bool {
    is_crewboard_stub(text)
        && text.lines().nth(2).is_some_and(|l| {
            l.contains(" agent with-env --env-file ")
                && l.contains(&sq(&env_file.display().to_string()))
        })
}

/// Is this file a stub that crewboard wrote (the stub of any agent)? Only its first lines are read.
pub(super) fn file_is_crewboard_stub(p: &Path) -> bool {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(p)
        .and_then(|f| f.take(512).read_to_end(&mut buf))
        .is_ok_and(|_| is_crewboard_stub(&String::from_utf8_lossy(&buf)))
}

/// Remove the token variables of the caller from a child that has no use for them.
pub(super) fn scrub_tokens(cmd: &mut Command) {
    for v in crate::agent_env::TOKEN_VARS {
        cmd.env_remove(v);
    }
}

/// The agent dir of `agent shim`: `--agent-dir`, else `--config-dir`, else a non-empty
/// `$CREWBOARD_CONFIG_DIR`. The default config dir of the owner is never an agent dir, so there
/// is no other fallback.
pub(super) fn shim_agent_dir(
    flag: Option<&Path>,
    config_dir: Option<&Path>,
    env: Option<std::ffi::OsString>,
) -> CbResult<PathBuf> {
    flag.map(Path::to_path_buf)
        .or_else(|| config_dir.map(Path::to_path_buf))
        .or_else(|| env.filter(|e| !e.is_empty()).map(PathBuf::from))
        .map(|d| absolute(&d))
        .ok_or_else(|| {
            CbError::config(
                "agent shim: no agent dir; pass --agent-dir <dir>, or --config-dir <dir>, or set $CREWBOARD_CONFIG_DIR (the default config dir is not an agent dir)",
            )
        })
}

/// The first `gh` on `path` that is an executable file and is not in `shim_dir` (the shim dir
/// comes first on the PATH of an agent, so a plain lookup would find the stub again). A path
/// that is a link to the stub does not count either, and neither does the stub of another agent
/// (a file that crewboard wrote). The result is absolute.
pub fn find_real_gh(path: &OsStr, shim_dir: &Path) -> Option<PathBuf> {
    let shim_dir = absolute(shim_dir);
    let stub = std::fs::canonicalize(shim_dir.join("gh")).ok();
    std::env::split_paths(path)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| absolute(&d))
        .filter(|d| *d != shim_dir)
        .map(|d| d.join("gh"))
        .find(|p| {
            is_executable(p)
                && (stub.is_none() || std::fs::canonicalize(p).ok() != stub)
                && !file_is_crewboard_stub(p)
        })
}

/// Make `<bin dir>/gh` from `text`. It refuses a file that is not a crewboard stub. The file is
/// written to a temp file in the same dir and renamed, with mode 755.
pub fn write_stub(bin_dir: &Path, text: &str) -> CbResult<()> {
    let target = bin_dir.join("gh");
    match std::fs::read(&target) {
        Ok(old) if !is_crewboard_stub(&String::from_utf8_lossy(&old)) => {
            return Err(CbError::config(format!(
                "{} exists and is not a crewboard gh stub; move it away and run this command again",
                target.display()
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(CbError::config(format!(
                "cannot read {}: {e}",
                target.display()
            )));
        }
    }
    std::fs::create_dir_all(bin_dir)
        .map_err(|e| CbError::config(format!("cannot make {}: {e}", bin_dir.display())))?;
    write_atomic(&target, text.as_bytes(), Mode::Exact(0o755))
        .map_err(|e| CbError::config(format!("cannot write {}: {e}", target.display())))
}

/// Make the empty `gh` config dir (mode 700). A `gh` that bypasses the stub finds no login there.
pub fn make_empty_gh_config(dir: &Path) -> CbResult<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CbError::config(format!("cannot make {}: {e}", dir.display())))?;
    set_mode(dir, 0o700)
        .map_err(|e| CbError::config(format!("cannot set the mode of {}: {e}", dir.display())))
}

// ---------------------------------------------------------------------------------------------
// the git identity of a work dir

/// The value of `credential.helper` for the stub. A path with a character that a shell would
/// split or expand is quoted.
pub fn helper_value(shim: &Path) -> String {
    let s = shim.display().to_string();
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@:,=".contains(c));
    if plain {
        format!("!{s} auth git-credential")
    } else {
        format!("!{} auth git-credential", sq(&s))
    }
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let mut c = Command::new("git");
    c.arg("-C").arg(repo).args(args);
    scrub_tokens(&mut c);
    let out = c.output().map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Is `repo` a git checkout (a plain clone or a linked worktree)?
pub fn is_checkout(repo: &Path) -> bool {
    git(repo, &["rev-parse", "--git-dir"]).is_ok()
}

/// Is `repo` a linked worktree? Its git dir is not the common git dir.
fn is_linked_worktree(repo: &Path) -> Result<bool, String> {
    let resolve = |arg: &str| -> Result<PathBuf, String> {
        let p = git(repo, &["rev-parse", arg])?;
        let p = PathBuf::from(p);
        let full = if p.is_absolute() { p } else { repo.join(p) };
        std::fs::canonicalize(&full).map_err(|e| format!("cannot read {}: {e}", full.display()))
    };
    Ok(resolve("--git-dir")? != resolve("--git-common-dir")?)
}

/// The identity of the crew account in one checkout.
pub struct Identity<'a> {
    pub repo: &'a Path,
    pub shim: &'a Path,
    pub login: &'a str,
    pub email: &'a str,
}

impl Identity<'_> {
    /// Are all four settings in place already?
    pub fn is_set(&self) -> bool {
        let get = |args: &[&str]| git(self.repo, args).unwrap_or_default();
        get(&["config", "user.name"]) == self.login
            && get(&["config", "user.email"]) == self.email
            && get(&["config", "--get-all", "credential.helper"])
                .lines()
                .last()
                == Some(helper_value(self.shim).as_str())
            && get(&["config", "--get", "url.https://github.com/.insteadOf"]) == "git@github.com:"
    }

    /// Set the identity in this checkout only (the owner's checkout keeps its own): no SSH key of
    /// the owner (`insteadOf`), the credential helper reset and then the stub, name and e-mail.
    /// A linked worktree gets the settings with `extensions.worktreeConfig`, worktree-local.
    pub fn set(&self) -> Result<String, String> {
        let scope = if is_linked_worktree(self.repo)? {
            git(self.repo, &["config", "extensions.worktreeConfig", "true"])?;
            "--worktree"
        } else {
            "--local"
        };
        let helper = helper_value(self.shim);
        for args in [
            vec![
                "config",
                scope,
                "url.https://github.com/.insteadOf",
                "git@github.com:",
            ],
            // an empty value resets the helpers of the system and the user; then ours
            vec!["config", scope, "--replace-all", "credential.helper", ""],
            vec![
                "config",
                scope,
                "--add",
                "credential.helper",
                helper.as_str(),
            ],
            vec!["config", scope, "user.name", self.login],
            vec!["config", scope, "user.email", self.email],
        ] {
            git(self.repo, &args)?;
        }
        Ok(format!(
            "git config ({scope}) set in {}: insteadOf, credential.helper, user.name, user.email",
            self.repo.display()
        ))
    }
}

/// The e-mail that GitHub accepts for a crew account: `<id>+<login>@users.noreply.github.com`.
pub fn noreply_email(login: &str, id: &str) -> String {
    format!("{id}+{login}@users.noreply.github.com")
}

/// The cache file `crew-id` of an agent dir: `<login> <id>`.
pub fn read_crew_id(agent_dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(agent_dir.join("crew-id")).ok()?;
    let mut w = text.split_whitespace();
    let (login, id) = (w.next()?, w.next()?);
    id.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| (login.to_string(), id.to_string()))
}

/// A GitHub login: letters, digits and single hyphens. It goes into a request path.
pub fn valid_login(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 39
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !s.starts_with('-')
}

// ---------------------------------------------------------------------------------------------
// the command

/// The options of `agent shim`.
#[derive(Debug, Default)]
pub struct ShimArgs {
    pub agent_dir: Option<PathBuf>,
    pub work_dir: Option<PathBuf>,
    pub real_gh: Option<PathBuf>,
    /// The global `--env-file`: the stub loads this file. Default: `<agent dir>/agent.env`.
    pub env_file: Option<PathBuf>,
    pub crew_login: Option<String>,
    /// The dir of the stub, the e-mail of the git identity, and git identity only.
    pub bin_dir: Option<PathBuf>,
    pub email: Option<String>,
    pub only_git: bool,
}

/// One line of the report of a step.
fn line(n: usize, total: usize, name: &str, already: bool, text: &str) {
    println!(
        "agent shim: [{n}/{total}] {name}: {}{text}",
        if already { "already done " } else { "done " }
    );
}

/// `agent shim`: make the stub (and, with `--work-dir`, the git identity of that checkout). It
/// prints one line for each step.
pub fn run_shim(a: &ShimArgs, config_dir_flag: Option<&Path>) -> CbResult<()> {
    let agent_dir = shim_agent_dir(
        a.agent_dir.as_deref(),
        config_dir_flag,
        std::env::var_os("CREWBOARD_CONFIG_DIR"),
    )?;
    let bin_dir = a
        .bin_dir
        .as_deref()
        .map(absolute)
        .unwrap_or_else(|| agent_dir.join("bin"));
    let total = 1 + usize::from(a.work_dir.is_some());
    let shim = bin_dir.join("gh");
    let empty = bin_dir
        .parent()
        .unwrap_or(&agent_dir)
        .join("gh-empty-config");
    let mut n = 0;

    if !a.only_git {
        // the compat option `--bin-dir` makes the dirs, as the old script did
        if a.bin_dir.is_none() && !agent_dir.is_dir() {
            return Err(missing_dir(&agent_dir));
        }
        let env_file = match &a.env_file {
            Some(f) => absolute(f),
            None => agent_dir.join("agent.env"),
        };
        if !env_file.is_file() {
            return Err(CbError::config(format!(
                "env file not found: {} (pass --env-file <file>, or link it with `crewboard agent add`)",
                env_file.display()
            )));
        }
        let real_gh = match &a.real_gh {
            Some(p) => absolute(p),
            None => find_real_gh(
                &std::env::var_os("PATH").unwrap_or_default(),
                &bin_dir,
            )
            .ok_or_else(|| {
                CbError::config("no real gh found on the PATH; install gh (https://cli.github.com) or pass --real-gh <path>")
            })?,
        };
        if file_is_crewboard_stub(&real_gh) {
            return Err(CbError::config(format!(
                "--real-gh {} is a crewboard gh stub (of another agent), not the real gh",
                real_gh.display()
            )));
        }
        if !is_executable(&real_gh) {
            return Err(CbError::config(format!(
                "no real gh found: {} is not an executable file; pass --real-gh <path>",
                real_gh.display()
            )));
        }
        if std::fs::canonicalize(&real_gh).ok() == std::fs::canonicalize(&shim).ok()
            && shim.exists()
        {
            return Err(CbError::config("--real-gh must not be the shim itself"));
        }
        let crewboard = std::env::current_exe()
            .map_err(|e| CbError::config(format!("cannot find the crewboard binary: {e}")))?;
        let crewboard = absolute(&crewboard);
        let text = stub_text(&agent_dir, &crewboard, &env_file, &real_gh);
        let same = std::fs::read_to_string(&shim).is_ok_and(|t| t == text) && empty.is_dir();
        if !same {
            write_stub(&bin_dir, &text)?;
            make_empty_gh_config(&empty)?;
        }
        n += 1;
        line(
            n,
            total,
            "gh shim",
            same,
            &format!("({}; real gh: {})", shim.display(), real_gh.display()),
        );
    }

    if let Some(wd) = &a.work_dir {
        let wd = absolute(wd);
        if !is_checkout(&wd) {
            return Err(CbError::config(format!(
                "not a git checkout: {}",
                wd.display()
            )));
        }
        let cached = read_crew_id(&agent_dir);
        let login = match (&a.crew_login, &cached) {
            (Some(l), _) => l.clone(),
            (None, Some((l, _))) => l.clone(),
            (None, None) => {
                return Err(CbError::config(format!(
                    "no crew login: pass --crew-login <login>, or run `crewboard agent add` (it writes {}/crew-id)",
                    agent_dir.display()
                )));
            }
        };
        if !valid_login(&login) {
            return Err(CbError::config(format!("'{login}' is not a GitHub login")));
        }
        let email = match (&a.email, &cached) {
            (Some(e), _) => e.clone(),
            (None, Some((l, id))) if *l == login => noreply_email(&login, id),
            _ => {
                return Err(CbError::config(format!(
                    "no id for the crew login {login} in {}/crew-id; run `crewboard agent add` first",
                    agent_dir.display()
                )));
            }
        };
        let id = Identity {
            repo: &wd,
            shim: &shim,
            login: &login,
            email: &email,
        };
        let already = id.is_set();
        if !already {
            id.set().map_err(CbError::config)?;
        }
        n += 1;
        line(
            n,
            total,
            "git config",
            already,
            &format!("({login} <{email}>)"),
        );
    }

    // `agent exec` finds the stub only in `<agent dir>/bin`
    if !a.only_git && bin_dir == agent_dir.join("bin") {
        println!(
            "agent shim: next step: start the runtime with the shim first on its PATH: crewboard agent exec --agent-dir {} -- <runtime>",
            sq(&agent_dir.display().to_string())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn text() -> String {
        stub_text(
            Path::new("/a/dev1"),
            Path::new("/usr/local/bin/crewboard"),
            Path::new("/a/dev1/agent.env"),
            Path::new("/usr/bin/gh"),
        )
    }

    #[test]
    fn the_stub_has_three_lines() {
        assert_eq!(
            text(),
            "#!/bin/sh\n# crewboard gh shim for /a/dev1: runs the real gh as the crew account.\nexec '/usr/local/bin/crewboard' agent with-env --env-file '/a/dev1/agent.env' -- '/usr/bin/gh' \"$@\"\n"
        );
    }

    #[test]
    fn a_quote_in_a_path_is_closed_and_reopened() {
        assert_eq!(sq("a'b"), "'a'\\''b'");
        let t = stub_text(
            Path::new("/a"),
            Path::new("/x y/o'k/crewboard"),
            Path::new("/e"),
            Path::new("/g"),
        );
        assert!(t.contains("exec '/x y/o'\\''k/crewboard' agent"), "{t}");
    }

    #[test]
    fn only_a_crewboard_stub_counts_as_one() {
        assert!(is_crewboard_stub(&text()));
        // the older stub is one too
        assert!(is_crewboard_stub(
            "#!/bin/bash\n# crewboard gh shim: run the real gh as the crew account (made by an older crewboard).\nexec bash x\n"
        ));
        assert!(!is_crewboard_stub("#!/bin/sh\nexec /usr/bin/gh \"$@\"\n"));
        assert!(!is_crewboard_stub(""));
        assert!(!is_crewboard_stub("#!/bin/sh\n"));
    }

    #[test]
    fn the_current_form_must_name_the_env_file() {
        let env = Path::new("/a/dev1/agent.env");
        assert!(is_current_stub_for(&text(), env));
        assert!(!is_current_stub_for(&text(), Path::new("/other.env")));
        let old = "#!/bin/bash\n# crewboard gh shim: run the real gh (made by an older crewboard).\nexec bash w --env-file /a/dev1/agent.env -- gh\n";
        assert!(!is_current_stub_for(old, env));
    }

    fn fake_gh(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join("gh");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_real_gh_is_the_first_one_that_is_not_the_shim() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("agent/bin");
        let shim = fake_gh(&bin);
        let real = fake_gh(&t.path().join("real"));
        let other = fake_gh(&t.path().join("other"));
        let path = std::env::join_paths([
            bin.as_path(),
            t.path().join("real").as_path(),
            t.path().join("other").as_path(),
        ])
        .unwrap();
        assert_eq!(find_real_gh(&path, &bin), Some(real.clone()));
        // twin: with the shim dir not named, the shim is found first
        assert_eq!(find_real_gh(&path, &t.path().join("elsewhere")), Some(shim));
        // a PATH without any other gh finds none
        let only = std::env::join_paths([bin.as_path()]).unwrap();
        assert_eq!(find_real_gh(&only, &bin), None);
        let _ = other;
    }

    #[test]
    fn a_link_to_the_stub_in_another_dir_is_not_the_real_gh() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("agent/bin");
        let shim = fake_gh(&bin);
        let elsewhere = t.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&shim, elsewhere.join("gh")).unwrap();
        let path = std::env::join_paths([elsewhere.as_path()]).unwrap();
        assert_eq!(find_real_gh(&path, &bin), None);
    }

    #[test]
    fn write_stub_makes_mode_755_and_replaces_only_a_crewboard_stub() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("bin");
        write_stub(&bin, &text()).unwrap();
        let mode = std::fs::metadata(bin.join("gh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o755);
        assert_eq!(std::fs::read_to_string(bin.join("gh")).unwrap(), text());
        // a second write replaces it, and leaves no temp file
        write_stub(&bin, &text()).unwrap();
        let names: Vec<_> = std::fs::read_dir(&bin)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("gh")]);
        // a file that is not a crewboard stub stays as it is, and the refusal names it
        std::fs::write(bin.join("gh"), "#!/bin/sh\necho mine\n").unwrap();
        let e = write_stub(&bin, &text()).unwrap_err().to_string();
        assert!(e.contains("is not a crewboard gh stub"), "{e}");
        assert_eq!(
            std::fs::read_to_string(bin.join("gh")).unwrap(),
            "#!/bin/sh\necho mine\n"
        );
    }

    #[test]
    fn the_credential_helper_is_quoted_only_when_a_shell_needs_it() {
        assert_eq!(
            helper_value(Path::new("/a/dev1/bin/gh")),
            "!/a/dev1/bin/gh auth git-credential"
        );
        assert_eq!(
            helper_value(Path::new("/a b/bin/gh")),
            "!'/a b/bin/gh' auth git-credential"
        );
    }

    #[test]
    fn a_login_is_checked_before_it_goes_into_a_path() {
        assert!(valid_login("crew-bot"));
        assert!(valid_login("A1"));
        assert!(!valid_login(""));
        assert!(!valid_login("-x"));
        assert!(!valid_login("a/b"));
        assert!(!valid_login("a b"));
        assert!(!valid_login("../x"));
    }

    #[test]
    fn the_crew_id_cache_needs_a_login_and_a_number() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read_crew_id(t.path()), None);
        std::fs::write(t.path().join("crew-id"), "crew-bot 4242\n").unwrap();
        assert_eq!(
            read_crew_id(t.path()),
            Some(("crew-bot".into(), "4242".into()))
        );
        std::fs::write(t.path().join("crew-id"), "crew-bot abc\n").unwrap();
        assert_eq!(read_crew_id(t.path()), None);
        std::fs::write(t.path().join("crew-id"), "crew-bot\n").unwrap();
        assert_eq!(read_crew_id(t.path()), None);
    }

    #[test]
    fn the_noreply_email_has_the_id_first() {
        assert_eq!(
            noreply_email("crew-bot", "4242"),
            "4242+crew-bot@users.noreply.github.com"
        );
    }

    #[test]
    fn the_agent_dir_of_shim_comes_from_a_flag_or_the_env_and_never_from_the_default() {
        let os = |s: &str| Some(std::ffi::OsString::from(s));
        let d = |a: Option<&str>, b: Option<&str>, e| {
            shim_agent_dir(a.map(Path::new), b.map(Path::new), e)
        };
        assert_eq!(
            d(Some("/a"), Some("/b"), os("/c")).unwrap(),
            PathBuf::from("/a")
        );
        assert_eq!(d(None, Some("/b"), os("/c")).unwrap(), PathBuf::from("/b"));
        assert_eq!(d(None, None, os("/c")).unwrap(), PathBuf::from("/c"));
        // an empty variable counts as none; with nothing at all the command refuses
        for e in [None, os("")] {
            let err = d(None, None, e).unwrap_err().to_string();
            assert!(
                err.contains("no agent dir") && err.contains("--agent-dir"),
                "{err}"
            );
        }
    }

    #[test]
    fn the_stub_of_another_agent_is_never_the_real_gh() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("agent/bin");
        std::fs::create_dir_all(&bin).unwrap();
        // another agent's stub comes first on the PATH, the real gh second
        let other = t.path().join("other/bin");
        write_stub(&other, &text()).unwrap();
        let real = fake_gh(&t.path().join("real"));
        let path =
            std::env::join_paths([other.as_path(), t.path().join("real").as_path()]).unwrap();
        assert_eq!(find_real_gh(&path, &bin), Some(real));
        // twin: with only the other stub on the PATH there is no real gh
        let only = std::env::join_paths([other.as_path()]).unwrap();
        assert_eq!(find_real_gh(&only, &bin), None);
        assert!(file_is_crewboard_stub(&other.join("gh")));
        assert!(!file_is_crewboard_stub(&t.path().join("real/gh")));
        assert!(!file_is_crewboard_stub(&t.path().join("missing")));
    }
}
