//! `crewboard skill install` and `crewboard skill path`: put the skill that is embedded in this
//! binary (`skill_embed`) on the host, and link it into the skills directory of each agent runtime.
//!
//! The shared copy is `~/.config/agent-skills/crewboard/`. Each runtime gets one symlink to it:
//! claude `~/.claude/skills/crewboard`, codex `~/.codex/skills/crewboard`, copilot
//! `~/.copilot/skills/crewboard`, hermes `~/.hermes/skills/shared/crewboard`. With `--link` the
//! shared path is a symlink to the `skill/` directory of a clone (live edits, for development).
//! A copy writes each file to a temp file next to it and renames it. The manifest `.crewboard-skill.json`
//! names the files of the copy, so that a later install removes a file that the new binary no
//! longer has. Never overwrites a real directory or a link that points elsewhere. Never changes a
//! file of a clone. Unix only (symlinks).

use crate::error::{CbError, CbResult};
use crate::fsutil::{self, Mode};
use crate::skill_embed::{FILES, sha256_hex, skill_version};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// The runtimes that get a link, in the order of the output.
pub const RUNTIMES: [&str; 4] = ["claude", "codex", "copilot", "hermes"];

/// The file in the shared copy that lists what an install wrote.
pub const MANIFEST: &str = ".crewboard-skill.json";

/// The options of `skill install`.
#[derive(Debug, Clone, Default)]
pub struct InstallArgs {
    pub runtimes: Vec<String>,
    pub link: bool,
    /// With `link`: the clone of crewboard (the directory that holds `skill/`).
    pub from: Option<PathBuf>,
    pub dry_run: bool,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Manifest {
    version: String,
    files: BTreeMap<String, String>,
}

/// The shared copy: `<home>/.config/agent-skills/crewboard`.
pub fn canonical_dir(home: &Path) -> PathBuf {
    home.join(".config").join("agent-skills").join("crewboard")
}

fn skills_dir(home: &Path, runtime: &str) -> PathBuf {
    match runtime {
        "claude" => home.join(".claude/skills"),
        "codex" => home.join(".codex/skills"),
        "copilot" => home.join(".copilot/skills"),
        _ => home.join(".hermes/skills/shared"),
    }
}

/// The home directory: `$HOME`.
pub fn home_dir() -> CbResult<PathBuf> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(PathBuf::from(h)),
        _ => Err(CbError::config("HOME is not set")),
    }
}

/// `crewboard skill path`: the shared copy of the current user.
pub fn path_line() -> CbResult<String> {
    Ok(format!("{}\n", canonical_dir(&home_dir()?).display()))
}

fn check_runtimes(names: &[String]) -> CbResult<()> {
    for n in names {
        if !RUNTIMES.contains(&n.as_str()) {
            return Err(CbError::refused(
                "unknown-runtime",
                format!("unknown runtime: {n} (use claude, codex, copilot or hermes)"),
            ));
        }
    }
    Ok(())
}

/// The `skill/` directory of a clone: `--from DIR` (the clone), else the first directory from
/// `cwd` upwards that has `skill/SKILL.md` next to a `Cargo.toml` of the package `crewboard`.
fn find_clone_skill(from: Option<&Path>, cwd: &Path) -> CbResult<PathBuf> {
    if let Some(dir) = from {
        let skill = dir.join("skill");
        if !skill.join("SKILL.md").is_file() {
            return Err(CbError::refused(
                "skill-source",
                format!("no skill/SKILL.md in {}", dir.display()),
            ));
        }
        return skill
            .canonicalize()
            .map_err(|e| CbError::config(format!("cannot resolve {}: {e}", skill.display())));
    }
    for dir in cwd.ancestors() {
        if dir.join("skill/SKILL.md").is_file() && is_crewboard_manifest(&dir.join("Cargo.toml")) {
            return dir
                .join("skill")
                .canonicalize()
                .map_err(|e| CbError::config(format!("cannot resolve {}: {e}", dir.display())));
        }
    }
    Err(CbError::config(
        "--link needs a clone of crewboard (pass --from <dir>)",
    ))
}

