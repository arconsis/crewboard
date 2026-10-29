//! The wake command at run time: build it, start it in its own process group with its output in
//! the log, and watch it until it ends, until the time budget ends it, or until a signal stops
//! it. A heartbeat thread keeps the board told while it runs.

use super::flow::{Flow, Reason};
use super::heartbeat;
use super::job as wake_job;
use super::procs::stop_group;
use crate::wake::steps::code_of;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// How the wake command ended.
pub enum Ended {
    Exit(u8),
    /// The time budget ended it; the command is stopped.
    Budget,
    /// A signal stopped the run (the exit code); the command is stopped first.
    Signal(u8),
}

impl Flow<'_, '_> {
    /// The command of the wake, not started: the words of the agent in its work dir, or
    /// `sh -c CMD`. The settings of the env file come first; what the wake itself sets comes after
    /// them, so a key of the env file cannot replace the reason, the events file, the `gh` shim
    /// or the grant for `lock run`.
    pub(super) fn wake_child(
        &self,
        cmd: &str,
        reason: Reason,
        env_file: Option<&crate::agent_env::AgentEnv>,
    ) -> Command {
        let mut c = match &self.s.direct {
            Some(d) => {
                let mut c = Command::new(wake_job::program(&d.argv[0], d.path.as_deref()));
                c.args(&d.argv[1..]).current_dir(&d.cwd);
                c
            }
            None => {
                let mut c = Command::new("sh");
                c.arg("-c").arg(cmd);
                c
            }
        };
        if let Some(env) = env_file {
            env.apply_to_wake(&mut c);
        }
        c.env("CREWBOARD_WAKE_REASON", reason.name())
            .env("CREWBOARD_RESULTS_DIR", &self.s.paths.results);
        if let Some(ev) = &self.s.events_file {
            c.env("CREWBOARD_EVENTS_FILE", ev);
        }
        if let Some(d) = &self.s.config_dir {
            c.env("CREWBOARD_CONFIG_DIR", d);
        }
        if let Some(d) = &self.s.direct {
            c.envs(d.env.iter().map(|(k, v)| (k, v)));
        }
        c
    }

    /// The wake command of the agent (its words, in its work dir), or `sh -c CMD`, in its own
    /// process group, its output appended to the log, no input. Its environment: the reason, the
    /// results dir, the events file, the config dir, what the agent options add (the `gh` shim,
    /// the grant for `lock run`) and the settings of the env file.
    pub(super) fn run_wake_command(&self, cmd: &str, reason: Reason) -> Ended {
        let mut child = match self.spawn_wake(cmd, reason) {
            Ok(c) => c,
            Err(e) => {
                self.log
                    .line(&format!("cannot start the wake command: {e}"));
                return Ended::Exit(127);
            }
        };
        self.watch(&mut child)
    }

    fn spawn_wake(&self, cmd: &str, reason: Reason) -> std::io::Result<Child> {
        let mut c = self.wake_child(cmd, reason, crate::agent_env::current());
        c.stdin(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.process_group(0);
        }
        let open = || {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.s.paths.log)
        };
        match (open(), open()) {
            (Ok(o), Ok(e)) => {
                c.stdout(o).stderr(e);
            }
            _ => {
                c.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        crate::spawn::spawn_retrying(&mut c)
    }

    /// Wait for the child, with the heartbeat ticker beside it. The budget is real elapsed time
    /// of a real process (`Instant`, not the wall clock of the [`Clock`](crate::clock::Clock)
    /// seam): a jump of the wall clock must not stop a wake, and the check polls every 50 ms.
    #[allow(
        clippy::disallowed_methods,
        reason = "real elapsed time of a child process"
    )]
    fn watch(&self, child: &mut Child) -> Ended {
        let note = |m: &str| self.log.line(m);
        let started = std::time::Instant::now();
        let budget = (self.s.max_wake.secs > 0).then(|| Duration::from_secs(self.s.max_wake.secs));
        let every = Duration::from_secs(self.s.heartbeat_every);
        let beat = (self.s.heartbeat_every > 0)
            .then_some(self.heartbeat_steps)
            .flatten();
        let (ctl, stop_rx, report_tx) = heartbeat::Control::new();
        // the ticker keeps the heartbeat fresh while a long wake runs; it stops with the wake
        std::thread::scope(|sc| {
            let ticker = beat.map(|make| {
                sc.spawn(move || {
                    let mut steps = make();
                    heartbeat::run(
                        &mut || heartbeat::wait_on(&stop_rx, every),
                        &mut *steps,
                        &mut |m| {
                            // the wake has ended when nobody reads any more
                            report_tx.send(m).ok(); // the receiver may be gone
                        },
                    );
                })
            });
            let end = loop {
                for m in ctl.reports.try_iter() {
                    self.log.line(&m);
                }
                match child.try_wait() {
                    Ok(Some(st)) => break Ended::Exit(code_of(st)),
                    Ok(None) => {}
                    Err(e) => {
                        self.log
                            .line(&format!("cannot wait for the wake command: {e}"));
                        break Ended::Exit(127);
                    }
                }
                if let Some(code) = self.signalled() {
                    stop_group(child, &note);
                    break Ended::Signal(code);
                }
                if budget.is_some_and(|b| started.elapsed() >= b) {
                    stop_group(child, &note);
                    break Ended::Budget;
                }
                crate::clock::pause(Duration::from_millis(50));
            };
            ctl.stop();
            if let Some(t) = ticker
                && t.join().is_err()
            {
                self.log.line("the heartbeat thread failed");
            }
            for m in ctl.reports.try_iter() {
                self.log.line(&m);
            }
            end
        })
    }
}
