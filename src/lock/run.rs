//! The subcommands of `crewboard lock`: the dispatch, `--pid runner`, and `run` (a child that
//! holds the lock for exactly as long as it lives).

use super::Session;
use super::args::{Cmd, Parsed, PidArg, RunCtx};
#[cfg(unix)]
use crate::base::ignore::best_effort_output;

/// The nearest `Runner.Worker` process among this process and its parents.
pub fn runner_pid(s: &Session) -> Option<u32> {
    let mut p = s.procs.pid();
    for _ in 0..64 {
        if p <= 1 {
            return None;
        }
        let info = s.procs.lookup(p);
        let comm = info.as_ref().map(|i| i.comm.as_str()).unwrap_or("");
        if comm.rsplit('/').next() == Some("Runner.Worker") {
            return Some(p);
        }
        p = info?.ppid?;
    }
    None
}

const NO_RUNNER: &str = "--pid runner: no Runner.Worker process among the parents of this script (not a GitHub Actions job?)";

/// Run one subcommand. The result is the exit code.
pub fn dispatch(s: &mut Session, p: &Parsed, ctx: &RunCtx) -> i32 {
    // `--pid runner` is judged before anything is written
    let pid_arg: Option<String> = match (&p.pid, p.cmd) {
        (PidArg::Number(n), _) => Some(n.clone()),
        (PidArg::Runner, Cmd::Take | Cmd::CiWait) => match runner_pid(s) {
            Some(r) => Some(r.to_string()),
            None => {
                s.log.say(NO_RUNNER);
                return 64;
            }
        },
        _ => None,
    };
    match p.cmd {
        Cmd::Take => {
            let pid = pid_arg.unwrap_or_else(|| s.procs.ppid().to_string());
            s.take(&pid)
        }
        Cmd::Release => s.release(),
        Cmd::Status => s.status(),
        Cmd::Break => s.break_if_dead(),
        Cmd::CiWait => s.ci_wait(pid_arg.as_deref()),
        Cmd::CiDone => s.ci_done(),
        Cmd::CiStatus => s.ci_status(),
        Cmd::Run => {
            #[cfg(unix)]
            {
                run_job(s, p, ctx)
            }
            #[cfg(not(unix))]
            {
                let _ = ctx;
                2
            }
        }
    }
}

/// `run`: take the lock with this process, run the command as a child, release.
#[cfg(unix)]
fn run_job(s: &mut Session, p: &Parsed, ctx: &RunCtx) -> i32 {
    use super::{detach, sys};
    use std::io::Write;
    use std::time::Duration;

    if p.detach
        && let Some(parent) = &ctx.supervise
    {
        return detach::supervise(s, p, ctx, parent);
    }
    let own = s.procs.pid().to_string();
    let rc = s.take(&own);
    if rc != 0 {
        return rc;
    }
    if p.detach {
        if s.nested {
            s.log
                .say("cannot detach a job inside a lock that your parent process holds");
            return 64;
        }
        return detach::start(s, p, ctx);
    }
    // a stop that came while the lock was taken: do not start the command (the gate's caller
    // handles signals; `crewboard lock run` itself installs its handler only here)
    if ctx.output.is_some() && sys::stop_requested().is_some() {
        let _ = s.release();
        return 143;
    }
    sys::catch_term_int_hup();
    let spawned = spawn_job(s, p, ctx);
    let mut got_signal = false;
    let mut forwarded = false;
    let code = match spawned {
        Err(e) => {
            // a shell reports 127 for a command it does not find, 126 for one it cannot run
            let line = format!("{}: {}", p.run_cmd[0], spawn_error_text(&e));
            // a caller that collects the output (the gate) gets the error text as the output
            let in_file = ctx
                .output
                .as_ref()
                .is_some_and(|f| std::fs::write(f, format!("{line}\n")).is_ok());
            if !in_file {
                best_effort_output(writeln!(std::io::stderr(), "{line}"));
            }
            if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            }
        }
        Ok(mut child) => loop {
            match child.try_wait() {
                Ok(Some(st)) => break crate::spawn::code_of(st),
                Ok(None) => {}
                Err(_) => break 1,
            }
            // The job is in our process group: a TERM or INT of the group (a wake budget stop,
            // Ctrl-C) reaches it already, so it is NOT forwarded (a second INT would force a
            // quit). We wait for the job, release the lock, and exit 143.
            if let Some(sig) = sys::caught() {
                got_signal = true;
                // A caller that collects the output (the gate) runs the job in its own process
                // group, so a signal of this process does not reach it: pass it on, once.
                if ctx.output.is_some() && !forwarded {
                    sys::signal_group(child.id(), sig);
                    forwarded = true;
                }
            }
            s.clock.sleep(Duration::from_millis(50));
        },
    };
    let _ = s.release();
    if got_signal { 143 } else { code }
}

