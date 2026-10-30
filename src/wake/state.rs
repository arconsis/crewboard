//! The state dir of one agent and the files in it. The formats are stable across releases, so
//! that a wake of an older release and one of this release can run on one host.
//!
//! ```text
//! <state dir>/wake.lock/{owner,pid,started}   the wake lock
//! last-wake      epoch of the end of the last wake
//! events.txt     the events file of the wake (also `.unhandled`, `.tmp`); the unit picks the name
//! results/       result files of detached jobs
//! net-backoff    "<failed runs> <epoch until which runs are skipped>"
//! stopped        "<ts> <call>: <reason>"; its presence stops all wakes
//! wake.log       the log; wake.log.quiet counts the silent polls
//! last-tick      the output of the last `tick`
//! ```

use crate::base::ignore::best_effort_output;
use crate::clock::{Clock, format_ts};
use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};

/// What picks the state dir.
#[derive(Debug, Default, Clone)]
pub struct StateKey<'a> {
    pub state_dir: Option<&'a Path>,
    pub lock_dir: Option<&'a Path>,
    pub config_dir: Option<&'a Path>,
    pub agent: Option<&'a str>,
    pub initiative: Option<&'a str>,
}

/// `<$XDG_STATE_HOME or ~/.local/state>/crewboard`. An empty variable counts as unset. `None`
/// when neither variable is there.
pub fn state_root(xdg_state_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    let base = match xdg_state_home.filter(|s| !s.is_empty()) {
        Some(x) => PathBuf::from(x),
        None => PathBuf::from(home.filter(|s| !s.is_empty())?)
            .join(".local")
            .join("state"),
    };
    Some(base.join("crewboard"))
}

/// The last component of a path as `basename` prints it: without a trailing slash.
fn basename(p: &str) -> &str {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { "" } else { "/" };
    }
    t.rsplit('/').next().unwrap_or(t)
}

/// The directory of a path as `dirname` prints it.
fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() { ".".into() } else { "/".into() };
    }
    match t.rfind('/') {
        None => ".".into(),
        Some(i) => match t[..i].trim_end_matches('/') {
            "" => "/".into(),
            d => d.into(),
        },
    }
}

/// Every byte outside `A-Za-z0-9._-` becomes `_`.
pub fn safe_key(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// The state dir of this agent. `--state-dir` as it is; else the directory of `--lock-dir`; else
/// `<root>/agents/<initiative>-<basename of the config dir or the agent>` (without the initiative
/// prefix when the name starts with it, unless the doubled dir exists already); else
/// `<root>/<initiative or default>`.
pub fn state_dir(k: &StateKey<'_>, root: &Path) -> PathBuf {
    if let Some(d) = k.state_dir {
        return d.to_path_buf();
    }
    if let Some(l) = k.lock_dir {
        return PathBuf::from(dirname(&l.to_string_lossy()));
    }
    let key = match (k.config_dir, k.agent) {
        (Some(c), _) => Some(basename(&c.to_string_lossy()).to_string()),
        (None, Some(a)) => Some(a.to_string()),
        (None, None) => None,
    };
    match key {
        Some(key) => {
            let agents = root.join("agents");
            let doubled = match k.initiative {
                Some(i) => format!("{i}-{key}"),
                None => key.clone(),
            };
            // an agent dir that starts with `<initiative>-` is not prefixed again; a state dir
            // of the old doubled name that exists is kept (it holds the cursor and the streak)
            let short = match k.initiative {
                Some(i) if key.starts_with(&format!("{i}-")) => key,
                _ => doubled.clone(),
            };
            let old = agents.join(safe_key(&doubled));
            if short != doubled && !old.exists() {
                agents.join(safe_key(&short))
            } else {
                old
            }
        }
        None => root.join(k.initiative.unwrap_or("default")),
    }
}

/// The per-initiative state dir of an older version (`<root>/<initiative or default>`), for the
/// default per-agent state dir only: its `last-wake` is read once and its wake lock is respected.
/// None when `--state-dir` or `--lock-dir` decides the dir, or when there is no agent key.
pub fn legacy_dir(k: &StateKey<'_>, root: &Path) -> Option<PathBuf> {
    if k.state_dir.is_some()
        || k.lock_dir.is_some()
        || (k.config_dir.is_none() && k.agent.is_none())
    {
        return None;
    }
    Some(root.join(k.initiative.unwrap_or("default")))
}

/// The paths of the state files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub dir: PathBuf,
    pub lock: PathBuf,
    pub log: PathBuf,
    pub quiet: PathBuf,
    pub last_wake: PathBuf,
    pub results: PathBuf,
    pub stop: PathBuf,
    pub net: PathBuf,
    pub last_tick: PathBuf,
    pub budget_streak: PathBuf,
}

