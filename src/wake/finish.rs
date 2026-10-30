//! The wake and its end. `Flow::wake` announces the wake, runs the command through the runtime
//! phase and then does what the way it ended asks for:
//!
//! - a signal: keep the events, release the host locks of the agent, exit 128 plus the signal;
//! - the time budget: tell the coordinator, keep the events and the count of stops, release the
//!   host locks, exit 0;
//! - exit 0: end the run of budget stops, check the receipts, exit 0;
//! - another exit: keep the events, exit 70.
//!
//! Every wake that ran also starts the back-off of the next wake for owned work.

use super::WakeExit;
use super::flow::{Flow, Reason};
use super::runtime::Ended;
use crate::base::ignore::best_effort_cleanup;
use crate::clock::format_ts;
#[cfg(unix)]
use crate::lock::procs::Alive;
#[cfg(unix)]
use crate::ops::lock_cmd;
use crate::wake::describe_head;
use crate::wake::events;
use crate::wake::state;
use std::path::PathBuf;

impl Flow<'_, '_> {
    /// Run the wake command under the lock, then finish. The result is the exit code.
    pub(super) fn wake(&mut self, reason: Reason) -> u8 {
        let cmd = match &self.s.direct {
            Some(d) => d.text.clone(),
            None => self.s.wake_cmd.clone().unwrap_or_default(),
        };
        if reason == Reason::Work {
            self.log.say(&format!(
                "no events, but the agent owns an In Progress or Ready ticket; waking: {cmd}"
            ));
        } else {
            self.log.say(&format!("events pending; waking: {cmd}"));
        }
        let start = format_ts(self.clock.now());
        match self.run_wake_command(&cmd, reason) {
            Ended::Signal(code) => {
                // a wake that a signal stopped may have been half handled: keep its events, and
                // free the locks that its group held
                self.keep_events();
                #[cfg(unix)]
                self.release_host_locks();
                code
            }
            Ended::Budget => {
                self.start_work_backoff();
                self.budget_end()
            }
            Ended::Exit(rc) => {
                self.start_work_backoff();
                self.after_exit(rc, &start)
            }
        }
    }

    /// Every wake, also a failed one, starts the back-off of the next wake for owned work.
    fn start_work_backoff(&self) {
        self.put(
            &self.s.paths.last_wake,
            format!("{}\n", self.clock.now()).as_bytes(),
        );
    }

    /// The wake command ended by itself with `wrc`.
    fn after_exit(&mut self, wrc: u8, start: &str) -> u8 {
        if wrc != 0 {
            self.keep_events();
            self.log.say(&format!("wake command failed: exit {wrc}"));
            return WakeExit::WakeFailed.code();
        }
        // a wake that ends by itself ends the run of budget stops
        best_effort_cleanup(std::fs::remove_file(&self.s.paths.budget_streak));
        if let Err(code) = self.check_acted(start) {
            return code;
        }
        self.log.say("wake finished OK");
        0
    }

