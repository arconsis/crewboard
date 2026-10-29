//! Helpers for the tests of `crewboard lock`: a rig with its own temp dir, a fake process list for
//! the CI detector, a live pid that is not an ancestor of any lock call, and the run of the real
//! binary. Nothing here changes the environment of the test process.

#![allow(dead_code)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const BIN: &str = env!("CARGO_BIN_EXE_crewboard");
pub const HOST: &str = "hostA";
pub const RUNNER_LINE: &str = "/home/runner/actions-runner/bin/Runner.Worker spawnclient 100 101";

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn all(&self) -> String {
        format!(
            "exit {}\nstdout:\n{}\nstderr:\n{}",
            self.code, self.stdout, self.stderr
        )
    }
}

pub fn now() -> i64 {
    crewboard::clock::now_epoch()
}

pub struct Rig {
    tmp: tempfile::TempDir,
    live: Child,
}

impl Default for Rig {
    fn default() -> Self {
        Self::new()
    }
}

impl Rig {
    pub fn new() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("ps.txt"), "").unwrap();
        // a live process that is NOT an ancestor of any lock call
        let live = Command::new("sleep")
            .arg("3000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Rig { tmp, live }
    }

    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// The path of a name inside the rig.
    pub fn p(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    pub fn s(&self, name: &str) -> String {
        self.p(name).to_string_lossy().to_string()
    }

    pub fn live(&self) -> u32 {
        self.live.id()
    }

    /// Put a `Runner.Worker` line in the fake process list, or take it out.
    pub fn set_runner(&self, present: bool) {
        std::fs::write(
            self.p("ps.txt"),
            if present {
                format!("{RUNNER_LINE}\n")
            } else {
                String::new()
            },
        )
        .unwrap();
    }

    /// `crewboard lock <args>` with a clean environment for the lock.
    pub fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.arg("lock")
            .args(args)
            .env("HOST_LOCK_HOST", HOST)
            .env("HOST_LOCK_PS_CMD", format!("cat {}", self.s("ps.txt")))
            .env_remove("CREWBOARD_HOST_LOCK")
            .env_remove("CREWBOARD_HOST_LOCK_PID")
            .env_remove("CREWBOARD_CONFIG_DIR")
            .env_remove("HOST_LOCK_MAX_AGE")
            .env_remove("HOST_LOCK_EMPTY_GRACE")
            .env_remove("HOST_LOCK_BOARD_TTL")
            .env_remove("HOST_LOCK_PARENT_CMD")
            .env_remove("HOST_LOCK_SUPERVISE")
            .stdin(Stdio::null());
        c
    }

    pub fn run(&self, args: &[&str]) -> Out {
        out_of(self.cmd(args).output().expect("run crewboard lock"))
    }

    /// Plant a valid owner file.
    pub fn plant(&self, name: &str, agent: &str, host: &str, pid: u32, epoch: i64) {
        plant(&self.p(name), agent, host, &pid.to_string(), epoch);
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.live.kill();
        let _ = self.live.wait();
    }
}

pub fn out_of(o: std::process::Output) -> Out {
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).to_string(),
        stderr: String::from_utf8_lossy(&o.stderr).to_string(),
    }
}

pub fn plant(dir: &Path, agent: &str, host: &str, pid: &str, epoch: i64) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("owner"),
        format!(
            "agent={agent}\nhost={host}\npid={pid}\nstarted=planted\nstarted_epoch={epoch}\nreason=planted\n"
        ),
    )
    .unwrap();
}

/// A pid of a process that has ended.
pub fn dead_pid() -> u32 {
    let mut c = Command::new("true").spawn().unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

pub fn owner_text(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("owner")).unwrap_or_default()
}

/// The value of a key of the owner file.
pub fn owner_field(dir: &Path, key: &str) -> Option<String> {
    owner_text(dir)
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")).map(str::to_string))
}

/// Wait until `f` is true, at most `secs`.
pub fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    f()
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    let rc = unsafe { libc::kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
