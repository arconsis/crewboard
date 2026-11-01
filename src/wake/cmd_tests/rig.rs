//! The rig of a run: a temp dir, a clock, a readable standard error, and the file helpers
//! of the tests.

use super::*;

#[derive(Clone, Default)]
pub(super) struct Buf(pub(super) Rc<RefCell<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A temp dir, a clock and a standard error that a test can read.
pub(super) struct Rig {
    pub(super) tmp: tempfile::TempDir,
    pub(super) clock: FakeClock,
    pub(super) err: Buf,
}

impl Rig {
    pub(super) fn new() -> Rig {
        Rig {
            tmp: tempfile::tempdir().unwrap(),
            clock: FakeClock::at(NOW),
            err: Buf::default(),
        }
    }

    pub(super) fn p(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    /// True when no temp file `<name>.tmp*` of an atomic write is left in the rig dir.
    pub(super) fn has_no_temp_of(&self, name: &str) -> bool {
        let prefix = format!("{name}.tmp");
        let mut n = 0;
        for e in std::fs::read_dir(self.tmp.path()).unwrap().flatten() {
            n += 1;
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                return false;
            }
        }
        n > 0 // the dir is read: a listing that is empty proves nothing
    }

    pub(super) fn state(&self) -> PathBuf {
        self.p("state")
    }

    pub(super) fn lock(&self) -> PathBuf {
        self.state().join("wake.lock")
    }

    pub(super) fn log_path(&self) -> PathBuf {
        self.state().join("wake.log")
    }

    pub(super) fn log(&self) -> String {
        std::fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    pub(super) fn stderr(&self) -> String {
        String::from_utf8(self.err.0.borrow().clone()).unwrap()
    }

    /// The wake command that records each wake as a line in `wakes`.
    pub(super) fn wake_cmd(&self) -> String {
        format!("echo wake >> '{}'", self.p("wakes").display())
    }

    pub(super) fn wakes(&self) -> usize {
        std::fs::read_to_string(self.p("wakes"))
            .map(|t| t.lines().count())
            .unwrap_or(0)
    }

    pub(super) fn write_state(&self, name: &str, text: &str) {
        std::fs::create_dir_all(self.state()).unwrap();
        std::fs::write(self.state().join(name), text).unwrap();
    }

    pub(super) fn state_text(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.state().join(name)).ok()
    }

    /// The settings for the flags: the defaults of a test, then `extra` (a flag in `extra`
    /// replaces the default of the same name).
    pub(super) fn settings(
        &self,
        extra: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Settings, BadFlag> {
        let wake = self.wake_cmd();
        let state = self.state().to_string_lossy().to_string();
        let mut argv: Vec<String> = Vec::new();
        for (flag, val) in [
            ("--state-dir", state.as_str()),
            ("--wake-cmd", wake.as_str()),
            ("--retry-delays", ""),
        ] {
            let own_job = flag == "--wake-cmd" && extra.contains(&"--runtime");
            if !extra.contains(&flag) && !own_job {
                argv.push(flag.into());
                argv.push(val.into());
            }
        }
        argv.extend(extra.iter().map(|s| s.to_string()));
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let a = parse(&refs);
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let get = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        match resolve(&a, &GlobalFlags::default(), &get, None)? {
            Plan::Run(s) => Ok(*s),
            Plan::PrintStateDir(_) | Plan::PrintCmd(_) => panic!("print"),
        }
    }

    /// One run with the flags; the exit code.
    pub(super) fn run(&self, steps: &mut Scripted, extra: &[&str]) -> u8 {
        self.run_env(steps, extra, &[])
    }

    pub(super) fn run_env(&self, steps: &mut Scripted, extra: &[&str], env: &[(&str, &str)]) -> u8 {
        let s = self.settings(extra, env).unwrap();
        self.run_settings(steps, &s)
    }

    pub(super) fn run_settings(&self, steps: &mut Scripted, s: &Settings) -> u8 {
        self.run_signals(steps, s, &|| None)
    }

    pub(super) fn run_signals(
        &self,
        steps: &mut Scripted,
        s: &Settings,
        signals: &dyn Fn() -> Option<i32>,
    ) -> u8 {
        std::fs::create_dir_all(&s.paths.dir).unwrap();
        let log = Log::with_err(s.paths.log.clone(), &self.clock, Box::new(self.err.clone()));
        Flow {
            s,
            steps,
            clock: &self.clock,
            pids: &SystemProcs,
            log: &log,
            host: "testhost",
            sig: Cell::new(None),
            signals,
            heartbeat_steps: None,
        }
        .execute()
    }
}

/// A pid of a process that is gone.
pub(super) fn dead_pid() -> u32 {
    let mut c = std::process::Command::new("sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let pid = c.id();
    c.wait().unwrap();
    pid
}

pub(super) fn plant_lock(dir: &Path, pid: u32, started: i64) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("owner"), "old-owner\n").unwrap();
    std::fs::write(dir.join("pid"), format!("{pid}\n")).unwrap();
    std::fs::write(dir.join("started"), format!("{started}\n")).unwrap();
}

pub(super) fn rep(n: usize, l: &str) -> Vec<&str> {
    vec![l; n]
}

pub(super) fn ev(rig: &Rig) -> String {
    rig.p("ev.txt").to_string_lossy().to_string()
}

pub(super) fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// A wake command that copies the events file it was given.
pub(super) fn copy_events(rig: &Rig, to: &str) -> String {
    format!(
        "cp \"$CREWBOARD_EVENTS_FILE\" '{}'; echo wake >> '{}'",
        rig.p(to).display(),
        rig.p("wakes").display()
    )
}
