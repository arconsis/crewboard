//! `crewboard update`, run as a process. The "installed" executable is a copy of the test binary
//! in a temporary directory, with a temporary HOME. The "new" binary in a tarball is a small
//! shell script that prints `crewboard <version>` for `--version` and handles `skill install`,
//! or a copy of the test binary itself. The release path runs against a loopback server that
//! speaks the part of the GitHub API that `update` uses (the shared fake of `tests/support`
//! answers text only, and the tarball is binary). No network.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::let_underscore_must_use,
    clippy::disallowed_methods,
    reason = "a test fails by panic and waits for real processes"
)]
#![cfg(unix)]

use crewboard::skill_embed::sha256_hex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

const INSTALLED_VERSION: &str = env!("CARGO_PKG_VERSION");

fn target() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else {
        "unsupported"
    }
}

/// Skip a test on a platform that has no release target.
macro_rules! need_target {
    () => {
        if target() == "unsupported" {
            return;
        }
    };
}

/// A host: HOME, the directory of the installed executable and a scratch directory.
struct Host {
    _t: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    exe: PathBuf,
    original: Vec<u8>,
}

impl Host {
    fn new() -> Host {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let home = root.join("home");
        let bindir = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bindir).unwrap();
        let exe = bindir.join("crewboard");
        fs::copy(env!("CARGO_BIN_EXE_crewboard"), &exe).unwrap();
        let original = fs::read(&exe).unwrap();
        Host {
            _t: t,
            root,
            home,
            exe,
            original,
        }
    }

    fn bindir(&self) -> PathBuf {
        self.exe.parent().unwrap().to_path_buf()
    }

    fn scratch(&self, name: &str) -> PathBuf {
        let d = self.root.join(name);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut c = Command::new(&self.exe);
        c.arg("update")
            .args(args)
            .env("HOME", &self.home)
            .env_remove("CREWBOARD_CONFIG_DIR")
            .env_remove("CREWBOARD_API_BASE")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            // a proxy of the machine must not catch the loopback server
            .env_remove("HTTP_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("ALL_PROXY")
            .env_remove("http_proxy")
            .env_remove("https_proxy")
            .env_remove("all_proxy")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost")
            .current_dir(&self.root);
        for (k, v) in env {
            c.env(k, v);
        }
        let o = c.output().unwrap();
        Run {
            code: o.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&o.stdout).into_owned(),
            err: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn unchanged(&self) {
        assert_eq!(fs::read(&self.exe).unwrap(), self.original, "exe changed");
        let names = self.leftovers();
        assert!(names.is_empty(), "left in the bin dir: {names:?}");
        assert!(!self.home.join("skill-ran").exists(), "skill install ran");
    }

    /// Anything in the bin dir except the executable.
    fn leftovers(&self) -> Vec<String> {
        fs::read_dir(self.bindir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "crewboard")
            .collect()
    }
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

impl Run {
    fn all(&self) -> String {
        format!(
            "code {}\n--- stdout\n{}--- stderr\n{}",
            self.code, self.out, self.err
        )
    }
}

/// The shell script that plays the new binary. `skill_exit` is the exit code of `skill install`.
fn script(version: &str, skill_exit: i32) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) echo 'crewboard {version}' ;;\n  skill) echo \"fake skill install $*\"; echo ran > \"$HOME/skill-ran\"; exit {skill_exit} ;;\nesac\n"
    )
    .into_bytes()
}

/// A gzip tar. Each entry is (path, type, content). The path is written as given.
fn tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (path, kind, data) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(*kind);
        let regular = *kind == tar::EntryType::Regular;
        h.set_size(if regular { data.len() as u64 } else { 0 });
        h.set_mode(0o755);
        if *kind == tar::EntryType::Symlink {
            h.set_link_name("/bin/sh").unwrap();
        }
        h.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        b.append(&h, *data).unwrap();
    }
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(&b.into_inner().unwrap()).unwrap();
    e.finish().unwrap()
}