fn is_crewboard_manifest(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse::<toml::Table>().ok())
        .and_then(|t| {
            t.get("package")?
                .get("name")?
                .as_str()
                .map(|n| n == "crewboard")
        })
        .unwrap_or(false)
}

/// What the run has to say and whether anything went wrong (exit 1).
struct Run<'a> {
    out: &'a mut dyn Write,
    err: &'a mut dyn Write,
    dry: bool,
    problems: bool,
    /// A step failed (`FAILED:`: a write, remove or link did not work): exit 3.
    failed: bool,
    /// A write to stdout or stderr failed (a closed pipe): the run reports it at the end.
    write_failed: bool,
}

impl Run<'_> {
    fn say(&mut self, line: String) {
        self.write_failed |= writeln!(self.out, "{line}").is_err();
    }

    fn problem(&mut self, line: String) {
        self.write_failed |= writeln!(self.err, "{line}").is_err();
        self.problems = true;
    }

    /// Print what is done (`do:`) or planned (`plan:`), and run `f` only when applying. A failure
    /// prints `FAILED:` and marks the run.
    fn step(&mut self, text: String, f: impl FnOnce() -> io::Result<()>) -> bool {
        if self.dry {
            self.say(format!("plan: {text}"));
            return true;
        }
        self.say(format!("do:   {text}"));
        match f() {
            Ok(()) => true,
            Err(e) => {
                self.write_failed |= writeln!(self.err, "FAILED: {text}: {e}").is_err();
                self.failed = true;
                false
            }
        }
    }
}

/// `crewboard skill install`. Prints the progress to `out`, the problems to `err`. The result is the
/// exit code: 0, 1 (a `SKIP:`: a link or write was not done because something else is in the way)
/// or 3 (a `FAILED:` step: a write, remove or link did not work; it wins over 1). A bad option is
/// an error.
pub fn install(
    home: &Path,
    cwd: &Path,
    a: &InstallArgs,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> CbResult<u8> {
    if cfg!(not(unix)) {
        return Err(CbError::refused(
            "unix-only",
            "crewboard skill install needs a Unix host (macOS or Linux); this build has no skill links on Windows",
        ));
    }
    check_runtimes(&a.runtimes)?;
    let src = if a.link {
        Some(find_clone_skill(a.from.as_deref(), cwd)?)
    } else {
        None
    };
    let canon = canonical_dir(home);
    let mut r = Run {
        out,
        err,
        dry: a.dry_run,
        problems: false,
        failed: false,
        write_failed: false,
    };
    print_header(&mut r, home, src.as_deref(), &canon);
    place_shared(&mut r, &canon, src.as_deref());
    let linked = link_runtimes(&mut r, home, &canon, &a.runtimes);
    summary(&mut r, &canon, &linked);
    if r.write_failed {
        return Err(CbError::config("cannot write the output of skill install"));
    }
    Ok(if r.failed { 3 } else { u8::from(r.problems) })
}

fn print_header(r: &mut Run<'_>, home: &Path, src: Option<&Path>, canon: &Path) {
    let mode = if r.dry { "dry" } else { "apply" };
    let source = match src {
        Some(s) => s.display().to_string(),
        None => format!("embedded in crewboard {}", env!("CARGO_PKG_VERSION")),
    };
    r.say(format!(
        "mode: {mode}   source: {source}   home: {}",
        home.display()
    ));
    let ver = match src {
        Some(s) => std::fs::read_to_string(s.join("SKILL.md"))
            .ok()
            .and_then(|t| skill_version(&t)),
        None => crate::skill_embed::embedded("SKILL.md")
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(skill_version),
    };
    r.say(format!(
        "skill version: {}",
        ver.as_deref().unwrap_or("unknown")
    ));
    if let Ok(t) = std::fs::read_to_string(canon.join("SKILL.md")) {
        r.say(format!(
            "installed skill version: {}",
            skill_version(&t).unwrap_or_default()
        ));
    }
}

/// Step 1: the shared copy, or its link to a clone.
fn place_shared(r: &mut Run<'_>, canon: &Path, src: Option<&Path>) {
    let meta = std::fs::symlink_metadata(canon).ok();
    let is_link = meta.as_ref().is_some_and(|m| m.file_type().is_symlink());
    match (is_link, src) {
        (true, _) => {
            // an unreadable link target compares unequal to the wanted target: the SKIP path
            let tgt = std::fs::read_link(canon).unwrap_or_default();
            if src == Some(tgt.as_path()) {
                r.say(format!("ok:   {} -> {}", canon.display(), tgt.display()));
            } else {
                r.problem(format!(
                    "SKIP: {} is a link to {} (remove it by hand to change the mode)",
                    canon.display(),
                    tgt.display()
                ));
            }
        }
        (false, Some(_)) if meta.is_some() => r.problem(format!(
            "SKIP: {} exists as a real directory; remove it by hand to switch to --link",
            canon.display()
        )),
        (false, Some(s)) => {
            let parent = canon.parent().unwrap_or(canon).to_path_buf();
            let made = r.step(format!("mkdir -p {}", parent.display()), || {
                std::fs::create_dir_all(&parent)
            });
            if made {
                let (s, c) = (s.to_path_buf(), canon.to_path_buf());
                r.step(format!("ln -s {} {}", s.display(), c.display()), || {
                    make_link(&s, &c)
                });
            }
        }
        (false, None) => copy_files(r, canon),
    }
}

/// Is the runtime on this host? Its skills directory or its config directory (`~/.claude`, ...)
/// exists, or its binary is on `PATH`.
fn runtime_present(home: &Path, rt: &str) -> bool {
    skills_dir(home, rt).is_dir() || home.join(format!(".{rt}")).is_dir() || on_path(rt)
}

/// Is an executable file `name` in a directory of `PATH`?
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| fsutil::find_on_path(name, &path).is_some())
}

