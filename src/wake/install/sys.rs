//! What `wake install` asks of the host: files and scheduler commands. A trait, so that the
//! plan is tested without a scheduler and the component tests run fake `systemctl`, `launchctl`
//! and `crontab` programs from the PATH.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// The result of one command. A program that cannot start is `ok: false` with the reason in
/// `stderr`.
#[derive(Debug, Clone, Default)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

pub trait Sys {
    fn exists(&self, p: &Path) -> bool;
    fn is_dir(&self, p: &Path) -> bool;
    fn is_executable(&self, p: &Path) -> bool;
    /// The text of a file (at most 256 KiB, lossy); `None` when it cannot be read.
    fn read_text(&self, p: &Path) -> Option<String>;
    /// The entries of a directory, sorted; empty when it cannot be read.
    fn list(&self, dir: &Path) -> Vec<std::path::PathBuf>;
    fn run(&self, program: &str, args: &[String], stdin: Option<&str>) -> Ran;
}

/// The real host.
pub struct RealSys;

impl Sys for RealSys {
    fn exists(&self, p: &Path) -> bool {
        std::fs::symlink_metadata(p).is_ok()
    }

    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }

    fn is_executable(&self, p: &Path) -> bool {
        crate::fsutil::is_executable(p)
    }

    fn read_text(&self, p: &Path) -> Option<String> {
        use std::io::Read;
        let mut buf = Vec::new();
        std::fs::File::open(p)
            .ok()?
            .take(256 * 1024)
            .read_to_end(&mut buf)
            .ok()?;
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    fn list(&self, dir: &Path) -> Vec<std::path::PathBuf> {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut v: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        v.sort();
        v
    }

    fn run(&self, program: &str, args: &[String], stdin: Option<&str>) -> Ran {
        let mut c = Command::new(program);
        c.args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = match crate::spawn::spawn_retrying(&mut c) {
            Ok(ch) => ch,
            Err(e) => {
                return Ran {
                    ok: false,
                    stdout: String::new(),
                    stderr: format!("cannot start {program}: {e}"),
                };
            }
        };
        if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
            // a program that exits before it reads shows up in its status, not here
            if let Err(e) = pipe.write_all(text.as_bytes()) {
                return Ran {
                    ok: false,
                    stdout: String::new(),
                    stderr: format!("cannot write to {program}: {e}"),
                };
            }
        }
        match child.wait_with_output() {
            Ok(o) => Ran {
                ok: o.status.success(),
                stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            },
            Err(e) => Ran {
                ok: false,
                stdout: String::new(),
                stderr: format!("cannot wait for {program}: {e}"),
            },
        }
    }
}