/// The tarball of a release: `crewboard-<version>-<target>/` with `crewboard` and a README.
fn good_tarball(version: &str, binary: &[u8]) -> Vec<u8> {
    let top = format!("crewboard-{version}-{}", target());
    tarball(&[
        (&format!("{top}/"), tar::EntryType::Directory, b""),
        (&format!("{top}/crewboard"), tar::EntryType::Regular, binary),
        (
            &format!("{top}/README.md"),
            tar::EntryType::Regular,
            b"readme",
        ),
    ])
}

type OwnedEntry = (String, tar::EntryType, Vec<u8>);

fn tar_name(version: &str) -> String {
    format!("crewboard-{version}-{}.tar.gz", target())
}

fn sums_line(name: &str, bytes: &[u8]) -> String {
    format!("{}  {name}\n", sha256_hex(bytes))
}

/// A directory like a downloaded release: the tarball and `SHA256SUMS`.
fn release_dir(host: &Host, version: &str, tgz: &[u8]) -> PathBuf {
    let d = host.scratch("release");
    fs::write(d.join(tar_name(version)), tgz).unwrap();
    fs::write(d.join("SHA256SUMS"), sums_line(&tar_name(version), tgz)).unwrap();
    d
}

fn from_args(d: &Path) -> Vec<String> {
    vec!["--from".to_string(), d.display().to_string()]
}

fn run_from(host: &Host, d: &Path) -> Run {
    let a = from_args(d);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    host.run(&a, &[])
}

// ---------------------------------------------------------------- --from

#[test]
fn from_a_directory_installs_the_binary_and_runs_skill_install() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("9.9.9", 0)));
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("9.9.9", 0));
    let mode = fs::metadata(&h.exe).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
    assert!(h.leftovers().is_empty(), "{:?}", h.leftovers());
    assert!(h.home.join("skill-ran").exists());
    // `skill install` of the new binary ran, and its output is shown
    assert!(
        r.out.contains("fake skill install skill install"),
        "{}",
        r.all()
    );
    assert!(r.out.contains("do:"), "{}", r.all());
    let last = r.out.lines().last().unwrap();
    assert_eq!(
        last,
        format!("crewboard {INSTALLED_VERSION} -> 9.9.9 ({})", target())
    );
}

#[test]
fn the_binary_of_the_tarball_may_be_older_than_the_installed_one() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "0.0.1", &good_tarball("0.0.1", &script("0.0.1", 0)));
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("0.0.1", 0));
}

#[test]
fn a_checksum_mismatch_is_refused_and_nothing_changes() {
    need_target!();
    let h = Host::new();
    let tgz = good_tarball("9.9.9", &script("9.9.9", 0));
    let d = release_dir(&h, "9.9.9", &tgz);
    fs::write(
        d.join("SHA256SUMS"),
        format!("{}  {}\n", "0".repeat(64), tar_name("9.9.9")),
    )
    .unwrap();
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-checksum"), "{}", r.all());
    h.unchanged();
}