/// Step 2: one link per runtime. A runtime that is named with `--runtime`, or that is present on
/// this host (`runtime_present`), gets a link. Returns the runtimes that have a link (or would
/// have one) and those that were skipped.
fn link_runtimes(r: &mut Run<'_>, home: &Path, canon: &Path, wanted: &[String]) -> LinkResult {
    let mut res = LinkResult::default();
    for rt in RUNTIMES {
        let dir = skills_dir(home, rt);
        if !dir.is_dir() {
            let named = wanted.iter().any(|w| w == rt);
            if !named && !runtime_present(home, rt) {
                r.say(format!(
                    "skip: {rt}: not found (no ~/.{rt}, no `{rt}` on PATH; add --runtime {rt} to link it anyway)"
                ));
                res.skipped.push(rt);
                continue;
            }
            let d = dir.clone();
            if !r.step(format!("mkdir -p {}", dir.display()), || {
                std::fs::create_dir_all(&d)
            }) {
                continue;
            }
        }
        if link_one(r, &dir.join("crewboard"), canon) {
            res.linked.push(rt);
        }
    }
    res
}

#[derive(Default)]
struct LinkResult {
    linked: Vec<&'static str>,
    skipped: Vec<&'static str>,
}

fn link_one(r: &mut Run<'_>, link: &Path, canon: &Path) -> bool {
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => {
            // an unreadable link target compares unequal to the shared copy: the SKIP path
            let tgt = std::fs::read_link(link).unwrap_or_default();
            if tgt == canon {
                r.say(format!("ok:   {} -> {}", link.display(), canon.display()));
                true
            } else {
                r.problem(format!(
                    "SKIP: {} points to {}, not {}",
                    link.display(),
                    tgt.display(),
                    canon.display()
                ));
                false
            }
        }
        Ok(_) => {
            r.problem(format!(
                "SKIP: {} is a real file or directory; not overwritten",
                link.display()
            ));
            false
        }
        Err(_) => {
            let (l, c) = (link.to_path_buf(), canon.to_path_buf());
            r.step(
                format!("ln -s {} {}", canon.display(), link.display()),
                || make_link(&c, &l),
            )
        }
    }
}

