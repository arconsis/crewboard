//! Is the lock stale? A pure function of a [`Snapshot`], the time and a process table. There is
//! no other way to take a lock over than one of these named reasons. The order of the rules and
//! every text follow spec/host-lock.

use super::board::BoardSays;
use super::owner;
use super::procs::{Alive, Procs};
use super::snapshot::{OwnerState, Snapshot};

/// The rules that do not depend on the lock itself.
#[derive(Debug, Clone)]
pub struct Rules {
    pub host: String,
    /// A lock older than this is stale. 0 = never.
    pub max_age: u64,
    /// An empty lock directory is stale only when older than this.
    pub empty_grace: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleReason {
    Overdue,
    PidDead,
    TooOld,
    BoardDead,
    BoardLeft,
    EmptyAfterGrace,
}

impl StaleReason {
    /// The words of the log line and of `status`.
    pub fn name(self) -> &'static str {
        match self {
            StaleReason::Overdue => "overdue",
            StaleReason::PidDead => "pid-dead",
            StaleReason::TooOld => "too-old",
            StaleReason::BoardDead => "board-dead",
            StaleReason::BoardLeft => "board-left",
            StaleReason::EmptyAfterGrace => "empty-after-grace",
        }
    }

    #[cfg(test)]
    pub const ALL: [StaleReason; 6] = [
        StaleReason::Overdue,
        StaleReason::PidDead,
        StaleReason::TooOld,
        StaleReason::BoardDead,
        StaleReason::BoardLeft,
        StaleReason::EmptyAfterGrace,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgement {
    /// No lock directory.
    Free,
    Held,
    Stale {
        reason: StaleReason,
        text: String,
    },
}

impl Judgement {
    /// `<name>: <text>`, the form that `status` prints.
    pub fn why(&self) -> Option<String> {
        match self {
            Judgement::Stale { reason, text } => Some(format!("{}: {text}", reason.name())),
            _ => None,
        }
    }
}

/// No control characters, 120 characters at most.
pub fn clean_text(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(120).collect()
}

/// A time option as signed seconds; a value above `i64::MAX` counts as the largest one.
pub fn secs(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn first_line_of(head: &str) -> String {
    let h = &head.as_bytes()[..head.len().min(400)];
    let text = String::from_utf8_lossy(h).to_string();
    clean_text(text.lines().next().unwrap_or(""))
}

pub fn alive_text(procs: &dyn Procs, pid: &str) -> bool {
    pid.parse::<u32>()
        .is_ok_and(|p| procs.alive(p) == Alive::Yes)
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `foreign holder: <file>: <first line>`; an invalid owner file:
/// `foreign holder: owner (<problem>): <first line>`.
pub fn foreign_desc(snap: &Snapshot) -> String {
    let Some(f) = snap.foreign.first() else {
        return String::new();
    };
    if f.name == "owner" && f.regular {
        return format!(
            "foreign holder: owner ({}): {}",
            owner::problem(snap.owner_text()),
            first_line_of(&f.head)
        );
    }
    if f.regular {
        return format!(
            "foreign holder: {}: {}",
            clean_text(&f.name),
            first_line_of(&f.head)
        );
    }
    format!(
        "foreign holder: {} (not a regular file)",
        clean_text(&f.name)
    )
}

/// Who holds the lock, in words.
pub fn describe_owner(snap: &Snapshot) -> String {
    if !snap.is_valid() {
        return if snap.foreign.is_empty() {
            "(empty lock directory, no owner file)".to_string()
        } else {
            foreign_desc(snap)
        };
    }
    let g = |k: &str| snap.owner_field(k).unwrap_or("");
    let mut s = format!(
        "{}@{} pid={} since {}",
        g("agent"),
        g("host"),
        g("pid"),
        g("started")
    );
    if !g("reason").is_empty() {
        s.push_str(&format!(" ({})", g("reason")));
    }
    if g("detached") == "1" {
        s.push_str(" [detached job]");
    }
    s
}

fn digits_field<'a>(snap: &'a Snapshot, key: &str) -> Option<&'a str> {
    snap.owner_field(key).filter(|v| all_digits(v))
}

/// A pid that nobody can judge: pid=0, no pid, or a pid of another host.
fn pid_unjudged(snap: &Snapshot, host: &str) -> bool {
    let p = snap.owner_field("pid").unwrap_or("");
    let h = snap.owner_field("host").unwrap_or("");
    p.is_empty() || p == "0" || h != host
}

fn num(s: &str) -> i64 {
    s.parse::<i64>().unwrap_or(i64::MAX / 4)
}

/// The notes after a held lock: why a holder cannot be proven dead, and how late it is.
pub fn held_notes(snap: &Snapshot, now: i64) -> String {
    if !snap.owner_present() {
        return String::new();
    }
    let mut out = String::new();
    if snap.owner_field("pid") == Some("0") {
        out = "pid=0: a dead holder cannot be detected".to_string();
    }
    if let (Some(ex), Some(ep)) = (
        digits_field(snap, "expect"),
        digits_field(snap, "started_epoch"),
    ) {
        let (ex_n, age) = (num(ex), now - num(ep));
        if age > ex_n {
            if !out.is_empty() {
                out.push_str("; ");
            }
            out.push_str(&format!(
                "overdue by {} min (expected {ex} s; stale after {} s)",
                (age - ex_n + 59) / 60,
                ex_n * 2 + 300
            ));
        }
    }
    if out.is_empty() {
        out
    } else {
        format!(" [{out}]")
    }
}

/// ` alive=yes|no|unknown age=<s>` for the status line of a held lock.
pub fn held_facts(snap: &Snapshot, host: &str, now: i64, procs: &dyn Procs) -> String {
    if !snap.is_valid() {
        return String::new();
    }
    let pid = snap.owner_field("pid").unwrap_or("");
    let mut alive = "unknown";
    if snap.owner_field("host") == Some(host) && all_digits(pid) && pid != "0" {
        alive = if alive_text(procs, pid) { "yes" } else { "no" };
    }
    let mut out = format!(" alive={alive}");
    if let Some(ep) = snap.owner_field("started_epoch") {
        out.push_str(&format!(" age={}", now - num(ep)));
    }
    out
}

/// The first pid that a foreign file records for THIS host: `(pid, file)`.
fn foreign_pid(snap: &Snapshot, host: &str) -> Option<(String, String)> {
    for f in &snap.foreign {
        if !f.regular {
            continue;
        }
        let line = |key: &str| -> String {
            owner::field(&f.head, key)
                .map(|v| v.chars().filter(|c| !c.is_whitespace()).collect())
                .unwrap_or_default()
        };
        let pid = line("pid");
        if !all_digits(&pid) || pid == "0" {
            continue;
        }
        let h = line("host");
        if h.is_empty() || h == host {
            return Some((pid, f.name.clone()));
        }
    }
    None
}

/// The minutes past `expect` (rounded up) when the lock is past `expect x 2 + 5 min` and its pid
/// cannot be judged.
fn overdue(snap: &Snapshot, host: &str, now: i64) -> Option<(String, i64)> {
    let ex = digits_field(snap, "expect")?;
    let ep = digits_field(snap, "started_epoch")?;
    if !pid_unjudged(snap, host) {
        return None;
    }
    let age = now - num(ep);
    (age > num(ex) * 2 + 300).then(|| (ex.to_string(), age))
}

pub fn judge(
    snap: &Snapshot,
    rules: &Rules,
    now: i64,
    procs: &dyn Procs,
    board: &mut dyn BoardSays,
) -> Judgement {
    use StaleReason::*;
    if !snap.exists {
        return Judgement::Free;
    }
    let stale = |reason: StaleReason, text: String| Judgement::Stale { reason, text };
    let inv = snap.is_invalid();
    let no_owner = matches!(snap.owner, OwnerState::Absent | OwnerState::Empty);
    if inv || (no_owner && !snap.foreign.is_empty()) {
        // A foreign holder (also an invalid owner file): never empty-after-grace, no
        // board, no overdue. Only a dead recorded pid or --max-age.
        if let Some((pid, file)) = foreign_pid(snap, &rules.host)
            && !alive_text(procs, &pid)
        {
            return stale(
                PidDead,
                format!("pid {pid} recorded in {} is gone", clean_text(&file)),
            );
        }
        let (dm, what) = if inv {
            (snap.owner_mtime, "the invalid owner file")
        } else {
            (snap.dir_mtime, "the foreign holder")
        };
        let Some(dm) = dm else {
            return Judgement::Held;
        };
        let age = now - dm;
        if rules.max_age > 0 && age > secs(rules.max_age) {
            return stale(
                TooOld,
                format!(
                    "{what} is {age} s old, over the limit of {} s",
                    rules.max_age
                ),
            );
        }
        return Judgement::Held;
    }
    if no_owner {
        // No owner file. A fresh directory is a taker between mkdir and the owner file.
        let Some(dm) = snap.dir_mtime else {
            return Judgement::Held;
        };
        let age = now - dm;
        if age > secs(rules.empty_grace) {
            return stale(
                EmptyAfterGrace,
                format!(
                    "the lock has no owner file and is {age} s old (grace {} s)",
                    rules.empty_grace
                ),
            );
        }
        return Judgement::Held;
    }
    let f = |k: &str| snap.owner_field(k).unwrap_or("");
    let (agent, host, pid) = (f("agent"), f("host"), f("pid"));
    if host == rules.host && !pid.is_empty() && pid != "0" && !alive_text(procs, pid) {
        return stale(PidDead, format!("owner pid {pid} is gone on host {host}"));
    }
    let epoch = digits_field(snap, "started_epoch");
    if rules.max_age > 0
        && let Some(ep) = epoch
    {
        let age = now - num(ep);
        if age > secs(rules.max_age) {
            return stale(
                TooOld,
                format!(
                    "the lock is {age} s old, over the limit of {} s",
                    rules.max_age
                ),
            );
        }
    }
    if let Some((ex, age)) = overdue(snap, &rules.host, now) {
        return stale(
            Overdue,
            format!(
                "the owner expected {ex} s, the pid cannot be judged, and the lock is {age} s old (limit expect x 2 + 5 min = {} s)",
                num(&ex) * 2 + 300
            ),
        );
    }
    if !agent.is_empty()
        && let Some(why) = board.says_gone(agent, now)
    {
        let reason = if why.starts_with("board-dead") {
            BoardDead
        } else {
            BoardLeft
        };
        let text = why.split_once(": ").map(|p| p.1.to_string()).unwrap_or(why);
        return stale(reason, text);
    }
    Judgement::Held
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::procs::FakeProcs;

    const NOW: i64 = 1_792_836_000;

    struct NoBoard;
    impl BoardSays for NoBoard {
        fn says_gone(&mut self, _a: &str, _now: i64) -> Option<String> {
            None
        }
    }
    struct Says(&'static str);
    impl BoardSays for Says {
        fn says_gone(&mut self, a: &str, _now: i64) -> Option<String> {
            Some(self.0.replace("{a}", a))
        }
    }

    fn rules() -> Rules {
        Rules {
            host: "hostA".into(),
            max_age: 14400,
            empty_grace: 30,
        }
    }

    /// A snapshot of a valid lock without touching the disk.
    fn valid(agent: &str, host: &str, pid: &str, epoch: i64, extra: &str) -> Snapshot {
        Snapshot {
            exists: true,
            owner: OwnerState::Valid(format!(
                "agent={agent}\nhost={host}\npid={pid}\nstarted=planted\nstarted_epoch={epoch}\nreason=planted\n{extra}"
            )),
            foreign: vec![],
            dir_mtime: Some(epoch),
            owner_mtime: Some(epoch),
        }
    }

    fn j(snap: &Snapshot, rules: &Rules, procs: &FakeProcs) -> Judgement {
        judge(snap, rules, NOW, procs, &mut NoBoard)
    }

    fn reason(j: &Judgement) -> Option<StaleReason> {
        match j {
            Judgement::Stale { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    #[test]
    fn h3_a_dead_pid_on_this_host_is_stale_a_live_one_is_held() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let dead = j(&valid("o@x", "hostA", "222", NOW, ""), &rules(), &p);
        assert_eq!(reason(&dead), Some(StaleReason::PidDead));
        assert_eq!(
            dead.why().unwrap(),
            "pid-dead: owner pid 222 is gone on host hostA"
        );
        assert_eq!(
            j(&valid("o@x", "hostA", "111", NOW, ""), &rules(), &p),
            Judgement::Held
        );
    }

    #[test]
    fn h4_too_old_is_stale_young_is_not() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let old = j(
            &valid("o@x", "hostA", "111", NOW - 100_000, ""),
            &rules(),
            &p,
        );
        assert_eq!(reason(&old), Some(StaleReason::TooOld));
        assert!(
            old.why()
                .unwrap()
                .contains("100000 s old, over the limit of 14400 s")
        );
    }

    #[test]
    fn h4b_a_young_lock_with_a_live_pid_is_held() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let mut r = rules();
        r.max_age = 3600;
        assert_eq!(
            j(&valid("o@x", "hostA", "111", NOW, ""), &r, &p),
            Judgement::Held
        );
        // the boundary: exactly the limit is still held, one second more is stale
        assert_eq!(
            j(&valid("o@x", "hostA", "111", NOW - 3600, ""), &r, &p),
            Judgement::Held
        );
        assert_eq!(
            reason(&j(&valid("o@x", "hostA", "111", NOW - 3601, ""), &r, &p)),
            Some(StaleReason::TooOld)
        );
    }

    #[test]
    fn h5_another_host_a_dead_looking_pid_is_not_judged() {
        let p = FakeProcs::new();
        assert_eq!(
            j(&valid("far@y", "hostB", "999", NOW, ""), &rules(), &p),
            Judgement::Held
        );
    }

    #[test]
    fn h5b_another_host_too_old_is_judged_by_age() {
        let p = FakeProcs::new();
        let mut r = rules();
        r.max_age = 3600;
        let v = j(&valid("far@y", "hostB", "999", NOW - 100_000, ""), &r, &p);
        assert_eq!(reason(&v), Some(StaleReason::TooOld));
    }

    #[test]
    fn e3_max_age_zero_never_expires_a_lock() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let mut r = rules();
        r.max_age = 0;
        assert_eq!(
            j(&valid("o@x", "hostA", "111", NOW - 9_999_999, ""), &r, &p),
            Judgement::Held
        );
    }

    #[test]
    fn e2_overdue_after_expect_times_two_plus_five_minutes() {
        let p = FakeProcs::new();
        let at = |age: i64| valid("a@x", "hostA", "0", NOW - age, "expect=1\n");
        // 302 s for expect=1: 290 s old is held, 310 s old is overdue
        assert_eq!(j(&at(290), &rules(), &p), Judgement::Held);
        let v = j(&at(310), &rules(), &p);
        assert_eq!(reason(&v), Some(StaleReason::Overdue));
        assert!(v.why().unwrap().starts_with("overdue: the owner expected 1 s, the pid cannot be judged, and the lock is 310 s old (limit expect x 2 + 5 min = 302 s)"));
    }

    #[test]
    fn e3_without_expect_pid0_keeps_the_age_rule_only() {
        let p = FakeProcs::new();
        assert_eq!(
            j(&valid("a@x", "hostA", "0", NOW - 2000, ""), &rules(), &p),
            Judgement::Held
        );
        let v = j(&valid("a@x", "hostA", "0", NOW - 20_000, ""), &rules(), &p);
        assert_eq!(reason(&v), Some(StaleReason::TooOld));
    }

    #[test]
    fn e3c_a_live_pid_with_expect_is_not_overdue() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let s = valid("a@x", "hostA", "111", NOW - 5000, "expect=1\n");
        assert_eq!(j(&s, &rules(), &p), Judgement::Held);
    }

    #[test]
    fn e3d_another_host_with_expect_is_overdue() {
        let p = FakeProcs::new();
        let s = valid("a@x", "hostB", "111", NOW - 5000, "expect=1\n");
        assert_eq!(reason(&j(&s, &rules(), &p)), Some(StaleReason::Overdue));
    }

    #[test]
    fn h8_the_board_says_dead_or_left_is_stale_and_the_texts_are_the_old_ones() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let s = valid("own@x", "hostA", "111", NOW, "");
        let dead = judge(
            &s,
            &rules(),
            NOW,
            &p,
            &mut Says("board-dead: the board says agent {a} is dead"),
        );
        assert_eq!(reason(&dead), Some(StaleReason::BoardDead));
        assert_eq!(
            dead.why().unwrap(),
            "board-dead: the board says agent own@x is dead"
        );
        let left = judge(
            &s,
            &rules(),
            NOW,
            &p,
            &mut Says("board-left: agent {a} has left the board"),
        );
        assert_eq!(reason(&left), Some(StaleReason::BoardLeft));
        // and a board that says nothing leaves it held
        assert_eq!(j(&s, &rules(), &p), Judgement::Held);
    }

