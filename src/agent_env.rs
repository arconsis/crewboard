//! The env file of an agent (`agent.env`) and the token it holds.
//!
//! crewboard reads the crew token itself. It does not source a shell file and it does not call
//! `std::env::set_var`: the loader runs once, before the command, and keeps the result in a
//! read-only [`OnceLock`]. The client reads [`token`]; a child process gets [`AgentEnv::apply`].
//!
//! The file format is a small dotenv subset (see [`parse_dotenv`]). Nothing in it is ever run:
//! `$(...)`, backticks and `$VAR` stay literal text.
//!
//! An *agent dir* is a config dir that has `agent.env` (a file or a link), or `bin/gh`, or
//! `gh-empty-config/`. In an agent dir the token comes from the file, and the token variables of
//! the caller are dropped, so that the login of the owner cannot slip in. The table in
//! [`decide_source`] lists every case.

use crate::base::Refusal;
use crate::base::ids;
use crate::error::{CbError, CbResult};
#[cfg(unix)]
use crate::fsutil::mode_is_private;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The variables that hold a GitHub token. A child of crewboard gets none of the caller's.
pub const TOKEN_VARS: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// The largest env file.
pub const MAX_BYTES: usize = 64 * 1024;

/// Why a line of an env file does not parse. It never holds the text of the line: a line that
/// lacks the `=` may be the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// The line number, counted from 1. 0 means the whole file.
    pub line: usize,
    pub why: &'static str,
}

/// Parse the env file: `KEY=VALUE` lines.
///
/// - An optional `export ` prefix, blank lines, `#` lines and a CR at the end of a line are skipped.
/// - A value in single or double quotes loses its quotes. There are no escapes and no expansion.
/// - An unquoted value must have no whitespace and none of `; & | < >`.
/// - A key is `[A-Za-z_][A-Za-z0-9_]*`. A key that comes twice counts once, the last one.
pub fn parse_dotenv(text: &str) -> Result<Vec<(String, String)>, ParseError> {
    if text.len() > MAX_BYTES {
        return Err(ParseError {
            line: 0,
            why: "the file is larger than 64 KiB",
        });
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out: Vec<(String, String)> = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let err = |why| ParseError { line: i + 1, why };
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = match line.strip_prefix("export") {
            Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
            _ => line,
        };
        let Some((key, val)) = line.split_once('=') else {
            return Err(err("expected KEY=VALUE"));
        };
        if !valid_key(key) {
            return Err(err("invalid key name (letters, digits and _ only)"));
        }
        let value = match val.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let rest = &val[1..];
                match rest.find(q) {
                    None => return Err(err("the quote is not closed")),
                    Some(end) if !rest[end + 1..].is_empty() => {
                        return Err(err("text after the closing quote"));
                    }
                    Some(end) => &rest[..end],
                }
            }
            _ if val.contains(char::is_whitespace) => {
                return Err(err("an unquoted value has a space; put it in quotes"));
            }
            _ if val.contains([';', '&', '|', '<', '>']) => {
                return Err(err(
                    "an unquoted value has a shell operator (; & | < >); put it in quotes",
                ));
            }
            _ => val,
        };
        if value.contains('\0') {
            return Err(err("the value has a NUL byte"));
        }
        match out.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.to_string(),
            None => out.push((key.to_string(), value.to_string())),
        }
    }
    Ok(out)
}

fn valid_key(k: &str) -> bool {
    let mut c = k.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

fn is_token_key(k: &str) -> bool {
    TOKEN_VARS.contains(&k)
}

/// Has the caller set this variable? A proxy variable counts in any case (`https_proxy` and
/// `HTTPS_PROXY` are one setting); every other name must match exactly.
pub fn caller_has(k: &str) -> bool {
    let set = |n: &str| std::env::var_os(n).is_some_and(|v| !v.is_empty());
    if is_proxy_key(k) {
        set(&k.to_ascii_lowercase()) || set(&k.to_ascii_uppercase()) || set(k)
    } else {
        std::env::var_os(k).is_some()
    }
}

fn is_proxy_key(k: &str) -> bool {
    ["https_proxy", "http_proxy", "no_proxy", "all_proxy"]
        .iter()
        .any(|p| k.eq_ignore_ascii_case(p))
}

/// Does crewboard load this key from an env file? The token keys, any `GH_*`, `GITHUB_*` and
/// `CREWBOARD_*` key, the proxy keys (any case) and the two certificate keys.
pub fn is_loaded_key(k: &str) -> bool {
    k.starts_with("GH_")
        || k.starts_with("GITHUB_")
        || k.starts_with("CREWBOARD_")
        || is_proxy_key(k)
        || k == "SSL_CERT_FILE"
        || k == "SSL_CERT_DIR"
}

/// What a check of the file found wrong. Pure, so a test injects the owner.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessProblem {
    /// Group or others have a right on the file.
    Mode(u32),
    /// The owner is neither the current user nor root.
    Owner(u32),
}