impl Paths {
    /// `lock` and `log` default into the state dir.
    pub fn new(dir: PathBuf, lock: Option<PathBuf>, log: Option<PathBuf>) -> Paths {
        let log = log.unwrap_or_else(|| dir.join("wake.log"));
        let mut quiet = log.clone().into_os_string();
        quiet.push(".quiet");
        Paths {
            lock: lock.unwrap_or_else(|| dir.join("wake.lock")),
            quiet: PathBuf::from(quiet),
            log,
            last_wake: dir.join("last-wake"),
            results: dir.join("results"),
            stop: dir.join("stopped"),
            net: dir.join("net-backoff"),
            last_tick: dir.join("last-tick"),
            budget_streak: dir.join("budget-streak"),
            dir,
        }
    }
}

/// The content of a file as `$(cat FILE)` gives it: without the trailing line breaks.
pub fn read_text(p: &Path) -> Option<String> {
    let t = std::fs::read_to_string(p).ok()?;
    Some(t.trim_end_matches('\n').to_string())
}

/// The wakes in a row that ended at the time budget on the same ticket. The file holds
/// `<count> <ticket>`; a wake that ends normally removes it.
pub fn read_budget_streak(p: &Path) -> Option<(u32, String)> {
    let t = read_text(p)?;
    let (n, ticket) = t.split_once(' ')?;
    Some((n.parse().ok().filter(|n| *n > 0)?, ticket.to_string()))
}

/// The epoch in `last-wake`; `None` when the file is missing or not a number.
pub fn read_last_wake(p: &Path) -> Option<i64> {
    i64::try_from(super::policy::parse_whole_secs(&read_text(p)?)?).ok()
}

/// The log of a run: `<UTC ts> poll-and-wake: <text>`. `say` also writes the line to standard
/// error, `line` only to the file. A log that cannot be written is ignored.
pub struct Log<'a> {
    path: PathBuf,
    clock: &'a dyn Clock,
    err: RefCell<Box<dyn Write + 'a>>,
}

impl<'a> Log<'a> {
    pub fn new(path: PathBuf, clock: &'a dyn Clock) -> Log<'a> {
        Log::with_err(path, clock, Box::new(std::io::stderr()))
    }

    pub fn with_err(path: PathBuf, clock: &'a dyn Clock, err: Box<dyn Write + 'a>) -> Log<'a> {
        Log {
            path,
            clock,
            err: RefCell::new(err),
        }
    }

    fn text(&self, msg: &str) -> String {
        format!("{} poll-and-wake: {msg}\n", format_ts(self.clock.now()))
    }

    pub fn say(&self, msg: &str) {
        let line = self.text(msg);
        best_effort_output(self.err.borrow_mut().write_all(line.as_bytes()));
        self.append(&line);
    }

    pub fn line(&self, msg: &str) {
        self.append(&self.text(msg));
    }

