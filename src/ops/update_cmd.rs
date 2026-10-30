//! `crewboard update`: replace the running binary with a release (or with a build in a local
//! directory), then run `skill install` of the new binary.
//!
//! Steps, in this order, and nothing on the host changes before step 4 has passed:
//! 1. pick the source: the latest release, `--release vX.Y.Z`, or `--from DIR` (no network);
//! 2. get `SHA256SUMS` and `crewboard-<version>-<target>.tar.gz`;
//! 3. verify: the sums list the tarball once and match, the tarball has one top directory named
//!    like the tarball and a regular file `crewboard` in it (no absolute path, no `..`, no link),
//!    and the unpacked binary prints `crewboard <version>` for `--version`;
//! 4. install: write `<exe>.new` next to the running executable, mode 755, fsync, rename over it
//!    (a running process keeps its old inode). Same sha256 as the installed file: no write;
//! 5. run `<exe> skill install` with the new binary, so the shared skill copy matches.
//!
//! No backup of the old version is kept: the way back is `--release <old tag>`.

use crate::base::ids;
use crate::base::ignore::{best_effort_cleanup, best_effort_output, child_may_have_ended};
use crate::error::{CbError, CbResult};
use crate::github::{API_BASE_ENV, GhError, parse_api_base};
use crate::skill_embed::sha256_hex;
use reqwest::blocking::Client as Http;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use serde_json::Value;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The repository that releases come from, unless `--repo` names another one (a fork).
pub const DEFAULT_REPO: &str = "arconsis/crewboard";

const DEFAULT_API_BASE: &str = "https://api.github.com";
/// The largest tarball (and the largest unpacked stream) that `update` reads.
const MAX_TARBALL: u64 = 256 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_SUMS: u64 = 1024 * 1024;
/// How long the unpacked binary has for `--version`.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// The options of `update`.
#[derive(Debug, Clone)]
pub struct UpdateArgs {
    /// `--release`: a tag `vX.Y.Z`.
    pub release: Option<String>,
    /// `--from`: a directory with `SHA256SUMS` and the tarball for this target.
    pub from: Option<PathBuf>,
    pub repo: String,
    pub check: bool,
}

// ---------------------------------------------------------------- pure helpers

/// The target triple of a build, or `None` when releases do not cover the platform.
pub fn target_for(os: &str, arch: &str, env: &str) -> Option<&'static str> {
    match (os, arch, env) {
        ("macos", "aarch64", _) => Some("aarch64-apple-darwin"),
        ("macos", "x86_64", _) => Some("x86_64-apple-darwin"),
        ("linux", "x86_64", "gnu") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64", "gnu") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// The target triple of this binary, from the compile-time cfg.
pub fn target_triple() -> Option<&'static str> {
    let env = if cfg!(target_env = "gnu") {
        "gnu"
    } else if cfg!(target_env = "musl") {
        "musl"
    } else {
        ""
    };
    target_for(std::env::consts::OS, std::env::consts::ARCH, env)
}

/// `X.Y.Z` with an optional `-pre` suffix (and a leading `v`) as numbers. A pre-release sorts
/// before the release of the same number.
fn parse_version(s: &str) -> Option<(u64, u64, u64, bool)> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let s = s.split_once('+').map_or(s, |(v, _)| v);
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) if !p.is_empty() => (c, true),
        Some(_) => return None,
        None => (s, false),
    };
    let mut it = core.split('.');
    let mut num = || {
        let p = it.next()?;
        if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse::<u64>().ok()
    };
    let v = (num()?, num()?, num()?);
    if it.next().is_some() {
        return None;
    }
    Some((v.0, v.1, v.2, pre))
}

/// Whether version `a` is older than version `b`. `None` when one does not parse.
fn is_older(a: &str, b: &str) -> Option<bool> {
    let key = |v: (u64, u64, u64, bool)| (v.0, v.1, v.2, !v.3);
    Some(key(parse_version(a)?) < key(parse_version(b)?))
}

/// `--release` value: exactly `vX.Y.Z`.
pub fn parse_tag(s: &str) -> Result<String, String> {
    match parse_version(s) {
        Some((_, _, _, false)) if s.starts_with('v') => Ok(s.to_string()),
        _ => Err(format!(
            "{s:?} is not a release tag; use vX.Y.Z, for example v0.6.0"
        )),
    }
}

/// `--repo` value: `OWNER/NAME`.
pub fn parse_repo(s: &str) -> Result<String, String> {
    let ok_part = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    match s.split_once('/') {
        Some((o, n)) if ok_part(o) && ok_part(n) => Ok(s.to_string()),
        _ => Err(format!("{s:?} is not OWNER/NAME")),
    }
}

fn tarball_name(version: &str, target: &str) -> String {
    format!("crewboard-{version}-{target}.tar.gz")
}

/// The version inside `crewboard-<version>-<target>.tar.gz`.
fn version_of_tarball(name: &str, target: &str) -> Option<String> {
    let v = name
        .strip_prefix("crewboard-")?
        .strip_suffix(&format!("-{target}.tar.gz"))?;
    parse_version(v)?;
    Some(v.to_string())
}