/// Start the command of `run` with the grant for its children. The directory and the output file
/// come from `ctx`.
#[cfg(unix)]
fn spawn_job(s: &Session, p: &Parsed, ctx: &RunCtx) -> std::io::Result<std::process::Child> {
    use super::phys_dir;
    use std::process::Command;

    let grant_dir = phys_dir(&s.dir.to_string_lossy());
    let grant_pid = s.lock_pid.clone().unwrap_or_default();
    let mut c = Command::new(&p.run_cmd[0]);
    c.args(&p.run_cmd[1..])
        .env("CREWBOARD_HOST_LOCK", &grant_dir)
        .env("CREWBOARD_HOST_LOCK_PID", &grant_pid);
    for v in &ctx.env_remove {
        c.env_remove(v);
    }
    if let Some(d) = &ctx.cwd {
        c.current_dir(d);
    }
    if let Some(path) = &ctx.output {
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        let out = std::fs::File::create(path).map_err(|e| {
            std::io::Error::other(format!(
                "cannot write the output file {}: {e}",
                path.display()
            ))
        })?;
        c.stderr(out.try_clone()?).stdout(out);
    }
    crate::spawn::spawn_retrying(&mut c)
}

#[cfg(unix)]
fn spawn_error_text(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "command not found".to_string(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => e.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Clock, FakeClock};
    use crate::lock::board::BoardCheck;
    use crate::lock::log::Log;
    use crate::lock::procs::FakeProcs;
    use crate::lock::{Grant, Opts};
    use std::path::Path;
    use std::time::Duration;

    const NOW: i64 = 1_792_836_000;

    /// A fake clock that calls a hook at every sleep (with the number of the sleep).
    struct OnSleep<'a> {
        inner: &'a FakeClock,
        hook: Box<dyn Fn(u32) + 'a>,
        n: std::cell::Cell<u32>,
    }

    impl<'a> OnSleep<'a> {
        fn new(inner: &'a FakeClock, hook: impl Fn(u32) + 'a) -> Self {
            OnSleep {
                inner,
                hook: Box::new(hook),
                n: Default::default(),
            }
        }
    }

    impl Clock for OnSleep<'_> {
        fn now(&self) -> i64 {
            self.inner.now()
        }
        fn sleep(&self, d: Duration) {
            self.n.set(self.n.get() + 1);
            (self.hook)(self.n.get());
            self.inner.sleep(d);
        }
    }

    struct Rig<'a> {
        clock: &'a dyn Clock,
        procs: &'a FakeProcs,
        log: &'a Log<'a>,
    }

    fn opts(owner: &str) -> Opts {
        Opts {
            owner: owner.into(),
            reason: "r".into(),
            wait: 0,
            poll: Duration::from_secs(5),
            expect: None,
            max_age: 14400,
            empty_grace: 30,
            ci_max_age: 21600,
            ci_mode: false,
            gate: false,
        }
    }

    fn session<'a>(dir: &Path, o: Opts, rig: &Rig<'a>) -> Session<'a> {
        Session {
            dir: dir.to_path_buf(),
            host: "hostA".into(),
            opts: o,
            clock: rig.clock,
            procs: rig.procs,
            log: rig.log,
            board: BoardCheck::new(None, 60, true, false, rig.log),
            grant: Grant::default(),
            nested: false,
            lock_pid: None,
            detached: false,
        }
    }

    fn plant(dir: &Path, agent: &str, host: &str, pid: &str, epoch: i64) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("owner"),
            format!("agent={agent}\nhost={host}\npid={pid}\nstarted=planted\nstarted_epoch={epoch}\nreason=planted\n"),
        )
        .unwrap();
    }

    #[test]
    fn h1_a_free_lock_is_taken_with_a_complete_owner_file() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("a@x");
        o.reason = "cargo test".into();
        let mut s = session(&lock, o, &rig);
        assert_eq!(s.take("77"), 0);
        let text = std::fs::read_to_string(lock.join("owner")).unwrap();
        assert!(
            text.starts_with("agent=a@x\nhost=hostA\npid=77\nstarted=20"),
            "{text}"
        );
        assert!(text.contains(&format!("started_epoch={NOW}\nreason=cargo test\n")));
    }

    #[test]
    fn h2_a_live_lock_refuses_the_second_taker_owner_unchanged() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "a@x", "hostA", "111", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&lock, opts("b@x"), &rig);
        assert_eq!(s.take("222"), 75);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=a@x")
        );
        assert!(
            log.said().iter().any(|l| l.contains("held by a@x@hostA")),
            "{:?}",
            log.said()
        );
    }

    #[test]
    fn h2b_wait_takes_the_lock_after_the_owner_releases() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "a@x", "hostA", "111", NOW);
        let (base, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        // the owner releases during the third poll
        let l2 = lock.clone();
        let c = OnSleep::new(&base, move |n| {
            if n == 3 {
                std::fs::remove_dir_all(&l2).unwrap();
            }
        });
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("b@x");
        o.wait = 10;
        o.poll = Duration::from_secs(1);
        let mut s = session(&lock, o, &rig);
        assert_eq!(s.take("222"), 0);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=b@x")
        );
        // it waited 3 polls, not the whole 10 s
        assert_eq!(base.now(), NOW + 3);
    }

    #[test]
    fn h2c_the_wait_message_is_shown_once_and_a_wait_over_gives_75() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "a@x", "hostA", "111", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("b@x");
        o.wait = 4;
        o.poll = Duration::from_secs(1);
        let mut s = session(&lock, o, &rig);
        assert_eq!(s.take("222"), 75);
        let said = log.said();
        assert_eq!(
            said.iter()
                .filter(|l| l.starts_with("waiting for lock"))
                .count(),
            1
        );
        assert_eq!(
            said.iter()
                .filter(|l| l.contains("is held by a@x@hostA"))
                .count(),
            1
        );
        // the wait took the time it should: 4 polls of 1 s
        assert_eq!(c.now(), NOW + 4);
    }

    #[test]
    fn e4b_the_wait_message_carries_the_pid0_and_overdue_notes() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "a@x", "hostA", "0", NOW - 560);
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(lock.join("owner"))
            .unwrap();
        std::io::Write::write_all(&mut f, b"expect=300\n").unwrap();
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("b@x");
        o.wait = 2;
        o.poll = Duration::from_millis(300);
        assert_eq!(session(&lock, o, &rig).take("77"), 75);
        let said = log.said();
        let wait = said
            .iter()
            .find(|l| l.starts_with("waiting for lock"))
            .unwrap();
        assert!(
            wait.contains("pid=0: a dead holder cannot be detected"),
            "{wait}"
        );
        assert!(wait.contains("overdue by 5 min"), "{wait}");
        // the final line has the same notes
        assert!(
            said.last().unwrap().contains("overdue by 5 min"),
            "{said:?}"
        );
    }

    #[test]
    fn h3_a_dead_pid_is_taken_over_with_one_log_line() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "old@x", "hostA", "999", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&lock, opts("new@x"), &rig);
        assert_eq!(s.take("77"), 0);
        let said = log.said();
        let takeovers: Vec<_> = said
            .iter()
            .filter(|l| l.contains("took over stale lock"))
            .collect();
        assert_eq!(takeovers.len(), 1, "{said:?}");
        assert!(takeovers[0].contains("old owner old@x@hostA"));
        assert!(takeovers[0].contains("reason=pid-dead (owner pid 999 is gone on host hostA)"));
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=new@x")
        );
        assert!(!d.path().join("l.takeover").exists());
        // no leftover stale dir
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, vec!["l".to_string()]);
    }

    #[test]
    fn h3b_a_live_pid_that_kill_cannot_reach_is_not_stale() {
        // the fake says Alive::Yes for a pid of another user, as the real one does for EPERM
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "other@x", "hostA", "1", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[1]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&lock, opts("new@x"), &rig);
        assert_eq!(s.take("77"), 75);
        assert_eq!(s.status(), 0);
        assert!(log.printed().last().unwrap().contains("alive=yes"));
    }

    #[test]
    fn h6_release_by_a_stranger_is_refused_by_the_owner_it_works() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 0);
        // not the owner
        assert_eq!(session(&lock, opts("b@x"), &rig).release(), 3);
        assert!(lock.join("owner").exists());
        assert!(log.said().iter().any(|l| l.starts_with("refused:")));
        // right agent, other host
        let mut other = session(&lock, opts("a@x"), &rig);
        other.host = "hostC".into();
        assert_eq!(other.release(), 3);
        assert!(lock.join("owner").exists());
        // the owner
        assert_eq!(session(&lock, opts("a@x"), &rig).release(), 0);
        assert!(!lock.exists());
    }

    #[test]
    fn a_release_of_a_missing_lock_says_so_and_is_0() {
        let d = tempfile::tempdir().unwrap();
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(
            session(&d.path().join("none"), opts("a@x"), &rig).release(),
            0
        );
        assert!(log.said()[0].starts_with("nothing to release:"));
    }

    #[test]
    fn o4_release_of_an_invalid_owner_file_is_refused_valid_it_is_released() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        std::fs::create_dir(&lock).unwrap();
        std::fs::write(
            lock.join("owner"),
            "agent=a@x\nhost=hostA\npid=5\nstarted=x\n",
        )
        .unwrap();
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("a@x"), &rig).release(), 3);
        assert!(lock.join("owner").exists());
        assert!(
            log.said()
                .iter()
                .any(|l| l.contains("held by foreign holder: owner (no numeric started_epoch=)"))
        );
        std::fs::write(
            lock.join("owner"),
            format!("agent=a@x\nhost=hostA\npid=5\nstarted_epoch={NOW}\n"),
        )
        .unwrap();
        assert_eq!(session(&lock, opts("a@x"), &rig).release(), 0);
        assert!(!lock.exists());
    }

    /// A clock that calls a hook at its first `now()`.
    struct OnNow<'a> {
        inner: &'a FakeClock,
        hook: Box<dyn Fn() + 'a>,
        done: std::cell::Cell<bool>,
    }

    impl Clock for OnNow<'_> {
        fn now(&self) -> i64 {
            if !self.done.replace(true) {
                (self.hook)();
            }
            self.inner.now()
        }
        fn sleep(&self, d: Duration) {
            self.inner.sleep(d);
        }
    }

    #[test]
    fn r16_a_release_puts_the_dir_back_when_another_taker_replaced_the_lock_before_the_rename() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let base = FakeClock::at(NOW);
        let p = FakeProcs::new();
        let log = Log::memory(&base);
        // the lock is ours when `release` looks, and another agent's when it renames: the hook
        // runs at the first `now()` after the check
        let l2 = lock.clone();
        let c = OnNow {
            inner: &base,
            hook: Box::new(move || plant(&l2, "thief@x", "hostA", "1", NOW)),
            done: Default::default(),
        };
        plant(&lock, "a@x", "hostA", "5", NOW);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("a@x"), &rig).release(), 3);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=thief@x")
        );
        assert!(
            log.said()
                .iter()
                .any(|l| l.starts_with("refused:") && l.contains("thief@x"))
        );
        // the other direction: nobody replaced it, the release removes it and leaves nothing
        let lock2 = d.path().join("m");
        plant(&lock2, "a@x", "hostA", "5", NOW);
        let rig = Rig {
            clock: &base,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock2, opts("a@x"), &rig).release(), 0);
        assert!(!lock2.exists());
        let left: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(left, vec!["l".to_string()]);
    }

    #[test]
    fn h10_status_free_held_stale() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("a@x"), &rig).status(), 1);
        assert_eq!(log.printed().last().unwrap(), "free");
        plant(&lock, "agent-a@host-b", "hostA", "111", NOW);
        assert_eq!(session(&lock, opts("a@x"), &rig).status(), 0);
        assert!(log.printed().last().unwrap().starts_with(
            "held: agent-a@host-b@hostA pid=111 since planted (planted) alive=yes age="
        ));
        plant(&lock, "agent-a@host-b", "hostA", "222", NOW);
        assert_eq!(session(&lock, opts("a@x"), &rig).status(), 2);
        assert!(
            log.printed()
                .last()
                .unwrap()
                .starts_with("stale: pid-dead:")
        );
    }

    #[test]
    fn h10d_break_if_dead_refuses_a_live_lock_h10e_removes_a_dead_one_h10f_free() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("x"), &rig).break_if_dead(), 0);
        assert_eq!(log.printed().last().unwrap(), "free");
        plant(&lock, "agent-a@host-b", "hostA", "111", NOW);
        let before = std::fs::read(lock.join("owner")).unwrap();
        assert_eq!(session(&lock, opts("x"), &rig).break_if_dead(), 3);
        assert!(
            log.printed()
                .last()
                .unwrap()
                .starts_with("refused: held: agent-a@host-b@hostA")
        );
        assert_eq!(std::fs::read(lock.join("owner")).unwrap(), before);
        plant(&lock, "agent-a@host-b", "hostA", "222", NOW);
        assert_eq!(session(&lock, opts("x"), &rig).break_if_dead(), 0);
        assert!(!lock.exists() && !d.path().join("l.takeover").exists());
        assert_eq!(
            log.said()
                .iter()
                .filter(|l| l.contains("took over stale lock"))
                .count(),
            1
        );
    }

    #[test]
    fn h13_a_running_ci_job_gives_76_and_no_lock_without_it_the_take_works() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        p.set_ci(Some("/x/Runner.Worker spawnclient"));
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 76);
        assert!(!lock.exists());
        assert!(log.said().iter().any(|l| {
            l.contains("CI first: a CI job runs on this host (Runner.Worker); not taking")
        }));
        p.set_ci(None);
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 0);
    }

    #[test]
    fn h13c_an_agent_waits_for_ci_and_then_takes() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        p.set_ci(Some("/x/Runner.Worker spawnclient"));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("a@x");
        o.wait = 3;
        o.poll = Duration::from_secs(1);
        // CI is still there for the whole wait: 76 and it says it waited
        assert_eq!(session(&lock, o.clone(), &rig).take("77"), 76);
        assert!(log.said().iter().any(|l| l.ends_with("; waiting")));
        // CI gone: the wait ends with the lock
        p.set_ci(None);
        assert_eq!(session(&lock, o, &rig).take("77"), 0);
    }

    #[test]
    fn h14_a_waiting_entry_blocks_an_agent_and_ci_done_frees_it() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(crate::clock::now_epoch()), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        assert_eq!(session(&lock, opts("ci-job-1"), &rig).ci_wait(None), 0);
        assert!(d.path().join("l.waiting/ci-job-1").is_file());
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 76);
        assert!(!lock.exists());
        assert_eq!(session(&lock, opts("x"), &rig).ci_status(), 0);
        assert!(
            log.printed()
                .last()
                .unwrap()
                .contains("waiting list entry ci-job-1")
        );
        assert_eq!(session(&lock, opts("ci-job-1"), &rig).ci_done(), 0);
        assert_eq!(session(&lock, opts("x"), &rig).ci_status(), 1);
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 0);
        assert!(!d.path().join("l.waiting").exists());
    }

    #[test]
    fn h15_a_ci_take_passes_waiting_ci_and_a_runner_but_a_held_lock_blocks_it() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (
            FakeClock::at(crate::clock::now_epoch()),
            FakeProcs::new().alive_pids(&[111]),
        );
        p.set_ci(Some("/x/Runner.Worker spawnclient"));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        session(&lock, opts("other-ci"), &rig).ci_wait(None);
        let mut o = opts("ci-1");
        o.ci_mode = true;
        assert_eq!(session(&lock, o.clone(), &rig).take("111"), 0);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=ci-1")
        );
        assert!(!d.path().join("l.waiting/ci-1").exists());
        // blocked by the HELD lock, and its own entry is removed again
        let mut o2 = opts("ci-2");
        o2.ci_mode = true;
        o2.wait = 1;
        o2.poll = Duration::from_millis(300);
        assert_eq!(session(&lock, o2, &rig).take("111"), 75);
        assert!(!d.path().join("l.waiting/ci-2").exists());
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=ci-1")
        );
    }

    #[test]
    fn h15c_a_waiting_ci_take_is_on_the_list_while_it_waits_then_gets_the_lock() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let now = crate::clock::now_epoch();
        let (base, p) = (FakeClock::at(now), FakeProcs::new().alive_pids(&[111]));
        plant(&lock, "ci-1", "hostA", "111", now);
        let listed = std::cell::Cell::new(false);
        let (l2, entry) = (lock.clone(), d.path().join("l.waiting/ci-3"));
        let c = OnSleep::new(&base, |n| {
            if n == 1 {
                listed.set(entry.exists());
            }
            if n == 2 {
                std::fs::remove_dir_all(&l2).unwrap();
            }
        });
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("ci-3");
        o.ci_mode = true;
        o.wait = 5;
        o.poll = Duration::from_secs(1);
        let mut s = session(&lock, o, &rig);
        assert_eq!(s.take("111"), 0);
        assert!(listed.get(), "the entry is on the list while it waits");
        assert!(!d.path().join("l.waiting").exists(), "and gone after");
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=ci-3")
        );
    }

    #[test]
    fn h16_no_takeover_of_a_stale_lock_while_ci_waits_after_it_the_takeover_works() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "dead@x", "hostA", "999", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        session(&lock, opts("ci-w"), &rig).ci_wait(None);
        // the entry is fresh in real time; the fake clock stands at NOW (a past time): keep it fresh
        let f = d.path().join("l.waiting/ci-w");
        let h = std::fs::File::options().write(true).open(&f).unwrap();
        h.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(NOW as u64))
            .unwrap();
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 76);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=dead@x")
        );
        session(&lock, opts("ci-w"), &rig).ci_done();
        assert_eq!(session(&lock, opts("a@x"), &rig).take("77"), 0);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=a@x")
        );
    }

    #[test]
    fn gt1_a_gate_skips_the_ci_wait_but_a_held_lock_still_blocks_it() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[111]));
        p.set_ci(Some("/x/Runner.Worker spawnclient"));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("a@hostA");
        assert_eq!(session(&lock, o.clone(), &rig).take("111"), 76);
        o.gate = true;
        assert_eq!(session(&lock, o.clone(), &rig).take("111"), 0);
        // the lock is held now
        let mut b = opts("b@hostA");
        b.gate = true;
        assert_eq!(session(&lock, b, &rig).take("6"), 75);
    }

    #[test]
    fn r_re_entry_a_granted_ancestor_gets_the_lock_at_once_without_a_second_owner_file() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "outer@x", "hostA", "4000", NOW);
        // this process 5000 has the parent 4000, the holder
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[4000]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&lock, opts("inner@x"), &rig);
        s.grant = Grant {
            dir: Some(lock.to_string_lossy().to_string()),
            pid: Some("4000".into()),
        };
        assert_eq!(s.take("5000"), 0);
        assert!(s.nested);
        assert!(log.said()[0].starts_with("re-entrant:"));
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=outer@x")
        );
        // its release leaves the outer lock alone, whatever the name
        assert_eq!(s.release(), 0);
        assert!(lock.join("owner").exists());
    }

    #[test]
    fn r4_an_ancestor_holder_without_a_grant_is_77_at_once_even_with_a_long_wait() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "outer@x", "hostA", "4000", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[4000]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut o = opts("b@x");
        o.wait = 30;
        let mut s = session(&lock, o, &rig);
        assert_eq!(s.take("5000"), 77);
        assert_eq!(c.now(), NOW, "no wait");
        assert!(log.said()[0].starts_with("held by your own parent process 4000"));
    }

    #[test]
    fn r3_a_non_ancestor_holder_blocks_even_when_the_environment_names_its_pid() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "other@x", "hostA", "3000", NOW);
        // 3000 is alive but not on the chain 5000 -> 4000 -> 1
        let (c, p) = (
            FakeClock::at(NOW),
            FakeProcs::new().alive_pids(&[3000]).with(4000, 1, "sh"),
        );
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let grant = Grant {
            dir: Some(lock.to_string_lossy().to_string()),
            pid: Some("3000".into()),
        };
        let mut s = session(&lock, opts("me@x"), &rig);
        s.grant = grant.clone();
        assert_eq!(s.take("0"), 75);
        let mut r = session(&lock, opts("me@x"), &rig);
        r.grant = grant;
        assert_eq!(r.release(), 3);
    }

    #[test]
    fn r7_the_grant_is_for_one_dir_another_dir_is_not_covered() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let other = d.path().join("m");
        plant(&lock, "outer@x", "hostA", "4000", NOW);
        plant(&other, "z@x", "hostA", "4000", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[4000]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let grant = Grant {
            dir: Some(lock.to_string_lossy().to_string()),
            pid: Some("4000".into()),
        };
        let mut s = session(&other, opts("y@x"), &rig);
        s.grant = grant;
        assert_eq!(s.take("5000"), 77);
    }

    #[test]
    fn r3b_a_grant_that_names_another_pid_than_the_holder_is_not_a_grant() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "outer@x", "hostA", "4000", NOW);
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[4000]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&lock, opts("t@x"), &rig);
        s.grant = Grant {
            dir: Some(lock.to_string_lossy().to_string()),
            pid: Some("9999".into()),
        };
        assert_eq!(s.take("5000"), 77, "the pid of the grant is not the holder");
        // the same grant with the right pid passes
        s.grant.pid = Some("4000".into());
        assert_eq!(s.take("5000"), 0);
    }

    #[test]
    fn h7_a_takeover_guard_held_by_another_taker_stops_this_one_and_a_dead_taker_guard_goes() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let real = crate::clock::now_epoch();
        plant(&lock, "dead@x", "hostA", "999", real);
        let guard = d.path().join("l.takeover");
        std::fs::create_dir(&guard).unwrap();
        let (c, p) = (FakeClock::at(real), FakeProcs::new());
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        // a fresh guard: another taker is at work, this one does not take over (and 75 follows)
        let mut s = session(&lock, opts("new@x"), &rig);
        assert_eq!(s.take("77"), 75);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=dead@x")
        );
        assert!(guard.exists(), "the guard of the other taker stays");
        // a guard older than 60 s belongs to a dead taker: it is removed, the next try works
        // a directory opens for writing only with backup semantics on Windows (and read-only on Unix)
        let mut open = std::fs::OpenOptions::new();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            open.write(true).custom_flags(0x0200_0000); // FILE_FLAG_BACKUP_SEMANTICS
        }
        #[cfg(not(windows))]
        open.read(true);
        let h = open.open(&guard).unwrap();
        h.set_modified(std::time::UNIX_EPOCH + Duration::from_secs((real - 120) as u64))
            .unwrap();
        assert_eq!(
            s.take("77"),
            75,
            "the try that removes the guard still fails"
        );
        assert!(!guard.exists());
        assert_eq!(s.take("77"), 0);
        assert!(
            std::fs::read_to_string(lock.join("owner"))
                .unwrap()
                .contains("agent=new@x")
        );
    }

    #[cfg(unix)]
    #[test]
    fn r6_a_symlink_spelling_of_the_dir_is_the_same_lock() {
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        plant(&lock, "outer@x", "hostA", "4000", NOW);
        let link = d.path().join("l.lnk");
        std::os::unix::fs::symlink(&lock, &link).unwrap();
        let (c, p) = (FakeClock::at(NOW), FakeProcs::new().alive_pids(&[4000]));
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let mut s = session(&link, opts("t@x"), &rig);
        s.grant = Grant {
            dir: Some(lock.to_string_lossy().to_string()),
            pid: Some("4000".into()),
        };
        assert_eq!(s.take("5000"), 0);
        assert!(s.nested);
        assert_eq!(s.release(), 0);
        assert!(lock.join("owner").exists());
    }

    #[test]
    fn h17d_the_runner_walk_finds_the_worker_behind_a_full_path_comm() {
        let (c, _) = (FakeClock::at(NOW), ());
        // chain: me(500) -> shell 500's parent... the table: 500 -> 400 (bash) ; 400 is the worker
        let mut p = FakeProcs::new()
            .with(500, 400, "/bin/bash")
            .with(400, 300, "/Users/x/actions-runner/bin/Runner.Worker")
            .with(300, 1, "/sbin/launchd");
        p.me = 500;
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let d = tempfile::tempdir().unwrap();
        let s = session(&d.path().join("l"), opts("ci-h"), &rig);
        assert_eq!(runner_pid(&s), Some(400));
    }

    #[test]
    fn h18_without_a_runner_ancestor_there_is_no_runner_pid() {
        let c = FakeClock::at(NOW);
        let mut p = FakeProcs::new()
            .with(500, 300, "/bin/bash")
            .with(300, 1, "/sbin/launchd");
        p.me = 500;
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let d = tempfile::tempdir().unwrap();
        let s = session(&d.path().join("l"), opts("ci-n"), &rig);
        assert_eq!(runner_pid(&s), None);
    }

    #[test]
    fn dispatch_take_without_a_runner_is_64_and_writes_nothing() {
        let c = FakeClock::at(NOW);
        let mut p = FakeProcs::new()
            .with(500, 300, "/bin/bash")
            .with(300, 1, "/sbin/launchd");
        p.me = 500;
        let log = Log::memory(&c);
        let rig = Rig {
            clock: &c,
            procs: &p,
            log: &log,
        };
        let d = tempfile::tempdir().unwrap();
        let lock = d.path().join("l");
        let mut s = session(&lock, opts("ci-n"), &rig);
        let parsed = match crate::lock::args::parse(
            &[
                "take".into(),
                lock.to_string_lossy().to_string(),
                "--holder".into(),
                "ci-n".into(),
                "--pid".into(),
                "runner".into(),
            ],
            &Default::default(),
        ) {
            Ok(crate::lock::args::Parse::Run(p)) => *p,
            _ => panic!(),
        };
        assert_eq!(dispatch(&mut s, &parsed, &RunCtx::default()), 64);
        assert!(!lock.exists() && !d.path().join("l.waiting").exists());
        assert!(log.said()[0].contains("no Runner.Worker"));
    }
}