#[test]
fn sums_that_list_the_tarball_twice_or_not_at_all_are_refused() {
    need_target!();
    let h = Host::new();
    let tgz = good_tarball("9.9.9", &script("9.9.9", 0));
    let d = release_dir(&h, "9.9.9", &tgz);
    let line = sums_line(&tar_name("9.9.9"), &tgz);
    fs::write(d.join("SHA256SUMS"), format!("{line}{line}")).unwrap();
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("2 times"), "{}", r.all());
    fs::write(d.join("SHA256SUMS"), sums_line("other.tar.gz", &tgz)).unwrap();
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("does not list"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_binary_with_another_version_than_its_tarball_is_refused() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("1.0.0", 0)));
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-version"), "{}", r.all());
    assert!(r.err.contains("crewboard 1.0.0"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_binary_that_does_not_run_is_refused() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", b"not an executable"));
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-version"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_tarball_with_a_link_or_a_bad_path_is_refused() {
    need_target!();
    let top = format!("crewboard-9.9.9-{}", target());
    let bin = script("9.9.9", 0);
    let cases: Vec<(&str, Vec<OwnedEntry>)> = vec![
        (
            "symlink",
            vec![
                (
                    format!("{top}/crewboard"),
                    tar::EntryType::Regular,
                    bin.clone(),
                ),
                (format!("{top}/link"), tar::EntryType::Symlink, vec![]),
            ],
        ),
        (
            "dotdot",
            vec![
                (
                    format!("{top}/crewboard"),
                    tar::EntryType::Regular,
                    bin.clone(),
                ),
                (
                    format!("{top}/../escape"),
                    tar::EntryType::Regular,
                    b"x".to_vec(),
                ),
            ],
        ),
        (
            "absolute",
            vec![
                (
                    "/tmp/escape".to_string(),
                    tar::EntryType::Regular,
                    b"x".to_vec(),
                ),
                (
                    format!("{top}/crewboard"),
                    tar::EntryType::Regular,
                    bin.clone(),
                ),
            ],
        ),
        (
            "two top dirs",
            vec![
                (
                    format!("{top}/crewboard"),
                    tar::EntryType::Regular,
                    bin.clone(),
                ),
                (
                    "other/crewboard".to_string(),
                    tar::EntryType::Regular,
                    bin.clone(),
                ),
            ],
        ),
        (
            "no binary",
            vec![(
                format!("{top}/README.md"),
                tar::EntryType::Regular,
                b"x".to_vec(),
            )],
        ),
    ];
    for (name, entries) in cases {
        let h = Host::new();
        let e: Vec<(&str, tar::EntryType, &[u8])> = entries
            .iter()
            .map(|(p, k, d)| (p.as_str(), *k, d.as_slice()))
            .collect();
        let d = release_dir(&h, "9.9.9", &tarball(&e));
        let r = run_from(&h, &d);
        assert_eq!(r.code, 2, "{name}: {}", r.all());
        assert!(r.err.contains("update-tarball"), "{name}: {}", r.all());
        h.unchanged();
    }
}

#[test]
fn check_prints_the_plan_and_changes_nothing() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("9.9.9", 0)));
    let before: Vec<_> = fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    let mut a = from_args(&d);
    a.push("--check".into());
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    let r = h.run(&a, &[]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.out.contains("mode: check"), "{}", r.all());
    assert!(
        r.out.contains(&format!("target: {}", target())),
        "{}",
        r.all()
    );
    assert!(
        r.out
            .contains(&format!("installed: crewboard {INSTALLED_VERSION}")),
        "{}",
        r.all()
    );
    assert!(r.out.contains("source: directory"), "{}", r.all());
    assert!(r.out.contains("plan: "), "{}", r.all());
    assert!(!r.out.contains("do:"), "{}", r.all());
    assert_eq!(
        r.out.lines().last().unwrap(),
        format!(
            "plan: crewboard {INSTALLED_VERSION} -> 9.9.9 ({})",
            target()
        )
    );
    h.unchanged();
    let after: Vec<_> = fs::read_dir(&d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(before.len(), after.len());
}

#[test]
fn the_same_binary_is_already_installed_and_is_not_written() {
    need_target!();
    let h = Host::new();
    // the tarball holds a copy of the installed binary, so its --version is the installed one
    let d = release_dir(
        &h,
        INSTALLED_VERSION,
        &good_tarball(INSTALLED_VERSION, &h.original),
    );
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.out.contains("already installed"), "{}", r.all());
    assert!(!r.out.contains("ok:   installed"), "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), h.original);
    assert!(h.leftovers().is_empty(), "{:?}", h.leftovers());
    // `skill install` still ran, with the real binary and the temporary HOME
    assert!(
        h.home
            .join(".config/agent-skills/crewboard/SKILL.md")
            .is_file(),
        "{}",
        r.all()
    );
    assert_eq!(
        r.out.lines().last().unwrap(),
        format!(
            "crewboard {INSTALLED_VERSION} -> {INSTALLED_VERSION} ({})",
            target()
        )
    );
}

