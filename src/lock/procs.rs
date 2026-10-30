//! Process facts behind one trait, so the stale rules run in a test with a table of fake
//! processes. `SystemProcs` is the real one (Unix only): `kill(pid, 0)`, `ps`, and the two
//! test hooks `HOST_LOCK_PS_CMD` and `HOST_LOCK_PARENT_CMD`.

/// Is a process alive?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alive {
    Yes,
    No,
}

impl Alive {
    /// The answer of `kill(pid, 0)`: `rc` is its result, `errno` the error when `rc` is not 0.
    /// EPERM means the process EXISTS and belongs to another user (a CI runner): it is alive
    ///. Only ESRCH (and a pid that cannot exist) means gone.
    pub fn from_kill(rc: i32, errno: i32) -> Alive {
        const EPERM: i32 = 1;
        if rc == 0 || errno == EPERM {
            Alive::Yes
        } else {
            Alive::No
        }
    }
}

/// The parent pid and the command name of one process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    /// `None` when the parent is not known.
    pub ppid: Option<u32>,
    pub comm: String,
}

pub trait Procs {
    /// The pid of this process.
    fn pid(&self) -> u32;
    /// The parent pid of this process.
    fn ppid(&self) -> u32;
    fn alive(&self, pid: u32) -> Alive;
    /// `None` when the process is unknown.
    fn lookup(&self, pid: u32) -> Option<ProcInfo>;
    /// The first line of the process list that names a CI runner job (`Runner.Worker`).
    fn ci_process_line(&self) -> Option<String>;
    /// The command line of a process, as `ps -o command=` shows it. `None` when the process is
    /// unknown.
    fn command(&self, pid: u32) -> Option<String>;
}

/// Does `line` hold `Runner.Worker` as a command: at the start or after a `/`, and at the end
/// or before a blank? (The pattern `(^|/)Runner\.Worker( |$)`.)
pub fn is_runner_line(line: &str) -> bool {
    const NAME: &str = "Runner.Worker";
    line.match_indices(NAME).any(|(i, _)| {
        let before_ok = i == 0 || line.as_bytes()[i - 1] == b'/';
        let after = line.as_bytes().get(i + NAME.len());
        before_ok && matches!(after, None | Some(b' '))
    })
}

/// `PPID COMM` as the test hook prints it (and as `ps -o ppid= -o comm=` does): the first word
/// is the parent pid, the rest is the name.
pub fn parse_ps_line(line: &str) -> Option<ProcInfo> {
    let t = line.trim_start();
    if t.is_empty() {
        return None;
    }
    let (first, rest) = match t.find(char::is_whitespace) {
        Some(i) => (&t[..i], t[i..].trim_start()),
        None => (t, ""),
    };
    let ppid = (!first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()))
        .then(|| first.parse::<u32>().ok())
        .flatten();
    Some(ProcInfo {
        ppid,
        comm: rest.trim_end().to_string(),
    })
}

#[cfg(unix)]
pub use system::{SystemProcs, ps};

#[cfg(unix)]
mod system {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    pub struct SystemProcs;

    /// The longest that one `ps` may take.
    const PS_LIMIT: Duration = Duration::from_secs(10);

    /// The one reader of `ps`: its standard output when it exits 0, else the reason as text.
    /// The standard input and error are closed or read; a `ps` that hangs is killed.
    pub fn ps(args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("ps");
        cmd.args(args);
        match crate::spawn::output_within(&mut cmd, PS_LIMIT) {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).to_string()),
            Ok(o) => Err(format!("ps exit {:?}", o.status.code())),
            Err(e) => Err(e.to_string()),
        }
    }

    fn sh_output(script: &str, arg: Option<&str>) -> Option<String> {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        if let Some(a) = arg {
            c.arg("sh").arg(a);
        }
        let o = c.stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(String::from_utf8_lossy(&o.stdout).to_string())
    }

    impl Procs for SystemProcs {
        fn pid(&self) -> u32 {
            std::process::id()
        }

        fn ppid(&self) -> u32 {
            // SAFETY: getppid has no arguments and cannot fail.
            unsafe { libc::getppid() as u32 }
        }

        fn alive(&self, pid: u32) -> Alive {
            let Ok(p) = i32::try_from(pid) else {
                return Alive::No;
            };
            // SAFETY: signal 0 only checks that the process exists.
            let rc = unsafe { libc::kill(p, 0) };
            let errno = if rc == 0 {
                0
            } else {
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            };
            Alive::from_kill(rc, errno)
        }

        fn lookup(&self, pid: u32) -> Option<ProcInfo> {
            let text = match std::env::var("HOST_LOCK_PARENT_CMD") {
                Ok(cmd) if !cmd.is_empty() => sh_output(&cmd, Some(&pid.to_string()))?,
                _ => ps(&["-o", "ppid=", "-o", "comm=", "-p", &pid.to_string()]).ok()?,
            };
            parse_ps_line(text.lines().next()?)
        }

        fn ci_process_line(&self) -> Option<String> {
            let script = match std::env::var("HOST_LOCK_PS_CMD") {
                Ok(c) if !c.is_empty() => c,
                _ => "ps -axo command=".to_string(),
            };
            sh_output(&script, None)?
                .lines()
                .find(|l| is_runner_line(l))
                .map(str::to_string)
        }

        fn command(&self, pid: u32) -> Option<String> {
            let text = ps(&["-o", "command=", "-p", &pid.to_string()]).ok()?;
            Some(text.trim().to_string())
        }
    }
}

