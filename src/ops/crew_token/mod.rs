//! `crewboard agent token` (old name `crew-token set`): put the token of the crew account into its env file.
//!
//! The token comes from standard input: typed without echo on a terminal, or piped. It is checked
//! against the GitHub API first (whose token it is, which scopes it has). Only a good token is
//! written, to `~/.config/crewboard/<initiative>.env` with mode 600, by a rename,
//! so the file is never half written. A directory that crewboard creates gets mode 700. An
//! existing directory keeps its mode. The token is never printed, and it is never an
//! argument of a command. `agent add --token-stdin` uses the same functions for a second host.

mod tty;

use crate::base::ids;
use crate::config::valid_initiative_name;
use crate::error::{CbError, CbResult};
use crate::fsutil::{new_file_permissions, write_atomic_with};
use crate::github::{Client, Token};
use crate::ops::doctor::{CheckVerdict, judge_scopes_in};
use std::io::{BufRead, IsTerminal, Read};
use std::path::{Path, PathBuf};
/// The longest input that is read: a token is far shorter.
const MAX_INPUT: u64 = 4096;

/// The options of `agent token`.
pub struct SetArgs<'a> {
    pub initiative: Option<&'a str>,
    /// The login the token must belong to (`--crew-login`).
    pub crew_login: Option<&'a str>,
    /// `--insecure-one-account`: a one-person trial, the token is the owner's, so the
    /// `workflow` scope passes with a note.
    pub trial: bool,
}

/// What the check found out about a token. Never the token.
#[derive(Debug, PartialEq, Eq)]
pub struct Checked {
    pub login: String,
    /// One phrase about the scopes, for the report.
    pub scopes: String,
}

/// `~/.config/crewboard/<initiative>.env`, the env file that `agent add` finds by default.
pub fn default_env_file(initiative: &str) -> CbResult<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| CbError::config("cannot find the home directory (HOME is not set)"))?;
    Ok(env_file_under(&home, initiative))
}

/// The same path for a given home dir. Pure.
pub fn env_file_under(home: &Path, initiative: &str) -> PathBuf {
    home.join(".config")
        .join("crewboard")
        .join(format!("{initiative}.env"))
}

/// The token in `raw`: the first non-empty line, without a leading `export ` or `GH_TOKEN=`, and
/// without quotes. A text that cannot be a token gives a reason that never holds the text.
pub fn clean_token(raw: &str) -> Result<String, &'static str> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .ok_or("no token was given (the input is empty)")?;
    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let line = line.strip_prefix("GH_TOKEN=").unwrap_or(line);
    let token = line.trim_matches(|c| c == '"' || c == '\'');
    if token.len() < 8 {
        return Err("the input is too short for a GitHub token");
    }
    if !token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(
            "the input has characters that a GitHub token does not have (letters, digits, _ and - only)",
        );
    }
    Ok(token.to_string())
}

/// Read the token: without echo from a terminal, else all of standard input.
pub fn read_token() -> CbResult<String> {
    let stdin = std::io::stdin();
    let raw = if stdin.is_terminal() {
        tty::read_hidden("GitHub token of the crew account (the input is hidden): ")
    } else {
        read_all(&mut stdin.lock())
    }
    .map_err(|e| CbError::config(format!("cannot read the token from standard input: {e}")))?;
    clean_token(&raw).map_err(|why| CbError::config(format!("crew-token: {why}")))
}

fn read_all(r: &mut dyn BufRead) -> std::io::Result<String> {
    let mut text = String::new();
    r.take(MAX_INPUT).read_to_string(&mut text)?;
    Ok(text)
}

/// Ask GitHub whose token this is and which scopes it has. A token that lacks a scope, or has
/// one that the crew account must not have, is refused with the fix. A fine-grained token shows
/// no scopes: it passes, and the report says so. In `trial` mode (one account for the owner and
/// the agents) the owner's `workflow` scope passes with a note; every other wide scope fails.
pub fn check(token: &str, want_login: Option<&str>, trial: bool) -> CbResult<Checked> {
    let client = Client::new(Token::new(token))?;
    let login = client.viewer_login()?;
    if let Some(want) = want_login
        && !want.eq_ignore_ascii_case(&login)
    {
        return Err(CbError::refused(
            ids::CREW_TOKEN_LOGIN,
            format!(
                "this token belongs to {login}, not to {want}; nothing was written. Make the token as {want}"
            ),
        ));
    }
    let scopes = match judge_scopes_in(client.oauth_scopes().as_deref(), trial) {
        CheckVerdict::Ok(text) => text,
        CheckVerdict::Fail { what, .. } => {
            return Err(CbError::refused(
                ids::CREW_TOKEN_SCOPES,
                format!(
                    "{what}; nothing was written. Make a new classic token as {login} with the scopes repo and project only, then run this command again"
                ),
            ));
        }
        CheckVerdict::Skip(text) => text,
    };
    Ok(Checked { login, scopes })
}