#[test]
fn a_failing_skill_install_is_reported_after_the_binary_is_updated() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("9.9.9", 1)));
    let r = run_from(&h, &d);
    // the exit code of `skill install` (1: a SKIP) comes through
    assert_eq!(r.code, 1, "{}", r.all());
    assert!(r.err.contains("binary IS updated to 9.9.9"), "{}", r.all());
    assert!(r.err.contains("`skill install` failed"), "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("9.9.9", 1));
    assert!(
        !r.out.contains("->"),
        "no final line after a failure: {}",
        r.all()
    );
}

#[test]
fn a_directory_that_is_not_writable_fails_with_its_path() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("9.9.9", 0)));
    fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o555)).unwrap();
    // a user that ignores the mode (root) cannot run this case
    let can_write = fs::File::create(h.bindir().join("probe")).is_ok();
    if can_write {
        let _ = fs::remove_file(h.bindir().join("probe"));
        fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let r = run_from(&h, &d);
    fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.code, 3, "{}", r.all());
    assert!(r.err.starts_with("FAILED:"), "{}", r.all());
    assert!(r.err.contains(h.bindir().to_str().unwrap()), "{}", r.all());
    h.unchanged();
}

#[test]
fn from_needs_exactly_one_tarball_for_this_target() {
    need_target!();
    let h = Host::new();
    let empty = h.scratch("empty");
    let r = run_from(&h, &empty);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-source"), "{}", r.all());
    let d = release_dir(&h, "9.9.9", &good_tarball("9.9.9", &script("9.9.9", 0)));
    fs::write(d.join(tar_name("9.9.8")), b"x").unwrap();
    let r = run_from(&h, &d);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("more than one tarball"), "{}", r.all());
    h.unchanged();
}

#[test]
fn bad_options_are_usage_errors() {
    let h = Host::new();
    for args in [
        vec!["--release", "9.9.9"],
        vec!["--release", "v9.9"],
        vec!["--release", "latest"],
        vec!["--release", "v9.9.9", "--from", "/x"],
        vec!["--repo", "no-slash"],
        vec!["--repo", "a/b/c"],
    ] {
        let r = h.run(&args, &[]);
        assert_eq!(r.code, 4, "{args:?}: {}", r.all());
    }
    h.unchanged();
}

// ---------------------------------------------------------------- a release on a fake GitHub

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

type Handler = dyn Fn(&str, &HashMap<String, String>) -> (u16, Vec<u8>) + Send + Sync;

