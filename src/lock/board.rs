//! The board check: does the board say that the owner agent of a lock is dead or has left?
//!
//! The probe is a trait. In the product it asks the board in process (`ops::lock_cmd`); the
//! test seam `--crewboard BIN` asks a stub binary through `crewboard status --json`. [`BoardCheck`] is the
//! state machine around it: one read per TTL (a failed read counts too), one note per cause, one "works again".

use super::log::Log;

/// One agent row of the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardAgent {
    pub name: String,
    pub liveness: String,
    pub state: String,
}

/// Why the board could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeError {
    /// The probe ran and failed. `cause` is the first line of the error (200 characters at most)
    /// and may be empty; `exit` is the exit code of a stub binary, when there was one.
    Failed { cause: String, exit: Option<i32> },
    /// The stub binary does not exist (`crewboard not found (BIN)`).
    NotFound(String),
    /// The answer has no agent list.
    NoList,
}

pub trait BoardProbe {
    fn agents(&self) -> Result<Vec<BoardAgent>, ProbeError>;
}

/// What the judge asks. The answer is the stale reason text (`board-dead: ...`) or `None`.
pub trait BoardSays {
    fn says_gone(&mut self, agent: &str, now: i64) -> Option<String>;
}

pub struct BoardCheck<'a> {
    probe: Option<Box<dyn BoardProbe + 'a>>,
    ttl: i64,
    /// A config dir was given (flag or `$CREWBOARD_CONFIG_DIR`).
    config_given: bool,
    /// `status` says "unavailable"; take and wait stay silent about it.
    say_unavailable: bool,
    log: &'a Log<'a>,
    cache: Option<(Vec<BoardAgent>, i64)>,
    fail_at: i64,
    /// The cause of the last note; empty while the check works.
    cause: String,
}

impl<'a> BoardCheck<'a> {
    /// `probe` is `None` for `--no-board`.
    pub fn new(
        probe: Option<Box<dyn BoardProbe + 'a>>,
        ttl: i64,
        config_given: bool,
        say_unavailable: bool,
        log: &'a Log<'a>,
    ) -> Self {
        BoardCheck {
            probe,
            ttl,
            config_given,
            say_unavailable,
            log,
            cache: None,
            fail_at: 0,
            cause: String::new(),
        }
    }

    /// One note per cause, not one per poll.
    fn fail(&mut self, cause: String) {
        if self.cause == cause {
            return;
        }
        self.log.say(&format!("board check skipped: {cause}"));
        self.cause = cause;
    }

    /// No config dir and no default config: a setup gap, not a failure. Only `status` says it.
    fn unavailable(&mut self) {
        if self.cause == "unavailable" {
            return;
        }
        self.cause = "unavailable".to_string();
        if self.say_unavailable {
            self.log.say(
                "board check unavailable: no agent config dir (set CREWBOARD_CONFIG_DIR or --config-dir to the config dir of the owner agent)",
            );
        }
    }

    /// A working check after a failure gets one line.
    fn ok(&mut self) {
        if self.cause.is_empty() {
            return;
        }
        self.cause.clear();
        self.log.say("board check works again");
    }
}

