//! The health of the scheduled wakes of one agent, read from its state dir: `doctor --setup`
//! shows it. The board shows the effect only: a wake whose command fails writes no heartbeat, so
//! the agent turns late and dead there, without the cause.

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

/// The model wakes of the unattended coordinator that the unit of this state dir started
/// (`coordinator-wakes`): the mode and the runtime of the last start, and the count of its day.
/// `None`: the unit started no model yet, or it is no `--tick --runtime` unit.
pub fn coordinator(dir: &Path) -> Option<crate::wake::policy::CoordWakes> {
    state::read_coord_wakes(&Paths::new(dir.to_path_buf(), None, None).coord_wakes)
}

/// The state of the model of a `--tick --runtime` unit: the model wakes that failed in a row,
/// the ones in a row that ended at the time budget, and the error of a model command that
/// the last run could not build.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Model {
    pub fails: u64,
    pub budget_stops: u32,
    pub error: Option<String>,
}

/// The time text of a `wake` call for this state dir that was refused inside a wake
/// (`CREWBOARD_WAKE` was set), when no run of the unit came after it. A unit whose own
/// environment has the variable never runs: each of its wakes ends there with exit 2.
pub fn refused_inside_wake(dir: &Path) -> Option<String> {
    state::read_text(&Paths::new(dir.to_path_buf(), None, None).inside_wake)
        .map(|t| t.lines().next().unwrap_or("").to_string())
}

/// Read the model state of the state dir.
pub fn model(dir: &Path) -> Model {
    let p = Paths::new(dir.to_path_buf(), None, None);
    Model {
        fails: state::read_text(&p.wake_fail).map_or(0, |t| NetBackoff::parse(&t).fails),
        budget_stops: state::read_budget_streak(&p.budget_streak).map_or(0, |(n, _)| n),
        error: state::read_text(&p.model_error).filter(|t| !t.trim().is_empty()),
    }
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
    fn the_model_wakes_of_the_coordinator_are_read_from_their_count_file() {
        let d = dir();
        assert_eq!(coordinator(d.path()), None, "a unit that started no model");
        std::fs::write(
            d.path().join("coordinator-wakes"),
            "2026-11-28 2 1795860000 00ff verify-and-merge hermes 0\n",
        )
        .unwrap();
        let w = coordinator(d.path()).unwrap();
        assert_eq!(
            (w.day.as_str(), w.count, w.mode.as_str(), w.runtime.as_str()),
            ("2026-11-28", 2, "verify-and-merge", "hermes")
        );
        // the file changes nothing in the health of the wakes
        assert_eq!(read(d.path()), Health::Running { last: None });
        std::fs::write(d.path().join("coordinator-wakes"), "broken\n").unwrap();
        assert_eq!(coordinator(d.path()), None);
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