    #[test]
    fn the_pid_rule_comes_before_the_board() {
        let p = FakeProcs::new();
        let s = valid("own@x", "hostA", "222", NOW, "");
        let v = judge(&s, &rules(), NOW, &p, &mut Says("board-dead: x"));
        assert_eq!(reason(&v), Some(StaleReason::PidDead));
    }

    fn empty_dir(mtime: i64) -> Snapshot {
        Snapshot {
            exists: true,
            owner: OwnerState::Absent,
            foreign: vec![],
            dir_mtime: Some(mtime),
            owner_mtime: None,
        }
    }

    #[test]
    fn h11_a_fresh_empty_dir_is_never_taken_an_old_one_is() {
        let p = FakeProcs::new();
        assert_eq!(j(&empty_dir(NOW - 5), &rules(), &p), Judgement::Held);
        let v = j(&empty_dir(NOW - 31), &rules(), &p);
        assert_eq!(reason(&v), Some(StaleReason::EmptyAfterGrace));
        assert_eq!(
            v.why().unwrap(),
            "empty-after-grace: the lock has no owner file and is 31 s old (grace 30 s)"
        );
        // exactly the grace is still fresh
        assert_eq!(j(&empty_dir(NOW - 30), &rules(), &p), Judgement::Held);
    }