impl BoardSays for BoardCheck<'_> {
    fn says_gone(&mut self, agent: &str, now: i64) -> Option<String> {
        let probe = self.probe.as_ref()?;
        let stale_cache = self
            .cache
            .as_ref()
            .is_none_or(|(_, at)| now - at >= self.ttl);
        if stale_cache {
            // a failed answer is kept as long as a good one: a rate limit must not be hit every poll
            if self.fail_at > 0 && now - self.fail_at < self.ttl {
                return None;
            }
            match probe.agents() {
                Err(ProbeError::NotFound(text)) => {
                    self.fail(text);
                    return None;
                }
                Err(ProbeError::Failed { cause, exit }) => {
                    self.cache = None;
                    self.fail_at = now;
                    if !self.config_given && cause.contains("no board config") {
                        self.unavailable();
                    } else if !cause.is_empty() {
                        self.fail(format!("crewboard status failed: {cause}"));
                    } else {
                        let rc = exit.map_or(String::new(), |c| c.to_string());
                        self.fail(format!("crewboard status failed (exit {rc})"));
                    }
                    return None;
                }
                Err(ProbeError::NoList) => {
                    self.cache = None;
                    self.fail_at = now;
                    self.fail("crewboard status gave no agent list".to_string());
                    return None;
                }
                Ok(rows) => {
                    self.cache = Some((rows, now));
                    self.fail_at = 0;
                    self.ok();
                }
            }
        }
        let (rows, _) = self.cache.as_ref()?;
        let row = rows.iter().find(|r| r.name == agent)?;
        if row.liveness == "dead" {
            return Some(format!("board-dead: the board says agent {agent} is dead"));
        }
        if row.state == "left" {
            return Some(format!("board-left: agent {agent} has left the board"));
        }
        None
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::cell::{Cell, RefCell};

    /// A probe that plays a script: one answer per call (the last one repeats).
    pub struct Script {
        answers: RefCell<Vec<Result<Vec<BoardAgent>, ProbeError>>>,
        pub calls: Cell<u32>,
    }

    impl Script {
        pub fn new(answers: Vec<Result<Vec<BoardAgent>, ProbeError>>) -> Self {
            Script {
                answers: RefCell::new(answers),
                calls: Cell::new(0),
            }
        }
    }

    impl BoardProbe for &Script {
        fn agents(&self) -> Result<Vec<BoardAgent>, ProbeError> {
            self.calls.set(self.calls.get() + 1);
            let mut a = self.answers.borrow_mut();
            if a.len() > 1 {
                a.remove(0)
            } else {
                a[0].clone()
            }
        }
    }

    pub fn agent(name: &str, liveness: &str, state: &str) -> BoardAgent {
        BoardAgent {
            name: name.into(),
            liveness: liveness.into(),
            state: state.into(),
        }
    }

    fn rows() -> Vec<BoardAgent> {
        vec![
            agent("own@x", "dead", "active"),
            agent("alive@x", "alive", "active"),
            agent("gone@x", "-", "left"),
        ]
    }

    fn fail(cause: &str) -> ProbeError {
        ProbeError::Failed {
            cause: cause.into(),
            exit: Some(1),
        }
    }

    fn check<'a>(
        s: &'a Script,
        log: &'a Log<'a>,
        ttl: i64,
        config: bool,
        status: bool,
    ) -> BoardCheck<'a> {
        BoardCheck::new(Some(Box::new(s)), ttl, config, status, log)
    }

    #[test]
    fn h8_dead_left_alive_and_unlisted() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Ok(rows())]);
        let mut b = check(&s, &log, 60, true, false);
        assert_eq!(
            b.says_gone("own@x", 1000).unwrap(),
            "board-dead: the board says agent own@x is dead"
        );
        assert_eq!(
            b.says_gone("gone@x", 1000).unwrap(),
            "board-left: agent gone@x has left the board"
        );
        // alive and not listed are not stale
        assert_eq!(b.says_gone("alive@x", 1000), None);
        assert_eq!(b.says_gone("unknown@x", 1000), None);
        assert!(log.said().is_empty());
    }

    #[test]
    fn h8e_no_board_never_asks() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let mut b = BoardCheck::new(None, 60, true, false, &log);
        assert_eq!(b.says_gone("own@x", 1000), None);
        assert!(log.said().is_empty());
    }

    #[test]
    fn h8f_an_unreachable_board_is_skipped_with_one_note() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(fail("error: boom"))]);
        let mut b = check(&s, &log, 0, true, false);
        assert_eq!(b.says_gone("own@x", 1000), None);
        assert_eq!(b.says_gone("own@x", 1001), None);
        assert_eq!(
            log.said(),
            vec!["board check skipped: crewboard status failed: error: boom"]
        );
    }

    #[test]
    fn h8g_the_cache_gives_one_call_for_many_polls() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Ok(vec![agent("alive@x", "alive", "active")])]);
        let mut b = check(&s, &log, 60, true, false);
        for t in 0..10 {
            assert_eq!(b.says_gone("alive@x", 1000 + t * 5), None);
        }
        assert_eq!(s.calls.get(), 1);
        // the other direction: after the TTL it asks again
        b.says_gone("alive@x", 1061);
        assert_eq!(s.calls.get(), 2);
    }

    #[test]
    fn h8h_three_failures_then_ok_give_one_skip_note_and_one_works_again() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![
            Err(fail("error: API rate limit exceeded")),
            Err(fail("error: API rate limit exceeded")),
            Err(fail("error: API rate limit exceeded")),
            Ok(vec![agent("alive@x", "alive", "active")]),
        ]);
        let mut b = check(&s, &log, 0, true, false);
        for t in 0..8 {
            assert_eq!(b.says_gone("alive@x", 1000 + t), None);
        }
        let said = log.said();
        assert_eq!(
            said.iter()
                .filter(|l| l.contains("board check skipped"))
                .count(),
            1
        );
        assert_eq!(said.iter().filter(|l| l.contains("works again")).count(), 1);
        assert_eq!(
            said[0],
            "board check skipped: crewboard status failed: error: API rate limit exceeded"
        );
    }

    #[test]
    fn h8h2_a_failed_read_is_kept_for_the_ttl_so_a_rate_limit_is_not_hit_every_poll() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(fail("error: x"))]);
        let mut b = check(&s, &log, 60, true, false);
        b.says_gone("a", 1000);
        b.says_gone("a", 1030);
        assert_eq!(s.calls.get(), 1);
        b.says_gone("a", 1061);
        assert_eq!(s.calls.get(), 2);
    }

    #[test]
    fn h8i_a_dead_owner_asks_the_board_once() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Ok(vec![agent("own@x", "dead", "active")])]);
        let mut b = check(&s, &log, 60, true, false);
        assert!(b.says_gone("own@x", 1000).is_some());
        assert_eq!(s.calls.get(), 1);
    }

    #[test]
    fn e6d_no_config_status_says_unavailable_once_and_no_failure_note() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(fail(
            "error: configuration error: no board config",
        ))]);
        let mut b = check(&s, &log, 0, false, true);
        assert_eq!(b.says_gone("own@x", 1000), None);
        assert_eq!(b.says_gone("own@x", 1001), None);
        let said = log.said();
        assert_eq!(said.len(), 1);
        assert!(said[0].starts_with("board check unavailable: no agent config dir"));
    }

    #[test]
    fn e6e_no_config_take_and_wait_stay_silent() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(fail(
            "error: configuration error: no board config",
        ))]);
        let mut b = check(&s, &log, 0, false, false);
        b.says_gone("own@x", 1000);
        b.says_gone("own@x", 1001);
        assert!(log.said().is_empty());
    }

    #[test]
    fn e6e_with_a_config_dir_the_same_text_is_a_failure_note() {
        // the other direction: a config dir was given, so "no board config" is a real failure
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(fail(
            "error: configuration error: no board config",
        ))]);
        let mut b = check(&s, &log, 0, true, true);
        b.says_gone("own@x", 1000);
        assert!(log.said()[0].starts_with("board check skipped: crewboard status failed:"));
    }

    #[test]
    fn a_failure_without_text_names_the_exit_code() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(ProbeError::Failed {
            cause: String::new(),
            exit: Some(3),
        })]);
        let mut b = check(&s, &log, 0, true, false);
        b.says_gone("a", 1000);
        assert_eq!(
            log.said(),
            vec!["board check skipped: crewboard status failed (exit 3)"]
        );
    }

    #[test]
    fn no_agent_list_and_a_missing_binary_have_their_own_notes() {
        let c = FakeClock::at(1000);
        let log = Log::memory(&c);
        let s = Script::new(vec![Err(ProbeError::NoList)]);
        let mut b = check(&s, &log, 0, true, false);
        b.says_gone("a", 1000);
        let s2 = Script::new(vec![Err(ProbeError::NotFound(
            "crewboard not found (nope)".into(),
        ))]);
        let mut b2 = check(&s2, &log, 0, true, false);
        b2.says_gone("a", 1000);
        assert_eq!(
            log.said(),
            vec![
                "board check skipped: crewboard status gave no agent list",
                "board check skipped: crewboard not found (nope)"
            ]
        );
    }
}