/// Write `GH_TOKEN=<token>` to `path` by a rename: the file gets mode 600. A parent dir that this
/// call creates gets mode 700; an existing dir keeps its mode. Returns true when a file was replaced.
pub fn write_env_file(path: &Path, token: &str) -> CbResult<bool> {
    let fail = |what: &str, e: std::io::Error| {
        CbError::config(format!("cannot {what} {}: {e}", path.display()))
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        make_private_dirs(dir, &fail)?;
    }
    let existed = path.symlink_metadata().is_ok();
    let text = format!("GH_TOKEN={token}\n");
    write_atomic_with(path, text.as_bytes(), new_file_permissions).map_err(|e| fail("write", e))?;
    Ok(existed)
}

/// Make `dir` and its missing parents. Only the dirs made here get mode 700.
fn make_private_dirs(dir: &Path, fail: &dyn Fn(&str, std::io::Error) -> CbError) -> CbResult<()> {
    let mut missing: Vec<&Path> = dir.ancestors().take_while(|d| !d.exists()).collect();
    std::fs::create_dir_all(dir).map_err(|e| fail("make the dir of", e))?;
    missing.reverse();
    for made in missing {
        set_private_dir(made).map_err(|e| fail("set the mode of the dir of", e))?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Check the token and write it to `path`. The report lines (no token in them).
pub fn store(token: &str, path: &Path, want_login: Option<&str>, trial: bool) -> CbResult<String> {
    let c = check(token, want_login, trial)?;
    let replaced = write_env_file(path, token)?;
    Ok(format!(
        "crew-token: the token belongs to {} ({})\ncrew-token: {} {} (mode 600)\n",
        c.login,
        c.scopes,
        if replaced { "replaced" } else { "wrote" },
        path.display()
    ))
}

/// `agent token`: read the token, check it, write the env file of the initiative.
pub fn run_set(a: &SetArgs) -> CbResult<String> {
    let initiative = initiative_of(a.initiative)?;
    let path = default_env_file(&initiative)?;
    let token = read_token()?;
    let mut out = store(&token, &path, a.crew_login, a.trial)?;
    out.push_str(&format!(
        "crew-token: next step: crewboard agent add <owner>/<repo> --env-file {} --role coordinator\n",
        path.display()
    ));
    Ok(out)
}

/// The initiative of the command: the flag, else the only initiative in the config dir.
fn initiative_of(flag: Option<&str>) -> CbResult<String> {
    if let Some(i) = flag {
        if !valid_initiative_name(i) {
            return Err(CbError::config(format!(
                "--initiative: '{i}' is not a valid initiative name"
            )));
        }
        return Ok(i.to_string());
    }
    let dir = crate::config::ConfigDir::resolve(None)?;
    let mut names = dir.board_initiatives();
    names.extend(dir.agent_initiatives());
    names.sort();
    names.dedup();
    match names.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(CbError::config(
            "pass --initiative <name>: it names the env file ~/.config/crewboard/<name>.env",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_the_first_line_without_decoration() {
        for raw in [
            "ghp_abcdefgh1234\n",
            "  ghp_abcdefgh1234  \r\n",
            "GH_TOKEN=ghp_abcdefgh1234\n",
            "export GH_TOKEN=\"ghp_abcdefgh1234\"\n",
            "# the crew token\n\nghp_abcdefgh1234\nsecond line\n",
        ] {
            assert_eq!(
                clean_token(raw).as_deref(),
                Ok("ghp_abcdefgh1234"),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn text_that_is_no_token_is_refused_without_echo() {
        for raw in [
            "",
            "\n\n",
            "short",
            "has space inside123456",
            "ghp_abc;rm -rf /",
        ] {
            let why = clean_token(raw).unwrap_err();
            assert!(raw.len() < 8 || !why.contains(raw), "{why}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_env_file_has_mode_600_in_a_dir_with_mode_700() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("cfg").join("x.env");
        assert!(!write_env_file(&p, "ghp_abcdefgh1234").unwrap());
        let mode = |f: &Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o600);
        assert_eq!(mode(p.parent().unwrap()), 0o700);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "GH_TOKEN=ghp_abcdefgh1234\n"
        );
        // a loose old file is replaced by a private one
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(write_env_file(&p, "ghp_other12345678").unwrap());
        assert_eq!(mode(&p), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_dir_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let mode = |f: &Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = t.path().join("crew.env");
        write_env_file(&p, "ghp_abcdefgh1234").unwrap();
        assert_eq!(mode(t.path()), 0o755);
        assert_eq!(mode(&p), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_new_dirs_get_mode_700() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let mode = |f: &Path| std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = t.path().join("a").join("b").join("x.env");
        write_env_file(&p, "ghp_abcdefgh1234").unwrap();
        assert_eq!(mode(&t.path().join("a")), 0o700);
        assert_eq!(mode(&t.path().join("a").join("b")), 0o700);
        assert_eq!(mode(t.path()), 0o755);
        assert_eq!(mode(&p), 0o600);
    }
}