    /// The mechanical end of a wake at its time budget: the wrapper does it, not the model. One
    /// `say` to the coordinator on the In Progress ticket (where the work is), then the host
    /// locks of the agent. Exit 0.
    fn budget_end(&mut self) -> u8 {
        let max = self.s.max_wake.text.clone();
        self.log
            .say(&format!("wake stopped at the time budget of {max}"));
        // A wake that ends at the budget is a failed wake for its events: the next tick wakes the
        // agent again. Uncommitted work in its checkout comes first in the kept block. The
        // wrapper reads the work tree; it never commits, pushes or stages anything.
        let wd = self
            .s
            .direct
            .as_ref()
            .map(|d| d.cwd.clone())
            .or_else(|| self.s.work_dir.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let log = self.log;
        let dirty = describe_head::dirty_checkouts(&wd, &mut |l| log.line(l));
        self.keep_events_with(dirty_header(&dirty).as_deref());
        let (rc, mine) = self.call("mine", &mut |s, o| s.mine_text(o));
        let mine = String::from_utf8_lossy(&mine).into_owned();
        let ticket = (rc.code == 0)
            .then(|| describe_head::first_ticket(&mine))
            .flatten();
        let streak = self.count_budget_stop(ticket.as_deref().unwrap_or("-"));
        match ticket {
            Some(t) => self.say_budget_end(&t, &mine, &max, &dirty, streak),
            None => self
                .log
                .line("no In Progress ticket: no message to the coordinator"),
        }
        #[cfg(unix)]
        self.release_host_locks();
        0
    }

    /// Count this stop at the time budget: the stops in a row on `ticket`, this one included. A
    /// stop on another ticket starts the count again.
    fn count_budget_stop(&self, ticket: &str) -> u32 {
        let n = match state::read_budget_streak(&self.s.paths.budget_streak) {
            Some((n, t)) if t == ticket => n.saturating_add(1),
            _ => 1,
        };
        self.put(
            &self.s.paths.budget_streak,
            format!("{n} {ticket}\n").as_bytes(),
        );
        n
    }

    fn say_budget_end(
        &mut self,
        ticket: &str,
        mine: &str,
        max: &str,
        dirty: &[PathBuf],
        streak: u32,
    ) {
        let wd = self
            .s
            .work_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        let agent = self.s.agent.clone();
        let log = self.log;
        let where_ = describe_head::describe(
            ticket,
            mine,
            &wd,
            agent.as_deref(),
            &mut *self.steps,
            &mut |l| log.line(l),
        );
        let uncommitted = if dirty.is_empty() {
            String::new()
        } else {
            format!(" Uncommitted work in {}.", dir_list(dirty))
        };
        let mut body = format!(
            "stopped at the time budget ({max}) of the wake wrapper. Ticket {ticket}, {where_}.{uncommitted} The next wake continues from here."
        );
        if streak >= 2 {
            body.push_str(
                "\n\nhit the time budget twice in a row: split the ticket or raise --max-wake",
            );
        }
        let mut anyway = false;
        match self.s.events_file.clone() {
            Some(ev) => {
                // Read the unread events first: `poll` moves the cursor, the events
                // go to the next wake (<events file>.unhandled), and `say` needs no --override-unread, so
                // the message has no override line.
                let (rc, out) = self.call("poll", &mut |s, o| s.poll(false, o));
                if rc.code == 0 {
                    let kept = events::unhandled_path(&ev);
                    let text = format!(
                        "== Events read at the time budget end ({}), not handled yet:\n{}\n",
                        format_ts(self.clock.now()),
                        String::from_utf8_lossy(&out)
                    );
                    let r = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&kept)
                        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()));
                    if let Err(e) = r {
                        self.log.line(&format!(
                            "cannot keep the events in {}: {e}",
                            kept.display()
                        ));
                    }
                }
            }
            // no events file to keep them in: post although a message is unread
            None => anyway = true,
        }
        let (rc, _) = self.call("say", &mut |s, _| s.say(ticket, &body, anyway));
        if rc.code == 0 {
            self.log.line(&format!("said on {ticket}: {body}"));
        } else {
            self.log.line(&format!(
                "say on {ticket} failed: exit {}: {}",
                rc.code, rc.msg
            ));
        }
    }

    /// Release the host locks of the agent, except one that a detached or running job holds.
    #[cfg(unix)]
    pub(super) fn release_host_locks(&self) {
        if self.s.host_locks.is_empty() {
            return;
        }
        let Some(agent) = self.s.agent.as_deref() else {
            self.log.line("host locks not released: --agent is not set");
            return;
        };
        for hl in &self.s.host_locks {
            let owner = std::fs::read_to_string(hl.join("owner")).unwrap_or_default();
            let field = |k: &str| {
                owner
                    .lines()
                    .find_map(|l| l.strip_prefix(k))
                    .map(|v| v.trim().to_string())
            };
            let pid: Option<u32> = field("pid=")
                .and_then(|p| p.parse().ok())
                .filter(|p| *p > 0);
            let detached = field("detached=").as_deref() == Some("1");
            if field("agent=").as_deref() == Some(agent)
                && (detached || pid.is_some_and(|p| self.pids.alive(p) == Alive::Yes))
            {
                self.log.line(&format!(
                    "host lock {}: kept for {agent}: held by a detached or running job (pid {}), not by this wake",
                    hl.display(),
                    pid.map_or("?".to_string(), |p| p.to_string())
                ));
                continue;
            }
            let (code, first) = lock_cmd::release_for(&hl.to_string_lossy(), agent);
            let first: String = first.chars().take(200).collect();
            if code == 0 {
                let note = if first.is_empty() {
                    String::new()
                } else {
                    format!(" ({first})")
                };
                self.log.line(&format!(
                    "host lock {}: released for {agent}{note}",
                    hl.display()
                ));
            } else {
                self.log.line(&format!(
                    "host lock {}: not released: {first}",
                    hl.display()
                ));
            }
        }
    }
}

/// The dirs, comma separated.
fn dir_list(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|d| d.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first line of the kept events of a wake that ended at its budget with uncommitted work.
fn dirty_header(dirs: &[PathBuf]) -> Option<String> {
    (!dirs.is_empty()).then(|| {
        format!(
            "The last wake hit the time budget. Uncommitted work in {}: commit or continue it first.",
            dir_list(dirs)
        )
    })
}
