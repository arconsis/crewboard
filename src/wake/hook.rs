//! The events hook of a Claude wake (`crewboard hook post-tool-use`). Claude Code runs it after
//! each tool call. Most calls end at the stamp check: the hook polls at most once per `every`
//! seconds. A poll is the call that `wake` makes, so the cursor moves in the same way. With
//! events, the hook appends them to the events file of the wake (the end-of-wake check reads
//! them there) and returns them as the hook JSON that the model reads. Everything else gives no
//! output. The hook never blocks a tool call: it always ends with exit 0, and a failure goes to
//! the log.
//!
//! The stamp is `<events file>.hook-stamp` (epoch seconds). Without a stamp the age of the events
//! file counts: `wake` wrote it with its own poll, so the first tool calls do not poll again.

use super::steps::Steps;
use crate::clock::{Clock, format_ts};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Seconds between two polls when neither `--every` nor `$CREWBOARD_HOOK_EVERY` says.
pub const DEFAULT_EVERY: i64 = 300;
/// Claude Code caps the hook text at 10000 characters; the events file has the rest.
pub const CUT_AT: usize = 9000;
const HEAD: &str = "crewboard: new events since this wake started:\n";

/// What the hook is given.
pub struct Input<'a> {
    /// `$CREWBOARD_EVENTS_FILE`; none or empty: the hook is not in a wake and does nothing.
    pub events_file: Option<&'a Path>,
    /// `--every`, else `$CREWBOARD_HOOK_EVERY`; anything but digits means the default.
    pub every: Option<&'a str>,
    pub log: Option<&'a Path>,
}

/// The seconds of `--every`; a value that is not a whole number gives the default.
pub fn every_secs(v: Option<&str>) -> i64 {
    v.and_then(|s| s.trim().parse::<u32>().ok())
        .map_or(DEFAULT_EVERY, i64::from)
}

/// The path of the stamp file.
pub fn stamp_path(events: &Path) -> PathBuf {
    let mut s = events.as_os_str().to_owned();
    s.push(".hook-stamp");
    PathBuf::from(s)
}

/// The time of the last poll: the stamp, else the mtime of the events file, else 0.
fn last_poll(events: &Path) -> i64 {
    let stamp = stamp_path(events);
    let from_stamp = match std::fs::read_to_string(&stamp) {
        Ok(t) => Some(t.split_whitespace().next().and_then(|w| w.parse().ok())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(None),
    };
    match from_stamp {
        Some(v) => v.unwrap_or(0),
        None => std::fs::metadata(events)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|d| i64::try_from(d.as_secs()).ok())
            .unwrap_or(0),
    }
}

/// The text for the model: the events, without the control characters that a JSON string or the
/// model has no use for (all below blank except tab, line feed and carriage return), cut at
/// [`CUT_AT`] characters.
pub fn context(events: &str, events_file: &Path) -> String {
    let all: String = format!("{HEAD}{events}")
        .chars()
        .filter(|c| !matches!(*c as u32, 0..=8 | 11 | 12 | 14..=31))
        .collect();
    if all.chars().count() <= CUT_AT {
        return all;
    }
    let mut cut: String = all.chars().take(CUT_AT).collect();
    cut.push_str(&format!(
        "\n(cut: the full text is in {})",
        events_file.display()
    ));
    cut
}

/// The hook JSON for the text.
pub fn hook_json(ctx: &str) -> String {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": ctx,
        }
    })
    .to_string()
}

fn log_line(input: &Input, clock: &dyn Clock, msg: &str) {
    let Some(path) = input.log else {
        return;
    };
    let line = format!("{} wake-events-hook: {msg}\n", format_ts(clock.now()));
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = r {
        // the hook has no other place to say it; Claude Code shows standard error of exit 0
        // only in its debug output
        eprintln!("wake-events-hook: cannot write {}: {e}", path.display());
    }
}