    #[test]
    fn h12_every_reason_has_a_distinct_old_name() {
        let names: Vec<_> = StaleReason::ALL.iter().map(|r| r.name()).collect();
        assert_eq!(
            names,
            vec![
                "overdue",
                "pid-dead",
                "too-old",
                "board-dead",
                "board-left",
                "empty-after-grace"
            ]
        );
    }

    fn foreign(name: &str, head: &str, dir_mtime: i64) -> Snapshot {
        Snapshot {
            exists: true,
            owner: OwnerState::Absent,
            foreign: vec![crate::lock::snapshot::Foreign {
                name: name.into(),
                regular: true,
                head: head.into(),
            }],
            dir_mtime: Some(dir_mtime),
            owner_mtime: None,
        }
    }

    #[test]
    fn f1_a_foreign_file_is_held_never_empty_after_grace() {
        let p = FakeProcs::new();
        let s = foreign("info", "github-actions e2e-arm64 run 4711\n", NOW - 99_999);
        let mut r = rules();
        r.max_age = 0;
        assert_eq!(j(&s, &r, &p), Judgement::Held);
        assert_eq!(
            describe_owner(&s),
            "foreign holder: info: github-actions e2e-arm64 run 4711"
        );
    }

    #[test]
    fn f3_a_foreign_dir_older_than_max_age_is_too_old_zero_never() {
        let p = FakeProcs::new();
        let s = foreign("info", "x\n", NOW - 100);
        let mut r = rules();
        r.max_age = 1;
        assert_eq!(reason(&j(&s, &r, &p)), Some(StaleReason::TooOld));
        r.max_age = 0;
        assert_eq!(j(&s, &r, &p), Judgement::Held);
    }