fn summary(r: &mut Run<'_>, canon: &Path, res: &LinkResult) {
    let join = |v: &[&str]| v.iter().map(|s| format!(" {s}")).collect::<String>();
    r.say("---".to_string());
    let verb = if r.dry {
        "would be linked for"
    } else {
        "linked for"
    };
    let names = if res.linked.is_empty() {
        " none".to_string()
    } else {
        join(&res.linked)
    };
    r.say(format!("skill {verb}:{names}"));
    if res.linked.is_empty() && res.skipped.len() == RUNTIMES.len() {
        r.say(format!(
            "no agent runtime found on this host: only the shared copy is installed ({}). Use --runtime <name> to link a runtime anyway.",
            canon.display()
        ));
    } else if !res.skipped.is_empty() {
        r.say(format!(
            "not found, not linked:{} (use --runtime <name> to link it anyway)",
            join(&res.skipped)
        ));
    }
    if r.dry {
        r.say("dry run only: nothing changed. Run again without --dry-run (or with --apply) to do it.".to_string());
    } else if !r.problems && !r.failed {
        r.say(format!("done. Shared copy: {}", canon.display()));
    } else {
        r.write_failed |= writeln!(r.err, "finished with problems (see above).").is_err();
    }
}

// ---- the copy ---------------------------------------------------------------------------------

fn copy_files(r: &mut Run<'_>, canon: &Path) {
    let (remove, kept) = stale_plan(canon);
    // Never write or remove through a symlink inside the shared copy: it could lead anywhere.
    let paths = FILES
        .iter()
        .map(|(p, _, _)| (*p).to_string())
        .chain(remove.iter().map(|(p, _)| p.clone()));
    for rel in paths {
        if let Some(link) = symlink_ancestor(canon, &rel) {
            r.problem(format!(
                "SKIP: {} is a link or cannot be checked; not writing or removing through it (remove it by hand)",
                link.display()
            ));
            return;
        }
    }
    let n = FILES.len();
    let c = canon.to_path_buf();
    if !r.step(
        format!("copy {n} skill files into {}", canon.display()),
        || write_files(&c),
    ) {
        return;
    }
    // The manifest comes after the removals. A path whose removal failed stays in it (with the
    // sha256 that it had), so that the next run tries again.
    let mut entries = manifest_entries();
    for (rel, sha) in &remove {
        let p = canon.join(rel);
        let done = r.step(format!("remove {}", p.display()), || {
            remove_file_and_empty_dirs(canon, &p)
        });
        if !done {
            entries.insert(rel.clone(), sha.clone());
        }
    }
    let mpath = canon.join(MANIFEST);
    r.step(format!("write {}", mpath.display()), || {
        write_manifest(canon, entries)
    });
    for rel in &kept {
        r.say(format!("kept: {} (changed)", canon.join(rel).display()));
    }
    remove_legacy_tests(r, canon);
}

/// A tests directory that a copy of an earlier installer left.
fn remove_legacy_tests(r: &mut Run<'_>, canon: &Path) {
    let legacy = canon.join("loops/tests");
    match std::fs::symlink_metadata(&legacy) {
        Ok(m) if m.file_type().is_symlink() => r.problem(format!(
            "SKIP: {} is a link; not removing through it (remove it by hand)",
            legacy.display()
        )),
        Ok(m) if m.is_dir() => {
            let l = legacy.clone();
            r.step(format!("remove {}", legacy.display()), || {
                std::fs::remove_dir_all(&l)
            });
        }
        _ => {}
    }
}

/// The first directory between `canon` and `rel` that is a symlink or cannot be inspected. A
/// directory that does not exist yet is no problem.
fn symlink_ancestor(canon: &Path, rel: &str) -> Option<PathBuf> {
    let comps: Vec<&str> = rel.split('/').collect();
    let mut dir = canon.to_path_buf();
    for c in &comps[..comps.len().saturating_sub(1)] {
        dir.push(c);
        match std::fs::symlink_metadata(&dir) {
            Ok(m) if m.file_type().is_symlink() => return Some(dir),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
            Err(_) => return Some(dir), // cannot be checked: treat it as a link
        }
    }
    None
}