/// One run of the hook. Returns the text for standard output (the hook JSON), if there is one.
/// The exit code is always 0, so the caller has none to take from here.
pub fn run(input: &Input, steps: &mut dyn Steps, clock: &dyn Clock) -> Option<String> {
    let events = input.events_file.filter(|p| !p.as_os_str().is_empty())?;
    let now = clock.now();
    // a stamp in the future (a clock that went back) counts as due; a hostile number must not
    // overflow the difference
    let last = last_poll(events);
    if last <= now && now.saturating_sub(last) < every_secs(input.every) {
        return None;
    }
    // the stamp first: a slow or failed poll does not make the next tool call poll again
    if let Err(e) =
        crate::wake::state::write_atomic(&stamp_path(events), format!("{now}\n").as_bytes())
    {
        log_line(input, clock, &format!("cannot write the stamp: {e}"));
    }
    let mut out = Vec::new();
    let rc = steps.poll(false, &mut out);
    match rc.code {
        0 => {}
        1 => return None,
        n => {
            log_line(
                input,
                clock,
                &format!("crewboard poll failed with exit {n}; no events delivered"),
            );
            return None;
        }
    }
    let text = String::from_utf8_lossy(&out);
    let text = text.trim_end_matches('\n');
    if text.is_empty() {
        return None;
    }
    // the events file first: the end-of-wake check reads it
    let block = format!(
        "\n== Events delivered during the wake (crewboard poll, {}):\n{text}\n",
        format_ts(now)
    );
    let r = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events)
        .and_then(|mut f| f.write_all(block.as_bytes()));
    if let Err(e) = r {
        log_line(
            input,
            clock,
            &format!("cannot append the events to {}: {e}", events.display()),
        );
    }
    log_line(
        input,
        clock,
        &format!(
            "delivered {} line(s) of events to the wake",
            text.lines().count()
        ),
    );
    Some(hook_json(&context(text, events)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::FakeClock;
    use crate::wake::steps::StepRc;

    /// A board that answers each `poll` with the next scripted result; other calls do not happen.
    struct Board {
        plan: Vec<(u8, &'static str)>,
        polls: usize,
    }

    impl Board {
        fn new(plan: &[(u8, &'static str)]) -> Board {
            Board {
                plan: plan.to_vec(),
                polls: 0,
            }
        }
    }

    impl Steps for Board {
        fn poll(&mut self, quiet: bool, out: &mut Vec<u8>) -> StepRc {
            assert!(!quiet, "the hook polls so that the cursor moves");
            let (code, text) = self.plan.get(self.polls).copied().unwrap_or((1, ""));
            self.polls += 1;
            out.extend_from_slice(text.as_bytes());
            StepRc {
                code,
                msg: String::new(),
            }
        }
        fn mine_quiet(&mut self) -> StepRc {
            unreachable!()
        }
        fn mine_text(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn say(&mut self, _: &str, _: &str, _: bool) -> StepRc {
            unreachable!()
        }
        fn tick(&mut self, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_list(&mut self, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn acted_check(&mut self, _: &Path, _: &str, _: &Path, _: &mut Vec<u8>) -> StepRc {
            unreachable!()
        }
        fn heartbeat(&mut self) -> StepRc {
            unreachable!()
        }
        fn pr_head(&mut self, _: &str, _: &str) -> Result<Option<(String, String)>, StepRc> {
            unreachable!()
        }
        fn open_prs(&mut self, _: &str) -> Result<Vec<(String, String, String)>, StepRc> {
            unreachable!()
        }
    }

    const NOW: i64 = 1_800_000_000;

    struct Rig {
        dir: tempfile::TempDir,
        clock: FakeClock,
    }

    impl Rig {
        fn new() -> Rig {
            let rig = Rig {
                dir: tempfile::tempdir().unwrap(),
                clock: FakeClock::at(NOW),
            };
            std::fs::write(rig.events(), "== Events:\n").unwrap();
            rig
        }
        fn events(&self) -> PathBuf {
            self.dir.path().join("events.txt")
        }
        fn log_path(&self) -> PathBuf {
            self.dir.path().join("hook.log")
        }
        fn stamp(&self, at: i64) {
            std::fs::write(stamp_path(&self.events()), format!("{at}\n")).unwrap();
        }
        fn run(&self, board: &mut Board, every: Option<&str>) -> Option<String> {
            let ev = self.events();
            let log = self.log_path();
            run(
                &Input {
                    events_file: Some(&ev),
                    every,
                    log: Some(&log),
                },
                board,
                &self.clock,
            )
        }
        fn log(&self) -> String {
            std::fs::read_to_string(self.log_path()).unwrap_or_default()
        }
        fn events_text(&self) -> String {
            std::fs::read_to_string(self.events()).unwrap()
        }
    }

    #[test]
    fn hk1_not_due_no_poll_no_output_and_an_unchanged_file() {
        let rig = Rig::new();
        rig.stamp(NOW - 10);
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert_eq!(rig.run(&mut b, Some("300")), None);
        assert_eq!(b.polls, 0);
        assert_eq!(rig.events_text(), "== Events:\n");
        // twin: the same stamp, 300 s later, is due
        rig.clock.advance(290);
        assert!(rig.run(&mut b, Some("300")).is_some());
        assert_eq!(b.polls, 1);
    }

    #[test]
    fn hk1b_without_a_stamp_the_age_of_the_events_file_counts() {
        let rig = Rig::new();
        // the file was written just now (the poll of the wake): not due, and no stamp is made
        let mtime = std::fs::metadata(rig.events())
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let rig = Rig {
            clock: FakeClock::at(mtime + 5),
            ..rig
        };
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert_eq!(rig.run(&mut b, Some("300")), None);
        assert_eq!(b.polls, 0);
        assert!(!stamp_path(&rig.events()).exists());
        // twin: a file that is older than the interval is due
        rig.clock.advance(300);
        assert!(rig.run(&mut b, Some("300")).is_some());
    }

    #[test]
    fn hk2_due_with_events_json_events_file_log_and_stamp() {
        let rig = Rig::new();
        rig.stamp(0);
        let mut b = Board::new(&[(0, "EVENT 1: owner merged #12 \"now\"\nEVENT 2: a\tb\n")]);
        let out = rig.run(&mut b, Some("300")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let ctx = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert_eq!(
            ctx,
            "crewboard: new events since this wake started:\nEVENT 1: owner merged #12 \"now\"\nEVENT 2: a\tb"
        );
        let t = rig.events_text();
        assert!(
            t.starts_with("== Events:\n\n== Events delivered during the wake (crewboard poll, 20"),
            "{t}"
        );
        assert!(
            t.ends_with("):\nEVENT 1: owner merged #12 \"now\"\nEVENT 2: a\tb\n"),
            "{t}"
        );
        assert!(
            rig.log()
                .contains("wake-events-hook: delivered 2 line(s) of events to the wake\n")
        );
        assert_eq!(
            std::fs::read_to_string(stamp_path(&rig.events())).unwrap(),
            format!("{NOW}\n")
        );
        // right after, not due again
        assert_eq!(rig.run(&mut b, Some("300")), None);
        assert_eq!(b.polls, 1);
    }

    #[test]
    fn hk3_due_but_no_events_gives_no_output_and_no_change() {
        let rig = Rig::new();
        let mut b = Board::new(&[(1, "")]);
        assert_eq!(rig.run(&mut b, Some("0")), None);
        assert_eq!(b.polls, 1);
        assert_eq!(rig.events_text(), "== Events:\n");
        assert_eq!(rig.log(), "");
        // exit 0 with an empty text is no event either
        let mut b = Board::new(&[(0, "\n\n")]);
        assert_eq!(rig.run(&mut b, Some("0")), None);
        assert_eq!(rig.events_text(), "== Events:\n");
    }

    #[test]
    fn hk4_a_failed_poll_is_one_log_line_and_no_output() {
        for code in [3u8, 4, 7, 127] {
            let rig = Rig::new();
            let mut b = Board::new(&[(code, "")]);
            assert_eq!(rig.run(&mut b, Some("0")), None);
            assert_eq!(rig.events_text(), "== Events:\n");
            let log = rig.log();
            assert_eq!(log.lines().count(), 1, "{log}");
            assert!(
                log.contains(&format!(
                    "wake-events-hook: crewboard poll failed with exit {code}; no events delivered"
                )),
                "{log}"
            );
            assert!(stamp_path(&rig.events()).exists(), "the stamp comes first");
        }
    }

    #[test]
    fn hk5_outside_a_wake_there_is_nothing() {
        let rig = Rig::new();
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        let log = rig.log_path();
        for none in [None, Some(Path::new(""))] {
            let r = run(
                &Input {
                    events_file: none,
                    every: Some("0"),
                    log: Some(&log),
                },
                &mut b,
                &rig.clock,
            );
            assert_eq!(r, None);
        }
        assert_eq!(b.polls, 0);
        assert!(!log.exists());
    }

    #[test]
    fn a_hostile_or_future_stamp_is_due_and_does_not_overflow() {
        for stamp in ["-9223372036854775808", "9223372036854775807", "-1"] {
            let rig = Rig::new();
            std::fs::write(stamp_path(&rig.events()), format!("{stamp}\n")).unwrap();
            let mut b = Board::new(&[(0, "EVENT 1\n")]);
            assert!(rig.run(&mut b, Some("300")).is_some(), "{stamp}");
            assert_eq!(b.polls, 1, "{stamp}");
        }
        // a stamp 10 s in the future with a long interval is due too (the clock went back)
        let rig = Rig::new();
        rig.stamp(NOW + 10);
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert!(rig.run(&mut b, Some("300")).is_some());
        // twin: 10 s in the past is not due
        let rig = Rig::new();
        rig.stamp(NOW - 10);
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert_eq!(rig.run(&mut b, Some("300")), None);
    }

    #[test]
    fn hk6_the_interval_is_a_number_else_the_default() {
        assert_eq!(every_secs(Some("0")), 0);
        assert_eq!(every_secs(Some("45")), 45);
        assert_eq!(every_secs(None), DEFAULT_EVERY);
        for bad in ["", "abc", "-5", "1.5", "99999999999999"] {
            assert_eq!(every_secs(Some(bad)), DEFAULT_EVERY, "{bad:?}");
        }
        // `0` makes the hook due at once, also right after a stamp
        let rig = Rig::new();
        rig.stamp(NOW);
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert!(rig.run(&mut b, Some("0")).is_some());
        // and a non-number means 300: not due 10 s after the stamp
        let mut b = Board::new(&[(0, "EVENT 2\n")]);
        rig.clock.advance(10);
        assert_eq!(rig.run(&mut b, Some("soon")), None);
        assert_eq!(b.polls, 0);
    }

    #[test]
    fn json_escaping_roundtrip() {
        let text = "q \"x\" back\\slash\ttab\nline\u{1F600} \u{0001}ctl é";
        let json = hook_json(&context(text, Path::new("/e")));
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["additionalContext"],
            format!("{HEAD}q \"x\" back\\slash\ttab\nline\u{1F600} ctl é")
        );
        assert!(!json.contains('\n'), "one line");
        // the removed set: 0-8, 11, 12, 14-31; kept: tab, line feed, carriage return, and 127
        let all: String = (0u8..=127).map(char::from).collect();
        let kept = context(&all, Path::new("/e"));
        let kept = &kept[HEAD.len()..];
        let want: String = (0u8..=127)
            .filter(|b| !matches!(b, 0..=8 | 11 | 12 | 14..=31))
            .map(char::from)
            .collect();
        assert_eq!(kept, want);
    }

    #[test]
    fn cut_counts_characters_not_bytes() {
        let room = CUT_AT - HEAD.chars().count();
        // exactly at the limit, 4 bytes per character: no cut
        let fits = "\u{1F600}".repeat(room);
        let c = context(&fits, Path::new("/e"));
        assert_eq!(c.chars().count(), CUT_AT);
        assert!(!c.contains("(cut:"));
        // one character more: cut, and the note names the file
        let over = "\u{1F600}".repeat(room + 1);
        let c = context(&over, Path::new("/e/ev.txt"));
        assert_eq!(
            c,
            format!(
                "{HEAD}{}\n(cut: the full text is in /e/ev.txt)",
                "\u{1F600}".repeat(room)
            )
        );
    }

    #[test]
    fn a_stamp_that_cannot_be_written_is_logged_and_the_poll_still_runs() {
        let rig = Rig::new();
        std::fs::create_dir(stamp_path(&rig.events())).unwrap();
        let mut b = Board::new(&[(0, "EVENT 1\n")]);
        assert!(rig.run(&mut b, Some("0")).is_some());
        assert!(
            rig.log().contains("cannot write the stamp"),
            "{}",
            rig.log()
        );
    }
}