/// The mode and the owner of the TARGET of a link (the caller gives the metadata of the target).
#[cfg(unix)]
pub fn check_access(mode: u32, file_uid: u32, my_uid: u32) -> Result<(), AccessProblem> {
    if !mode_is_private(mode) {
        return Err(AccessProblem::Mode(mode & 0o777));
    }
    if file_uid != my_uid && file_uid != 0 {
        return Err(AccessProblem::Owner(file_uid));
    }
    Ok(())
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no arguments and cannot fail.
    unsafe { libc::geteuid() }
}

/// Which env file is in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    None,
    /// `<config dir>/agent.env`.
    AgentEnv,
    /// `--env-file F`.
    Explicit,
}

/// Where the token of the client comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenSource {
    /// The `agent.env` of the config dir.
    AgentEnv,
    /// The file of `--env-file`.
    EnvFile,
    /// A token variable of the caller.
    CallerVar(&'static str),
    /// `gh auth token`.
    GhAuth,
    /// An agent dir without `agent.env` and without a token variable: crewboard would use the
    /// login of `gh`, which is the owner. Refused.
    Refused,
}

/// The precedence of the token. Pure.
///
/// | file | agent dir | token variable | source |
/// |---|---|---|---|
/// | `--env-file` | any | any | the file |
/// | `agent.env` | yes | any | the file; the variables of the caller are dropped |
/// | none | any | set | the variable |
/// | none | no | none | `gh auth token` |
/// | none | yes | none | refused |
pub fn decide_source(
    file: FileKind,
    in_agent_dir: bool,
    caller_var: Option<&'static str>,
) -> TokenSource {
    match (file, caller_var) {
        (FileKind::Explicit, _) => TokenSource::EnvFile,
        (FileKind::AgentEnv, _) => TokenSource::AgentEnv,
        (FileKind::None, Some(v)) => TokenSource::CallerVar(v),
        (FileKind::None, None) if in_agent_dir => TokenSource::Refused,
        (FileKind::None, None) => TokenSource::GhAuth,
    }
}

/// The first of `GH_TOKEN` and `GITHUB_TOKEN` that the caller has set to something.
fn caller_token_var() -> Option<&'static str> {
    caller_token_vars().first().copied()
}

/// Every one of `GH_TOKEN` and `GITHUB_TOKEN` that the caller has set to something.
pub fn caller_token_vars() -> Vec<&'static str> {
    ["GH_TOKEN", "GITHUB_TOKEN"]
        .into_iter()
        .filter(|v| {
            std::env::var(v)
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false)
        })
        .collect()
}

/// Where the token of an owner command comes from (spec/identity#author-and-signature).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerSource {
    /// The file of `--env-file`: the owner named it.
    EnvFile,
    /// The owner's own `gh` login (`gh auth token`). The token variables of the caller are
    /// ignored.
    GhLogin,
    /// An agent dir without `--env-file`: the order of an agent command ([`decide_source`]).
    /// The login of `gh` on an agent host is the owner's, and an agent must not reach it
    /// through crewboard.
    AgentDir,
}

/// The token of an owner command. Pure.
///
/// | `--env-file` | agent dir | source |
/// |---|---|---|
/// | yes | any | the file |
/// | no | no | `gh auth token`; `GH_TOKEN` and `GITHUB_TOKEN` are ignored |
/// | no | yes | the order of an agent command (`agent.env`, else a variable, else refused) |
///
/// A plain dir has no `agent.env` (the file makes a dir an agent dir), so no agent env file
/// can decide the token of an owner command outside an agent dir.
pub fn decide_owner_source(explicit_file: bool, in_agent_dir: bool) -> OwnerSource {
    match (explicit_file, in_agent_dir) {
        (true, _) => OwnerSource::EnvFile,
        (false, false) => OwnerSource::GhLogin,
        (false, true) => OwnerSource::AgentDir,
    }
}

/// The one note of an owner command that ignored token variables of the caller: it names the
/// variables, never a value. `None` when none was set. Pure.
pub fn owner_note(ignored: &[&str]) -> Option<String> {
    let names = match ignored {
        [] => return None,
        [one] => format!("{one} is set and ignored"),
        many => format!("{} are set and ignored", many.join(" and ")),
    };
    Some(format!(
        "note: {names}: an owner command runs with your own gh login (`gh auth token`). Pass --env-file to use the token of a file"
    ))
}

/// The source of the token of an owner command in this run. Without a load (a unit test) it is
/// the `gh` login.
pub fn owner_source() -> OwnerSource {
    match current() {
        Some(e) => decide_owner_source(e.kind == FileKind::Explicit, e.in_agent_dir),
        None => OwnerSource::GhLogin,
    }
}

/// The config dir of this run, as the loader saw it.
pub fn config_dir() -> Option<&'static Path> {
    current().map(|e| e.dir.as_path())
}

/// The env file of one run, loaded.
pub struct AgentEnv {
    kind: FileKind,
    /// The path as given (the link, not its target).
    file: Option<PathBuf>,
    /// The config dir.
    dir: PathBuf,
    in_agent_dir: bool,
    token: Option<String>,
    /// The keys of the file that crewboard loads, in file order.
    vars: Vec<(String, String)>,
    /// The keys that crewboard does not use (a model API key, a custom variable), with their
    /// values: only `crewboard wake` hands them on, to the wake command.
    ignored: Vec<(String, String)>,
}