/// What this install removes and what it keeps, as paths relative to `canon`.
///
/// With a manifest of an earlier install: a file it lists that this binary does not have is
/// removed when its sha256 still equals the manifest (else it is kept: the user changed it). The
/// manifest file itself and a path that is not a plain relative path never count (the manifest
/// is a file of the user). Without a manifest but with a `SKILL.md` (a copy of the old shell
/// installer): the regular files under `loops/` and `references/` that this binary does not have.
fn stale_plan(canon: &Path) -> (Vec<(String, String)>, Vec<String>) {
    let ours = |p: &str| FILES.iter().any(|(q, _, _)| *q == p);
    let text = match std::fs::read_to_string(canon.join(MANIFEST)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound && canon.join("SKILL.md").is_file() => {
            let extras = legacy_extras(canon)
                .into_iter()
                .filter(|p| !ours(p))
                .filter_map(|p| {
                    let sha = sha256_hex(&std::fs::read(canon.join(&p)).ok()?);
                    Some((p, sha))
                })
                .collect();
            return (extras, Vec::new());
        }
        Err(_) => return (Vec::new(), Vec::new()),
    };
    let Ok(old) = serde_json::from_str::<Manifest>(&text) else {
        return (Vec::new(), Vec::new());
    };
    let (mut remove, mut kept) = (Vec::new(), Vec::new());
    for (p, sha) in old.files {
        let plain = !p.is_empty()
            && p != MANIFEST
            && Path::new(&p)
                .components()
                .all(|c| matches!(c, Component::Normal(_)));
        if !plain || ours(&p) {
            continue;
        }
        let path = canon.join(&p);
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            continue; // gone already, or not a regular file: nothing to remove
        }
        match std::fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == sha => remove.push((p, sha)),
            _ => kept.push(p),
        }
    }
    (remove, kept)
}

/// The regular files under `loops/` and `references/` of `canon`, relative, sorted. A symlink is
/// never listed and never followed.
fn legacy_extras(canon: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            if t.is_dir() {
                walk(root, &path, out);
            } else if t.is_file()
                && let Ok(rel) = path.strip_prefix(root)
                && let Some(rel) = rel.to_str()
            {
                // a name that is not UTF-8 is skipped: it could not be printed or listed right
                out.push(rel.replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    for top in ["loops", "references"] {
        let dir = canon.join(top);
        if std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            walk(canon, &dir, &mut out);
        }
    }
    out.sort();
    out
}

fn remove_file_and_empty_dirs(canon: &Path, file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == canon || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// The manifest entries of this binary: path -> sha256.
fn manifest_entries() -> BTreeMap<String, String> {
    FILES
        .iter()
        .map(|(rel, bytes, _)| ((*rel).to_string(), sha256_hex(bytes)))
        .collect()
}

fn write_files(canon: &Path) -> io::Result<()> {
    for (rel, bytes, exec) in FILES {
        write_atomic(&canon.join(rel), bytes, if *exec { 0o755 } else { 0o644 })?;
    }
    Ok(())
}

fn write_manifest(canon: &Path, files: BTreeMap<String, String>) -> io::Result<()> {
    let manifest = Manifest {
        version: crate::skill_embed::embedded("SKILL.md")
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(skill_version)
            .unwrap_or_else(|| "unknown".to_string()),
        files,
    };
    let json = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    write_atomic(&canon.join(MANIFEST), &json, 0o644)
}

/// Write `path` atomically with `mode` (see [`crate::fsutil::write_atomic`]). A running script
/// keeps its old file. A `<path>.new` from an older version (a leftover, or a planted link) is
/// removed first; it is never followed.
fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".new");
    match std::fs::remove_file(path.with_file_name(name)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    fsutil::write_atomic(path, bytes, Mode::Exact(mode))
}

#[cfg(unix)]
fn make_link(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn make_link(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "no symlinks"))
}

/// `install` with the real home, directory and streams.
pub fn run_install(a: &InstallArgs) -> CbResult<u8> {
    let home = home_dir()?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    install(
        &home,
        &cwd,
        a,
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    )
}
