//! The health of the scheduled wakes of one agent, read from its state dir: `doctor --setup`
//! shows it. The board cannot show it: a wake that cannot start its model still polls, so the
//! heartbeat goes on and the agent looks alive.

use crate::wake::policy::NetBackoff;
use crate::wake::state::{self, Paths, StateKey};
use std::path::{Path, PathBuf};

/// What the state files say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// No state dir: no scheduled wake has run for this agent on this host.
    NeverRan,
    /// The wakes run. `last`: the end of the last wake command, when one ran.
    Running { last: Option<i64> },
    /// The wake command failed this many times in a row.
    Failing { fails: u64 },
    /// An exit 4 stopped the wakes; the first line of the stop file.
    Stopped { line: String },
    /// GitHub or the network failed in this many runs in a row.
    Network { fails: u64 },
}

/// Wake commands that failed in a row, from this count on, are a fault to show: one failed
/// wake can be a model limit or a restart.
pub const FAILING_FROM: u64 = 2;

/// The default state dir of the agent of `config_dir` in `initiative`, as `wake` picks it.
pub fn state_dir(
    config_dir: &Path,
    initiative: &str,
    xdg_state_home: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    let root = state::state_root(xdg_state_home, home)?;
    let key = StateKey {
        config_dir: Some(config_dir),
        initiative: Some(initiative),
        ..StateKey::default()
    };
    Some(state::state_dir(&key, &root))
}

/// Read the state dir. A stop comes first (nothing runs), then failed wake commands, then the
/// network.
pub fn read(dir: &Path) -> Health {
    if !dir.is_dir() {
        return Health::NeverRan;
    }
    let p = Paths::new(dir.to_path_buf(), None, None);
    if let Some(text) = state::read_text(&p.stop) {
        return Health::Stopped {
            line: text.lines().next().unwrap_or("").to_string(),
        };
    }
    let count = |f: &Path| state::read_text(f).map_or(0, |t| NetBackoff::parse(&t).fails);
    let fails = count(&p.wake_fail);
    if fails >= FAILING_FROM {
        return Health::Failing { fails };
    }
    let net = count(&p.net);
    if net >= FAILING_FROM {
        return Health::Network { fails: net };
    }
    Health::Running {
        last: state::read_last_wake(&p.last_wake),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn a_missing_state_dir_means_no_wake_has_run() {
        let d = dir();
        assert_eq!(read(&d.path().join("none")), Health::NeverRan);
    }

    #[test]
    fn an_empty_state_dir_runs_and_names_the_last_wake_when_there_is_one() {
        let d = dir();
        assert_eq!(read(d.path()), Health::Running { last: None });
        std::fs::write(d.path().join("last-wake"), "1700000000\n").unwrap();
        assert_eq!(
            read(d.path()),
            Health::Running {
                last: Some(1_700_000_000)
            }
        );
    }

    #[test]
    fn one_failed_wake_is_no_fault_and_two_in_a_row_are() {
        let d = dir();
        std::fs::write(d.path().join("wake-fail"), "1 0\n").unwrap();
        assert_eq!(read(d.path()), Health::Running { last: None });
        std::fs::write(d.path().join("wake-fail"), "7 1700003600\n").unwrap();
        assert_eq!(read(d.path()), Health::Failing { fails: 7 });
    }

    #[test]
    fn a_stop_comes_before_failed_wakes_and_the_network() {
        let d = dir();
        std::fs::write(d.path().join("net-backoff"), "3 0\n").unwrap();
        assert_eq!(read(d.path()), Health::Network { fails: 3 });
        std::fs::write(d.path().join("wake-fail"), "2 0\n").unwrap();
        assert_eq!(read(d.path()), Health::Failing { fails: 2 });
        std::fs::write(d.path().join("stopped"), "ts poll: HTTP 401\nmore\n").unwrap();
        assert_eq!(
            read(d.path()),
            Health::Stopped {
                line: "ts poll: HTTP 401".into()
            }
        );
    }

    #[test]
    fn the_state_dir_is_the_one_that_wake_uses() {
        let got = state_dir(Path::new("/c/demo-dev"), "demo", None, Some("/home/u")).unwrap();
        assert_eq!(
            got,
            PathBuf::from("/home/u/.local/state/crewboard/agents/demo-dev")
        );
        assert_eq!(state_dir(Path::new("/c/x"), "demo", None, None), None);
    }
}