/// The sha256 that a `SHA256SUMS` text (sha256sum format) lists for `name`. The name must
/// appear exactly once.
fn sums_hash(text: &str, name: &str) -> Result<String, String> {
    let mut found: Vec<String> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let bad = || format!("SHA256SUMS line {} is not in sha256sum format", i + 1);
        let (hash, rest) = line.split_once(char::is_whitespace).ok_or_else(bad)?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(bad());
        }
        let file = rest.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        if file == name {
            found.push(hash.to_ascii_lowercase());
        }
    }
    match found.len() {
        0 => Err(format!("SHA256SUMS does not list {name}")),
        1 => Ok(found.remove(0)),
        n => Err(format!(
            "SHA256SUMS lists {name} {n} times (it must list it once)"
        )),
    }
}

/// What can go wrong while the tarball is read.
#[derive(Debug)]
enum UnpackError {
    /// The tarball breaks a rule (exit 2).
    Bad(String),
    /// A write failed (a failed step, exit 3).
    Io(io::Error),
}

impl From<io::Error> for UnpackError {
    fn from(e: io::Error) -> Self {
        UnpackError::Io(e)
    }
}

/// Check the path of one tar entry: only normal components, the first one is `top`. Returns the
/// number of components.
fn check_entry_path(path: &Path, top: &str) -> Result<usize, String> {
    let mut n = 0;
    for (i, c) in path.components().enumerate() {
        match c {
            Component::Normal(s) => {
                if i == 0 && s != top {
                    return Err(format!(
                        "the tarball has an entry outside {top}/: {}",
                        path.display()
                    ));
                }
                n += 1;
            }
            _ => {
                return Err(format!(
                    "the tarball has an unsafe path (absolute, `.` or `..`): {}",
                    path.display()
                ));
            }
        }
    }
    if n == 0 {
        return Err("the tarball has an empty path".into());
    }
    Ok(n)
}

/// Read the tarball, validate every entry and write the file `<top>/crewboard` to `dest` (mode
/// 700). Nothing else is written: no entry is unpacked by its own path.
fn unpack_binary(tarball: &[u8], top: &str, dest: &Path) -> Result<(), UnpackError> {
    use tar::EntryType;
    let gz = flate2::read::GzDecoder::new(tarball).take(MAX_UNPACKED);
    let mut archive = tar::Archive::new(gz);
    let wanted = format!("{top}/crewboard");
    let mut found = false;
    let entries = archive
        .entries()
        .map_err(|e| UnpackError::Bad(format!("the tarball cannot be read: {e}")))?;
    for entry in entries {
        let mut entry =
            entry.map_err(|e| UnpackError::Bad(format!("the tarball cannot be read: {e}")))?;
        let kind = entry.header().entry_type();
        if kind == EntryType::XGlobalHeader {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| UnpackError::Bad(format!("the tarball has an unreadable path: {e}")))?
            .into_owned();
        // macOS tar adds AppleDouble entries (`._name`); they are metadata, never extracted
        if kind == EntryType::Regular
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("._"))
            && path.components().all(|c| matches!(c, Component::Normal(_)))
        {
            continue;
        }
        let depth = check_entry_path(&path, top).map_err(UnpackError::Bad)?;
        match kind {
            EntryType::Directory => {}
            EntryType::Regular | EntryType::Continuous => {
                if depth < 2 {
                    return Err(UnpackError::Bad(format!(
                        "the tarball has a file outside {top}/: {}",
                        path.display()
                    )));
                }
                if path == Path::new(&wanted) {
                    if found {
                        return Err(UnpackError::Bad(format!("the tarball has {wanted} twice")));
                    }
                    found = true;
                    write_limited(&mut entry, dest)?;
                }
            }
            _ => {
                return Err(UnpackError::Bad(format!(
                    "the tarball has a link or special file: {}",
                    path.display()
                )));
            }
        }
    }
    if !found {
        return Err(UnpackError::Bad(format!(
            "the tarball has no regular file {wanted}"
        )));
    }
    Ok(())
}

fn write_limited(src: &mut dyn Read, dest: &Path) -> Result<(), UnpackError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o700);
    }
    let mut f = opts.open(dest)?;
    let n = io::copy(&mut src.take(MAX_TARBALL + 1), &mut f)?;
    if n > MAX_TARBALL {
        return Err(UnpackError::Bad(
            "the binary in the tarball is too large".into(),
        ));
    }
    f.sync_all()?;
    Ok(())
}

// ---------------------------------------------------------------- GitHub

/// The token for the API: `GH_TOKEN`, else `GITHUB_TOKEN`, else none (anonymous).
fn env_token() -> Option<String> {
    ["GH_TOKEN", "GITHUB_TOKEN"].into_iter().find_map(|v| {
        let t = std::env::var(v).ok()?;
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_string())
    })
}

struct Api {
    http: Http,
    base: String,
    token: Option<String>,
    repo: String,
}