impl std::fmt::Debug for AgentEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // no value, no token
        f.debug_struct("AgentEnv")
            .field("kind", &self.kind)
            .field("file", &self.file)
            .field("dir", &self.dir)
            .field("in_agent_dir", &self.in_agent_dir)
            .field(
                "keys",
                &self.vars.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field(
                "ignored",
                &self.ignored.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl AgentEnv {
    pub fn kind(&self) -> FileKind {
        self.kind
    }

    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// The token of the file. A caller that prints or logs text must keep it out of that text.
    #[cfg(unix)]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// The value of every token key of the file (`GH_TOKEN`, `GITHUB_TOKEN`, the two enterprise
    /// keys), for a caller that must keep each of them out of the text it prints or logs.
    #[cfg(unix)]
    pub fn token_values(&self) -> Vec<String> {
        self.vars
            .iter()
            .filter(|(k, v)| is_token_key(k) && !v.is_empty())
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// The keys of the file that crewboard does not use itself.
    pub fn ignored_keys(&self) -> Vec<&str> {
        self.ignored.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// The names of the keys that were loaded (never the values).
    pub fn loaded_keys(&self) -> Vec<&str> {
        self.vars.iter().map(|(k, _)| k.as_str()).collect()
    }

    /// Where the token comes from in this run.
    pub fn source(&self) -> TokenSource {
        decide_source(self.kind, self.in_agent_dir, caller_token_var())
    }

    /// The text of the warning for each key that is not loaded.
    pub fn ignored_warnings(&self) -> Vec<String> {
        let label = label_of(self.kind);
        self.ignored
            .iter()
            .map(|(k, _)| format!("{label}: key {k} ignored (not a crewboard setting)"))
            .collect()
    }

    /// The settings of the file that a child of crewboard must get. The token variables of the
    /// caller go first when a file is in force; then the keys of the file are set. A key that the
    /// caller has set wins, except a token key.
    pub fn apply(&self, cmd: &mut Command) {
        if self.kind != FileKind::None {
            for v in TOKEN_VARS {
                cmd.env_remove(v);
            }
        }
        for (k, v) in &self.vars {
            if is_token_key(k) || !caller_has(k) {
                cmd.env(k, v);
            }
        }
    }

    /// [`Self::apply`] for the wake command, which also gets the keys that crewboard does not use
    /// (sourcing the whole file would hand a model API key in it to the wake).
    /// They go to this child only: crewboard's own process and its other children never get them.
    /// A key of the file replaces a variable of the caller, as sourcing a shell file would. The caller applies
    /// what the wake itself sets after this.
    pub fn apply_to_wake(&self, cmd: &mut Command) {
        self.apply(cmd);
        for (k, v) in &self.ignored {
            cmd.env(k, v);
        }
    }

    /// The proxy settings of the file that the caller did not set: (lower-case name, value).
    /// `caller_has` tells whether the caller's environment has a variable.
    pub fn proxy_entries(&self, caller_has: &dyn Fn(&str) -> bool) -> Vec<(String, String)> {
        self.vars
            .iter()
            .filter(|(k, v)| is_proxy_key(k) && !v.is_empty() && !caller_has(k))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .collect()
    }
}

fn label_of(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Explicit => "env file",
        _ => "agent.env",
    }
}

/// Is this config dir an agent dir? It has `agent.env` (also a link that points nowhere),
/// `bin/gh` or `gh-empty-config/`.
pub fn is_agent_dir(dir: &Path) -> bool {
    dir.join("agent.env").symlink_metadata().is_ok()
        || dir.join("bin").join("gh").exists()
        || dir.join("gh-empty-config").is_dir()
}

fn problem(label: &str, what: impl std::fmt::Display) -> CbError {
    CbError::config(format!("{label} {what}"))
}

/// Read, check and parse one env file. The file is opened once and the checks read the metadata
/// of the open handle, so the file that is checked is the file that is read. `my_uid` is the
/// current user (a parameter for tests).
fn read_file(path: &Path, label: &str, my_uid: Option<u32>) -> CbResult<Vec<(String, String)>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => problem(label, format!("not found: {}", path.display())),
        _ => problem(label, format!("cannot be read ({e}): {}", path.display())),
    })?;
    let meta = file
        .metadata()
        .map_err(|e| problem(label, format!("cannot be read ({e}): {}", path.display())))?;
    if !meta.is_file() {
        return Err(problem(label, format!("is not a file: {}", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        match check_access(meta.mode(), meta.uid(), my_uid.unwrap_or_else(current_uid)) {
            Ok(()) => {}
            Err(AccessProblem::Mode(m)) => {
                return Err(problem(
                    label,
                    format!(
                        "{} has mode {m:o}; group and others must have no access. Run: chmod 600 {}",
                        target.display(),
                        target.display()
                    ),
                ));
            }
            Err(AccessProblem::Owner(uid)) => {
                return Err(problem(
                    label,
                    format!(
                        "{} belongs to uid {uid}; it must belong to you or to root",
                        target.display()
                    ),
                ));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = my_uid;
    let mut bytes = Vec::new();
    // one byte more than the cap, so that a file that grew after the check is still caught
    (&mut file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| problem(label, format!("cannot be read ({e}): {}", path.display())))?;
    if bytes.len() > MAX_BYTES {
        return Err(problem(label, "is larger than 64 KiB"));
    }
    let text = String::from_utf8(bytes).map_err(|_| problem(label, "is not UTF-8"))?;
    parse_dotenv(&text).map_err(|e| {
        if e.line == 0 {
            problem(label, e.why)
        } else {
            CbError::config(format!("{label} line {}: {}", e.line, e.why))
        }
    })
}

/// Load the env file of one run: `--env-file`, else `<config_dir>/agent.env`, else none.
///
/// Fails closed (exit 4) when the file is missing, open to group or others, owned by someone
/// else, does not parse, or defines neither `GH_TOKEN` nor `GITHUB_TOKEN`.
pub fn load(config_dir: &Path, env_file: Option<&Path>) -> CbResult<AgentEnv> {
    load_as(config_dir, env_file, None)
}

/// [`load`] with the current user injected (`None`: the real one).
pub fn load_as(
    config_dir: &Path,
    env_file: Option<&Path>,
    my_uid: Option<u32>,
) -> CbResult<AgentEnv> {
    let in_agent_dir = is_agent_dir(config_dir);
    let implicit = config_dir.join("agent.env");
    let (kind, file) = match env_file {
        Some(f) => (FileKind::Explicit, Some(f.to_path_buf())),
        None if implicit.symlink_metadata().is_ok() => (FileKind::AgentEnv, Some(implicit)),
        None => (FileKind::None, None),
    };
    let mut env = AgentEnv {
        kind,
        file: file.clone(),
        dir: config_dir.to_path_buf(),
        in_agent_dir,
        token: None,
        vars: Vec::new(),
        ignored: Vec::new(),
    };
    let Some(file) = file else {
        return Ok(env);
    };
    let label = label_of(kind);
    let pairs = read_file(&file, label, my_uid)?;
    let find = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    env.token = find("GH_TOKEN").or_else(|| find("GITHUB_TOKEN"));
    if env.token.is_none() {
        return Err(CbError::config(format!(
            "{label} has no GH_TOKEN or GITHUB_TOKEN"
        )));
    }
    for (k, v) in pairs {
        if is_loaded_key(&k) {
            env.vars.push((k, v));
        } else {
            env.ignored.push((k, v));
        }
    }
    Ok(env)
}

static LOADED: OnceLock<Result<AgentEnv, CbError>> = OnceLock::new();

/// Remove the four token variables from a child, when an env file is in force. A child that is
/// not given the keys of the file (a job that `lock run` starts, a version probe) must not carry
/// a token of the caller either.
pub fn scrub_children(cmd: &mut Command) {
    if current().is_some_and(|e| e.kind != FileKind::None) {
        for v in TOKEN_VARS {
            cmd.env_remove(v);
        }
    }
}

/// Record that no env file could be loaded at all (no config dir). Every command that needs a
/// token then fails with this error.
pub fn init_error(e: CbError) -> &'static Result<AgentEnv, CbError> {
    LOADED.get_or_init(|| Err(e))
}

/// The error of a failed load, if there was one. A client is never built after it.
pub fn failure() -> Option<&'static CbError> {
    LOADED.get().and_then(|r| r.as_ref().err())
}

/// Load once for this process. The first call decides; a later call changes nothing.
pub fn init(config_dir: &Path, env_file: Option<&Path>) -> &'static Result<AgentEnv, CbError> {
    LOADED.get_or_init(|| load(config_dir, env_file))
}

/// The loaded env, if [`init`] ran and succeeded.
pub fn current() -> Option<&'static AgentEnv> {
    LOADED.get().and_then(|r| r.as_ref().ok())
}

/// The result of [`init`], if it ran. `doctor` reads the error to report it.
pub fn state() -> Option<&'static Result<AgentEnv, CbError>> {
    LOADED.get()
}

/// The token of the env file, if one is in force.
pub fn token() -> Option<&'static str> {
    current().and_then(|e| e.token.as_deref())
}