struct Server {
    base: String,
    log: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(make: impl FnOnce(&str) -> Box<Handler>) -> Server {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let base = format!("http://{addr}");
        let handler = make(&base);
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (log2, stop2) = (log.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for conn in l.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(c) = conn {
                    serve(c, &*handler, &log2);
                }
            }
        });
        Server {
            base,
            log,
            stop,
            thread: Some(thread),
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.log.lock().unwrap().clone()
    }

    fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.path).collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.base.trim_start_matches("http://"));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(conn: TcpStream, handler: &Handler, log: &Mutex<Vec<Seen>>) {
    let mut rd = BufReader::new(conn.try_clone().unwrap());
    let mut first = String::new();
    if rd.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut it = first.split_whitespace();
    let method = it.next().unwrap_or("").to_string();
    let path = it.next().unwrap_or("").to_string();
    let mut headers = HashMap::new();
    loop {
        let mut l = String::new();
        if rd.read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let mut body = vec![
        0u8;
        headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    ];
    let _ = rd.read_exact(&mut body);
    log.lock().unwrap().push(Seen {
        method,
        path: path.clone(),
        headers: headers.clone(),
    });
    let (status, bytes) = handler(&path, &headers);
    let mut c = conn;
    // a 302 carries its target URL as the body of the answer of the handler
    let location = if status == 302 {
        format!("Location: {}\r\n", String::from_utf8_lossy(&bytes))
    } else {
        String::new()
    };
    let bytes = if status == 302 { Vec::new() } else { bytes };
    let _ = write!(
        c,
        "HTTP/1.1 {status} X\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = c.write_all(&bytes);
    let _ = c.flush();
}

/// The release JSON of `tag`; the assets point at `<base>/assets/<name>`.
fn release_json(base: &str, repo: &str, tag: &str, asset_base: Option<&str>) -> Vec<u8> {
    let version = tag.trim_start_matches('v');
    let names = [tar_name(version), "SHA256SUMS".to_string()];
    let assets: Vec<String> = names
        .iter()
        .map(|n| {
            format!(
                "{{\"name\":\"{n}\",\"url\":\"{}/repos/{repo}/releases/assets/{n}\"}}",
                asset_base.unwrap_or(base)
            )
        })
        .collect();
    format!(
        "{{\"tag_name\":\"{tag}\",\"assets\":[{}]}}",
        assets.join(",")
    )
    .into_bytes()
}

/// A fake GitHub with one release `tag` of `repo` (latest and by tag). The assets need
/// `Accept: application/octet-stream`, as the real API does.
fn github(repo: &'static str, tag: &'static str, binary_version: &'static str) -> Server {
    github_with(repo, tag, binary_version, None)
}

fn github_with(
    repo: &'static str,
    tag: &'static str,
    binary_version: &'static str,
    asset_base: Option<&'static str>,
) -> Server {
    Server::start(move |base| {
        let base = base.to_string();
        let version = tag.trim_start_matches('v').to_string();
        let tgz = good_tarball(&version, &script(binary_version, 0));
        let sums = sums_line(&tar_name(&version), &tgz).into_bytes();
        Box::new(move |path, headers| {
            let json = |t: &str| (200, release_json(&base, repo, t, asset_base));
            if path == format!("/repos/{repo}/releases/latest")
                || path == format!("/repos/{repo}/releases/tags/{tag}")
            {
                return json(tag);
            }
            if let Some(name) = path.strip_prefix(&format!("/repos/{repo}/releases/assets/")) {
                if headers.get("accept").map(String::as_str) != Some("application/octet-stream") {
                    return (415, b"{\"message\":\"wrong accept\"}".to_vec());
                }
                return if name == "SHA256SUMS" {
                    (200, sums.clone())
                } else if name == tar_name(&version) {
                    (200, tgz.clone())
                } else {
                    (404, b"{\"message\":\"Not Found\"}".to_vec())
                };
            }
            (404, b"{\"message\":\"Not Found\"}".to_vec())
        })
    })
}

#[test]
fn the_latest_release_is_downloaded_with_the_token_and_installed() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let r = h.run(
        &[],
        &[
            ("CREWBOARD_API_BASE", &s.base),
            ("GH_TOKEN", "tok-secret-1"),
        ],
    );
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("9.9.9", 0));
    assert!(
        r.out
            .contains("source: github arconsis/crewboard latest release v9.9.9"),
        "{}",
        r.all()
    );
    assert!(h.home.join("skill-ran").exists());
    // every request carried the token; the asset requests asked for the bytes
    let seen = s.seen();
    assert!(seen.len() >= 3, "{seen:?}");
    for q in &seen {
        assert_eq!(q.method, "GET");
        assert_eq!(
            q.headers.get("authorization").map(String::as_str),
            Some("Bearer tok-secret-1")
        );
    }
    // a token is never printed
    assert!(
        !r.out.contains("tok-secret") && !r.err.contains("tok-secret"),
        "{}",
        r.all()
    );
}

#[test]
fn github_token_is_the_second_choice_and_none_is_anonymous() {
    need_target!();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let h = Host::new();
    let r = h.run(
        &[],
        &[("CREWBOARD_API_BASE", &s.base), ("GITHUB_TOKEN", "tok-two")],
    );
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        s.seen()
            .iter()
            .all(|q| q.headers.get("authorization").map(String::as_str) == Some("Bearer tok-two"))
    );
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let h = Host::new();
    let r = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        s.seen()
            .iter()
            .all(|q| !q.headers.contains_key("authorization")),
        "{:?}",
        s.seen()
    );
    // GH_TOKEN wins over GITHUB_TOKEN
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let h = Host::new();
    let r = h.run(
        &[],
        &[
            ("CREWBOARD_API_BASE", &s.base),
            ("GH_TOKEN", "one"),
            ("GITHUB_TOKEN", "two"),
        ],
    );
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        s.seen()
            .iter()
            .all(|q| q.headers.get("authorization").map(String::as_str) == Some("Bearer one"))
    );
}