    #[test]
    fn f4_the_text_has_the_first_line_without_control_characters_capped() {
        let long = format!("job \u{1b}[31mred\ttab{}\nsecond line\n", "x".repeat(300));
        let s = foreign("zz-info", &long, NOW);
        let d = describe_owner(&s);
        let first = d.strip_prefix("foreign holder: zz-info: ").unwrap();
        assert!(first.starts_with("job [31mredtabxxx"));
        assert_eq!(first.len(), 120);
        assert!(!d.contains("second line"));
        assert!(!d.bytes().any(|b| b < 0x20 || b == 0x7f));
    }

    #[test]
    fn f5_a_foreign_dir_pid_dead_live_and_another_host() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let dead = foreign("lockinfo", "tool x\npid=222\n", NOW);
        let v = j(&dead, &rules(), &p);
        assert_eq!(reason(&v), Some(StaleReason::PidDead));
        assert_eq!(
            v.why().unwrap(),
            "pid-dead: pid 222 recorded in lockinfo is gone"
        );
        // F5b a live pid blocks
        assert_eq!(
            j(&foreign("lockinfo", "tool x\npid=111\n", NOW), &rules(), &p),
            Judgement::Held
        );
        // F5c another host's pid is not judged; this host's is
        assert_eq!(
            j(
                &foreign("lockinfo", "tool x\nhost=hostB\npid=222\n", NOW),
                &rules(),
                &p
            ),
            Judgement::Held
        );
        assert_eq!(
            reason(&j(
                &foreign("lockinfo", "tool x\nhost=hostA\npid=222\n", NOW),
                &rules(),
                &p
            )),
            Some(StaleReason::PidDead)
        );
        // pid=0 means none
        assert_eq!(
            j(&foreign("lockinfo", "pid=0\n", NOW), &rules(), &p),
            Judgement::Held
        );
    }

    fn invalid_owner(content: &str, owner_mtime: i64, dir_mtime: i64) -> Snapshot {
        Snapshot {
            exists: true,
            owner: OwnerState::Invalid {
                content: Some(content.into()),
            },
            foreign: vec![crate::lock::snapshot::Foreign {
                name: "owner".into(),
                regular: true,
                head: content.into(),
            }],
            dir_mtime: Some(dir_mtime),
            owner_mtime: Some(owner_mtime),
        }
    }

    #[test]
    fn o1_an_invalid_owner_is_too_old_by_its_own_mtime_not_the_dirs() {
        let p = FakeProcs::new();
        let mut r = rules();
        r.max_age = 3600;
        let s = invalid_owner("agent-a@host-b\n", NOW - 100_000, NOW);
        let v = j(&s, &r, &p);
        assert_eq!(reason(&v), Some(StaleReason::TooOld));
        assert!(v.why().unwrap().starts_with(
            "too-old: the invalid owner file is 100000 s old, over the limit of 3600 s"
        ));
        assert_eq!(
            describe_owner(&s),
            "foreign holder: owner (no key=value fields): agent-a@host-b"
        );
        // --max-age 0 never; a fresh owner file in an old dir is held
        r.max_age = 0;
        assert_eq!(j(&s, &r, &p), Judgement::Held);
        r.max_age = 3600;
        assert_eq!(
            j(
                &invalid_owner("agent-a@host-b\n", NOW, NOW - 100_000),
                &r,
                &p
            ),
            Judgement::Held
        );
    }

    #[test]
    fn o3_an_invalid_owner_with_a_dead_pid_line_is_pid_dead() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let dead = invalid_owner("agent=a@x\nhost=hostA\npid=222\n", NOW, NOW);
        let v = j(&dead, &rules(), &p);
        assert_eq!(
            v.why().unwrap(),
            "pid-dead: pid 222 recorded in owner is gone"
        );
        assert_eq!(
            describe_owner(&dead),
            "foreign holder: owner (no numeric started_epoch=): agent=a@x"
        );
        // live pid, and another host's dead pid: held
        assert_eq!(
            j(
                &invalid_owner("agent=a@x\nhost=hostA\npid=111\n", NOW, NOW),
                &rules(),
                &p
            ),
            Judgement::Held
        );
        assert_eq!(
            j(
                &invalid_owner("agent=a@x\nhost=hostB\npid=222\n", NOW, NOW),
                &rules(),
                &p
            ),
            Judgement::Held
        );
    }

    #[test]
    fn o5_a_valid_owner_with_an_old_mtime_is_judged_by_started_epoch() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let mut s = valid("a@x", "hostA", "111", NOW, "");
        s.owner_mtime = Some(NOW - 100_000);
        s.dir_mtime = Some(NOW - 100_000);
        let mut r = rules();
        r.max_age = 3600;
        assert_eq!(j(&s, &r, &p), Judgement::Held);
    }

    #[test]
    fn o6_an_owner_that_is_a_directory_is_named_not_a_regular_file() {
        let s = Snapshot {
            exists: true,
            owner: OwnerState::Invalid { content: None },
            foreign: vec![crate::lock::snapshot::Foreign {
                name: "owner".into(),
                regular: false,
                head: String::new(),
            }],
            dir_mtime: Some(NOW),
            owner_mtime: Some(NOW),
        };
        assert_eq!(
            describe_owner(&s),
            "foreign holder: owner (not a regular file)"
        );
        assert_eq!(j(&s, &rules(), &FakeProcs::new()), Judgement::Held);
        let mut old = s.clone();
        old.owner_mtime = Some(NOW - 100_000);
        let mut r = rules();
        r.max_age = 3600;
        assert_eq!(
            reason(&j(&old, &r, &FakeProcs::new())),
            Some(StaleReason::TooOld)
        );
    }

    #[test]
    fn clean_text_cuts_on_characters_not_bytes() {
        let t = clean_text(&"\u{e4}".repeat(200));
        assert_eq!(t.chars().count(), 120);
        assert!(!t.contains('\u{fffd}'), "no cut inside a character");
        // control characters go, also in the middle
        assert_eq!(clean_text("a\u{1b}[0m\tb"), "a[0mb");
    }

    #[test]
    fn a_time_option_above_i64_max_is_the_largest_value_and_never_expires_a_lock() {
        assert_eq!(secs(u64::MAX), i64::MAX);
        assert_eq!(secs(60), 60);
        let p = FakeProcs::new().alive_pids(&[111]);
        let mut r = rules();
        r.max_age = u64::MAX;
        assert_eq!(
            j(&valid("o@x", "hostA", "111", NOW - 100_000, ""), &r, &p),
            Judgement::Held
        );
        // the other direction: a small limit still expires it
        r.max_age = 3600;
        assert_eq!(
            reason(&j(&valid("o@x", "hostA", "111", NOW - 100_000, ""), &r, &p)),
            Some(StaleReason::TooOld)
        );
        // the grace caps the same way: a huge grace keeps an old empty dir
        r.empty_grace = u64::MAX;
        assert_eq!(j(&empty_dir(NOW - 100_000), &r, &p), Judgement::Held);
    }

    #[test]
    fn a_missing_dir_is_free() {
        let s = Snapshot {
            exists: false,
            owner: OwnerState::Absent,
            foreign: vec![],
            dir_mtime: None,
            owner_mtime: None,
        };
        assert_eq!(j(&s, &rules(), &FakeProcs::new()), Judgement::Free);
    }

    #[test]
    fn e4_notes_say_pid0_and_how_late_the_job_is() {
        // 260 s past expect=300: 5 min rounded up
        let s = valid("a@x", "hostA", "0", NOW - 560, "expect=300\n");
        assert_eq!(
            held_notes(&s, NOW),
            " [pid=0: a dead holder cannot be detected; overdue by 5 min (expected 300 s; stale after 900 s)]"
        );
        // before expect only the pid=0 note
        let early = valid("a@x", "hostA", "0", NOW - 10, "expect=3600\n");
        assert_eq!(
            held_notes(&early, NOW),
            " [pid=0: a dead holder cannot be detected]"
        );
        // a real pid, no note
        assert_eq!(
            held_notes(&valid("a@x", "hostA", "111", NOW - 10, ""), NOW),
            ""
        );
    }

    #[test]
    fn h10b_held_facts_say_alive_and_age() {
        let p = FakeProcs::new().alive_pids(&[111]);
        let s = valid("agent-a@host-b", "hostA", "111", NOW - 7, "");
        assert_eq!(held_facts(&s, "hostA", NOW, &p), " alive=yes age=7");
        let dead = valid("agent-a@host-b", "hostA", "222", NOW - 7, "");
        assert_eq!(held_facts(&dead, "hostA", NOW, &p), " alive=no age=7");
        // another host or pid 0: unknown
        let far = valid("agent-a@host-b", "hostB", "222", NOW - 7, "");
        assert_eq!(held_facts(&far, "hostA", NOW, &p), " alive=unknown age=7");
    }

    #[test]
    fn describe_owner_names_agent_host_pid_reason_and_a_detached_job() {
        let s = valid("a@x", "hostA", "111", NOW, "detached=1\n");
        assert_eq!(
            describe_owner(&s),
            "a@x@hostA pid=111 since planted (planted) [detached job]"
        );
        let empty = empty_dir(NOW);
        assert_eq!(
            describe_owner(&empty),
            "(empty lock directory, no owner file)"
        );
    }
}