impl Api {
    fn new(repo: &str) -> CbResult<Self> {
        let base = match crate::agent_env::var(API_BASE_ENV) {
            Some(v) if !v.trim().is_empty() => parse_api_base(&v)?,
            _ => DEFAULT_API_BASE.to_string(),
        };
        let http = Http::builder()
            .timeout(Duration::from_secs(300))
            // reqwest drops the Authorization header when a redirect leaves the origin (the
            // asset URL of the API redirects to the download host); the limit is pinned here
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| GhError::Network(e.to_string()))?;
        Ok(Api {
            http,
            base,
            token: env_token(),
            repo: repo.to_string(),
        })
    }

    /// One GET. `what` names the thing for the 404 text. The token goes only to the API base.
    fn get(&self, url: &str, accept: &str, what: &str) -> CbResult<reqwest::blocking::Response> {
        if !url.starts_with(&format!("{}/", self.base)) {
            return Err(GhError::Shape(format!(
                "refused to fetch {what}: its URL is not under {}/, and no request was sent",
                self.base
            ))
            .into());
        }
        let mut r = self
            .http
            .get(url)
            .header(ACCEPT, accept)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(USER_AGENT, concat!("crewboard/", env!("CARGO_PKG_VERSION")));
        if let Some(t) = &self.token {
            r = r.header(AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = r
            .send()
            .map_err(|e| GhError::Network(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        if status < 400 {
            return Ok(resp);
        }
        let text = resp.text().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| text.chars().take(200).collect());
        match status {
            404 => Err(CbError::refused(
                ids::UPDATE_RELEASE,
                format!(
                    "{what} does not exist or is not readable. If the repo is a private fork or mirror, put a token with read access in GH_TOKEN (for example `GH_TOKEN=$(gh auth token) crewboard update`)"
                ),
            )),
            401 => Err(GhError::Auth(format!(
                "{message}; check GH_TOKEN (it needs read access to {})",
                self.repo
            ))
            .into()),
            _ => Err(GhError::Http {
                status,
                message,
                accepted_permissions: None,
            }
            .into()),
        }
    }

    /// The release JSON: the latest one, or the one of `tag`.
    fn release(&self, tag: Option<&str>) -> CbResult<Value> {
        let (path, what) = match tag {
            Some(t) => (
                format!("releases/tags/{t}"),
                format!("the release {t} of {}", self.repo),
            ),
            None => (
                "releases/latest".to_string(),
                format!("the latest release of {}", self.repo),
            ),
        };
        let url = format!("{}/repos/{}/{path}", self.base, self.repo);
        let text = self
            .get(&url, "application/vnd.github+json", &what)?
            .text()
            .map_err(|e| GhError::Network(format!("reading the release: {e}")))?;
        serde_json::from_str(&text)
            .map_err(|e| GhError::Shape(format!("the release is not JSON: {e}")).into())
    }

    /// The API URL of the asset `name` of `release`.
    fn asset_url(release: &Value, name: &str) -> CbResult<String> {
        release
            .get("assets")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|x| x.get("name").and_then(Value::as_str) == Some(name))
            })
            .and_then(|x| x.get("url").and_then(Value::as_str))
            .map(str::to_string)
            .ok_or_else(|| {
                CbError::refused(
                    ids::UPDATE_RELEASE,
                    format!("the release has no asset {name} (it may not publish this target)"),
                )
            })
    }

    /// The bytes of the asset `name` of `release`, at most `max`.
    fn asset(&self, release: &Value, name: &str, max: u64) -> CbResult<Vec<u8>> {
        let url = Api::asset_url(release, name)?;
        let resp = self.get(
            &url,
            "application/octet-stream",
            &format!("the asset {name}"),
        )?;
        let mut buf = Vec::new();
        resp.take(max + 1)
            .read_to_end(&mut buf)
            .map_err(|e| GhError::Network(format!("reading {name}: {e}")))?;
        if buf.len() as u64 > max {
            return Err(CbError::refused(
                ids::UPDATE_RELEASE,
                format!("the asset {name} is larger than {max} bytes"),
            ));
        }
        Ok(buf)
    }
}

// ---------------------------------------------------------------- the run

struct Say<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    check: bool,
    write_failed: bool,
}

impl Say<'_> {
    fn line(&mut self, l: String) {
        self.write_failed |= writeln!(self.out, "{l}").is_err();
    }

    /// A step: `do:` when it runs, `plan:` for `--check`.
    fn step(&mut self, text: String) {
        let p = if self.check { "plan:" } else { "do:  " };
        self.line(format!("{p} {text}"));
    }

    fn failed(&mut self, text: &str, e: &dyn std::fmt::Display) -> u8 {
        self.failed_with(3, text, e)
    }

    /// Like `failed`, with the exit code of the child that failed.
    fn failed_with(&mut self, code: u8, text: &str, e: &dyn std::fmt::Display) -> u8 {
        self.write_failed |= writeln!(self.err, "FAILED: {text}: {e}").is_err();
        code
    }
}

/// A temporary directory next to the installed binary. It is removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn create_in(dir: &Path) -> io::Result<Self> {
        let nanos = crate::clock::now_nanos();
        let p = dir.join(format!(".crewboard-update-{}-{nanos}", std::process::id()));
        #[allow(unused_mut)] // only the Unix build sets the mode
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&p)?;
        Ok(TempDir(p))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        best_effort_cleanup(std::fs::remove_dir_all(&self.0));
    }
}