#[test]
fn release_installs_a_tag_that_is_older_than_the_installed_version() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v0.0.1", "0.0.1");
    let r = h.run(&["--release", "v0.0.1"], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        s.paths()
            .contains(&"/repos/arconsis/crewboard/releases/tags/v0.0.1".to_string()),
        "{:?}",
        s.paths()
    );
    assert_eq!(fs::read(&h.exe).unwrap(), script("0.0.1", 0));
    assert_eq!(
        r.out.lines().last().unwrap(),
        format!("crewboard {INSTALLED_VERSION} -> 0.0.1 ({})", target())
    );
}

#[test]
fn repo_names_the_fork() {
    need_target!();
    let h = Host::new();
    let s = github("someone/fork", "v9.9.9", "9.9.9");
    let r = h.run(
        &["--repo", "someone/fork"],
        &[("CREWBOARD_API_BASE", &s.base)],
    );
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        s.paths()
            .iter()
            .all(|p| p.starts_with("/repos/someone/fork/")),
        "{:?}",
        s.paths()
    );
}

#[test]
fn an_older_latest_release_is_not_installed() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v0.0.1", "0.0.1");
    let r = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.out.contains("not downgrading"), "{}", r.all());
    assert!(r.out.contains("--release v0.0.1"), "{}", r.all());
    assert_eq!(
        s.paths(),
        vec!["/repos/arconsis/crewboard/releases/latest".to_string()]
    );
    h.unchanged();
}

#[test]
fn check_looks_up_the_release_but_downloads_and_changes_nothing() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let r = h.run(&["--check"], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.out
            .contains("source: github arconsis/crewboard latest release v9.9.9"),
        "{}",
        r.all()
    );
    assert_eq!(
        r.out.lines().last().unwrap(),
        format!(
            "plan: crewboard {INSTALLED_VERSION} -> 9.9.9 ({})",
            target()
        )
    );
    assert_eq!(
        s.paths(),
        vec!["/repos/arconsis/crewboard/releases/latest".to_string()]
    );
    h.unchanged();
}

#[test]
fn a_missing_release_says_how_to_pass_a_token() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let r = h.run(&["--release", "v1.2.3"], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(
        r.err.contains("does not exist or is not readable"),
        "{}",
        r.all()
    );
    assert!(
        r.err.contains("GH_TOKEN=$(gh auth token) crewboard update"),
        "{}",
        r.all()
    );
    h.unchanged();
}

