//! File helpers: the private-mode check, atomic file writes, and the executable checks (`set_mode`,
//! `is_executable`, `find_on_path`).
//!
//! Atomic file writes. A reader (or a crash, or a full disk) never leaves a half-written file at
//! the target path: the data goes to a temp file in the same directory, is synced, is renamed
//! over the target, and the directory is synced (unix).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, UNIX_EPOCH};

/// Temp files of a crashed run older than this are removed by the next write.
const STALE_TEMP: Duration = Duration::from_secs(3600);

/// True if the file mode allows no access for group and others.
#[cfg(unix)]
pub fn mode_is_private(mode: u32) -> bool {
    mode & 0o077 == 0
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// The prefix of every temp name of `path`: `<name>.tmp.`
fn temp_prefix(path: &Path) -> std::ffi::OsString {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp.");
    name
}

/// The temp path next to `path`: `<name>.tmp.<pid>.<n>` (`n` keeps threads apart).
fn temp_path(path: &Path) -> PathBuf {
    let mut name = temp_prefix(path);
    name.push(format!(
        "{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

/// Permissions of a new private file: 0600 on unix, the default elsewhere.
#[cfg(unix)]
pub fn new_file_permissions(f: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
pub fn new_file_permissions(_f: &File) -> io::Result<()> {
    Ok(())
}

/// The permissions of the file that [`write_atomic`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// What the process umask gives a new file.
    Default,
    /// These permissions exactly (unix; other systems ignore them).
    Exact(u32),
}

/// Write `data` to `path` atomically: temp file next to `path`, fsync, rename, fsync of the
/// directory (unix). A failure to set the mode is an error before the rename. A failure leaves
/// the old file and removes the temp file. The parent directory must exist (see
/// [`write_atomic_with`] for one that is created).
///
/// A symlink at `path` is replaced, not written through. Temp files of a crashed run are swept
/// by the next write to the same target.
pub fn write_atomic(path: &Path, data: &[u8], mode: Mode) -> io::Result<()> {
    match mode {
        Mode::Default => write_core(path, data, |_| Ok(())),
        Mode::Exact(m) => write_core(path, data, |f| set_file_mode(f, m)),
    }
}

/// The `prepare` step of [`Mode::Keep`]: the permissions of the old file, or 0600 for a new one.
fn keep_permissions(path: &Path) -> impl FnOnce(&File) -> io::Result<()> + use<> {
    let old = std::fs::metadata(path).ok();
    move |f| match &old {
        Some(m) => f.set_permissions(m.permissions()),
        None => new_file_permissions(f),
    }
}

/// [`write_atomic`] with [`Mode::Keep`] that creates missing parent directories.
pub fn write_atomic_private(path: &Path, data: &[u8]) -> io::Result<()> {
    write_atomic_with(path, data, keep_permissions(path))
}

/// Like [`write_atomic`], and creates missing parent directories. `prepare` runs on the new temp
/// file before any data is written, for example to set its permissions. If it fails, the temp
/// file is removed, the target is untouched and the error is returned.
pub fn write_atomic_with(
    path: &Path,
    data: &[u8],
    prepare: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    write_core(path, data, prepare)
}

fn write_core(
    path: &Path,
    data: &[u8],
    prepare: impl FnOnce(&File) -> io::Result<()>,
) -> io::Result<()> {
    let tmp = temp_path(path);
    let run = || -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        prepare(&f)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        rename(&tmp, path)
    };
    run().inspect_err(|_| {
        // the write failed already; the temp file is only litter
        std::fs::remove_file(&tmp).ok();
    })?;
    sync_parent(path);
    sweep_stale_temps(path, STALE_TEMP);
    Ok(())
}

/// Best effort: sync the directory of `path`, so that the rename survives a power loss. The new
/// file is in place already; a failure here (a directory that cannot be opened) is no reason to
/// report the write as failed.
#[cfg(unix)]
pub fn sync_parent(path: &Path) {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(dir).and_then(|d| d.sync_all()).ok();
}

#[cfg(not(unix))]
pub fn sync_parent(_path: &Path) {}

/// Set the permissions of an open file (unix; elsewhere nothing happens).
#[cfg(unix)]
fn set_file_mode(f: &File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_file_mode(_f: &File, _mode: u32) -> io::Result<()> {
    Ok(())
}

/// Set the permissions of a path (unix; elsewhere nothing happens).
pub fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Is `p` a regular file that someone may execute? On other systems than unix: a regular file.
pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// The first `name` in the directories of a PATH value that is an executable file. An empty
/// entry is skipped.
pub fn find_on_path(name: &str, path: impl AsRef<std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(path.as_ref())
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

/// Windows: a scanner or a reader can hold the target for a few ms ("access denied").
#[cfg(windows)]
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if tries < 2 && e.kind() == io::ErrorKind::PermissionDenied => {
                tries += 1;
                crate::clock::pause(Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

#[cfg(not(windows))]
fn rename(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

/// Best effort: remove `<name>.tmp.*` next to `path` that are older than `max_age`. They are
/// left by a process that was killed between create and rename.
fn sweep_stale_temps(path: &Path, max_age: Duration) {
    let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) else {
        return sweep_in(Path::new("."), &temp_prefix(path), max_age);
    };
    sweep_in(dir, &temp_prefix(path), max_age);
}

fn sweep_in(dir: &Path, prefix: &std::ffi::OsStr, max_age: Duration) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let prefix = prefix.to_string_lossy();
    let now_secs = u64::try_from(crate::clock::now_epoch()).unwrap_or(0);
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with(prefix.as_ref()) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .is_some_and(|t| now_secs.saturating_sub(t.as_secs()) > max_age.as_secs());
        if old {
            // best effort: a temp file that stays is swept by the next write
            std::fs::remove_file(e.path()).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("sub").join("s.json");
        write_atomic_with(&p, b"zero", |_| Ok(())).unwrap(); // makes the directory
        write_atomic(&p, b"one", Mode::Default).unwrap();
        write_atomic(&p, b"two", Mode::Default).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
        assert_eq!(names(&d.path().join("sub")), ["s.json"]);
    }

    #[test]
    fn a_failing_prepare_keeps_the_old_file_and_removes_the_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        std::fs::write(&p, "old").unwrap();
        let e = write_atomic_with(&p, b"new", |_| {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "no"))
        })
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old");
        assert_eq!(names(d.path()), ["s.json"]);
    }

    #[test]
    fn only_old_temp_files_of_the_same_target_are_swept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        let old = d.path().join("s.json.tmp.1.0");
        let fresh = d.path().join("s.json.tmp.2.0");
        let other = d.path().join("t.json.tmp.1.0");
        for f in [&old, &fresh, &other] {
            std::fs::write(f, "x").unwrap();
        }
        let two_h = std::time::SystemTime::now() - Duration::from_secs(7200);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(two_h)
            .unwrap();
        File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(two_h)
            .unwrap();
        write_atomic(&p, b"new", Mode::Default).unwrap();
        assert_eq!(
            names(d.path()),
            ["s.json", "s.json.tmp.2.0", "t.json.tmp.1.0"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_write_keeps_the_old_mode_and_makes_new_files_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let n = d.path().join("new");
        write_atomic_private(&n, b"x").unwrap();
        assert_eq!(mode(&n), 0o600);
        let o = d.path().join("old");
        std::fs::write(&o, "x").unwrap();
        std::fs::set_permissions(&o, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic_private(&o, b"y").unwrap();
        assert_eq!(mode(&o), 0o640);
        assert_eq!(std::fs::read_to_string(&o).unwrap(), "y");
    }

    #[test]
    fn a_failed_rename_onto_a_directory_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("dir");
        std::fs::create_dir(&p).unwrap();
        assert!(write_atomic(&p, b"x", Mode::Default).is_err());
        assert_eq!(names(d.path()), ["dir"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_dir_is_an_error_and_leaves_the_old_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("s.json");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        // root ignores directory modes: skip when a write still works
        let probe = d.path().join("probe");
        if std::fs::write(&probe, "x").is_ok() {
            std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let r = write_atomic(&p, b"new", Mode::Default);
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old");
        assert_eq!(names(d.path()), ["s.json"]);
    }

    #[test]
    fn a_write_into_a_missing_directory_is_an_error_and_makes_no_directory() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("no").join("s.json");
        assert!(write_atomic(&p, b"x", Mode::Default).is_err());
        assert!(!d.path().join("no").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_sets_the_exact_mode_on_a_new_and_on_a_replaced_file() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let p = d.path().join("run.sh");
        write_atomic(&p, b"#!/bin/sh\n", Mode::Exact(0o755)).unwrap();
        assert_eq!(mode(&p), 0o755);
        assert!(is_executable(&p));
        // a replaced file gets the mode that is asked for, not the old one
        write_atomic(&p, b"y", Mode::Exact(0o600)).unwrap();
        assert_eq!(
            (mode(&p), std::fs::read(&p).unwrap()),
            (0o600, b"y".to_vec())
        );
        assert!(!is_executable(&p));
        assert_eq!(names(d.path()), ["run.sh"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_target_is_replaced_and_not_written_through() {
        let d = tempfile::tempdir().unwrap();
        let victim = d.path().join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let p = d.path().join("s.json");
        std::os::unix::fs::symlink(&victim, &p).unwrap();
        write_atomic(&p, b"new", Mode::Default).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        set_mode(&p, mode).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_wants_a_regular_file_with_an_execute_bit() {
        let d = tempfile::tempdir().unwrap();
        assert!(is_executable(&script(d.path(), "x", 0o755)));
        assert!(!is_executable(&script(d.path(), "plain", 0o644)));
        // a directory has the bit, but is not a program
        assert!(!is_executable(d.path()));
        assert!(!is_executable(&d.path().join("missing")));
    }

    #[cfg(unix)]
    #[test]
    fn find_on_path_takes_the_first_executable_and_skips_the_rest() {
        let (a, b, c) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        script(a.path(), "tool", 0o644); // not executable: skipped
        let want = script(b.path(), "tool", 0o755);
        script(c.path(), "tool", 0o755); // later: not taken
        let path = std::env::join_paths([
            Path::new(""), // an empty entry is skipped
            a.path(),
            b.path(),
            c.path(),
        ])
        .unwrap();
        assert_eq!(find_on_path("tool", &path), Some(want));
        assert_eq!(find_on_path("other", &path), None);
        assert_eq!(find_on_path("tool", std::ffi::OsStr::new("")), None);
    }
}

/// An advisory exclusive lock on a lock file; it is released when the guard is dropped (or the
/// process ends). It uses the file locking of the standard library, so it holds on every
/// platform (`flock` on Unix, `LockFileEx` on Windows).
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

/// Take the exclusive lock of `lock_path` (the file is created, mode 0600 on Unix, with its
/// parent directory). The call waits while another process holds it; every holder keeps it only
/// for one short read-modify-write, so the wait is short. An error means the lock file cannot be
/// opened: the caller decides whether to go on without the lock.
pub fn lock_exclusive(lock_path: &Path) -> io::Result<FileLock> {
    if let Some(d) = lock_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d)?;
    }
    let mut open = OpenOptions::new();
    open.create(true).truncate(false).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    let file = open.open(lock_path)?;
    loop {
        match file.lock() {
            Ok(()) => return Ok(FileLock { _file: file }),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}