/// `--from`: the one tarball of this target in `dir`, and its version.
fn find_local_tarball(dir: &Path, target: &str) -> CbResult<(String, String)> {
    let rd = std::fs::read_dir(dir).map_err(|e| {
        CbError::refused(
            ids::UPDATE_SOURCE,
            format!("cannot read {}: {e}", dir.display()),
        )
    })?;
    let mut found: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let n = e.file_name().into_string().ok()?;
            let v = version_of_tarball(&n, target)?;
            Some((n, v))
        })
        .collect();
    found.sort();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(CbError::refused(
            ids::UPDATE_SOURCE,
            format!(
                "{} has no crewboard-<version>-{target}.tar.gz",
                dir.display()
            ),
        )),
        _ => Err(CbError::refused(
            ids::UPDATE_SOURCE,
            format!(
                "{} has more than one tarball for {target} ({}); keep one",
                dir.display(),
                found
                    .iter()
                    .map(|f| f.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn read_capped(path: &Path, max: u64) -> CbResult<Vec<u8>> {
    let f = std::fs::File::open(path).map_err(|e| {
        CbError::refused(
            ids::UPDATE_SOURCE,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    let mut buf = Vec::new();
    f.take(max + 1).read_to_end(&mut buf).map_err(|e| {
        CbError::refused(
            ids::UPDATE_SOURCE,
            format!("cannot read {}: {e}", path.display()),
        )
    })?;
    if buf.len() as u64 > max {
        return Err(CbError::refused(
            ids::UPDATE_SOURCE,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    Ok(buf)
}

/// Run `bin --version`, at most `VERSION_TIMEOUT`, and return its first line.
#[allow(
    clippy::disallowed_methods,
    reason = "real elapsed time of a child process"
)]
pub(crate) fn version_line(bin: &Path) -> Result<String, String> {
    let mut child = crate::spawn::spawn_retrying(
        Command::new(bin)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    )
    .map_err(|e| format!("it cannot run on this host: {e}"))?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < VERSION_TIMEOUT => {
                crate::clock::pause(Duration::from_millis(10));
            }
            Ok(None) => {
                child_may_have_ended(child.kill());
                child_may_have_ended(child.wait());
                return Err("`--version` did not finish in 30 s".into());
            }
            Err(e) => return Err(format!("cannot wait for it: {e}")),
        }
    };
    let mut out = String::new();
    if let Some(so) = child.stdout.take() {
        so.take(4096).read_to_string(&mut out).ok(); // a read error keeps what was read so far
    }
    if !status.success() {
        return Err(format!("`--version` exited with {status}"));
    }
    Ok(out.lines().next().unwrap_or("").trim().to_string())
}

/// Write `<exe>.new` (the verified file is renamed into place, so no copy is made), mode 755,
/// fsync, then rename it over `exe`.
fn install_file(unpacked: &Path, exe: &Path) -> io::Result<()> {
    let mut new_name = exe.as_os_str().to_os_string();
    new_name.push(".new");
    let new = PathBuf::from(new_name);
    let res = (|| {
        std::fs::rename(unpacked, &new)?;
        crate::fsutil::set_mode(&new, 0o755)?;
        std::fs::File::open(&new)?.sync_all()?;
        std::fs::rename(&new, exe)
    })();
    if res.is_err() {
        best_effort_cleanup(std::fs::remove_file(&new));
        return res;
    }
    crate::fsutil::sync_parent(exe);
    Ok(())
}

/// `crewboard update`. The result is the exit code: 0, or 3 (a `FAILED:` step). A rule that
/// says no (a bad checksum, tarball or version, an unknown target, a missing release) is an
/// error with exit 2, a GitHub or network failure 3 or 4 (a refused token).
pub fn update(
    a: &UpdateArgs,
    exe: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> CbResult<u8> {
    let target = target_triple().ok_or_else(|| {
        CbError::refused(
            ids::UPDATE_TARGET,
            format!(
                "no release exists for this platform ({} {}); releases cover aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu and aarch64-unknown-linux-gnu",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        )
    })?;
    let installed = env!("CARGO_PKG_VERSION");
    let mut s = Say {
        out,
        err,
        check: a.check,
        write_failed: false,
    };
    let mode = if a.check { "check" } else { "apply" };
    s.line(format!(
        "mode: {mode}   target: {target}   installed: crewboard {installed} at {}",
        exe.display()
    ));
    let code = run_steps(a, exe, target, installed, &mut s)?;
    if s.write_failed {
        return Err(CbError::config("cannot write the output of update"));
    }
    Ok(code)
}

/// What `run_steps` knows about the source before it downloads or reads the tarball.
enum Fetch {
    Dir(PathBuf),
    Release(Api, Value),
}

/// Why a step ends the update: a refusal or error, or an exit code that the step already
/// reported (`Exit(0)` is a success that needs no more work).
enum Stop {
    Err(CbError),
    Exit(u8),
}

impl From<CbError> for Stop {
    fn from(e: CbError) -> Self {
        Stop::Err(e)
    }
}

type Step<T> = Result<T, Stop>;

/// The text of each step, for `do:` and for `plan:` lines.
struct Texts {
    get: String,
    verify: String,
    version: String,
    install: String,
    skill: String,
    last: String,
}

impl Texts {
    fn new(
        exe: &Path,
        name: &str,
        fetch: &Fetch,
        version: &str,
        installed: &str,
        target: &str,
    ) -> Self {
        Texts {
            get: match fetch {
                Fetch::Dir(d) => format!("read SHA256SUMS and {name} in {}", d.display()),
                Fetch::Release(..) => format!("download SHA256SUMS and {name}"),
            },
            verify: format!("verify: SHA256SUMS lists {name} once and the sha256 matches"),
            version: format!(
                "verify: the unpacked crewboard prints `crewboard {version}` for --version"
            ),
            install: format!(
                "install {} (write {0}.new, rename over it; skipped when the sha256 is the same)",
                exe.display()
            ),
            skill: format!("{} skill install", exe.display()),
            last: format!("crewboard {installed} -> {version} ({target})"),
        }
    }
}

fn run_steps(
    a: &UpdateArgs,
    exe: &Path,
    target: &str,
    installed: &str,
    s: &mut Say<'_>,
) -> CbResult<u8> {
    match steps(a, exe, target, installed, s) {
        Ok(()) => Ok(0),
        Err(Stop::Exit(code)) => Ok(code),
        Err(Stop::Err(e)) => Err(e),
    }
}

fn steps(a: &UpdateArgs, exe: &Path, target: &str, installed: &str, s: &mut Say<'_>) -> Step<()> {
    let (version, name, fetch) = resolve_source(a, target, installed, s)?;
    // 2. before any download: the directory of the binary must be writable, and a release must
    // list the two assets (metadata only). `--check` fails where a run would fail.
    let dir = exe.parent().unwrap_or(Path::new("."));
    let tmp = match TempDir::create_in(dir) {
        Ok(t) => t,
        Err(e) => {
            let code = s.failed(&format!("write in {}", dir.display()), &e);
            s.line(format!(
                "hint: run as the owner of {}, or update with the tool that installed the binary",
                dir.display()
            ));
            return Err(Stop::Exit(code));
        }
    };
    if let Fetch::Release(_, rel) = &fetch {
        Api::asset_url(rel, "SHA256SUMS")?;
        Api::asset_url(rel, &name)?;
    }
    if is_older(&version, installed) == Some(true) {
        s.line(format!("note: downgrade {installed} -> {version}"));
    }
    let t = Texts::new(exe, &name, &fetch, &version, installed, target);
    if a.check {
        plan_only(s, &t, &name, dir);
        return Ok(());
    }
    s.step(t.get.clone());
    let (sums, bytes) = download(&fetch, &name)?;
    // 3. verify, before anything on the host changes
    s.step(t.verify.clone());
    verify_download(&sums, &bytes, &name, s)?;
    let unpacked = unpack_new(&bytes, &name, &tmp, s)?;
    s.step(t.version.clone());
    check_version(&unpacked, &name, &version, s)?;
    // 4. install
    s.step(t.install.clone());
    install_binary(&unpacked, exe, &version, installed, s)?;
    drop(tmp);
    // 5. the skill of the new binary
    s.step(t.skill.clone());
    install_skill(exe, &version, s)?;
    s.line(t.last);
    Ok(())
}

/// Step 1: the source and the version that it holds. `Stop::Exit(0)` when the latest release is
/// older than the installed one.
fn resolve_source(
    a: &UpdateArgs,
    target: &str,
    installed: &str,
    s: &mut Say<'_>,
) -> Step<(String, String, Fetch)> {
    if let Some(dir) = &a.from {
        let (name, version) = find_local_tarball(dir, target)?;
        s.line(format!("source: directory {}", dir.display()));
        return Ok((version, name, Fetch::Dir(dir.clone())));
    }
    let api = Api::new(&a.repo)?;
    let rel = api.release(a.release.as_deref())?;
    let tag = rel
        .get("tag_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if parse_tag(&tag).is_err() || a.release.as_ref().is_some_and(|r| *r != tag) {
        return Err(CbError::refused(
            ids::UPDATE_RELEASE,
            format!(
                "the release has the tag {tag:?}, expected {}",
                a.release.as_deref().unwrap_or("a tag vX.Y.Z")
            ),
        )
        .into());
    }
    let which = if a.release.is_some() {
        "release"
    } else {
        "latest release"
    };
    s.line(format!("source: github {} {which} {tag}", a.repo));
    let version = tag.trim_start_matches('v').to_string();
    if a.release.is_none() && is_older(&version, installed) == Some(true) {
        s.line(format!(
            "ok:   not downgrading: the latest release {tag} is older than the installed {installed}. To install it anyway: crewboard update --release {tag}"
        ));
        return Err(Stop::Exit(0));
    }
    let name = tarball_name(&version, target);
    Ok((version, name, Fetch::Release(api, rel)))
}

/// `--check`: print the plan and change nothing.
fn plan_only(s: &mut Say<'_>, t: &Texts, name: &str, dir: &Path) {
    s.step(t.get.clone());
    s.step(t.verify.clone());
    s.step(format!(
        "unpack {name} into {} and check its paths",
        dir.join(".crewboard-update-*").display()
    ));
    s.step(t.version.clone());
    s.step(t.install.clone());
    s.step(t.skill.clone());
    s.line(format!("plan: {}", t.last));
}

/// Read `SHA256SUMS` and the tarball from the directory or the release.
fn download(fetch: &Fetch, name: &str) -> CbResult<(String, Vec<u8>)> {
    Ok(match fetch {
        Fetch::Dir(d) => (
            read_capped(&d.join("SHA256SUMS"), MAX_SUMS).and_then(|b| {
                String::from_utf8(b)
                    .map_err(|_| CbError::refused(ids::UPDATE_SOURCE, "SHA256SUMS is not text"))
            })?,
            read_capped(&d.join(name), MAX_TARBALL)?,
        ),
        Fetch::Release(api, rel) => (
            String::from_utf8(api.asset(rel, "SHA256SUMS", MAX_SUMS)?)
                .map_err(|_| CbError::refused(ids::UPDATE_RELEASE, "SHA256SUMS is not text"))?,
            api.asset(rel, name, MAX_TARBALL)?,
        ),
    })
}

/// The sha256 of the tarball must match its line in `SHA256SUMS`.
fn verify_download(sums: &str, bytes: &[u8], name: &str, s: &mut Say<'_>) -> CbResult<()> {
    let want = sums_hash(sums, name).map_err(|m| CbError::refused(ids::UPDATE_CHECKSUM, m))?;
    let got = sha256_hex(bytes);
    if got != want {
        return Err(CbError::refused(
            ids::UPDATE_CHECKSUM,
            format!("the sha256 of {name} is {got}, SHA256SUMS says {want}; nothing was changed"),
        ));
    }
    s.line(format!("ok:   sha256 {got} ({} bytes)", bytes.len()));
    Ok(())
}

/// Unpack the binary into the temporary directory. Returns its path.
fn unpack_new(bytes: &[u8], name: &str, tmp: &TempDir, s: &mut Say<'_>) -> Step<PathBuf> {
    s.step(format!(
        "unpack {name} into {} and check its paths",
        tmp.0.display()
    ));
    let unpacked = tmp.0.join("crewboard");
    let top = name.trim_end_matches(".tar.gz");
    match unpack_binary(bytes, top, &unpacked) {
        Ok(()) => Ok(unpacked),
        Err(UnpackError::Bad(m)) => Err(CbError::refused(ids::UPDATE_TARBALL, m).into()),
        Err(UnpackError::Io(e)) => Err(Stop::Exit(
            s.failed(&format!("unpack into {}", tmp.0.display()), &e),
        )),
    }
}

/// The unpacked binary must print the version of the release.
fn check_version(unpacked: &Path, name: &str, version: &str, s: &mut Say<'_>) -> CbResult<()> {
    let said = version_line(unpacked)
        .map_err(|m| CbError::refused(ids::UPDATE_VERSION, format!("the binary in {name}: {m}")))?;
    if said != format!("crewboard {version}") {
        return Err(CbError::refused(
            ids::UPDATE_VERSION,
            format!(
                "the binary in {name} prints {said:?}, expected \"crewboard {version}\"; nothing was changed"
            ),
        ));
    }
    s.line(format!("ok:   the new binary prints {said:?}"));
    Ok(())
}

/// Replace the installed binary, unless it has the same sha256.
fn install_binary(
    unpacked: &Path,
    exe: &Path,
    version: &str,
    installed: &str,
    s: &mut Say<'_>,
) -> Step<()> {
    let new_sha = sha256_hex(&std::fs::read(unpacked).unwrap_or_default());
    let same = std::fs::read(exe).is_ok_and(|b| sha256_hex(&b) == new_sha);
    if !same && version == installed {
        s.line(format!(
            "note: same version {version}, different build (sha256 {new_sha})"
        ));
    }
    if same {
        s.line(format!("ok:   already installed: {}", exe.display()));
    } else if let Err(e) = install_file(unpacked, exe) {
        return Err(Stop::Exit(
            s.failed(&format!("install {}", exe.display()), &e),
        ));
    } else {
        s.line(format!("ok:   installed {}", exe.display()));
    }
    Ok(())
}

/// Run `skill install` of the new binary.
fn install_skill(exe: &Path, version: &str, s: &mut Say<'_>) -> Step<()> {
    best_effort_output(s.out.flush());
    let status = crate::spawn::status_retrying(Command::new(exe).args(["skill", "install"]));
    match status {
        Ok(st) if st.success() => Ok(()),
        Ok(st) => {
            let m = format!(
                "the binary IS updated to {version}, but `skill install` failed ({st}); run `{} skill install` by hand",
                exe.display()
            );
            let code = st.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(3);
            Err(Stop::Exit(s.failed_with(code.max(1), "skill install", &m)))
        }
        Err(e) => {
            let m = format!(
                "the binary IS updated to {version}, but `skill install` did not start ({e}); run `{} skill install` by hand",
                exe.display()
            );
            Err(Stop::Exit(s.failed("skill install", &m)))
        }
    }
}

/// The path of the running executable, links resolved.
fn current_exe() -> CbResult<PathBuf> {
    std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| CbError::config(format!("cannot find the running executable: {e}")))
}

pub fn run_update(a: &UpdateArgs) -> CbResult<u8> {
    let exe = current_exe()?;
    update(a, &exe, &mut io::stdout().lock(), &mut io::stderr().lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    type Entry<'a> = (&'a str, tar::EntryType, &'a [u8]);

    const TOP: &str = "crewboard-1.2.3-x86_64-unknown-linux-gnu";

    #[test]
    fn target_detection_covers_the_four_release_targets_only() {
        assert_eq!(
            target_for("macos", "aarch64", ""),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            target_for("macos", "x86_64", ""),
            Some("x86_64-apple-darwin")
        );
        assert_eq!(
            target_for("linux", "x86_64", "gnu"),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            target_for("linux", "aarch64", "gnu"),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(target_for("linux", "x86_64", "musl"), None);
        assert_eq!(target_for("windows", "x86_64", "msvc"), None);
        assert_eq!(target_for("linux", "riscv64", "gnu"), None);
        assert_eq!(target_for("freebsd", "x86_64", ""), None);
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert_eq!(is_older("0.9.0", "0.10.0"), Some(true));
        assert_eq!(is_older("0.10.0", "0.9.0"), Some(false));
        assert_eq!(is_older("1.0.0", "1.0.0"), Some(false));
        assert_eq!(is_older("v1.0.0", "1.0.1"), Some(true));
        assert_eq!(is_older("1.0.0-rc1", "1.0.0"), Some(true));
        assert_eq!(is_older("1.0.0", "1.0.0-rc1"), Some(false));
        assert_eq!(is_older("x", "1.0.0"), None);
        assert_eq!(is_older("1.0", "1.0.0"), None);
        assert_eq!(is_older("1.0.0.1", "1.0.0"), None);
    }

    #[test]
    fn a_dev_build_of_main_sorts_between_the_last_release_and_the_next() {
        // main is `0.4.0-dev`: the release v0.3.0 is older (no downgrade by a plain update),
        // the release v0.4.0 is newer
        assert_eq!(is_older("0.3.0", "0.4.0-dev"), Some(true));
        assert_eq!(is_older("0.4.0-dev", "0.4.0"), Some(true));
        assert_eq!(is_older("0.4.0", "0.4.0-dev"), Some(false));
        assert_eq!(is_older("0.4.0-dev", "0.3.0"), Some(false));
        // build metadata is ignored
        assert_eq!(is_older("0.4.0+abc", "0.4.0"), Some(false));
        assert_eq!(is_older("0.3.9+abc", "0.4.0"), Some(true));
        // the package version of this build is a version that parses
        assert!(parse_version(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn tag_and_repo_formats() {
        assert!(parse_tag("v0.4.0").is_ok());
        for bad in [
            "0.4.0",
            "v0.4",
            "v0.4.0-rc1",
            "latest",
            "v0.4.0/../x",
            "v1.2.x",
            "",
        ] {
            assert!(parse_tag(bad).is_err(), "{bad}");
        }
        assert!(parse_repo("arconsis/crewboard").is_ok());
        assert!(parse_repo("a-b/c_d.e").is_ok());
        for bad in [
            "crewboard",
            "a/b/c",
            "/b",
            "a/",
            "../x",
            "a/..",
            "a b/c",
            "a/b?x=1",
        ] {
            assert!(parse_repo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tarball_names_round_trip() {
        let t = "aarch64-apple-darwin";
        let n = tarball_name("0.4.0", t);
        assert_eq!(n, "crewboard-0.4.0-aarch64-apple-darwin.tar.gz");
        assert_eq!(version_of_tarball(&n, t).as_deref(), Some("0.4.0"));
        assert_eq!(version_of_tarball(&n, "x86_64-apple-darwin"), None);
        assert_eq!(
            version_of_tarball("crewboard-x-aarch64-apple-darwin.tar.gz", t),
            None
        );
        assert_eq!(
            version_of_tarball("other-0.4.0-aarch64-apple-darwin.tar.gz", t),
            None
        );
    }

    #[test]
    fn sums_must_list_the_name_exactly_once() {
        let h1 = "a".repeat(64);
        let h2 = "B".repeat(64);
        let text = format!("{h1}  one.tar.gz\n\n{h2} *two.tar.gz\n");
        assert_eq!(sums_hash(&text, "one.tar.gz").unwrap(), h1);
        assert_eq!(sums_hash(&text, "two.tar.gz").unwrap(), "b".repeat(64));
        assert!(
            sums_hash(&text, "three.tar.gz")
                .unwrap_err()
                .contains("does not list")
        );
        let twice = format!("{h1}  one.tar.gz\n{h2}  one.tar.gz\n");
        assert!(
            sums_hash(&twice, "one.tar.gz")
                .unwrap_err()
                .contains("2 times")
        );
        // a name that only contains the wanted name does not count
        let part = format!("{h1}  xone.tar.gz\n");
        assert!(sums_hash(&part, "one.tar.gz").is_err());
        // a bad line is an error, never skipped
        assert!(sums_hash("nothex  one.tar.gz\n", "one.tar.gz").is_err());
        assert!(sums_hash("abc\n", "one.tar.gz").is_err());
    }

    /// A tar entry: (path, type, content).
    fn tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, kind, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(*kind);
            h.set_size(if *kind == tar::EntryType::Regular {
                data.len() as u64
            } else {
                0
            });
            h.set_mode(0o755);
            if matches!(*kind, tar::EntryType::Symlink | tar::EntryType::Link) {
                h.set_link_name("/etc/passwd").unwrap();
            }
            // write the path bytes as they are: `set_path` would refuse `..` and `/x`
            let raw = h.as_old_mut();
            raw.name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        let tar = b.into_inner().unwrap();
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(&tar).unwrap();
        e.finish().unwrap()
    }

    fn unpack(entries: &[(&str, tar::EntryType, &[u8])]) -> Result<Vec<u8>, String> {
        let d = std::env::temp_dir().join(format!(
            "cb-update-unit-{}-{}",
            std::process::id(),
            sha256_hex(format!("{entries:?}").as_bytes())
        ));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("crewboard");
        let _ = std::fs::remove_file(&dest);
        let r = unpack_binary(&tarball(entries), TOP, &dest);
        let bytes = std::fs::read(&dest).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&d);
        match r {
            Ok(()) => Ok(bytes),
            Err(UnpackError::Bad(m)) => Err(m),
            Err(UnpackError::Io(e)) => panic!("io: {e}"),
        }
    }

    use tar::EntryType::{Directory as D, Link, Regular as F, Symlink};

    #[test]
    fn a_good_tarball_unpacks_only_the_binary() {
        let bin = unpack(&[
            ("crewboard-1.2.3-x86_64-unknown-linux-gnu/", D, b""),
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard",
                F,
                b"BIN",
            ),
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/README.md",
                F,
                b"hi",
            ),
        ])
        .unwrap();
        assert_eq!(bin, b"BIN");
        // no directory entry is needed
        assert!(
            unpack(&[(
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard",
                F,
                b"B"
            )])
            .is_ok()
        );
    }

    #[test]
    fn appledouble_entries_are_skipped_and_nothing_else_outside_is_allowed() {
        let bin = unpack(&[
            ("._crewboard-1.2.3-x86_64-unknown-linux-gnu", F, b"meta"),
            ("crewboard-1.2.3-x86_64-unknown-linux-gnu/", D, b""),
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/._crewboard",
                F,
                b"meta",
            ),
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard",
                F,
                b"BIN",
            ),
        ])
        .unwrap();
        assert_eq!(bin, b"BIN");
        // a `._` name does not hide a link or an unsafe path
        let e = unpack(&[
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard",
                F,
                b"B",
            ),
            ("crewboard-1.2.3-x86_64-unknown-linux-gnu/../._x", F, b"x"),
        ])
        .unwrap_err();
        assert!(e.contains("unsafe path"), "{e}");
        let e = unpack(&[
            (
                "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard",
                F,
                b"B",
            ),
            ("crewboard-1.2.3-x86_64-unknown-linux-gnu/._l", Symlink, b""),
        ])
        .unwrap_err();
        assert!(e.contains("link or special"), "{e}");
    }

    #[test]
    fn bad_tarballs_are_refused() {
        let bin = "crewboard-1.2.3-x86_64-unknown-linux-gnu/crewboard";
        let cases: Vec<(&str, Vec<Entry<'_>>, &str)> = vec![
            (
                "dotdot",
                vec![
                    (bin, F, b"B"),
                    ("crewboard-1.2.3-x86_64-unknown-linux-gnu/../evil", F, b"x"),
                ],
                "unsafe path",
            ),
            (
                "dotdot first",
                vec![("../crewboard", F, b"x")],
                "unsafe path",
            ),
            (
                "absolute",
                vec![("/tmp/evil", F, b"x"), (bin, F, b"B")],
                "unsafe path",
            ),
            (
                "symlink",
                vec![
                    (bin, F, b"B"),
                    ("crewboard-1.2.3-x86_64-unknown-linux-gnu/l", Symlink, b""),
                ],
                "link or special",
            ),
            (
                "hardlink",
                vec![
                    ("crewboard-1.2.3-x86_64-unknown-linux-gnu/l", Link, b""),
                    (bin, F, b"B"),
                ],
                "link or special",
            ),
            (
                "binary is a link",
                vec![(bin, Symlink, b"")],
                "link or special",
            ),
            (
                "two top dirs",
                vec![(bin, F, b"B"), ("other-dir/crewboard", F, b"B")],
                "outside",
            ),
            (
                "wrong top dir",
                vec![(
                    "crewboard-9.9.9-x86_64-unknown-linux-gnu/crewboard",
                    F,
                    b"B",
                )],
                "outside",
            ),
            ("file at top", vec![("crewboard", F, b"B")], "outside"),
            (
                "no binary",
                vec![(
                    "crewboard-1.2.3-x86_64-unknown-linux-gnu/README.md",
                    F,
                    b"x",
                )],
                "no regular file",
            ),
            ("binary is a dir", vec![(bin, D, b"")], "no regular file"),
            ("twice", vec![(bin, F, b"A"), (bin, F, b"B")], "twice"),
        ];
        for (name, entries, want) in cases {
            let e = unpack(&entries).unwrap_err();
            assert!(e.contains(want), "{name}: {e}");
        }
        // not a gzip tar at all
        let d = std::env::temp_dir().join(format!("cb-update-unit-garbage-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let r = unpack_binary(b"not a tarball", TOP, &d.join("crewboard"));
        let _ = std::fs::remove_dir_all(&d);
        assert!(matches!(r, Err(UnpackError::Bad(_))));
    }
}