#[test]
fn a_refused_token_is_a_configuration_error() {
    need_target!();
    let h = Host::new();
    let s =
        Server::start(|_| Box::new(|_, _| (401, b"{\"message\":\"Bad credentials\"}".to_vec())));
    let r = h.run(
        &[],
        &[("CREWBOARD_API_BASE", &s.base), ("GH_TOKEN", "bad-tok")],
    );
    assert_eq!(r.code, 4, "{}", r.all());
    assert!(!r.err.contains("bad-tok"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_release_with_another_tag_than_asked_is_refused() {
    need_target!();
    let h = Host::new();
    // the tag route answers with the release v9.9.8
    let s = Server::start(|base| {
        let body = release_json(base, "arconsis/crewboard", "v9.9.8", None);
        Box::new(move |_, _| (200, body.clone()))
    });
    let r = h.run(&["--release", "v9.9.9"], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-release"), "{}", r.all());
    h.unchanged();
}

#[test]
fn an_asset_url_on_another_host_is_not_followed() {
    need_target!();
    let h = Host::new();
    let s = github_with(
        "arconsis/crewboard",
        "v9.9.9",
        "9.9.9",
        Some("http://127.0.0.1:9"),
    );
    let r = h.run(
        &[],
        &[("CREWBOARD_API_BASE", &s.base), ("GH_TOKEN", "tok-x")],
    );
    assert_eq!(r.code, 3, "{}", r.all());
    assert!(r.err.contains("not under"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_release_without_the_tarball_of_this_target_is_refused() {
    need_target!();
    let h = Host::new();
    let s = Server::start(|base| {
        let body = format!(
            "{{\"tag_name\":\"v9.9.9\",\"assets\":[{{\"name\":\"SHA256SUMS\",\"url\":\"{base}/repos/arconsis/crewboard/releases/assets/s\"}}]}}"
        )
        .into_bytes();
        Box::new(move |_, _| (200, body.clone()))
    });
    let r = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains(&tar_name("9.9.9")), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_tampered_download_is_refused_before_anything_changes() {
    need_target!();
    let h = Host::new();
    // the sums belong to another tarball than the one that is served
    let s = Server::start(|base| {
        let tgz = good_tarball("9.9.9", &script("9.9.9", 0));
        let other = good_tarball("9.9.9", &script("9.9.9", 7));
        let sums = sums_line(&tar_name("9.9.9"), &other).into_bytes();
        let json = release_json(base, "arconsis/crewboard", "v9.9.9", None);
        Box::new(move |path, _| {
            if path.ends_with("/SHA256SUMS") {
                (200, sums.clone())
            } else if path.ends_with(".tar.gz") {
                (200, tgz.clone())
            } else {
                (200, json.clone())
            }
        })
    });
    let r = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-checksum"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_build_on_a_platform_without_release_is_refused() {
    // runs only where no release target exists (for example a musl or Windows host); there the
    // refusal comes before any file is read
    if target() != "unsupported" {
        return;
    }
    let h = Host::new();
    let r = h.run(&["--check"], &[]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains("update-target"), "{}", r.all());
}

// ---------------------------------------------------------------- review round

#[test]
fn a_redirect_to_another_origin_carries_no_token_and_installs() {
    need_target!();
    let h = Host::new();
    let tgz = good_tarball("9.9.9", &script("9.9.9", 0));
    let tgz2 = tgz.clone();
    // the download host: another port, so another origin
    let cdn = Server::start(move |_| {
        let t = tgz2.clone();
        Box::new(move |_, _| (200, t.clone()))
    });
    let cdn_url = format!("{}/blob", cdn.base);
    let sums = sums_line(&tar_name("9.9.9"), &tgz).into_bytes();
    let api = Server::start(move |base| {
        let json = release_json(base, "arconsis/crewboard", "v9.9.9", None);
        let tar_asset = format!(
            "/repos/arconsis/crewboard/releases/assets/{}",
            tar_name("9.9.9")
        );
        Box::new(move |path, _| {
            if path == tar_asset {
                (302, cdn_url.clone().into_bytes())
            } else if path.ends_with("/SHA256SUMS") {
                (200, sums.clone())
            } else {
                (200, json.clone())
            }
        })
    });
    let r = h.run(
        &[],
        &[
            ("CREWBOARD_API_BASE", &api.base),
            ("GH_TOKEN", "tok-redirect"),
        ],
    );
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("9.9.9", 0));
    // the API saw the token, the other origin did not
    assert!(
        api.seen()
            .iter()
            .all(|q| q.headers.get("authorization").map(String::as_str)
                == Some("Bearer tok-redirect"))
    );
    let cdn_seen = cdn.seen();
    assert_eq!(cdn_seen.len(), 1, "{cdn_seen:?}");
    assert!(
        !cdn_seen[0].headers.contains_key("authorization"),
        "{cdn_seen:?}"
    );
}

#[test]
fn a_directory_that_is_not_writable_is_found_before_any_download() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o555)).unwrap();
    let can_write = fs::File::create(h.bindir().join("probe")).is_ok();
    if can_write {
        let _ = fs::remove_file(h.bindir().join("probe"));
        fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let apply = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    let check = h.run(&["--check"], &[("CREWBOARD_API_BASE", &s.base)]);
    fs::set_permissions(h.bindir(), fs::Permissions::from_mode(0o755)).unwrap();
    for r in [&apply, &check] {
        assert_eq!(r.code, 3, "{}", r.all());
        assert!(r.err.starts_with("FAILED:"), "{}", r.all());
        assert!(r.err.contains(h.bindir().to_str().unwrap()), "{}", r.all());
        assert!(r.out.contains("hint:"), "{}", r.all());
    }
    // only the release lookup: no asset was requested
    assert!(
        s.paths().iter().all(|p| p.ends_with("/releases/latest")),
        "{:?}",
        s.paths()
    );
}

#[test]
fn check_fails_for_a_release_without_the_tarball_of_this_target() {
    need_target!();
    let h = Host::new();
    let s = Server::start(|base| {
        let body = format!(
            "{{\"tag_name\":\"v9.9.9\",\"assets\":[{{\"name\":\"SHA256SUMS\",\"url\":\"{base}/repos/arconsis/crewboard/releases/assets/s\"}}]}}"
        )
        .into_bytes();
        Box::new(move |_, _| (200, body.clone()))
    });
    let r = h.run(&["--check"], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(r.err.contains(&tar_name("9.9.9")), "{}", r.all());
    h.unchanged();
}

#[test]
fn an_appledouble_entry_in_the_tarball_is_skipped() {
    need_target!();
    let h = Host::new();
    let top = format!("crewboard-9.9.9-{}", target());
    let tgz = tarball(&[
        (&format!("._{top}"), tar::EntryType::Regular, b"meta"),
        (&format!("{top}/"), tar::EntryType::Directory, b""),
        (
            &format!("{top}/._crewboard"),
            tar::EntryType::Regular,
            b"meta",
        ),
        (
            &format!("{top}/crewboard"),
            tar::EntryType::Regular,
            &script("9.9.9", 0),
        ),
    ]);
    let d = release_dir(&h, "9.9.9", &tgz);
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script("9.9.9", 0));
}

#[test]
fn from_an_older_version_prints_a_downgrade_note() {
    need_target!();
    let h = Host::new();
    let d = release_dir(&h, "0.0.1", &good_tarball("0.0.1", &script("0.0.1", 0)));
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(
        r.out
            .contains(&format!("note: downgrade {INSTALLED_VERSION} -> 0.0.1")),
        "{}",
        r.all()
    );
}

#[test]
fn the_same_version_with_another_build_is_replaced_with_a_note() {
    need_target!();
    let h = Host::new();
    let d = release_dir(
        &h,
        INSTALLED_VERSION,
        &good_tarball(INSTALLED_VERSION, &script(INSTALLED_VERSION, 0)),
    );
    let r = run_from(&h, &d);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.out.contains("note: same version"), "{}", r.all());
    assert_eq!(fs::read(&h.exe).unwrap(), script(INSTALLED_VERSION, 0));
}

#[test]
fn a_latest_release_older_than_the_installed_version_is_not_installed() {
    need_target!();
    // v0.0.1 is older than any build (a release or a `<next>-dev` build of main), so a plain
    // update does not replace it; the dev ordering itself is a unit test in update_cmd.rs
    let h = Host::new();
    let s = github("arconsis/crewboard", "v0.0.1", "0.0.1");
    let r = h.run(&[], &[("CREWBOARD_API_BASE", &s.base)]);
    assert_eq!(r.code, 0, "{}", r.all());
    assert!(r.out.contains("not downgrading"), "{}", r.all());
    h.unchanged();
}

#[test]
fn a_token_is_not_printed_on_a_404_or_a_401() {
    need_target!();
    let h = Host::new();
    let s = github("arconsis/crewboard", "v9.9.9", "9.9.9");
    let r = h.run(
        &["--release", "v1.2.3"],
        &[("CREWBOARD_API_BASE", &s.base), ("GH_TOKEN", "tok-404")],
    );
    assert_eq!(r.code, 2, "{}", r.all());
    assert!(
        !r.out.contains("tok-404") && !r.err.contains("tok-404"),
        "{}",
        r.all()
    );
}