/// A table of fake processes (tests).
#[cfg(test)]
#[derive(Default)]
pub struct FakeProcs {
    pub me: u32,
    pub parent: u32,
    pub live: std::collections::BTreeSet<u32>,
    pub table: std::collections::BTreeMap<u32, ProcInfo>,
    pub runner_line: std::cell::RefCell<Option<String>>,
}

#[cfg(test)]
impl FakeProcs {
    pub fn new() -> Self {
        FakeProcs {
            me: 5000,
            parent: 4000,
            ..Default::default()
        }
    }
    pub fn alive_pids(mut self, pids: &[u32]) -> Self {
        self.live.extend(pids);
        self
    }
    pub fn with(mut self, pid: u32, ppid: u32, comm: &str) -> Self {
        self.table.insert(
            pid,
            ProcInfo {
                ppid: Some(ppid),
                comm: comm.to_string(),
            },
        );
        self.live.insert(pid);
        self
    }
    pub fn set_ci(&self, line: Option<&str>) {
        *self.runner_line.borrow_mut() = line.map(str::to_string);
    }
}

#[cfg(test)]
impl Procs for FakeProcs {
    fn pid(&self) -> u32 {
        self.me
    }
    fn ppid(&self) -> u32 {
        self.parent
    }
    fn alive(&self, pid: u32) -> Alive {
        if self.live.contains(&pid) {
            Alive::Yes
        } else {
            Alive::No
        }
    }
    fn lookup(&self, pid: u32) -> Option<ProcInfo> {
        self.table.get(&pid).cloned()
    }
    fn ci_process_line(&self) -> Option<String> {
        self.runner_line.borrow().clone()
    }
    fn command(&self, pid: u32) -> Option<String> {
        self.table.get(&pid).map(|i| i.comm.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eperm_counts_as_alive() {
        // kill(2) failed with EPERM (1): the process exists, it is another user's
        assert_eq!(Alive::from_kill(-1, 1), Alive::Yes);
        assert_eq!(Alive::from_kill(0, 0), Alive::Yes);
        // the other direction: ESRCH (3) is gone, and so is any other failure
        assert_eq!(Alive::from_kill(-1, 3), Alive::No);
        assert_eq!(Alive::from_kill(-1, 22), Alive::No);
    }

    #[cfg(unix)]
    #[test]
    fn eperm_a_live_pid_of_another_user_is_alive_for_the_real_procs() {
        // pid 1 belongs to root: kill(1, 0) is EPERM for every other user (and 0 for root)
        assert_eq!(SystemProcs.alive(1), Alive::Yes);
        // a pid that cannot exist is gone
        assert_eq!(SystemProcs.alive(u32::MAX), Alive::No);
    }

    #[test]
    fn runner_lines_are_matched_as_a_command_not_as_a_substring() {
        assert!(is_runner_line(
            "/home/runner/actions-runner/bin/Runner.Worker spawnclient 100 101"
        ));
        assert!(is_runner_line("Runner.Worker"));
        assert!(is_runner_line("Runner.Worker spawnclient"));
        // pass direction above, fail direction below
        assert!(!is_runner_line("/bin/OtherRunner.Worker x"));
        assert!(!is_runner_line("/home/x/Runner.Workers x"));
        assert!(!is_runner_line("vim Runner.Worker.log"));
        assert!(!is_runner_line(""));
    }

    #[test]
    fn h17d_the_walk_line_gives_parent_and_name_with_a_full_path() {
        let p = parse_ps_line("  300 /Users/x/actions-runner/bin/Runner.Worker").unwrap();
        assert_eq!(p.ppid, Some(300));
        assert_eq!(p.comm, "/Users/x/actions-runner/bin/Runner.Worker");
        let q = parse_ps_line("abc comm").unwrap();
        assert_eq!(q.ppid, None);
        assert!(parse_ps_line("   ").is_none());
    }
}
