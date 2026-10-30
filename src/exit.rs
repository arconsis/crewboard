//! Process exit codes (spec/cli#exit-codes).
//!
//! The code means something different for `watch` / `poll` than for every other
//! command. "No events" and "error" never share a code: a watcher that cannot see
//! events must never look like a quiet board.

/// The meaning of a code depends on the command family that returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `watch` and `poll`, and `mine` (exit 1: the agent owns no In Progress or Ready ticket).
    Watch,
    /// `lock`: its own codes (spec/host-lock): 0, 1 (free, no CI), 2, 3, 64, 75, 76, 77, or
    /// the code of the command that `run` ran.
    Lock,
    /// `wake`: 0, 2, 3, 4, 70 (the wake command failed), 71 (the lock cannot be taken).
    Wake,
    /// `repeat`: 0 all runs green, 1 a red run, 2 the commit changed, 3 CI did not finish, 4 setup
    /// error, 64 usage.
    Repeat,
    /// Every other command.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Exit {
    /// `watch` / `poll`: events were printed. Other commands: success.
    Ok = 0,
    /// `watch` / `poll` only: no events (timeout, or nothing new). Not an error.
    /// Other commands never return this code.
    NoEvents = 1,
    /// Refused by a rule. The message names the rule.
    Refused = 2,
    /// GitHub or network error.
    Network = 3,
    /// Configuration or key error.
    Config = 4,
}

impl Exit {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Whether `family` may return this code at all.
    pub fn allowed_for(self, family: Family) -> bool {
        match self {
            Exit::NoEvents => matches!(family, Family::Watch | Family::Lock | Family::Repeat),
            _ => true,
        }
    }
}

impl From<Exit> for std::process::ExitCode {
    fn from(e: Exit) -> Self {
        std::process::ExitCode::from(e.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Exit {
        const ALL: [Exit; 5] = [
            Exit::Ok,
            Exit::NoEvents,
            Exit::Refused,
            Exit::Network,
            Exit::Config,
        ];

        /// Human-readable meaning for the given command family.
        fn meaning(self, family: Family) -> &'static str {
            match (self, family) {
                (Exit::Ok, Family::Watch) => "events printed",
                (Exit::Ok, Family::Other | Family::Lock | Family::Wake | Family::Repeat) => {
                    "success"
                }
                (Exit::NoEvents, Family::Watch) => "no events (timeout or nothing new)",
                (Exit::NoEvents, Family::Lock) => "free, or no CI",
                (Exit::NoEvents, Family::Repeat) => "a red run",
                (Exit::NoEvents, Family::Other | Family::Wake) => "not used",
                (Exit::Refused, _) => "refused by a rule",
                (Exit::Network, _) => "GitHub or network error",
                (Exit::Config, _) => "configuration or key error",
            }
        }
    }

    #[test]
    fn codes_match_spec_table() {
        let codes: Vec<u8> = Exit::ALL.iter().map(|e| e.code()).collect();
        assert_eq!(codes, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn no_events_is_distinct_from_every_error() {
        // Must pass: watch may return 1.
        assert!(Exit::NoEvents.allowed_for(Family::Watch));
        // Must be refused: other commands never return 1.
        assert!(!Exit::NoEvents.allowed_for(Family::Other));
        for e in [Exit::Refused, Exit::Network, Exit::Config] {
            assert_ne!(e.code(), Exit::NoEvents.code());
        }
    }

    #[test]
    fn meanings_differ_by_family_only_where_spec_says() {
        assert_eq!(Exit::Ok.meaning(Family::Watch), "events printed");
        assert_eq!(Exit::Ok.meaning(Family::Other), "success");
        assert_eq!(
            Exit::Refused.meaning(Family::Watch),
            Exit::Refused.meaning(Family::Other)
        );
    }
}