    fn append(&self, line: &str) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            best_effort_output(f.write_all(line.as_bytes()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use std::rc::Rc;

    #[test]
    fn s2_the_state_dir_follows_the_agent() {
        let root = Path::new("/st/crewboard");
        let key = |c: Option<&str>, a: Option<&str>, i: Option<&str>| {
            state_dir(
                &StateKey {
                    config_dir: c.map(Path::new),
                    agent: a,
                    initiative: i,
                    ..StateKey::default()
                },
                root,
            )
        };
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            key(Some("/x/agents/dev1"), None, Some("demo")),
            p("/st/crewboard/agents/demo-dev1")
        );
        assert_eq!(
            key(Some("/x/agents/dev2"), None, Some("demo")),
            p("/st/crewboard/agents/demo-dev2")
        );
        assert_eq!(
            key(None, Some("dev@host-a"), Some("demo")),
            p("/st/crewboard/agents/demo-dev_host-a")
        );
        // a trailing slash of the config dir does not change the key
        assert_eq!(
            key(Some("/x/agents/dev1/"), None, Some("demo")),
            p("/st/crewboard/agents/demo-dev1")
        );
        // without an initiative the key is the basename alone
        assert_eq!(
            key(Some("/x/dev1"), None, None),
            p("/st/crewboard/agents/dev1")
        );
        // neither a config dir nor an agent: the old per-initiative dir, or `default`
        assert_eq!(key(None, None, Some("demo")), p("/st/crewboard/demo"));
        assert_eq!(key(None, None, None), p("/st/crewboard/default"));
        // the config dir wins over the agent name
        assert_eq!(
            key(Some("/x/d1"), Some("a"), Some("i")),
            p("/st/crewboard/agents/i-d1")
        );
    }

    #[test]
    fn s2_a_new_agent_dir_with_the_initiative_prefix_gets_the_short_state_dir() {
        let d = tempfile::tempdir().unwrap();
        let k = StateKey {
            config_dir: Some(Path::new("/x/agents/demo-hermes")),
            initiative: Some("demo"),
            ..StateKey::default()
        };
        assert_eq!(state_dir(&k, d.path()), d.path().join("agents/demo-hermes"));
        // the prefix must be followed by a dash: `demox` is not `demo-x`
        let k2 = StateKey {
            config_dir: Some(Path::new("/x/agents/demox")),
            initiative: Some("demo"),
            ..StateKey::default()
        };
        assert_eq!(state_dir(&k2, d.path()), d.path().join("agents/demo-demox"));
    }

    #[test]
    fn s2_an_existing_doubled_state_dir_is_kept() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("agents/demo-demo-hermes")).unwrap();
        let k = StateKey {
            config_dir: Some(Path::new("/x/agents/demo-hermes")),
            initiative: Some("demo"),
            ..StateKey::default()
        };
        assert_eq!(
            state_dir(&k, d.path()),
            d.path().join("agents/demo-demo-hermes")
        );
    }

    #[test]
    fn s2_a_lock_dir_gives_its_parent_and_a_state_dir_wins() {
        let root = Path::new("/st/crewboard");
        let lock = state_dir(
            &StateKey {
                lock_dir: Some(Path::new("/y/wake.lock")),
                config_dir: Some(Path::new("/x/agents/dev1")),
                initiative: Some("demo"),
                ..StateKey::default()
            },
            root,
        );
        assert_eq!(lock, PathBuf::from("/y"));
        let rel = state_dir(
            &StateKey {
                lock_dir: Some(Path::new("wake.lock")),
                ..StateKey::default()
            },
            root,
        );
        assert_eq!(rel, PathBuf::from("."));
        let both = state_dir(
            &StateKey {
                state_dir: Some(Path::new("/s")),
                lock_dir: Some(Path::new("/y/wake.lock")),
                ..StateKey::default()
            },
            root,
        );
        assert_eq!(both, PathBuf::from("/s"));
    }

    #[test]
    fn the_root_follows_xdg_then_home() {
        assert_eq!(
            state_root(Some("/x"), Some("/h")),
            Some(PathBuf::from("/x/crewboard"))
        );
        assert_eq!(
            state_root(Some(""), Some("/h")),
            Some(PathBuf::from("/h/.local/state/crewboard"))
        );
        assert_eq!(
            state_root(None, Some("/h")),
            Some(PathBuf::from("/h/.local/state/crewboard"))
        );
        assert_eq!(state_root(None, None), None);
        assert_eq!(state_root(Some(""), Some("")), None);
    }

    #[test]
    fn the_key_replaces_every_byte_outside_the_set() {
        assert_eq!(safe_key("a.b_c-D9"), "a.b_c-D9");
        assert_eq!(safe_key("dev@host a/b"), "dev_host_a_b");
        // two bytes, two underscores, as `tr -c` does
        assert_eq!(safe_key("ä"), "__");
    }

    #[test]
    fn the_paths_sit_in_the_state_dir_unless_given() {
        let p = Paths::new(PathBuf::from("/s"), None, None);
        assert_eq!(p.lock, PathBuf::from("/s/wake.lock"));
        assert_eq!(p.log, PathBuf::from("/s/wake.log"));
        assert_eq!(p.quiet, PathBuf::from("/s/wake.log.quiet"));
        assert_eq!(p.net, PathBuf::from("/s/net-backoff"));
        let q = Paths::new(
            PathBuf::from("/s"),
            Some("/l/x.d".into()),
            Some("/t/p.log".into()),
        );
        assert_eq!(
            (q.lock, q.quiet),
            (PathBuf::from("/l/x.d"), PathBuf::from("/t/p.log.quiet"))
        );
    }

    #[test]
    fn the_budget_streak_is_a_count_and_a_ticket_or_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("budget-streak");
        assert_eq!(read_budget_streak(&f), None);
        std::fs::write(&f, "2 demo/app#12\n").unwrap();
        assert_eq!(read_budget_streak(&f), Some((2, "demo/app#12".into())));
        for junk in ["", "x demo/app#12\n", "0 demo/app#12\n", "3\n", "-1 a\n"] {
            std::fs::write(&f, junk).unwrap();
            assert_eq!(read_budget_streak(&f), None, "{junk:?}");
        }
    }

    #[test]
    fn last_wake_is_a_number_or_nothing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("last-wake");
        assert_eq!(read_last_wake(&f), None);
        std::fs::write(&f, "1700000000\n").unwrap();
        assert_eq!(read_last_wake(&f), Some(1_700_000_000));
        // digits only
        for junk in ["soon\n", "+5\n", "5x\n", " 5\n", "-5\n", "\n"] {
            std::fs::write(&f, junk).unwrap();
            assert_eq!(read_last_wake(&f), None, "{junk:?}");
        }
    }

    /// An stderr that a test can read.
    #[derive(Clone, Default)]
    struct Buf(Rc<RefCell<Vec<u8>>>);

    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn say_goes_to_stderr_and_the_log_and_line_only_to_the_log() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("wake.log");
        let clock = FakeClock::at(1_700_000_000);
        let err = Buf::default();
        let l = Log::with_err(log.clone(), &clock, Box::new(err.clone()));
        l.say("events pending");
        l.line("only the log");
        let file = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            file,
            "2023-11-14T22:13:20Z poll-and-wake: events pending\n2023-11-14T22:13:20Z poll-and-wake: only the log\n"
        );
        let stderr = String::from_utf8(err.0.borrow().clone()).unwrap();
        assert_eq!(
            stderr,
            "2023-11-14T22:13:20Z poll-and-wake: events pending\n"
        );
    }

    #[test]
    fn the_legacy_dir_is_only_for_the_default_per_agent_state() {
        let root = Path::new("/r");
        let key = |state: Option<&'static str>,
                   lock: Option<&'static str>,
                   cfg: Option<&'static str>,
                   agent: Option<&'static str>| StateKey {
            state_dir: state.map(Path::new),
            lock_dir: lock.map(Path::new),
            config_dir: cfg.map(Path::new),
            agent,
            initiative: Some("demo"),
        };
        assert_eq!(
            legacy_dir(&key(None, None, Some("/x/dev1"), None), root),
            Some(PathBuf::from("/r/demo"))
        );
        assert_eq!(
            legacy_dir(&key(None, None, None, Some("a@b")), root),
            Some(PathBuf::from("/r/demo"))
        );
        assert_eq!(
            legacy_dir(&key(Some("/s"), None, Some("/x"), None), root),
            None
        );
        assert_eq!(
            legacy_dir(&key(None, Some("/l/wake.lock"), Some("/x"), None), root),
            None
        );
        assert_eq!(
            legacy_dir(&key(None, None, None, None), root),
            None,
            "no agent key: the old default is the dir itself"
        );
    }
}