/// The token of `--env-file`, if that file is in force. An owner command takes no other file.
pub fn explicit_token() -> Option<&'static str> {
    current()
        .filter(|e| e.kind == FileKind::Explicit)
        .and_then(|e| e.token.as_deref())
}

/// A setting for this process: the variable of the caller, else the key of the env file. Only
/// the keys that crewboard loads are in the file side. Not for token keys.
pub fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            current()?
                .vars
                .iter()
                .find(|(k, v)| k == name && !v.is_empty() && !is_token_key(k))
                .map(|(_, v)| v.clone())
        })
}

/// The refusal for a client that would use the login of `gh` in an agent dir. `None` when the
/// token source is fine.
pub fn check_token_source() -> Option<Refusal> {
    let env = current()?;
    (env.source() == TokenSource::Refused).then(|| {
        let dir = env.dir.display();
        Refusal::new(
            ids::OWNER_TOKEN_IN_AGENT_DIR,
            format!(
                "{dir} is an agent dir; it has no agent.env and no GH_TOKEN, so crewboard would use the login of gh (the owner). Link the env file of the crew account with `ln -sfn <file> {dir}/agent.env`, or put the crew token in {dir}/agent.env"
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> Vec<(String, String)> {
        parse_dotenv(text).unwrap()
    }

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    #[test]
    fn parse_plain_export_comment_blank() {
        assert_eq!(ok("GH_TOKEN=abc\n"), vec![pair("GH_TOKEN", "abc")]);
        assert_eq!(ok("export GH_TOKEN=abc"), vec![pair("GH_TOKEN", "abc")]);
        assert_eq!(
            ok("# a comment\n\n  \nA=1\n  # indented comment\nB=2\n"),
            vec![pair("A", "1"), pair("B", "2")]
        );
        // `export` as the start of a key is a key, not the prefix
        assert_eq!(ok("exporter=1"), vec![pair("exporter", "1")]);
    }

    #[test]
    fn parse_quotes_are_removed_and_nothing_is_expanded() {
        assert_eq!(ok("A=\"two words\""), vec![pair("A", "two words")]);
        assert_eq!(ok("A='two words'"), vec![pair("A", "two words")]);
        assert_eq!(ok("A=\"it's\""), vec![pair("A", "it's")]);
        assert_eq!(ok("A='say \"x\"'"), vec![pair("A", "say \"x\"")]);
        // no escapes
        assert_eq!(ok(r#"A="a\nb""#), vec![pair("A", r"a\nb")]);
        // the quotes keep the blanks inside
        assert_eq!(ok("A=\" x \""), vec![pair("A", " x ")]);
    }

    #[test]
    fn parse_command_substitution_and_variables_stay_literal() {
        assert_eq!(ok("A=$(x)"), vec![pair("A", "$(x)")]);
        assert_eq!(ok("A=`x`"), vec![pair("A", "`x`")]);
        assert_eq!(ok("A=$HOME"), vec![pair("A", "$HOME")]);
        assert_eq!(ok("A=\"$(touch /x)\""), vec![pair("A", "$(touch /x)")]);
    }

    #[test]
    fn parse_crlf_empty_value_and_last_key_wins() {
        assert_eq!(ok("A=1\r\nB=2\r\n"), vec![pair("A", "1"), pair("B", "2")]);
        assert_eq!(ok("A=1\r"), vec![pair("A", "1")]);
        assert_eq!(ok("A=\n"), vec![pair("A", "")]);
        assert_eq!(ok("A=1\nA=2\n"), vec![pair("A", "2")]);
        assert_eq!(ok("\u{feff}A=1"), vec![pair("A", "1")]);
    }

    #[test]
    fn parse_a_space_in_an_unquoted_value_names_the_line_and_not_the_value() {
        let e = parse_dotenv("A=1\nGH_TOKEN=abc def\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(e.why.contains("space"), "{e:?}");
        assert!(!e.why.contains("abc") && !e.why.contains("def"));
        // the twin: the same value in quotes parses
        assert_eq!(
            ok("GH_TOKEN=\"abc def\""),
            vec![pair("GH_TOKEN", "abc def")]
        );
    }

    #[test]
    fn parse_rejects_any_whitespace_and_shell_operators_in_an_unquoted_value() {
        // a no-break space and an ideographic space are whitespace too
        for v in [
            "a\u{a0}b",
            "a\u{3000}b",
            "a\tb",
            "x # comment",
            "$(cat /etc/passwd)",
            "a;B=y",
            "a&b",
            "a|b",
            "a<b",
            "a>b",
        ] {
            let e = parse_dotenv(&format!("A={v}\n")).unwrap_err();
            assert_eq!(e.line, 1, "{v:?}");
            assert!(!e.why.contains("passwd"), "{e:?}");
        }
        // the twin: quoted, they are literal text
        assert_eq!(ok("A=\"a;B=y\""), vec![pair("A", "a;B=y")]);
        assert_eq!(
            ok("A='$(cat /etc/passwd)'"),
            vec![pair("A", "$(cat /etc/passwd)")]
        );
        // KEY="x" # comment: text after the closing quote
        assert!(parse_dotenv("KEY=\"x\" # comment").is_err());
    }

    #[test]
    fn parse_errors_name_the_line_and_never_hold_the_text() {
        for (text, line) in [
            ("A=1\nghp_secretsecret\n", 2),
            ("1A=x\n", 1),
            ("A B=x\n", 1),
            ("A =x\n", 1),
            ("=x\n", 1),
            ("\n\nA=\"open\n", 3),
            ("A=\"x\"y\n", 1),
            ("A='x' 'y'\n", 1),
        ] {
            let e = parse_dotenv(text).unwrap_err();
            assert_eq!(e.line, line, "{text:?}: {e:?}");
            assert!(!e.why.contains("secret"), "{e:?}");
        }
        let e = parse_dotenv("ghp_secretsecret").unwrap_err();
        assert!(!e.why.contains("secret"), "{e:?}");
    }

    #[test]
    fn parse_caps_the_file_at_64_kib() {
        let line = "A=1\n";
        let at_cap = format!("A={}", "x".repeat(MAX_BYTES - 2));
        assert_eq!(at_cap.len(), MAX_BYTES);
        assert!(parse_dotenv(&at_cap).is_ok());
        let over = format!("{at_cap}{line}");
        let e = parse_dotenv(&over).unwrap_err();
        assert_eq!(e.line, 0);
        assert!(e.why.contains("64 KiB"));
    }

    #[test]
    fn loaded_keys_follow_the_list() {
        for k in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "GH_HOST",
            "GITHUB_API_URL",
            "CREWBOARD_API_BASE",
            "HTTPS_PROXY",
            "https_proxy",
            "No_Proxy",
            "ALL_PROXY",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            assert!(is_loaded_key(k), "{k}");
        }
        for k in [
            "FOO",
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "SSL_CERT",
            "GHX",
            "MY_GH_TOKEN",
        ] {
            assert!(!is_loaded_key(k), "{k}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn mode_and_owner_checks() {
        // the pass direction: 600 and 400 owned by me, 600 owned by root
        assert_eq!(check_access(0o100600, 501, 501), Ok(()));
        assert_eq!(check_access(0o100400, 501, 501), Ok(()));
        assert_eq!(check_access(0o100600, 0, 501), Ok(()));
        // red: group or others have a right
        for m in [0o644, 0o640, 0o604, 0o660, 0o700 | 0o001, 0o620] {
            assert_eq!(
                check_access(0o100000 | m, 501, 501),
                Err(AccessProblem::Mode(m)),
                "{m:o}"
            );
        }
        // red: another owner (not root)
        assert_eq!(
            check_access(0o100600, 502, 501),
            Err(AccessProblem::Owner(502))
        );
    }

    #[cfg(unix)]
    mod files {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn write(dir: &Path, name: &str, text: &str, mode: u32) -> PathBuf {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
            p
        }

        fn msg(r: CbResult<AgentEnv>) -> String {
            r.unwrap_err().to_string()
        }

        #[test]
        fn mode_and_owner_of_a_real_file() {
            let d = tempfile::tempdir().unwrap();
            let f = write(d.path(), "ok.env", "GH_TOKEN=ghp_test1\n", 0o600);
            assert!(load(d.path(), Some(&f)).is_ok());
            // red: 644
            let open = write(d.path(), "open.env", "GH_TOKEN=ghp_test1\n", 0o644);
            let m = msg(load(d.path(), Some(&open)));
            assert!(m.contains("mode 644") && m.contains("chmod 600"), "{m}");
            assert!(!m.contains("ghp_test1"));
            // another owner, by an injected uid
            let m = msg(load_as(d.path(), Some(&f), Some(4_000_001)));
            // the file belongs to the real user, who is not 4000001 and not root
            if current_uid() != 0 {
                assert!(m.contains("belongs to uid"), "{m}");
            }
        }

        #[test]
        fn every_token_key_of_the_file_is_listed_and_no_other_value() {
            let d = tempfile::tempdir().unwrap();
            let f = write(
                d.path(),
                "t.env",
                "GH_TOKEN=ghp_one\nGITHUB_TOKEN=ghp_two\nGH_ENTERPRISE_TOKEN=ghe_three\nGITHUB_ENTERPRISE_TOKEN=ghe_four\nGH_HOST=example.org\n",
                0o600,
            );
            let mut v = load(d.path(), Some(&f)).unwrap().token_values();
            v.sort();
            assert_eq!(v, ["ghe_four", "ghe_three", "ghp_one", "ghp_two"]);
        }

        #[test]
        fn mode_is_judged_on_the_target_of_a_link() {
            let d = tempfile::tempdir().unwrap();
            let agent = d.path().join("agent");
            std::fs::create_dir(&agent).unwrap();
            let target = write(d.path(), "crew.env", "GH_TOKEN=ghp_test1\n", 0o644);
            std::os::unix::fs::symlink(&target, agent.join("agent.env")).unwrap();
            let m = msg(load(&agent, None));
            assert!(m.contains("mode 644"), "{m}");
            // the twin: the same link to a 600 target loads
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
            let e = load(&agent, None).unwrap();
            assert_eq!(e.kind(), FileKind::AgentEnv);
            assert!(e.in_agent_dir);
            // a link that points nowhere is an agent dir with a missing file: fail closed
            std::fs::remove_file(&target).unwrap();
            let m = msg(load(&agent, None));
            assert!(m.contains("not found"), "{m}");
        }

        #[test]
        fn a_file_without_a_token_is_refused() {
            let d = tempfile::tempdir().unwrap();
            let f = write(d.path(), "no.env", "GH_HOST=github.com\nGH_TOKEN=\n", 0o600);
            assert!(msg(load(d.path(), Some(&f))).contains("no GH_TOKEN or GITHUB_TOKEN"));
            let g = write(d.path(), "git.env", "GITHUB_TOKEN=ghp_test2\n", 0o600);
            let e = load(d.path(), Some(&g)).unwrap();
            assert_eq!(e.token.as_deref(), Some("ghp_test2"));
        }

        #[test]
        fn a_bad_line_names_the_line_and_not_the_value() {
            let d = tempfile::tempdir().unwrap();
            let f = write(
                d.path(),
                "bad.env",
                "GH_HOST=x\nGH_TOKEN=ghp_test3 more\n",
                0o600,
            );
            let m = msg(load(d.path(), Some(&f)));
            assert!(m.contains("env file line 2:"), "{m}");
            assert!(!m.contains("ghp_test3"), "{m}");
        }

        #[test]
        fn keys_outside_the_list_are_ignored_with_a_warning_line() {
            let d = tempfile::tempdir().unwrap();
            let f = write(
                d.path(),
                "k.env",
                "GH_TOKEN=ghp_test4\nFOO=1\nHTTPS_PROXY=http://p:3128\nCREWBOARD_X=1\n",
                0o600,
            );
            let e = load(d.path(), Some(&f)).unwrap();
            assert_eq!(e.ignored_keys(), ["FOO"]);
            assert_eq!(e.loaded_keys(), ["GH_TOKEN", "HTTPS_PROXY", "CREWBOARD_X"]);
            assert_eq!(
                e.ignored_warnings(),
                ["env file: key FOO ignored (not a crewboard setting)"]
            );
            assert!(!format!("{e:?}").contains("ghp_test4"));
        }

        #[test]
        fn apply_to_wake_adds_the_other_keys_and_apply_does_not() {
            let d = tempfile::tempdir().unwrap();
            let f = write(
                d.path(),
                "w.env",
                "GH_TOKEN=ghp_test6\nGH_HOST=h.example\nFOO=1\nANTHROPIC_API_KEY=sk-test-key-6\nexport BAR=2\n",
                0o600,
            );
            let e = load(d.path(), Some(&f)).unwrap();
            let envs = |cmd: &Command| -> std::collections::HashMap<String, Option<String>> {
                cmd.get_envs()
                    .map(|(k, v)| {
                        (
                            k.to_string_lossy().into_owned(),
                            v.map(|v| v.to_string_lossy().into_owned()),
                        )
                    })
                    .collect()
            };
            let mut wake = Command::new("true");
            e.apply_to_wake(&mut wake);
            let w = envs(&wake);
            assert_eq!(w["FOO"].as_deref(), Some("1"));
            assert_eq!(w["ANTHROPIC_API_KEY"].as_deref(), Some("sk-test-key-6"));
            assert_eq!(w["BAR"].as_deref(), Some("2"));
            assert_eq!(w["GH_HOST"].as_deref(), Some("h.example"));
            assert_eq!(w["GH_TOKEN"].as_deref(), Some("ghp_test6"));
            assert_eq!(w["GITHUB_TOKEN"], None, "a token of the caller is removed");
            // every other child gets the settings of crewboard only
            let mut other = Command::new("true");
            e.apply(&mut other);
            let o = envs(&other);
            for k in ["FOO", "ANTHROPIC_API_KEY", "BAR"] {
                assert!(!o.contains_key(k), "{k}");
            }
            assert_eq!(o["GH_HOST"].as_deref(), Some("h.example"));
            // crewboard's own process is untouched
            assert!(std::env::var_os("ANTHROPIC_API_KEY").is_none());
            assert!(
                !format!("{e:?}").contains("sk-test-key-6"),
                "no value in Debug"
            );
        }

        #[test]
        fn the_owner_source_of_a_loaded_dir() {
            let src =
                |e: &AgentEnv| decide_owner_source(e.kind() == FileKind::Explicit, e.in_agent_dir);
            // a plain dir: no agent.env can exist there, the own gh login
            let plain = tempfile::tempdir().unwrap();
            assert_eq!(
                src(&load(plain.path(), None).unwrap()),
                OwnerSource::GhLogin
            );
            // an agent dir with agent.env: the agent order, never the gh login
            let d = tempfile::tempdir().unwrap();
            write(d.path(), "agent.env", "GH_TOKEN=ghp_test_agent\n", 0o600);
            let agent = load(d.path(), None).unwrap();
            assert_eq!(agent.kind(), FileKind::AgentEnv);
            assert_eq!(src(&agent), OwnerSource::AgentDir);
            // --env-file wins in both dirs
            let f = write(d.path(), "owner.env", "GH_TOKEN=ghp_test_owner\n", 0o600);
            assert_eq!(
                src(&load(d.path(), Some(&f)).unwrap()),
                OwnerSource::EnvFile
            );
            assert_eq!(
                src(&load(plain.path(), Some(&f)).unwrap()),
                OwnerSource::EnvFile
            );
        }

        #[test]
        fn the_agent_dir_is_found_by_its_three_marks() {
            let d = tempfile::tempdir().unwrap();
            assert!(!is_agent_dir(d.path()));
            std::fs::create_dir(d.path().join("gh-empty-config")).unwrap();
            assert!(is_agent_dir(d.path()));
            let e = tempfile::tempdir().unwrap();
            std::fs::create_dir(e.path().join("bin")).unwrap();
            assert!(!is_agent_dir(e.path()));
            std::fs::write(e.path().join("bin").join("gh"), "").unwrap();
            assert!(is_agent_dir(e.path()));
            let f = tempfile::tempdir().unwrap();
            write(f.path(), "agent.env", "GH_TOKEN=x\n", 0o600);
            assert!(is_agent_dir(f.path()));
        }
    }

    #[test]
    fn the_owner_token_the_three_rows() {
        // --env-file wins in any dir
        assert_eq!(decide_owner_source(true, false), OwnerSource::EnvFile);
        assert_eq!(decide_owner_source(true, true), OwnerSource::EnvFile);
        // a plain dir: the own gh login, whatever the caller has in GH_TOKEN
        assert_eq!(decide_owner_source(false, false), OwnerSource::GhLogin);
        // an agent dir: the agent order, never the gh login
        assert_eq!(decide_owner_source(false, true), OwnerSource::AgentDir);
    }

    #[test]
    fn the_owner_note_names_the_variables_and_nothing_else() {
        assert_eq!(owner_note(&[]), None);
        let one = owner_note(&["GH_TOKEN"]).unwrap();
        assert!(one.contains("GH_TOKEN is set and ignored"), "{one}");
        assert!(
            one.contains("--env-file") && one.contains("gh auth token"),
            "{one}"
        );
        let two = owner_note(&["GH_TOKEN", "GITHUB_TOKEN"]).unwrap();
        assert!(
            two.contains("GH_TOKEN and GITHUB_TOKEN are set and ignored"),
            "{two}"
        );
        assert_eq!(two.lines().count(), 1);
    }

    #[test]
    fn precedence_the_five_rows() {
        use FileKind::{AgentEnv as Agent, Explicit, None as NoFile};
        use TokenSource as S;
        // 1. an agent dir with agent.env: the file, whatever the caller has
        assert_eq!(decide_source(Agent, true, Some("GH_TOKEN")), S::AgentEnv);
        assert_eq!(decide_source(Agent, true, None), S::AgentEnv);
        // 2. --env-file: the file wins over the caller, in any dir
        assert_eq!(decide_source(Explicit, false, Some("GH_TOKEN")), S::EnvFile);
        assert_eq!(decide_source(Explicit, true, None), S::EnvFile);
        // 3. no file and a variable: the variable, also in an agent dir without agent.env
        assert_eq!(
            decide_source(NoFile, false, Some("GITHUB_TOKEN")),
            S::CallerVar("GITHUB_TOKEN")
        );
        assert_eq!(
            decide_source(NoFile, true, Some("GH_TOKEN")),
            S::CallerVar("GH_TOKEN")
        );
        // 4. a plain dir, no variable: gh
        assert_eq!(decide_source(NoFile, false, None), S::GhAuth);
        // 5. an agent dir without agent.env, no variable: refused
        assert_eq!(decide_source(NoFile, true, None), S::Refused);
    }

    #[test]
    fn apply_drops_the_caller_tokens_and_sets_the_file() {
        let env = AgentEnv {
            kind: FileKind::AgentEnv,
            file: Some(PathBuf::from("/x/agent.env")),
            dir: PathBuf::from("/x"),
            in_agent_dir: true,
            token: Some("ghp_test5".into()),
            vars: vec![pair("GH_TOKEN", "ghp_test5"), pair("GH_HOST", "h.example")],
            ignored: vec![],
        };
        let mut cmd = Command::new("true");
        env.apply(&mut cmd);
        let envs: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(envs["GH_TOKEN"].as_deref(), Some("ghp_test5"));
        // the three other token variables are removed (set to None), never passed
        for v in [
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            assert_eq!(envs[v], None, "{v}");
        }
        // no file: apply touches nothing
        let none = AgentEnv {
            kind: FileKind::None,
            file: None,
            dir: PathBuf::from("/x"),
            in_agent_dir: false,
            token: None,
            vars: vec![],
            ignored: vec![],
        };
        let mut cmd = Command::new("true");
        none.apply(&mut cmd);
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn proxy_entries_skip_what_the_caller_set() {
        let env = AgentEnv {
            kind: FileKind::AgentEnv,
            file: None,
            dir: PathBuf::from("/x"),
            in_agent_dir: true,
            token: None,
            vars: vec![
                pair("HTTPS_PROXY", "http://p:3128"),
                pair("No_Proxy", "localhost"),
                pair("GH_HOST", "h"),
                pair("ALL_PROXY", ""),
            ],
            ignored: vec![],
        };
        let none = |_: &str| false;
        assert_eq!(
            env.proxy_entries(&none),
            vec![
                pair("https_proxy", "http://p:3128"),
                pair("no_proxy", "localhost")
            ]
        );
        let has_https = |k: &str| k == "HTTPS_PROXY";
        assert_eq!(
            env.proxy_entries(&has_https),
            vec![pair("no_proxy", "localhost")]
        );
    }
}
